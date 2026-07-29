use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::external_mcp::config::ExternalMcpConfig;
use crate::external_mcp::protocol::{McpCallToolResult, McpListToolsResult, McpTool};
use crate::external_mcp::transport_stdio::StdioTransport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalMcpState {
    Disabled,
    Stopped,
    Starting,
    Initializing,
    Ready,
    Restarting,
    Error,
    Stopping,
}

impl ExternalMcpState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Restarting => "restarting",
            Self::Error => "error",
            Self::Stopping => "stopping",
        }
    }
}

/// 向前端暴露的状态 DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalMcpStatusDto {
    pub config_id: String,
    pub name: String,
    pub enabled: bool,
    pub state: String,
    pub pid: Option<u32>,
    pub discovered_tools_count: usize,
    pub error_message: Option<String>,
}

/// 测试连接返回结果 DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResultDto {
    pub success: bool,
    pub duration_ms: u64,
    pub protocol_version: Option<String>,
    pub discovered_tools: Vec<McpTool>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub resolved_command_display: String,
}

pub struct ExternalMcpInstance {
    pub config: ExternalMcpConfig,
    workspace_id: String,
    workspace_path: PathBuf,
    state: RwLock<ExternalMcpState>,
    transport: RwLock<Option<StdioTransport>>,
    discovered_tools: RwLock<Vec<McpTool>>,
    error_message: RwLock<Option<String>>,
    restart_attempts: RwLock<Vec<Instant>>,
}

impl ExternalMcpInstance {
    pub fn new(workspace_id: String, workspace_path: PathBuf, config: ExternalMcpConfig) -> Self {
        let initial_state = if config.enabled {
            ExternalMcpState::Stopped
        } else {
            ExternalMcpState::Disabled
        };
        Self {
            config,
            workspace_id,
            workspace_path,
            state: RwLock::new(initial_state),
            transport: RwLock::new(None),
            discovered_tools: RwLock::new(Vec::new()),
            error_message: RwLock::new(None),
            restart_attempts: RwLock::new(Vec::new()),
        }
    }

    pub async fn state(&self) -> ExternalMcpState {
        self.state.read().await.clone()
    }

    pub async fn discovered_tools(&self) -> Vec<McpTool> {
        self.discovered_tools.read().await.clone()
    }

    pub async fn status_dto(&self) -> ExternalMcpStatusDto {
        let state = self.state.read().await.clone();
        let pid = match &*self.transport.read().await {
            Some(t) => t.pid(),
            None => None,
        };
        let discovered_tools_count = self.discovered_tools.read().await.len();
        let error_message = self.error_message.read().await.clone();

        ExternalMcpStatusDto {
            config_id: self.config.id.clone(),
            name: self.config.name.clone(),
            enabled: self.config.enabled,
            state: state.as_str().to_string(),
            pid,
            discovered_tools_count,
            error_message,
        }
    }

