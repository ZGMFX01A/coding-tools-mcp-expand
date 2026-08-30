use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 浏览器端上报的 Turn 生命周期事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTurnEventKind {
    TurnStarted,
    TurnUpdated,
    StreamCompleted,
    ConversationResolved,
    TurnClosed,
}

/// 浏览器端上报的 Turn 生命周期事件 (V2 强类型与校验契约)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTurnEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub tab_instance_id: String,
    pub observer_id: String,
    pub tab_id: u64,
    pub sequence: u64,
    pub event: BrowserTurnEventKind,
    pub workspace_id: String,
    pub conversation_id: Option<String>,
    pub turn_id: String,
    pub request_id: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
}

/// 浏览器 Turn 状态机流转
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTurnStatus {
    /// 当前正在活跃生成/调用中
    Active,
    /// 单个生成流已结束，进入短暂静默窗口（15s 静默窗口内仍可作为候选）
    StreamIdle,
    /// 超过 15 分钟无任何流式/工具事件更新，自动退出候选池
    Stale,
    /// 已被同一 Tab 下一条消息（新 Turn）正式替换
    CompletedByNextTurn,
    /// 页面关闭或显式关闭
    Closed,
    /// 超时过期
    Expired,
}

/// 浏览器 Turn 注册表上下文
#[derive(Debug, Clone)]
pub struct BrowserTurnContext {
    pub workspace_id: String,
    pub observer_id: String,
    pub tab_id: u64,
    pub tab_instance_id: String,
    pub last_applied_sequence: u64,
    pub conversation_id: Option<String>,
    pub turn_id: String,
    pub request_id: Option<String>,
    pub browser_started_at_ms: u64,
    pub effective_started_at: Instant,
    pub timer_origin: &'static str,
    pub server_received_at: Instant,
    pub last_seen_at: Instant,
    pub clock_skew_ms: i64,
    pub status: BrowserTurnStatus,
    pub stream_idle_since: Option<Instant>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
}

/// 浏览器 Turn 注册表，以 (workspace_id, observer_id, tab_instance_id) 为键管理各 Tab 实例的当前 Turn 状态。
/// 数字 tab_id 只作为诊断字段，不能承担 Tab 实例身份。
#[derive(Debug)]
pub struct BrowserTurnRegistry {
    contexts: Mutex<HashMap<(String, String, String), BrowserTurnContext>>,
    processed_events: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
    max_entries: usize,
}

impl Default for BrowserTurnRegistry {
    fn default() -> Self {
        Self::new(Duration::from_secs(2 * 3600), 256)
    }
}

