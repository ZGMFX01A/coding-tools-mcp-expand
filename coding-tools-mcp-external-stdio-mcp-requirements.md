# Coding Tools MCP 外部 stdio MCP 聚合能力需求文档

## 1. 文档信息

- 项目名称：Coding Tools MCP
- 功能名称：外部 stdio MCP 聚合
- 首个接入对象：fast-context-mcp
- 文档类型：产品需求 + 技术实现约束
- 目标版本：首个可用版本
- 适用平台：Windows 桌面端为主，兼容现有项目支持的平台

---

## 2. 背景

Coding Tools MCP 当前主要用于将网页端 Agent 与本地开发环境连接，使网页端 Agent 能够操作本地工作区，包括：

- 浏览和读取文件
- 搜索文本
- 修改代码
- 执行命令
- 操作 Git
- 管理工作区
- 通过公网 MCP 地址供 ChatGPT 等网页端 Agent 调用

现有能力已经解决了“网页端 Agent 如何操作本地代码”的问题，但仍缺少针对中大型代码库的项目级代码理解能力。

当前普通文件搜索主要依赖文件名、关键词、文本匹配和逐个读取文件。对于稍大的 Java、Vue、TypeScript 等项目，需求通常会跨越多个模块和调用层，例如：

```text
Controller
→ Service
→ ServiceImpl
→ Mapper
→ Entity
→ DTO / VO
→ 配置文件
→ 定时任务
→ MQ 消费者
```

如果缺少代码索引和语义检索能力，Agent 容易出现：

- 难以定位真实业务入口
- 无法快速理解跨文件调用关系
- 反复搜索和读取大量文件
- 上下文消耗过高
- 修改局部逻辑时遗漏关联实现
- 项目规模增大后效果明显下降

因此，需要为 Coding Tools MCP 增加外部 stdio MCP 接入能力，使其能够聚合 fast-context-mcp 等代码索引 MCP，并将外部 MCP 工具通过现有公网 MCP 接口统一暴露给网页端 Agent。

---

## 3. 建设目标

### 3.1 核心目标

为 Coding Tools MCP 增加通用的“外部 stdio MCP”管理和聚合能力。

用户可以为每个工作区配置一个或多个本地 stdio MCP Server。Coding Tools MCP 负责：

1. 启动外部 MCP 子进程
2. 完成 MCP 初始化握手
3. 获取外部 MCP 工具列表
4. 将外部工具合并到当前工作区的公网 MCP 工具列表
5. 将网页端 Agent 的工具调用转发给对应外部 MCP
6. 管理外部 MCP 的生命周期、日志、异常和状态
7. 保证不同工作区之间完全隔离

### 3.2 首个验收对象

首个实际接入对象为：

```text
fast-context-mcp@1.3.0
```

示例启动配置：

```json
{
  "name": "fast-context",
  "enabled": true,
  "command": "npx",
  "args": [
    "-y",
    "--prefer-online",
    "fast-context-mcp@1.3.0"
  ],
  "env": {
    "FC_INCLUDE_SNIPPETS": "true",
    "WINDSURF_API_KEY": ""
  },
  "allowedTools": [
    "extract_windsurf_key",
    "fast_context_search"
  ]
}
```

最终网页端 Agent 应能通过 Coding Tools MCP 调用：

```text
fast-context__fast_context_search
```

并返回当前工作区的代码语义搜索结果。

---

## 4. 非目标范围

首个版本不包含以下内容：

- 不将外部 MCP 工具暴露到 Actions/OpenAPI
- 不提供远程 HTTP MCP 聚合
- 不支持 SSE MCP 作为外部服务
- 不实现外部 MCP 市场
- 不实现自动下载所有第三方 MCP
- 不重构现有文件、命令、Git、历史会话、OAuth、FRP、Cloudflare 模块
- 不通过 `exec_command` 临时执行外部 MCP
- 不在每次工具调用时重新启动外部 MCP
- 不要求首版支持外部 MCP 之间互相调用

首个版本仅聚焦：

> 每个工作区配置并聚合本地 stdio MCP Server。

---

## 5. 用户场景

### 5.1 代码语义搜索

用户在 Coding Tools MCP 中打开一个 Java 项目，并启用 fast-context。

网页端 Agent 接收到需求：

