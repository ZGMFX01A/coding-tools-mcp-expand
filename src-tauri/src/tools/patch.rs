use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::context::ToolContext;
use crate::tools::workspace::{tool_ok, Workspace, WorkspaceError};

pub fn apply_patch(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let _mutation = ctx.mutation_lock.lock().unwrap_or_else(|e| e.into_inner());
    let ws = &ctx.workspace;
    let patch = args
        .get("patch")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("patch is required"))?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let confirm = args
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || ctx.policy.skip_permission_gates();

    let file_patches = parse_unified_diff(patch)?;
    if file_patches.is_empty() {
        return Err(patch_failed("No files were modified."));
    }
    if let Some(path) = file_patches
        .iter()
        .find(|file| is_protected_repository_asset(&file.path))
        .map(|file| file.path.as_str())
    {
        return Err(protected_repository_asset(format!(
            "禁止删除仓库保护资产: {path}"
        )));
    }
    if !confirm {
        if let Some(path) = file_patches
            .iter()
            .find(|file| {
                (file.is_deleted || file.move_to.is_some()) && is_critical_file(&file.path)
            })
            .map(|file| file.path.as_str())
        {
            return Err(dangerous_operation(format!(
                "删除关键项目文件需要 confirm=true: {path}"
            )));
        }
    }

    let mut affected = Vec::new();
    let mut staged: HashMap<String, Option<String>> = HashMap::new();
    let mut baseline = HashMap::new();
    let mut permissions = HashMap::new();
    let mut block_counts = HashMap::<String, usize>::new();

    for fp in &file_patches {
        ws.reject_unsafe_text(&fp.path)?;
        let resolved = ws.resolve_for_write(&fp.path)?;
        ws.reject_write_symlink(&fp.path)?;

        let before = super::changes::read_optional(&resolved.path)?;
        baseline
            .entry(resolved.display.clone())
            .or_insert(before.clone());
        *block_counts.entry(resolved.display.clone()).or_default() += 1;
        if !fp.is_new_file && staged.get(&resolved.display) == Some(&None) {
            return Err(WorkspaceError::not_found(format!(
                "File removed by an earlier patch block: {}",
                fp.path
            )));
        }
        if !fp.is_new_file && before.is_none() && !staged.contains_key(&resolved.display) {
            return Err(WorkspaceError::not_found(format!(
                "File not found: {}",
                fp.path
            )));
        }
        let original = if fp.is_new_file || fp.is_deleted {
            // An Add File envelope is replacement content even when an earlier
            // Delete File for the same path exists in this transaction.
            String::new()
        } else if let Some(Some(text)) = staged.get(&resolved.display) {
            text.clone()
        } else if let Some(bytes) = before {
            String::from_utf8(bytes).map_err(|_| WorkspaceError::Tool {
                code: "UNSUPPORTED_ENCODING",
                message: "Patch updates require UTF-8".into(),
                category: "validation",
                retryable: false,
            })?
        } else {
            return Err(patch_failed(format!("File not found: {}", fp.path)));
        };

        if fp.is_deleted {
            staged.insert(resolved.display.clone(), None);
            affected.push(json!({ "path": resolved.display, "operation": "delete" }));
            continue;
        }

        let (updated, evidence) = super::patch_match::apply(&original, &fp.hunks)?;
        let output_path = if let Some(destination) = &fp.move_to {
            ws.reject_write_symlink(destination)?;
            ws.reject_protected_write_path(destination)?;
            let target = ws.resolve_for_write(destination)?;
            if target.existed || baseline.contains_key(&target.display) {
                return Err(patch_failed(
                    "Move destination already exists or is touched by this request",
                ));
            }
            baseline.insert(target.display.clone(), None);
            let source_permissions = permissions.get(&resolved.display).cloned().or_else(|| {
                fs::metadata(&resolved.path)
                    .ok()
                    .map(|meta| meta.permissions())
            });
            if let Some(mode) = source_permissions {
                permissions.insert(target.display.clone(), mode);
            }
            staged.insert(resolved.display.clone(), None);
            affected.push(super::changes::metadata(
                &resolved.display,
                "delete",
                None,
                json!([]),
            ));
            target.display
        } else {
            resolved.display.clone()
        };
        let op = if fp.move_to.is_some() || !resolved.existed {
            "add"
        } else {
            "update"
        };
        let mut metadata = super::changes::metadata(
            &output_path,
            op,
            Some(updated.as_bytes()),
            evidence["changed_ranges"].clone(),
        );
        metadata["match_quality"] = evidence["match_quality"].clone();
        metadata["already_applied_hunks"] = evidence["already_applied_hunks"].clone();
        affected.push(metadata);
        staged.insert(output_path.clone(), Some(updated));
    }

    // One final revision per path. Repeated blocks use staged contents, while
    // their merged range is measured against the original transaction baseline.
    let mut final_files = std::collections::BTreeMap::<String, Value>::new();
    for evidence in affected {
        let path = evidence["path"].as_str().unwrap().to_string();
        let updated = staged[&path].as_ref().map(|s| s.as_bytes());
        let original = baseline[&path].as_deref();
        let operation = if updated.is_none() {
            "delete"
        } else if original.is_none() {
            "add"
        } else {
            "update"
        };
        let ranges = if block_counts.get(&path).copied().unwrap_or(0) > 1 {
            final_changed_range(original.unwrap_or_default(), updated.unwrap_or_default())
        } else {
            evidence
                .get("changed_ranges")
                .cloned()
                .unwrap_or_else(|| json!([]))
        };
        let mut metadata = super::changes::metadata(&path, operation, updated, ranges);
        if let Some(quality) = evidence.get("match_quality") {
            let grades = ["exact", "trailing_ws", "indent"];
            let rank = |q: &Value| {
                grades
                    .iter()
                    .position(|v| Some(*v) == q.as_str())
                    .unwrap_or(0)
            };
            let previous = final_files
                .get(&path)
                .map(|v| rank(&v["match_quality"]))
                .unwrap_or(0);
            metadata["match_quality"] = json!(grades[previous.max(rank(quality))]);
        }
        metadata["already_applied_hunks"] = evidence
            .get("already_applied_hunks")
            .cloned()
            .unwrap_or_else(|| json!([]));
        final_files.insert(path, metadata);
    }
    let affected = final_files.into_values().collect::<Vec<_>>();
    let summaries = affected
        .iter()
        .map(|file| {
            format!(
                "{} {}",
                match file["operation"].as_str() {
                    Some("add") => "A",
                    Some("delete") => "D",
                    _ => "M",
                },
                file["path"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    let files_created = affected_paths(&affected, "add");
    let files_modified = affected_paths(&affected, "update");
    let files_deleted = affected_paths(&affected, "delete");

    let staged_bytes = staged
        .iter()
        .map(|(path, text)| (path.clone(), text.as_ref().map(|s| s.as_bytes().to_vec())))
        .collect::<HashMap<_, _>>();
    let changed = staged_bytes
        .iter()
        .any(|(path, bytes)| baseline.get(path) != Some(bytes));
    if !dry_run {
        if changed {
            commit_transaction(ws, &staged_bytes, &baseline, &permissions)?;
        }
        let change_id = Uuid::new_v4().simple().to_string();
        return Ok(tool_ok(json!({
            "dry_run": false,
            "workspace_changed": changed,
            "already_applied": !changed,
            "clean": true,
            "change_id": change_id,
            "summary": summaries.join("\n"),
            "affected_files": affected,
            "files_created": files_created,
            "files_modified": files_modified,
            "files_deleted": files_deleted,
            "recovery": "git",
            "warnings": []
        })));
    }

    Ok(tool_ok(json!({
        "dry_run": true,
        "workspace_changed": false,
        "already_applied": !changed,
        "preflight": true,
        "clean": true,
        "summary": summaries.join("\n"),
        "affected_files": affected,
        "would_create": files_created,
        "would_modify": files_modified,
        "would_delete": files_deleted,
        "warnings": []
    })))
}

pub fn patch_check(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let mut check_args = args.clone();
    check_args["dry_run"] = Value::Bool(true);
    let mut result = apply_patch(ctx, &check_args)?;
    if let Some(object) = result.as_object_mut() {
        object.insert("preflight".into(), Value::Bool(true));
    }
    Ok(result)
}

fn final_changed_range(original: &[u8], updated: &[u8]) -> Value {
    let old = super::changes::normalize_lf(&String::from_utf8_lossy(original));
    let new = super::changes::normalize_lf(&String::from_utf8_lossy(updated));
    let old = old.split_terminator('\n').collect::<Vec<_>>();
    let new = new.split_terminator('\n').collect::<Vec<_>>();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    if prefix == old.len() && prefix == new.len() {
        return json!([]);
    }
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    json!([{"hunk_index":0,"old_start_line":prefix+1,"old_end_line":old.len()-suffix,"new_start_line":prefix+1,"new_end_line":new.len()-suffix}])
}

#[derive(Debug)]
struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
    is_new_file: bool,
    is_deleted: bool,
    move_to: Option<String>,
}

#[derive(Debug)]
pub(super) struct Hunk {
    pub(super) lines: Vec<HunkLine>,
}

#[derive(Debug)]
pub(super) enum HunkLine {
    Anchor(String),
    EndOfFile,
    Context(String),
    Add(String),
    Remove(String),
}

fn parse_unified_diff(patch: &str) -> Result<Vec<FilePatch>, WorkspaceError> {
    if patch
        .lines()
        .any(|line| line.trim_end_matches('\r') == "*** Begin Patch")
    {
        return parse_codex_patch(patch);
    }

    let mut files = Vec::new();
    let mut current: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
        if line.starts_with("--- ") {
            if let Some(h) = current_hunk.take() {
                if let Some(ref mut f) = current {
                    f.hunks.push(h);
                }
            }
            if let Some(f) = current.take() {
                files.push(f);
            }
            let path = parse_diff_path(line.strip_prefix("--- ").unwrap_or(""));
            current = Some(FilePatch {
                path,
                hunks: Vec::new(),
                is_new_file: line.contains("/dev/null"),
                is_deleted: false,
                move_to: None,
            });
        } else if line.starts_with("+++ ") {
            if let Some(ref mut f) = current {
                let new_path = parse_diff_path(line.strip_prefix("+++ ").unwrap_or(""));
                if !new_path.is_empty() && new_path != "/dev/null" {
                    f.path = new_path;
                }
                if line.contains("/dev/null") {
                    f.is_deleted = true;
                }
            }
        } else if line.starts_with("@@") {
            if let Some(h) = current_hunk.take() {
                if let Some(ref mut f) = current {
                    f.hunks.push(h);
                }
            }
            current_hunk = Some(Hunk { lines: Vec::new() });
            if line.starts_with("@@") {
                let anchor = line
                    .trim_start_matches("@@")
                    .trim()
                    .trim_end_matches("@@")
                    .trim();
                if !anchor.is_empty() && !anchor.starts_with('-') {
                    current_hunk
                        .as_mut()
                        .unwrap()
                        .lines
                        .push(HunkLine::Anchor(anchor.into()));
                }
            }
        } else if let Some(ref mut hunk) = current_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(HunkLine::Add(rest.to_string()));
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(HunkLine::Remove(rest.to_string()));
            } else if let Some(rest) = line.strip_prefix(' ') {
                hunk.lines.push(HunkLine::Context(rest.to_string()));
            } else if line.is_empty() {
                hunk.lines.push(HunkLine::Context(String::new()));
            }
        }
    }
    if let Some(h) = current_hunk.take() {
        if let Some(ref mut f) = current {
            f.hunks.push(h);
        }
    }
    if let Some(f) = current.take() {
        files.push(f);
    }
    Ok(files)
}

