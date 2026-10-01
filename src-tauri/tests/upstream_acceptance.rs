use coding_tools_mcp_desktop_lib::tools::{call_tool, ToolContext};
use serde_json::{json, Value};
use std::{
    fs,
    sync::{Arc, Barrier},
};
fn context() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
    let ws = tempfile::tempdir().unwrap();
    let harness = tempfile::tempdir().unwrap();
    let ctx = ToolContext::for_test(ws.path().into(), harness.path().into()).unwrap();
    (ws, harness, ctx)
}
fn read(ctx: &ToolContext, path: &str) -> Value {
    call_tool(ctx, "read_file", &json!({"path":path}))
}

#[test]
fn stale_revision_breaker_releases_after_changed_arguments_or_real_writes() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a"), "original").unwrap();
    let stale = json!({"changes":[{"action":"write","path":"a","revision":"0".repeat(64),"content":"updated"}]});
    for _ in 0..2 {
        assert_eq!(
            call_tool(&ctx, "apply_changes", &stale)["error"]["code"],
            "REVISION_MISMATCH"
        );
    }
    assert_eq!(
        call_tool(&ctx, "apply_changes", &stale)["error"]["code"],
        "REPEATED_CALL_BLOCKED"
    );
    let mut different = stale.clone();
    different["changes"][0]["revision"] = json!("1".repeat(64));
    assert_eq!(
        call_tool(&ctx, "apply_changes", &different)["error"]["code"],
        "REVISION_MISMATCH"
    );
    let revision = read(&ctx, "a")["revision"].clone();
    let written = call_tool(
        &ctx,
        "apply_changes",
        &json!({"changes":[{"action":"write","path":"a","revision":revision,"content":"actual mutation"}]}),
    );
    assert_eq!(written["workspace_changed"], true);
    assert_eq!(
        call_tool(&ctx, "apply_changes", &stale)["error"]["code"],
        "REVISION_MISMATCH"
    );
}
#[test]
fn repeated_patch_blocks_preserve_all_edits_and_one_final_revision() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a"), "old\ncontext\n").unwrap();
    let result = call_tool(
        &ctx,
        "apply_patch",
        &json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-old\n+first\n*** Update File: a\n@@\n-context\n+second\n*** End Patch\n"}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(
        fs::read_to_string(ws.path().join("a")).unwrap(),
        "first\nsecond\n"
    );
    assert_eq!(result["affected_files"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["affected_files"][0]["revision"],
        read(&ctx, "a")["revision"]
    );
    assert_eq!(
        result["affected_files"][0]["changed_ranges"][0]["old_start_line"],
        1
    );
    assert_eq!(
        result["affected_files"][0]["changed_ranges"][0]["old_end_line"],
        2
    );
}

