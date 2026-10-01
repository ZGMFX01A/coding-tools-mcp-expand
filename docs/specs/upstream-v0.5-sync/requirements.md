# 取消对话超时并同步上游能力

上游快照：xyTom/coding-tools-mcp `9d2c179cb307a7121bf7a83420d9d4f7815afb45`，包含 v0.5.0 和发布后本地工具事件日志。上一轮核心同步为 `271142b`（v0.3.0）。

## 验收

1. 删除所有按对话累计时间告警、限制工具、缩短命令超时、阻止外部 MCP 或中止网页生成的机制；Observer 只观察和上报生命周期，不控制网页请求。
2. 保留普通命令的独立超时、网络请求超时和有限内存/日志容量。命令默认 lifetime=300s、yield=10s，与对话时长无关。
3. 移植 apply_changes 六种动作、按读取 revision 校验、行编辑、原子预检/恢复、重复路径/重叠编辑拒绝及无变化不写文件。
4. read_file 同一份字节返回 SHA-256 revision，文本提示带版本；apply_patch 提供锚点、EOF、分级匹配、已应用识别、失败修复信息、变更行和版本证据。
5. apply_patch/apply_changes 支持有界幂等重放，并发相同 key 只执行一次；不同参数复用 key 拒绝；dry_run 不占用 key。
6. 相同参数的确定性失败第三次熔断；实际修改/可写命令第一次终态使旧判定失效；普通工具成功不掩盖其他失败。
7. git_diff 默认包含未跟踪文件；工具提供独立 outputSchema；request_permissions 仅 dangerous 模式公开，直接调用保持兼容。
8. 提供默认关闭的 structured-only 与 write-path 配置，Linux Landlock ABI>=3 时可执行；其他平台如实报告 enforced=false。
9. 本地事件日志默认关闭、只存元数据、有容量上限、跨重启保留、独占写入、存储故障不影响工具结果；MCP/Actions/外部工具具有一致覆盖。
10. 操作统计区分请求成功与命令退出状态，重复轮询不重复计数；公开输出保留当前工具兼容字段。
11. 上游评测工具适配 Rust 服务；CLI 支持显示当前版本；更新使用文档与同步矩阵。

## 边界与证据

继续使用 Rust/Tauri，不把上游 Python 服务作为运行时依赖。不修改已有 FRP 工作区改动，不提交或发版。保留 get_default_cwd/kill_session 等当前兼容工具，新功能接入共享 tools 内核。验证覆盖浏览器观察器、MCP、Actions、文件边界、并发、换行、回滚和真实命令结果。

GitNexus 已刷新并通过 CLI 调用 impact；ToolContext 构造、公共注册表等传播到 MCP/Actions 的路径为 HIGH，须独立 change review。mcp-probe-kit 已读，但当前会话未提供对应 MCP 工具，按仓库现状和源码落实规格及验证。
