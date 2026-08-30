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

    pub fn advance_ms(&self, ms: u64) {
        self.advance(Duration::from_millis(ms));
    }
}

impl BudgetClock for MockBudgetClock {
    fn now(&self) -> Instant {
        *self.current.read().expect("mock clock lock poisoned")
    }
}

/// 工具属性标识结构体，避免单一枚举互斥导致的语义冲突
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolBudgetTraits {
    /// 是否属于探索/发散型工具（会在代码库或历史中大规模寻找新线索）
    pub investigation: bool,
    /// 是否会修改代码或工作区状态
    pub mutating: bool,
    /// 是否会开启新的长任务或危险执行
    pub starts_new_work: bool,
    /// 是否属于安全无发散的只读验证
    pub verification_safe: bool,
    /// 是否允许在 WRAP_UP 阶段 (27m~28m) 执行
    pub wrap_up_allowed: bool,
    /// 是否允许在 FINALIZATION 阶段 (28m~28m55s) 执行
    pub finalization_safe: bool,
}

/// 对给定工具根据其名称、参数以及是否 External 进行属性分类判定
pub fn get_tool_traits(tool_name: &str, is_external: bool, args: &Value) -> ToolBudgetTraits {
    if is_external {
        return ToolBudgetTraits {
            investigation: true,
            mutating: false,
            starts_new_work: true,
            verification_safe: false,
            wrap_up_allowed: false,
            finalization_safe: false,
        };
    }

    let canonical = crate::tools::registry::canonical_tool_name(tool_name);
    match canonical {
        // 探索型工具（在 27m WRAP_UP 阶段立即禁止）
        "search_text" | "grep_text" | "grep" | "list_files" | "history_session_search"
        | "history_session_read" | "git_log" | "git_show" | "git_blame" | "view_image" => {
            ToolBudgetTraits {
                investigation: true,
                mutating: false,
                starts_new_work: false,
                verification_safe: false,
                wrap_up_allowed: false,
                finalization_safe: false,
            }
        }

        // 修改代码工具（WRAP_UP 允许完成最后修改，FINALIZATION 严禁）
        "apply_patch" => ToolBudgetTraits {
            investigation: false,
            mutating: true,
            starts_new_work: false,
            verification_safe: false,
            wrap_up_allowed: true,
            finalization_safe: false,
        },

        // 命令执行工具（结合参数智能识别验证类命令，WRAP_UP 阶段限额 30s，FINALIZATION 阶段禁止）
        "exec_command" => {
            let is_verif = args
                .get("cmd")
                .and_then(Value::as_str)
                .map(is_wrap_up_verification_command)
                .unwrap_or(false);

            ToolBudgetTraits {
                investigation: false,
                mutating: false,
                starts_new_work: !is_verif,
                verification_safe: is_verif,
                wrap_up_allowed: is_verif,
                finalization_safe: false,
            }
        }

        // 会话杀死（允许作为最终 cleanup 特例）
        "kill_session" => ToolBudgetTraits {
            investigation: false,
            mutating: true,
            starts_new_work: false,
            verification_safe: true,
            wrap_up_allowed: true,
            finalization_safe: true,
        },

        // 会话 checkpoint 与任务完成（具有副作用但在收尾阶段必须允许）
        "history_session_checkpoint" | "finish_task" => ToolBudgetTraits {
            investigation: false,
            mutating: true,
            starts_new_work: false,
            verification_safe: true,
            wrap_up_allowed: true,
            finalization_safe: true,
        },

        // 核心收尾与只读事实确认工具（全阶段至 28m55s 允许）
        "read_file" | "git_status" | "git_diff" | "read_output" | "project_state"
        | "change_summary" | "task_context" => ToolBudgetTraits {
            investigation: false,
            mutating: false,
            starts_new_work: false,
            verification_safe: true,
            wrap_up_allowed: true,
            finalization_safe: true,
        },

        // 轻量预检与环境诊断
        "patch_check" | "check_exec_environment" | "exec_health_check" | "get_default_cwd" => {
            ToolBudgetTraits {
                investigation: false,
                mutating: false,
                starts_new_work: false,
                verification_safe: true,
                wrap_up_allowed: true,
                finalization_safe: true,
            }
        }

        // 其他状态工具（WRAP_UP 允许，FINALIZATION 默认收拢以防发散）
        "list_dir" | "server_info" | "harness_status" | "operation_log" | "list_task_events" => {
            ToolBudgetTraits {
                investigation: false,
                mutating: false,
                starts_new_work: false,
                verification_safe: true,
                wrap_up_allowed: true,
                finalization_safe: false,
            }
        }

        // 其他可能开启新状态的工具（WRAP_UP 与 FINALIZATION 均禁止）
        "set_default_cwd" | "start_task" | "update_task" | "pause_task" | "resume_task"
        | "history_session_bootstrap" | "history_session_validate" | "request_permissions"
        | "write_stdin" => ToolBudgetTraits {
            investigation: false,
            mutating: true,
            starts_new_work: true,
            verification_safe: false,
            wrap_up_allowed: false,
            finalization_safe: false,
        },

        // 兜底默认值
        _ => ToolBudgetTraits {
            investigation: true,
            mutating: false,
            starts_new_work: true,
            verification_safe: false,
            wrap_up_allowed: false,
            finalization_safe: false,
        },
    }
}

