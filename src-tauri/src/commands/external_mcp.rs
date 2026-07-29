use std::path::PathBuf;
use tauri::State;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::external_mcp::config::ExternalMcpConfig;
use crate::external_mcp::detection::FastContextDetectionResult;
use crate::external_mcp::instance::{ExternalMcpStatusDto, TestConnectionResultDto};
use crate::external_mcp::protocol::McpTool;

#[tauri::command]
pub fn detect_fast_context_env() -> AppResult<FastContextDetectionResult> {
    Ok(crate::external_mcp::detect_fast_context_env())
}

#[tauri::command]
pub fn list_external_mcps(
    state: State<'_, AppState>,
    workspace_id: String,
) -> AppResult<Vec<ExternalMcpStatusDto>> {
    let result = state.with_runtime(|runtime| {
        let mgr = runtime.external_mcp.clone();
        let list = tauri::async_runtime::block_on(async move {
            mgr.get_workspace_statuses(&workspace_id).await
        });
        Ok(list)
    })?;
    Ok(result)
}

#[tauri::command]
pub async fn save_external_mcp(
    state: State<'_, AppState>,
    workspace_id: String,
    config: ExternalMcpConfig,
) -> AppResult<ExternalMcpStatusDto> {
    config.validate().map_err(AppError::Message)?;

    let profile = state.with_workspaces(|store| {
        let mut prof = store
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("工作区未找到: {workspace_id}")))?;

        // 更新或追加配置
        if let Some(pos) = prof.external_mcps.iter().position(|c| c.id == config.id) {
            prof.external_mcps[pos] = config.clone();
        } else {
            prof.external_mcps.push(config.clone());
        }

        store.update(prof.clone())?;
        Ok(prof)
    })?;

    // 如果该工作区 MCP 服务处于运行状态，重启该外部 MCP 实例
    let (is_running, mgr) = state.with_runtime(|runtime| {
        let is_running = runtime.is_running(&profile.id, crate::runtime::ServiceKind::Mcp);
        let mgr = runtime.external_mcp.clone();
        Ok((is_running, mgr))
    })?;

    let ws_path = PathBuf::from(&profile.path);
    if is_running {
        Ok(mgr.reconnect_instance(&profile.id, &ws_path, &config).await.unwrap_or_else(|_| ExternalMcpStatusDto {
            config_id: config.id.clone(),
            name: config.name.clone(),
            enabled: config.enabled,
            state: "error".to_string(),
            pid: None,
            discovered_tools_count: 0,
            error_message: Some("重新连接失败".to_string()),
        }))
    } else {
        Ok(ExternalMcpStatusDto {
            config_id: config.id,
            name: config.name,
            enabled: config.enabled,
            state: if config.enabled { "stopped".to_string() } else { "disabled".to_string() },
            pid: None,
            discovered_tools_count: 0,
            error_message: None,
        })
    }
}

#[tauri::command]
pub async fn delete_external_mcp(
    state: State<'_, AppState>,
    workspace_id: String,
    config_id: String,
) -> AppResult<()> {
    let profile = state.with_workspaces(|store| {
        let mut prof = store
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("工作区未找到: {workspace_id}")))?;

        prof.external_mcps.retain(|c| c.id != config_id);
        store.update(prof.clone())?;
        Ok(prof)
    })?;

    let mgr = state.with_runtime(|runtime| Ok(runtime.external_mcp.clone()))?;
    let dummy_cfg = ExternalMcpConfig {
        id: config_id,
        name: String::new(),
        enabled: false,
        command: String::new(),
        args: vec![],
        env: std::collections::HashMap::new(),
        allowed_tools: vec![],
        auto_restart: false,
        initialize_timeout_seconds: 30,
        call_timeout_seconds: 120,
    };
    let ws_path = PathBuf::from(&profile.path);
    let _ = mgr.reconnect_instance(&profile.id, &ws_path, &dummy_cfg).await;

    Ok(())
}

#[tauri::command]
pub fn test_external_mcp_connection(
    state: State<'_, AppState>,
    workspace_id: String,
    config: ExternalMcpConfig,
) -> AppResult<TestConnectionResultDto> {
    config.validate().map_err(AppError::Message)?;

    let profile = state.with_workspaces(|store| {
        store
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("工作区未找到: {workspace_id}")))
    })?;

    let res = state.with_runtime(|runtime| {
        let mgr = runtime.external_mcp.clone();
        let ws_path = PathBuf::from(&profile.path);
        let cfg = config.clone();
        let dto = tauri::async_runtime::block_on(async move {
            mgr.test_connection(&workspace_id, &ws_path, &cfg).await
        });
        Ok(dto)
    })?;

    Ok(res)
}

#[tauri::command]
pub fn reconnect_external_mcp(
    state: State<'_, AppState>,
    workspace_id: String,
    config_id: String,
) -> AppResult<ExternalMcpStatusDto> {
    let profile = state.with_workspaces(|store| {
        store
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("工作区未找到: {workspace_id}")))
    })?;

    let config = profile
        .external_mcps
        .iter()
        .find(|c| c.id == config_id)
        .cloned()
        .ok_or_else(|| AppError::Message(format!("配置未找到: {config_id}")))?;

    let dto = state.with_runtime(|runtime| {
        let mgr = runtime.external_mcp.clone();
        let ws_path = PathBuf::from(&profile.path);
        let cfg = config.clone();
        let res = tauri::async_runtime::block_on(async move {
            mgr.reconnect_instance(&workspace_id, &ws_path, &cfg).await
        }).map_err(AppError::Message)?;
        Ok(res)
    })?;

    Ok(dto)
}

#[tauri::command]
pub fn get_external_mcp_discovered_tools(
    state: State<'_, AppState>,
    workspace_id: String,
    config_id: String,
) -> AppResult<Vec<McpTool>> {
    let tools = state.with_runtime(|runtime| {
        let mgr = runtime.external_mcp.clone();
        let list = tauri::async_runtime::block_on(async move {
            mgr.get_discovered_tools(&workspace_id, &config_id).await
        });
        Ok(list)
    })?;
    Ok(tools)
}
