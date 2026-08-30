use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 浏览器端上报的 Turn 生命周期事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserTurnEvent {
    pub schema_version: u32,
    pub observer_id: String,
    pub tab_id: u64,
    pub event: String,
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
    /// 单个生成流已结束，进入短暂静默窗口（静默窗口内仍可关联同轮新流）
    StreamIdle,
    /// 已被同一 Tab 下一条消息（新 Turn）正式替换
    CompletedByNextTurn,
    /// 超时过期
    Expired,
}

/// 浏览器 Turn 注册表上下文
#[derive(Debug, Clone)]
pub struct BrowserTurnContext {
    pub workspace_id: String,
    pub observer_id: String,
    pub tab_id: u64,
    pub conversation_id: Option<String>,
    pub turn_id: String,
    pub request_id: Option<String>,
    pub browser_started_at_ms: u64,
    pub effective_started_at: Instant,
    pub timer_origin: &'static str,
    pub server_received_at: Instant,
    pub last_seen_at: Instant,
    pub status: BrowserTurnStatus,
    pub stream_idle_since: Option<Instant>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
}

/// 浏览器 Turn 注册表，以 (workspace_id, observer_id, tab_id) 为键管理各 Tab 的当前 Turn 状态
#[derive(Debug)]
pub struct BrowserTurnRegistry {
    contexts: Mutex<HashMap<(String, String, u64), BrowserTurnContext>>,
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
            ttl,
            max_entries,
        }
    }

    /// 记录来自浏览器的 Turn 事件并更新状态
    pub fn record_event(
        &self,
        workspace_id: &str,
        event: BrowserTurnEvent,
        now: Instant,
    ) {
        let mut map = self.contexts.lock().expect("browser turn registry lock poisoned");
        self.cleanup_stale_locked(&mut map, now);

        let key = (
            workspace_id.to_string(),
            event.observer_id.clone(),
            event.tab_id,
        );

        match event.event.as_str() {
            "turn_started" => {
                if map.len() >= self.max_entries && !map.contains_key(&key) {
                    self.evict_oldest_locked(&mut map);
                }

                // 计算有效 started_at（若时间偏差正常采用 browser_observed_turn_start，否则 server fallback）
                let (effective_started_at, timer_origin) = (now, "browser_observed_turn_start");

                let ctx = BrowserTurnContext {
                    workspace_id: workspace_id.to_string(),
                    observer_id: event.observer_id,
                    tab_id: event.tab_id,
                    conversation_id: event.conversation_id,
                    turn_id: event.turn_id,
                    request_id: event.request_id,
                    browser_started_at_ms: event.started_at,
                    effective_started_at,
                    timer_origin,
                    server_received_at: now,
                    last_seen_at: now,
                    status: BrowserTurnStatus::Active,
                    stream_idle_since: None,
                    requested_model: event.requested_model,
                    actual_model: event.actual_model,
                };
                map.insert(key, ctx);
            }
            "turn_updated" | "conversation_resolved" => {
                if let Some(ctx) = map.get_mut(&key) {
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
                        // 若处于 stream_idle，有更新则重新激活
                        if ctx.status == BrowserTurnStatus::StreamIdle {
                            ctx.status = BrowserTurnStatus::Active;
                            ctx.stream_idle_since = None;
                        }
                    }
                }
            }
            "stream_completed" => {
                if let Some(ctx) = map.get_mut(&key) {
                    if ctx.turn_id == event.turn_id {
                        ctx.status = BrowserTurnStatus::StreamIdle;
                        ctx.stream_idle_since = Some(now);
                        ctx.last_seen_at = now;
                    }
                }
            }
            _ => {}
        }
    }

    /// 获取特定工作区内符合条件的活跃候选 Turn（必须限制在同 workspace_id 内）
    pub fn get_active_candidates(&self, workspace_id: &str, now: Instant) -> Vec<BrowserTurnContext> {
        let quiet_window = Duration::from_secs(15);
        let map = self.contexts.lock().expect("browser turn registry lock poisoned");

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
                    BrowserTurnStatus::CompletedByNextTurn | BrowserTurnStatus::Expired => false,
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
        map.get(&(workspace_id.to_string(), observer_id.to_string(), tab_id))
            .cloned()
    }

    fn cleanup_stale_locked(
        &self,
        map: &mut HashMap<(String, String, u64), BrowserTurnContext>,
        now: Instant,
    ) {
        map.retain(|_, ctx| now.saturating_duration_since(ctx.last_seen_at) <= self.ttl);
    }

    fn evict_oldest_locked(
        &self,
        map: &mut HashMap<(String, String, u64), BrowserTurnContext>,
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
    /// 第一次 MCP 调用时恰好存在唯一合理活跃候选，建立新绑定
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
    conversation_id: Option<String>,
    turn_id: String,
    bound_at: Instant,
    last_used_at: Instant,
}

