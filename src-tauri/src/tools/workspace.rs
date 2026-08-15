use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};
use thiserror::Error;

pub const DEFAULT_EXCLUDED_NAMES: &[&str] = &[
    ".git",
    ".reference",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "__pycache__",
];

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub display: String,
    pub path: PathBuf,
    pub existed: bool,
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("{message}")]
    Tool {
        code: &'static str,
        message: String,
        category: &'static str,
        retryable: bool,
    },
    #[error("{message}")]
    ToolDetails {
        code: &'static str,
        message: String,
        category: &'static str,
        retryable: bool,
        details: Value,
    },
}

impl WorkspaceError {
    pub fn message(&self) -> String {
        match self {
            Self::Tool { message, .. } | Self::ToolDetails { message, .. } => message.clone(),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::Tool {
            code: "INVALID_ARGUMENT",
            message: message.into(),
            category: "validation",
            retryable: false,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::Tool {
            code: "NOT_FOUND",
            message: message.into(),
            category: "not_found",
            retryable: false,
        }
    }

    pub fn absolute_path_denied() -> Self {
        Self::Tool {
            code: "ABSOLUTE_PATH_DENIED",
            message: "Absolute paths are denied.".into(),
            category: "security",
            retryable: false,
        }
    }

    pub fn path_outside_workspace() -> Self {
        Self::Tool {
            code: "PATH_OUTSIDE_WORKSPACE",
            message: "Path escapes the configured workspace.".into(),
            category: "security",
            retryable: false,
        }
    }

    pub fn symlink_escape() -> Self {
        Self::Tool {
            code: "SYMLINK_ESCAPE",
            message: "Path escapes the configured workspace.".into(),
            category: "security",
            retryable: false,
        }
    }

    pub fn not_a_directory(message: impl Into<String>) -> Self {
        Self::Tool {
            code: "NOT_A_DIRECTORY",
            message: message.into(),
            category: "validation",
            retryable: false,
        }
    }

    pub fn to_error_value(&self) -> Value {
        match self {
            Self::Tool {
                code,
                message,
                category,
                retryable,
            } => json!({
                "code": code,
                "message": message,
                "category": category,
                "retryable": retryable,
                "details": {}
            }),
            Self::ToolDetails {
                code,
                message,
                category,
                retryable,
                details,
            } => json!({
                "code": code,
                "message": message,
                "category": category,
                "retryable": retryable,
                "details": details
            }),
        }
    }
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: PathBuf) -> WorkspaceResult<Self> {
        let root = root
            .canonicalize()
            .map_err(|_| WorkspaceError::invalid_argument("Workspace root must exist"))?;
        if !root.is_dir() {
            return Err(WorkspaceError::invalid_argument(
                "Workspace root must be a directory",
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn root_display(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    pub fn reject_unsafe_text(&self, raw_path: &str) -> WorkspaceResult<()> {
        if raw_path.is_empty() {
            return Err(WorkspaceError::invalid_argument(
                "Path must be a non-empty string",
            ));
        }
        if raw_path.contains('\0') {
            return Err(WorkspaceError::invalid_argument("Path contains a NUL byte"));
        }
        if raw_path.starts_with('/') || raw_path.starts_with('\\') {
            return Err(WorkspaceError::absolute_path_denied());
        }
        if raw_path.len() >= 2 {
            let bytes = raw_path.as_bytes();
            if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                return Err(WorkspaceError::absolute_path_denied());
            }
        }
        for part in Path::new(raw_path).components() {
            if matches!(part, Component::ParentDir) {
                return Err(WorkspaceError::path_outside_workspace());
            }
        }
        Ok(())
    }

    pub fn resolve_existing(&self, raw_path: &str) -> WorkspaceResult<ResolvedPath> {
        self.resolve_existing_at(&self.root, raw_path)
    }

    /// 解析只读路径。显式的绝对路径和 `..` 路径允许指向 Workspace 外部，
    /// 但不会被任何写入工具复用。
    pub fn resolve_read_path(&self, raw_path: &str) -> WorkspaceResult<ResolvedPath> {
        let raw = if raw_path.is_empty() { "." } else { raw_path };
        self.validate_read_text(raw)?;
        let input = Path::new(raw);
        let candidate = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.root
                .join(raw.replace('/', std::path::MAIN_SEPARATOR_STR))
        };
        let resolved = candidate
            .canonicalize()
            .map_err(|_| WorkspaceError::not_found(format!("Path not found: {raw}")))?;
        let explicit_external = input.is_absolute()
            || input
                .components()
                .any(|part| matches!(part, Component::ParentDir));
        if !explicit_external && candidate.starts_with(&self.root) {
            self.ensure_inside_workspace(&candidate, &resolved)?;
        }
        Ok(ResolvedPath {
            display: relative_display(&self.root, &resolved),
            path: resolved,
            existed: true,
        })
    }

    pub fn resolve_existing_at(
        &self,
        base: &Path,
        raw_path: &str,
    ) -> WorkspaceResult<ResolvedPath> {
        let raw = if raw_path.is_empty() { "." } else { raw_path };
        self.reject_unsafe_text(raw)?;
        let base = self.validate_base(base)?;
        let candidate = base.join(raw.replace('/', std::path::MAIN_SEPARATOR_STR));
        let resolved = candidate
            .canonicalize()
            .map_err(|_| WorkspaceError::not_found(format!("Path not found: {raw}")))?;
        self.ensure_inside_workspace(&candidate, &resolved)?;
        Ok(ResolvedPath {
            display: relative_display(&self.root, &resolved),
            path: resolved,
            existed: true,
        })
    }

    pub fn resolve_for_write(&self, raw_path: &str) -> WorkspaceResult<ResolvedPath> {
        self.reject_unsafe_text(raw_path)?;
        self.reject_protected_write_path(raw_path)?;
        let pure = Path::new(raw_path);
        if pure.file_name().is_none() || raw_path == "." || raw_path == ".." {
            return Err(WorkspaceError::invalid_argument("Invalid write target"));
        }
        let candidate = self
            .root
            .join(raw_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if candidate.exists() || candidate.is_symlink() {
            let resolved = candidate
                .canonicalize()
                .map_err(|_| WorkspaceError::not_found(format!("Path not found: {raw_path}")))?;
            self.ensure_inside_workspace(&candidate, &resolved)?;
            return Ok(ResolvedPath {
                display: relative_display(&self.root, &resolved),
                path: resolved,
                existed: true,
            });
        }
        let parent = candidate.parent().unwrap_or(&self.root);
        let resolved_parent = if parent.exists() {
            parent
                .canonicalize()
                .map_err(|_| WorkspaceError::not_found("Parent directory not found"))?
        } else {
            self.ensure_parent_chain(parent)?;
            parent.to_path_buf()
        };
        if !resolved_parent.starts_with(&self.root) {
            return Err(WorkspaceError::path_outside_workspace());
        }
        Ok(ResolvedPath {
            display: raw_path.replace('\\', "/"),
            path: candidate,
            existed: false,
        })
    }

    fn ensure_parent_chain(&self, parent: &Path) -> WorkspaceResult<()> {
        let mut cursor = parent;
        while !cursor.exists() {
            if cursor == self.root || cursor.parent() == Some(cursor) {
                break;
            }
            cursor = cursor.parent().unwrap_or(cursor);
        }
        if cursor.exists() {
            let resolved = cursor
                .canonicalize()
                .map_err(|_| WorkspaceError::not_found("Parent directory not found"))?;
            if !resolved.starts_with(&self.root) {
                return Err(WorkspaceError::path_outside_workspace());
            }
        }
        Ok(())
    }

    fn validate_base(&self, base: &Path) -> WorkspaceResult<PathBuf> {
        let resolved = base
            .canonicalize()
            .map_err(|_| WorkspaceError::not_found("Base path not found"))?;
        if !resolved.is_dir() {
            return Err(WorkspaceError::not_a_directory("Base is not a directory"));
        }
        if !resolved.starts_with(&self.root) {
            return Err(WorkspaceError::path_outside_workspace());
        }
        Ok(resolved)
    }

    fn ensure_inside_workspace(&self, candidate: &Path, resolved: &Path) -> WorkspaceResult<()> {
        if !resolved.starts_with(&self.root) {
            if candidate.is_symlink() {
                return Err(WorkspaceError::symlink_escape());
            }
            return Err(WorkspaceError::path_outside_workspace());
        }
        Ok(())
    }

    pub fn reject_write_symlink(&self, raw_path: &str) -> WorkspaceResult<()> {
        self.reject_unsafe_text(raw_path)?;
        let candidate = self
            .root
            .join(raw_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if candidate.is_symlink() {
            return Err(WorkspaceError::symlink_escape());
        }
        Ok(())
    }

    pub fn reject_protected_write_path(&self, raw_path: &str) -> WorkspaceResult<()> {
        let normalized = raw_path.replace('\\', "/");
        let first = normalized.split('/').next().unwrap_or("");
        if matches!(first, ".git" | ".github") {
            return Err(WorkspaceError::Tool {
                code: "PROTECTED_PATH",
                message: format!("禁止普通文件操作写入受保护目录: {raw_path}"),
                category: "security",
                retryable: false,
            });
        }
        Ok(())
    }

    fn validate_read_text(&self, raw_path: &str) -> WorkspaceResult<()> {
        if raw_path.contains('\0') {
            return Err(WorkspaceError::invalid_argument("Path contains a NUL byte"));
        }
        Ok(())
    }

    pub fn is_ignored_path(
        &self,
        path: &Path,
        include_hidden: bool,
        include_ignored: bool,
    ) -> bool {
        let Ok(scan_path) = path.strip_prefix(&self.root) else {
            // Workspace 外的读取路径不套用 Workspace 内部的隐藏/构建目录过滤，
            // 否则 Windows 临时目录等路径会被误判为隐藏目录而无法读取。
            return false;
        };
        let parts: Vec<String> = scan_path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if !include_hidden {
            for part in &parts {
                if part.starts_with('.') && part != "." {
                    return true;
                }
            }
        }
        if !include_ignored {
            for part in &parts {
                if DEFAULT_EXCLUDED_NAMES.contains(&part.as_str()) {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_safe_existing_path(&self, path: &Path) -> bool {
        path.canonicalize()
            .map(|p| p.starts_with(&self.root))
            .unwrap_or(false)
    }

    pub fn is_safe_read_path(&self, path: &Path) -> bool {
        path.exists() || path.is_symlink()
    }
}

pub fn relative_display(root: &Path, path: &Path) -> String {
    let display = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
    #[cfg(windows)]
    {
        if let Some(unc) = display.strip_prefix("//?/UNC/") {
            return format!("//{unc}");
        }
        if let Some(normal) = display.strip_prefix("//?/") {
            return normal.to_string();
        }
    }
    display
}

pub fn tool_ok(mut value: Value) -> Value {
    if value.get("ok").is_none() {
        value
            .as_object_mut()
            .expect("tool result object")
            .insert("ok".into(), Value::Bool(true));
    }
    value
}

pub fn tool_err(error: WorkspaceError) -> Value {
    json!({
        "ok": false,
        "status": "error",
        "summary": error.message(),
        "error": error.to_error_value()
    })
}

pub fn tool_err_code(
    code: &'static str,
    message: impl Into<String>,
    category: &'static str,
) -> Value {
    let message = message.into();
    json!({
        "ok": false,
        "status": "error",
        "summary": message.clone(),
        "error": {
            "code": code,
            "message": message,
            "category": category,
            "retryable": false,
            "details": {}
        }
    })
}

pub fn wrap_tool_result(structured: Value) -> Value {
    wrap_mcp_tool_result("", &serde_json::json!({}), structured)
}

pub fn wrap_mcp_tool_result(tool_name: &str, args: &Value, structured: Value) -> Value {
    let is_error = structured.get("ok").and_then(Value::as_bool) == Some(false);
    let content = if tool_name == "view_image"
        && args
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("mcp_image")
            == "mcp_image"
        && !is_error
    {
        vec![json!({
            "type": "image",
            "data": structured.get("base64").and_then(Value::as_str).unwrap_or(""),
            "mimeType": structured
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
        })]
    } else {
        vec![json!({
            "type": "text",
            "text": render_tool_text(tool_name, args, &structured)
        })]
    };
    json!({
        "content": content,
        "structuredContent": structured,
        "isError": is_error
    })
}

pub fn render_tool_text(tool_name: &str, _args: &Value, payload: &Value) -> String {
    let is_error = payload.get("ok").and_then(Value::as_bool) == Some(false);
    if is_error {
        return render_tool_error(payload);
    }

    match tool_name {
        "read_file" => {
            let content = payload.get("content").and_then(Value::as_str).unwrap_or("");
            if payload.get("truncated").and_then(Value::as_bool) == Some(true) {
                let start_line = payload.get("start_line").and_then(Value::as_u64).unwrap_or(1);
                let end_line = payload.get("end_line").and_then(Value::as_u64).unwrap_or(0);
                let total_lines = payload.get("total_lines").and_then(Value::as_u64).unwrap_or(0);
                let next_start = payload.get("next_start_line").and_then(Value::as_u64);
                let hint = if let Some(ns) = next_start {
                    format!("; continue with read_file(start_line={ns})")
                } else {
                    "; content truncated".to_string()
                };
                format!("[Showing lines {start_line}-{end_line} of {total_lines}{hint}]\n{content}")
            } else {
                content.to_string()
            }
        }
        "list_dir" | "list_files" => {
            let entries = payload.get("entries").or_else(|| payload.get("files"));
            if let Some(arr) = entries.and_then(Value::as_array) {
                if arr.is_empty() {
                    "No entries found.".to_string()
                } else {
                    let mut lines = Vec::new();
                    for item in arr.iter().take(200) {
                        let path = item
                            .get("path")
                            .or_else(|| item.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let kind = item.get("type").and_then(Value::as_str);
                        if let Some(k) = kind {
                            lines.push(format!("{path} [{k}]"));
                        } else {
                            lines.push(path.to_string());
                        }
                    }
                    if payload.get("truncated").and_then(Value::as_bool) == Some(true) || arr.len() > 200 {
                        lines.push(format!(
                            "... results truncated (showing {} of {} items); use continuation to fetch more.",
                            lines.len().min(arr.len()),
                            arr.len()
                        ));
                    }
                    lines.join("\n")
                }
            } else {
                "No entries found.".to_string()
            }
        }
        "search_text" | "grep_text" | "grep" => {
            if let Some(matches) = payload.get("matches").and_then(Value::as_array) {
                if matches.is_empty() {
                    "No matches found.".to_string()
                } else {
                    let mut lines = Vec::new();
                    for m in matches.iter().take(100) {
                        let p = m.get("path").and_then(Value::as_str).unwrap_or("");
                        let line = m.get("line").and_then(Value::as_u64).unwrap_or(0);
                        let preview = m.get("preview").and_then(Value::as_str).unwrap_or("");
                        lines.push(format!("{p}:{line}: {preview}"));
                    }
                    if payload.get("truncated").and_then(Value::as_bool) == Some(true) || matches.len() > 100 {
                        let total = payload
                            .get("total_matches")
                            .and_then(Value::as_u64)
                            .unwrap_or(matches.len() as u64);
                        lines.push(format!(
                            "... showing {} of {} matches; narrow query or raise max_results.",
                            lines.len().min(matches.len()),
                            total
                        ));
                    }
                    lines.join("\n")
                }
            } else {
                "No matches found.".to_string()
            }
        }
        "apply_patch" | "patch_check" => {
            let dry_run = payload.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            let prefix = if dry_run {
                "Patch validated"
            } else {
                "Patch applied"
            };
            let affected = payload
                .get("affected_files")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            let summary = payload.get("summary").and_then(Value::as_str).unwrap_or("").trim();
            if summary.is_empty() {
                format!("{prefix} to {affected} file(s).")
            } else {
                format!("{prefix} to {affected} file(s).\n{summary}")
            }
        }
        "exec_command" => {
            let status = payload.get("status").and_then(Value::as_str).unwrap_or("unknown");
            let mut header = vec![format!("Status: {status}")];
            if let Some(code) = payload.get("exit_code").and_then(Value::as_i64) {
                header.push(format!("exit code {code}"));
            }
            if let Some(ms) = payload.get("elapsed_ms").and_then(Value::as_u64) {
                header.push(format!("{ms} ms"));
            }
            let mut sections = vec![header.join(" | ")];
            let stdout = payload.get("stdout").and_then(Value::as_str).unwrap_or("");
            let stderr = payload.get("stderr").and_then(Value::as_str).unwrap_or("");
            if !stdout.is_empty() {
                sections.push(stdout.to_string());
            }
            if !stderr.is_empty() {
                sections.push(format!("stderr:\n{stderr}"));
            }
            if payload.get("status").and_then(Value::as_str) == Some("running") {
                if let Some(id) = payload
                    .get("session_id")
                    .or_else(|| payload.get("command_id"))
                    .and_then(Value::as_str)
                {
                    sections.push(format!(
                        "Command still running; poll with read_output(output_ref=\"session:{id}:stdout\")."
                    ));
                }
            }
            sections.join("\n")
        }
        "read_output" => {
            let content = payload.get("content").and_then(Value::as_str).unwrap_or("");
            if let Some(next_offset) = payload.get("next_offset").and_then(Value::as_u64) {
                let ref_id = payload
                    .get("stream_output_ref")
                    .or_else(|| payload.get("output_ref"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                format!("{content}\n[more: read_output(output_ref=\"{ref_id}\", offset={next_offset})]")
            } else {
                content.to_string()
            }
        }
        "git_status" => {
            let branch = payload.get("branch").and_then(Value::as_str).unwrap_or("detached");
            let mut lines = vec![format!("## {branch}")];
            if let Some(entries) = payload.get("entries").and_then(Value::as_array) {
                if entries.is_empty() {
                    lines.push("Working tree clean.".to_string());
                } else {
                    for entry in entries {
                        let idx = entry.get("index_status").and_then(Value::as_str).unwrap_or(" ");
                        let wt = entry.get("worktree_status").and_then(Value::as_str).unwrap_or(" ");
                        let p = entry.get("path").and_then(Value::as_str).unwrap_or("");
                        lines.push(format!("{idx}{wt} {p}"));
                    }
                }
            }
            lines.join("\n")
        }
        "git_diff" => {
            payload.get("diff").and_then(Value::as_str).unwrap_or("No diff.").to_string()
        }
        "git_log" => {
            if let Some(commits) = payload.get("commits").and_then(Value::as_array) {
                if commits.is_empty() {
                    "No commits found.".to_string()
                } else {
                    let mut lines = Vec::new();
                    for c in commits {
                        let h = c.get("short_hash").and_then(Value::as_str).unwrap_or("");
                        let s = c.get("subject").and_then(Value::as_str).unwrap_or("");
                        lines.push(format!("{h} {s}"));
                    }
                    lines.join("\n")
                }
            } else {
                "No commits found.".to_string()
            }
        }
        "git_show" => {
            payload.get("content").and_then(Value::as_str).unwrap_or("No output.").to_string()
        }
        _ => {
            if let Some(summary) = payload.get("summary").and_then(Value::as_str) {
                summary.to_string()
            } else if let Some(message) = payload.get("message").and_then(Value::as_str) {
                message.to_string()
            } else {
                format!("{tool_name}: completed.")
            }
        }
    }
}

fn render_tool_error(payload: &Value) -> String {
    let error = payload.get("error");
    let code = error
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("TOOL_ERROR");
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("summary").and_then(Value::as_str))
        .unwrap_or("Tool call failed.");
    let mut lines = vec![format!("{code}: {message}")];

    if let Some(e) = error {
        let category = e.get("category").and_then(Value::as_str);
        let retryable = e.get("retryable").and_then(Value::as_bool);
        let mut facts = Vec::new();
        if let Some(cat) = category {
            facts.push(format!("Category: {cat}."));
        }
        if let Some(ret) = retryable {
            facts.push(format!("Retryable: {}.", if ret { "yes" } else { "no" }));
            if !ret {
                facts.push("Do not repeat this call unchanged.".to_string());
            }
        }
        if !facts.is_empty() {
            lines.push(facts.join(" "));
        }

        if let Some(hint) = e
            .get("recovery_hint")
            .or_else(|| e.get("details").and_then(|d| d.get("retry_hint")))
            .and_then(Value::as_str)
        {
            lines.push(format!("Retry: {hint}"));
        } else if let Some(sug) = e.get("details").and_then(|d| d.get("suggestion")).and_then(Value::as_str) {
            lines.push(format!("Suggested action: {sug}"));
        }
    }

    lines.join("\n")
}