/// 判断命令是否属于 WRAP_UP 阶段允许的轻量验证命令
pub fn is_wrap_up_verification_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || trimmed.len() > 1000 {
        return false;
    }
    // 检查管道、重定向与链式语法
    if trimmed.contains('|')
        || trimmed.contains(';')
        || trimmed.contains('&')
        || trimmed.contains('>')
        || trimmed.contains('<')
    {
        return false;
    }

    let Ok(parts) = shell_words::split(trimmed) else {
        return false;
    };
    if parts.is_empty() {
        return false;
    }

    let exe = parts[0].to_ascii_lowercase();
    let base_exe = exe
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&exe)
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat");

    // 严禁包含安装、克隆、下载、启动服务等动词
    let forbidden_verbs = [
        "install", "add", "clone", "get", "fetch", "pull", "push", "serve", "server", "start",
        "dev", "watch", "run-server", "daemon", "rm", "del", "delete", "clean", "upgrade",
    ];
    for p in &parts[1..] {
        let low = p.to_ascii_lowercase();
        if forbidden_verbs.iter().any(|v| low == *v) {
            return false;
        }
        if low.starts_with("http://") || low.starts_with("https://") || low.starts_with("git@") {
            return false;
        }
    }

    match base_exe {
        "cargo" => {
            if parts.len() >= 2 {
                let sub = parts[1].to_ascii_lowercase();
                sub == "check" || sub == "test" || sub == "clippy" || sub == "--version" || sub == "-v"
            } else {
                false
            }
        }
        "npm" | "pnpm" | "yarn" | "bun" => {
            if parts.len() >= 2 {
                let sub = parts[1].to_ascii_lowercase();
                if sub == "test" || sub == "check" || sub == "lint" || sub == "--version" || sub == "-v" {
                    return true;
                }
                if sub == "run" && parts.len() >= 3 {
                    let script = parts[2].to_ascii_lowercase();
                    return script == "check"
                        || script == "build"
                        || script == "test"
                        || script == "lint"
                        || script == "typecheck";
                }
            }
            false
        }
        "git" => {
            if parts.len() >= 2 {
                let sub = parts[1].to_ascii_lowercase();
                sub == "status" || sub == "diff" || sub == "branch" || sub == "--version" || sub == "show"
            } else {
                false
            }
        }
        "python" | "python3" | "py" => {
            if parts.len() >= 2 {
                let sub = parts[1].to_ascii_lowercase();
                sub == "--version" || sub == "-v" || sub == "-m"
            } else {
                false
            }
        }
        "pytest" | "ruff" | "mypy" | "eslint" | "tsc" => true,
        "node" | "rustc" | "go" | "dotnet" | "deno" => {
            if parts.len() >= 2 {
                let sub = parts[1].to_ascii_lowercase();
                sub == "--version" || sub == "-v" || sub == "version" || sub == "test"
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Agent Turn 预算配置
#[derive(Debug, Clone)]
pub struct AgentTurnBudgetConfig {
    pub enabled: bool,
    /// 触发 25 分钟提醒的时间阈值
    pub warning_after: Duration,
    /// 触发 27 分钟收敛阶段（禁止 investigation 与外部工具）
    pub wrap_up_after: Duration,
    /// 触发 28 分钟收尾阶段（仅允许最小只读收尾工具）
    pub finalization_after: Duration,
    /// 调度截止预留时间（默认 5s，在 29m - 5s = 28m55s 之后拒绝启动新工具）
    pub deadline_reserve: Duration,
    /// 29 分钟硬停止时间阈值
    pub hard_stop_after: Duration,
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
            wrap_up_after: Duration::from_secs(27 * 60),
            finalization_after: Duration::from_secs(28 * 60),
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
        wrap_up_ms: u64,
        finalization_ms: u64,
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
            wrap_up_after: Duration::from_millis(wrap_up_ms),
            finalization_after: Duration::from_millis(finalization_ms),
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

use crate::mcp::browser_turn::TurnIdentity;

/// 唯一标识一个客户端会话/单轮在特定工作区中的 Turn Key
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnKey {
    pub workspace_id: String,
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnLifecyclePhase {
    Active,
    Closing,
}

/// 单轮 Agent 运行状态
#[derive(Debug, Clone)]
pub struct AgentTurnState {
    pub turn_seq: u64,
    pub phase: TurnLifecyclePhase,
    /// 观测到的起始时间（Browser 模式为 browser_observed_turn_start，Fallback 模式为 first_observed_tool_call）
    pub started_at: Instant,
    pub timer_origin: &'static str,
    pub last_call_started_at: Instant,
    pub last_call_completed_at: Option<Instant>,
    pub active_calls: usize,
    pub warning_emitted: bool,
    pub wrap_up_emitted: bool,
    pub finalization_emitted: bool,
    pub hard_stopped_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnBudgetStatus {
    Normal,
    Warning,
    WrapUp,
    Urgent,
    Finalization,
    DispatchCutoff,
    HardStop,
    Unmanaged,
}

impl TurnBudgetStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::WrapUp => "wrap_up",
            Self::Urgent => "urgent",
            Self::Finalization => "finalization",
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
    /// 受政策限制被拒绝（WRAP_UP 或 FINALIZATION 阶段）
    Restricted {
        snapshot: TurnBudgetSnapshot,
        error_payload: Value,
        content_text: String,
    },
    /// 被硬阻止（DispatchCutoff 或 HardStop）
    Blocked {
        snapshot: TurnBudgetSnapshot,
        error_payload: Value,
        content_text: String,
    },
    /// 未托管（仅在预算显式禁用时返回）
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

    /// 对尚未建立状态的 Browser Turn 也执行完整的阶段门禁。
    ///
    /// Browser 事件可能先于第一次 MCP 调用到达，因此新建状态时的
    /// `effective_started_at` 可能已经位于 WRAP_UP/FINALIZATION 甚至 HARD_STOP。
    /// 这里不能把首次调用无条件视为 NORMAL，否则换 key、延迟首调或状态淘汰
    /// 都会形成预算旁路。
    fn reject_initial_call_if_disallowed(
        &self,
        elapsed: Duration,
        cutoff_point: Duration,
        tool_name: &str,
        traits: ToolBudgetTraits,
        timer_origin: &'static str,
        workspace_id: &str,
        sanitized_session: &str,
    ) -> Option<(CallDecision, String)> {
        if elapsed >= self.config.hard_stop_after {
            let snapshot = TurnBudgetSnapshot {
                status: TurnBudgetStatus::HardStop,
                elapsed_seconds: elapsed.as_secs(),
                remaining_seconds: 0,
                should_wrap_up: true,
                should_stop_tool_calls: true,
                timer_origin,
            };
            let content_text = "[TURN BUDGET HARD STOP]\nTool execution was not started because this agent turn reached the local safety limit. Do not call any more tools in this turn. Respond to the user now with completed work, verification results, and remaining work.".to_string();
            let error_payload = json!({
                "ok": false,
                "error": {
                    "code": "AGENT_TURN_BUDGET_EXHAUSTED",
                    "message": "Tool execution was not started because this agent turn reached the local safety limit.",
                    "category": "turn_budget",
                    "retryable": false,
                    "recovery_hint": "DO NOT RETRY THIS TOOL OR CALL ANY OTHER TOOL. Respond to the user immediately with current progress."
                }
            });
            return Some((
                CallDecision::Blocked {
                    snapshot,
                    error_payload,
                    content_text,
                },
                format!(
                    "[turn-budget] hard_stop workspace={} session={} turn_seq=1 elapsed_secs={} tool={}",
                    workspace_id,
                    sanitized_session,
                    elapsed.as_secs(),
                    tool_name
                ),
            ));
        }

        if elapsed >= cutoff_point {
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
                timer_origin,
            };
            let content_text = "[TURN BUDGET DISPATCH CUTOFF]\nTool execution dispatch was cutoff because this agent turn has less than 5 seconds of local execution budget remaining. Do not call additional tools. Respond to the user now with completed work, verification results, and remaining work.".to_string();
            let error_payload = json!({
                "ok": false,
                "error": {
                    "code": "AGENT_TURN_BUDGET_DISPATCH_CUTOFF",
                    "message": "Tool execution dispatch was cutoff because this agent turn is near the local safety limit.",
                    "category": "turn_budget",
                    "retryable": false,
                    "recovery_hint": "DO NOT RETRY THIS TOOL OR CALL ANY OTHER TOOL. Respond to the user immediately with completed work, verification results, and remaining work."
                }
            });
            return Some((
                CallDecision::Blocked {
                    snapshot,
                    error_payload,
                    content_text,
                },
                format!(
                    "[turn-budget] dispatch_cutoff workspace={} session={} turn_seq=1 elapsed_secs={} tool={}",
                    workspace_id,
                    sanitized_session,
                    elapsed.as_secs(),
                    tool_name
                ),
            ));
        }

        if elapsed >= self.config.finalization_after && !traits.finalization_safe {
            let snapshot = TurnBudgetSnapshot {
                status: TurnBudgetStatus::Finalization,
                elapsed_seconds: elapsed.as_secs(),
                remaining_seconds: cutoff_point.saturating_sub(elapsed).as_secs(),
                should_wrap_up: true,
                should_stop_tool_calls: false,
                timer_origin,
            };
            let content_text = format!(
                "[TURN BUDGET RESTRICTED]\nTool execution of '{}' was blocked because this turn is in finalization mode (elapsed: {}s). Code changes, new executions, searches, and external tools are no longer allowed. Use only finalization-safe tools (e.g. read_file, git_status, git_diff, history_session_checkpoint, finish_task) to verify the current state, then respond to the user immediately.",
                tool_name,
                elapsed.as_secs()
            );
            let error_payload = json!({
                "ok": false,
                "error": {
                    "code": "AGENT_TURN_WRAP_UP_RESTRICTED",
                    "message": format!("Tool '{}' is blocked in finalization mode.", tool_name),
                    "category": "turn_budget",
                    "retryable": false,
                    "recovery_hint": "Do not retry this tool. This turn is in finalization mode. Code changes, new executions, searches, and external tools are no longer allowed. Use only finalization-safe tools to verify the current state, then respond to the user immediately."
                }
            });
            return Some((
                CallDecision::Restricted {
                    snapshot,
                    error_payload,
                    content_text,
                },
                format!(
                    "[turn-budget] tool_rejected_by_finalization workspace={} session={} turn_seq=1 elapsed_secs={} tool={}",
                    workspace_id,
                    sanitized_session,
                    elapsed.as_secs(),
                    tool_name
                ),
            ));
        }

        if elapsed >= self.config.wrap_up_after && !traits.wrap_up_allowed {
            let snapshot = TurnBudgetSnapshot {
                status: TurnBudgetStatus::WrapUp,
                elapsed_seconds: elapsed.as_secs(),
                remaining_seconds: cutoff_point.saturating_sub(elapsed).as_secs(),
                should_wrap_up: true,
                should_stop_tool_calls: false,
                timer_origin,
            };
            let content_text = format!(
                "[TURN BUDGET RESTRICTED]\nTool execution of '{}' was blocked because this turn is in wrap-up mode (elapsed: {}s). New investigation and task expansion are no longer allowed. Finish only the already identified work using finalization-safe tools, then respond to the user.",
                tool_name,
                elapsed.as_secs()
            );
            let error_payload = json!({
                "ok": false,
                "error": {
                    "code": "AGENT_TURN_WRAP_UP_RESTRICTED",
                    "message": format!("Tool '{}' is blocked in wrap-up mode.", tool_name),
                    "category": "turn_budget",
                    "retryable": false,
                    "recovery_hint": "Do not retry this tool. This turn is in wrap-up mode. New investigation is no longer allowed. Finish only the already identified work using finalization-safe tools, then respond to the user."
                }
            });
            return Some((
                CallDecision::Restricted {
                    snapshot,
                    error_payload,
                    content_text,
                },
                format!(
                    "[turn-budget] tool_rejected_by_wrap_up workspace={} session={} turn_seq=1 elapsed_secs={} tool={}",
                    workspace_id,
                    sanitized_session,
                    elapsed.as_secs(),
                    tool_name
                ),
            ));
        }

        None
    }

    /// 根据解析后的 TurnIdentity 进行评估与状态登记
    pub fn start_call_with_identity(
        self: &Arc<Self>,
        identity: TurnIdentity,
        tool_name: &str,
        is_external: bool,
        args: &Value,
    ) -> CallDecision {
        if !self.config.enabled {
            return CallDecision::Unmanaged;
        }

        let (key, is_browser, effective_started_at, initial_timer_origin) = match identity {
            TurnIdentity::Browser {
                workspace_id,
                session_id,
                conversation_id,
                turn_id,
                effective_started_at,
                timer_origin,
            } => (
                TurnKey {
                    workspace_id,
                    session_id,
                    conversation_id: Some(conversation_id),
                    turn_id: Some(turn_id),
                },
                true,
                effective_started_at,
                timer_origin,
            ),
            TurnIdentity::SessionFallback {
                workspace_id,
                session_id,
            } => {
                let s = session_id.trim();
                if s.is_empty() {
                    return CallDecision::Unmanaged;
                }
                (
                    TurnKey {
                        workspace_id,
                        session_id: s.to_string(),
                        conversation_id: None,
                        turn_id: None,
                    },
                    false,
                    self.clock.now(),
                    "first_observed_tool_call",
                )
            }
            TurnIdentity::WorkspaceFallback {
                workspace_id,
                fallback_id,
            } => (
                TurnKey {
                    workspace_id,
                    session_id: fallback_id,
                    conversation_id: None,
                    turn_id: None,
                },
                false,
                self.clock.now(),
                "workspace_fallback",
            ),
        };

        let now = self.clock.now();
        let mut states = self.states.lock().expect("agent turn budget lock poisoned");
        self.cleanup_stale_states_locked(&mut states, now);

        let workspace_id = key.workspace_id.clone();
        let sanitized_session = Self::sanitize_session_for_log(&key.session_id);
        let traits = get_tool_traits(tool_name, is_external, args);

        let (decision, log_event) = match states.get_mut(&key) {
            None => {
                // 如果是 Browser 模式且有新 key，软关闭同 (workspace_id, session_id) 下残留的旧 Turn 状态
                if is_browser {
                    states.retain(|k, v| {
                        if k.workspace_id == key.workspace_id && k.session_id == key.session_id && *k != key {
                            if v.active_calls > 0 {
                                v.phase = TurnLifecyclePhase::Closing;
                                v.hard_stopped_at = Some(now);
                                true // 保持直到 active_calls 归零
                            } else {
                                false
                            }
                        } else {
                            true
                        }
                    });
                }

                if states.len() >= self.config.max_states {
                    self.evict_oldest_inactive_locked(&mut states);
                }

                let start_time = if is_browser {
                    effective_started_at
                } else {
                    now
                };

                let elapsed = now.saturating_duration_since(start_time);
                let cutoff = self.config.hard_stop_after.saturating_sub(self.config.deadline_reserve);
                if let Some((decision, log_line)) = self.reject_initial_call_if_disallowed(
                    elapsed,
                    cutoff,
                    tool_name,
                    traits,
                    initial_timer_origin,
                    &workspace_id,
                    &sanitized_session,
                ) {
                    (decision, Some(log_line))
                } else {

                    // 首个观测到的调用，创建新 Turn
                    let new_state = AgentTurnState {
                        turn_seq: 1,
                        phase: TurnLifecyclePhase::Active,
                        started_at: start_time,
                        timer_origin: initial_timer_origin,
                        last_call_started_at: now,
                        last_call_completed_at: None,
                        active_calls: 1,
                        warning_emitted: false,
                        wrap_up_emitted: false,
                        finalization_emitted: false,
                        hard_stopped_at: None,
                    };
                    states.insert(key.clone(), new_state);

                    let runtime_budget = cutoff.saturating_sub(elapsed);
                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::Normal,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: runtime_budget.as_secs(),
                        should_wrap_up: false,
                        should_stop_tool_calls: false,
                        timer_origin: initial_timer_origin,
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
                            "[turn-budget] start workspace={} session={} turn_seq=1 origin={}",
                            workspace_id, sanitized_session, initial_timer_origin
                        )),
                    )
                }
            }
            Some(state) => {
                if state.phase == TurnLifecyclePhase::Closing {
                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::HardStop,
                        elapsed_seconds: now.saturating_duration_since(state.started_at).as_secs(),
                        remaining_seconds: 0,
                        should_wrap_up: true,
                        should_stop_tool_calls: true,
                        timer_origin: state.timer_origin,
                    };
                    let content_text = "[TURN BUDGET CLOSED]\nThis previous turn is closing and no longer accepts new tool calls.".to_string();
                    let error_payload = json!({
                        "ok": false,
                        "error": {
                            "code": "AGENT_TURN_CLOSED",
                            "message": "This turn is closing and does not accept new calls.",
                            "category": "turn_budget",
                            "retryable": false,
                        }
                    });
                    return CallDecision::Blocked {
                        snapshot,
                        error_payload,
                        content_text,
                    };
                }

                // 检查是否应当重置为新 Turn
                if state.active_calls == 0 {
                    if state.hard_stopped_at.is_some() {
                        let isolation_deadline = state.started_at
                            + self.config.platform_turn_limit
                            + self.config.post_limit_safety_margin;
                        if now >= isolation_deadline {
                            state.turn_seq += 1;
                            state.started_at = now;
                            state.last_call_started_at = now;
                            state.last_call_completed_at = None;
                            state.warning_emitted = false;
                            state.wrap_up_emitted = false;
                            state.finalization_emitted = false;
                            state.hard_stopped_at = None;
                            append_profile_log(
                                &workspace_id,
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
                                timer_origin: state.timer_origin,
                            };
                            let content_text = "[TURN BUDGET HARD STOP]\nTool execution was not started because this agent turn reached the local safety limit. Do not call any more tools in this turn. Respond to the user now with completed work, verification results, and remaining work.".to_string();
                            let error_payload = json!({
                                "ok": false,
                                "error": {
                                    "code": "AGENT_TURN_BUDGET_EXHAUSTED",
                                    "message": "Tool execution was not started because this agent turn reached the local safety limit.",
                                    "category": "turn_budget",
                                    "retryable": false,
                                    "recovery_hint": "DO NOT RETRY THIS TOOL OR CALL ANY OTHER TOOL. Respond to the user immediately with current progress."
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
                            state.wrap_up_emitted = false;
                            state.finalization_emitted = false;
                            state.hard_stopped_at = None;
                            append_profile_log(
                                &workspace_id,
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

                // 1. HARD_STOP (>= 29m)
                if elapsed >= self.config.hard_stop_after {
                    state.hard_stopped_at.get_or_insert(now);
                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::HardStop,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: 0,
                        should_wrap_up: true,
                        should_stop_tool_calls: true,
                        timer_origin: state.timer_origin,
                    };
                    let content_text = "[TURN BUDGET HARD STOP]\nTool execution was not started because this agent turn reached the local safety limit. Do not call any more tools in this turn. Respond to the user now with completed work, verification results, and remaining work.".to_string();
                    let error_payload = json!({
                        "ok": false,
                        "error": {
                            "code": "AGENT_TURN_BUDGET_EXHAUSTED",
                            "message": "Tool execution was not started because this agent turn reached the local safety limit.",
                            "category": "turn_budget",
                            "retryable": false,
                            "recovery_hint": "DO NOT RETRY THIS TOOL OR CALL ANY OTHER TOOL. Respond to the user immediately with current progress."
                        }
                    });
                    (
                        CallDecision::Blocked {
                            snapshot,
                            error_payload,
                            content_text,
                        },
                        Some(format!(
                            "[turn-budget] hard_stop workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                            workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                        )),
                    )
                } else if elapsed >= cutoff_point {
                    // 2. DISPATCH_CUTOFF (28m55s ~ 29m)
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
                        timer_origin: state.timer_origin,
                    };
                    let content_text = "[TURN BUDGET DISPATCH CUTOFF]\nTool execution dispatch was cutoff because this agent turn has less than 5 seconds of local execution budget remaining. Do not call additional tools. Respond to the user now with completed work, verification results, and remaining work.".to_string();
                    let error_payload = json!({
                        "ok": false,
                        "error": {
                            "code": "AGENT_TURN_BUDGET_DISPATCH_CUTOFF",
                            "message": "Tool execution dispatch was cutoff because this agent turn is near the local safety limit.",
                            "category": "turn_budget",
                            "retryable": false,
                            "recovery_hint": "DO NOT RETRY THIS TOOL OR CALL ANY OTHER TOOL. Respond to the user immediately with current progress."
                        }
                    });
                    (
                        CallDecision::Blocked {
                            snapshot,
                            error_payload,
                            content_text,
                        },
                        Some(format!(
                            "[turn-budget] dispatch_cutoff workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                            workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                        )),
                    )
                } else if elapsed >= self.config.finalization_after {
                    // 3. FINALIZATION (28m ~ 28m55s): 仅允许 finalization_safe 工具
                    if !traits.finalization_safe {
                        let snapshot = TurnBudgetSnapshot {
                            status: TurnBudgetStatus::Finalization,
                            elapsed_seconds: elapsed.as_secs(),
                            remaining_seconds: cutoff_point.saturating_sub(elapsed).as_secs(),
                            should_wrap_up: true,
                            should_stop_tool_calls: false,
                            timer_origin: state.timer_origin,
                        };
                        let content_text = format!(
                            "[TURN BUDGET RESTRICTED]\nTool execution of '{}' was blocked because this turn is in finalization mode (elapsed: {}s). Code changes, new executions, searches, and external tools are no longer allowed. Use only finalization-safe tools (e.g. read_file, git_status, git_diff, history_session_checkpoint, finish_task) to verify the current state, then respond to the user immediately.",
                            tool_name, elapsed.as_secs()
                        );
                        let error_payload = json!({
                            "ok": false,
                            "error": {
                                "code": "AGENT_TURN_WRAP_UP_RESTRICTED",
                                "message": format!("Tool '{}' is blocked in finalization mode.", tool_name),
                                "category": "turn_budget",
                                "retryable": false,
                                "recovery_hint": "Do not retry this tool. This turn is in finalization mode. Code changes, new executions, searches, and external tools are no longer allowed. Use only finalization-safe tools to verify current state, then respond to the user immediately."
                            }
                        });
                        (
                            CallDecision::Restricted {
                                snapshot,
                                error_payload,
                                content_text,
                            },
                            Some(format!(
                                "[turn-budget] tool_rejected_by_finalization workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                                workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                            )),
                        )
                    } else {
                        // 允许 finalization 工具执行
                        state.active_calls += 1;
                        state.last_call_started_at = now;
                        let runtime_budget = cutoff_point.saturating_sub(elapsed);
                        let first_fin = !state.finalization_emitted;
                        state.finalization_emitted = true;

                        let snapshot = TurnBudgetSnapshot {
                            status: TurnBudgetStatus::Finalization,
                            elapsed_seconds: elapsed.as_secs(),
                            remaining_seconds: runtime_budget.as_secs(),
                            should_wrap_up: true,
                            should_stop_tool_calls: false,
                            timer_origin: state.timer_origin,
                        };
                        let guard = TurnCallGuard::new(self.clone(), key.clone());
                        (
                            CallDecision::Allowed {
                                guard,
                                runtime_budget,
                                snapshot,
                                emit_full_warning: false,
                                emit_urgent: first_fin,
                            },
                            if first_fin {
                                Some(format!(
                                    "[turn-budget] urgent_injected workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                                    workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                                ))
                            } else {
                                None
                            },
                        )
                    }
                } else if elapsed >= self.config.wrap_up_after {
                    // 4. WRAP_UP (27m ~ 28m): 禁止 investigation，允许最后修改与验证
                    if !traits.wrap_up_allowed {
                        let snapshot = TurnBudgetSnapshot {
                            status: TurnBudgetStatus::WrapUp,
                            elapsed_seconds: elapsed.as_secs(),
                            remaining_seconds: cutoff_point.saturating_sub(elapsed).as_secs(),
                            should_wrap_up: true,
                            should_stop_tool_calls: false,
                            timer_origin: state.timer_origin,
                        };
                        let content_text = format!(
                            "[TURN BUDGET RESTRICTED]\nTool execution of '{}' was blocked because this turn is in wrap-up mode (elapsed: {}s). New investigation and task expansion are no longer allowed. Finish only the already identified work using finalization-safe tools, then respond to the user.",
                            tool_name, elapsed.as_secs()
                        );
                        let error_payload = json!({
                            "ok": false,
                            "error": {
                                "code": "AGENT_TURN_WRAP_UP_RESTRICTED",
                                "message": format!("Tool '{}' is blocked in wrap-up mode.", tool_name),
                                "category": "turn_budget",
                                "retryable": false,
                                "recovery_hint": "Do not retry this tool. This turn is in wrap-up mode. New investigation is no longer allowed. Finish only the already identified work using finalization-safe tools, then respond to the user."
                            }
                        });
                        (
                            CallDecision::Restricted {
                                snapshot,
                                error_payload,
                                content_text,
                            },
                            Some(format!(
                                "[turn-budget] tool_rejected_by_wrap_up workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                                workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                            )),
                        )
                    } else {
                        state.active_calls += 1;
                        state.last_call_started_at = now;
                        let mut runtime_budget = cutoff_point.saturating_sub(elapsed);
                        if tool_name == "exec_command" {
                            // WRAP_UP exec 强制压缩至最多 30s
                            runtime_budget = runtime_budget.min(Duration::from_secs(30));
                        }

                        let snapshot = TurnBudgetSnapshot {
                            status: TurnBudgetStatus::WrapUp,
                            elapsed_seconds: elapsed.as_secs(),
                            remaining_seconds: runtime_budget.as_secs(),
                            should_wrap_up: true,
                            should_stop_tool_calls: false,
                            timer_origin: state.timer_origin,
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
                            None,
                        )
                    }
                } else if elapsed >= self.config.warning_after {
                    // 5. WARNING (25m ~ 27m): 允许所有工具，但记录 post_warning_investigation_attempt
                    state.active_calls += 1;
                    state.last_call_started_at = now;
                    let runtime_budget = cutoff_point.saturating_sub(elapsed);

                    let first_warning = !state.warning_emitted;
                    state.warning_emitted = true;

                    let mut log_msgs = Vec::new();
                    if first_warning {
                        log_msgs.push(format!(
                            "[turn-budget] warning_injected workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                            workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                        ));
                    }
                    if traits.investigation && !first_warning {
                        log_msgs.push(format!(
                            "[turn-budget] post_warning_investigation_attempt workspace={} session={} turn_seq={} elapsed_secs={} tool={}",
                            workspace_id, sanitized_session, state.turn_seq, elapsed.as_secs(), tool_name
                        ));
                    }

                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::Warning,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: runtime_budget.as_secs(),
                        should_wrap_up: true,
                        should_stop_tool_calls: false,
                        timer_origin: state.timer_origin,
                    };
                    let guard = TurnCallGuard::new(self.clone(), key.clone());
                    (
                        CallDecision::Allowed {
                            guard,
                            runtime_budget,
                            snapshot,
                            emit_full_warning: first_warning,
                            emit_urgent: false,
                        },
                        if !log_msgs.is_empty() {
                            Some(log_msgs.join("\n"))
                        } else {
                            None
                        },
                    )
                } else {
                    // 6. NORMAL (< 25m)
                    state.active_calls += 1;
                    state.last_call_started_at = now;
                    let runtime_budget = cutoff_point.saturating_sub(elapsed);

                    let snapshot = TurnBudgetSnapshot {
                        status: TurnBudgetStatus::Normal,
                        elapsed_seconds: elapsed.as_secs(),
                        remaining_seconds: runtime_budget.as_secs(),
                        should_wrap_up: false,
                        should_stop_tool_calls: false,
                        timer_origin: state.timer_origin,
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
                        None,
                    )
                }
            }
        };

        if let Some(log_line) = log_event {
            append_profile_log(&workspace_id, "mcp-requests.log", &log_line);
        }

        decision
    }

    /// 在工具调用前进行评估与状态登记（兼容旧调用方）。
    /// 缺失 session 也必须进入工作区级 fallback，不能返回 Unmanaged 绕过预算。
    pub fn start_call(
        self: &Arc<Self>,
        workspace_id: &str,
        session_id: Option<&str>,
        tool_name: &str,
        is_external: bool,
        args: &Value,
    ) -> CallDecision {
        let identity = match session_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => TurnIdentity::SessionFallback {
                workspace_id: workspace_id.to_string(),
                session_id: s.to_string(),
            },
            None => TurnIdentity::WorkspaceFallback {
                workspace_id: workspace_id.to_string(),
                fallback_id: format!("unmanaged_ws_{}", workspace_id),
            },
        };
        self.start_call_with_identity(identity, tool_name, is_external, args)
    }

    /// 在工具调用完成时更新 active_calls 和 last_call_completed_at，并在 Closing 状态完全结束时安全回收
    pub fn complete_call(&self, key: &TurnKey, now: Instant) {
        let mut states = self.states.lock().expect("agent turn budget lock poisoned");
        let mut should_remove = false;
        if let Some(state) = states.get_mut(key) {
            state.active_calls = state.active_calls.saturating_sub(1);
            state.last_call_completed_at = Some(now);
            if state.phase == TurnLifecyclePhase::Closing && state.active_calls == 0 {
                should_remove = true;
            }
        }
        if should_remove {
            states.remove(key);
        }
    }

    /// 惰性清理过期与超出容量的状态
    fn cleanup_stale_states_locked(
        &self,
        states: &mut HashMap<TurnKey, AgentTurnState>,
        now: Instant,
    ) {
        states.retain(|_, state| {
            if state.active_calls > 0 {
                // 只要存在活跃调用，无论何种阶段均严格保持存活，确保 Guard 安全析构
                return true;
            }
            if state.phase == TurnLifecyclePhase::Closing {
                // 无活跃调用的 Closing 状态直接清理
                return false;
            }
            let last_active = state
                .last_call_completed_at
                .unwrap_or(state.last_call_started_at);
            now.saturating_duration_since(last_active) < self.config.state_ttl
        });

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

    /// 为被阻断或受限的调用生成统一的返回结构
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
        } else if snapshot.status == TurnBudgetStatus::Finalization || snapshot.status == TurnBudgetStatus::Urgent {
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
