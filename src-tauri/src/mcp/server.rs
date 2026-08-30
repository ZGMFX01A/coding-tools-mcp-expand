use std::sync::Arc;

use serde_json::Value;

use crate::mcp::protocol::{
    detect_request_context, negotiate_legacy_version, shape_result, MODERN_PROTOCOL_VERSIONS,
};
use crate::tools::{
    call_tool, list_tools_for_profile, wrap_mcp_tool_result, SharedToolContext, ToolContext,
    Workspace,
};
use crate::workspace::AuthConfig;

pub type SharedState = SharedToolContext;

pub fn handle_request(state: &SharedState, body: &Value) -> Value {
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let params = body.get("params").cloned().unwrap_or(Value::Null);

    if id.is_null() && method.starts_with("notifications/") {
        return Value::Null;
    }

    let req_ctx = detect_request_context(method, &params);

    let result = match method {
        "initialize" => {
            let req_version = params.get("protocolVersion").and_then(Value::as_str);
            let negotiated = negotiate_legacy_version(req_version);
            Ok(initialize_result(&negotiated))
        }
        "server/discover" => Ok(discover_result()),
        "notifications/initialized" | "notifications/cancelled" => return Value::Null,
        "ping" => Ok(serde_json::json!({})),
        "tools/list" => {
            let mut tools = list_tools_for_profile(&state.tool_profile);
            if let Some(ref external_mgr) = state.external_mcp {
                let external_tools = tauri::async_runtime::block_on(
                    external_mgr.get_aggregated_tools(&state.workspace_id),
                );
                tools.extend(external_tools);
            }
            Ok(serde_json::json!({ "tools": tools }))
        }
        "tools/call" => handle_tools_call(state, &params),
        _ => Err(serde_json::json!({
            "code": -32601,
            "message": format!("Method not found: {method}")
        })),
    };

    match result {
        Ok(result) => {
            let shaped = shape_result(&req_ctx, result);
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": shaped })
        }
        Err(error) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    }
}

pub fn initialize_result(protocol_version: &str) -> Value {
    serde_json::json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": false },
            "logging": {}
        },
        "serverInfo": {
            "name": "coding-tools-mcp",
            "title": "Coding Tools MCP",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Use these tools only for local coding operations inside the configured workspace. At the start of every new ChatGPT conversation, before answering the user's first request, call history_session_bootstrap exactly once and pass the user's verbatim first request as initial_user_input. Treat bootstrap as required conversation initialization: it creates or resumes a lossless Markdown archive and returns bounded current state, not all history. Use history_session_search followed by history_session_read only when exact earlier context is needed. history_session_read returns a bounded UTF-8-safe page; follow next_cursor with the returned content hash until the relevant archive is complete. Repeated successful bootstrap calls in the same conversation resume the same session and must not create duplicates. Preserve session_key and current_path returned by bootstrap, then pass them unchanged as session_key and expected_path to every history_session_checkpoint call. After completing each user-requested task in the conversation, call history_session_checkpoint before the final response and pass that user's verbatim request as raw_user_input. Only state that progress was saved after checkpoint returns ok=true with the same session_key and path. The server cannot access ChatGPT transcript text that was not provided as a tool argument; persistence is not automatic background persistence."
    })
}

pub fn discover_result() -> Value {
    serde_json::json!({
        "supportedVersions": MODERN_PROTOCOL_VERSIONS,
        "capabilities": {
            "tools": { "listChanged": false },
            "logging": {}
        },
        "instructions": "Use these tools only for local coding operations inside the configured workspace. At the start of every new ChatGPT conversation, before answering the user's first request, call history_session_bootstrap exactly once and pass the user's verbatim first request as initial_user_input."
    })
}

