mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use coding_tools_mcp_desktop_lib::mcp::turn_budget::{
    AgentTurnBudgetConfig, AgentTurnBudgetManager, BudgetClock, CallDecision, MockBudgetClock, TurnBudgetStatus,
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
fn test_turn_budget_initialization_and_first_turn() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::for_test_ms(
        1000, 2000, 3000, 200, 500, 800, 4000, 500,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock));

    let decision = manager.start_call("ws-1", Some("session-abc"));
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
        1000, 2000, 3000, 200, 500, 800, 4000, 500,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    let decision = manager.start_call("ws-1", Some("session-xyz"));
    if let CallDecision::Allowed { guard, .. } = decision {
        // 在作用域内，模拟发生异常/Early Return
        {
            let _move_guard = guard;
            // 退出作用域自动 drop，调用 complete_call
        }
    }

    // 时间前移 100ms
    clock.advance(Duration::from_millis(100));

    // 再次调用，确认 active_calls 已归 0，并且可以正常处理
    let decision2 = manager.start_call("ws-1", Some("session-xyz"));
    match decision2 {
        CallDecision::Allowed { guard, snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
            drop(guard);
        }
        _ => panic!("Expected Allowed decision after guard drop"),
    }
}

#[test]
fn test_dynamic_idle_reset_matrix() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // warning: 25m, urgent: 28m, hard_stop: 29m
    let config = AgentTurnBudgetConfig::default();
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 1. 早期 (<20m): 90s idle 重置
    {
        let d = manager.start_call("ws-1", Some("sess-1"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
    }
    // 60s 后再次调用（< 90s），属于同一 turn
    clock.advance(Duration::from_secs(60));
    {
        let d = manager.start_call("ws-1", Some("sess-1"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.elapsed_seconds, 60);
            drop(guard);
        }
    }
    // 95s 后（距离上次调用 60s 点流逝 95s），超过 early_idle_reset (90s)，重置新 turn
    clock.advance(Duration::from_secs(95));
    {
        let d = manager.start_call("ws-1", Some("sess-1"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.elapsed_seconds, 0); // 重置为 0
            drop(guard);
        }
    }

    // 2. 中期 (20m~25m): 90s 不重置，180s 重置
    {
        let d = manager.start_call("ws-1", Some("sess-mid"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
    }
    // 每隔 60s 推进一次调用，平滑推进 20 次，达到 20m (1200s)
    for _ in 0..20 {
        clock.advance(Duration::from_secs(60));
        let d = manager.start_call("ws-1", Some("sess-mid"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
    }
    // 此时处于中期 (20m)。过了 100s（>90s 但 <180s），不应该重置 (总用时 21m40s = 1300s)
    clock.advance(Duration::from_secs(100));
    {
        let d = manager.start_call("ws-1", Some("sess-mid"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.elapsed_seconds, 20 * 60 + 100);
            drop(guard);
        }
    }
    // 过了 190s（>180s），且总耗时 1300s + 190s = 1490s (< 1500s 仍在中期)，应该重置为新 turn
    clock.advance(Duration::from_secs(190));
    {
        let d = manager.start_call("ws-1", Some("sess-mid"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.elapsed_seconds, 0);
            drop(guard);
        }
    }

    // 3. 晚期 (>=25m Warning 阶段): 严禁普通 idle 重置
    {
        let d = manager.start_call("ws-1", Some("sess-warn"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
    }
    // 每隔 60s 推进一次调用，直到 26m
    for _ in 0..26 {
        clock.advance(Duration::from_secs(60));
        let d = manager.start_call("ws-1", Some("sess-warn"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
    }
    // 达到 26m，进入 Warning
    {
        let d = manager.start_call("ws-1", Some("sess-warn"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.status, TurnBudgetStatus::Warning);
            drop(guard);
        }
    }
    // 即使空闲了 300s，在未跨越平台隔离线前，绝对不能仅靠普通 idle 变成新 turn
    clock.advance(Duration::from_secs(300));
    {
        let d = manager.start_call("ws-1", Some("sess-warn"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.status, TurnBudgetStatus::Warning);
            assert_eq!(snapshot.elapsed_seconds, 26 * 60 + 300);
            drop(guard);
        }
    }
}

#[test]
fn test_hard_stop_and_platform_isolation_recovery() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // 阈值：warning=25s, urgent=28s, hard_stop=29s, reserve=1s, platform_limit=30s, margin=5s
    let config = AgentTurnBudgetConfig::for_test_ms(
        25_000, 28_000, 29_000, 1_000, 5_000, 8_000, 30_000, 5_000,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 0s 初始化并每隔 3s 推进一次（保证未超过 5s 的 early_idle_reset），直到 27s
    for _ in 0..9 {
        let d = manager.start_call("ws-1", Some("sess-hs"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
        clock.advance(Duration::from_millis(3_000));
    }

    // 再前移 2.5s，到达 29.5s HardStop 阶段
    clock.advance(Duration::from_millis(2_500));
    {
        let d = manager.start_call("ws-1", Some("sess-hs"));
        match d {
            CallDecision::Blocked { snapshot, error_payload, content_text } => {
                assert_eq!(snapshot.status, TurnBudgetStatus::HardStop);
                assert!(content_text.contains("[TURN BUDGET HARD STOP]"));
                assert_eq!(error_payload["error"]["code"], "AGENT_TURN_BUDGET_EXHAUSTED");
            }
            _ => panic!("Expected Blocked(HardStop)"),
        }
    }

    // 31s 时重试（HardStop 后仅过了 1.5s，且总耗时 31s < 30s + 5s = 35s 平台隔离线）
    // 必须继续拒绝！
    clock.advance(Duration::from_millis(1_500));
    {
        let d = manager.start_call("ws-1", Some("sess-hs"));
        match d {
            CallDecision::Blocked { snapshot, .. } => {
                assert_eq!(snapshot.status, TurnBudgetStatus::HardStop);
            }
            _ => panic!("Expected continuous Blocked(HardStop) before platform isolation line"),
        }
    }

    // 前移至 36s（跨过 30s + 5s = 35s 平台隔离线），下次调用自动创建新 Turn
    clock.advance(Duration::from_millis(5_000));
    {
        let d = manager.start_call("ws-1", Some("sess-hs"));
        match d {
            CallDecision::Allowed { guard, snapshot, .. } => {
                assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
                assert_eq!(snapshot.elapsed_seconds, 0); // 成功重置为新 turn
                drop(guard);
            }
            _ => panic!("Expected recovery and new turn after platform isolation deadline"),
        }
    }
}

#[test]
fn test_concurrent_warning_emits_full_warning_only_once() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // warning=10s, urgent=20s, hard_stop=30s
    let config = AgentTurnBudgetConfig::for_test_ms(
        10_000, 20_000, 30_000, 1_000, 5_000, 5_000, 40_000, 5_000,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 0s 初始化并每隔 3s 推进，直到 9s
    for _ in 0..3 {
        let d = manager.start_call("ws-1", Some("sess-concur"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
        clock.advance(Duration::from_millis(3_000));
    }

    // 前移 3s 到 12s（处于 10s Warning 阶段），两个并发请求同时发生
    clock.advance(Duration::from_millis(3_000));
    let d1 = manager.start_call("ws-1", Some("sess-concur"));
    let d2 = manager.start_call("ws-1", Some("sess-concur"));

    let (emit1, emit2) = match (d1, d2) {
        (
            CallDecision::Allowed { guard: g1, emit_full_warning: e1, .. },
            CallDecision::Allowed { guard: g2, emit_full_warning: e2, .. },
        ) => {
            drop(g1);
            drop(g2);
            (e1, e2)
        }
        _ => panic!("Expected both Allowed"),
    };

    // 必须有且仅有一个请求得到完整的 emit_full_warning
    assert!(
        (emit1 && !emit2) || (!emit1 && emit2),
        "Exactly one concurrent call must receive emit_full_warning=true"
    );
}

#[test]
fn test_top_level_meta_injection_preserves_external_structured_content() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let config = AgentTurnBudgetConfig::for_test_ms(
        10_000, 20_000, 30_000, 1_000, 5_000, 5_000, 40_000, 5_000,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock));

    let decision = manager.start_call("ws-1", Some("sess-meta"));
    if let CallDecision::Allowed { guard, snapshot, emit_full_warning, .. } = decision {
        let external_raw_result = json!({
            "content": [{
                "type": "text",
                "text": "fast-context search result"
            }],
            "structuredContent": {
                "customSchemaField": 42,
                "strictNestedArray": ["a", "b"]
            },
            "isError": false
        });

        let decorated = manager.decorate_allowed_result(external_raw_result.clone(), &snapshot, emit_full_warning);

        // 1. 验证第三方 structuredContent 绝对没有被修改
        assert_eq!(decorated["structuredContent"], external_raw_result["structuredContent"]);

        // 2. 验证机器可读元数据注入在顶层 _meta
        assert_eq!(
            decorated["_meta"]["coding-tools/agentTurnBudget"]["status"],
            "normal"
        );
        assert_eq!(
            decorated["_meta"]["coding-tools/agentTurnBudget"]["timerOrigin"],
            "first_observed_tool_call"
        );

        drop(guard);
    }
}

#[test]
fn test_dispatch_cutoff_semantic_separation() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // warning=25s, urgent=28s, hard_stop=29s, reserve=1s (cutoff=28s)
    let config = AgentTurnBudgetConfig::for_test_ms(
        25_000, 28_000, 29_000, 1_000, 5_000, 8_000, 30_000, 5_000,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 0s 初始化并推进到 27s
    for _ in 0..9 {
        let d = manager.start_call("ws-1", Some("sess-cutoff"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
        clock.advance(Duration::from_millis(3_000));
    }

    // 28.5s（处于 28s cutoff ~ 29s hard_stop 之间）
    clock.advance(Duration::from_millis(1_500));
    {
        let d = manager.start_call("ws-1", Some("sess-cutoff"));
        match d {
            CallDecision::Blocked { snapshot, error_payload, content_text } => {
                assert_eq!(snapshot.status, TurnBudgetStatus::DispatchCutoff);
                assert!(content_text.contains("[TURN BUDGET DISPATCH CUTOFF]"));
                assert_eq!(error_payload["error"]["code"], "AGENT_TURN_BUDGET_DISPATCH_CUTOFF");
            }
            _ => panic!("Expected Blocked(DispatchCutoff)"),
        }
    }
}

#[test]
fn test_rejected_calls_do_not_refresh_timestamps() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // warning=25s, urgent=28s, hard_stop=29s, reserve=1s, platform_limit=30s, margin=5s
    let config = AgentTurnBudgetConfig::for_test_ms(
        25_000, 28_000, 29_000, 1_000, 5_000, 8_000, 30_000, 5_000,
    );
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    for _ in 0..9 {
        let d = manager.start_call("ws-1", Some("sess-reject"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
        clock.advance(Duration::from_millis(3_000));
    }

    // 29.5s HardStop
    clock.advance(Duration::from_millis(2_500));
    let _ = manager.start_call("ws-1", Some("sess-reject"));

    // 疯狂重试 10 次（每隔 500ms 重试一次）
    for _ in 0..10 {
        clock.advance(Duration::from_millis(500));
        let d = manager.start_call("ws-1", Some("sess-reject"));
        assert!(matches!(d, CallDecision::Blocked { .. }));
    }

    // 此时时间为 29.5s + 5.0s = 34.5s
    // 再过 1s 到达 35.5s（跨过 30s + 5s = 35s 平台隔离线）
    clock.advance(Duration::from_millis(1_000));
    let recovery = manager.start_call("ws-1", Some("sess-reject"));
    match recovery {
        CallDecision::Allowed { guard, snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
            assert_eq!(snapshot.elapsed_seconds, 0);
            drop(guard);
        }
        _ => panic!("Expected recovery even after repeated rejection retries"),
    }
}

#[test]
fn test_state_ttl_and_lru_eviction() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    let mut config = AgentTurnBudgetConfig::default();
    config.max_states = 3;
    config.state_ttl = Duration::from_secs(100);

    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));

    // 依次插入 3 条状态，每次推进 1s 确保有严格时间序 (sess-1 最旧)
    for i in 1..=3 {
        let d = manager.start_call("ws-1", Some(&format!("sess-{i}")));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
        clock.advance(Duration::from_secs(1));
    }

    // 插入第 4 条，触发 max_states=3 淘汰严格最旧的 sess-1
    clock.advance(Duration::from_secs(10));
    {
        let d = manager.start_call("ws-1", Some("sess-4"));
        if let CallDecision::Allowed { guard, .. } = d {
            drop(guard);
        }
    }

    // 此时重新访问 sess-1，应当作为全新 turn 重新创建 (turn_seq = 1, elapsed = 0)
    {
        let d = manager.start_call("ws-1", Some("sess-1"));
        if let CallDecision::Allowed { guard, snapshot, .. } = d {
            assert_eq!(snapshot.elapsed_seconds, 0);
            drop(guard);
        }
    }
}

#[test]
fn test_end_to_end_jsonrpc_tools_call_lifecycle() {
    let t0 = Instant::now();
    let clock = Arc::new(MockBudgetClock::new(t0));
    // 阈值：warning=200ms, urgent=400ms, hard_stop=600ms, reserve=50ms
    let config = AgentTurnBudgetConfig::for_test_ms(
        200, 400, 600, 50, 500, 500, 1000, 100,
    );
    let (_workspace, _harness, ctx, _manager) = test_context_with_mock_budget(config, clock.clone());
    let shared_ctx = Arc::new(ctx);

    let session_meta = json!({
        "openai/session": "chatgpt-e2e-session"
    });

    // 1. 阶段 1：Normal 调用
    let normal_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "list_files",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res1 = handle_request(&shared_ctx, &normal_req);
    assert_eq!(res1["id"], 1);
    assert_eq!(res1["result"]["isError"], false);
    assert_eq!(
        res1["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "normal"
    );

    // 2. 阶段 2：Warning 触发 (推进 250ms)
    clock.advance(Duration::from_millis(250));
    let warn_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "list_files",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res2 = handle_request(&shared_ctx, &warn_req);
    assert_eq!(res2["id"], 2);
    assert_eq!(res2["result"]["isError"], false);
    assert_eq!(
        res2["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "warning"
    );
    let text2 = res2["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text2.contains("[TURN BUDGET WARNING]"));

    // 3. 阶段 3：Urgent 触发 (推进 200ms 至 450ms)
    clock.advance(Duration::from_millis(200));
    let urgent_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "list_files",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res3 = handle_request(&shared_ctx, &urgent_req);
    assert_eq!(res3["id"], 3);
    assert_eq!(res3["result"]["isError"], false);
    assert_eq!(
        res3["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "urgent"
    );
    let text3 = res3["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text3.contains("[TURN BUDGET URGENT]"));

    // 4. 阶段 4：HardStop 触发 (推进 200ms 至 650ms)
    clock.advance(Duration::from_millis(200));
    let hs_req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "list_files",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res4 = handle_request(&shared_ctx, &hs_req);
    assert_eq!(res4["id"], 4);
    assert_eq!(res4["result"]["isError"], true);
    assert_eq!(
        res4["result"]["_meta"]["coding-tools/agentTurnBudget"]["status"],
        "hard_stop"
    );
    let text4 = res4["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text4.contains("[TURN BUDGET HARD STOP]"));
    assert_eq!(
        res4["result"]["structuredContent"]["error"]["code"],
        "AGENT_TURN_BUDGET_EXHAUSTED"
    );
}
