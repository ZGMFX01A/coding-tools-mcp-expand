use std::sync::Arc;
use std::time::{Duration, Instant};

use coding_tools_mcp_desktop_lib::mcp::browser_turn::{
    BrowserTurnEvent, BrowserTurnRegistry, CorrelationConfidence, TurnCorrelator, TurnIdentity,
};
use coding_tools_mcp_desktop_lib::mcp::turn_budget::{
    AgentTurnBudgetConfig, AgentTurnBudgetManager, BudgetClock, CallDecision, TurnBudgetStatus,
};
use serde_json::json;

struct TestClock {
    now: std::sync::atomic::AtomicU64,
    base: Instant,
}

impl TestClock {
    fn new() -> Self {
        Self {
            now: std::sync::atomic::AtomicU64::new(0),
            base: Instant::now(),
        }
    }

    fn advance(&self, duration: Duration) {
        self.now.fetch_add(
            duration.as_millis() as u64,
            std::sync::atomic::Ordering::SeqCst,
        );
    }
}

impl coding_tools_mcp_desktop_lib::mcp::turn_budget::BudgetClock for TestClock {
    fn now(&self) -> Instant {
        let ms = self.now.load(std::sync::atomic::Ordering::SeqCst);
        self.base + Duration::from_millis(ms)
    }
}

#[test]
fn test_single_active_candidate_and_binding_reuse() {
    let registry = BrowserTurnRegistry::default();
    let correlator = TurnCorrelator::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    // 1. 上报来自 Tab 100 的 turn_started 事件
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 100,
            event: "turn_started".to_string(),
            conversation_id: Some("conv_123".to_string()),
            turn_id: "turn_abc".to_string(),
            request_id: Some("req_1".to_string()),
            started_at: 1000,
            completed_at: None,
            requested_model: Some("auto".to_string()),
            actual_model: Some("o3-mini".to_string()),
        },
        now,
    );

    // 2. 首次关联：应为 SingleActiveCandidate 并建立绑定
    let (confidence, identity) = correlator.correlate("ws_test", "sess_001", &registry, now);
    assert_eq!(confidence, CorrelationConfidence::SingleActiveCandidate);
    match identity {
        TurnIdentity::Browser {
            workspace_id,
            session_id,
            conversation_id,
            turn_id,
            timer_origin,
            ..
        } => {
            assert_eq!(workspace_id, "ws_test");
            assert_eq!(session_id, "sess_001");
            assert_eq!(conversation_id, "conv_123");
            assert_eq!(turn_id, "turn_abc");
            assert_eq!(timer_origin, "browser_observed_turn_start");
        }
        _ => panic!("Expected Browser TurnIdentity"),
    }

    // 3. 第二次关联：应为 ExistingBinding 复用
    let (confidence2, identity2) = correlator.correlate("ws_test", "sess_001", &registry, now);
    assert_eq!(confidence2, CorrelationConfidence::ExistingBinding);
    assert_eq!(identity2.conversation_id(), Some("conv_123"));
    assert_eq!(identity2.turn_id(), Some("turn_abc"));
}

#[test]
fn test_ambiguous_multiple_candidates_falls_back_safely() {
    let registry = BrowserTurnRegistry::default();
    let correlator = TurnCorrelator::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    // 同时存在两个 Tab 的活跃 Turn
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            event: "turn_started".to_string(),
            conversation_id: Some("conv_A".to_string()),
            turn_id: "turn_A1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 102,
            event: "turn_started".to_string(),
            conversation_id: Some("conv_B".to_string()),
            turn_id: "turn_B1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    // 关联：因为有两个候选，必须 Ambiguous 降级为 SessionFallback 避免误杀
    let (confidence, identity) = correlator.correlate("ws_test", "sess_001", &registry, now);
    assert_eq!(confidence, CorrelationConfidence::Ambiguous);
    assert!(matches!(identity, TurnIdentity::SessionFallback { .. }));
}