/// 解析后的 Turn 唯一标识
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
}

impl TurnIdentity {
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::Browser { workspace_id, .. } | Self::SessionFallback { workspace_id, .. } => {
                workspace_id
            }
        }
    }

    pub fn session_id(&self) -> &str {
        match self {
            Self::Browser { session_id, .. } | Self::SessionFallback { session_id, .. } => {
                session_id
            }
        }
    }

    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::Browser { conversation_id, .. } => Some(conversation_id),
            Self::SessionFallback { .. } => None,
        }
    }

    pub fn turn_id(&self) -> Option<&str> {
        match self {
            Self::Browser { turn_id, .. } => Some(turn_id),
            Self::SessionFallback { .. } => None,
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

    /// 关联 MCP session 与 Browser Turn 注册表
    pub fn correlate(
        &self,
        workspace_id: &str,
        session_id: &str,
        registry: &BrowserTurnRegistry,
        now: Instant,
    ) -> (CorrelationConfidence, TurnIdentity) {
        let binding_key = (workspace_id.to_string(), session_id.to_string());
        let mut bindings = self.bindings.lock().expect("turn correlator lock poisoned");

        // 1. 检查已有绑定
        if let Some(binding) = bindings.get_mut(&binding_key) {
            if now.saturating_duration_since(binding.last_used_at) <= self.binding_ttl {
                if let Some(ctx) = registry.get_turn_context(
                    workspace_id,
                    &binding.observer_id,
                    binding.tab_id,
                ) {
                    // 确认该 Tab 的 turn_id 与 conversation_id 仍一致
                    if ctx.turn_id == binding.turn_id
                        && ctx.status != BrowserTurnStatus::CompletedByNextTurn
                    {
                        binding.last_used_at = now;
                        let conv_id = ctx.conversation_id.unwrap_or_else(|| "unknown_conv".to_string());
                        return (
                            CorrelationConfidence::ExistingBinding,
                            TurnIdentity::Browser {
                                workspace_id: workspace_id.to_string(),
                                session_id: session_id.to_string(),
                                conversation_id: conv_id,
                                turn_id: ctx.turn_id,
                                effective_started_at: ctx.effective_started_at,
                                timer_origin: ctx.timer_origin,
                            },
                        );
                    }
                }
            }
            // 绑定失效或已变更，清除旧绑定
            bindings.remove(&binding_key);
        }

        // 2. 在同工作区内查找活跃候选
        let candidates = registry.get_active_candidates(workspace_id, now);

        if candidates.len() == 1 {
            let cand = &candidates[0];
            let conv_id = cand.conversation_id.clone().unwrap_or_else(|| "unknown_conv".to_string());
            let new_binding = SessionBinding {
                observer_id: cand.observer_id.clone(),
                tab_id: cand.tab_id,
                conversation_id: cand.conversation_id.clone(),
                turn_id: cand.turn_id.clone(),
                bound_at: now,
                last_used_at: now,
            };
            bindings.insert(binding_key, new_binding);

            (
                CorrelationConfidence::SingleActiveCandidate,
                TurnIdentity::Browser {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                    conversation_id: conv_id,
                    turn_id: cand.turn_id.clone(),
                    effective_started_at: cand.effective_started_at,
                    timer_origin: cand.timer_origin,
                },
            )
        } else if candidates.len() > 1 {
            // 多候选歧义 -> 安全 fallback 避免误杀其他 Tab
            (
                CorrelationConfidence::Ambiguous,
                TurnIdentity::SessionFallback {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                },
            )
        } else {
            // 无候选 -> 安全 fallback
            (
                CorrelationConfidence::None,
                TurnIdentity::SessionFallback {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                },
            )
        }
    }
}
