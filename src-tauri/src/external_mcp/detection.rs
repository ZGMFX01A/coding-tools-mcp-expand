use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastContextDetectionResult {
    pub detected: bool,
    pub mode: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub entry_path: Option<String>,
    pub message: String,
}

pub fn detect_fast_context_env() -> FastContextDetectionResult {
    let cmd_name = "fast-context-mcp";
    let cmd_variant = format!("{cmd_name}.cmd");

    if which::which(cmd_name).is_ok() || which::which(&cmd_variant).is_ok() {
        return FastContextDetectionResult {
            detected: true,
            mode: Some("local_cmd".to_string()),
            command: Some(cmd_name.to_string()),
            args: vec![],
            entry_path: None,
            message: "已检测到本机全局命令: fast-context-mcp (推荐)".to_string(),
        };
    }

    let mut possible_paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            possible_paths.push(format!("{appdata}\\npm\\node_modules\\fast-context-mcp\\dist\\index.js"));
            possible_paths.push(format!("{appdata}\\npm\\fast-context-mcp.cmd"));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        possible_paths.push("/usr/local/lib/node_modules/fast-context-mcp/dist/index.js".to_string());
        possible_paths.push("/usr/local/bin/fast-context-mcp".to_string());
    }

    for path_str in possible_paths {
        let p = Path::new(&path_str);
        if p.exists() {
            if path_str.ends_with(".js") {
                return FastContextDetectionResult {
                    detected: true,
                    mode: Some("local_file".to_string()),
                    command: Some("node".to_string()),
                    args: vec![path_str.clone()],
                    entry_path: Some(path_str),
                    message: "已检测到 npm 全局安装的入口文件".to_string(),
                };
            } else {
                return FastContextDetectionResult {
                    detected: true,
                    mode: Some("local_cmd".to_string()),
                    command: Some(path_str.clone()),
                    args: vec![],
                    entry_path: None,
                    message: "已检测到 npm 全局命令入口".to_string(),
                };
            }
        }
    }

    FastContextDetectionResult {
        detected: false,
        mode: None,
        command: None,
        args: vec![],
        entry_path: None,
        message: "未检测到本机 fast-context，请选择本地入口文件，或通过 npx 运行。".to_string(),
    }
}
