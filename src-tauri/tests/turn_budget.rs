mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use coding_tools_mcp_desktop_lib::mcp::turn_budget::{
    get_tool_traits, is_wrap_up_verification_command, AgentTurnBudgetConfig,
    AgentTurnBudgetManager, CallDecision, MockBudgetClock, SystemBudgetClock,
    TurnBudgetStatus,
};
use coding_tools_mcp_desktop_lib::mcp::handle_request;
use coding_tools_mcp_desktop_lib::tools::ToolContext;
use serde_json::json;

fn test_context_with_mock_budget(
    config: AgentTurnBudgetConfig,
    clock: Arc<MockBudgetClock>,
) -> (tempfile::TempDir, tempfile::TempDir, ToolContext, Arc<AgentTurnBudgetManager>) {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let harness = tempfile::tempdir().expect("harness tempdir");
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock));
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("tool context")
        .with_turn_budget_manager(manager.clone());
    (workspace, harness, ctx, manager)
}

#[test]
fn test_tool_traits_and_command_classification() {
    let empty_args = json!({});

    // 1. 探索型工具
    let st_traits = get_tool_traits("search_text", false, &empty_args);
    assert!(st_traits.investigation);
    assert!(!st_traits.wrap_up_allowed);
    assert!(!st_traits.finalization_safe);

    // 2. 外部 MCP 工具
    let ext_traits = get_tool_traits("fast_context_search", true, &empty_args);
    assert!(ext_traits.investigation);
    assert!(!ext_traits.wrap_up_allowed);
    assert!(!ext_traits.finalization_safe);

    // 3. 修改型工具
    let patch_traits = get_tool_traits("apply_patch", false, &empty_args);
    assert!(patch_traits.mutating);
    assert!(patch_traits.wrap_up_allowed);
    assert!(!patch_traits.finalization_safe);

    // 4. 收尾型与只读工具
    let rf_traits = get_tool_traits("read_file", false, &empty_args);
    assert!(!rf_traits.investigation);
    assert!(rf_traits.wrap_up_allowed);
    assert!(rf_traits.finalization_safe);

    let ck_traits = get_tool_traits("history_session_checkpoint", false, &empty_args);
    assert!(ck_traits.mutating);
    assert!(ck_traits.wrap_up_allowed);
    assert!(ck_traits.finalization_safe);

    let ft_traits = get_tool_traits("finish_task", false, &empty_args);
    assert!(ft_traits.mutating);
    assert!(ft_traits.wrap_up_allowed);
    assert!(ft_traits.finalization_safe);

    // 5. 命令智能分类
    assert!(is_wrap_up_verification_command("cargo check"));
    assert!(is_wrap_up_verification_command("cargo test --test turn_budget"));
    assert!(is_wrap_up_verification_command("npm run check"));
    assert!(is_wrap_up_verification_command("npm run build"));
    assert!(is_wrap_up_verification_command("git status"));
    assert!(is_wrap_up_verification_command("git diff"));
    assert!(is_wrap_up_verification_command("python --version"));

    assert!(!is_wrap_up_verification_command("npm install"));
    assert!(!is_wrap_up_verification_command("pip install torch"));
    assert!(!is_wrap_up_verification_command("cargo install cargo-watch"));
    assert!(!is_wrap_up_verification_command("git clone https://github.com/a/b.git"));
    assert!(!is_wrap_up_verification_command("cargo check && cargo build"));
}

#[test]
fn test_turn_budget_initialization_and_first_turn() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::for_test_ms(
        1000, 1500, 2000, 3000, 200, 500, 800, 4000, 500,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock));

    let decision = manager.start_call("ws-1", Some("session-abc"), "list_files", false, &json!({}));
    match decision {
        CallDecision::Allowed {
            guard,
            runtime_budget,
            snapshot,
            emit_full_warning,
            emit_urgent,
        } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
            assert_eq!(snapshot.elapsed_seconds, 0);
            assert_eq!(snapshot.should_wrap_up, false);
            assert_eq!(snapshot.should_stop_tool_calls, false);
            assert_eq!(snapshot.timer_origin, "first_observed_tool_call");
            assert_eq!(emit_full_warning, false);
            assert_eq!(emit_urgent, false);
            assert_eq!(runtime_budget, Duration::from_millis(2800)); // 3000 - 200

            drop(guard);
        }
        _ => panic!("Expected Allowed decision"),
    }
}

