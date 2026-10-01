# 上游 v0.5 功能同步

同步基线是本项目的 `271142b`（2026-08-15，上游 v0.3.0 核心移植），目标是 [xyTom/coding-tools-mcp 的 9d2c179](https://github.com/xyTom/coding-tools-mcp/commit/9d2c179cb307a7121bf7a83420d9d4f7815afb45)。目标包含 v0.5.0，以及发布后合入的本地工具事件日志。继续使用 Rust/Tauri，不依赖上游 Python 服务启动桌面客户端。

## 已同步能力

| 能力 | 当前入口和行为 |
| --- | --- |
| 文件版本 | `read_file` 返回完整文件字节的 SHA-256 `revision`；文本提示也带版本。分页不会改变版本计算范围。 |
| 结构化编辑 | `apply_changes` 支持 create、write、edit、delete、move、copy；已存在的源文件必须提供版本。最多 100 个动作，每文件最多 200 个行编辑。 |
| 行编辑 | replace、delete 用 start_line/end_line，insert_before/insert_after 用 line；全部按原文件行号定位，重叠和重复路径会拒绝。保留 BOM、CRLF、CR 和末尾换行形态。 |
| 补丁定位 | `apply_patch` 支持向前查找的 `@@ 文本锚点`、EOF 和 Move to；按 exact、trailing_ws、indent 分级，拒绝歧义，保留原有上下文。 |
| 修改证据 | 返回每个文件的新版本、总行数、changed_ranges 和匹配等级；失败包含 hunk_index、候选位置及附近编号行。 |
| 安全重试 | 两个写入工具接受 idempotency_key，同参数回放成功结果，不同参数复用 key 拒绝；最多保留 64 个成功写入请求，dry_run 和失败不缓存。 |
| 重复失败保护 | 同工具同参数的确定性失败在第三次调用时返回 REPEATED_CALL_BLOCKED。换 key 无法绕过；实际文件修改或可能写入的命令首次结束会清除过期失败判断。 |
| 命令结果 | operation_outcome 区分 running、exited_0、exited_nonzero、timeout、signal、spawn_error；非零退出不会伪装成操作成功，重复轮询只统计一次终态。 |
| 命令输出保留 | 默认运行上限 300 秒，首次等待 10 秒；最多 16 个活跃命令。完成输出保留 300 秒，最多保留最近 32 个已完成命令。 |
| Git 差异 | `git_diff` 默认包含未跟踪文件；include_untracked=false 可关闭。未跟踪文件最多 100 个，跳过超大文件，不读取逃逸链接目标。 |
| 工具协议 | 每个工具提供 outputSchema；request_permissions 仅 dangerous 模式出现在实时目录，原有直接调用保留兼容。 |
| 受限命令写入 | 提供默认关闭的 structured-only 和 write-path；需要 Linux Landlock ABI≥3。Windows/macOS 会报告 enforced=false。 |
| 本地事件日志 | 默认关闭；开启后记录工具开始/结束、耗时、状态等元数据，不保存参数、文件内容或命令输出。单文件 1 MiB，加 3 个轮转备份；独占写入，跨重启保留。 |
| 评测和 CLI | 上游 benchmarks/agent_eval 已适配 Rust headless 服务；支持 --version 和 --serve。 |

MCP 和 Actions 继续调用同一个工具内核，同工作区、权限模式和工具配置共享命令会话及重试账本。不同配置仍共享文件写入锁。

## 编辑示例

先调用 `read_file({"path":"src/example.rs"})`，保存返回的 revision，再提交：

```json
{
  "changes": [{
    "action": "edit",
    "path": "src/example.rs",
    "revision": "把 read_file 返回的 64 位 SHA-256 放在这里",
    "edits": [{"op":"replace","start_line":12,"end_line":14,"content":"新的内容"}]
  }],
  "idempotency_key": "example-edit-1"
}
```

revision 过期时重新读取，不能直接重试旧版本。未存在文件可 create 或 write；move/copy 的目标必须不存在。删除或移动关键项目文件仍须 confirm=true，仓库保护目录仍不可写。dry_run 只预检，无变化的请求不重写文件。

同一补丁的多次 File 块依次作用于暂存内容，最终每个路径只返回一份版本证据；重复块的变更范围按原始文件合并。事务提交前把原文件保留为同目录备份。修改失败时通过重命名恢复原文件及权限；若恢复本身也失败，`PATCH_ROLLBACK_FAILED` 的 `recovery_backups` 会给出保留下来的原件位置，须先恢复并检查工作区再重试。

## 运行选项

```text
coding-tools-mcp-desktop --version
coding-tools-mcp-desktop --serve --workspace <项目目录> --port 28766
coding-tools-mcp-desktop --workspace-mutation structured-only --write-path target --write-path dist
```

也可在启动客户端前设置 `CODING_TOOLS_MCP_WORKSPACE_MUTATION=structured-only`，并用平台路径分隔符配置 `CODING_TOOLS_MCP_WRITE_PATHS`。write-path 必须是工作区内的目录，只适用于 structured-only。dangerous 模式不启用 Landlock 限制。通过 server_info/check_exec_environment 查看实际策略及可执行平台边界。

设置 `CODING_TOOLS_MCP_EVENT_LOG_DIR` 为一个绝对路径私有目录可启用事件日志。各进程必须使用不同目录；日志故障只关闭记录，不改变工具执行结果。Unix 会校验所有者和目录/文件权限；Windows 应由操作者确保目录 ACL 为私有。现有 Harness 历史记录功能继续使用自己的存储。

评测只需要 Python 标准库。先构建 Rust 服务，再运行 `python -m benchmarks.agent_eval.run_eval --validate-only` 验证三个离线练习任务。详见 [评测说明](../benchmarks/agent_eval/README.md)。同步时没有运行外部付费模型或发布新版本。

## 对话超时机制移除

删除了服务端对话预算状态机、时间告警、工具限制、强制收尾、运行时预算参数及外部 MCP 超时削减。扩展删除自动停止计时器、远程控制轮询、STOP_TURN 消息、替换请求 signal 和主动关闭 WebSocket 的路径。Observer 只观察、显示和上报自然生命周期事件。

普通命令上限、网络请求超时和缓存过期仍保留，它们不会主动关闭 ChatGPT 对话。重新构建/重启桌面服务后生效；Chrome/Edge 需要在扩展管理页重新加载本项目的 dist 目录，并刷新 ChatGPT 页面。浏览器平台自己的会话限制由平台决定。

## 验证范围

Windows 上通过 Rust 回归、39 项扩展测试、前端构建和类型检查。真实本地 MCP 接口验证了工具目录、版本编辑、幂等回放、命令非零退出与输出保留，以及事件日志隐私和跨重启追加。6 项原有配置/密钥测试需要写入当前用户的应用数据目录，因工作区权限限制未执行。当前环境没有可运行的 Linux 工具链，Landlock 分支尚未在 Linux 编译或执行；Windows 正确报告未启用隔离。没有运行付费模型评测或发布安装包。