impl BrowserTurnRegistry {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            contexts: Mutex::new(HashMap::new()),
            processed_events: Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    /// 记录来自浏览器的 Turn 事件并更新状态（支持序号单调性、去重、时钟偏斜矫正与严格显式关闭）
    pub fn record_event(
        &self,
        workspace_id: &str,
        event: BrowserTurnEvent,
        now: Instant,
    ) {
        if event.workspace_id != workspace_id {
            return;
        }

        // 1. 事件 ID 去重检查与清理
        {
            let mut seen = self.processed_events.lock().expect("processed_events lock poisoned");
            let event_ttl = Duration::from_secs(600);
            seen.retain(|_, ts| now.saturating_duration_since(*ts) <= event_ttl);
            if seen.contains_key(&event.event_id) {
                // 已处理过的事件，幂等忽略
                return;
            }
            if seen.len() >= 1024 {
                if let Some(oldest_key) = seen.iter().min_by_key(|(_, ts)| **ts).map(|(k, _)| k.clone()) {
                    seen.remove(&oldest_key);
                }
            }
            seen.insert(event.event_id.clone(), now);
        }

        let mut map = self.contexts.lock().expect("browser turn registry lock poisoned");
        self.cleanup_stale_locked(&mut map, now);

        let key = (
            workspace_id.to_string(),
            event.observer_id.clone(),
            event.tab_instance_id.clone(),
        );

        match event.event {
            BrowserTurnEventKind::TurnClosed => {
                if let Some(ctx) = map.get_mut(&key) {
                    // 严格三要素校验：tab_instance_id、turn_id 完全一致且 sequence 递增
                    if ctx.tab_instance_id == event.tab_instance_id
                        && ctx.turn_id == event.turn_id
                        && event.sequence > ctx.last_applied_sequence
                    {
                        ctx.status = BrowserTurnStatus::Closed;
                        ctx.last_seen_at = now;
                        ctx.last_applied_sequence = event.sequence;
                    }
                }
            }
            BrowserTurnEventKind::TurnStarted => {
                if let Some(existing) = map.get_mut(&key) {
                    // 同实例 sequence 单调校验
                    if existing.tab_instance_id == event.tab_instance_id && event.sequence <= existing.last_applied_sequence {
                        return;
                    }
                    if event.started_at < existing.browser_started_at_ms {
                        // 收到旧 Turn 的延迟启动事件，拒绝覆盖新 Turn
                        return;
                    }
                    // 旧 Turn 标记为已被下一轮替换
                    existing.status = BrowserTurnStatus::CompletedByNextTurn;
                }

                // 页面刷新会生成新的 tab_instance_id。将同一 observer/tab 的旧实例
                // 标记为已替换，避免旧实例与新实例同时成为活跃候选。
                for (existing_key, existing) in map.iter_mut() {
                    if existing_key.0 == workspace_id
                        && existing_key.1 == event.observer_id
                        && existing.tab_id == event.tab_id
                        && existing_key.2 != event.tab_instance_id
                    {
                        existing.status = BrowserTurnStatus::CompletedByNextTurn;
                    }
                }

                if map.len() >= self.max_entries && !map.contains_key(&key) {
                    self.evict_oldest_locked(&mut map);
                }

                // Clock Skew 算法：统一 [-30s, +120s] 偏斜窗口
                let server_wall_now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                let skew_ms = server_wall_now_ms - (event.started_at as i64);

                let (effective_started_at, timer_origin) = if (-30_000..=120_000).contains(&skew_ms) {
                    if skew_ms > 0 {
                        let elapsed = Duration::from_millis(skew_ms as u64);
                        (now.checked_sub(elapsed).unwrap_or(now), "browser_observed_turn_start")
                    } else {
                        (now, "browser_observed_turn_start")
                    }
                } else {
                    (now, "server_skew_fallback")
                };

                let ctx = BrowserTurnContext {
                    workspace_id: workspace_id.to_string(),
                    observer_id: event.observer_id,
                    tab_id: event.tab_id,
                    tab_instance_id: event.tab_instance_id,
                    last_applied_sequence: event.sequence,
                    conversation_id: event.conversation_id,
                    turn_id: event.turn_id,
                    request_id: event.request_id,
                    browser_started_at_ms: event.started_at,
                    effective_started_at,
                    timer_origin,
                    server_received_at: now,
                    last_seen_at: now,
                    clock_skew_ms: skew_ms,
                    status: BrowserTurnStatus::Active,
                    stream_idle_since: None,
                    requested_model: event.requested_model,
                    actual_model: event.actual_model,
                };
                map.insert(key, ctx);
            }
            BrowserTurnEventKind::TurnUpdated => {
                if let Some(ctx) = map.get_mut(&key) {
                    if ctx.tab_instance_id != event.tab_instance_id {
                        return;
                    }
                    if event.sequence <= ctx.last_applied_sequence {
                        return;
                    }
                    if ctx.turn_id == event.turn_id {
                        if event.conversation_id.is_some() {
                            ctx.conversation_id = event.conversation_id;
                        }
                        if event.actual_model.is_some() {
                            ctx.actual_model = event.actual_model;
                        }
                        if event.requested_model.is_some() {
                            ctx.requested_model = event.requested_model;
                        }
                        if event.request_id.is_some() {
                            ctx.request_id = event.request_id;
                        }
                        ctx.last_seen_at = now;
                        ctx.last_applied_sequence = event.sequence;
                        // 若处于 StreamIdle 或 Stale，有新更新则重新激活为 Active
                        if ctx.status == BrowserTurnStatus::StreamIdle || ctx.status == BrowserTurnStatus::Stale {
                            ctx.status = BrowserTurnStatus::Active;
                            ctx.stream_idle_since = None;
                        }
                    }
                }
            }
            BrowserTurnEventKind::ConversationResolved => {
                if let Some(ctx) = map.get_mut(&key) {
                    if ctx.tab_instance_id != event.tab_instance_id {
                        return;
                    }
                    if event.sequence <= ctx.last_applied_sequence {
                        return;
                    }
                    if ctx.turn_id == event.turn_id {
                        // 仅当原始 conversation_id 为空时允许 resolve，绝不允许历史导航覆盖已有 ID
                        if ctx.conversation_id.is_none() && event.conversation_id.is_some() {
                            ctx.conversation_id = event.conversation_id;
                        }
                        ctx.last_seen_at = now;
                        ctx.last_applied_sequence = event.sequence;
                        if ctx.status == BrowserTurnStatus::StreamIdle || ctx.status == BrowserTurnStatus::Stale {
                            ctx.status = BrowserTurnStatus::Active;
                            ctx.stream_idle_since = None;
                        }
                    }
                }
            }
            BrowserTurnEventKind::StreamCompleted => {
                if let Some(ctx) = map.get_mut(&key) {
                    if ctx.tab_instance_id != event.tab_instance_id {
                        return;
                    }
                    if event.sequence <= ctx.last_applied_sequence {
                        return;
                    }
                    if ctx.turn_id == event.turn_id {
                        ctx.status = BrowserTurnStatus::StreamIdle;
                        ctx.stream_idle_since = Some(now);
                        ctx.last_seen_at = now;
                        ctx.last_applied_sequence = event.sequence;
                    }
                }
            }
        }
    }