```text
修改船员证书到期提醒逻辑，增加船员确认节点。
```

Agent 首先调用：

```text
fast-context__fast_context_search
```

定位：

- 工作流定义
- 发起入口
- 证书提醒 Service
- 船员确认状态字段
- 定时任务
- 相关 Mapper 和实体

之后再使用 Coding Tools MCP 原有工具：

```text
read_file
search_text
apply_patch
git_diff
```

完成代码验证和修改。

### 5.2 多工作区隔离

用户同时启动：

```text
工作区 A：船证项目
工作区 B：短信检索项目
```

每个工作区拥有独立的 fast-context 子进程。

在工作区 A 调用代码搜索时，不得返回工作区 B 的代码内容。

### 5.3 外部 MCP 异常

fast-context 子进程意外退出时：

- Coding Tools MCP 主服务不得崩溃
- 原有文件和 Git 工具仍可使用
- GUI 显示外部 MCP 异常
- 系统按策略自动重启
- 超过重试限制后停止重试
- 用户可手动点击“重新连接”

---

## 6. 功能需求

## 6.1 外部 MCP 配置管理

每个工作区应拥有独立的外部 MCP 配置列表。

单个配置至少包含：

```json
{
  "id": "唯一标识",
  "name": "fast-context",
  "enabled": true,
  "command": "npx",
  "args": [],
  "env": {},
  "allowedTools": [],
  "autoRestart": true,
  "initializeTimeoutSeconds": 30,
  "callTimeoutSeconds": 120
}
```

字段说明：

| 字段 | 必填 | 说明 |
|---|---:|---|
| id | 是 | 配置唯一标识 |
| name | 是 | 显示名称，同时用于工具命名空间 |
| enabled | 是 | 是否随工作区启动 |
| command | 是 | 外部 MCP 启动命令 |
| args | 否 | 命令参数列表 |
| env | 否 | 环境变量 |
| allowedTools | 否 | 工具白名单，为空时允许全部工具 |
| autoRestart | 否 | 异常退出后是否自动重启 |
| initializeTimeoutSeconds | 否 | 初始化超时时间 |
| callTimeoutSeconds | 否 | 单次工具调用超时时间 |

要求：

- 配置必须持久化
- 配置归属于具体工作区
- 不允许不同工作区共享同一配置实例
- 旧工作区配置升级后必须兼容
- 缺失新字段时使用默认值
- 删除配置时必须停止对应子进程

---

## 6.2 外部 MCP 子进程启动

当工作区 MCP 服务启动时：

1. 读取该工作区的外部 MCP 配置
2. 筛选 `enabled = true` 的配置
3. 为每个配置启动独立子进程
4. 将子进程工作目录设置为当前工作区根目录
5. 通过 stdin/stdout 进行 MCP JSON-RPC 通信
6. stderr 单独读取并写入日志
7. 初始化成功后标记为可用
8. 初始化失败时记录错误，但不影响主 MCP 服务

禁止：

- 每次调用工具时重新启动子进程
- 多工作区复用同一子进程
- 将敏感环境变量完整写入日志
- 子进程失败时终止主进程

---

## 6.3 Windows 命令兼容

Windows 平台必须兼容：

```text
npx
npm
pnpm
yarn
```

对于 `npx` 等命令，应处理：

```text
npx.cmd
npm.cmd
pnpm.cmd
yarn.cmd
```

实现要求：

- 优先按用户输入执行
- 找不到命令时，在 Windows 下尝试对应 `.cmd`
- 正确处理带空格的路径
- 不使用字符串拼接执行整条命令
- command 与 args 必须分开传递
- 关闭时应终止完整子进程树
- 不得残留 `node.exe`、`cmd.exe` 或 `npx.cmd` 子进程

---

## 6.4 MCP 初始化流程

外部 stdio MCP 启动后，应完成标准初始化流程：

```text
initialize
→ initialize response
→ notifications/initialized
→ tools/list
```

`initialize` 请求中应包含：

- 客户端名称
- 客户端版本
- 当前支持的协议版本
- 当前工作区 root 信息
- 当前工作区 URI

当前工作区应以标准 URI 形式传递，例如：

```text
file:///D:/workspace/project-a
```

要求：