fn parse_codex_patch(patch: &str) -> Result<Vec<FilePatch>, WorkspaceError> {
    let mut files = Vec::new();
    let mut current: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut ended = false;
    for raw in patch.lines() {
        let line = raw.trim_end_matches('\r');
        if line == "*** Begin Patch" {
            continue;
        }
        if line == "*** End Patch" {
            finish_codex_file(&mut files, &mut current, &mut current_hunk);
            ended = true;
            continue;
        }
        if ended {
            if !line.trim().is_empty() {
                return Err(patch_failed("Unexpected text after End Patch"));
            }
            continue;
        }
        let header = line
            .strip_prefix("*** Add File: ")
            .map(|p| (p, true, false))
            .or_else(|| {
                line.strip_prefix("*** Update File: ")
                    .map(|p| (p, false, false))
            })
            .or_else(|| {
                line.strip_prefix("*** Delete File: ")
                    .map(|p| (p, false, true))
            });
        if let Some((path, is_new_file, is_deleted)) = header {
            finish_codex_file(&mut files, &mut current, &mut current_hunk);
            current = Some(FilePatch {
                path: parse_diff_path(path),
                hunks: Vec::new(),
                is_new_file,
                is_deleted,
                move_to: None,
            });
            if is_new_file {
                current_hunk = Some(Hunk { lines: Vec::new() });
            }
            continue;
        }
        if let Some(destination) = line.strip_prefix("*** Move to: ") {
            let file = current
                .as_mut()
                .ok_or_else(|| patch_failed("Move to requires Update File"))?;
            if file.is_new_file || file.is_deleted || file.move_to.is_some() {
                return Err(patch_failed("Move to requires one Update File"));
            }
            file.move_to = Some(parse_diff_path(destination));
            continue;
        }
        if line.starts_with("@@") {
            if let Some(hunk) = current_hunk.take() {
                if let Some(file) = current.as_mut() {
                    file.hunks.push(hunk);
                }
            }
            let anchor = line
                .trim_start_matches("@@")
                .trim()
                .trim_end_matches("@@")
                .trim();
            let mut lines = Vec::new();
            if !anchor.is_empty() && !anchor.starts_with('-') {
                lines.push(HunkLine::Anchor(anchor.into()));
            }
            current_hunk = Some(Hunk { lines });
            continue;
        }
        let file = current
            .as_ref()
            .ok_or_else(|| patch_failed("Expected a file header"))?;
        if file.is_deleted {
            if !line.is_empty() {
                return Err(patch_failed("Delete File must not contain hunks"));
            }
            continue;
        }
        let hunk = current_hunk.get_or_insert_with(|| Hunk { lines: Vec::new() });
        if line == "*** End of File" {
            hunk.lines.push(HunkLine::EndOfFile);
            continue;
        }
        if let Some(value) = line.strip_prefix('+') {
            hunk.lines.push(HunkLine::Add(value.into()));
        } else if let Some(value) = line.strip_prefix('-') {
            hunk.lines.push(HunkLine::Remove(value.into()));
        } else if let Some(value) = line.strip_prefix(' ') {
            hunk.lines.push(HunkLine::Context(value.into()));
        } else if line.is_empty() {
            hunk.lines.push(HunkLine::Context(String::new()));
        } else {
            return Err(patch_failed("Invalid patch hunk line"));
        }
    }
    if !ended {
        return Err(patch_failed("Missing End Patch"));
    }
    Ok(files)
}

