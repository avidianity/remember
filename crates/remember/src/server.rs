//! The stdio MCP server one Agent spawns. The process is the Agent's identity:
//! Registration happens automatically during the handshake and is never passed
//! on tool calls (see ADR 0001).

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use remember_core::{Hub, project};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};

use crate::tools;

#[derive(Clone)]
pub struct RememberServer {
    hub: Arc<Mutex<Hub>>,
    agent: Arc<OnceLock<String>>,
    tools: Arc<Vec<Tool>>,
    project: String,
}

impl RememberServer {
    /// `cwd` locates the Project: every supported client starts the server in
    /// the project directory (MCP roots are deprecated by SEP-2577).
    pub fn new(hub: Hub, cwd: &Path) -> Self {
        RememberServer {
            hub: Arc::new(Mutex::new(hub)),
            agent: Arc::new(OnceLock::new()),
            tools: Arc::new(tools::definitions()),
            project: project::identify(cwd),
        }
    }

    /// Marks the Agent offline once its client disconnects.
    pub fn shutdown(&self) {
        if let Some(agent) = self.agent.get() {
            let _ = self.hub.lock().expect("hub lock").end(agent);
        }
    }

    fn run(&self, name: &str, args: Option<rmcp::model::JsonObject>) -> (String, bool) {
        let Some(agent) = self.agent.get() else {
            return ("error: not initialized".to_string(), true);
        };
        let mut hub = self.hub.lock().expect("hub lock");
        let result = hub
            .ensure_supported()
            .and_then(|()| hub.touch(agent))
            .and_then(|()| tools::call(&mut hub, agent, name, args));
        let (mut text, is_error) = match result {
            Ok(text) => (text, false),
            Err(e) => (format!("error: {e}"), true),
        };
        if name != "inbox"
            && let Ok(unread) = hub.unread_count(agent)
            && unread > 0
        {
            text.push_str(&format!("\ninbox: {unread} unread"));
        }
        (text, is_error)
    }
}

/// Maps the MCP client name to an Agent Kind; unknown names pass through.
pub fn agent_kind(client_name: &str) -> String {
    match client_name {
        "codex-mcp-client" => "codex".to_string(),
        other => other.to_string(),
    }
}

impl ServerHandler for RememberServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("remember", env!("CARGO_PKG_VERSION")))
            .with_instructions(tools::INSTRUCTIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        context.peer.set_peer_info(request.clone());
        let result = self.negotiate_initialize(&request)?;
        if self.agent.get().is_none() {
            let kind = agent_kind(&request.client_info.name);
            let agent = self
                .hub
                .lock()
                .expect("hub lock")
                .register(&kind, &self.project)
                .map_err(|e| McpError::internal_error(format!("registration failed: {e}"), None))?;
            let _ = self.agent.set(agent);
        }
        Ok(result)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools.as_ref().clone()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let (text, is_error) = self.run(&request.name, request.arguments);
        let content = vec![ContentBlock::text(text)];
        let result = if is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        };
        Ok(result.into())
    }
}
