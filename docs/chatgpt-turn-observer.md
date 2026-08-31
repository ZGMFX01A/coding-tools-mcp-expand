# Coding Tools ChatGPT Turn Observer

`Coding Tools ChatGPT Turn Observer` 是一个独立、极简的 Chrome / Chromium Manifest V3 浏览器扩展，专为 **Coding Tools MCP** 深度定制。

它负责在 ChatGPT Web 端透明观测真实的对话边界（`conversationId`）、轮次生命周期（`turnId` / `inputMessageId`、`startedAt`、`completedAt`）以及实际调用的推理模型（`actual_model`），并通过安全通道上报给本地或远端的 Coding Tools 实例。

---

## 核心价值与问题解决

1. **彻底修复“旧对话 HardStop 污染新对话”的真实 Bug**：
   - **痛点**：过去仅依赖 `_meta.openai/session` 启发式推断。当某长会话耗尽 25 分钟本地执行预算进入 HardStop 后，用户切换到新对话时，旧的 session 仍可能被复用，导致新对话首个工具调用直接被误杀。
   - **解决**：Observer 精确上报 `conversation_id` 与 `turn_id`。一旦用户切换对话或开启新一轮提问，Coding Tools 后端立即为新 Turn 初始化全新预算（0 秒耗时、Normal 阶段），彻底消除历史限制继承。
2. **Turn 预算起点精确对其浏览器观测**：
   - 将预算计时起点从“服务端首次观测到工具调用的时刻 (`first_observed_tool_call`)”升级为“浏览器发起并观测到用户本轮生成开始的时刻 (`browser_observed_turn_start`)”。
3. **真实模型精准呈现**：
   - 严格基于 ChatGPT 响应侧证据识别实际模型（例如 Auto 路由实际使用的 `o3-mini` 或 `gpt-4o`），杜绝请求侧参数冒充。
4. **全链路安全与多工作区隔离**：
   - 采用工作区作用域的 `BrowserBridgeToken`，通过 Bearer Token 强鉴权。
   - 4 级置信度关联算法（`ExistingBinding`、`SingleActiveCandidate`、`Ambiguous`、`None`），多 Tab 并发时自动安全降级为会话启发式，绝不错杀。

---

## 架构设计

```
┌────────────────────────────────────────────────────────┐
│                   ChatGPT Web 浏览器                    │
│                                                        │
│  ┌───────────────────────┐   postMessage   ┌─────────┐ │
│  │ MAIN World            │ <─────────────> │ ISOLATED│ │
│  │ (page-hook.js)        │                 │ (bridge)│ │
│  │ • 拦截 fetch / WS     │                 │ • 状态机 │ │
│  │ • SSE / JSON 帧解析   │                 │ • UI 浮窗│ │
│  └───────────────────────┘                 └────┬────┘ │
└─────────────────────────────────────────────────┼──────┘
                                                  │ HTTP POST (Token)
                                                  ▼
┌────────────────────────────────────────────────────────┐
│               Coding Tools MCP 后端                     │
│                                                        │
│  ┌──────────────────────────────────────────────────┐  │
│  │ Listener: POST /internal/chatgpt-turn-event      │  │
│  │          GET  /internal/chatgpt-turn-observer/   │  │
│  │               status / control                   │  │
│  └──────────────────────────┬───────────────────────┘  │
│                             ▼                          │
│  ┌──────────────────────────────────────────────────┐  │
│  │ BrowserTurnRegistry                              │  │
│  │ • 跟踪各 Tab 的 Active / StreamIdle / Completed  │  │
│  └──────────────────────────┬───────────────────────┘  │
│                             ▼                          │
│  ┌──────────────────────────────────────────────────┐  │
│  │ TurnCorrelator                                   │  │
│  │ • 4 级置信度匹配与 Session 绑定缓存              │  │
│  └──────────────────────────┬───────────────────────┘  │
│                             ▼                          │
│  ┌──────────────────────────────────────────────────┐  │
│  │ AgentTurnBudget                                  │  │
│  │ • 基于 TurnIdentity 评估阶段与安全限制            │  │
│  └──────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────┘
```

---

## 安装与配置

### 1. 编译构建浏览器扩展

扩展源代码位于 `browser-extension/chatgpt-turn-observer/`：

```bash
cd browser-extension/chatgpt-turn-observer
npm install
npm run build
```

构建完成后，产物将生成在 `browser-extension/chatgpt-turn-observer/dist/` 目录中。

### 2. 在浏览器中加载扩展

1. 打开 Chrome / Chromium 浏览器，进入 `chrome://extensions/`。
2. 开启右上角的 **“开发者模式” (Developer mode)**。
3. 点击 **“加载已解压的扩展程序” (Load unpacked)**。
4. 选择 `browser-extension/chatgpt-turn-observer/dist/` 文件夹。

### 3. 配置扩展选项 (Options)