fn handle_tools_call(state: &SharedState, params: &Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| serde_json::json!({ "code": -32602, "message": "Missing tool name" }))?;

    let openai_session = params
        .get("_meta")
        .and_then(|meta| meta.get("openai/session"))
        .and_then(Value::as_str);

    let mut args = tool_arguments(name, params);
    if !args.is_object() {
        args = serde_json::json!({});
    }

    let is_external = if let Some(ref external_mgr) = state.external_mcp {
        tauri::async_runtime::block_on(external_mgr.find_tool_entry(&state.workspace_id, name))
    } else {
        None
    };

    let decision = if let Some(session) = openai_session.map(str::trim).filter(|s| !s.is_empty()) {
        let now = state.turn_budget.clock().now();
        let (_confidence, identity) = state.turn_correlator.correlate(
            &state.workspace_id,
            session,
            &state.turn_registry,
            now,
        );
        state.turn_budget.start_call_with_identity(
            identity,
            name,
            is_external.is_some(),
            &args,
        )
    } else {
        // 缺少 session 时，绑定至该工作区专属的稳定 fallback 预算桶，确保连续调用累计消耗预算，且不同工作区物理隔离。
        let fallback_id = format!("unmanaged_ws_{}", state.workspace_id);
        let identity = crate::mcp::browser_turn::TurnIdentity::WorkspaceFallback {
            workspace_id: state.workspace_id.clone(),
            fallback_id,
        };
        state.turn_budget.start_call_with_identity(
            identity,
            name,
            is_external.is_some(),
            &args,
        )
    };

    match decision {
        crate::mcp::turn_budget::CallDecision::Blocked {
            snapshot,
            error_payload,
            content_text,
        }
        | crate::mcp::turn_budget::CallDecision::Restricted {
            snapshot,
            error_payload,
            content_text,
        } => {
            return Ok(state.turn_budget.build_blocked_result(
                &snapshot,
                &error_payload,
                &content_text,
            ));
        }
        crate::mcp::turn_budget::CallDecision::Allowed {
            guard,
            runtime_budget,
            snapshot,
            emit_full_warning,
            ..
        } => {
            args["_runtime_budget_ms"] = serde_json::json!(runtime_budget.as_millis() as u64);

            // 检查是否为外部 stdio MCP 工具
            if is_external.is_some() {
                if let Some(ref external_mgr) = state.external_mcp {
                    let res = tauri::async_runtime::block_on(
                        external_mgr.call_external_tool_with_budget(
                            &state.workspace_id,
                            name,
                            &args,
                            Some(runtime_budget),
                        ),
                    );
                    drop(guard);
                    return match res {
                        Ok(val) => {
                            Ok(state
                                .turn_budget
                                .decorate_allowed_result(val, &snapshot, emit_full_warning))
                        }
                        Err(err_msg) => Err(serde_json::json!({
                            "code": -32603,
                            "message": format!("外部工具调用失败: {err_msg}")
                        })),
                    };
                }
            }

            let canonical_name = crate::tools::registry::canonical_tool_name(name);
            let known = crate::tools::registry::exposed_tool_names(&state.tool_profile);
            if !known.iter().any(|n| n == &canonical_name) {
                drop(guard);
                return Err(serde_json::json!({
                    "code": -32602,
                    "message": format!("Unknown tool: {name}"),
                    "data": { "reason": "unknown_tool" }
                }));
            }

            let structured = call_tool(state.as_ref(), canonical_name, &args);
            let wrapped = wrap_mcp_tool_result(canonical_name, &args, structured);
            drop(guard);
            Ok(state
                .turn_budget
                .decorate_allowed_result(wrapped, &snapshot, emit_full_warning))
        }
        crate::mcp::turn_budget::CallDecision::Unmanaged => {
            let args = tool_arguments(name, params);

            // 检查是否为外部 stdio MCP 工具
            if let Some(ref external_mgr) = state.external_mcp {
                let is_external = tauri::async_runtime::block_on(
                    external_mgr.find_tool_entry(&state.workspace_id, name),
                );
                if is_external.is_some() {
                    let res = tauri::async_runtime::block_on(
                        external_mgr.call_external_tool(&state.workspace_id, name, &args),
                    );
                    return match res {
                        Ok(val) => Ok(val),
                        Err(err_msg) => Err(serde_json::json!({
                            "code": -32603,
                            "message": format!("外部工具调用失败: {err_msg}")
                        })),
                    };
                }
            }

            let canonical_name = crate::tools::registry::canonical_tool_name(name);
            let known = crate::tools::registry::exposed_tool_names(&state.tool_profile);
            if !known.iter().any(|n| n == &canonical_name) {
                return Err(serde_json::json!({
                    "code": -32602,
                    "message": format!("Unknown tool: {name}"),
                    "data": { "reason": "unknown_tool" }
                }));
            }

            let structured = call_tool(state.as_ref(), canonical_name, &args);
            Ok(wrap_mcp_tool_result(canonical_name, &args, structured))
        }
    }
}

