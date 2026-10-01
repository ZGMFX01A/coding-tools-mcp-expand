//! Revision-checked, declarative edits. Line numbers always refer to the original file.
use super::{
    context::ToolContext,
    reliability::revision,
    workspace::{tool_ok, WorkspaceError},
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;

pub fn apply_changes(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let _mutation = ctx.mutation_lock.lock().unwrap_or_else(|e| e.into_inner());
    let changes = args["changes"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 100)
        .ok_or_else(|| WorkspaceError::invalid_argument("changes must contain 1 to 100 entries"))?;
    if args.to_string().len() > ctx.policy.max_patch_bytes {
        return Err(WorkspaceError::invalid_argument("Changes are too large"));
    }
    let dry_run = args["dry_run"] == true;
    let confirm = args["confirm"] == true || ctx.policy.skip_permission_gates();
    let ws = &ctx.workspace;
    let mut seen = HashSet::new();
    let mut baseline = HashMap::new();
    let mut staged = HashMap::new();
    let mut affected = Vec::new();
    let mut permissions = HashMap::new();
    for (index, change) in changes.iter().enumerate() {
        let action = required(change, "action")?;
        if !["create", "write", "edit", "delete", "move", "copy"].contains(&action) {
            return Err(WorkspaceError::invalid_argument("Unknown change action"));
        }
        let raw = required(change, "path")?;
        ws.reject_write_symlink(raw)?;
        ws.reject_protected_write_path(raw)?;
        let resolved = ws.resolve_for_write(raw)?;
        let path = resolved.display;
        reserve(&mut seen, &path)?;
        let original = read_optional(&resolved.path)?;
        if action == "create" {
            if change.get("revision").is_some() {
                return Err(WorkspaceError::invalid_argument(
                    "create does not accept revision",
                ));
            }
            if original.is_some() {
                return Err(error(
                    "ALREADY_EXISTS",
                    "create requires a missing path",
                    json!({"path": path}),
                ));
            }
        } else if let Some(bytes) = &original {
            let expected = required(change, "revision")?;
            if expected != revision(bytes) {
                return Err(error(
                    "REVISION_MISMATCH",
                    "File changed since it was read; read it again",
                    json!({"path": path, "change_index": index, "current_revision": revision(bytes)}),
                ));
            }
        } else if action != "write" {
            return Err(WorkspaceError::not_found(format!("File not found: {path}")));
        } else if change.get("revision").is_some() {
            return Err(error(
                "REVISION_MISMATCH",
                "Missing file has no revision",
                json!({"path": path}),
            ));
        }
        baseline.insert(path.clone(), original.clone());
        for (field, allowed) in [
            ("content", matches!(action, "create" | "write")),
            ("edits", action == "edit"),
            ("to", matches!(action, "move" | "copy")),
        ] {
            if change.get(field).is_some() && !allowed {
                return Err(WorkspaceError::invalid_argument(format!(
                    "{action} does not accept {field}"
                )));
            }
        }
        if matches!(action, "delete" | "move") && super::patch::is_critical_file(&path) && !confirm
        {
            return Err(error(
                "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION",
                "Removing a critical project file requires confirm=true",
                json!({"path": path}),
            ));
        }
        let mut ranges = json!([]);
        let updated = match action {
            "create" | "write" => Some(required(change, "content")?.as_bytes().to_vec()),
            "edit" => {
                let bytes = original.as_ref().unwrap();
                if bytes.contains(&0) {
                    return Err(error(
                        "BINARY_FILE",
                        "Line edits require a text file",
                        json!({"path":path}),
                    ));
                }
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    error(
                        "UNSUPPORTED_ENCODING",
                        "Line edits require UTF-8",
                        json!({"path":path}),
                    )
                })?;
                let (new, evidence) = edit_lines(text, &change["edits"])?;
                ranges = evidence;
                Some(new.into_bytes())
            }
            "delete" => None,
            "move" | "copy" => {
                let destination = required(change, "to")?;
                ws.reject_write_symlink(destination)?;
                ws.reject_protected_write_path(destination)?;
                let dest = ws.resolve_for_write(destination)?;
                reserve(&mut seen, &dest.display)?;
                if read_optional(&dest.path)?.is_some() {
                    return Err(error(
                        "ALREADY_EXISTS",
                        "Destination already exists",
                        json!({"path":dest.display}),
                    ));
                }
                baseline.insert(dest.display.clone(), None);
                if let Ok(meta) = fs::metadata(&resolved.path) {
                    permissions.insert(dest.display.clone(), meta.permissions());
                }
                staged.insert(dest.display.clone(), original.clone());
                affected.push(metadata(
                    &dest.display,
                    "add",
                    original.as_deref(),
                    json!([]),
                ));
                if action == "copy" {
                    original.clone()
                } else {
                    None
                }
            }
            _ => unreachable!(),
        };
        let operation = if updated.is_none() {
            "delete"
        } else if original.is_none() {
            "add"
        } else {
            "update"
        };
        affected.push(metadata(&path, operation, updated.as_deref(), ranges));
        if action != "copy" && original != updated {
            staged.insert(path, updated);
        }
    }
    let changed = staged
        .iter()
        .any(|(path, bytes)| baseline.get(path) != Some(bytes));
    if !dry_run && changed {
        super::patch::commit_transaction(ws, &staged, &baseline, &permissions)?;
    }
    let paths = |operation: &str| {
        affected
            .iter()
            .filter(|v| v["operation"] == operation)
            .map(|v| v["path"].clone())
            .collect::<Vec<_>>()
    };
    Ok(tool_ok(
        json!({"dry_run":dry_run,"clean":true,"workspace_changed":changed && !dry_run,
        "already_applied":!changed,"affected_files":affected,"files_created":paths("add"),
        "files_modified":paths("update"),"files_deleted":paths("delete"),"match_quality":"exact",
        "summary":if dry_run {"Changes validated"} else {"Changes applied"},"warnings":[]}),
    ))
}

