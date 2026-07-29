use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::external_mcp::config::ExternalMcpConfig;
use crate::external_mcp::namespace::make_public_tool_name;
use crate::external_mcp::protocol::McpTool;

/// 内部记录的外部工具明细条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalToolEntry {
    /// 对外暴露的公网工具名 (如 fast-context__fast_context_search)
    pub public_name: String,
    /// 原始 MCP 工具名 (如 fast_context_search)
    pub original_name: String,
    /// 配置 ID
    pub server_id: String,
    /// 显示名称
    pub server_name: String,
    /// 描述信息
    pub description: Option<String>,
    /// inputSchema
    pub input_schema: Value,
    /// annotations
    pub annotations: Option<Value>,
}

#[derive(Default, Debug, Clone)]
pub struct ExternalToolRegistry {
    /// public_name -> ExternalToolEntry
    tools: HashMap<String, ExternalToolEntry>,
}

impl ExternalToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 根据配置与外部 MCP 发现的原始工具列表，注册并生成命名空间工具
    pub fn register_server_tools(
        &mut self,
        config: &ExternalMcpConfig,
        discovered_tools: &[McpTool],
    ) {
        let allowed_set: Option<HashSet<&str>> = if config.allowed_tools.is_empty() {
            None
        } else {
            Some(config.allowed_tools.iter().map(|s| s.as_str()).collect())
        };

        for tool in discovered_tools {
            // 白名单校验
            if let Some(ref set) = allowed_set {
                if !set.contains(tool.name.as_str()) {
                    continue;
                }
            }

            let base_public_name = make_public_tool_name(&config.name, &config.id, &tool.name);
            let mut final_public_name = base_public_name.clone();
            let mut counter = 2;

            // 处理重名冲突
            while self.tools.contains_key(&final_public_name) {
                final_public_name = format!("{base_public_name}-{counter}");
                counter += 1;
            }

            let desc = match &tool.description {
                Some(d) => format!("[External MCP: {}] {}", config.name, d),
                None => format!("[External MCP: {}]", config.name),
            };

            let entry = ExternalToolEntry {
                public_name: final_public_name.clone(),
                original_name: tool.name.clone(),
                server_id: config.id.clone(),
                server_name: config.name.clone(),
                description: Some(desc),
                input_schema: tool.input_schema.clone(),
                annotations: tool.annotations.clone(),
            };

            self.tools.insert(final_public_name, entry);
        }
    }

    /// 清除指定 server_id 的所有工具
    pub fn remove_server_tools(&mut self, server_id: &str) {
        self.tools.retain(|_, entry| entry.server_id != server_id);
    }

    /// 获取所有公开的 MCP 工具结构（用于公网 tools/list）
    pub fn to_mcp_tools_json(&self) -> Vec<Value> {
        let mut list = Vec::new();
        for entry in self.tools.values() {
            let mut item = serde_json::json!({
                "name": entry.public_name,
                "description": entry.description.as_deref().unwrap_or(""),
                "inputSchema": entry.input_schema
            });
            if let Some(ref ann) = entry.annotations {
                item["annotations"] = ann.clone();
            }
            list.push(item);
        }
        list
    }

    /// 查找指定公网工具名的注册条目
    pub fn get(&self, public_name: &str) -> Option<&ExternalToolEntry> {
        self.tools.get(public_name)
    }

    /// 获取当前注册的所有条目
    pub fn all_entries(&self) -> Vec<ExternalToolEntry> {
        self.tools.values().cloned().collect()
    }
}
