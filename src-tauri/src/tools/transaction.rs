//! Optimistic, staged multi-file commit. Validation completes before any target changes.
use super::{
    changes::read_optional,
    workspace::{Workspace, WorkspaceError},
};
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf};
use uuid::Uuid;

pub(super) fn commit(
    ws: &Workspace,
    staged: &HashMap<String, Option<Vec<u8>>>,
    baseline: &HashMap<String, Option<Vec<u8>>>,
    permissions: &HashMap<String, fs::Permissions>,
) -> Result<HashMap<PathBuf, Option<Vec<u8>>>, WorkspaceError> {
    commit_impl(ws, staged, baseline, permissions, |_, _| Ok(()))
}

fn commit_impl(
    ws: &Workspace,
    staged: &HashMap<String, Option<Vec<u8>>>,
    baseline: &HashMap<String, Option<Vec<u8>>>,
    permissions: &HashMap<String, fs::Permissions>,
    mut checkpoint: impl FnMut(&str, &str) -> std::io::Result<()>,
) -> Result<HashMap<PathBuf, Option<Vec<u8>>>, WorkspaceError> {
    let conflict = |path: &str| WorkspaceError::ToolDetails {
        code: "PATCH_CONFLICT",
        message: "File changed while the request was being prepared; read it again".into(),
        category: "validation",
        retryable: true,
        details: json!({"path":path}),
    };
    let mut paths = staged
        .keys()
        .filter(|path| baseline.get(*path) != staged.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    let mut originals = HashMap::new();
    let mut resolved_paths = HashMap::new();
    for (path, expected) in baseline {
        ws.reject_protected_write_path(path)?;
        ws.reject_write_symlink(path)?;
        let resolved = ws.resolve_for_write(path)?;
        if &read_optional(&resolved.path)? != expected {
            return Err(conflict(path));
        }
        originals.insert(resolved.path.clone(), expected.clone());
        resolved_paths.insert(path.clone(), resolved.path);
    }
    if paths.iter().any(|path| !baseline.contains_key(path)) {
        return Err(WorkspaceError::invalid_argument(
            "Transaction is missing a file baseline",
        ));
    }
    let mut prepared = HashMap::new();
    let mut recovery = HashMap::new();
    let mut created_dirs = Vec::new();
    let mut touched = Vec::new();
    let mut installed = std::collections::HashSet::new();
    let result = (|| -> Result<(), WorkspaceError> {
        for path in &paths {
            if let Some(bytes) = &staged[path] {
                let target = &resolved_paths[path];
                let mut missing = Vec::new();
                let mut parent = target.parent();
                while let Some(dir) = parent {
                    if dir.exists() {
                        break;
                    }
                    missing.push(dir.to_path_buf());
                    parent = dir.parent();
                }
                for dir in missing.into_iter().rev() {
                    fs::create_dir(&dir).map_err(io_error)?;
                    created_dirs.push(dir);
                }
                let temp = target
                    .with_file_name(format!(".coding-tools-stage-{}", Uuid::new_v4().simple()));
                prepared.insert(path.clone(), temp.clone());
                use std::io::Write;
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temp).map_err(io_error)?;
                file.write_all(bytes).map_err(io_error)?;
                file.sync_all().map_err(io_error)?;
                if let Ok(metadata) = fs::metadata(target) {
                    fs::set_permissions(&temp, metadata.permissions()).map_err(io_error)?;
                } else if let Some(mode) = permissions.get(path) {
                    fs::set_permissions(&temp, mode.clone()).map_err(io_error)?;
                }
            }
        }
        for (path, expected) in baseline {
            ws.reject_write_symlink(path)?;
            let resolved = ws.resolve_for_write(path)?;
            if resolved.path != resolved_paths[path] || &read_optional(&resolved.path)? != expected
            {
                return Err(conflict(path));
            }
        }
        for path in &paths {
            ws.reject_write_symlink(path)?;
            let resolved = ws.resolve_for_write(path)?;
            if resolved.path != resolved_paths[path]
                || read_optional(&resolved.path)? != baseline[path]
            {
                return Err(conflict(path));
            }
            let target = &resolved.path;
            if baseline[path].is_some() {
                // Preserve Windows read-only target semantics before moving its original.
                #[cfg(windows)]
                if fs::metadata(target)
                    .map_err(io_error)?
                    .permissions()
                    .readonly()
                {
                    return Err(io_error(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Target is read-only",
                    )));
                }
                let backup = target
                    .with_file_name(format!(".coding-tools-backup-{}", Uuid::new_v4().simple()));
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                drop(options.open(&backup).map_err(io_error)?);
                if let Err(error) = super::patch::replace_file(target, &backup) {
                    cleanup(&backup);
                    return Err(io_error(error));
                }
                recovery.insert(path.clone(), backup);
                touched.push(path.clone());
                sync_parent(target);
            }
            checkpoint("install", path).map_err(io_error)?;
            if let Some(temp) = prepared.get(path) {
                // The destination must still be absent after its original was reserved.
                if read_optional(target)?.is_some() {
                    return Err(conflict(path));
                }
                super::patch::replace_file(temp, target).map_err(io_error)?;
                installed.insert(path.clone());
                if baseline[path].is_none() {
                    touched.push(path.clone());
                }
                sync_parent(target);
            }
        }
        Ok(())
    })();
    if let Err(cause) = result {
        let mut rollback_errors = Vec::new();
        for path in touched.iter().rev() {
            let target = &resolved_paths[path];
            let restored = checkpoint("rollback", path).and_then(|_| {
                if let Some(backup) = recovery.get(path) {
                    super::patch::replace_file(backup, target)
                } else if installed.contains(path) {
                    // This newly installed file has no original to preserve.
                    #[cfg(windows)]
                    if let Ok(meta) = fs::metadata(target) {
                        let mut mode = meta.permissions();
                        if mode.readonly() {
                            mode.set_readonly(false);
                            fs::set_permissions(target, mode)?;
                        }
                    }
                    fs::remove_file(target)
                } else {
                    Ok(())
                }
            });
            if let Err(error) = restored {
                rollback_errors.push(json!({"path":path,"error":error.to_string()}));
            } else {
                sync_parent(target);
            }
        }
        for temp in prepared.values() {
            cleanup(temp);
        }
        for dir in created_dirs.iter().rev() {
            let _ = fs::remove_dir(dir);
        }
        if !rollback_errors.is_empty() {
            let preserved = recovery
                .iter()
                .filter(|(_, backup)| backup.exists())
                .map(|(path, backup)| {
                    (
                        path.clone(),
                        super::workspace::relative_display(ws.root(), backup),
                    )
                })
                .collect::<HashMap<_, _>>();
            return Err(WorkspaceError::ToolDetails {
                code:"PATCH_ROLLBACK_FAILED", message:"The transaction failed and recovery was incomplete; original backups have been preserved. Inspect recovery_backups before retrying.".into(),
                category:"execution", retryable:false,
                details:json!({"affected_paths":touched,"rollback_errors":rollback_errors,"recovery_backups":preserved,"cause":cause.to_error_value()}),
            });
        }
        return Err(cause);
    }
    for temp in prepared.values() {
        cleanup(temp);
    }
    // Retain an undeletable hidden backup rather than turn successful installation into data loss.
    for backup in recovery.values() {
        cleanup(backup);
        sync_parent(backup);
    }
    Ok(originals)
}
fn sync_parent(path: &std::path::Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
fn io_error(error: std::io::Error) -> WorkspaceError {
    WorkspaceError::Tool {
        code: "PATCH_FAILED",
        message: format!("File transaction failed: {error}"),
        category: "execution",
        retryable: false,
    }
}
fn cleanup(path: &std::path::Path) {
    #[cfg(windows)]
    if let Ok(meta) = fs::metadata(path) {
        let mut permissions = meta.permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
    }
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    fn inputs() -> (
        tempfile::TempDir,
        Workspace,
        HashMap<String, Option<Vec<u8>>>,
        HashMap<String, Option<Vec<u8>>>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            fs::write(directory.path().join(name), b"original").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                directory.path().join("a"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let ws = Workspace::new(directory.path().into()).unwrap();
        let staged = HashMap::from([("a".into(), None), ("b".into(), Some(b"new".to_vec()))]);
        let baseline = HashMap::from([
            ("a".into(), Some(b"original".to_vec())),
            ("b".into(), Some(b"original".to_vec())),
        ]);
        (directory, ws, staged, baseline)
    }
    #[test]
    fn failed_install_restores_deleted_original_with_its_permissions() {
        let (dir, ws, staged, baseline) = inputs();
        let result = commit_impl(&ws, &staged, &baseline, &HashMap::new(), |phase, path| {
            if phase == "install" && path == "b" {
                Err(std::io::Error::other("injected installation failure"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        for name in ["a", "b"] {
            assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"original");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.path().join("a"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
    #[test]
    fn failed_restore_retains_the_original_backup_and_reports_its_location() {
        let (dir, ws, staged, baseline) = inputs();
        let result = commit_impl(&ws, &staged, &baseline, &HashMap::new(), |phase, path| {
            if (phase == "install" && path == "b") || (phase == "rollback" && path == "a") {
                Err(std::io::Error::other("injected filesystem failure"))
            } else {
                Ok(())
            }
        })
        .unwrap_err()
        .to_error_value();
        assert_eq!(result["code"], "PATCH_ROLLBACK_FAILED");
        let backup = result["details"]["recovery_backups"]["a"].as_str().unwrap();
        assert_eq!(fs::read(dir.path().join(backup)).unwrap(), b"original");
        assert!(!dir.path().join("a").exists());
        assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"original");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.path().join(backup))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(!fs::read_dir(dir.path()).unwrap().flatten().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with(".coding-tools-stage-")));
    }
}