    /// 启动外部 MCP 实例并完成 initialize 握手与 tools/list 获取
    pub async fn start(&self) -> Result<Vec<McpTool>, String> {
        if !self.config.enabled {
            *self.state.write().await = ExternalMcpState::Disabled;
            return Err("配置未启用".to_string());
        }

        *self.state.write().await = ExternalMcpState::Starting;
        *self.error_message.write().await = None;

        let transport_res = StdioTransport::spawn(
            self.workspace_id.clone(),
            self.workspace_path.clone(),
            self.config.name.clone(),
            self.config.command.clone(),
            self.config.args.clone(),
            self.config.env.clone(),
        )
        .await;

        let mut transport = match transport_res {
            Ok(t) => t,
            Err(e) => {
                *self.state.write().await = ExternalMcpState::Error;
                *self.error_message.write().await = Some(e.clone());
                return Err(e);
            }
        };

        *self.state.write().await = ExternalMcpState::Initializing;

        let root_uri = format!(
            "file:///{}",
            self.workspace_path
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );

        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "coding-tools-mcp",
                "version": env!("CARGO_PKG_VERSION")
            },
            "rootUri": root_uri,
            "workspaceFolders": [
                {
                    "uri": root_uri,
                    "name": self.config.name
                }
            ]
        });

        let init_timeout = Duration::from_secs(self.config.initialize_timeout_seconds);

        // 1. 发送 initialize 请求
        let init_res = transport.send_request("initialize", Some(init_params), init_timeout).await;
        if let Err(e) = init_res {
            transport.kill().await;
            *self.state.write().await = ExternalMcpState::Error;
            let err_msg = format!("MCP 初始化失败: {e}");
            *self.error_message.write().await = Some(err_msg.clone());
            return Err(err_msg);
        }

        // 2. 发送 notifications/initialized 通知
        if let Err(e) = transport.send_notification("notifications/initialized", None).await {
            transport.kill().await;
            *self.state.write().await = ExternalMcpState::Error;
            let err_msg = format!("发送 initialized 通知失败: {e}");
            *self.error_message.write().await = Some(err_msg.clone());
            return Err(err_msg);
        }

        // 3. 发送 tools/list 请求
        let tools_res = transport.send_request("tools/list", None, init_timeout).await;
        let tools = match tools_res {
            Ok(val) => match serde_json::from_value::<McpListToolsResult>(val) {
                Ok(list_res) => list_res.tools,
                Err(e) => {
                    transport.kill().await;
                    *self.state.write().await = ExternalMcpState::Error;
                    let err_msg = format!("解析 tools/list 响应失败: {e}");
                    *self.error_message.write().await = Some(err_msg.clone());
                    return Err(err_msg);
                }
            },
            Err(e) => {
                transport.kill().await;
                *self.state.write().await = ExternalMcpState::Error;
                let err_msg = format!("请求 tools/list 失败: {e}");
                *self.error_message.write().await = Some(err_msg.clone());
                return Err(err_msg);
            }
        };

        *self.discovered_tools.write().await = tools.clone();
        *self.transport.write().await = Some(transport);
        *self.state.write().await = ExternalMcpState::Ready;

        Ok(tools)
    }

    /// 停止外部 MCP 实例
    pub async fn stop(&self) {
        *self.state.write().await = ExternalMcpState::Stopping;
        if let Some(mut t) = self.transport.write().await.take() {
            t.kill().await;
        }
        *self.discovered_tools.write().await = Vec::new();
        if self.config.enabled {
            *self.state.write().await = ExternalMcpState::Stopped;
        } else {
            *self.state.write().await = ExternalMcpState::Disabled;
        }
    }

    /// 转发 tools/call
    pub async fn call_tool(&self, original_tool_name: &str, arguments: &Value) -> Result<Value, String> {
        let state = self.state.read().await.clone();
        if state != ExternalMcpState::Ready {
            return Err(format!("外部 MCP 实例 '{}' 当前不可用 (状态: {})", self.config.name, state.as_str()));
        }

        let transport_guard = self.transport.read().await;
        let transport = transport_guard.as_ref().ok_or("实例传输通道未创建")?;

        let params = serde_json::json!({
            "name": original_tool_name,
            "arguments": arguments
        });

        let call_timeout = Duration::from_secs(self.config.call_timeout_seconds);
        let res_val = transport.send_request("tools/call", Some(params), call_timeout).await?;

        // 尝试解析为 McpCallToolResult 结构
        if let Ok(call_res) = serde_json::from_value::<McpCallToolResult>(res_val.clone()) {
            Ok(serde_json::to_value(call_res).unwrap_or(res_val))
        } else {
            Ok(res_val)
        }
    }

    /// 执行临时测试连接 (用完立刻杀死临时子进程)
    /// 执行临时测试连接 (用完立刻杀死临时子进程)
    pub async fn test_connection(
        workspace_id: &str,
        workspace_path: &PathBuf,
        config: &ExternalMcpConfig,
    ) -> TestConnectionResultDto {
        let start_time = Instant::now();

        let validation = match crate::external_mcp::transport_stdio::validate_launch_config(&config.command, &config.args) {
            Ok(v) => v,
            Err(e) => {
                return TestConnectionResultDto {
                    success: false,
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    protocol_version: None,
                    discovered_tools: Vec::new(),
                    error_kind: Some(e.error_kind().to_string()),
                    error_message: Some(e.message()),
                    resolved_command_display: format!("{} {}", config.command, config.args.join(" ")).trim().to_string(),
                };
            }
        };

        let resolved_command_display = validation.display_cmd.clone();

        let transport_res = StdioTransport::spawn(
            workspace_id.to_string(),
            workspace_path.clone(),
            format!("{}-test", config.name),
            config.command.clone(),
            config.args.clone(),
            config.env.clone(),
        )
        .await;

        let mut transport = match transport_res {
            Ok(t) => t,
            Err(e) => {
                return TestConnectionResultDto {
                    success: false,
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    protocol_version: None,
                    discovered_tools: Vec::new(),
                    error_kind: Some("process_spawn_failed".to_string()),
                    error_message: Some(format!("进程启动失败: {e}")),
                    resolved_command_display,
                };
            }
        };

        let init_timeout = Duration::from_secs(config.initialize_timeout_seconds);

        let root_uri = format!(
            "file:///{}",
            workspace_path
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );

        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "coding-tools-mcp-test",
                "version": env!("CARGO_PKG_VERSION")
            },
            "rootUri": root_uri
        });

        let init_res = transport.send_request("initialize", Some(init_params), init_timeout).await;
        let protocol_version = match &init_res {
            Ok(v) => v.get("protocolVersion").and_then(Value::as_str).map(|s| s.to_string()),
            Err(_) => None,
        };

        if let Err(e) = init_res {
            transport.kill().await;
            let is_timeout = e.contains("超时");
            let kind = if is_timeout { "initialize_timeout" } else { "initialize_failed" };
            return TestConnectionResultDto {
                success: false,
                duration_ms: start_time.elapsed().as_millis() as u64,
                protocol_version: None,
                discovered_tools: Vec::new(),
                error_kind: Some(kind.to_string()),
                error_message: Some(format!("初始化失败: {e}")),
                resolved_command_display,
            };
        }

        let _ = transport.send_notification("notifications/initialized", None).await;

        let tools_res = transport.send_request("tools/list", None, init_timeout).await;
        transport.kill().await;

        let elapsed = start_time.elapsed().as_millis() as u64;

        match tools_res {
            Ok(val) => match serde_json::from_value::<McpListToolsResult>(val) {
                Ok(list_res) => TestConnectionResultDto {
                    success: true,
                    duration_ms: elapsed,
                    protocol_version,
                    discovered_tools: list_res.tools,
                    error_kind: None,
                    error_message: None,
                    resolved_command_display,
                },
                Err(e) => TestConnectionResultDto {
                    success: false,
                    duration_ms: elapsed,
                    protocol_version,
                    discovered_tools: Vec::new(),
                    error_kind: Some("initialize_failed".to_string()),
                    error_message: Some(format!("解析 tools/list 失败: {e}")),
                    resolved_command_display,
                },
            },
            Err(e) => TestConnectionResultDto {
                success: false,
                duration_ms: elapsed,
                protocol_version,
                discovered_tools: Vec::new(),
                error_kind: Some("initialize_failed".to_string()),
                error_message: Some(format!("获取 tools/list 失败: {e}")),
                resolved_command_display,
            },
        }
    }
}