- 初始化超时可配置
- 初始化失败时保存具体错误
- 未初始化完成的外部 MCP 不得向公网暴露工具
- 初始化成功后才允许执行 `tools/call`
- 需要兼容 fast-context-mcp 实际支持的协议版本

---

## 6.5 外部工具发现

初始化成功后调用：

```text
tools/list
```

系统应保存外部 MCP 返回的：

- 工具名
- 工具描述
- inputSchema
- annotations
- 其他兼容字段

如果配置了 `allowedTools`：

- 只暴露白名单中的工具
- 白名单中不存在的工具应在 GUI 中提示
- 不应导致初始化失败

如果 `allowedTools` 为空：

- 暴露外部 MCP 返回的全部工具

---

## 6.6 工具命名空间

为避免外部 MCP 工具与主 MCP 工具冲突，所有外部工具必须增加命名空间。

命名格式：

```text
{normalizedServerName}__{originalToolName}
```

示例：

```text
fast-context__fast_context_search
fast-context__extract_windsurf_key
```

命名规则：

- 保留字母、数字、短横线和下划线
- 空格替换为短横线
- 名称统一转为小写
- 连续非法字符合并
- 服务名为空或无法规范化时使用配置 ID
- 最终名称必须保持稳定

如果仍发生冲突：

```text
fast-context-2__fast_context_search
```

或使用配置 ID 作为后缀。

不得覆盖主 MCP 已有工具。

---

## 6.7 合并公网 tools/list

Coding Tools MCP 对外响应 `tools/list` 时，应合并：

```text
现有内置工具
+
已初始化成功的外部 MCP 工具
```

要求：

- 外部 MCP 未启动成功时不暴露其工具
- 外部 MCP 崩溃后应立即或尽快从工具列表中移除
- 工具描述中可注明来源，例如：

```text
[External MCP: fast-context]
```

- inputSchema 应保持原样
- 不得破坏现有工具 Schema
- 不得影响现有 Actions 工具列表

---

## 6.8 外部 tools/call 转发

当网页端 Agent 调用：

```text
fast-context__fast_context_search
```

系统应：

1. 根据命名空间定位外部 MCP 实例
2. 还原原始工具名：

```text
fast_context_search
```

3. 构造标准 `tools/call` 请求
4. 通过子进程 stdin 发送
5. 等待对应 JSON-RPC 响应
6. 将结果转换为主 MCP 可返回的格式
7. 保留内容、结构化数据和错误信息

应兼容：

- text content
- image content
- resource content
- structuredContent
- `isError = true`
- 空内容
- 多段内容

调用超时时：

- 仅终止本次等待
- 返回明确的工具调用超时错误
- 不直接杀死正常运行的子进程
- 连续超时达到阈值后可将实例标记为异常

---

## 6.9 stdio JSON-RPC 通信

需要实现稳定的 stdio JSON-RPC 客户端。

要求：

- 支持请求
- 支持响应
- 支持通知
- 支持并发请求
- 按请求 ID 正确匹配响应
- 独立读取 stdout
- 独立读取 stderr
- 正确处理逐行 JSON
- 兼容 `\r\n` 和 `\n`
- 忽略空行
- 外部进程输出非 JSON 内容时不得导致主服务崩溃
- 非法输出写入错误日志
- 请求 ID 不得冲突
- 进程退出时清理所有等待中的请求

对于外部 MCP 发来的通知：

- 未识别通知可以记录调试日志并忽略
- 不应导致连接中断

---

## 6.10 生命周期管理

外部 MCP 实例状态至少包括：

```text
disabled
stopped
starting
initializing
ready
restarting
error
stopping
```

状态流转示例：

```text
stopped
→ starting
→ initializing
→ ready
```

异常退出：

```text
ready
→ error
→ restarting
→ starting
```

工作区停止：

```text
ready
→ stopping
→ stopped
```

要求：

- 工作区 MCP 启动时启动外部 MCP
- 工作区 MCP 停止时停止外部 MCP
- 应用退出时清理所有子进程
- 编辑配置并保存后，应提示或自动重启对应实例
- 禁用配置后立即停止实例
- 删除配置后立即停止实例
- 主 MCP 重启后重新初始化外部 MCP

---

## 6.11 自动重启策略

