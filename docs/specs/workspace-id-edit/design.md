# 设计文档：workspace-id-edit

## 概述

为工作区增加「展示 ID」与「修改 ID」能力。当前 `WorkspaceProfile.id` 由 [`WorkspaceProfile::new()`](src-tauri/src/workspace/model.rs:253) 生成（32 位无连字符 UUID），前端仅通过路由 `/workspace/{id}` 隐式使用，页面和表单都没有展示与编辑入口。本设计让用户能：

1. 在工作区详情页看到当前 ID（只读展示 + 一键复制）。
2. 把 ID 改成自定义值（受格式与唯一性约束），修改后数据关联随之迁移，访问地址更新为新 ID。

## 需求

| # | 需求 | 说明 |
|---|---|---|
| FR-1 | 展示工作区 ID | 详情页头部展示 `profile.id`，支持复制 |
| FR-2 | 修改工作区 ID | 用户可输入新 ID 并保存，保存后跳转到新地址 |
| FR-3 | ID 校验 | 非空、长度 ≤ 64、字符限 `[A-Za-z0-9_-]`、全局唯一 |
| FR-4 | 关联数据迁移 | secrets、`last_workspace_id`、FRP 进程状态随新 ID 迁移/清理 |
| NFR-1 | 运行中保护 | MCP / Actions 服务运行中不允许修改 ID，需先停止 |
| NFR-2 | 幂等 | 新 ID 与当前 ID 相同时为无操作，返回当前 profile |

## 技术方案

### 1. 后端新增命令 `update_workspace_id`

文件：[`src-tauri/src/commands/workspace.rs`](src-tauri/src/commands/workspace.rs)

```rust
#[tauri::command]
pub fn update_workspace_id(
    state: State<'_, AppState>,
    current_id: String,
    new_id: String,
) -> AppResult<WorkspaceProfile>
```

执行顺序（与现有 [`delete_workspace`](src-tauri/src/commands/workspace.rs:55) 的停止-迁移风格一致）：

1. **校验 `new_id`**：
   - `trim()` 后非空，长度 `1..=64`；
   - 只允许 `[A-Za-z0-9_-]`——与 FRP 目录名清理 [`sanitize_workspace_id`](src-tauri/src/tunnel/frp/client.rs:52) 保持一致，避免两个不同 ID 清理后落到同一目录；
   - `new_id == current_id` → 直接返回当前 profile（幂等）。
2. **唯一性**：`new_id` 不得与任何其他 profile 的 `id` 冲突。
3. **运行中保护**：通过 `state.with_runtime` 检查 `is_running(current_id, Mcp/Actions)`，任一运行中则返回错误，要求先停止服务。
4. **停止旧 ID 关联的隧道**：`tauri::async_runtime::block_on(tunnel::drop_workspace(&current_id))`——复用现有 [`drop_workspace`](src-tauri/src/tunnel/access.rs:52)，停止 frpc 进程并清理 PID 文件。
5. **数据迁移**（`state.with_workspaces` 内）：
   - `store.rename_workspace_id(&current_id, &new_id)`：改 profile.id、迁移 `workspace_secrets`、更新 `last_workspace_id`（若指向旧 ID），最后 `save()`。
6. 返回迁移后的 `WorkspaceProfile`。

### 2. DataStore 新增方法

文件：[`src-tauri/src/data/store.rs`](src-tauri/src/data/store.rs)

```rust
pub fn rename_workspace_id(&mut self, old_id: &str, new_id: &str) -> AppResult<WorkspaceProfile>
```

- 按 `old_id` 定位 profile（找不到报错）；
- 校验 `new_id` 唯一（命令层已校验，这里兜底）；
- `workspace_secrets` 中把 `old_id` 整份迁移到 `new_id`（`HashMap::remove` + `insert`）；
- 更新 `profiles[index].id = new_id`；
- 若 `data.last_workspace_id == old_id`，更新为 `new_id`；
- `self.save()`，返回新 profile。

### 3. 前端

**API**：[`src/lib/api/workspaces.ts`](src/lib/api/workspaces.ts) 增加