1. 在扩展列表或扩展图标右键菜单中点击 **“选项” (Options)**。
2. 配置项说明：
   - **上报模式 (Bridge Mode)**：
     - `Auto`（推荐）：优先尝试 Local Base URL，若不可用自动降级至 Remote Base URL。
     - `Local`：强制仅使用本地地址（如 `http://127.0.0.1:28766`）。
     - `Remote`：强制仅使用公网隧道地址（如 `https://mcp-myws.example.com`）。
   - **Local Base URL**：本地 MCP 监听基准地址（默认 `http://127.0.0.1:28766`）。
   - **Remote Base URL**：远端公网隧道基准地址（在 Desktop 工作区概览或 FRP / Cloudflare 设置中复制）。
   - **Browser Bridge Token**：在 Coding Tools Desktop 的 **“设置 -> 共享密钥 -> ChatGPT Observer 桥接密钥”** 中复制。
3. 点击 **“测试连接”**，验证是否显示 `连接成功` 并正确显示远端工作区 ID 与版本号。
4. 点击 **“保存配置”**。

---

## 浏览器端悬浮窗说明

在访问 `https://chatgpt.com/*` 时，页面右侧会自动显示极简半透明悬浮卡片：

- **Turn 计时器**：
  - 生成中显示毫秒/秒级实时计时。
   - 单个流结束后进入 1 秒静默等待窗口，若无新流则停止计时并显示该轮总耗时。
- **模型信息**：
  - 响应确认后显示 `模型: <actual_model>`（如 `o3-mini`、`gpt-4o`）。
  - 若正在路由或仅有请求参数，显示 `请求模型: <model>`、`实际模型: 检测中...`。
- **MCP 桥接状态**：
  - 实时显示 `[MCP: Local 200]`、`[MCP: Remote 200]` 或 `[MCP: 离线]`。
- **交互**：
  - 支持全视口自由拖拽，自动记住上次拖拽位置。
  - 支持点击折叠/展开微型徽标模式。
- **超时停止**：
  - 握手会读取后端返回的预算阈值；达到 warning 阈值时悬浮窗提示，达到 hard-stop 阈值时通过反向 `postMessage` 请求 MAIN world 中止实际请求，并补发 `turn_closed`。
  - 页面请求异常、SSE 读取中断或目标 WebSocket 非正常关闭也会上报 `turn_closed`，避免后端保留悬挂 Turn。

---

## 后端接口说明

### 1. 状态与鉴权测试接口
- **路径**：`GET /internal/chatgpt-turn-observer/status`
- **请求头**：`Authorization: Bearer <BrowserBridgeToken>`
- **返回**：
  ```json
  {
    "ok": true,
    "service": "chatgpt_turn_observer",
    "version": "0.2.3",
    "workspace_id": "my_workspace_id",
    "turn_budget": {
      "warning_after_seconds": 1380,
      "hard_stop_after_seconds": 1500
    }
  }
  ```

### 2. Turn 生命周期事件上报接口
- **路径**：`POST /internal/chatgpt-turn-event`
- **请求头**：`Authorization: Bearer <BrowserBridgeToken>`
- **Payload**（限制 <= 8KB）：
  ```json
  {
    "schema_version": 1,
    "observer_id": "550e8400-e29b-41d4-a716-446655440000",
    "tab_id": 123456,
    "event": "turn_started",
    "conversation_id": "6724a87b-402c-8005-b040-cfc9ecb00123",
    "turn_id": "a4b3c2d1-0000-0000-0000-000000000000",
    "request_id": null,
    "started_at": 1740888888000,
    "completed_at": null,
    "requested_model": "auto",
    "actual_model": "o3-mini"
  }
  ```
- **事件类型**：
  - `turn_started`：用户发送消息，新一轮生成开始。
  - `turn_updated`：解析到更新的会话或模型信息。
  - `stream_completed`：单次 SSE 流或 ReadableStream 结束，进入静默窗口。
  - `conversation_resolved`：新建会话完成后分配到真正的 `conversationId`。
  - `turn_closed`：页面关闭、切换会话、新 Turn 开始或生成中止。
- **处理结果**：成功应用返回 `200 {"ok":true,"applied":true}`；重复的同一 `event_id` 返回 `200` 且 `duplicate=true`；序号过期、Turn 不匹配等未应用事件返回 `409 EVENT_NOT_APPLIED`。

### 3. 超时控制校准接口
- **路径**：`GET /internal/chatgpt-turn-observer/control`
- **查询参数**：`observer_id`、`tab_instance_id`、`tab_id`、`turn_id`。
- **返回**：活动 Turn 达到后端预算 warning 阈值时返回 `command: "warn"`，达到 hard-stop 阈值时返回 `command: "stop_turn"`；未命中时返回 `command: null`。扩展在活动 Turn 期间通过 Local 或 Remote Base URL 轮询该接口。
