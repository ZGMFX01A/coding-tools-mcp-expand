# 任务清单：workspace-id-edit

## 概述

为工作区增加「展示 ID」与「修改 ID」能力：详情页展示可复制的 ID，支持自定义修改并迁移关联数据（secrets、last_workspace_id、FRP 状态），运行中服务禁止修改。

## 交付物清单（Scope-lock）

- **预计新建文件数**：1 个业务源码测试文件（`src-tauri/src/data/store.rs` 内新增测试模块，不新建独立文件）；规格文档 2 个（design.md、tasks.md）。
- **预计修改文件数**：
  1. `src-tauri/src/data/store.rs` — 新增 `rename_workspace_id` + 单测
  2. `src-tauri/src/commands/workspace.rs` — 新增 `update_workspace_id` 命令
  3. `src-tauri/src/commands/mod.rs` — 导出并注册新命令
  4. `src-tauri/src/lib.rs` — invoke_handler 注册 `update_workspace_id`
  5. `src/lib/api/workspaces.ts` — 新增 `updateWorkspaceId`
  6. `src/lib/components/WorkspaceMetaForm.svelte` — 新增 ID 输入行与 `onSaveId`
  7. `src/routes/workspace/[id]/+page.svelte` — 展示 ID + `saveWorkspaceId`（确认、跳转、错误 toast）

## 任务列表

### 阶段 1：后端数据层

- [ ] 1.1 在 `DataStore` 增加 `rename_workspace_id(&mut self, old_id, new_id) -> AppResult<WorkspaceProfile>`：定位 profile、唯一性兜底校验、迁移 `workspace_secrets`、更新 `last_workspace_id`、保存。
- [ ] 1.2 在 `data/store.rs` 测试模块新增单测：secrets 迁移、last_workspace 更新、新旧 ID 冲突报错、幂等。

### 阶段 2：后端命令层

- [ ] 2.1 在 `commands/workspace.rs` 新增 `update_workspace_id`：校验格式（空/长度/字符集）、幂等、唯一性、运行中拦截、`block_on(tunnel::drop_workspace(old_id))`、调用 `rename_workspace_id`。
- [ ] 2.2 `commands/mod.rs` 导出 `update_workspace_id`。
- [ ] 2.3 `lib.rs` 的 invoke_handler 注册 `update_workspace_id`。
- [ ] 2.4 校验函数（格式/唯一性）单测。

### 阶段 3：前端

- [ ] 3.1 `api/workspaces.ts` 新增 `updateWorkspaceId(currentId, newId)`。
- [ ] 3.2 `WorkspaceMetaForm.svelte` 新增「工作区 ID」输入行与 `onSaveId` 回调。
- [ ] 3.3 详情页 header 展示可复制的「工作区 ID」（复用 CopyFieldRow 布局）。
- [ ] 3.4 详情页实现 `saveWorkspaceId`：confirm 二次确认 → `updateWorkspaceId` → 更新 store → `goto` 新地址；运行中错误用 toast 展示。

### 阶段 4：验证

- [ ] 4.1 `cargo test`（Rust 全量测试通过）。
- [ ] 4.2 `npm run check`（svelte-check 0 errors）。
- [ ] 4.3 手工验证：复制 ID、修改 ID 跳转、运行中拦截提示。
