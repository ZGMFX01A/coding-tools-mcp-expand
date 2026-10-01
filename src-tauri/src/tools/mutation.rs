//! Experimental command write boundary. Structured tools remain revision checked.
use super::{
    context::ToolContext,
    workspace::{Workspace, WorkspaceError},
};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct MutationPolicy {
    pub mode: String,
    pub write_paths: Vec<PathBuf>,
    error: Option<String>,
}
impl MutationPolicy {
    pub fn from_env(workspace: &Workspace) -> Self {
        let mode = std::env::var("CODING_TOOLS_MCP_WORKSPACE_MUTATION")
            .unwrap_or_else(|_| "unrestricted".into())
            .trim()
            .to_lowercase();
        let mut error = if ["unrestricted", "structured-only"].contains(&mode.as_str()) {
            None
        } else {
            Some("workspace mutation must be unrestricted or structured-only".into())
        };
        let mut write_paths = Vec::new();
        if let Some(value) = std::env::var_os("CODING_TOOLS_MCP_WRITE_PATHS") {
            if mode != "structured-only" && !value.is_empty() {
                error = Some("write paths require structured-only mode".into());
            }
            for path in std::env::split_paths(&value).filter(|p| !p.as_os_str().is_empty()) {
                let raw = path.to_string_lossy();
                match workspace.resolve_for_write(&raw) {
                    Ok(resolved)
                        if resolved.path != workspace.root()
                            && (!resolved.existed || resolved.path.is_dir()) =>
                    {
                        if !write_paths.contains(&resolved.path) {
                            write_paths.push(resolved.path);
                        }
                    }
                    _ => {
                        error = Some(
                            "Write paths must name directories strictly inside the workspace"
                                .into(),
                        )
                    }
                }
            }
        }
        Self {
            mode,
            write_paths,
            error,
        }
    }
    pub fn payload(&self, permission: &str) -> Value {
        let available = landlock_abi();
        let enforced = self.mode == "structured-only"
            && self.error.is_none()
            && available >= 3
            && permission != "dangerous";
        json!({"mode":self.mode,"write_paths":self.write_paths,"enforced":enforced,
            "enforced_by":if enforced{"landlock"}else{"none"},"landlock_abi":available,
            "structured_write_tools":["apply_patch","apply_changes"],
            "warnings":if self.mode=="structured-only" && !enforced {vec!["structured-only requires enabled Linux Landlock ABI 3 or newer; subprocess writes are not restricted on this platform"]}else{vec![]},
            "configuration_error":self.error})
    }
    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if let Some(error) = &self.error {
            return Err(WorkspaceError::invalid_argument(error.clone()));
        }
        Ok(())
    }
}
pub fn landlock_abi() -> i32 {
    #[cfg(target_os = "linux")]
    {
        unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<u8>(),
                0,
                1,
            )
            .max(0) as i32
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

pub struct CommandTempDir {
    path: PathBuf,
}
impl CommandTempDir {
    pub fn create() -> std::io::Result<Self> {
        let base = std::env::temp_dir().canonicalize()?;
        let path = base.join(format!(
            "coding-tools-command-{}",
            uuid::Uuid::new_v4().simple()
        ));
        #[allow(unused_mut)] // DirBuilderExt::mode needs a mutable builder on Unix.
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self { path })
    }
}
impl Drop for CommandTempDir {
    fn drop(&mut self) {
        // This absolute, UUID-named directory was created exclusively by this guard.
        // Rust's remove_dir_all does not traverse symlinks inside the directory.
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
pub struct CommandSandbox {
    pub enforced: bool,
    pub temporary: Option<CommandTempDir>,
}

pub fn configure(
    ctx: &ToolContext,
    command: &mut tokio::process::Command,
) -> Result<CommandSandbox, WorkspaceError> {
    ctx.mutation_policy.validate()?;
    if ctx.mutation_policy.mode != "structured-only"
        || ctx.policy.skip_permission_gates()
        || landlock_abi() < 3
    {
        return Ok(CommandSandbox {
            enforced: false,
            temporary: None,
        });
    }
    #[cfg(target_os = "linux")]
    {
        let temporary = linux::configure(ctx, command)?;
        Ok(CommandSandbox {
            enforced: true,
            temporary: Some(temporary),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = command;
        Ok(CommandSandbox {
            enforced: false,
            temporary: None,
        })
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        fs::File,
        os::fd::{AsRawFd, FromRawFd},
        os::unix::fs::OpenOptionsExt,
    };
    const RIGHTS: u64 = (1 << 1)
        | (1 << 4)
        | (1 << 5)
        | (1 << 6)
        | (1 << 7)
        | (1 << 8)
        | (1 << 9)
        | (1 << 10)
        | (1 << 11)
        | (1 << 12)
        | (1 << 13)
        | (1 << 14);
    #[repr(C)]
    struct Ruleset {
        handled_access_fs: u64,
    }
    #[repr(C, packed)]
    struct Beneath {
        allowed_access: u64,
        parent_fd: i32,
    }
    fn fail() -> WorkspaceError {
        WorkspaceError::Tool {
            code: "SANDBOX_UNAVAILABLE",
            message: "Could not establish the Linux command write boundary".into(),
            category: "security",
            retryable: false,
        }
    }
    pub(super) fn configure(
        ctx: &ToolContext,
        command: &mut tokio::process::Command,
    ) -> Result<CommandTempDir, WorkspaceError> {
        let attr = Ruleset {
            handled_access_fs: RIGHTS,
        };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr,
                std::mem::size_of::<Ruleset>(),
                0,
            )
        };
        if fd < 0 {
            return Err(fail());
        }
        let rules = unsafe { File::from_raw_fd(fd as i32) };
        let temporary = CommandTempDir::create().map_err(|_| fail())?;
        let tmp = temporary.path.clone();
        let mut roots = ctx.mutation_policy.write_paths.clone();
        roots.push(tmp.clone());
        for root in roots {
            if root != tmp {
                let display =
                    super::super::workspace::relative_display(ctx.workspace.root(), &root);
                let resolved = ctx.workspace.resolve_for_write(&display)?;
                if resolved.path != root {
                    return Err(fail());
                }
                std::fs::create_dir_all(&root).map_err(|_| fail())?;
                if ctx.workspace.resolve_existing(&display)?.path != root {
                    return Err(fail());
                }
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&root)
                .map_err(|_| fail())?;
            let rule = Beneath {
                allowed_access: RIGHTS,
                parent_fd: file.as_raw_fd(),
            };
            if unsafe { libc::syscall(libc::SYS_landlock_add_rule, rules.as_raw_fd(), 1, &rule, 0) }
                < 0
            {
                return Err(fail());
            }
        }
        // Device output must remain writable while all regular files outside the allowlist are denied.
        for device in ["/dev/null", "/dev/tty"] {
            if let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_CLOEXEC)
                .open(device)
            {
                let rule = Beneath {
                    allowed_access: 1 << 1,
                    parent_fd: file.as_raw_fd(),
                };
                if unsafe {
                    libc::syscall(libc::SYS_landlock_add_rule, rules.as_raw_fd(), 1, &rule, 0)
                } < 0
                {
                    return Err(fail());
                }
            }
        }
        command
            .env("TMPDIR", &tmp)
            .env("TMP", &tmp)
            .env("TEMP", &tmp);
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::syscall(libc::SYS_landlock_restrict_self, rules.as_raw_fd(), 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(temporary)
    }
}

#[cfg(test)]
mod temporary_tests {
    use super::*;
    #[test]
    fn owned_command_temp_is_removed_on_scope_failure_or_completion() {
        let directory = CommandTempDir::create().unwrap();
        let path = directory.path.clone();
        std::fs::create_dir(path.join("nested")).unwrap();
        std::fs::write(path.join("nested/result"), "temporary data").unwrap();
        assert!(path.exists());
        drop(directory);
        assert!(!path.exists());
    }
}