    /// 获取特定工作区内符合条件的活跃候选 Turn（必须限制在同 workspace_id 内）
    pub fn get_active_candidates(&self, workspace_id: &str, now: Instant) -> Vec<BrowserTurnContext> {
        let quiet_window = Duration::from_secs(15);
        let stale_timeout = Duration::from_secs(15 * 60); // 15 分钟无活动转为 Stale
        let mut map = self.contexts.lock().expect("browser turn registry lock poisoned");

        // 状态动态流转检测
        for ctx in map.values_mut() {
            if ctx.status == BrowserTurnStatus::Active && now.saturating_duration_since(ctx.last_seen_at) > stale_timeout {
                ctx.status = BrowserTurnStatus::Stale;
            }
        }

        map.values()
            .filter(|ctx| {
                if ctx.workspace_id != workspace_id {
                    return false;
                }
                match ctx.status {
                    BrowserTurnStatus::Active => true,
                    BrowserTurnStatus::StreamIdle => {
                        if let Some(idle_since) = ctx.stream_idle_since {
                            now.saturating_duration_since(idle_since) <= quiet_window
                        } else {
                            false
                        }
                    }
                    BrowserTurnStatus::Stale
                    | BrowserTurnStatus::CompletedByNextTurn
                    | BrowserTurnStatus::Closed
                    | BrowserTurnStatus::Expired => false,
                }
            })
            .cloned()
            .collect()
    }

    /// 根据 key 直接获取特定 Turn 上下文
    pub fn get_turn_context(
        &self,
        workspace_id: &str,
        observer_id: &str,
        tab_id: u64,
    ) -> Option<BrowserTurnContext> {
        let map = self.contexts.lock().expect("browser turn registry lock poisoned");
        map.values()
            .find(|ctx| {
                ctx.workspace_id == workspace_id
                    && ctx.observer_id == observer_id
                    && ctx.tab_id == tab_id
            })
            .cloned()
    }

    pub fn get_turn_context_by_instance(
        &self,
        workspace_id: &str,
        observer_id: &str,
        tab_instance_id: &str,
    ) -> Option<BrowserTurnContext> {
        let map = self.contexts.lock().expect("browser turn registry lock poisoned");
        map.get(&(
            workspace_id.to_string(),
            observer_id.to_string(),
            tab_instance_id.to_string(),
        ))
        .cloned()
    }

    fn cleanup_stale_locked(
        &self,
        map: &mut HashMap<(String, String, String), BrowserTurnContext>,
        now: Instant,
    ) {
        map.retain(|_, ctx| now.saturating_duration_since(ctx.last_seen_at) <= self.ttl);
    }

    fn evict_oldest_locked(
        &self,
        map: &mut HashMap<(String, String, String), BrowserTurnContext>,
    ) {
        if let Some(oldest_key) = map
            .iter()
            .min_by_key(|(_, ctx)| ctx.last_seen_at)
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest_key);
        }
    }
}

/// 关联置信度模型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationConfidence {
    /// 命中已有 session -> Turn 绑定缓存，直接复用
    ExistingBinding,
    /// 第一次 MCP 调用时恰好存在唯一合理活跃候选，建立启发式绑定
    SingleActiveCandidate,
    /// 存在多个活跃候选 Tab 产生歧义，必须安全 fallback 绝不错杀
    Ambiguous,
    /// 没有任何活跃候选（Observer 离线），完全 fallback
    None,
}

/// Session 绑定记录
#[derive(Debug, Clone)]
struct SessionBinding {
    observer_id: String,
    tab_id: u64,
    tab_instance_id: String,
    conversation_id: Option<String>,
    turn_id: String,
    #[allow(dead_code)]
    bound_at: Instant,
    last_used_at: Instant,
}

/// 解析后的 Turn 唯一标识 (支持 Browser、SessionFallback 与独立的 WorkspaceFallback)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TurnIdentity {
    Browser {
        workspace_id: String,
        session_id: String,
        conversation_id: String,
        turn_id: String,
        effective_started_at: Instant,
        timer_origin: &'static str,
    },
    SessionFallback {
        workspace_id: String,
        session_id: String,
    },
    WorkspaceFallback {
        workspace_id: String,
        fallback_id: String,
    },
}