#[cfg(unix)]
#[test]
fn natural_unix_signal_is_distinct_from_nonzero_exit() {
    let (_ws, _harness, ctx) = context();
    let result = call_tool(
        &ctx,
        "exec_command",
        &json!({"cmd":"python3 -c \"import os, signal; os.kill(os.getpid(), signal.SIGTERM)\"","yield_time_ms":1000}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["operation_outcome"], "signal");
    assert_eq!(result["command_ok"], false);
}

#[test]
fn chained_patch_moves_preserve_content_and_source_permissions() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a"), "executable content\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(ws.path().join("a"), fs::Permissions::from_mode(0o751)).unwrap();
    }
    let result = call_tool(
        &ctx,
        "apply_patch",
        &json!({"patch":"*** Begin Patch\n*** Update File: a\n*** Move to: b\n@@\n executable content\n*** Update File: b\n*** Move to: c\n@@\n executable content\n*** End Patch\n"}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert!(!ws.path().join("a").exists());
    assert!(!ws.path().join("b").exists());
    assert_eq!(
        fs::read_to_string(ws.path().join("c")).unwrap(),
        "executable content\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(ws.path().join("c"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
    }
}

#[test]
fn all_six_actions_and_original_line_numbers_work() {
    let (ws, _harness, ctx) = context();
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"create","path":"a","content":"\u{feff}one\r\ntwo\r\nthree\r\n"}]})
        )["ok"],
        true
    );
    let before = read(&ctx, "a");
    let edits = json!({"changes":[{"action":"edit","path":"a","revision":before["revision"],"edits":[
        {"op":"insert_before","line":1,"content":"zero"},{"op":"replace","start_line":2,"end_line":2,"content":"changed"},
        {"op":"insert_after","line":3,"content":"four"}]}]});
    assert_eq!(call_tool(&ctx, "apply_changes", &edits)["ok"], true);
    assert_eq!(
        fs::read_to_string(ws.path().join("a")).unwrap(),
        "\u{feff}zero\r\none\r\nchanged\r\nthree\r\nfour\r\n"
    );
    let a = read(&ctx, "a");
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"copy","path":"a","to":"b","revision":a["revision"]}]})
        )["ok"],
        true
    );
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"move","path":"a","to":"c","revision":a["revision"]}]})
        )["ok"],
        true
    );
    assert!(!ws.path().join("a").exists());
    let b = read(&ctx, "b");
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"write","path":"b","content":"updated","revision":b["revision"]}]})
        )["ok"],
        true
    );
    let b = read(&ctx, "b");
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"delete","path":"b","revision":b["revision"]}]})
        )["ok"],
        true
    );
    assert!(!ws.path().join("b").exists());
}
#[test]
fn overlapping_edits_and_duplicate_paths_are_rejected_without_side_effects() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a"), "one\ntwo\nthree\n").unwrap();
    let before = read(&ctx, "a");
    let result = call_tool(
        &ctx,
        "apply_changes",
        &json!({"changes":[{"action":"edit","path":"a","revision":before["revision"],"edits":[
        {"op":"delete","start_line":1,"end_line":2},{"op":"replace","start_line":2,"end_line":3,"content":"new"}]}]}),
    );
    assert_eq!(result["error"]["code"], "PATCH_HUNKS_OVERLAP");
    assert_eq!(read(&ctx, "a")["revision"], before["revision"]);
    let duplicate = call_tool(
        &ctx,
        "apply_changes",
        &json!({"changes":[{"action":"create","path":"b","content":"x"},{"action":"create","path":"./b","content":"y"}]}),
    );
    assert_eq!(duplicate["ok"], false);
    assert!(!ws.path().join("b").exists());
}
#[test]
fn dry_runs_noops_and_new_keys_cannot_reset_the_repeat_breaker() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a"), "same").unwrap();
    let before = read(&ctx, "a");
    let missing = json!({"path":"missing"});
    for _ in 0..2 {
        assert_eq!(read(&ctx, "missing")["error"]["code"], "NOT_FOUND");
    }
    let args = json!({"changes":[{"action":"write","path":"a","revision":before["revision"],"content":"same"}]});
    assert_eq!(
        call_tool(&ctx, "apply_changes", &args)["workspace_changed"],
        false
    );
    assert_eq!(
        call_tool(&ctx, "exec_command", &json!({"cmd":"pwd"}))["workspace_may_have_changed"],
        false
    );
    assert_eq!(
        call_tool(&ctx, "read_file", &missing)["error"]["code"],
        "REPEATED_CALL_BLOCKED"
    );
    let dry = json!({"idempotency_key":"dry","dry_run":true,"changes":[{"action":"create","path":"b","content":"b"}]});
    assert_eq!(call_tool(&ctx, "apply_changes", &dry)["ok"], true);
    assert!(!ws.path().join("b").exists());
    let mut actual = dry.clone();
    actual["dry_run"] = json!(false);
    assert_eq!(call_tool(&ctx, "apply_changes", &actual)["ok"], true);
    assert_eq!(
        call_tool(&ctx, "read_file", &missing)["error"]["code"],
        "NOT_FOUND"
    );
    let bad = "*** Begin Patch\n*** Update File: a\n@@\n-never\n+other\n*** End Patch\n";
    for key in ["one", "two"] {
        assert_eq!(
            call_tool(
                &ctx,
                "apply_patch",
                &json!({"patch":bad,"idempotency_key":key})
            )["error"]["code"],
            "PATCH_CONTEXT_NOT_FOUND"
        );
    }
    assert_eq!(
        call_tool(
            &ctx,
            "apply_patch",
            &json!({"patch":bad,"idempotency_key":"three"})
        )["error"]["code"],
        "REPEATED_CALL_BLOCKED"
    );
}
#[test]
fn copy_does_not_rewrite_a_readonly_source() {
    let (ws, _harness, ctx) = context();
    let path = ws.path().join("source");
    fs::write(&path, "copy me").unwrap();
    let before = read(&ctx, "source");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let result = call_tool(
        &ctx,
        "apply_changes",
        &json!({"changes":[{"action":"copy","path":"source","to":"destination","revision":before["revision"]}]}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    assert_eq!(
        fs::read_to_string(ws.path().join("destination")).unwrap(),
        "copy me"
    );
    #[cfg(windows)]
    for file in ["source", "destination"] {
        let path = ws.path().join(file);
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
    }
}
#[test]
fn simultaneous_keys_share_one_execution_across_transport_contexts() {
    let (ws, harness, ctx) = context();
    let other = ToolContext::for_test(ws.path().into(), harness.path().into()).unwrap();
    let contexts = [Arc::new(ctx), Arc::new(other)];
    let barrier = Arc::new(Barrier::new(8));
    let args = json!({"idempotency_key":"same","changes":[{"action":"create","path":"a","content":"one execution"}]});
    let results = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|i| {
                let ctx = contexts[i % 2].clone();
                let barrier = barrier.clone();
                let args = args.clone();
                scope.spawn(move || {
                    barrier.wait();
                    call_tool(&ctx, "apply_changes", &args)
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(results.iter().all(|r| r["ok"] == true), "{results:?}");
    assert_eq!(
        results
            .iter()
            .filter(|r| r["idempotent_replay"] == true)
            .count(),
        7
    );
    assert_eq!(
        fs::read_to_string(ws.path().join("a")).unwrap(),
        "one execution"
    );
}
#[test]
fn default_cwd_is_applied_once_to_structured_and_codex_edits() {
    let (ws, _harness, ctx) = context();
    fs::create_dir(ws.path().join("sub")).unwrap();
    assert_eq!(
        call_tool(&ctx, "set_default_cwd", &json!({"path":"sub"}))["ok"],
        true
    );
    assert_eq!(
        call_tool(
            &ctx,
            "apply_changes",
            &json!({"changes":[{"action":"create","path":"a","content":"old\n"}]})
        )["ok"],
        true
    );
    assert_eq!(
        call_tool(
            &ctx,
            "apply_patch",
            &json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-old\n+new\n*** End Patch\n"})
        )["ok"],
        true
    );
    assert_eq!(
        fs::read_to_string(ws.path().join("sub/a")).unwrap(),
        "new\n"
    );
    assert!(!ws.path().join("sub/sub").exists());
}
#[test]
fn anchors_eof_and_match_grades_reject_ambiguous_edits() {
    let (ws, _harness, ctx) = context();
    fs::write(ws.path().join("a"), "first\nold\nsecond\nold\n").unwrap();
    let ambiguity = call_tool(
        &ctx,
        "apply_patch",
        &json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-old\n+new\n*** End Patch\n"}),
    );
    assert_eq!(ambiguity["error"]["code"], "PATCH_CONTEXT_AMBIGUOUS");
    assert_eq!(
        ambiguity["error"]["details"]["candidate_lines"],
        json!([2, 4])
    );
    assert_eq!(
        call_tool(
            &ctx,
            "apply_patch",
            &json!({"patch":"*** Begin Patch\n*** Update File: a\n@@ second\n-old\n+new\n*** End of File\n*** Move to: b\n*** End Patch\n"})
        )["ok"],
        true
    );
    assert!(!ws.path().join("a").exists());
    assert_eq!(
        fs::read_to_string(ws.path().join("b")).unwrap(),
        "first\nold\nsecond\nnew\n"
    );
    // A single added line without unchanged context must never masquerade as replay.
    assert_eq!(
        call_tool(
            &ctx,
            "apply_patch",
            &json!({"patch":"*** Begin Patch\n*** Update File: b\n@@\n-old\n+new\n*** End Patch\n"})
        )["ok"],
        true
    );
}
#[test]
fn untracked_diff_defaults_to_included_and_can_be_disabled() {
    let (ws, _harness, ctx) = context();
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(ws.path())
        .output()
        .unwrap();
    assert!(git.status.success());
    fs::write(ws.path().join("untracked.txt"), "new text\n").unwrap();
    assert!(call_tool(&ctx, "git_diff", &json!({}))["diff"]
        .as_str()
        .unwrap()
        .contains("+new text"));
    assert_eq!(
        call_tool(&ctx, "git_diff", &json!({"include_untracked":false}))["diff"],
        ""
    );
}
#[test]
fn commands_report_truth_retain_output_and_count_terminal_outcomes_once() {
    let (_ws, _harness, ctx) = context();
    let command = call_tool(
        &ctx,
        "exec_command",
        &json!({"cmd":"python -c \"print('retained'); raise SystemExit(7)\"","yield_time_ms":1000}),
    );
    assert_eq!(command["ok"], true, "{command}");
    assert_eq!(command["command_ok"], false);
    assert_eq!(command["operation_outcome"], "exited_nonzero");
    let output_ref = command["output_refs"]["stdout"].clone();
    for _ in 0..3 {
        assert!(
            call_tool(&ctx, "read_output", &json!({"output_ref":output_ref}))["content"]
                .as_str()
                .unwrap()
                .contains("retained")
        );
    }
    let diagnostics = ctx.reliability.diagnostics();
    assert_eq!(
        diagnostics["operation_failure_streaks"]["exec_command:exited_nonzero"],
        1
    );
}
#[test]
fn mcp_catalog_gates_permissions_and_exposes_structured_contracts() {
    let (_ws, _harness, ctx) = context();
    let catalog = coding_tools_mcp_desktop_lib::tools::registry::list_tools_for_context(&ctx);
    assert!(!catalog
        .iter()
        .any(|tool| tool["name"] == "request_permissions"));
    let edits = catalog
        .iter()
        .find(|tool| tool["name"] == "apply_changes")
        .unwrap();
    assert!(edits["outputSchema"]["properties"]["affected_files"].is_object());
    let read = call_tool(
        &ctx,
        "request_permissions",
        &json!({"tool_name":"apply_patch","permission":"network_access","reason":"test","arguments":{}}),
    );
    assert_eq!(read["error"]["code"], "ELICITATION_UNSUPPORTED");
}
#[cfg(windows)]
#[test]
fn commit_failure_restores_earlier_files_and_removes_staging_artifacts() {
    let (ws, _harness, ctx) = context();
    for path in ["a", "b"] {
        fs::write(ws.path().join(path), "old").unwrap();
    }
    let a = read(&ctx, "a");
    let b = read(&ctx, "b");
    let path = ws.path().join("b");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let result = call_tool(
        &ctx,
        "apply_changes",
        &json!({"changes":[{"action":"write","path":"a","content":"new","revision":a["revision"]},{"action":"write","path":"b","content":"new","revision":b["revision"]}]}),
    );
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(fs::read_to_string(ws.path().join("a")).unwrap(), "old");
    assert_eq!(fs::read_to_string(ws.path().join("b")).unwrap(), "old");
    assert_eq!(fs::read_dir(ws.path()).unwrap().count(), 2);
}