```ts
export async function updateWorkspaceId(currentId: string, newId: string): Promise<WorkspaceProfile> {
  return invoke("update_workspace_id", { currentId, newId });
}
```

**展示**：详情页头部 [`src/routes/workspace/[id]/+page.svelte`](src/routes/workspace/[id]/+page.svelte:516) 的 header 中，标题下方加入只读展示（复用 [`CopyFieldRow`](src/lib/components/CopyFieldRow.svelte) 的 label/值/复制布局），文案「工作区 ID」。

**编辑**：扩展 [`WorkspaceMetaForm.svelte`](src/lib/components/WorkspaceMetaForm.svelte)：
- 新增「工作区 ID」输入行与 `onSaveId(id)` 回调；
- 页面实现 `saveWorkspaceId`：`confirm` 二次确认（说明会改变访问地址且需先停止服务）→ 调用 `updateWorkspaceId` → 更新 `workspaces` store → `goto('/workspace/{newId}')`；
- 后端返回「服务运行中」错误时，用 toast 展示错误原因。

## 数据迁移清单

| 关联数据 | 位置 | 处理 |
|---|---|---|
| `workspace_secrets` | `data/profiles.json` | 整份 key 迁移到新 ID |
| `last_workspace_id` | `data/profiles.json` | 若等于旧 ID，改为新 ID |
| `profiles[].id` | `data/profiles.json` | 更新为主键 |
| FRP 进程 / PID 文件 | `app_config_dir/frpc/{旧id}/` | `drop_workspace` 停止进程并清理 PID 文件 |
| RuntimeSupervisor.entries | 内存 | 运行中已拒绝，无残留 |
| ExternalMcpManager.instances/registries | 内存 | 运行中已拒绝；MCP 停止时已随 `drop_workspace` 清理 |

## 设计决策

### 决策 1：字符集限定 `[A-Za-z0-9_-]`

**问题**：ID 会被用于 FRP 管理目录名（`managed_frpc_dir` 内 `sanitize_workspace_id`），若不限制字符，不同 ID 可能清理后落到同一目录或引入路径问题。

**决策**：新 ID 只允许 ASCII 字母、数字、`-`、`_`，长度 ≤ 64。

### 决策 2：运行中禁止修改

**问题**：运行时状态（runtime entries、external MCP 实例、frpc 进程）都以 ID 为 key，运行中迁移会导致状态丢失或错位。

**决策**：命令层检查 MCP/Actions 是否运行，运行中直接返回错误；前端在确认框中提示先停止服务。相比自动停止，避免打断用户正在运行的隧道与 Agent 会话，行为更可预期。

### 决策 3：复用 `drop_workspace` 处理 FRP 清理

**问题**：修改 ID 后旧 ID 的 frpc 进程与 PID 文件需停止/清理。

**决策**：直接复用 `tunnel::drop_workspace(old_id)`（现有能力）。旧目录残留的 `frpc.toml` 不影响功能（新 ID 使用新目录），不额外引入删除逻辑，控制改动范围。

## 测试策略

- **Rust 单测**（`data/store.rs`）：`rename_workspace_id` 迁移 secrets、更新 `last_workspace_id`、新旧 ID 冲突报错、幂等（同 ID 无操作）。
- **Rust 单测**（`commands/workspace.rs` 或校验函数）：ID 格式校验（空、超长、非法字符、重复）。
- **前端**：`npm run check`（svelte-check）0 errors；手工验证复制、修改跳转、运行中拦截。

## 风险评估

| 风险 | 影响 | 缓解措施 |
|---|---|---|
| 运行中修改导致状态错位 | 高 | 命令层硬拦截，前端二次确认提示先停止 |
| 字符集放太宽导致目录冲突 | 中 | 限定 `[A-Za-z0-9_-]` 并全局唯一校验 |
| secrets 迁移遗漏 | 高 | `rename_workspace_id` 内原子完成 secrets + id + last_workspace，单测覆盖 |
| 前端 URL 未跳转 | 中 | 保存成功后 `goto` 新地址，workspaces store 同步更新 |
