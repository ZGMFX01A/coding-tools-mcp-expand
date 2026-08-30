use std::sync::Arc;
use std::time::{Duration, Instant};

use coding_tools_mcp_desktop_lib::mcp::browser_turn::{
    BrowserTurnEvent, BrowserTurnEventKind, BrowserTurnRegistry, CorrelationConfidence, TurnCorrelator, TurnIdentity,
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
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 100,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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
            assert_eq!(timer_origin, "server_skew_fallback");
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
fn test_established_binding_survives_stream_idle_without_budget_fallback() {
    let registry = BrowserTurnRegistry::default();
    let correlator = TurnCorrelator::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb7e".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_idle".to_string(),
            tab_id: 110,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
            conversation_id: Some("conv_idle".to_string()),
            turn_id: "turn_idle".to_string(),
            request_id: Some("req_idle".to_string()),
            started_at: 1000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    let (initial_confidence, _) = correlator.correlate("ws_test", "sess_idle", &registry, now);
    assert_eq!(initial_confidence, CorrelationConfidence::SingleActiveCandidate);

    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb7f".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_idle".to_string(),
            tab_id: 110,
            sequence: 2,
            event: BrowserTurnEventKind::StreamCompleted,
            workspace_id: "ws_test".to_string(),
            conversation_id: Some("conv_idle".to_string()),
            turn_id: "turn_idle".to_string(),
            request_id: Some("req_idle".to_string()),
            started_at: 1000,
            completed_at: Some(2000),
            requested_model: None,
            actual_model: None,
        },
        now,
    );

    // 超过 15 秒静默候选窗口后，不得因为没有“活跃候选”而切换到新预算起点。
    clock.advance(Duration::from_secs(16));
    assert!(registry.get_active_candidates("ws_test", clock.now()).is_empty());

    let (confidence, identity) = correlator.correlate("ws_test", "sess_idle", &registry, clock.now());
    assert_eq!(confidence, CorrelationConfidence::ExistingBinding);
    match identity {
        TurnIdentity::Browser {
            conversation_id,
            turn_id,
            timer_origin,
            ..
        } => {
            assert_eq!(conversation_id, "conv_idle");
            assert_eq!(turn_id, "turn_idle");
            assert_eq!(timer_origin, "server_skew_fallback");
        }
        _ => panic!("Expected established Browser binding to survive stream idle"),
    }
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
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb61".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb62".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a22".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 102,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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
fn test_existing_binding_downgrades_to_ambiguous_when_new_tab_arrives_p0_3() {
    let registry = BrowserTurnRegistry::default();
    let correlator = TurnCorrelator::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    // 1. 只有 Tab 101 时关联建立 ExistingBinding
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb61".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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

    let (conf1, _) = correlator.correlate("ws_test", "sess_001", &registry, now);
    assert_eq!(conf1, CorrelationConfidence::SingleActiveCandidate);

    // 2. 此时 Tab 102 发起新的活跃 Turn
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb62".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a22".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 102,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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

    // 3. 再次关联：虽然已有 Tab 101 的 binding，但因为出现了多 Tab 竞争，必须强制 Ambiguous 降级！(P0-3 修复验证)
    let (conf2, identity2) = correlator.correlate("ws_test", "sess_001", &registry, now);
    assert_eq!(conf2, CorrelationConfidence::Ambiguous);
    assert!(matches!(identity2, TurnIdentity::SessionFallback { .. }));
}

#[test]
fn test_turn_closed_strictly_requires_all_three_criteria() {
    let registry = BrowserTurnRegistry::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb01".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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

    // 1. 发送 turn_id 不匹配的关闭事件 -> 忽略，依然活跃
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb02".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 2,
            event: BrowserTurnEventKind::TurnClosed,
            workspace_id: "ws_test".to_string(),
            conversation_id: Some("conv_A".to_string()),
            turn_id: "turn_WRONG".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: Some(2000),
            requested_model: None,
            actual_model: None,
        },
        now,
    );
    let cands1 = registry.get_active_candidates("ws_test", now);
    assert_eq!(cands1.len(), 1);

    // 2. 发送 sequence <= last_applied 的旧关闭事件 -> 忽略，依然活跃
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb03".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 1,
            event: BrowserTurnEventKind::TurnClosed,
            workspace_id: "ws_test".to_string(),
            conversation_id: Some("conv_A".to_string()),
            turn_id: "turn_A1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: Some(2000),
            requested_model: None,
            actual_model: None,
        },
        now,
    );
    let cands2 = registry.get_active_candidates("ws_test", now);
    assert_eq!(cands2.len(), 1);

    // 3. 发送三要素完全匹配且 sequence > last_sequence 的关闭事件 -> 成功关闭
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb04".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 3,
            event: BrowserTurnEventKind::TurnClosed,
            workspace_id: "ws_test".to_string(),
            conversation_id: Some("conv_A".to_string()),
            turn_id: "turn_A1".to_string(),
            request_id: None,
            started_at: 1000,
            completed_at: Some(2000),
            requested_model: None,
            actual_model: None,
        },
        now,
    );
    let cands3 = registry.get_active_candidates("ws_test", now);
    assert_eq!(cands3.len(), 0);
}

#[test]
fn test_active_transitions_to_stale_after_15_minutes() {
    let registry = BrowserTurnRegistry::default();
    let clock = Arc::new(TestClock::new());
    let now = clock.now();

    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb11".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 1,
            event: BrowserTurnEventKind::TurnStarted,
            workspace_id: "ws_test".to_string(),
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

    // 10 分钟后：依然活跃
    clock.advance(Duration::from_secs(10 * 60));
    assert_eq!(registry.get_active_candidates("ws_test", clock.now()).len(), 1);

    // 16 分钟后：流转为 Stale，退出候选池
    clock.advance(Duration::from_secs(6 * 60));
    assert_eq!(registry.get_active_candidates("ws_test", clock.now()).len(), 0);

    // 收到新的更新事件：重新恢复为 Active
    registry.record_event(
        "ws_test",
        BrowserTurnEvent {
            schema_version: 1,
            event_id: "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb12".to_string(),
            tab_instance_id: "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string(),
            observer_id: "obs_1".to_string(),
            tab_id: 101,
            sequence: 2,
            event: BrowserTurnEventKind::TurnUpdated,
            workspace_id: "ws_test".to_string(),
            conversation_id: Some("conv_A".to_string()),
            turn_id: "turn_A1".to_string(),
            request_id: Some("req_stream_2".to_string()),
            started_at: 1000,
            completed_at: None,
            requested_model: None,
            actual_model: None,
        },
        clock.now(),
    );
    assert_eq!(registry.get_active_candidates("ws_test", clock.now()).len(), 1);
}
