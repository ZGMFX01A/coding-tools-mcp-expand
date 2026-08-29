# Agent Turn Budget 与 Tool Policy 规范

## 1. 概述与核心定位

针对 ChatGPT 网页端单轮 Agent 执行约 30 分钟被平台强制切断的痛点，Coding Tools MCP 提供两层本地防护机制：
1. **Agent Turn Budget 状态机**：基于首个可观察工具调用（`timer_origin = first_observed_tool_call`）与动态空闲窗口，维护轮次生命周期与平台隔离线恢复。
2. **Turn Budget Tool Policy 渐进式工具管控**：不再单方面依赖 Warning 提示词让 Agent 自觉收尾，而在 27 分钟开始主动切断探索源头，28 分钟开始冻结写操作并只保留最小收尾工具。

---

## 2. 五阶段渐进式工具管控矩阵

| 阶段 (`Phase`) | 时间范围（生产默认） | 核心管控目标 | 允许的工具类别 | 被禁止的工具 (`AGENT_TURN_WRAP_UP_RESTRICTED`) |
|---|---|---|---|---|
| **NORMAL** | $0 \le elapsed < 25\text{min}$ | 正常执行全量工具 | 全量 Core / External 工具 | 无 |
| **WARNING** | $25\text{min} \le elapsed < 27\text{min}$ | 提示 Agent 规划收尾，记录 `post_warning_investigation_attempt` | 全量工具放行，首次置顶长文本 `[TURN BUDGET WARNING]` | 无 |
| **WRAP_UP** | $27\text{min} \le elapsed < 28\text{min}$ | **切断一切探索源头**，允许完成最后一次修改与轻量验证 | `apply_patch`、轻量验证 `exec_command`（$\le 30\text{s}$）、`read_file`、`git_status`、`git_diff`、`checkpoint`、`finish_task` | `search_text`、`grep_text`、`list_files`、`history_session_search`、`fast_context_search` 及所有外部 MCP |
| **FINALIZATION** | $28\text{min} \le elapsed < 28\text{min}55\text{s}$ | **冻结代码修改**，仅限生成最终汇报所必需的事实确认 | `read_file`、`git_status`、`git_diff`、`read_output`、`history_session_checkpoint`、`finish_task`、`kill_session` (cleanup) | `apply_patch`、所有 `exec_command`、所有探索与外部 MCP |
| **DISPATCH_CUTOFF** | $28\text{min}55\text{s} \le elapsed < 29\text{min}$ | 预留 5 秒网络安全裕量，网关层短路拦截新工具 | 无 | 全部工具 |
| **HARD_STOP** | $elapsed \ge 29\text{min}$ | 短路拦截，返回 `retryable: false` | 无 | 全部工具 |

---

## 3. ToolBudgetTraits 属性分类

```rust
pub struct ToolBudgetTraits {
    pub investigation: bool,
    pub mutating: bool,
    pub starts_new_work: bool,
    pub verification_safe: bool,
    pub wrap_up_allowed: bool,
    pub finalization_safe: bool,
}
```

- **`history_session_checkpoint` / `finish_task`**：`mutating: true, finalization_safe: true, wrap_up_allowed: true`（特例收尾放行）。
- **`apply_patch`**：`mutating: true, wrap_up_allowed: true, finalization_safe: false`（WRAP_UP 允许最后修复，FINALIZATION 冻结）。
- **`search_text` / `grep_text` / `fast_context_search`**：`investigation: true, wrap_up_allowed: false, finalization_safe: false`（27m 强制切断）。
- **`read_file` / `git_status` / `git_diff`**：`finalization_safe: true, wrap_up_allowed: true`（始终允许至 28m55s）。
- **`exec_command`**：结合参数智能判定 `is_wrap_up_verification_command`（仅在 WRAP_UP 放行验证命令并限时 30s，FINALIZATION 严禁）。

---

## 4. 拒绝协议结构

```json
{
  "content": [
    {
      "type": "text",
      "text": "[TURN BUDGET RESTRICTED]\nTool execution of 'search_text' was blocked because this turn is in wrap-up mode. New investigation is no longer allowed. Finish only the already identified work using finalization-safe tools, then respond to the user."
    }
  ],
  "structuredContent": {
    "ok": false,
    "error": {
      "code": "AGENT_TURN_WRAP_UP_RESTRICTED",
      "message": "Tool 'search_text' is blocked in wrap-up mode.",
      "category": "turn_budget",
      "retryable": false,
      "recovery_hint": "Do not retry this tool. This turn is in wrap-up mode. New investigation is no longer allowed. Finish only already identified work using finalization-safe tools, then respond to the user."
    }
  },
  "_meta": {
    "coding-tools/agentTurnBudget": {
      "status": "wrap_up",
      "elapsedSeconds": 1650,
      "remainingSeconds": 85,
      "shouldWrapUp": true,
      "shouldStopToolCalls": false,
      "timerOrigin": "first_observed_tool_call"
    }
  },
  "isError": true
}
```
