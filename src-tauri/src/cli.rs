//! Headless entry point for the upstream agent evaluation harness and local integrations.
use crate::{
    tools::{PolicySettings, ToolContext, Workspace},
    workspace::AuthConfig,
};
use axum::{extract::State, routing::post, Json, Router};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

pub fn run(args: &[String]) -> Result<bool, String> {
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "coding-tools-mcp {} (desktop; upstream 9d2c179)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(true);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("coding-tools-mcp [--version] [--serve --workspace PATH --port PORT --tool-profile core|advanced|read-only --permission-mode safe|trusted|dangerous --harness-root PATH] [--workspace-mutation unrestricted|structured-only] [--write-path PATH ...]");
        return Ok(true);
    }
    let value = |flag: &str| -> Result<Option<String>, String> {
        let Some(index) = args.iter().position(|a| a == flag) else {
            return Ok(None);
        };
        args.get(index + 1)
            .filter(|a| !a.starts_with("--"))
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("Missing value for {flag}"))
    };
    if let Some(mode) = value("--workspace-mutation")? {
        if !["unrestricted", "structured-only"].contains(&mode.as_str()) {
            return Err("Invalid workspace mutation mode".into());
        }
        std::env::set_var("CODING_TOOLS_MCP_WORKSPACE_MUTATION", mode);
    }
    let mut write_paths = std::env::var_os("CODING_TOOLS_MCP_WRITE_PATHS")
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default();
    for (index, flag) in args.iter().enumerate() {
        if flag == "--write-path" {
            let path = args
                .get(index + 1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("Missing value for --write-path")?;
            write_paths.push(PathBuf::from(path));
        }
    }
    if !write_paths.is_empty() {
        if std::env::var("CODING_TOOLS_MCP_WORKSPACE_MUTATION").unwrap_or_default()
            != "structured-only"
        {
            return Err("--write-path requires structured-only mode".into());
        }
        std::env::set_var(
            "CODING_TOOLS_MCP_WRITE_PATHS",
            std::env::join_paths(write_paths).map_err(|e| e.to_string())?,
        );
    }
    if !args.iter().any(|a| a == "--serve") {
        return Ok(false);
    }
    if value("--host")?.is_some_and(|host| host != "127.0.0.1" && host != "localhost") {
        return Err("Headless serving binds to localhost only".into());
    }
    let root = PathBuf::from(value("--workspace")?.ok_or("--serve requires --workspace")?);
    let port = value("--port")?
        .unwrap_or_else(|| "28766".into())
        .parse::<u16>()
        .map_err(|_| "Invalid port")?;
    let profile = value("--tool-profile")?.unwrap_or_else(|| "core".into());
    if !["core", "advanced", "read-only", "compat-readonly-all"].contains(&profile.as_str()) {
        return Err("Invalid tool profile".into());
    }
    let permission = value("--permission-mode")?.unwrap_or_else(|| "trusted".into());
    if !["safe", "trusted", "dangerous"].contains(&permission.as_str()) {
        return Err("Invalid permission mode".into());
    }
    let workspace = Workspace::new(root).map_err(|e| e.message())?;
    let policy = PolicySettings {
        permission_mode: permission.clone(),
        ..PolicySettings::default()
    };
    let auth = AuthConfig {
        auth_type: "noauth".into(),
        ..AuthConfig::default()
    };
    let context = if let Some(harness) = value("--harness-root")? {
        ToolContext::from_workspace_with_harness_root(
            workspace,
            auth,
            policy,
            profile,
            permission,
            PathBuf::from(harness),
        )
    } else {
        ToolContext::from_workspace(workspace, auth, policy, profile, permission)
    };
    context
        .mutation_policy
        .validate()
        .map_err(|e| e.message())?;
    let state = Arc::new(context);
    tauri::async_runtime::block_on(async move {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| e.to_string())?;
        let router = Router::new()
            .route("/mcp", post(mcp_post))
            .with_state(state);
        axum::serve(listener, router)
            .await
            .map_err(|e| e.to_string())
    })?;
    Ok(true)
}
async fn mcp_post(
    State(state): State<Arc<ToolContext>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let result =
        tauri::async_runtime::spawn_blocking(move || crate::mcp::handle_request(&state, &request))
            .await;
    Json(result.unwrap_or_else(|_|serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Tool worker failed"}})))
}