fn reserve(seen: &mut HashSet<String>, path: &str) -> Result<(), WorkspaceError> {
    let key = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    };
    if !seen.insert(key) {
        return Err(WorkspaceError::invalid_argument(
            "Each normalized path may occur only once per request; combine its edits",
        ));
    }
    Ok(())
}
pub(crate) fn read_optional(path: &std::path::Path) -> Result<Option<Vec<u8>>, WorkspaceError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WorkspaceError::invalid_argument(format!(
            "Cannot read target file: {e}"
        ))),
    }
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, WorkspaceError> {
    value[key]
        .as_str()
        .ok_or_else(|| WorkspaceError::invalid_argument(format!("{key} must be a string")))
}
fn error(code: &'static str, message: &str, details: Value) -> WorkspaceError {
    WorkspaceError::ToolDetails {
        code,
        message: message.into(),
        category: "validation",
        retryable: false,
        details,
    }
}
pub(crate) fn normalize_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
pub(crate) fn metadata(path: &str, operation: &str, bytes: Option<&[u8]>, ranges: Value) -> Value {
    json!({"path":path,"operation":operation,"revision":bytes.map(revision),"revision_algorithm":"sha256",
        "total_lines":bytes.map(|b| normalize_lf(&String::from_utf8_lossy(b)).split_terminator('\n').count()),"changed_ranges":ranges})
}
fn edit_lines(original: &str, edits: &Value) -> Result<(String, Value), WorkspaceError> {
    let edits = edits
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 200)
        .ok_or_else(|| WorkspaceError::invalid_argument("edits must contain 1 to 200 entries"))?;
    let bom = original.starts_with('\u{feff}');
    let text = original.trim_start_matches('\u{feff}');
    let ending = if text.contains("\r\n") {
        "\r\n"
    } else if text.contains('\r') && !text.contains('\n') {
        "\r"
    } else {
        "\n"
    };
    let normalized = normalize_lf(text);
    let mut lines = normalized
        .split_terminator('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let total = lines.len();
    let trailing = normalized.ends_with('\n');
    let mut placements = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let op = required(edit, "op")?;
        let number = |key: &str| {
            edit[key].as_u64().map(|n| n as usize).ok_or_else(|| {
                WorkspaceError::invalid_argument(format!("{key} must be an integer"))
            })
        };
        let (start, end) = match op {
            "replace" | "delete" => {
                let a = number("start_line")?;
                let b = number("end_line")?;
                if a < 1 || b < a {
                    return Err(WorkspaceError::invalid_argument("Invalid line range"));
                }
                (a - 1, b)
            }
            "insert_before" => {
                let n = number("line")?;
                if n < 1 {
                    return Err(WorkspaceError::invalid_argument(
                        "insert_before line must be >=1",
                    ));
                }
                (n - 1, n - 1)
            }
            "insert_after" => {
                let n = number("line")?;
                (n, n)
            }
            _ => return Err(WorkspaceError::invalid_argument("Unknown edit operation")),
        };
        if end > total || start > total {
            return Err(error(
                "INVALID_ARGUMENT",
                "Edit is beyond the original file",
                json!({"edit_index":index,"total_lines":total}),
            ));
        }
        let content = if op == "delete" {
            if edit.get("content").is_some() {
                return Err(WorkspaceError::invalid_argument(
                    "delete does not accept content",
                ));
            }
            String::new()
        } else {
            normalize_lf(required(edit, "content")?)
        };
        let replacement = if content.is_empty() {
            vec![]
        } else {
            content.split('\n').map(str::to_string).collect()
        };
        placements.push((start, end, index, replacement));
    }
    placements.sort_by_key(|v| (v.0, v.1));
    for pair in placements.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.0 < a.1 || (a.0 == a.1 && b.0 == b.1 && a.0 == b.0) {
            return Err(error(
                "PATCH_HUNKS_OVERLAP",
                "Edits overlap; combine them",
                json!({"edit_indexes":[a.2,b.2]}),
            ));
        }
    }
    let mut ranges = Vec::new();
    let mut delta = 0isize;
    for (a, b, index, replacement) in &placements {
        if lines[*a..*b] != replacement[..] {
            ranges.push(json!({"hunk_index":index,"old_start_line":a+1,"old_end_line":b,"new_start_line":(*a as isize+delta+1),"new_end_line":(*a as isize+delta+replacement.len() as isize)}));
        }
        delta += replacement.len() as isize - (*b - *a) as isize;
    }
    for (a, b, _, replacement) in placements.into_iter().rev() {
        lines.splice(a..b, replacement);
    }
    let mut output = lines.join(ending);
    if !lines.is_empty() && (trailing || total == 0) {
        output.push_str(ending);
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok((output, json!(ranges)))
}

pub fn input_schema() -> Value {
    json!({"type":"object","required":["changes"],"additionalProperties":false,
    "properties":{
        "changes":{"type":"array","minItems":1,"maxItems":100,"items":{
            "type":"object","required":["action","path"],"additionalProperties":false,
            "properties":{
                "action":{"type":"string","enum":["create","write","edit","delete","move","copy"]},
                "path":{"type":"string","minLength":1},"to":{"type":"string","minLength":1},
                "revision":{"type":"string","pattern":"^[a-f0-9]{64}$"},"content":{"type":"string"},
                "edits":{"type":"array","minItems":1,"maxItems":200,"items":{"type":"object",
                    "required":["op"],"additionalProperties":false,"properties":{
                        "op":{"type":"string","enum":["replace","delete","insert_before","insert_after"]},
                        "start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},
                        "line":{"type":"integer","minimum":0},"content":{"type":"string"}
                    }}}
            }}},
        "dry_run":{"type":"boolean","default":false},"confirm":{"type":"boolean","default":false},
        "idempotency_key":{"type":"string","minLength":1,"maxLength":128},"reason":{"type":"string"}
    }})
}
