use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::harness::Harness;
use crate::tools::policy::PolicySettings;
use crate::tools::session::SessionStore;
use crate::tools::workspace::{relative_display, Workspace};
use crate::workspace::AuthConfig;

pub struct ToolContext {
    pub workspace: Workspace,
    pub auth: AuthConfig,
    pub policy: PolicySettings,
    pub tool_profile: String,
    pub permission_mode: String,
    pub harness: Harness,
    default_cwd: Mutex<PathBuf>,
    pub sessions: Arc<SessionStore>,
    pub workspace_id: String,
    pub external_mcp: Option<crate::external_mcp::SharedExternalMcpManager>,
    pub turn_budget: Arc<crate::mcp::turn_budget::AgentTurnBudgetManager>,
    pub turn_registry: Arc<crate::mcp::BrowserTurnRegistry>,
    pub turn_correlator: Arc<crate::mcp::TurnCorrelator>,
}

pub type SharedToolContext = Arc<ToolContext>;

impl ToolContext {
    pub fn new(workspace_path: PathBuf) -> Result<Self, String> {
        let workspace = Workspace::new(workspace_path).map_err(|e| e.message())?;
        let auth = AuthConfig {
            auth_type: "noauth".into(),
            ..AuthConfig::default()
        };
        Ok(Self::from_workspace_with_external_mcp(
            workspace,
            auth,
            PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            String::new(),
            None,
        ))
    }

    pub fn from_workspace(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
    ) -> Self {
        Self::from_workspace_with_external_mcp(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            String::new(),
            None,
        )
    }

    pub fn from_workspace_with_external_mcp(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        workspace_id: String,
        external_mcp: Option<crate::external_mcp::SharedExternalMcpManager>,
    ) -> Self {
        let harness_root = Harness::default_root().expect("无法初始化 Harness 数据目录");
        Self::from_workspace_with_harness_root_and_external_mcp(
            workspace,
            auth,
            policy,
            crate::tools::registry::normalize_tool_profile(&tool_profile).into(),
            permission_mode,
            harness_root,
            workspace_id,
            external_mcp,
        )
    }

    pub fn from_workspace_with_harness_root(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        harness_root: PathBuf,
    ) -> Self {
        Self::from_workspace_with_harness_root_and_external_mcp(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            harness_root,
            String::new(),
            None,
        )
    }

    pub fn from_workspace_with_harness_root_and_external_mcp(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        harness_root: PathBuf,
        workspace_id: String,
        external_mcp: Option<crate::external_mcp::SharedExternalMcpManager>,
    ) -> Self {
        let root = workspace.root().to_path_buf();
        Self {
            workspace,
            auth,
            policy,
            tool_profile: crate::tools::registry::normalize_tool_profile(&tool_profile).into(),
            permission_mode,
            harness: Harness::new(root.clone(), harness_root).expect("无法初始化 Harness"),
            default_cwd: Mutex::new(root),
            sessions: Arc::new(SessionStore::new()),
            workspace_id,
            external_mcp,
            turn_budget: Arc::new(crate::mcp::turn_budget::AgentTurnBudgetManager::new(
                crate::mcp::turn_budget::AgentTurnBudgetConfig::default(),
            )),
            turn_registry: Arc::new(crate::mcp::BrowserTurnRegistry::default()),
            turn_correlator: Arc::new(crate::mcp::TurnCorrelator::default()),
        }
    }

    pub fn with_turn_budget_manager(mut self, manager: Arc<crate::mcp::turn_budget::AgentTurnBudgetManager>) -> Self {
        self.turn_budget = manager;
        self
    }

    pub fn with_turn_registry_and_correlator(
        mut self,
        registry: Arc<crate::mcp::BrowserTurnRegistry>,
        correlator: Arc<crate::mcp::TurnCorrelator>,
    ) -> Self {
        self.turn_registry = registry;
        self.turn_correlator = correlator;
        self
    }

    pub fn for_test(workspace_path: PathBuf, harness_root: PathBuf) -> Result<Self, String> {
        let workspace = Workspace::new(workspace_path).map_err(|e| e.message())?;
        Ok(Self::from_workspace_with_harness_root(
            workspace,
            AuthConfig {
                auth_type: "noauth".into(),
                ..AuthConfig::default()
            },
            PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            harness_root,
        ))
    }

    pub fn workspace_path(&self) -> String {
        self.workspace.root_display()
    }

    pub fn default_cwd_display(&self) -> String {
        let cwd = self.default_cwd.lock().expect("cwd lock");
        relative_display(self.workspace.root(), &cwd)
    }

    pub fn set_default_cwd(&self, path: PathBuf) {
        *self.default_cwd.lock().expect("cwd lock") = path;
    }

    pub fn default_cwd_path(&self) -> PathBuf {
        self.default_cwd.lock().expect("cwd lock").clone()
    }
}
