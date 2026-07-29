use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// 外部 stdio MCP 服务配置结构体
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalMcpConfig {
    /// 唯一标识
    pub id: String,
    /// 显示名称（同时作为工具命名空间前缀）
    pub name: String,
    /// 是否开启
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 启动命令（如 npx, node, python 等）
    pub command: String,
    /// 命令参数列表
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量设置
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// 工具白名单列表，为空表示暴露所有发现的工具
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// 异常退出后是否自动重启
    #[serde(default = "default_true")]
    pub auto_restart: bool,
    /// 初始化超时时间（秒）
    #[serde(default = "default_initialize_timeout")]
    pub initialize_timeout_seconds: u64,
    /// 单次工具调用超时时间（秒）
    #[serde(default = "default_call_timeout")]
    pub call_timeout_seconds: u64,
}

fn default_true() -> bool {
    true
}

fn default_initialize_timeout() -> u64 {
    30
}

fn default_call_timeout() -> u64 {
    120
}

impl ExternalMcpConfig {
    /// 校验配置基础合法性
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("配置 ID 不能为空".to_string());
        }
        if self.name.trim().is_empty() {
            return Err("配置名称不能为空".to_string());
        }
        if self.command.trim().is_empty() {
            return Err("启动命令不能为空".to_string());
        }
        Ok(())
    }
}