fn finish_codex_file(
    files: &mut Vec<FilePatch>,
    current: &mut Option<FilePatch>,
    current_hunk: &mut Option<Hunk>,
) {
    if let Some(hunk) = current_hunk.take() {
        if let Some(file) = current.as_mut() {
            file.hunks.push(hunk);
        }
    }
    if let Some(file) = current.take() {
        files.push(file);
    }
}

fn affected_paths(affected: &[Value], operation: &str) -> Vec<String> {
    affected
        .iter()
        .filter(|file| file["operation"] == operation)
        .filter_map(|file| file["path"].as_str().map(str::to_string))
        .collect()
}

fn parse_diff_path(raw: &str) -> String {
    let trimmed = raw.trim();
    let path = trimmed
        .strip_prefix("a/")
        .or_else(|| trimmed.strip_prefix("b/"))
        .unwrap_or(trimmed);
    if path == "/dev/null" {
        return String::new();
    }
    path.replace('\\', "/")
}

#[cfg(test)]
fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, WorkspaceError> {
    super::patch_match::apply(original, hunks).map(|v| v.0)
}

pub(crate) fn commit_transaction(
    ws: &Workspace,
    staged: &HashMap<String, Option<Vec<u8>>>,
    baseline: &HashMap<String, Option<Vec<u8>>>,
    permissions: &HashMap<String, fs::Permissions>,
) -> Result<HashMap<PathBuf, Option<Vec<u8>>>, WorkspaceError> {
    super::transaction::commit(ws, staged, baseline, permissions)
}