外部 MCP 意外退出时，若启用了自动重启，应按退避策略重试。

建议策略：

```text
第 1 次：2 秒
第 2 次：5 秒
第 3 次：10 秒
第 4 次：30 秒
第 5 次及以后：60 秒
```

限制：

- 10 分钟内最多自动重启 5 次
- 超过限制后状态设为 `error`
- 用户可手动点击“重新连接”
- 用户主动停止时不得自动重启
- 工作区关闭时不得自动重启
- 修改配置导致的重启不计入异常次数

---

## 6.12 GUI 功能

在工作区配置中增加“外部 MCP”区域。

### 6.12.1 配置列表

每条配置显示：

- 名称
- 启用状态
- 当前状态
- 已发现工具数量
- 最近错误
- 操作按钮

操作按钮：

- 编辑
- 启用/禁用
- 测试连接
- 查看工具
- 重新连接
- 删除

### 6.12.2 新增/编辑表单

字段：

- 名称
- 启用
- 启动命令
- 参数列表
- 环境变量
- 工具白名单
- 自动重启
- 初始化超时
- 调用超时

参数列表应支持：

- 逐项添加
- 删除
- 调整顺序
- 粘贴多行参数

环境变量应支持：

- Key/Value 编辑
- 敏感值隐藏
- 删除
- 空值保存
- 防止重复 Key

工具白名单应支持：

- 手动填写
- 测试连接后从发现的工具中勾选
- 全选
- 清空

### 6.12.3 测试连接

点击“测试连接”后：

1. 使用当前未保存表单配置启动临时子进程
2. 完成 initialize
3. 调用 tools/list
4. 显示发现的工具
5. 测试结束后关闭临时子进程

测试结果显示：

- 是否启动成功
- 初始化耗时
- 协议版本
- 发现工具数量
- 工具列表
- stderr 摘要
- 错误原因

测试不得影响当前正在运行的正式实例。

### 6.12.4 状态显示

状态应使用清晰文本：

```text
未启用
未启动
正在启动
正在初始化
运行中
正在重启
异常
正在停止
```

错误信息至少显示：

- 错误类型
- 最近发生时间
- 简短错误内容
- 查看详细日志入口

---

## 6.13 日志

外部 MCP 应拥有独立日志分类。

至少记录：

- 配置名称
- 工作区 ID
- 工作区路径
- 启动命令
- 参数
- 初始化开始
- 初始化成功
- 协议版本
- tools/list 结果
- 暴露工具数量
- tools/call 开始和结束
- 调用耗时
- 调用错误
- 子进程退出码
- stderr
- 自动重启
- 主动停止

安全要求：

- 环境变量值默认不写日志
- 名称包含以下关键字时必须脱敏：

```text
KEY
TOKEN
SECRET
PASSWORD
PASS
AUTH
CREDENTIAL
```

日志中只显示：

```text
WINDSURF_API_KEY=***
```

不得输出完整密钥。

---

## 6.14 配置持久化与兼容

要求：

- 新配置结构应兼容已有工作区
- 旧工作区没有 `externalMcps` 字段时默认为空列表
- 应提供配置版本号或迁移逻辑
- 迁移失败不得导致工作区无法打开
- 无效配置应在 GUI 标记，而不是直接导致应用崩溃
- 保存配置时应进行基础校验
- command 为空时禁止启用
- name 重复时给出提示
- 配置 ID 必须稳定

建议结构：

```json
{
  "workspaceId": "xxx",
  "externalMcps": [
    {
      "id": "mcp-fast-context",
      "name": "fast-context",
      "enabled": true,
      "command": "npx",
      "args": [
        "-y",
        "--prefer-online",
        "fast-context-mcp@1.3.0"
      ],
      "env": {
        "FC_INCLUDE_SNIPPETS": "true",
        "WINDSURF_API_KEY": ""
      },
      "allowedTools": [
        "extract_windsurf_key",
        "fast_context_search"
      ],
      "autoRestart": true,
      "initializeTimeoutSeconds": 30,
      "callTimeoutSeconds": 120
    }
  ]
}
```

---

## 7. fast-context 首个适配要求

虽然整体实现必须是通用外部 stdio MCP，但首版必须确保 fast-context 可正常使用。

默认示例：

