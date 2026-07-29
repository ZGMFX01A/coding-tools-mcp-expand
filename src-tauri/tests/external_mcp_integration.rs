mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use coding_tools_mcp_desktop_lib::external_mcp::config::ExternalMcpConfig;
use coding_tools_mcp_desktop_lib::external_mcp::detection::detect_fast_context_env;
use coding_tools_mcp_desktop_lib::external_mcp::instance::ExternalMcpInstance;
use coding_tools_mcp_desktop_lib::external_mcp::manager::ExternalMcpManager;
use coding_tools_mcp_desktop_lib::external_mcp::transport_stdio::validate_launch_config;
use coding_tools_mcp_desktop_lib::mcp::server::{handle_request, SharedState};
use coding_tools_mcp_desktop_lib::tools::context::ToolContext;
use common::*;
use serde_json::json;

#[tokio::test]
async fn test_detection_and_validation_rules() {
    // 1. 本机环境检测函数
    let detection = detect_fast_context_env();
    println!("Detection result: detected={}, mode={:?}, message={}", detection.detected, detection.mode, detection.message);
    assert!(!detection.message.is_empty());

    // 2. 无效命令校验 (COMMAND_NOT_FOUND)
    let invalid_cmd_res = validate_launch_config("definitely_not_exist_mcp_command_12345", &[]);
    assert!(invalid_cmd_res.is_err());
    let err = invalid_cmd_res.unwrap_err();
    assert_eq!(err.error_kind(), "command_not_found");
    assert!(err.message().contains("未找到本地命令"));

    // 3. 缺少脚本文件的 node 模式校验 (ENTRY_FILE_NOT_FOUND)
    let invalid_node_res = validate_launch_config("node", &[]);
    assert!(invalid_node_res.is_err());
    let err_node = invalid_node_res.unwrap_err();
    assert_eq!(err_node.error_kind(), "entry_file_not_found");
    assert!(err_node.message().contains("未提供入口文件"));
}

#[tokio::test]
async fn test_connection_handshake_with_fast_context() {
    let fx = tiny_js_fixture();

    let npx_cmd = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let cfg = ExternalMcpConfig {
        id: "mcp-fc-test".to_string(),
        name: "fast-context".to_string(),
        enabled: true,
        command: npx_cmd.to_string(),
        args: vec!["-y".to_string(), "--prefer-offline".to_string(), "fast-context-mcp@1.3.0".to_string()],
        env: HashMap::from([("FC_INCLUDE_SNIPPETS".to_string(), "true".to_string())]),
        allowed_tools: vec!["extract_windsurf_key".to_string(), "fast_context_search".to_string()],
        auto_restart: true,
        initialize_timeout_seconds: 30,
        call_timeout_seconds: 120,
    };

    let result = ExternalMcpInstance::test_connection("ws-test-handshake", &fx.root, &cfg).await;
    println!("Test connection result: success={}, duration={}ms, protocol_version={:?}, tools_count={}",
        result.success, result.duration_ms, result.protocol_version, result.discovered_tools.len());

    assert!(result.success, "Test connection failed: {:?}", result.error_message);
    assert_eq!(result.protocol_version.as_deref(), Some("2024-11-05"));
    assert!(result.discovered_tools.iter().any(|t| t.name == "fast_context_search"));
}

