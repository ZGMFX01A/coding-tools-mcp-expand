use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tunnel::append_profile_log;

/// 时钟抽象，用于生产系统时间与单元测试虚拟时间的无缝解耦
pub trait BudgetClock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Debug, Default)]
pub struct SystemBudgetClock;

impl BudgetClock for SystemBudgetClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug)]
pub struct MockBudgetClock {
    current: RwLock<Instant>,
}

impl MockBudgetClock {
    pub fn new(initial: Instant) -> Self {
        Self {
            current: RwLock::new(initial),
        }
    }

    pub fn set(&self, time: Instant) {
        *self.current.write().expect("mock clock lock poisoned") = time;
    }

    pub fn advance(&self, duration: Duration) {
        let mut guard = self.current.write().expect("mock clock lock poisoned");
        *guard += duration;
    }
}

impl BudgetClock for MockBudgetClock {
    fn now(&self) -> Instant {
        *self.current.read().expect("mock clock lock poisoned")
    }
}

/// Agent Turn 预算配置
#[derive(Debug, Clone)]
pub struct AgentTurnBudgetConfig {
    pub enabled: bool,
    /// 触发 25 分钟提醒的时间阈值
    pub warning_after: Duration,
    /// 触发 28 分钟紧急收尾的时间阈值
    pub urgent_after: Duration,
    /// 29 分钟硬停止时间阈值
    pub hard_stop_after: Duration,
    /// 调度截止预留时间（默认 5s，在 29m - 5s = 28m55s 之后拒绝启动新工具）
    pub deadline_reserve: Duration,
    /// 早期（<20m）普通空闲重置阈值（默认 90s）
    pub early_idle_reset: Duration,
    /// 中期（20m~25m）空闲重置阈值（默认 180s）
    pub mid_idle_reset: Duration,
    /// 平台单轮执行假定硬限制（默认 30m）
    pub platform_turn_limit: Duration,
    /// 平台硬限制后安全隔离裕量（默认 15s）
    pub post_limit_safety_margin: Duration,
    /// 状态在内存中的最大存活时间（TTL，默认 2 小时）
    pub state_ttl: Duration,
    /// 状态池最大保留容量
    pub max_states: usize,
}

impl Default for AgentTurnBudgetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warning_after: Duration::from_secs(25 * 60),
            urgent_after: Duration::from_secs(28 * 60),
            hard_stop_after: Duration::from_secs(29 * 60),
            deadline_reserve: Duration::from_secs(5),
            early_idle_reset: Duration::from_secs(90),
            mid_idle_reset: Duration::from_secs(180),
            platform_turn_limit: Duration::from_secs(30 * 60),
            post_limit_safety_margin: Duration::from_secs(15),
            state_ttl: Duration::from_secs(2 * 3600),
            max_states: 128,
        }
    }
}

impl AgentTurnBudgetConfig {
    /// 构造用于快速测试的毫秒级阈值配置
    pub fn for_test_ms(
        warning_ms: u64,
        urgent_ms: u64,
        hard_stop_ms: u64,
        reserve_ms: u64,
        early_idle_ms: u64,
        mid_idle_ms: u64,
        platform_limit_ms: u64,
        margin_ms: u64,
    ) -> Self {
        Self {
            enabled: true,
            warning_after: Duration::from_millis(warning_ms),
            urgent_after: Duration::from_millis(urgent_ms),
            hard_stop_after: Duration::from_millis(hard_stop_ms),
            deadline_reserve: Duration::from_millis(reserve_ms),
            early_idle_reset: Duration::from_millis(early_idle_ms),
            mid_idle_reset: Duration::from_millis(mid_idle_ms),
            platform_turn_limit: Duration::from_millis(platform_limit_ms),
            post_limit_safety_margin: Duration::from_millis(margin_ms),
            state_ttl: Duration::from_secs(60),
            max_states: 128,
        }
    }
}

/// 唯一标识一个 ChatGPT 客户端会话在特定工作区中的 Turn Key
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnKey {
    pub workspace_id: String,
    pub session_id: String,
}

