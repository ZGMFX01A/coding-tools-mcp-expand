# Agent Turn Budget（ChatGPT 单轮工具执行时间保护）

## 1. 概述与核心背景

ChatGPT 网页端对单轮 Agent 执行（思考链 + 连续工具调用）有约 30 分钟的硬性限制。如果单轮工具调用超时，执行会被强制断开，并可能损坏该会话后续的 MCP 工具连接。

Coding Tools MCP 提供了本地 **Agent Turn Budget** 机制，在本地尽最大可能避免 Agent 工具调用跨过平台的 30 分钟硬限制。

> [!IMPORTANT]
> **关键设计语义**：
> 1. **计时原点**：`timer_origin = "first_observed_tool_call"`（从本地 MCP 服务端观察到的首个工具调用开始计时，不代表用户在 ChatGPT 网页端发送消息的真实物理时间）。
> 2. **近似判定声明**：ChatGPT 请求稳定携带 `params._meta["openai/session"]`（对话 ID），但协议未提供 `turn_id` / `run_id`。系统基于 `openai/session` + 动态空闲窗口（Dynamic Idle Gap Heuristic）进行本地近似 Agent Turn 管理。
> 3. **保守截止原则**：*Turn detection is deliberately conservative near the deadline.* 越靠近截止时间，状态重置判定越保守，防止因为 Agent 内部深度推理暂停而误将预算洗掉。
> 4. **不中断基础设施**：29 分钟 Hard Stop 或 28m55s Dispatch Cutoff 仅在网关层短路阻断新工具启动并返回收尾指引，**绝对不关闭 HTTP Listener、不断开 FRP/Cloudflare 隧道、不 kill External MCP 进程、不退出 Tauri 桌面端**。

---

## 2. 状态机与阶段划分

| 阶段 | 时间范围（生产默认） | 行为说明 | 机器状态 `status` |
|---|---|---|---|
| **Normal** | $0 \le elapsed < 25\text{min}$ | 工具正常执行，注入顶层 `_meta`。 | `"normal"` |
| **Warning** | $25\text{min} \le elapsed < 28\text{min}$ | 工具正常执行，首次跨越时置顶注入长文本 `[TURN BUDGET WARNING]`，要求 Agent 尽快收尾。 | `"warning"` |
| **Urgent** | $28\text{min} \le elapsed < 28\text{min}55\text{s}$ | 工具正常执行，每次置顶注入短提醒 `[TURN BUDGET URGENT]`，进入紧急收尾。 | `"urgent"` |
| **Dispatch Cutoff** | $28\text{min}55\text{s} \le elapsed < 29\text{min}$ | 预留 5 秒安全裕量，禁止启动任何新的 Core / External 工具，立即返回收尾要求。 | `"dispatch_cutoff"` |
| **Hard Stop** | $elapsed \ge 29\text{min}$ | 短路拦截所有工具调用，返回 `isError: true` 与 `AGENT_TURN_BUDGET_EXHAUSTED` 错误。 | `"hard_stop"` |

---

## 3. 核心机制详解

### 3.1 动态 Idle Reset 与平台隔离线恢复

- **早期（$elapsed < 20\text{min}$）**：
  若工具调用完成后的空闲间隔 $\ge 90\text{s}$，下一次调用自动重置为新 Turn（`turn_seq += 1`, `started_at = now`）。
- **中期（$20\text{min} \le elapsed < 25\text{min}$）**：
  允许更大的推理停顿，空闲间隔阈值提升至 $180\text{s}$。
- **晚期（$elapsed \ge 25\text{min}$）**：
  **严禁仅靠普通 idle gap 重置 Turn**，保持当前计时直到跨越平台隔离线。
- **HardStop 后的平台隔离线恢复**：
  必须同时满足：
  $$now \ge state.started\_at + 30\text{min} + 15\text{s}$$
  且 $active\_calls == 0$ 时，下一次调用才允许自动开启新 Turn。
  被 HardStop 拒绝的调用绝对不刷新任何时间戳。

### 3.2 顶层 `_meta` 注入与第三方 Schema 保护

为确保 External MCP（如 `fast-context`）声明的 strict `outputSchema` 不被破坏：
- External MCP 的 `structuredContent` 原封不动保留。
- 机器元数据统一放入 Tool Result 顶层的 `_meta["coding-tools/agentTurnBudget"]`：
  ```json
  {
    "content": [
      {
        "type": "text",
        "text": "[TURN BUDGET WARNING]...\nOriginal tool output"
      }
    ],
    "structuredContent": { ... },
    "_meta": {
      "coding-tools/agentTurnBudget": {
        "status": "warning",
        "elapsedSeconds": 1501,
        "remainingSeconds": 234,
        "shouldWrapUp": true,
        "shouldStopToolCalls": false,
        "timerOrigin": "first_observed_tool_call"
      }
    },
    "isError": false
  }
  ```

### 3.3 RAII `TurnCallGuard` 保证活跃计数清零

`start_call` 成功时返回 `TurnCallGuard`，实现 `Drop` trait。无论在 Core 工具正常返回、外部超时、权限错误或 Panic Unwind，Guard 在离开作用域时必定自动获取锁将 `active_calls -= 1` 并记录 `last_call_completed_at = now`，杜绝任何状态泄漏。

### 3.4 超时动态削峰（External MCP & exec_command）

- **External MCP**：
  $$effective\_timeout = \min(configured\_timeout, runtime\_budget)$$
  超时发生时仅移除该次 pending RPC 等待，**绝对不调用 `transport.kill()`**，保持外部 MCP 进程与 PID 稳定。
- **exec_command**：
  $$effective\_timeout = \min(requested\_timeout, runtime\_budget)$$
  超时到期安全调用 `session.kill_and_wait()` 终止子进程树，防止孤儿进程。
