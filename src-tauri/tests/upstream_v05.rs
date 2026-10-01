use coding_tools_mcp_desktop_lib::{
    mcp::handle_request,
    tools::{call_tool, ToolContext},
};
use serde_json::json;
use std::{fs, sync::Arc};

fn context() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
    let workspace = tempfile::tempdir().unwrap();
    let harness = tempfile::tempdir().unwrap();
    let ctx = ToolContext::for_test(workspace.path().into(), harness.path().into()).unwrap();
    (workspace, harness, ctx)
}

#[test]
fn mcp_calls_have_no_conversation_deadline_or_budget_metadata() {
    let (_ws, _harness, ctx) = context();
    let response = handle_request(
        &Arc::new(ctx),
        &json!({"jsonrpc":"2.0", "id":1,
        "method":"tools/call", "params":{"name":"server_info", "arguments":{},
        "_meta":{"openai/session":"long-conversation"}}}),
    );
    assert!(response["result"]["_meta"]["coding-tools/agentTurnBudget"].is_null());
    assert!(!response.to_string().contains("TURN BUDGET"));
}

#[test]
fn revision_edits_are_atomic_and_reject_stale_reads() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    let read = call_tool(&ctx, "read_file", &json!({"path":"a.txt"}));
    assert_eq!(read["revision"].as_str().unwrap().len(), 64);
    let edits = json!({"changes":[{"action":"edit", "path":"a.txt", "revision":read["revision"],
        "edits":[{"op":"replace", "start_line":2, "end_line":2, "content":"changed\r\nextra"}]}]});
    let result = call_tool(&ctx, "apply_changes", &edits);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(
        fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "one\r\nchanged\r\nextra\r\nthree\r\n"
    );
    let stale = call_tool(&ctx, "apply_changes", &edits);
    assert_eq!(stale["error"]["code"], "REVISION_MISMATCH");
    let batch = call_tool(
        &ctx,
        "apply_changes",
        &json!({"changes":[
        {"action":"create", "path":"new.txt", "content":"new"},
        {"action":"delete", "path":"a.txt", "revision":read["revision"]}]}),
    );
    assert_eq!(batch["ok"], false);
    assert!(!ws.path().join("new.txt").exists());
}

#[test]
fn same_key_replays_but_different_request_is_rejected() {
    let (ws, _harness, ctx) = context();
    let args = json!({"idempotency_key":"retry-1", "changes":[{"action":"create", "path":"a.txt", "content":"one"}]});
    let first = call_tool(&ctx, "apply_changes", &args);
    assert_eq!(first["ok"], true, "{first}");
    let replay = call_tool(&ctx, "apply_changes", &args);
    assert_eq!(replay["idempotent_replay"], true);
    let changed = call_tool(
        &ctx,
        "apply_changes",
        &json!({"idempotency_key":"retry-1",
        "changes":[{"action":"create", "path":"b.txt", "content":"two"}]}),
    );
    assert_eq!(changed["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    assert!(!ws.path().join("b.txt").exists());
}

#[test]
fn deterministic_retries_are_bounded_and_writes_reset_them() {
    let (_ws, _harness, ctx) = context();
    let args = json!({"path":"missing.txt"});
    assert_eq!(
        call_tool(&ctx, "read_file", &args)["error"]["code"],
        "NOT_FOUND"
    );
    assert_eq!(
        call_tool(&ctx, "read_file", &args)["error"]["code"],
        "NOT_FOUND"
    );
    assert_eq!(
        call_tool(&ctx, "read_file", &args)["error"]["code"],
        "REPEATED_CALL_BLOCKED"
    );
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"create","path":"missing.txt","content":"fixed"}]})
        )["ok"],
        true
    );
    assert_eq!(call_tool(&ctx, "read_file", &args)["content"], "fixed");
}

#[test]
fn patch_reports_quality_evidence_and_replay() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a.txt"), "header\n  old  \nend\n").unwrap();
    let args = json!({"patch":"*** Begin Patch\n*** Update File: a.txt\n@@ header\n-old\n+new\n end\n*** End of File\n*** End Patch\n"});
    let result = call_tool(&ctx, "apply_patch", &args);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["affected_files"][0]["match_quality"], "indent");
    assert!(result["affected_files"][0]["revision"].is_string());
    assert!(result["affected_files"][0]["changed_ranges"].is_array());
    let replay = call_tool(&ctx, "apply_patch", &args);
    assert_eq!(replay["already_applied"], true, "{replay}");
}
