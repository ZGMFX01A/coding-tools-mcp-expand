//! Bounded request retry ledgers shared by MCP and Actions for one workspace.
use super::workspace::tool_err_code;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex};

#[derive(Default)]
pub struct Reliability {
    state: Mutex<State>,
    ready: Condvar,
}
#[derive(Default)]
struct State {
    cache: VecDeque<(String, String, Value)>,
    pending: HashMap<String, String>,
    failures: VecDeque<(String, String, u32)>,
    generation: u64,
    counted_commands: VecDeque<String>,
    operation_failures: HashMap<String, u64>,
}

pub fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct Reservation<'a> {
    ledger: &'a Reliability,
    key: Option<String>,
}
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some(key) = &self.key {
            self.ledger
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending
                .remove(key);
            self.ledger.ready.notify_all();
        }
    }
}

impl Reliability {
    pub fn run(&self, name: &str, args: &Value, execute: impl FnOnce() -> Value) -> Value {
        let mut normalized = args.clone();
        if let Some(object) = normalized.as_object_mut() {
            object.remove("idempotency_key");
        }
        let fingerprint = format!("{name}:{}", revision(normalized.to_string().as_bytes()));
        let mut key = None;
        if matches!(name, "apply_patch" | "apply_changes") {
            if let Some(value) = args.get("idempotency_key") {
                let Some(text) = value
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.len() <= 128)
                else {
                    return tool_err_code(
                        "INVALID_ARGUMENT",
                        "idempotency_key must be a non-empty string of at most 128 bytes",
                        "validation",
                    );
                };
                key = Some(format!("{name}:{text}"));
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = &key {
            loop {
                if let Some(index) = state.cache.iter().position(|(k, _, _)| k == key) {
                    let entry = state.cache.remove(index).unwrap();
                    let mut result = entry.2.clone();
                    let same = entry.1 == fingerprint;
                    state.cache.push_back(entry);
                    if !same {
                        return tool_err_code(
                            "IDEMPOTENCY_KEY_REUSED",
                            "Use a new idempotency_key for different arguments",
                            "validation",
                        );
                    }
                    result["idempotent_replay"] = json!(true);
                    return result;
                }
                match state.pending.get(key) {
                    Some(other) if other != &fingerprint => {
                        return tool_err_code(
                            "IDEMPOTENCY_KEY_REUSED",
                            "This key is already executing different arguments",
                            "validation",
                        )
                    }
                    Some(_) => state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner()),
                    None => break,
                }
            }
        }
        if let Some((_, code, count)) = state
            .failures
            .iter()
            .find(|(fp, _, count)| fp == &fingerprint && *count >= 2)
        {
            let mut error = tool_err_code("REPEATED_CALL_BLOCKED", "This exact request has failed twice. Change the arguments or repair the workspace before retrying.", "validation");
            error["error"]["details"] =
                json!({"previous_error_code": code, "failure_count": count});
            return error;
        }
        if let Some(key) = &key {
            state.pending.insert(key.clone(), fingerprint.clone());
        }
        let generation = state.generation;
        drop(state);
        let _reservation = Reservation {
            ledger: self,
            key: key.clone(),
        };
        let mut result = execute();
        let outcome = operation_outcome(&result);
        if let Some(outcome) = outcome {
            result["operation_outcome"] = json!(outcome);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let succeeded = result["ok"] == true;
        let dry_run = args["dry_run"] == true || name == "patch_check";
        let wrote = succeeded
            && !dry_run
            && matches!(name, "apply_patch" | "apply_changes")
            && result["workspace_changed"] == true;
        // Only the first terminal observation of a command invalidates failures.
        let terminal_command = matches!(
            name,
            "exec_command" | "write_stdin" | "kill_session" | "read_output"
        ) && outcome.is_some_and(|v| v != "running");
        let first_terminal = if terminal_command {
            if let Some(id) = result["session_id"].as_str() {
                if state.counted_commands.iter().any(|old| old == id) {
                    false
                } else {
                    state.counted_commands.push_back(id.to_owned());
                    while state.counted_commands.len() > 512 {
                        state.counted_commands.pop_front();
                    }
                    true
                }
            } else {
                true
            }
        } else {
            false
        };
        if wrote || (first_terminal && result["workspace_may_have_changed"] != false) {
            state.failures.clear();
            state.generation = state.generation.wrapping_add(1);
        }
        if !succeeded && state.generation == generation {
            if let Some(code) = result["error"]["code"]
                .as_str()
                .filter(|code| deterministic(code, result["error"]["retryable"] == true))
            {
                if let Some(index) = state
                    .failures
                    .iter()
                    .position(|(fp, _, _)| fp == &fingerprint)
                {
                    let (_, old_code, count) = state.failures.remove(index).unwrap();
                    state.failures.push_back((
                        fingerprint.clone(),
                        code.to_owned(),
                        if old_code == code { count + 1 } else { 1 },
                    ));
                } else {
                    state
                        .failures
                        .push_back((fingerprint.clone(), code.to_owned(), 1));
                }
                while state.failures.len() > 256 {
                    state.failures.pop_front();
                }
            }
        } else if succeeded {
            state.failures.retain(|(fp, _, _)| fp != &fingerprint);
        }
        if succeeded && !dry_run {
            if let Some(key) = key {
                state.cache.push_back((key, fingerprint, result.clone()));
                while state.cache.len() > 64 {
                    state.cache.pop_front();
                }
            }
        }
        let counts_operation = outcome.is_none() || first_terminal;
        if counts_operation {
            let failed = !succeeded || outcome.is_some_and(|v| v != "exited_0" && v != "running");
            let counter_name = if terminal_command {
                "exec_command"
            } else {
                name
            };
            let code = result["error"]["code"]
                .as_str()
                .or(outcome)
                .unwrap_or("tool_error");
            if failed {
                // Tool names are bounded by the registry; error codes and outcomes are server-owned.
                let entry = state
                    .operation_failures
                    .entry(format!("{counter_name}:{code}"))
                    .or_default();
                *entry += 1;
            } else {
                state
                    .operation_failures
                    .retain(|key, _| !key.starts_with(&format!("{counter_name}:")));
            }
        }
        result
    }

    pub fn diagnostics(&self) -> Value {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        json!({"idempotency_cache_entries": state.cache.len(), "idempotency_cache_limit": 64,
            "repeated_failure_entries": state.failures.len(), "operation_failure_streaks": state.operation_failures})
    }
}

fn deterministic(code: &str, retryable: bool) -> bool {
    !matches!(
        code,
        "IDEMPOTENCY_KEY_REUSED"
            | "PATCH_CONFLICT"
            | "COMMAND_LIMIT"
            | "COMMAND_LIMIT_REACHED"
            | "PATCH_ROLLBACK_FAILED"
    ) && (!retryable
        || matches!(
            code,
            "PATCH_CONTEXT_NOT_FOUND"
                | "PATCH_CONTEXT_AMBIGUOUS"
                | "REVISION_MISMATCH"
                | "REVISION_REQUIRED"
        ))
}

pub fn operation_outcome(result: &Value) -> Option<&'static str> {
    match result["termination_reason"].as_str() {
        Some("running") => Some("running"),
        Some("timeout") => Some("timeout"),
        Some("killed" | "crashed") => Some("signal"),
        Some("spawn_failed") => Some("spawn_error"),
        Some("exited") => Some(if result["exit_code"] == 0 {
            "exited_0"
        } else {
            "exited_nonzero"
        }),
        _ if result.get("command_ok").is_some() => Some(match result["command_ok"].as_bool() {
            Some(true) => "exited_0",
            Some(false) => "exited_nonzero",
            None => "running",
        }),
        _ => None,
    }
}