```json
{
  "name": "fast-context",
  "enabled": true,
  "command": "npx",
  "args": [
    "-y",
    "--prefer-online",
    "fast-context-mcp@1.3.0"
  ],
  "env": {
    "FC_INCLUDE_SNIPPETS": "true",
    "WINDSURF_API_KEY": ""
  },
  "allowedTools": [
    "extract_windsurf_key",
    "fast_context_search"
  ]
}
```

验收要求：

- 子进程 cwd 为当前工作区
- 能完成 initialize
- 能获取 `fast_context_search`
- 对外工具名为：

```text
fast-context__fast_context_search
```

- 调用能搜索当前工作区代码
- 搜索结果不得来自其他工作区
- 启用 `FC_INCLUDE_SNIPPETS=true` 时返回代码片段
- `WINDSURF_API_KEY` 为空时按 fast-context 自身逻辑处理
- 需要支持调用 `extract_windsurf_key`
- npx 首次下载时间较长时，应在 GUI 显示“正在启动”，而不是直接报超时
- 初始化超时允许用户调整

---

## 8. 安全要求

### 8.1 工作区隔离

- 每个工作区独立子进程
- 每个工作区独立配置
- 每个工作区独立请求映射
- 不允许跨工作区复用工具实例
- 不允许跨工作区读取缓存结果
- cwd 必须绑定工作区根目录

### 8.2 命令执行安全

- 不使用 shell 拼接 command + args
- 参数必须按数组传入
- 环境变量只注入当前子进程
- 不修改系统全局环境变量
- GUI 应明确提示：外部 MCP 是本地可执行程序，拥有当前用户权限

### 8.3 工具权限

- 支持 allowedTools 白名单
- 默认示例仅开放必要工具
- 外部 MCP 新增工具后，若配置了白名单，不应自动暴露
- GUI 应显示“发现但未授权”的工具

### 8.4 敏感信息

- 密钥不写入普通日志
- GUI 默认隐藏敏感环境变量
- 导出配置时提示可能包含敏感信息
- 不将环境变量通过公网 tools/list 暴露

---

## 9. 性能要求

- 主 MCP 启动不应被单个外部 MCP 无限阻塞
- 多个外部 MCP 应并行初始化
- 单个外部 MCP 初始化失败不影响其他实例
- tools/list 合并应使用内存缓存
- 不应在每次公网 `tools/list` 时重新调用所有外部 MCP
- 外部工具列表仅在以下场景刷新：
  - 初始化成功
  - 用户手动刷新
  - 外部 MCP 发出工具列表变化通知
  - 配置重启
- 并发工具调用应按 JSON-RPC ID 正确匹配
- 不得串行阻塞全部外部 MCP
- fast-context 常驻后，后续调用不得重复执行 npx 初始化

---

## 10. 错误处理

至少覆盖以下错误：

| 错误场景 | 预期行为 |
|---|---|
| command 不存在 | 标记为异常，显示命令未找到 |
| npx.cmd 找不到 | 尝试 Windows 命令兼容后再报错 |
| 初始化超时 | 停止临时初始化，标记超时 |
| stdout 输出非法 JSON | 记录原始输出，保持主服务运行 |
| tools/list 失败 | 不暴露工具，保留实例错误 |
| tools/call 超时 | 返回工具调用超时 |
| 子进程退出 | 清理等待请求并按策略重启 |
| 工作区关闭 | 正常停止，不自动重启 |
| 工具名冲突 | 自动命名空间处理 |
| allowedTools 配置不存在工具 | GUI 警告，不阻止其他工具使用 |
| 外部 MCP 返回 isError | 原样向网页端返回 |
| 环境变量格式错误 | 保存前校验 |
| 配置文件旧版本 | 自动迁移或使用默认值 |

---

## 11. 技术实现建议

## 11.1 模块划分

建议新增以下逻辑模块，具体目录按项目现有结构调整：

```text
external_mcp/
├── config
├── manager
├── instance
├── process
├── protocol
├── transport_stdio
├── tool_registry
├── namespace
├── lifecycle
└── error
```

职责建议：

### ExternalMcpManager

负责：

- 按工作区管理所有外部 MCP 实例
- 启动和停止
- 获取状态
- 获取聚合工具
- 路由 tools/call
- 应用退出时统一清理