#[tokio::test]
async fn test_public_mcp_tools_list_aggregation_and_tools_call_forwarding() {
    let fx = tiny_js_fixture();
    let ws_id = "ws-test-public-mcp";

    let npx_cmd = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let cfg = ExternalMcpConfig {
        id: "mcp-fc-pub".to_string(),
        name: "fast-context".to_string(),
        enabled: true,
        command: npx_cmd.to_string(),
        args: vec!["-y".to_string(), "--prefer-offline".to_string(), "fast-context-mcp@1.3.0".to_string()],
        env: HashMap::from([("FC_INCLUDE_SNIPPETS".to_string(), "true".to_string())]),
        allowed_tools: vec!["extract_windsurf_key".to_string(), "fast_context_search".to_string()],
        auto_restart: true,
        initialize_timeout_seconds: 30,
        call_timeout_seconds: 120,
    };

    let mgr = Arc::new(ExternalMcpManager::new());
    mgr.start_workspace_mcps(ws_id, &fx.root, &[cfg]).await;

    // 状态应变成 Ready
    let statuses = mgr.get_workspace_statuses(ws_id).await;
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, "ready");

    let mut context = ToolContext::new(fx.root.clone()).expect("context");
    context.workspace_id = ws_id.to_string();
    context.external_mcp = Some(mgr.clone());

    let state = SharedState::new(context);

    // 1. 测试公网 tools/list 动态合并与路由响应
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list"
    });
    let state_a = state.clone();
    let list_val = tokio::task::spawn_blocking(move || handle_request(&state_a, &list_req)).await.unwrap();
    let tools_array = list_val["result"]["tools"].as_array().expect("tools should be array");

    let tool_names: Vec<String> = tools_array
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    println!("Public tools/list aggregated tool count: {}", tool_names.len());
    assert!(tool_names.contains(&"fast-context__fast_context_search".to_string()));
    assert!(tool_names.contains(&"fast-context__extract_windsurf_key".to_string()));

    // 2. 测试公网 tools/call 命名空间解析与请求转发
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "fast-context__fast_context_search",
            "arguments": {
                "project_path": fx.root.to_string_lossy().to_string(),
                "query": "math"
            }
        }
    });

    let state_b = state.clone();
    let call_val = tokio::task::spawn_blocking(move || handle_request(&state_b, &call_req)).await.unwrap();
    assert_eq!(call_val["jsonrpc"], "2.0");
    assert_eq!(call_val["id"], 2);
    assert!(call_val.get("result").is_some());

    // 验证调用已经成功路由至外部 stdio MCP 进程并收到响应
    let content = &call_val["result"]["content"];
    assert!(content.is_array());
    println!("tools/call fast-context__fast_context_search response content: {:?}", content);

    // 清理工作区进程
    mgr.stop_workspace_mcps(ws_id).await;
}

#[tokio::test]
async fn test_dual_workspace_isolation_and_lifecycle_cleanup() {
    let fx_a = tiny_js_fixture();
    let fx_b = malicious_fixture();
    let ws_a_id = "ws-isolation-a";
    let ws_b_id = "ws-isolation-b";

    let npx_cmd = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let cfg_a = ExternalMcpConfig {
        id: "mcp-fc-a".to_string(),
        name: "fast-context".to_string(),
        enabled: true,
        command: npx_cmd.to_string(),
        args: vec!["-y".to_string(), "--prefer-offline".to_string(), "fast-context-mcp@1.3.0".to_string()],
        env: HashMap::from([("FC_INCLUDE_SNIPPETS".to_string(), "true".to_string())]),
        allowed_tools: vec!["fast_context_search".to_string()],
        auto_restart: true,
        initialize_timeout_seconds: 30,
        call_timeout_seconds: 120,
    };

    let cfg_b = ExternalMcpConfig {
        id: "mcp-fc-b".to_string(),
        name: "fast-context".to_string(),
        enabled: true,
        command: npx_cmd.to_string(),
        args: vec!["-y".to_string(), "--prefer-offline".to_string(), "fast-context-mcp@1.3.0".to_string()],
        env: HashMap::from([("FC_INCLUDE_SNIPPETS".to_string(), "true".to_string())]),
        allowed_tools: vec!["extract_windsurf_key".to_string()],
        auto_restart: true,
        initialize_timeout_seconds: 30,
        call_timeout_seconds: 120,
    };

    let mgr = Arc::new(ExternalMcpManager::new());

    // 启动工作区 A 和 B
    mgr.start_workspace_mcps(ws_a_id, &fx_a.root, &[cfg_a]).await;
    mgr.start_workspace_mcps(ws_b_id, &fx_b.root, &[cfg_b]).await;

    let st_a = mgr.get_workspace_statuses(ws_a_id).await;
    let st_b = mgr.get_workspace_statuses(ws_b_id).await;

    assert_eq!(st_a[0].state, "ready");
    assert_eq!(st_b[0].state, "ready");

    let pid_a = st_a[0].pid.expect("PID A should exist");
    let pid_b = st_b[0].pid.expect("PID B should exist");

    println!("Workspace A PID: {}, Workspace B PID: {}", pid_a, pid_b);
    assert_ne!(pid_a, pid_b, "Workspaces A and B must run on separate child processes!");

    // 停止工作区 A，确认工作区 B 不受影响
    mgr.stop_workspace_mcps(ws_a_id).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let st_a_after = mgr.get_workspace_statuses(ws_a_id).await;
    let st_b_after = mgr.get_workspace_statuses(ws_b_id).await;
    assert!(st_a_after.is_empty());
    assert_eq!(st_b_after[0].state, "ready");

    // 停止工作区 B
    mgr.stop_workspace_mcps(ws_b_id).await;
    let st_b_final = mgr.get_workspace_statuses(ws_b_id).await;
    assert!(st_b_final.is_empty());
}