fn tool_arguments(name: &str, params: &Value) -> Value {
    let mut args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if name.starts_with("history_session_") {
        if let Some(session_key) = params
            .get("_meta")
            .and_then(|meta| meta.get("openai/session"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !args.is_object() {
                args = serde_json::json!({});
            }
            args["_host_session_key"] = Value::String(session_key.to_string());
        }
    }
    args
}

pub fn new_state(
    workspace: Workspace,
    auth: AuthConfig,
    policy: crate::tools::policy::PolicySettings,
    tool_profile: String,
    permission_mode: String,
) -> SharedState {
    Arc::new(ToolContext::from_workspace(
        workspace,
        auth,
        policy,
        tool_profile,
        permission_mode,
    ))
}

pub fn new_state_with_external_mcp(
    workspace: Workspace,
    auth: AuthConfig,
    policy: crate::tools::policy::PolicySettings,
    tool_profile: String,
    permission_mode: String,
    workspace_id: String,
    external_manager: Option<std::sync::Arc<crate::external_mcp::ExternalMcpManager>>,
) -> SharedState {
    Arc::new(ToolContext::from_workspace_with_external_mcp(
        workspace,
        auth,
        policy,
        tool_profile,
        permission_mode,
        workspace_id,
        external_manager,
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use serde_json::json;

    use crate::tools::ToolContext;

    use super::{handle_request, initialize_result, tool_arguments};

    #[test]
    fn initialize_instructions_define_the_history_persistence_workflow() {
        let initialized = initialize_result("2025-06-18");
        let instructions = initialized["instructions"].as_str().expect("instructions");
        assert!(instructions.contains("history_session_bootstrap"));
        assert!(instructions.contains("At the start of every new ChatGPT conversation"));
        assert!(instructions.contains("before answering the user's first request"));
        assert!(instructions.contains("required conversation initialization"));
        assert!(instructions.contains("initial_user_input"));
        assert!(instructions.contains("must not create duplicates"));
        assert!(instructions.contains("history_session_checkpoint"));
        assert!(instructions.contains("raw_user_input"));
        assert!(instructions.contains("history_session_search"));
        assert!(instructions.contains("history_session_read"));
        assert!(instructions.contains("follow next_cursor"));
        assert!(instructions.contains("session_key and current_path returned by bootstrap"));
        assert!(instructions.contains("session_key and expected_path"));
        assert!(instructions.contains("After completing each user-requested task"));
        assert!(instructions.contains("before the final response"));
        assert!(instructions.contains("checkpoint returns ok=true"));
        assert!(instructions.contains("not automatic background persistence"));
    }

    #[test]
    fn initialize_does_not_claim_tool_catalog_notifications_without_a_stream() {
        let initialized = initialize_result("2025-06-18");

        assert_eq!(initialized["capabilities"]["tools"]["listChanged"], false);
    }

    #[test]
    fn test_multi_version_initialize_and_discover() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        // 1. initialize 协商 2025-11-25
        let res_1125 = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25"
                }
            }),
        );
        assert_eq!(res_1125["result"]["protocolVersion"], "2025-11-25");

        // 2. initialize 协商 2025-06-18
        let res_0618 = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18"
                }
            }),
        );
        assert_eq!(res_0618["result"]["protocolVersion"], "2025-06-18");

        // 3. initialize 协商 2024-11-05
        let res_1105 = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05"
                }
            }),
        );
        assert_eq!(res_1105["result"]["protocolVersion"], "2024-11-05");

        // 4. server/discover 返回 2026-07-28
        let res_discover = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "server/discover",
                "params": {}
            }),
        );
        assert_eq!(res_discover["result"]["resultType"], "complete");
        assert_eq!(res_discover["result"]["supportedVersions"][0], "2026-07-28");
        assert!(res_discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"].is_string());

        // 5. modern tools/list 带有 _meta
        let res_tools = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/list",
                "params": {
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28"
                    }
                }
            }),
        );
        assert_eq!(res_tools["result"]["resultType"], "complete");
        assert!(res_tools["result"]["tools"].is_array());
    }

    #[test]
    fn workspace_prompt_initializes_or_restores_a_chatgpt_session() {
        let component = include_str!("../../../src/lib/components/ChatGptSessionPrompt.svelte");

        assert!(component.contains("ChatGPT 新会话启动提示词"));
        assert!(component.contains("请初始化或恢复当前项目会话"));
        assert!(component.contains("如果没有历史记录"));
        assert!(component.contains("initial_user_input"));
        assert!(component.contains("raw_user_input"));
        assert!(component.contains("history_session_search"));
        assert!(component.contains("history_session_checkpoint"));
        assert!(!component.contains("打开连接器设置"));
    }

    #[test]
    fn chatgpt_session_metadata_is_injected_only_for_history_tools() {
        let params = json!({
            "arguments": {"session_key": "explicit"},
            "_meta": {"openai/session": "chatgpt-conversation"}
        });
        let history = tool_arguments("history_session_bootstrap", &params);
        assert_eq!(history["session_key"], "explicit");
        assert_eq!(history["_host_session_key"], "chatgpt-conversation");

        let existing = tool_arguments("read_file", &params);
        assert_eq!(existing["session_key"], "explicit");
        assert!(existing.get("_host_session_key").is_none());
    }

    #[test]
    fn host_session_key_takes_precedence_over_explicit_session_key() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let response = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "history_session_bootstrap",
                    "arguments": {
                        "session_key": "explicit-session",
                        "initial_user_input": "保存首轮原文"
                    },
                    "_meta": {"openai/session": "chatgpt-session"}
                }
            }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["ok"], true);
        assert_eq!(structured["session_key_source"], "platform_conversation_id");
        assert_eq!(structured["session_key"], "chatgpt-session");
        assert_eq!(structured["initial_input_captured"], true);
        let content = fs::read_to_string(workspace.path().join("docs/history-session/1.md"))
            .expect("read history file");
        assert!(content.contains("**Session key:** chatgpt-session"));
        assert!(!content.contains("**Session key:** explicit-session"));
    }

    #[test]
    fn legacy_grep_calls_are_mapped_to_the_public_grep_text_tool() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        fs::write(workspace.path().join("sample.txt"), "catalog needle")
            .expect("write sample file");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let response = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "grep",
                    "arguments": {"query": "needle", "path": "."}
                }
            }),
        );

        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["structuredContent"]["ok"], true);
    }
}