#[test]
fn test_raii_guard_guarantees_active_calls_cleanup() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::for_test_ms(
        1000, 1500, 2000, 3000, 200, 500, 800, 4000, 500,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    let decision = manager.start_call("ws-1", Some("session-xyz"), "read_file", false, &json!({}));
    if let CallDecision::Allowed { guard, .. } = decision {
        {
            let _move_guard = guard;
        }
    }

    clock.advance(Duration::from_millis(100));

    let decision2 = manager.start_call("ws-1", Some("session-xyz"), "read_file", false, &json!({}));
    match decision2 {
        CallDecision::Allowed { guard, snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
            drop(guard);
        }
        _ => panic!("Expected Allowed decision after guard drop"),
    }
}

#[test]
fn test_warning_and_wrap_up_and_finalization_lifecycle() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // 阈值: warning=1000ms, wrap_up=1500ms, finalization=2000ms, hard_stop=3000ms, reserve=200ms(cutoff=2800ms)
    let config = AgentTurnBudgetConfig::for_test_ms(
        1000, 1500, 2000, 3000, 200, 500, 800, 4000, 500,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 1. Normal (0ms)
    let d0 = manager.start_call("ws-1", Some("s-1"), "search_text", false, &json!({}));
    assert!(matches!(d0, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, snapshot, .. } = d0 {
        assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
        drop(guard);
    }

    // 2. Warning 阶段 (1100ms)
    clock.advance(Duration::from_millis(1100));
    // (a) search_text 仍允许，但记录 warning
    let d_warn_st = manager.start_call("ws-1", Some("s-1"), "search_text", false, &json!({}));
    assert!(matches!(d_warn_st, CallDecision::Allowed { emit_full_warning: true, .. }));
    if let CallDecision::Allowed { guard, snapshot, .. } = d_warn_st {
        assert_eq!(snapshot.status, TurnBudgetStatus::Warning);
        drop(guard);
    }
    // (b) fast_context 外部工具在 Warning 阶段仍允许
    let d_warn_ext = manager.start_call("ws-1", Some("s-1"), "fast_context_search", true, &json!({}));
    assert!(matches!(d_warn_ext, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_warn_ext {
        drop(guard);
    }

    // 3. WRAP_UP 阶段 (1600ms): 禁止 investigation 与外部工具，允许 apply_patch 与短验证 exec
    clock.advance(Duration::from_millis(500)); // total: 1600ms
    // (a) search_text 被拒绝
    let d_wrap_st = manager.start_call("ws-1", Some("s-1"), "search_text", false, &json!({}));
    match d_wrap_st {
        CallDecision::Restricted { error_payload, content_text, snapshot } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::WrapUp);
            assert_eq!(error_payload["error"]["code"], "AGENT_TURN_WRAP_UP_RESTRICTED");
            assert_eq!(error_payload["error"]["category"], "turn_budget");
            assert_eq!(error_payload["error"]["retryable"], false);
            assert!(content_text.contains("[TURN BUDGET RESTRICTED]"));
        }
        _ => panic!("Expected Restricted for search_text in WRAP_UP"),
    }
    // (b) fast_context 外部工具被拒绝
    let d_wrap_ext = manager.start_call("ws-1", Some("s-1"), "fast_context_search", true, &json!({}));
    assert!(matches!(d_wrap_ext, CallDecision::Restricted { .. }));
    // (c) grep_text 被拒绝
    let d_wrap_grep = manager.start_call("ws-1", Some("s-1"), "grep_text", false, &json!({}));
    assert!(matches!(d_wrap_grep, CallDecision::Restricted { .. }));
    // (d) list_files 被拒绝
    let d_wrap_lf = manager.start_call("ws-1", Some("s-1"), "list_files", false, &json!({}));
    assert!(matches!(d_wrap_lf, CallDecision::Restricted { .. }));
    // (e) apply_patch 允许完成最后修改
    let d_wrap_patch = manager.start_call("ws-1", Some("s-1"), "apply_patch", false, &json!({}));
    assert!(matches!(d_wrap_patch, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_wrap_patch {
        drop(guard);
    }
    // (f) read_file 允许
    let d_wrap_rf = manager.start_call("ws-1", Some("s-1"), "read_file", false, &json!({}));
    assert!(matches!(d_wrap_rf, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_wrap_rf {
        drop(guard);
    }
    // (g) 验证型 exec 允许
    let d_wrap_exec = manager.start_call("ws-1", Some("s-1"), "exec_command", false, &json!({ "cmd": "cargo check" }));
    assert!(matches!(d_wrap_exec, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, runtime_budget, .. } = d_wrap_exec {
        assert!(runtime_budget <= Duration::from_secs(30));
        drop(guard);
    }
    // (h) 非验证型 exec (如 npm install) 拒绝
    let d_wrap_install = manager.start_call("ws-1", Some("s-1"), "exec_command", false, &json!({ "cmd": "npm install" }));
    assert!(matches!(d_wrap_install, CallDecision::Restricted { .. }));

    // 4. FINALIZATION 阶段 (2100ms): 严禁 apply_patch 与 exec，仅允许只读收尾
    clock.advance(Duration::from_millis(500)); // total: 2100ms
    // (a) apply_patch 拒绝
    let d_fin_patch = manager.start_call("ws-1", Some("s-1"), "apply_patch", false, &json!({}));
    assert!(matches!(d_fin_patch, CallDecision::Restricted { .. }));
    // (b) exec_command 拒绝
    let d_fin_exec = manager.start_call("ws-1", Some("s-1"), "exec_command", false, &json!({ "cmd": "cargo check" }));
    assert!(matches!(d_fin_exec, CallDecision::Restricted { .. }));
    // (c) search_text 拒绝
    let d_fin_st = manager.start_call("ws-1", Some("s-1"), "search_text", false, &json!({}));
    assert!(matches!(d_fin_st, CallDecision::Restricted { .. }));
    // (d) read_file 允许
    let d_fin_rf = manager.start_call("ws-1", Some("s-1"), "read_file", false, &json!({}));
    assert!(matches!(d_fin_rf, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_fin_rf {
        drop(guard);
    }
    // (e) git_status 允许
    let d_fin_gs = manager.start_call("ws-1", Some("s-1"), "git_status", false, &json!({}));
    assert!(matches!(d_fin_gs, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_fin_gs {
        drop(guard);
    }
    // (f) git_diff 允许
    let d_fin_gd = manager.start_call("ws-1", Some("s-1"), "git_diff", false, &json!({}));
    assert!(matches!(d_fin_gd, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_fin_gd {
        drop(guard);
    }
    // (g) history_session_checkpoint 允许
    let d_fin_chk = manager.start_call("ws-1", Some("s-1"), "history_session_checkpoint", false, &json!({}));
    assert!(matches!(d_fin_chk, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_fin_chk {
        drop(guard);
    }
    // (h) finish_task 允许
    let d_fin_ft = manager.start_call("ws-1", Some("s-1"), "finish_task", false, &json!({}));
    assert!(matches!(d_fin_ft, CallDecision::Allowed { .. }));
    if let CallDecision::Allowed { guard, .. } = d_fin_ft {
        drop(guard);
    }

    // 5. DISPATCH_CUTOFF (2850ms, cutoff=2800ms)
    clock.advance(Duration::from_millis(750)); // total: 2850ms
    let d_cutoff = manager.start_call("ws-1", Some("s-1"), "read_file", false, &json!({}));
    match d_cutoff {
        CallDecision::Blocked { error_payload, .. } => {
            assert_eq!(error_payload["error"]["code"], "AGENT_TURN_BUDGET_DISPATCH_CUTOFF");
        }
        _ => panic!("Expected Blocked DispatchCutoff"),
    }

    // 6. HARD_STOP (3100ms >= 3000ms)
    clock.advance(Duration::from_millis(250)); // total: 3100ms
    let d_hs = manager.start_call("ws-1", Some("s-1"), "read_file", false, &json!({}));
    match d_hs {
        CallDecision::Blocked { error_payload, snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::HardStop);
            assert_eq!(error_payload["error"]["code"], "AGENT_TURN_BUDGET_EXHAUSTED");
            assert_eq!(error_payload["error"]["category"], "turn_budget");
            assert_eq!(error_payload["error"]["retryable"], false);
            assert!(error_payload["error"]["recovery_hint"].as_str().unwrap().contains("DO NOT RETRY THIS TOOL"));
        }
        _ => panic!("Expected Blocked HardStop"),
    }
}

#[test]
fn test_dynamic_idle_reset_matrix() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::default();
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 1. 早期 (<20m): 90s idle 重置
    {
        let d1 = manager.start_call("ws-1", Some("session-early"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, .. } = d1 {
            drop(guard);
        }
        clock.advance(Duration::from_secs(91));
        let d2 = manager.start_call("ws-1", Some("session-early"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, snapshot, .. } = d2 {
            assert_eq!(snapshot.elapsed_seconds, 0);
            drop(guard);
        }
    }

    // 2. 中期 (20m~25m): 180s idle 重置
    {
        let d1 = manager.start_call("ws-1", Some("session-mid"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, .. } = d1 {
            drop(guard);
        }
        // 每隔 60s 推进一次，连续运行到 21 分钟（1260s，保证中间没有单次超过 90s 空闲）
        for _ in 0..21 {
            clock.advance(Duration::from_secs(60));
            let d = manager.start_call("ws-1", Some("session-mid"), "read_file", false, &json!({}));
            if let CallDecision::Allowed { guard, .. } = d {
                drop(guard);
            }
        }
        clock.advance(Duration::from_secs(50)); // 1260s + 50s = 1310s (21m50s): 50s < 180s 不重置
        let d2 = manager.start_call("ws-1", Some("session-mid"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, snapshot, .. } = d2 {
            assert!(snapshot.elapsed_seconds >= 21 * 60);
            drop(guard);
        }
        clock.advance(Duration::from_secs(185)); // 1310s + 185s = 1495s < 1500s: 185s >= 180s 重置
        let d3 = manager.start_call("ws-1", Some("session-mid"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, snapshot, .. } = d3 {
            assert_eq!(snapshot.elapsed_seconds, 0);
            drop(guard);
        }
    }

    // 3. 晚期 (>=25m Warning 阶段): 严禁普通 idle 重置
    {
        let d1 = manager.start_call("ws-1", Some("session-late"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, .. } = d1 {
            drop(guard);
        }
        // 推进到 26 分钟
        for _ in 0..26 {
            clock.advance(Duration::from_secs(60));
            let d = manager.start_call("ws-1", Some("session-late"), "read_file", false, &json!({}));
            if let CallDecision::Allowed { guard, .. } = d {
                drop(guard);
            }
        }
        clock.advance(Duration::from_secs(200)); // 即使停顿 200s 也不重置
        let d2 = manager.start_call("ws-1", Some("session-late"), "read_file", false, &json!({}));
        if let CallDecision::Allowed { guard, snapshot, .. } = d2 {
            assert!(snapshot.elapsed_seconds >= 26 * 60);
            drop(guard);
        }
    }
}

#[test]
fn test_hard_stop_and_platform_isolation_recovery() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::default();
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 启动初始调用
    let d1 = manager.start_call("ws-1", Some("session-hs"), "read_file", false, &json!({}));
    if let CallDecision::Allowed { guard, .. } = d1 {
        drop(guard);
    }

    // 推进到 29:05 触发 HardStop
    clock.advance(Duration::from_secs(29 * 60 + 5));
    let d_hs = manager.start_call("ws-1", Some("session-hs"), "read_file", false, &json!({}));
    assert!(matches!(d_hs, CallDecision::Blocked { .. }));

    // 推进 20s (至 29:25): 严禁在此刻重置！
    clock.advance(Duration::from_secs(20));
    let d_retry = manager.start_call("ws-1", Some("session-hs"), "read_file", false, &json!({}));
    assert!(matches!(d_retry, CallDecision::Blocked { .. }));

    // 推进到 30m16s (超过 30m + 15s 隔离线): 必须成功重置为新 Turn
    clock.advance(Duration::from_secs(46)); // 29m25s + 46s = 30m11s
    clock.advance(Duration::from_secs(10)); // 30m21s
    let d_recovered = manager.start_call("ws-1", Some("session-hs"), "read_file", false, &json!({}));
    match d_recovered {
        CallDecision::Allowed { guard, snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
            assert_eq!(snapshot.elapsed_seconds, 0);
            drop(guard);
        }
        _ => panic!("Expected Recovery after platform isolation"),
    }
}

#[test]
fn test_top_level_meta_injection_preserves_external_structured_content() {
    let config = AgentTurnBudgetConfig::default();
    let clock = Arc::new(SystemBudgetClock);
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock));

    let snapshot = coding_tools_mcp_desktop_lib::mcp::turn_budget::TurnBudgetSnapshot {
        status: TurnBudgetStatus::Warning,
        elapsed_seconds: 1501,
        remaining_seconds: 239,
        should_wrap_up: true,
        should_stop_tool_calls: false,
        timer_origin: "first_observed_tool_call",
    };

    let original_external_result = json!({
        "content": [{
            "type": "text",
            "text": "fast context results"
        }],
        "structuredContent": {
            "matches": ["sym1", "sym2"],
            "strict_field": 123
        },
        "isError": false
    });

    let decorated = manager.decorate_allowed_result(original_external_result, &snapshot, true);

    // 验证 structuredContent 完全保持原样
    assert_eq!(decorated["structuredContent"]["matches"][0], "sym1");
    assert_eq!(decorated["structuredContent"]["strict_field"], 123);

    // 验证顶层 _meta 注入
    let meta = &decorated["_meta"]["coding-tools/agentTurnBudget"];
    assert_eq!(meta["status"], "warning");
    assert_eq!(meta["elapsedSeconds"], 1501);
    assert_eq!(meta["shouldWrapUp"], true);

    // 验证 content[0].text 被前置注入 WARNING 文本
    let text = decorated["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("[TURN BUDGET WARNING]"));
    assert!(text.ends_with("fast context results"));
}

#[test]
fn test_end_to_end_jsonrpc_tools_call_lifecycle() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::for_test_ms(
        1000, 1500, 2000, 3000, 200, 500, 800, 4000, 500,
    );
    let (_ws, _harness, ctx, _mgr) = test_context_with_mock_budget(config, clock.clone());
    let shared_ctx = Arc::new(ctx);

    let session_meta = json!({
        "openai/session": "chatgpt-client-session-123"
    });

    // 1. Normal 调用
    let req0 = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "list_files",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res0 = handle_request(&shared_ctx, &req0);
    assert_eq!(res0["id"], 1);
    assert_eq!(res0["result"]["isError"], false);
    assert_eq!(
        res0["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "normal"
    );

    // 2. 推进到 WRAP_UP 阶段 (1600ms): 调用 search_text 被短路阻断
    clock.advance(Duration::from_millis(1600));
    let req_wrap = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "search_text",
            "arguments": { "query": "test" },
            "_meta": session_meta
        }
    });
    let res_wrap = handle_request(&shared_ctx, &req_wrap);
    assert_eq!(res_wrap["id"], 2);
    assert_eq!(res_wrap["result"]["isError"], true);
    assert_eq!(
        res_wrap["result"]["structuredContent"]["error"]["code"],
        "AGENT_TURN_WRAP_UP_RESTRICTED"
    );
    assert_eq!(
        res_wrap["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "wrap_up"
    );

    // 3. 推进到 HARD_STOP (3100ms)
    clock.advance(Duration::from_millis(1500));
    let req_hs = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "read_file",
            "arguments": { "path": "nonexistent.txt" },
            "_meta": session_meta
        }
    });
    let res_hs = handle_request(&shared_ctx, &req_hs);
    assert_eq!(res_hs["id"], 3);
    assert_eq!(res_hs["result"]["isError"], true);
    assert_eq!(
        res_hs["result"]["structuredContent"]["error"]["code"],
        "AGENT_TURN_BUDGET_EXHAUSTED"
    );
    assert_eq!(
        res_hs["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "hard_stop"
    );
}