### ExternalMcpInstance

负责单个外部 MCP：

- 子进程生命周期
- 初始化
- tools/list
- tools/call
- 状态
- 自动重启
- 错误记录

### StdioTransport

负责：

- stdin 写入
- stdout 读取
- stderr 读取
- JSON-RPC 编解码
- 请求 ID 管理
- pending request 映射
- 超时控制

### ExternalToolRegistry

负责：

- 外部工具缓存
- allowedTools 过滤
- 命名空间生成
- 冲突处理
- 工具来源映射
- tools/call 路由

---

## 11.2 进程模型

建议每个外部 MCP 配置对应一个常驻子进程：

```text
Workspace A
├── Coding Tools MCP
├── fast-context process
└── other MCP process

Workspace B
├── Coding Tools MCP
└── fast-context process
```

禁止：

```text
Workspace A ─┐
             ├── 共享 fast-context process
Workspace B ─┘
```

---

## 11.3 工具路由

建议维护映射：

```text
公网工具名
→ 工作区 ID
→ 外部 MCP 配置 ID
→ 原始工具名
```

示例：

```text
fast-context__fast_context_search
→ workspace-001
→ external-mcp-001
→ fast_context_search
```

收到 tools/call 后，通过映射定位实例并转发。

---

## 11.4 工具缓存

初始化成功后缓存工具列表。

缓存内容：

```json
{
  "publicName": "fast-context__fast_context_search",
  "originalName": "fast_context_search",
  "serverId": "external-mcp-001",
  "description": "...",
  "inputSchema": {}
}
```

外部 MCP 异常时：

- 将实例标记为非 ready
- 从聚合 tools/list 中移除对应工具
- 保留最近一次发现结果用于 GUI 展示，但不得继续暴露

---

## 12. 测试要求

## 12.1 测试 fixture

必须提供一个可控的测试 stdio MCP Server，禁止自动测试依赖：

- 真实 npx
- npm 网络
- fast-context 在线安装
- 外部 API

测试 MCP fixture 至少支持：

```text
initialize
notifications/initialized
tools/list
tools/call
```

提供工具：

```text
echo
get_workspace_root
return_error
sleep
exit_process
```

---

## 12.2 单元测试

至少覆盖：

- JSON-RPC 请求编码
- JSON-RPC 响应解析
- 通知解析
- initialize 流程
- initialized 通知
- tools/list
- tools/call
- 并发请求 ID 匹配
- 调用超时
- 初始化超时
- 非法 JSON 输出
- stderr 读取
- 子进程退出
- pending request 清理
- 自动重启
- 主动停止不重启
- 命名空间生成
- 工具名冲突
- allowedTools 为空
- allowedTools 白名单
- 旧配置兼容
- 工作区隔离
- 敏感环境变量脱敏

---

## 12.3 集成测试

至少覆盖：

1. 启动工作区和测试 MCP
2. 完成初始化
3. 公网 tools/list 出现外部工具
4. 调用外部工具成功
5. 停止工作区后子进程退出
6. 两个工作区同时运行且 root 不同
7. 一个外部 MCP 崩溃不影响主工具
8. 配置禁用后工具从列表移除
9. 配置重新启用后恢复
10. 应用退出后无残留子进程

---

## 12.4 fast-context 手工验收

使用真实配置：

```json
{
  "name": "fast-context",
  "enabled": true,
  "command": "npx",
  "args": [
    "-y",
    "--prefer-online",
    "fast-context-mcp@1.3.0"
  ],
  "env": {
    "FC_INCLUDE_SNIPPETS": "true",
    "WINDSURF_API_KEY": ""
  },
  "allowedTools": [
    "extract_windsurf_key",
    "fast_context_search"
  ]
}
```

验收步骤：

1. 打开一个 Java 工作区
2. 启动工作区 MCP
3. GUI 显示 fast-context 运行中
4. 查看已发现工具
5. 确认存在：

```text
fast_context_search
```

6. 网页端重新连接 Coding Tools MCP
7. tools/list 中出现：

```text
fast-context__fast_context_search
```

8. 使用自然语言搜索项目业务入口
9. 返回当前工作区相关文件和代码片段
10. 停止工作区
11. 检查无残留 npx、node 子进程

---

## 13. 验收标准

