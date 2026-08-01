---
doc_type: issue-fix
severity: medium
tags: [windows, process, exec, ui, fix]
status: fixed
issue_date: 2026-08-01
fix_commit: 73658ba
release: v0.1.28
---

# 修复记录：调用 cmd / powershell 时弹出置顶 GUI 窗口

## 现象（Symptom）

使用本工具调用 `cmd` / `powershell` 等命令行工具时，会突然弹出一个独立的控制台（conhost）GUI 窗口，且置顶显示在最上层，干扰操作。

## 根因（Root Cause）

Windows 平台下，[`command_for_program`](../../../src-tauri/src/tools/exec.rs) 中启动 `cmd.exe` / `pwsh` / `powershell.exe` 子进程时，**没有设置 `CREATE_NO_WINDOW`（`0x08000000`）创建标志**。

Windows 对控制台程序（conhost 类）默认会为新子进程分配新的控制台窗口。由于父进程（Tauri 桌面应用）本身无控制台，spawn 出的 `cmd` / `powershell` 子进程就会获得一个全新的可见 conhost 窗口，从而出现"弹窗置顶"现象。

## 修复（Fix）

在 [`run_command`](../../../src-tauri/src/tools/exec.rs) 中，spawn 前对 Windows 子进程统一追加 `CREATE_NO_WINDOW` 创建标志：

```rust
#[cfg(windows)]
{
    // CREATE_NO_WINDOW (0x08000000)：子进程若为控制台程序则不创建新控制台窗口，
    // 避免调用 cmd / powershell 等命令时弹出置顶的 conhost 窗口
    command
        .as_std_mut()
        .creation_flags(0x08000000);
}
```

效果：
- 控制台子进程完全不创建可见窗口（隐藏后自然不存在"置顶"问题）。
- `stdin/stdout/stderr` 仍通过管道捕获，不影响输出采集与交互逻辑。
- 该标志对非控制台程序（如 GUI 应用）无副作用，统一作用于所有 Windows 子进程。

## 验证（Verification）

- 修复代码已提交（`73658ba`）并随 `v0.1.28` 发布。
- 本机无 Rust 编译环境，未做本地构建验证；建议在 Windows 上执行一次 `exec_command` 调用 `cmd` / `powershell`，确认不再弹出置顶窗口。

## 范围（Scope）

仅改动 [`src-tauri/src/tools/exec.rs`](../../../src-tauri/src/tools/exec.rs) 一处（`run_command` 内新增 `#[cfg(windows)]` 块），不影响其他平台与非命令执行路径。

## 备注

- 本次未接入 CodeStable 体系（用户选择跳过 onboard），因此本记录存放在 `docs/issues/` 而非 `.codestable/issues/`。
- 工作区遗留的 history 会话 `workspace_root` 兼容改动与版本号同步（0.1.28）已随同一提交发布。
