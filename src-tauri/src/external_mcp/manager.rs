use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::external_mcp::config::ExternalMcpConfig;
use crate::external_mcp::instance::{ExternalMcpInstance, ExternalMcpStatusDto, TestConnectionResultDto};
use crate::external_mcp::protocol::McpTool;
use crate::external_mcp::tool_registry::{ExternalToolEntry, ExternalToolRegistry};

#[derive(Default)]
pub struct ExternalMcpManager {
    /// workspace_id -> (config_id -> ExternalMcpInstance)
    instances: RwLock<HashMap<String, HashMap<String, Arc<ExternalMcpInstance>>>>,
    /// workspace_id -> ExternalToolRegistry
    registries: RwLock<HashMap<String, ExternalToolRegistry>>,
}

pub type SharedExternalMcpManager = Arc<ExternalMcpManager>;

impl ExternalMcpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 启动指定工作区的所有已启用的外部 MCP 服务
    pub async fn start_workspace_mcps(
        &self,
        workspace_id: &str,
        workspace_path: &PathBuf,
        configs: &[ExternalMcpConfig],
    ) {
        // 先清理可能存在的旧实例
        self.stop_workspace_mcps(workspace_id).await;

        let mut instance_map = HashMap::new();
        let mut registry = ExternalToolRegistry::new();

        for config in configs {
            let instance = Arc::new(ExternalMcpInstance::new(
                workspace_id.to_string(),
                workspace_path.clone(),
                config.clone(),
            ));

            if config.enabled {
                match instance.start().await {
                    Ok(discovered) => {
                        registry.register_server_tools(config, &discovered);
                    }
                    Err(e) => {
                        eprintln!("外部 MCP [{}] 启动失败: {e}", config.name);
                    }
                }
            }

            instance_map.insert(config.id.clone(), instance);
        }

        self.instances.write().await.insert(workspace_id.to_string(), instance_map);
        self.registries.write().await.insert(workspace_id.to_string(), registry);
    }

    /// 停止指定工作区的所有外部 MCP 服务
    pub async fn stop_workspace_mcps(&self, workspace_id: &str) {
        if let Some(map) = self.instances.write().await.remove(workspace_id) {
            for (_, instance) in map {
                instance.stop().await;
            }
        }
        self.registries.write().await.remove(workspace_id);
    }

    /// 重新连接/重启指定工作区的单条 MCP 实例
    pub async fn reconnect_instance(
        &self,
        workspace_id: &str,
        workspace_path: &PathBuf,
        config: &ExternalMcpConfig,
    ) -> Result<ExternalMcpStatusDto, String> {
        let instance = {
            let mut instances_map = self.instances.write().await;
            let ws_map = instances_map.entry(workspace_id.to_string()).or_default();
            
            if let Some(existing) = ws_map.remove(&config.id) {
                existing.stop().await;
            }

            let new_instance = Arc::new(ExternalMcpInstance::new(
                workspace_id.to_string(),
                workspace_path.clone(),
                config.clone(),
            ));
            ws_map.insert(config.id.clone(), new_instance.clone());
            new_instance
        };

        let result = if config.enabled {
            match instance.start().await {
                Ok(discovered) => {
                    let mut regs = self.registries.write().await;
                    let reg = regs.entry(workspace_id.to_string()).or_default();
                    reg.remove_server_tools(&config.id);
                    reg.register_server_tools(config, &discovered);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            let mut regs = self.registries.write().await;
            if let Some(reg) = regs.get_mut(workspace_id) {
                reg.remove_server_tools(&config.id);
            }
            Ok(())
        };

        let dto = instance.status_dto().await;
        result.map(|_| dto)
    }

    /// 获取工作区聚合的外部 MCP 工具列表 (格式化为公网 MCP JSON)
    pub async fn get_aggregated_tools(&self, workspace_id: &str) -> Vec<Value> {
        let regs = self.registries.read().await;
        if let Some(reg) = regs.get(workspace_id) {
            reg.to_mcp_tools_json()
        } else {
            Vec::new()
        }
    }

    /// 查找工具条目
    pub async fn find_tool_entry(&self, workspace_id: &str, public_tool_name: &str) -> Option<ExternalToolEntry> {
        let regs = self.registries.read().await;
        regs.get(workspace_id)?.get(public_tool_name).cloned()
    }

    /// 执行外部工具转发调用
    pub async fn call_external_tool(
        &self,
        workspace_id: &str,
        public_tool_name: &str,
        arguments: &Value,
    ) -> Result<Value, String> {
        let tool_entry = self
            .find_tool_entry(workspace_id, public_tool_name)
            .await
            .ok_or_else(|| format!("未在工作区找到外部工具: {public_tool_name}"))?;

        let instance = {
            let map = self.instances.read().await;
            map.get(workspace_id)
                .and_then(|m| m.get(&tool_entry.server_id).cloned())
                .ok_or_else(|| format!("外部 MCP 服务实例不存在: {}", tool_entry.server_name))?
        };

        instance.call_tool(&tool_entry.original_name, arguments).await
    }

    /// 获取工作区所有外部 MCP 状态 DTO 列表
    pub async fn get_workspace_statuses(&self, workspace_id: &str) -> Vec<ExternalMcpStatusDto> {
        let map_guard = self.instances.read().await;
        if let Some(ws_map) = map_guard.get(workspace_id) {
            let mut list = Vec::new();
            for instance in ws_map.values() {
                list.push(instance.status_dto().await);
            }
            list
        } else {
            Vec::new()
        }
    }

    /// 获取工作区某外部 MCP 现已发现的工具列表
    pub async fn get_discovered_tools(&self, workspace_id: &str, config_id: &str) -> Vec<McpTool> {
        let map_guard = self.instances.read().await;
        if let Some(ws_map) = map_guard.get(workspace_id) {
            if let Some(instance) = ws_map.get(config_id) {
                return instance.discovered_tools().await;
            }
        }
        Vec::new()
    }

    /// 执行测试连接
    pub async fn test_connection(
        &self,
        workspace_id: &str,
        workspace_path: &PathBuf,
        config: &ExternalMcpConfig,
    ) -> TestConnectionResultDto {
        ExternalMcpInstance::test_connection(workspace_id, workspace_path, config).await
    }
}