impl TurnIdentity {
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::Browser { workspace_id, .. }
            | Self::SessionFallback { workspace_id, .. }
            | Self::WorkspaceFallback { workspace_id, .. } => workspace_id,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Browser { session_id, .. } | Self::SessionFallback { session_id, .. } => {
                Some(session_id)
            }
            Self::WorkspaceFallback { fallback_id, .. } => Some(fallback_id),
        }
    }

    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::Browser { conversation_id, .. } => Some(conversation_id),
            Self::SessionFallback { .. } | Self::WorkspaceFallback { .. } => None,
        }
    }

    pub fn turn_id(&self) -> Option<&str> {
        match self {
            Self::Browser { turn_id, .. } => Some(turn_id),
            Self::SessionFallback { .. } | Self::WorkspaceFallback { .. } => None,
        }
    }
}

/// Turn 关联器，负责在 MCP tools/call 与 BrowserTurnRegistry 之间建立安全绑定与解析
#[derive(Debug)]
pub struct TurnCorrelator {
    bindings: Mutex<HashMap<(String, String), SessionBinding>>,
    binding_ttl: Duration,
}

impl Default for TurnCorrelator {
    fn default() -> Self {
        Self::new(Duration::from_secs(600)) // 10 分钟绑定 TTL
    }
}

impl TurnCorrelator {
    pub fn new(binding_ttl: Duration) -> Self {
        Self {
            bindings: Mutex::new(HashMap::new()),
            binding_ttl,
        }
    }

    /// 关联 MCP session 与 Browser Turn 注册表（严格多候选 Ambiguous 降级优先状态机）
    pub fn correlate(
        &self,
        workspace_id: &str,
        session_id: &str,
        registry: &BrowserTurnRegistry,
        now: Instant,
    ) -> (CorrelationConfidence, TurnIdentity) {
        let binding_key = (workspace_id.to_string(), session_id.to_string());
        let mut bindings = self.bindings.lock().expect("turn correlator lock poisoned");

        // 1. 查询当前工作区内所有活跃候选
        let candidates = registry.get_active_candidates(workspace_id, now);

        // 2. 多候选歧义 -> 无论之前是否存在 binding，必须强制安全 Ambiguous fallback，避免误杀其他 Tab！
        if candidates.len() > 1 {
            bindings.remove(&binding_key);
            return (
                CorrelationConfidence::Ambiguous,
                TurnIdentity::SessionFallback {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                },
            );
        }

        // 3. 恰好有一个活跃候选（单候选启发式 HeuristicSingleActiveCandidate）
        if candidates.len() == 1 {
            let cand = &candidates[0];
            let conv_id = cand.conversation_id.clone().unwrap_or_else(|| "unknown_conv".to_string());

            // 检查已有绑定是否与该唯一候选完全匹配（比对 observer_id, tab_id, tab_instance_id, turn_id, conversation_id）
            if let Some(binding) = bindings.get_mut(&binding_key) {
                if now.saturating_duration_since(binding.last_used_at) <= self.binding_ttl
                    && binding.observer_id == cand.observer_id
                    && binding.tab_id == cand.tab_id
                    && binding.tab_instance_id == cand.tab_instance_id
                    && binding.turn_id == cand.turn_id
                    && binding.conversation_id == cand.conversation_id
                    && cand.status != BrowserTurnStatus::CompletedByNextTurn
                    && cand.status != BrowserTurnStatus::Closed
                    && cand.status != BrowserTurnStatus::Stale
                {
                    binding.last_used_at = now;
                    return (
                        CorrelationConfidence::ExistingBinding,
                        TurnIdentity::Browser {
                            workspace_id: workspace_id.to_string(),
                            session_id: session_id.to_string(),
                            conversation_id: conv_id,
                            turn_id: cand.turn_id.clone(),
                            effective_started_at: cand.effective_started_at,
                            timer_origin: cand.timer_origin,
                        },
                    );
                }
            }

            // 新建立启发式绑定
            let new_binding = SessionBinding {
                observer_id: cand.observer_id.clone(),
                tab_id: cand.tab_id,
                tab_instance_id: cand.tab_instance_id.clone(),
                conversation_id: cand.conversation_id.clone(),
                turn_id: cand.turn_id.clone(),
                bound_at: now,
                last_used_at: now,
            };
            bindings.insert(binding_key, new_binding);

            return (
                CorrelationConfidence::SingleActiveCandidate,
                TurnIdentity::Browser {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                    conversation_id: conv_id,
                    turn_id: cand.turn_id.clone(),
                    effective_started_at: cand.effective_started_at,
                    timer_origin: cand.timer_origin,
                },
            );
        }

        // 4. 没有候选 (candidates.len() == 0)
        bindings.remove(&binding_key);
        (
            CorrelationConfidence::None,
            TurnIdentity::SessionFallback {
                workspace_id: workspace_id.to_string(),
                session_id: session_id.to_string(),
            },
        )
    }
}