/// 单轮 Agent 运行状态
#[derive(Debug, Clone)]
pub struct AgentTurnState {
    pub turn_seq: u64,
    /// 本地观测到的首个工具调用开始时间 (timer_origin = first_observed_tool_call)
    pub started_at: Instant,
    pub last_call_started_at: Instant,
    pub last_call_completed_at: Option<Instant>,
    pub active_calls: usize,
    pub warning_emitted: bool,
    pub urgent_emitted: bool,
    pub hard_stopped_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnBudgetStatus {
    Normal,
    Warning,
    Urgent,
    DispatchCutoff,
    HardStop,
    Unmanaged,
}

impl TurnBudgetStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Urgent => "urgent",
            Self::DispatchCutoff => "dispatch_cutoff",
            Self::HardStop => "hard_stop",
            Self::Unmanaged => "unmanaged",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnBudgetSnapshot {
    pub status: TurnBudgetStatus,
    pub elapsed_seconds: u64,
    pub remaining_seconds: u64,
    pub should_wrap_up: bool,
    pub should_stop_tool_calls: bool,
    pub timer_origin: &'static str,
}

/// RAII Guard，在工具调用结束（无论正常、错误或 panic）时自动递减 active_calls
pub struct TurnCallGuard {
    manager: Arc<AgentTurnBudgetManager>,
    key: TurnKey,
}

impl TurnCallGuard {
    pub fn new(manager: Arc<AgentTurnBudgetManager>, key: TurnKey) -> Self {
        Self { manager, key }
    }
}

impl Drop for TurnCallGuard {
    fn drop(&mut self) {
        let now = self.manager.clock.now();
        self.manager.complete_call(&self.key, now);
    }
}

/// 工具调用决策
pub enum CallDecision {
    /// 允许执行
    Allowed {
        guard: TurnCallGuard,
        runtime_budget: Duration,
        snapshot: TurnBudgetSnapshot,
        emit_full_warning: bool,
        emit_urgent: bool,
    },
    /// 被阻止（DispatchCutoff 或 HardStop）
    Blocked {
        snapshot: TurnBudgetSnapshot,
        error_payload: Value,
        content_text: String,
    },
    /// 未托管（无 session 时降级）
    Unmanaged,
}

/// Agent Turn Budget 管理器
pub struct AgentTurnBudgetManager {
    config: AgentTurnBudgetConfig,
    clock: Arc<dyn BudgetClock>,
    states: Mutex<HashMap<TurnKey, AgentTurnState>>,
}

impl AgentTurnBudgetManager {
    pub fn new(config: AgentTurnBudgetConfig) -> Self {
        Self {
            config,
            clock: Arc::new(SystemBudgetClock),
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_clock(config: AgentTurnBudgetConfig, clock: Arc<dyn BudgetClock>) -> Self {
        Self {
            config,
            clock,
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &AgentTurnBudgetConfig {
        &self.config
    }

    pub fn clock(&self) -> &Arc<dyn BudgetClock> {
        &self.clock
    }

    /// 对 session_id 进行安全脱敏用于日志
    pub fn sanitize_session_for_log(session_id: &str) -> String {
        let trimmed = session_id.trim();
        if trimmed.is_empty() {
            return "empty".to_string();
        }
        if trimmed.len() <= 8 {
            return format!("{}***", &trimmed[..trimmed.len().min(4)]);
        }
        format!("{}...{}", &trimmed[..4], &trimmed[trimmed.len() - 4..])
    }

    /// 在工具调用前进行评估与状态登记
    pub fn start_call(
        self: &Arc<Self>,
        workspace_id: &str,
        session_id: Option<&str>,
    ) -> CallDecision {
        if !self.config.enabled {
            return CallDecision::Unmanaged;
        }

        let session = match session_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => return CallDecision::Unmanaged,
        };

        let key = TurnKey {
            workspace_id: workspace_id.to_string(),
            session_id: session,
        };

        let now = self.clock.now();
        let mut states = self.states.lock().expect("agent turn budget lock poisoned");
        self.cleanup_stale_states_locked(&mut states, now);

        let sanitized_session = Self::sanitize_session_for_log(&key.session_id);

        let (decision, log_event) = match states.get_mut(&key) {
            None => {
                // 如果即将插入新 key 且容量已达上限，先淘汰最旧非活跃状态
                if states.len() >= self.config.max_states {
                    self.evict_oldest_inactive_locked(&mut states);
                }

                // 首个观测到的调用，创建新 Turn
                let new_state = AgentTurnState {
                    turn_seq: 1,
                    started_at: now,
                    last_call_started_at: now,
                    last_call_completed_at: None,
                    active_calls: 1,
                    warning_emitted: false,
                    urgent_emitted: false,
                    hard_stopped_at: None,
                };
                states.insert(key.clone(), new_state);

                let cutoff = self.config.hard_stop_after.saturating_sub(self.config.deadline_reserve);
                let runtime_budget = cutoff;
                let snapshot = TurnBudgetSnapshot {
                    status: TurnBudgetStatus::Normal,
                    elapsed_seconds: 0,
                    remaining_seconds: runtime_budget.as_secs(),
                    should_wrap_up: false,
                    should_stop_tool_calls: false,
                    timer_origin: "first_observed_tool_call",
                };

                let guard = TurnCallGuard::new(self.clone(), key.clone());
                (
                    CallDecision::Allowed {
                        guard,
                        runtime_budget,
                        snapshot,
                        emit_full_warning: false,
                        emit_urgent: false,
                    },
                    Some(format!(
                        "[turn-budget] new workspace={} session={} turn_seq=1",
                        workspace_id, sanitized_session
                    )),
                )
            }
            Some(state) => {
                // 检查是否应当重置为新 Turn
                if state.active_calls == 0 {
                    if state.hard_stopped_at.is_some() {
                        // HardStop 状态下的重置要求：跨越平台硬限制隔离线 (started_at + platform_turn_limit + post_limit_safety_margin)
                        let isolation_deadline = state.started_at
                            + self.config.platform_turn_limit
                            + self.config.post_limit_safety_margin;
                        if now >= isolation_deadline {
                            state.turn_seq += 1;
                            state.started_at = now;
                            state.last_call_started_at = now;
                            state.last_call_completed_at = None;
                            state.warning_emitted = false;
                            state.urgent_emitted = false;
                            state.hard_stopped_at = None;
                            append_profile_log(
                                workspace_id,
                                "mcp-requests.log",
                                &format!(
                                    "[turn-budget] reset-after-platform-limit workspace={} session={} new_turn_seq={}",
                                    workspace_id, sanitized_session, state.turn_seq
                                ),
                            );
                        } else {
                            // 尚未跨越平台隔离线：保持 HardStop 拒绝，绝对不刷新任何时间戳！
                            let elapsed = now.saturating_duration_since(state.started_at);
                            let snapshot = TurnBudgetSnapshot {
                                status: TurnBudgetStatus::HardStop,
                                elapsed_seconds: elapsed.as_secs(),
                                remaining_seconds: 0,
                                should_wrap_up: true,
                                should_stop_tool_calls: true,
                                timer_origin: "first_observed_tool_call",
                            };
                            let content_text = "[TURN BUDGET HARD STOP]\nTool execution was not started because this agent turn reached the local safety limit. Do not call any more tools in this turn. Respond to the user now with completed work, verification results, and remaining work.".to_string();
                            let error_payload = json!({
                                "ok": false,
                                "error": {
                                    "code": "AGENT_TURN_BUDGET_EXHAUSTED",
                                    "message": "Tool execution was not started because this agent turn reached the local safety limit.",
                                    "category": "execution",
                                    "retryable": false,
                                    "recovery_hint": "Respond to the user now without calling additional tools."
                                }
                            });
                            return CallDecision::Blocked {
                                snapshot,
                                error_payload,
                                content_text,
                            };
                        }
                    } else {
                        // 非 HardStop 状态下的动态 Idle Reset 判定
                        let elapsed = now.saturating_duration_since(state.started_at);
                        let idle = state
                            .last_call_completed_at
                            .map(|completed| now.saturating_duration_since(completed))
                            .unwrap_or_else(|| now.saturating_duration_since(state.last_call_started_at));

                        let early_cutoff = self.config.warning_after.mul_f64(0.8);
                        let should_reset = if elapsed < early_cutoff {
                            idle >= self.config.early_idle_reset
                        } else if elapsed < self.config.warning_after {
                            idle >= self.config.mid_idle_reset
                        } else {
                            // elapsed >= warning_after: 临近截止严禁普通 idle 重置
                            false
                        };

                        if should_reset {
                            state.turn_seq += 1;
                            state.started_at = now;
                            state.last_call_started_at = now;
                            state.last_call_completed_at = None;
                            state.warning_emitted = false;
                            state.urgent_emitted = false;
                            state.hard_stopped_at = None;
                            append_profile_log(
                                workspace_id,
                                "mcp-requests.log",
                                &format!(
                                    "[turn-budget] reset-by-idle workspace={} session={} new_turn_seq={}",
                                    workspace_id, sanitized_session, state.turn_seq
                                ),
                            );
                        }
                    }
                }

                let elapsed = now.saturating_duration_since(state.started_at);
                let cutoff_point = self
                    .config
                    .hard_stop_after
                    .saturating_sub(self.config.deadline_reserve);

                if elapsed >= self.config.hard_stop_after {
                    // >= 29 分钟：触发 HardStop
                    state.hard_stopped_at.get_or_insert(now);
                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::HardStop,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: 0,
                        should_wrap_up: true,
                        should_stop_tool_calls: true,
                        timer_origin: "first_observed_tool_call",
                    };
                    let content_text = "[TURN BUDGET HARD STOP]\nTool execution was not started because this agent turn reached the local safety limit. Do not call any more tools in this turn. Respond to the user now with completed work, verification results, and remaining work.".to_string();
                    let error_payload = json!({
                        "ok": false,
                        "error": {
                            "code": "AGENT_TURN_BUDGET_EXHAUSTED",
                            "message": "Tool execution was not started because this agent turn reached the local safety limit.",
                            "category": "execution",
                            "retryable": false,
                            "recovery_hint": "Respond to the user now without calling additional tools."
                        }
                    });
                    (
                        CallDecision::Blocked {
                            snapshot,
                            error_payload,
                            content_text,
                        },
                        Some(format!(
                            "[turn-budget] hard-stop workspace={} session={} turn_seq={} elapsed_secs={}",
                            workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs()
                        )),
                    )
                } else if elapsed >= cutoff_point {
                    // 28分55秒 ~ 29分钟：进入 Dispatch Cutoff 阶段
                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::DispatchCutoff,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: self
                            .config
                            .hard_stop_after
                            .saturating_sub(elapsed)
                            .as_secs(),
                        should_wrap_up: true,
                        should_stop_tool_calls: true,
                        timer_origin: "first_observed_tool_call",
                    };
                    let content_text = "[TURN BUDGET DISPATCH CUTOFF]\nTool execution dispatch was cutoff because this agent turn has less than 5 seconds of local execution budget remaining. Do not call additional tools. Respond to the user now with completed work, verification results, and remaining work.".to_string();
                    let error_payload = json!({
                        "ok": false,
                        "error": {
                            "code": "AGENT_TURN_BUDGET_DISPATCH_CUTOFF",
                            "message": "Tool execution dispatch was cutoff because this agent turn is near the local safety limit.",
                            "category": "execution",
                            "retryable": false,
                            "recovery_hint": "Respond to the user now without calling additional tools."
                        }
                    });
                    (
                        CallDecision::Blocked {
                            snapshot,
                            error_payload,
                            content_text,
                        },
                        Some(format!(
                            "[turn-budget] dispatch-cutoff workspace={} session={} turn_seq={} elapsed_secs={}",
                            workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs()
                        )),
                    )
                } else {
                    // 允许启动调用
                    state.active_calls += 1;
                    state.last_call_started_at = now;
                    let runtime_budget = cutoff_point.saturating_sub(elapsed);

                    let (status, emit_full_warning, emit_urgent, log_str) =
                        if elapsed >= self.config.urgent_after {
                            let first_urgent = !state.urgent_emitted;
                            state.urgent_emitted = true;
                            (
                                TurnBudgetStatus::Urgent,
                                false,
                                first_urgent,
                                if first_urgent {
                                    Some(format!(
                                        "[turn-budget] urgent workspace={} session={} turn_seq={} elapsed_secs={}",
                                        workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs()
                                    ))
                                } else {
                                    None
                                },
                            )
                        } else if elapsed >= self.config.warning_after {
                            let first_warning = !state.warning_emitted;
                            state.warning_emitted = true;
                            (
                                TurnBudgetStatus::Warning,
                                first_warning,
                                false,
                                if first_warning {
                                    Some(format!(
                                        "[turn-budget] warning workspace={} session={} turn_seq={} elapsed_secs={}",
                                        workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs()
                                    ))
                                } else {
                                    None
                                },
                            )
                        } else {
                            (TurnBudgetStatus::Normal, false, false, None)
                        };

                    let snapshot = TurnBudgetSnapshot {
                        status,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: runtime_budget.as_secs(),
                        should_wrap_up: status == TurnBudgetStatus::Warning
                            || status == TurnBudgetStatus::Urgent,
                        should_stop_tool_calls: false,
                        timer_origin: "first_observed_tool_call",
                    };

                    let guard = TurnCallGuard::new(self.clone(), key.clone());
                    (
                        CallDecision::Allowed {
                            guard,
                            runtime_budget,
                            snapshot,
                            emit_full_warning,
                            emit_urgent,
                        },
                        log_str,
                    )
                }
            }
        };

        if let Some(log_line) = log_event {
            append_profile_log(workspace_id, "mcp-requests.log", &log_line);
        }

        decision
    }

    /// 在工具调用完成时更新 active_calls 和 last_call_completed_at
    pub fn complete_call(&self, key: &TurnKey, now: Instant) {
        let mut states = self.states.lock().expect("agent turn budget lock poisoned");
        if let Some(state) = states.get_mut(key) {
            state.active_calls = state.active_calls.saturating_sub(1);
            state.last_call_completed_at = Some(now);
        }
    }

    /// 惰性清理过期与超出容量的状态
    fn cleanup_stale_states_locked(
        &self,
        states: &mut HashMap<TurnKey, AgentTurnState>,
        now: Instant,
    ) {
        // 1. 移除 TTL 过期且无活跃调用的状态
        states.retain(|_, state| {
            if state.active_calls > 0 {
                return true;
            }
            let last_active = state
                .last_call_completed_at
                .unwrap_or(state.last_call_started_at);
            now.saturating_duration_since(last_active) < self.config.state_ttl
        });

        // 2. 超出最大条目数时，移除最旧的非活跃状态
        while states.len() > self.config.max_states {
            if !self.evict_oldest_inactive_locked(states) {
                break;
            }
        }
    }

    /// 淘汰最旧的一条非活跃状态，若无可淘汰状态返回 false
    fn evict_oldest_inactive_locked(&self, states: &mut HashMap<TurnKey, AgentTurnState>) -> bool {
        let oldest_inactive = states
            .iter()
            .filter(|(_, s)| s.active_calls == 0)
            .min_by_key(|(_, s)| s.last_call_completed_at.unwrap_or(s.last_call_started_at))
            .map(|(k, _)| k.clone());

        if let Some(key_to_evict) = oldest_inactive {
            states.remove(&key_to_evict);
            true
        } else {
            false
        }
    }

    /// 为被阻断的调用生成统一的返回结构
    pub fn build_blocked_result(
        &self,
        snapshot: &TurnBudgetSnapshot,
        error_payload: &Value,
        content_text: &str,
    ) -> Value {
        let mut res = json!({
            "content": [{
                "type": "text",
                "text": content_text
            }],
            "structuredContent": error_payload,
            "isError": true
        });
        inject_budget_meta(&mut res, snapshot);
        res
    }

    /// 统一修饰正常执行的工具输出结果
    /// 1. 在顶层 `_meta["coding-tools/agentTurnBudget"]` 注入机器可读状态，严禁修改第三方 structuredContent
    /// 2. 在 `content[0].text` 前置注入警告提示
    pub fn decorate_allowed_result(
        &self,
        mut result: Value,
        snapshot: &TurnBudgetSnapshot,
        emit_full_warning: bool,
    ) -> Value {
        inject_budget_meta(&mut result, snapshot);

        if emit_full_warning && snapshot.status == TurnBudgetStatus::Warning {
            prepend_content_text(
                &mut result,
                "[TURN BUDGET WARNING]\nThis agent turn has been using tools for about 25 minutes.\nStop expanding the task. Finish only the currently necessary work, then respond to the user with completed work, verification results, and remaining work.\n\n",
            );
        } else if snapshot.status == TurnBudgetStatus::Urgent {
            prepend_content_text(
                &mut result,
                "[TURN BUDGET URGENT]\nLess than one minute of local tool budget remains. Do not start new investigation. Finish only essential cleanup and respond to the user.\n\n",
            );
        }

        result
    }
}

/// 在结果顶层 `_meta["coding-tools/agentTurnBudget"]` 注入预算状态
fn inject_budget_meta(result: &mut Value, snapshot: &TurnBudgetSnapshot) {
    if let Some(obj) = result.as_object_mut() {
        let meta_entry = obj
            .entry("_meta")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(meta_obj) = meta_entry.as_object_mut() {
            meta_obj.insert(
                "coding-tools/agentTurnBudget".to_string(),
                serde_json::to_value(snapshot).unwrap_or_default(),
            );
        }
    }
}

/// 在 content[0].text 最前部 prepend 提示文本
fn prepend_content_text(result: &mut Value, warning_text: &str) {
    if let Some(content_arr) = result
        .get_mut("content")
        .and_then(Value::as_array_mut)
    {
        if let Some(first) = content_arr.first_mut() {
            if let Some(first_obj) = first.as_object_mut() {
                if first_obj.get("type").and_then(Value::as_str) == Some("text") {
                    let old_text = first_obj
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    first_obj.insert(
                        "text".to_string(),
                        Value::String(format!("{warning_text}{old_text}")),
                    );
                }
            }
        }
    }
}
