# A v0.3.0 → C 核心 MCP Runtime 能力审计矩阵

本报告基于官方原始业务底层 A (v0.3.0 tag 及 upstream-a/main) 真实代码，对比当前本地 Rust/Tauri 项目 C 的核心 MCP Runtime 实现。

## 能力对比矩阵

| 领域 / 能力 | A 0.3.0 真实代码行为 | C 当前实际代码行为 | 状态 | 处置决策 |
|---|---|---|---|---|
| **1. Patch 事务与回滚** | `patching.py`: 多文件临时文件预写、基线检查、重命名提交、失败完整回滚原文件、保留 backup 路径 | `patch.rs`: `commit_staged_bytes` 已实现临时文件预写、多文件失败回滚、原文件备份恢复 | **部分对齐** | 补齐 UTF-8 BOM 探测与保留、增强多位置歧义检测与错误提示 |
| **2. Patch 换行符与格式** | 支持 CRLF/LF 保持，支持 BOM 剥离与恢复，支持 Add/Delete/Update/Move | `patch.rs`: 支持 CRLF/LF 保持，缺少 BOM 处理；支持 Add/Delete/Update | **部分对齐** | 增加 BOM 保持；验证换行符无损 |
| **3. Exec 输出缓冲 (Head+Tail)** | `processes.py`: 每流 512KB 预算，Head 占 1/8 预算单独保留，Tail 滚动截断，记录 dropped/omitted bytes | `session.rs`: 1MB 预算纯 Tail 滚动截断，丢失 Head；记录总字节但无 Head 保护 | **未对齐** | `ExecSession` 引入 Head 缓冲区（1/8预算）与 Tail 滚动截断，snapshot 输出 Head+Tail |
| **4. Exec 环境变量隔离 (Env Scrubbing)** | `server.py`: `shell_env_inherit` 支持 core/all/none，默认 core；过滤 SENSITIVE_ENV_RE 及 RISKY_ENV_NAMES | `exec.rs`: 无隔离，直接继承主进程全量环境变量，泄露 API keys/secrets | **未对齐** | 实现 `ShellEnvPolicy`（默认 core），过滤敏感变量与危险变量，保留 Windows/POSIX 必要工具变量 |
| **5. 进程树管理与超时** | `processes.py`: `terminate_process_group` + SIGKILL fallback | `exec.rs` + `platform/windows.rs`: `terminate_process_tree` + JobObject + SessionStore | **C 更强，不需要修改** | C 针对 Windows JobObject/ProcessTree 更深入，保留 C |
| **6. 文件安全与工作区边界** | `server.py`: canonical 路径解析，禁止逃逸，限制 default cwd 在 root 内 | `workspace.rs`: `resolve_existing`, `resolve_for_write`, symlink/UNC/device 校验 | **已对齐** | C 已有严格的边界保护与符号链接检查，保留并补充测试 |
| **7. 搜索与文件读取** | `server.py`: 基础 grep / read_file | `file.rs`: 2MB 大文件流式保护、二进制探测、BufReader、行数/字节上限 | **C 更强，不需要修改** | C 性能与保护机制更完备，保留 C |
| **8. structuredContent** | `tool_results.py`: `content` (人类/模型文本) + `structuredContent` (机器结构) 分离 | `dispatch.rs` + `context.rs`: 已有 `wrap_mcp_tool_result`，部分工具待后续 Phase A4 统一 | **部分对齐** | 留待后续 Phase A4 处理 |
| **9. 分页与延续 (Pagination)** | `processes.py` / `textutils.py`: `cursor`, `offset`, `limit` | `session.rs` `read_output`: 支持 output_refs 分页 | **部分对齐** | 留待后续 Phase A4 处理 |
| **10. 错误恢复契约** | `errors.py`: `ToolFailure` (code, message, category, retryable, details) | `WorkspaceError`: category, retryable, suggestion, details | **已对齐 / 部分对齐** | 留待后续 Phase A5 统一 |
| **11. Permission 权限模式** | safe / trusted / dangerous，控制能力开关 | `policy.rs`: profile-based policy | **架构差异 / 部分对齐** | 留待后续 Phase A6 处理 |
| **12. MCP 协议多版本** | `protocol.py`: 2025-11-25 / 2025-06-18 / 2026-07-28 | `server.rs`: 2024-11-05 / 2025-06-18 | **部分对齐** | 留待后续 Phase A7 处理 |
