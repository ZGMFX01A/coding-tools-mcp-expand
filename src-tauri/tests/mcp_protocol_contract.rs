mod common;

use std::sync::Arc;

use common::*;
use serde_json::json;

#[test]
fn test_mcp_legacy_protocol_2025_11_25_lifecycle() {
    let fx = tiny_js_fixture();
    let harness = tempfile::tempdir().expect("harness tempdir");
    let state = Arc::new(
        coding_tools_mcp_desktop_lib::tools::ToolContext::for_test(
            fx.root.clone(),
            harness.path().to_path_buf(),
        )
        .expect("tool context"),
    );

    // 1. initialize with 2025-11-25
    let init_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "clientInfo": { "name": "test-client", "version": "1.0" },
                "capabilities": {}
            }
        }),
    );
    assert_eq!(init_res["jsonrpc"], "2.0");
    assert_eq!(init_res["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init_res["result"]["capabilities"]["tools"]["listChanged"], false);

    // 2. notifications/initialized
    let notify_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    );
    assert!(notify_res.is_null());

    // 3. tools/list
    let list_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    );
    assert!(list_res["result"]["tools"].as_array().unwrap().len() > 0);

    // 4. tools/call read_file
    let call_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "read_file",
                "arguments": {
                    "path": "src/math.js"
                }
            }
        }),
    );
    assert_eq!(call_res["result"]["isError"], false);
    assert!(call_res["result"]["structuredContent"]["content"].is_string());
}

#[test]
fn test_mcp_legacy_protocol_2025_06_18_lifecycle() {
    let fx = tiny_js_fixture();
    let harness = tempfile::tempdir().expect("harness tempdir");
    let state = Arc::new(
        coding_tools_mcp_desktop_lib::tools::ToolContext::for_test(
            fx.root.clone(),
            harness.path().to_path_buf(),
        )
        .expect("tool context"),
    );

    // initialize with 2025-06-18
    let init_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18"
            }
        }),
    );
    assert_eq!(init_res["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn test_mcp_legacy_protocol_2024_11_05_lifecycle() {
    let fx = tiny_js_fixture();
    let harness = tempfile::tempdir().expect("harness tempdir");
    let state = Arc::new(
        coding_tools_mcp_desktop_lib::tools::ToolContext::for_test(
            fx.root.clone(),
            harness.path().to_path_buf(),
        )
        .expect("tool context"),
    );

    // initialize with 2024-11-05
    let init_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": "req-legacy",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05"
            }
        }),
    );
    assert_eq!(init_res["result"]["protocolVersion"], "2024-11-05");
}

#[test]
fn test_mcp_unknown_protocol_version_falls_back_gracefully() {
    let fx = tiny_js_fixture();
    let harness = tempfile::tempdir().expect("harness tempdir");
    let state = Arc::new(
        coding_tools_mcp_desktop_lib::tools::ToolContext::for_test(
            fx.root.clone(),
            harness.path().to_path_buf(),
        )
        .expect("tool context"),
    );

    // 请求未知未来或私有协议版本，自动协商降级为支持的最新 legacy 版本
    let init_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "initialize",
            "params": {
                "protocolVersion": "2099-99-99"
            }
        }),
    );
    assert_eq!(init_res["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn test_mcp_modern_protocol_2026_07_28_discover_and_meta_requests() {
    let fx = tiny_js_fixture();
    let harness = tempfile::tempdir().expect("harness tempdir");
    let state = Arc::new(
        coding_tools_mcp_desktop_lib::tools::ToolContext::for_test(
            fx.root.clone(),
            harness.path().to_path_buf(),
        )
        .expect("tool context"),
    );

    // 1. server/discover
    let discover_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": "disc-1",
            "method": "server/discover",
            "params": {}
        }),
    );
    assert_eq!(discover_res["result"]["resultType"], "complete");
    assert_eq!(discover_res["result"]["supportedVersions"][0], "2026-07-28");
    assert!(discover_res["result"]["_meta"]["io.modelcontextprotocol/serverInfo"].is_object());

    // 2. tools/list with modern _meta
    let list_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": "list-1",
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28"
                }
            }
        }),
    );
    assert_eq!(list_res["result"]["resultType"], "complete");
    assert!(list_res["result"]["_meta"]["io.modelcontextprotocol/serverInfo"].is_object());

    // 3. tools/call with modern _meta
    let call_res = coding_tools_mcp_desktop_lib::mcp::handle_request(
        &state,
        &json!({
            "jsonrpc": "2.0",
            "id": "call-1",
            "method": "tools/call",
            "params": {
                "name": "search_text",
                "arguments": {
                    "query": "math"
                },
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28"
                }
            }
        }),
    );
    assert_eq!(call_res["result"]["resultType"], "complete");
    assert_eq!(call_res["result"]["structuredContent"]["query"], "math");
}