#[test]
fn test_new_conversation_or_turn_resets_budget_and_clears_hard_stop() {
    let clock = Arc::new(TestClock::new());
    let config = AgentTurnBudgetConfig::default();
    let manager = Arc::new(AgentTurnBudgetManager::with_clock(config, clock.clone()));
    let registry = BrowserTurnRegistry::default();
    let correlator = TurnCorrelator::default();

    // 1. 旧会话 conv_old 第一轮耗尽预算进入 HardStop
    let now = clock.now();
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 200,
            event: "turn_started".to_string(),
            conversation_id: Some("conv_old".to_string()),
            turn_id: "turn_old_1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    let (_, identity1) = correlator.correlate("ws_test", "sess_openai", &registry, now);
    let decision1 = manager.start_call_with_identity(
        identity1,
        "read_file",
        false,
        &json!({ "path": "test.txt" }),
    );
    assert!(matches!(decision1, CallDecision::Allowed { .. }));
    drop(decision1);

    // 时间流逝 29 分钟 10 秒 -> 进入 HardStop
    clock.advance(Duration::from_secs(29 * 60 + 10));
    let now_hardstop = clock.now();
    let (_, identity_hardstop) = correlator.correlate("ws_test", "sess_openai", &registry, now_hardstop);
    let decision_hs = manager.start_call_with_identity(
        identity_hardstop,
        "read_file",
        false,
        &json!({ "path": "test.txt" }),
    );
    match decision_hs {
        CallDecision::Blocked { snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::HardStop);
        }
        _ => panic!("Expected HardStop Blocked decision"),
    }

    // 2. 用户切换至新对话 conv_new 开启新 Turn！
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 200,
            event: "turn_started".to_string(),
            conversation_id: Some("conv_new".to_string()),
            turn_id: "turn_new_1".to_string(),
            request_id: None,
            started_at: 2000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        now_hardstop,
    );

    // 3. 下一次工具调用到达：立即建立新 binding 并获得全新预算！
    let (conf_new, identity_new) = correlator.correlate("ws_test", "sess_openai", &registry, now_hardstop);
    assert_eq!(conf_new, CorrelationConfidence::SingleActiveCandidate);
    assert_eq!(identity_new.conversation_id(), Some("conv_new"));
    assert_eq!(identity_new.turn_id(), Some("turn_new_1"));

    let decision_new = manager.start_call_with_identity(
        identity_new,
        "read_file",
        false,
        &json!({ "path": "test.txt" }),
    );

    // 验证：绝对不能再是 HardStop，必须是 Allowed 且状态为 Normal！
    match decision_new {
        CallDecision::Allowed { snapshot, .. } => {
            assert_eq!(snapshot.status, TurnBudgetStatus::Normal);
            assert_eq!(snapshot.elapsed_seconds, 0);
            assert_eq!(snapshot.timer_origin, "browser_observed_turn_start");
        }
        _ => panic!("Expected Allowed Normal decision for new conversation turn!"),
    }
}

#[test]
fn test_stream_idle_quiet_window_lifecycle() {
    let registry = BrowserTurnRegistry::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 300,
            event: "turn_started".to_string(),
            conversation_id: Some("conv_quiet".to_string()),
            turn_id: "turn_q1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    // 生成流结束，进入 StreamIdle
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            observer_id: "obs_1".to_string(),
            tab_id: 300,
            event: "stream_completed".to_string(),
            conversation_id: Some("conv_quiet".to_string()),
            turn_id: "turn_q1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: Some(2000),
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    // 5秒后（静默窗口 15s 内）：依然是候选
    clock.advance(Duration::from_secs(5));
    let candidates_within_window = registry.get_active_candidates("ws_test", clock.now());
    assert_eq!(candidates_within_window.len(), 1);

    // 20秒后（超过静默窗口）：不再作为候选
    clock.advance(Duration::from_secs(15));
    let candidates_after_window = registry.get_active_candidates("ws_test", clock.now());
    assert_eq!(candidates_after_window.len(), 0);
}