pub(crate) fn replace_file(
    temp: &std::path::Path,
    path: &std::path::Path,
) -> Result<(), std::io::Error> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let from = temp
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let to = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|_| std::io::Error::last_os_error())
    }
    #[cfg(not(windows))]
    {
        fs::rename(temp, path)
    }
}

pub(crate) fn is_critical_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or("");
    if matches!(first, ".git" | ".github") {
        return true;
    }
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    name == ".gitignore"
        || name == "Cargo.toml"
        || name == "Cargo.lock"
        || name == "package.json"
        || name == "package-lock.json"
        || name == "pnpm-lock.yaml"
        || name == "tauri.conf.json"
        || name.starts_with("README")
        || name.starts_with("LICENSE")
        || name.starts_with("vite.config.")
        || name == "pyproject.toml"
}

fn is_protected_repository_asset(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or("");
    matches!(first, ".git" | ".github")
}

fn dangerous_operation(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Tool {
        code: "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION",
        message: message.into(),
        category: "permission",
        retryable: false,
    }
}

fn protected_repository_asset(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Tool {
        code: "PROTECTED_REPOSITORY_ASSET",
        message: message.into(),
        category: "security",
        retryable: false,
    }
}

fn patch_failed(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Tool {
        code: "PATCH_FAILED",
        message: message.into(),
        category: "validation",
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::context::ToolContext;
    use serde_json::json;
    use tempfile::tempdir;

    fn context_with_file() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
        let workspace = tempdir().expect("workspace");
        let harness = tempdir().expect("harness");
        std::fs::write(workspace.path().join("main.rs"), "old\n").expect("file");
        let context =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("context");
        (workspace, harness, context)
    }

    fn patch() -> Value {
        json!({
            "patch": "--- a/main.rs\n+++ b/main.rs\n@@\n-old\n+new\n"
        })
    }

    #[test]
    fn patch_check_does_not_modify_workspace() {
        let (_workspace, _harness, context) = context_with_file();
        let result = patch_check(&context, &patch()).expect("patch check");
        assert_eq!(result["preflight"], true);
        assert_eq!(
            std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
            "old\n"
        );
    }

    #[test]
    fn preserves_crlf_when_inserting_multiple_lines() {
        let input = "one\r\ntwo\r\n";
        let hunk = Hunk {
            lines: vec![
                HunkLine::Context("one".into()),
                HunkLine::Add("insert-a".into()),
                HunkLine::Add("insert-b".into()),
                HunkLine::Context("two".into()),
            ],
        };
        assert_eq!(
            apply_hunks(input, &[hunk]).expect("patch"),
            "one\r\ninsert-a\r\ninsert-b\r\ntwo\r\n"
        );
    }

    #[test]
    fn delete_then_add_same_path_replaces_instead_of_concatenating_old_content() {
        let (_workspace, _harness, context) = context_with_file();
        let result = apply_patch(
            &context,
            &json!({
                "patch": "*** Begin Patch\n*** Delete File: main.rs\n*** Add File: main.rs\n+fresh\n*** End Patch\n"
            }),
        )
        .expect("replace file");
        assert_eq!(result["files_modified"], json!(["main.rs"]));
        assert_eq!(
            std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
            "fresh\n"
        );
    }

    #[test]
    fn validation_failure_in_later_file_keeps_all_files_unchanged() {
        let (_workspace, _harness, context) = context_with_file();
        let error = apply_patch(
            &context,
            &json!({
                "patch": "--- a/main.rs\n+++ b/main.rs\n@@\n-old\n+new\n--- a/missing.rs\n+++ b/missing.rs\n@@\n-old\n+new\n"
            }),
        )
        .expect_err("later file fails preflight");
        assert_eq!(error.to_error_value()["code"], "NOT_FOUND");
        assert_eq!(
            std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
            "old\n"
        );
    }

    #[test]
    fn preserves_utf8_bom() {
        let input = "\u{feff}header\nbody\n";
        let hunk = Hunk {
            lines: vec![
                HunkLine::Context("header".into()),
                HunkLine::Remove("body".into()),
                HunkLine::Add("updated_body".into()),
            ],
        };
        let output = apply_hunks(input, &[hunk]).expect("patch with bom");
        assert!(output.starts_with('\u{feff}'), "BOM must be preserved");
        assert_eq!(output, "\u{feff}header\nupdated_body\n");
    }

    #[test]
    fn multi_file_patch_success() {
        let (workspace, _harness, context) = context_with_file();
        std::fs::write(workspace.path().join("second.rs"), "alpha\n").expect("second");
        let result = apply_patch(
            &context,
            &json!({
                "patch": "--- a/main.rs\n+++ b/main.rs\n@@\n-old\n+new\n--- a/second.rs\n+++ b/second.rs\n@@\n-alpha\n+beta\n"
            }),
        )
        .expect("multi file patch");
        assert_eq!(result["clean"], true);
        assert_eq!(
            std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(context.workspace.root().join("second.rs")).unwrap(),
            "beta\n"
        );
    }

    #[test]
    fn ambiguous_context_returns_error() {
        let input = "duplicate\nmiddle\nduplicate\n";
        let hunk = Hunk {
            lines: vec![
                HunkLine::Context("duplicate".into()),
                HunkLine::Add("inserted".into()),
            ],
        };
        let err = apply_hunks(input, &[hunk]).expect_err("should fail ambiguous");
        let val = err.to_error_value();
        assert_eq!(val["code"], "PATCH_CONTEXT_AMBIGUOUS");
    }
}
