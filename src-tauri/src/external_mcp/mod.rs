pub mod config;
pub mod detection;
pub mod fixture;
pub mod instance;
pub mod manager;
pub mod namespace;
pub mod protocol;
pub mod tool_registry;
pub mod transport_stdio;

pub use config::ExternalMcpConfig;
pub use detection::{detect_fast_context_env, FastContextDetectionResult};
pub use instance::{ExternalMcpState, ExternalMcpStatusDto, TestConnectionResultDto};
pub use manager::{ExternalMcpManager, SharedExternalMcpManager};

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use serde_json::json;

    use super::config::ExternalMcpConfig;
    use super::namespace::{make_public_tool_name, normalize_server_name};
    use super::protocol::McpTool;
    use super::tool_registry::ExternalToolRegistry;
    use super::transport_stdio::sanitize_env_for_log;

    #[test]
    fn test_sanitize_env_for_log() {
        let mut env = HashMap::new();
        env.insert("FC_INCLUDE_SNIPPETS".to_string(), "true".to_string());
        env.insert("WINDSURF_API_KEY".to_string(), "sk-secret-12345".to_string());
        env.insert("DB_PASSWORD".to_string(), "pass123".to_string());
        env.insert("NORMAL_VAR".to_string(), "hello".to_string());

        let sanitized = sanitize_env_for_log(&env);
        assert_eq!(sanitized.get("FC_INCLUDE_SNIPPETS").unwrap(), "true");
        assert_eq!(sanitized.get("WINDSURF_API_KEY").unwrap(), "***");
        assert_eq!(sanitized.get("DB_PASSWORD").unwrap(), "***");
        assert_eq!(sanitized.get("NORMAL_VAR").unwrap(), "hello");
    }

    #[test]
    fn test_tool_registry_allowed_tools_whitelist() {
        let config = ExternalMcpConfig {
            id: "mcp-fc".to_string(),
            name: "fast-context".to_string(),
            enabled: true,
            command: "npx".to_string(),
            args: vec![],
            env: HashMap::new(),
            allowed_tools: vec!["fast_context_search".to_string()],
            auto_restart: true,
            initialize_timeout_seconds: 30,
            call_timeout_seconds: 120,
        };

        let discovered = vec![
            McpTool {
                name: "fast_context_search".to_string(),
                description: Some("Search code".to_string()),
                input_schema: json!({}),
                annotations: None,
            },
            McpTool {
                name: "extract_windsurf_key".to_string(),
                description: Some("Extract key".to_string()),
                input_schema: json!({}),
                annotations: None,
            },
        ];

        let mut registry = ExternalToolRegistry::new();
        registry.register_server_tools(&config, &discovered);

        let tools_json = registry.to_mcp_tools_json();
        assert_eq!(tools_json.len(), 1);
        assert_eq!(tools_json[0]["name"], "fast-context__fast_context_search");
    }
}
