# A v0.3.0 → C 核心 MCP Runtime 能力审计矩阵

本报告基于官方原始业务底层 A (v0.3.0 tag 及 upstream-a/main) 真实代码，对比当前本地 Rust/Tauri 项目 C 的核心 MCP Runtime 实现。

## 能力对比矩阵

| 领域 / 能力 | A 0.3.0 真实代码行为 | C 当前实际代码行为 | 状态 | 处置决策 |
|---|---|---|---|---|
| **1. Patch 事务与回滚** | `patching.py`: 多文件临时文件预写、基线检查、重命名提交、失败完整回滚原文件、保留 backup 路径 | `patch.rs`: `commit_staged_bytes` 已实现临时文件预写、多文件失败回滚、原文件备份恢复、UTF-8 BOM 探测与保留、歧义上下文检测 | **已对齐 (A1)** | 完成并验证 UTF-8 BOM 保持、歧义检测与全套回滚单测 |
| **2. Patch 换行符与格式** | 支持 CRLF/LF 保持，支持 BOM 剥离与恢复，支持 Add/Delete/Update/Move | `patch.rs`: 支持 CRLF/LF 保持与无损保留，支持 BOM 恢复，支持 Add/Delete/Update | **已对齐 (A1)** | 完成换行符与 BOM 无损保留 |
| **3. Exec 输出缓冲 (Head+Tail)** | `processes.py`: 每流 512KB 预算，Head 占 1/8 预算单独保留，Tail 滚动截断，记录 dropped/omitted bytes | `session.rs`: 单流 1MB 预算（Head 128KB 冻结 + Tail 896KB 滚动截断），严格 `Head + Tail <= 1MB`，snapshot 输出 Head+Tail | **已对齐 (A2)** | 完成 Head/Tail 内存预算修复与单流/双流严格预算单测 |
| **4. Exec 环境变量隔离 (Env Scrubbing)** | `server.py`: `shell_env_inherit` 支持 core/all/none，默认 core；过滤 SENSITIVE_ENV_RE 及 RISKY_ENV_NAMES | `exec.rs`: `ShellEnvPolicy`（默认 core），过滤敏感变量与危险变量，保留 Windows/POSIX 必要工具链 | **已对齐 (A3)** | 完成默认环境变量过滤与安全子进程隔离 |
| **5. 进程树管理与超时** | `processes.py`: `terminate_process_group` + SIGKILL fallback | `exec.rs` + `platform/windows.rs`: `terminate_process_tree` + JobObject + SessionStore | **C 更强，不需要修改** | C 针对 Windows JobObject/ProcessTree 更深入，保留 C |
| **6. 文件安全与工作区边界** | `server.py`: canonical 路径解析，禁止逃逸，限制 default cwd 在 root 内 | `workspace.rs`: `resolve_existing`, `resolve_for_write`, symlink/UNC/device 校验 | **已对齐** | C 已有严格的边界保护与符号链接检查，保留并补充测试 |
| **7. 搜索与文件读取** | `server.py`: 基础 grep / read_file | `file.rs`: 2MB 大文件流式保护、二进制探测、BufReader、行数/字节上限 | **C 更强，不需要修改** | C 性能与保护机制更完备，保留 C |
| **8. structuredContent** | `tool_results.py`: `content` (人类/模型文本) + `structuredContent` (机器结构) 分离 | `workspace.rs`: `render_tool_text` 渲染精炼文本摘要，`structuredContent` 承载完整截断后机器结构 | **已对齐 (A4)** | 移除了 content 内冗余完整 JSON 字符串序列化，消除双重 payload 浪费 |
| **9. 分页与延续 (Pagination)** | `processes.py` / `textutils.py`: `cursor`, `offset`, `limit`, `has_more` | `file.rs` / `git.rs` / `session.rs`: 统一提供 `has_more`, `next_start_line`/`next_skip`/`next_offset`, `continuation` | **已对齐 (A4)** | 完成核心工具机器可读延续字段补充 |
| **10. 错误恢复契约** | `errors.py`: `ToolFailure` (code, message, category, retryable, details, recovery_hint) | `workspace.rs`: 统一 `WorkspaceError` 输出 `code`, `message`, `category`, `retryable`, `details`, `recovery_hint` | **已对齐 (A5)** | 完成标准错误恢复契约与分类，外部 MCP 响应原样透传 |
| **11. Permission 权限模式** | safe / trusted / dangerous，控制命令与网络门禁，tools/list 始终完整暴露 | `policy.rs` + `dispatch.rs`: safe (拦截网络命令)、trusted (本地开发)、dangerous (放宽门禁，保留绝对工作区硬边界)；tools/list 不受权限模式隐藏 | **已对齐 (A6)** | 统一 permission 拒绝错误格式与 recovery_hint，保持 External MCP allowedTools 独立与实时切换能力 |
| **12. MCP 协议多版本** | `protocol.py`: 2025-11-25 / 2025-06-18 / 2026-07-28 | `protocol.rs` + `server.rs`: 支持 2024-11-05 / 2025-06-18 / 2025-11-25 (Legacy 协商) + 2026-07-28 (Modern Discover / _meta 路由) | **已对齐 (A7)** | 完成多版本协商、server/discover 与 Modern Era 协议扩展，保持旧客户端兼容 |

