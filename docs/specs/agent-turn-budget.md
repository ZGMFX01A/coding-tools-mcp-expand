# Agent Turn Budget 与 ChatGPT Turn Observer 完整规范

## 1. 概述与核心定位

针对 ChatGPT 网页端单轮 Agent 执行约 30 分钟被平台强制切断的痛点，Coding Tools MCP 提供深度协同防护：
1. **ChatGPT Turn Observer 前端扩展**：在浏览器沙箱中监听真实对话流与用户动作，向 MCP 本地服务上报高精度 Turn 生命周期事件。
2. **Turn Correlator 关联消解引擎**：将无状态或带有 session 的 MCP 工具请求与浏览器当前真实 Turn 进行高可靠单向关联，支持单候选优先、多 Tab 竞争安全降级与会话切换硬重置。
3. **Agent Turn Budget 渐进式管控状态机**：基于真实对话发起时间（或工具观测基准时间）与动态空闲窗口，主动在 25m 发出警告、27m 切断探索、28m 冻结写操作并保留收尾工具、28m55s 网关级短路阻断，并在新 Turn 开启时对旧 Turn 实施软关闭保护与安全回收。

---

## 2. ChatGPT Turn Observer 协议规范 (V1)

### 2.1 规范与事件强类型
- `schema_version`: 固定整数 `1`
- `event`: 严格强类型枚举 `turn_started` | `turn_updated` | `stream_completed` | `conversation_resolved` | `turn_closed`
- `event_id`: 客户端为每个事件生成的唯一 UUIDv4
- `tab_instance_id`: 每次标签页加载/刷新时生成的唯一种子 UUIDv4
- `sequence`: 单个 tab_instance 内从 1 开始严格单调递增的正整数
- `workspace_id`: 与本地 MCP 握手确认的工作区标识，严禁跨工作区篡改

### 2.2 载荷结构
```json
{
  "schema_version": 1,
  "event_id": "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d",
  "tab_instance_id": "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
  "observer_id": "obs_67b93198_a1",
  "tab_id": 101,
  "sequence": 1,
  "event": "turn_started",
  "workspace_id": "ws_default_123",
  "conversation_id": "67b93198-abc-123",
  "turn_id": "msg-user-submit-101",
  "request_id": "req-999-stream",
  "started_at": 1709123456789,
  "completed_at": null,
  "requested_model": "gpt-4o",
  "actual_model": "gpt-5.6-sol-preview"
}
```

### 2.3 核心生命周期行为
- **严格 isNewUserTurn 判定**：仅当 HTTP 请求 `action` 在白名单 (`next`, `create`)、存在且最后一个消息为 `user` 角色、且该用户消息 ID 未在 LRU 中出现时才派发 `turn_started`；继续生成、重试、分支、工具回调严禁误触发新 Turn。
- **可靠完成上报**：在 SSE/WS 收到 `[DONE]` 终止符或静默超时（默认 1000ms）后派发 `stream_completed`，避免服务端长时间悬挂。
- **页面销毁与导航清理**：当用户切换会话 URL、刷新或关闭 Tab 时，派发 `turn_closed` 事件并重置本地状态。
- **Outbox 队列与错误隔离**：浏览器端维护容量受限的 Outbox 串行发送队列，网络错误重试并复用相同 `event_id` 与 `sequence`；普通更新可在队列溢出时淘汰，但 `turn_started`、`stream_completed`、`turn_closed` 等生命周期关键事件不得被淘汰。若 Local 服务返回 `400` / `401` / `403` / `409` / `422` 业务拒绝状态码，立即停止并报错，绝不向 Remote 冗余重复投递。

---

## 3. Turn Correlator 与多 Tab 冲突消解

1. **单候选匹配 (`SingleActiveCandidate`)**：当前工作区仅存在 1 个处于活跃或静默保护窗口内的浏览器 Turn，直接绑定。
2. **已有绑定复用 (`ExistingBinding`)**：多次连续工具调用复用已建立的绑定；若对应的浏览器 Turn 仍处于活跃状态且无新 Tab 竞争，维持高置信度。
3. **多 Tab 竞争降级 (`Ambiguous`)**：若检测到工作区内存在多个活跃候选 Tab（例如用户在两个标签页同时向 ChatGPT 提问），系统**立即解除现有绑定并降级为 `SessionFallback`**，避免误关联到错误会话。
4. **缺少 Session 隔离 (`WorkspaceFallback`)**：若 MCP 请求未携带 `_meta["openai/session"]`，系统使用稳定的 `fallback_id: format!("unmanaged_ws_{workspace_id}")`，让同一工作区的连续无 Session 调用累计进入受控预算；不同工作区使用不同预算桶，严禁绕过管控。

---

## 4. 五阶段渐进式工具管控矩阵

| 阶段 (`Phase`) | 时间范围（生产默认） | 核心管控目标 | 允许的工具类别 | 被禁止的工具 (`AGENT_TURN_WRAP_UP_RESTRICTED`) |
|---|---|---|---|---|
| **NORMAL** | $0 \le elapsed < 25\text{min}$ | 正常执行全量工具 | 全量 Core / External 工具 | 无 |
| **WARNING** | $25\text{min} \le elapsed < 27\text{min}$ | 提示 Agent 规划收尾，记录 `post_warning_investigation_attempt` | 全量工具放行，首次置顶长文本 `[TURN BUDGET WARNING]` | 无 |
| **WRAP_UP** | $27\text{min} \le elapsed < 28\text{min}$ | **切断一切探索源头**，允许完成最后一次修改与轻量验证 | `apply_patch`、轻量验证 `exec_command`（$\le 30\text{s}$）、`read_file`、`git_status`、`git_diff`、`checkpoint`、`finish_task` | `search_text`、`grep_text`、`list_files`、`history_session_search`、`fast_context_search` 及所有外部 MCP |
| **FINALIZATION** | $28\text{min} \le elapsed < 28\text{min}55\text{s}$ | **冻结代码修改**，仅限生成最终汇报所必需的事实确认 | `read_file`、`git_status`、`git_diff`、`read_output`、`history_session_checkpoint`、`finish_task`、`kill_session` (cleanup) | `apply_patch`、所有 `exec_command`、所有探索与外部 MCP |
| **DISPATCH_CUTOFF** | $28\text{min}55\text{s} \le elapsed < 29\text{min}$ | 预留 5 秒网络安全裕量，网关层短路拦截新工具 | 无 | 全部工具 |
| **HARD_STOP** | $elapsed \ge 29\text{min}$ | 短路拦截，返回 `retryable: false` | 无 | 全部工具 |

---

## 5. Turn 软关闭状态机与资源回收

```
[ Active Turn ]
      │
      │ (新 Turn 到达，且旧 Turn 存在活跃执行 active_calls > 0)
      ▼
[ Closing Turn ] ────> 拒绝所有新进入的工具调用 (Blocked)
      │
      │ (RAII TurnCallGuard::drop，active_calls 递减至 0)
      ▼
[ Reclaimed / Removed ]
```

- 若在旧 Turn 执行期间新的 Turn 请求到达，旧 Turn 进入 `Closing` 状态，保护正在执行的长任务安全完成。
- 新 Turn 立即以全新的 TurnKey 独立启动，获得重置的完整预算时间。
- 外部 MCP 工具调用设置 `runtime_budget`，超时后安全返回并记录日志，防止进程残留。
