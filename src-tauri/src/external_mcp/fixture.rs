use serde_json::Value;

use crate::external_mcp::protocol::McpTool;

/// 生成测试 Mock 工具列表
pub fn mock_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "echo".to_string(),
            description: Some("Echo test tool".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                }
            }),
            annotations: None,
        },
        McpTool {
            name: "get_workspace_root".to_string(),
            description: Some("Get workspace root path".to_string()),
            input_schema: serde_json::json!({ "type": "object" }),
            annotations: None,
        },
        McpTool {
            name: "return_error".to_string(),
            description: Some("Return an error for testing".to_string()),
            input_schema: serde_json::json!({ "type": "object" }),
            annotations: None,
        },
        McpTool {
            name: "sleep".to_string(),
            description: Some("Sleep for test duration".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "seconds": { "type": "number" }
                }
            }),
            annotations: None,
        },
        McpTool {
            name: "exit_process".to_string(),
            description: Some("Exit process for crash testing".to_string()),
            input_schema: serde_json::json!({ "type": "object" }),
            annotations: None,
        },
    ]
}

/// 处理测试工具的调用逻辑
pub fn handle_fixture_call(tool_name: &str, args: &Value, workspace_root: &str) -> Result<Value, String> {
    match tool_name {
        "echo" => {
            let msg = args.get("message").and_then(Value::as_str).unwrap_or("");
            Ok(serde_json::json!({
                "content": [
                    { "type": "text", "text": format!("echo: {msg}") }
                ]
            }))
        }
        "get_workspace_root" => Ok(serde_json::json!({
            "content": [
                { "type": "text", "text": workspace_root }
            ]
        })),
        "return_error" => Ok(serde_json::json!({
            "isError": true,
            "content": [
                { "type": "text", "text": "Simulated tool error" }
            ]
        })),
        _ => Err(format!("Unknown fixture tool: {tool_name}")),
    }
}