首版必须同时满足以下条件：

### 13.1 基础能力

- 可为工作区新增外部 stdio MCP
- 可保存配置
- 可启动和停止
- 可完成 MCP 初始化
- 可发现工具
- 可将工具合并到公网 MCP
- 可转发 tools/call
- 可返回正确结果

### 13.2 fast-context

- 支持 fast-context-mcp@1.3.0
- 对外出现：

```text
fast-context__fast_context_search
```

- 可以搜索当前工作区
- 返回结果包含代码片段
- 不搜索其他工作区

### 13.3 稳定性

- 外部 MCP 异常不导致主服务崩溃
- 自动重启受频率限制
- 工作区停止后子进程退出
- 应用退出后无残留进程
- 两个工作区完全隔离

### 13.4 兼容性

- 旧工作区配置正常打开
- 原有 MCP 工具不受影响
- Actions 不受影响
- FRP 和 Cloudflare 功能不受影响
- Windows 下 npx 可正常启动

### 13.5 GUI

- 可新增、编辑、删除
- 可测试连接
- 可查看工具
- 可查看状态和错误
- 可配置白名单
- 敏感环境变量默认隐藏

---

## 14. 推荐实施顺序

### 第一阶段：后端最小闭环

1. 分析现有 MCP tools/list 和 tools/call
2. 实现 stdio JSON-RPC 客户端
3. 实现外部子进程启动
4. 实现 initialize
5. 实现 tools/list
6. 实现 tools/call
7. 实现工具命名空间
8. 用测试 fixture 验证

### 第二阶段：接入工作区

1. 增加工作区外部 MCP 配置
2. 增加配置持久化
3. 绑定工作区 cwd
4. 工作区启动和停止联动
5. 多工作区隔离
6. 兼容旧配置

### 第三阶段：稳定性

1. 初始化超时
2. 调用超时
3. 异常退出
4. 自动重启
5. 进程树清理
6. 日志和脱敏
7. 外部 MCP 故障隔离

### 第四阶段：GUI

1. 配置列表
2. 新增和编辑
3. 环境变量编辑
4. 参数编辑
5. 工具白名单
6. 测试连接
7. 状态和错误
8. 已发现工具列表

### 第五阶段：fast-context 验收

1. 加入 README 示例
2. 使用真实 fast-context
3. 测试 Java 项目
4. 测试两个工作区
5. 测试工作区停止
6. 检查残留进程
7. 验证网页端调用

---

## 15. AI 开发约束

AI 在实施本需求时必须遵守：

- 先分析现有代码结构
- 优先定位工作区配置、MCP tools/list、tools/call、进程管理和 GUI 配置页
- 不猜测现有架构
- 不大规模重构无关模块
- 不改变现有接口行为
- 不把 fast-context 写死到核心实现
- 不通过 exec_command 模拟 MCP 调用
- 不在每次 tools/call 时启动 npx
- 后端实现应支持未来接入其他 stdio MCP
- 默认添加适量中文注释
- 复杂 public 方法使用合适的文档注释
- 核心状态流转、子进程管理、协议处理必须有注释
- 异常处理不得静默吞掉
- 敏感信息不得进入日志
- 自动测试不得依赖公网
- 完成后只输出关键修改、测试结果和已知限制

---

## 16. 最终预期效果

改造完成后，Coding Tools MCP 不再只是网页端 Agent 操作本地文件的桥接器，而是一个面向本地开发环境的 MCP 聚合网关。

最终架构：

```text
ChatGPT / 网页 Agent
        ↓ HTTPS MCP
Coding Tools MCP
        ├── 文件读取
        ├── 文本搜索
        ├── 文件修改
        ├── 命令执行
        ├── Git
        ├── 工作区管理
        └── 外部 stdio MCP
              ├── fast-context
              ├── 其他代码索引 MCP
              ├── 数据库 MCP
              └── 未来其他本地 MCP
```

网页端 Agent 的推荐工作流：

```text
fast-context__fast_context_search
→ 定位模块、类、方法和调用链
→ read_file 验证源码
→ 修改代码
→ git_diff 检查结果
```

该能力完成后，Coding Tools MCP 才能较稳定地支持中大型真实项目，而不是仅适用于小型代码库和简单文件编辑。
