use std::path::PathBuf;

use tauri::State;

use crate::app_state::{bootstrap_workspace, teardown_workspace, AppState};
use crate::error::{AppError, AppResult};
use crate::platform::open_path_in_file_manager;
use crate::runtime::ServiceKind;
use crate::tunnel::drop_workspace as drop_tunnel_workspace;
use crate::workspace::resources::{
    assign_free_workspace_ports, validate_workspace_resources_update,
};
use crate::workspace::WorkspaceProfile;

#[tauri::command]
pub fn list_workspaces(state: State<'_, AppState>) -> AppResult<Vec<WorkspaceProfile>> {
    state.with_workspaces(|store| Ok(store.list().to_vec()))
}

#[tauri::command]
pub fn create_workspace(
    state: State<'_, AppState>,
    path: String,
    name: Option<String>,
) -> AppResult<WorkspaceProfile> {
    state.with_workspaces(|store| {
        let mut profile = WorkspaceProfile::new(path, name);
        // Create should not fail just because default ports are already claimed.
        // Pick free ports now; start/update still enforce conflict checks.
        assign_free_workspace_ports(store.list(), &mut profile)?;
        bootstrap_workspace(store, &profile.id)?;
        store.add(profile.clone())?;
        Ok(profile)
    })
}

#[tauri::command]
pub fn update_workspace(state: State<'_, AppState>, profile: WorkspaceProfile) -> AppResult<()> {
    state.with_workspaces(|store| {
        let current = store
            .get(&profile.id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {}", profile.id)))?;
        validate_workspace_resources_update(store.list(), &current, &profile)?;
        store.update(profile)
    })
}

#[tauri::command]
pub fn open_workspace_directory(path: String) -> AppResult<()> {
    let path = PathBuf::from(path.trim());
    open_path_in_file_manager(&path)
}

#[tauri::command]
pub fn delete_workspace(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let profile = state.with_workspaces(|store| {
        store
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })?;
    tauri::async_runtime::block_on(drop_tunnel_workspace(&id))?;
    state.with_runtime(|runtime| {
        runtime.drop_workspace(&profile);
        Ok(())
    })?;
    state.with_workspaces(|store| {
        if store.remove(&id)?.is_some() {
            teardown_workspace(store, &id)?;
        }
        Ok(())
    })
}

/// 校验工作区 ID 格式：非空、长度 ≤ 64、仅允许 [A-Za-z0-9_-]。
/// 该约束与 FRP 目录名清理规则一致，避免不同 ID 清理后落到同一目录。
fn validate_workspace_id(id: &str) -> AppResult<()> {
    if id.is_empty() {
        return Err(AppError::Message("工作区 ID 不能为空。".into()));
    }
    if id.len() > 64 {
        return Err(AppError::Message(
            "工作区 ID 长度不能超过 64 个字符。".into(),
        ));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(AppError::Message(
            "工作区 ID 只能包含字母、数字、连字符（-）和下划线（_）。".into(),
        ));
    }
    Ok(())
}

/// 修改工作区 ID：幂等判断 → 格式校验 → 唯一性检查 → 运行中拦截 →
/// 停止旧 ID 隧道并清理 PID → 迁移 secrets / last_workspace_id / profile.id。
#[tauri::command]
pub fn update_workspace_id(
    state: State<'_, AppState>,
    current_id: String,
    new_id: String,
) -> AppResult<WorkspaceProfile> {
    let new_id = new_id.trim().to_string();

    // 幂等：新旧 ID 相同视为无操作，直接返回当前 profile
    if new_id == current_id {
        return state.with_workspaces(|store| {
            store
                .get(&current_id)
                .cloned()
                .ok_or_else(|| AppError::Message(format!("workspace not found: {current_id}")))
        });
    }

    validate_workspace_id(&new_id)?;

    // 唯一性检查：新 ID 不能与任何已有工作区冲突（store 层有兜底校验）
    let exists = state.with_workspaces(|store| {
        Ok(store.list().iter().any(|item| item.id == new_id))
    })?;
    if exists {
        return Err(AppError::Message(format!("工作区 ID 已存在：{new_id}")));
    }

    // 运行中拦截：MCP / Actions 任一服务运行中都不允许修改 ID，避免内存状态错位
    let running = state.with_runtime(|runtime| {
        Ok(runtime.is_running(&current_id, ServiceKind::Mcp)
            || runtime.is_running(&current_id, ServiceKind::Actions))
    })?;
    if running {
        return Err(AppError::Message(
            "工作区服务正在运行，请先停止 MCP 和 Actions 服务后再修改 ID。".into(),
        ));
    }

    // 停止旧 ID 关联的隧道进程并清理其 PID 文件
    tauri::async_runtime::block_on(drop_tunnel_workspace(&current_id))?;

    state.with_workspaces(|store| store.rename_workspace_id(&current_id, &new_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_workspace_id_accepts_legal_ids() {
        assert!(validate_workspace_id("my-workspace_01").is_ok());
        assert!(validate_workspace_id("a").is_ok());
        assert!(validate_workspace_id(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_workspace_id_rejects_illegal_ids() {
        // 空
        assert!(validate_workspace_id("").is_err());
        // 超长
        assert!(validate_workspace_id(&"x".repeat(65)).is_err());
        // 非法字符：空格、斜杠、中文
        assert!(validate_workspace_id("my workspace").is_err());
        assert!(validate_workspace_id("../etc").is_err());
        assert!(validate_workspace_id("工作区").is_err());
    }
}
