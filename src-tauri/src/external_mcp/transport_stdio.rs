use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::timeout;

use crate::external_mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::tunnel::append_profile_log;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// 脱敏包含敏感关键字的环境变量
pub fn sanitize_env_for_log(env: &HashMap<String, String>) -> HashMap<String, String> {
    let sensitive_keywords = [
        "KEY", "TOKEN", "SECRET", "PASSWORD", "PASS", "AUTH", "CREDENTIAL",
    ];

    env.iter()
        .map(|(k, v)| {
            let k_upper = k.to_uppercase();
            let is_sensitive = sensitive_keywords.iter().any(|kw| k_upper.contains(kw));
            if is_sensitive && !v.is_empty() {
                (k.clone(), "***".to_string())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchValidation {
    pub resolved_command: String,
    pub display_cmd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    CommandNotFound(String),
    EntryFileNotFound(String),
}

impl LaunchError {
    pub fn error_kind(&self) -> &'static str {
        match self {
            Self::CommandNotFound(_) => "command_not_found",
            Self::EntryFileNotFound(_) => "entry_file_not_found",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::CommandNotFound(m) => m.clone(),
            Self::EntryFileNotFound(m) => m.clone(),
        }
    }
}

/// 校验启动命令及入口文件的存在性与合规性
pub fn validate_launch_config(command: &str, args: &[String]) -> Result<LaunchValidation, LaunchError> {
    let trimmed_cmd = command.trim();
    if trimmed_cmd.is_empty() {
        return Err(LaunchError::CommandNotFound("未找到本地命令，请指定 MCP 入口文件，或选择通过 npx 运行。".to_string()));
    }

    let resolved = resolve_windows_command(trimmed_cmd);
    let path = Path::new(&resolved);

    // 1. 命令可执行性检查
    let command_exists = if path.is_absolute() || path.components().count() > 1 {
        path.exists()
    } else {
        which::which(&resolved).is_ok() || which::which(trimmed_cmd).is_ok()
    };

    if !command_exists {
        return Err(LaunchError::CommandNotFound("未找到本地命令，请指定 MCP 入口文件，或选择通过 npx 运行。".to_string()));
    }

    // 2. 如果命令为 node (或 node.exe / node.cmd)，检查入口文件
    let is_node = trimmed_cmd.eq_ignore_ascii_case("node")
        || resolved.to_lowercase().contains("node.exe")
        || resolved.to_lowercase().contains("node.cmd");

    if is_node {
        let script_arg = args.iter().find(|arg| {
            let lower = arg.to_lowercase();
            lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") || Path::new(arg.as_str()).extension().is_some()
        });

        match script_arg {
            Some(script_path_str) => {
                let script_path = Path::new(script_path_str);
                if !script_path.exists() {
                    return Err(LaunchError::EntryFileNotFound(format!(
                        "入口文件不存在: {script_path_str}"
                    )));
                }

                let ext = script_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if ext != "js" && ext != "mjs" && ext != "cjs" {
                    return Err(LaunchError::EntryFileNotFound(
                        "入口文件类型无效，只支持 .js / .mjs / .cjs 文件".to_string(),
                    ));
                }
            }
            None => {
                return Err(LaunchError::EntryFileNotFound(
                    "使用 node 运行但未提供入口文件，请在参数中指定 .js / .mjs / .cjs 入口文件".to_string(),
                ));
            }        }
    }

    let display_cmd = if args.is_empty() {
        trimmed_cmd.to_string()
    } else {
        format!("{trimmed_cmd} {}", args.join(" "))
    };

    Ok(LaunchValidation {
        resolved_command: resolved,
        display_cmd,
    })
}

/// 解析 Windows 平台可执行命令（如 npx -> npx.cmd）
fn resolve_windows_command(command: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let trimmed = command.trim();
        if trimmed.eq_ignore_ascii_case("npx")
            || trimmed.eq_ignore_ascii_case("npm")
            || trimmed.eq_ignore_ascii_case("pnpm")
            || trimmed.eq_ignore_ascii_case("yarn")
        {
            return format!("{trimmed}.cmd");
        }
        let path = Path::new(trimmed);
        if path.extension().is_none() {
            let cmd_variant = format!("{trimmed}.cmd");
            if which::which(&cmd_variant).is_ok() {
                return cmd_variant;
            }
            let exe_variant = format!("{trimmed}.exe");
            if which::which(&exe_variant).is_ok() {
                return exe_variant;
            }
            let bat_variant = format!("{trimmed}.bat");
            if which::which(&bat_variant).is_ok() {
                return bat_variant;
            }
        }
    }
    command.to_string()
}

pub struct StdioTransport {
    #[allow(dead_code)]
    workspace_id: String,
    #[allow(dead_code)]
    config_name: String,
    child_pid: Option<u32>,
    stdin_tx: mpsc::Sender<String>,
    pending_requests: Arc<RwLock<HashMap<u64, oneshot::Sender<Result<JsonRpcResponse, String>>>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl StdioTransport {
    pub async fn spawn(
        workspace_id: String,
        workspace_path: PathBuf,
        config_name: String,
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<Self, String> {
        let validation = validate_launch_config(&command, &args).map_err(|e| e.message())?;
        let resolved_command = validation.resolved_command;

        let sanitized_env = sanitize_env_for_log(&env);
        append_profile_log(
            &workspace_id,
            "stderr.log",
            &format!(
                "[external-mcp:{config_name}] 正在启动子进程: command={resolved_command}, args={args:?}, env={sanitized_env:?}, cwd={workspace_path:?}"
            ),
        );

        #[cfg(target_os = "windows")]
        let mut cmd = {
            let lower = resolved_command.to_lowercase();
            let is_batch = lower.ends_with(".cmd") || lower.ends_with(".bat");
            if is_batch {
                let mut c = tokio::process::Command::new("cmd.exe");
                c.arg("/d").arg("/c").arg(&resolved_command);
                c.args(&args);
                c
            } else {
                let mut c = tokio::process::Command::new(&resolved_command);
                c.args(&args);
                c
            }
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = tokio::process::Command::new(&resolved_command);
            c.args(&args);
            c
        };

        cmd.current_dir(&workspace_path)
            .envs(&env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            // CREATE_NO_WINDOW: 0x08000000
            cmd.creation_flags(0x08000000);
        }

        let mut child = cmd.spawn().map_err(|e| {
            format!("无法启动外部 MCP 子进程 '{resolved_command}': {e}")
        })?;

        let child_pid = child.id();
        let stdin = child.stdin.take().ok_or("无法获取子进程 stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取子进程 stdout")?;
        let stderr = child.stderr.take().ok_or("无法获取子进程 stderr")?;

        let pending_requests: Arc<RwLock<HashMap<u64, oneshot::Sender<Result<JsonRpcResponse, String>>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        // Stdin 写入任务
        tokio::spawn(async move {
            let mut stdin_writer = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if let Err(e) = stdin_writer.write_all(line.as_bytes()).await {
                    eprintln!("写入 stdin 失败: {e}");
                    break;
                }
                if let Err(e) = stdin_writer.flush().await {
                    eprintln!("flush stdin 失败: {e}");
                    break;
                }
            }
        });

        // Stderr 读取日志任务
        let ws_id_stderr = workspace_id.clone();
        let cfg_name_stderr = config_name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                append_profile_log(
                    &ws_id_stderr,
                    "stderr.log",
                    &format!("[external-mcp:{cfg_name_stderr}:stderr] {line}"),
                );
            }
        });

        // Stdout 读取 JSON-RPC 响应任务
        let pending_map = pending_requests.clone();
        let ws_id_stdout = workspace_id.clone();
        let cfg_name_stdout = config_name.clone();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                tokio::select! {
                    res = reader.next_line() => {
                        match res {
                            Ok(Some(line)) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                // 解析 JSON-RPC 消息
                                match serde_json::from_str::<Value>(trimmed) {
                                    Ok(json_val) => {
                                        // 检查是否为响应 (包含 id)
                                        if let Some(id_val) = json_val.get("id") {
                                            if let Some(req_id) = id_val.as_u64() {
                                                if let Ok(resp) = serde_json::from_value::<JsonRpcResponse>(json_val.clone()) {
                                                    let mut map = pending_map.write().await;
                                                    if let Some(sender) = map.remove(&req_id) {
                                                        let _ = sender.send(Ok(resp));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        append_profile_log(
                                            &ws_id_stdout,
                                            "stderr.log",
                                            &format!("[external-mcp:{cfg_name_stdout}:stdout:non-json] {trimmed} (error: {err})"),
                                        );
                                    }
                                }
                            }
                            Ok(None) => break, // EOF
                            Err(e) => {
                                append_profile_log(
                                    &ws_id_stdout,
                                    "stderr.log",
                                    &format!("[external-mcp:{cfg_name_stdout}:stdout:read_error] {e}"),
                                );
                                break;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        break;
                    }
                }
            }

            // 清理子进程与 pending 请求
            let mut map = pending_map.write().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err("子进程已退出或连接断开".to_string()));
            }

            let _ = child.kill().await;
        });

        Ok(Self {
            workspace_id,
            config_name,
            child_pid,
            stdin_tx,
            pending_requests,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child_pid
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout_duration: Duration,
    ) -> Result<Value, String> {
        let req_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(Value::from(req_id), method, params);
        let json_str = serde_json::to_string(&req).map_err(|e| format!("序列化请求失败: {e}"))? + "\n";

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending_requests.write().await;
            map.insert(req_id, tx);
        }

        if let Err(e) = self.stdin_tx.send(json_str).await {
            let mut map = self.pending_requests.write().await;
            map.remove(&req_id);
            return Err(format!("发送请求到 stdin 失败: {e}"));
        }

        match timeout(timeout_duration, rx).await {
            Ok(Ok(Ok(response))) => {
                if let Some(err) = response.error {
                    Err(format!("MCP JSON-RPC 错误 [code {}]: {}", err.code, err.message))
                } else if let Some(res) = response.result {
                    Ok(res)
                } else {
                    Ok(Value::Null)
                }
            }
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => {
                let mut map = self.pending_requests.write().await;
                map.remove(&req_id);
                Err("请求响应 channel 已关闭".to_string())
            }
            Err(_) => {
                let mut map = self.pending_requests.write().await;
                map.remove(&req_id);
                Err(format!("请求 '{method}' 超时 ({}秒)", timeout_duration.as_secs()))
            }
        }
    }

    pub async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), String> {
        let notif = crate::external_mcp::protocol::JsonRpcNotification::new(method, params);
        let json_str = serde_json::to_string(&notif).map_err(|e| format!("序列化通知失败: {e}"))? + "\n";
        self.stdin_tx
            .send(json_str)
            .await
            .map_err(|e| format!("发送通知到 stdin 失败: {e}"))
    }

    pub async fn kill(&mut self) {
        if let Some(shutdown) = self.shutdown_tx.take() {
            let _ = shutdown.send(());
        }
        let mut map = self.pending_requests.write().await;
        for (_, tx) in map.drain() {
            let _ = tx.send(Err("MCP 传输通道已被终止 (kill)".to_string()));
        }
        if let Some(pid) = self.child_pid.take() {
            kill_process_tree(pid);
        }
    }
}

/// 终止子进程树
fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
