//! The stdio MCP server one Agent spawns. The process is the Agent's identity:
//! Registration happens automatically on the first request and is never passed
//! on tool calls (see ADR 0001).

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use remember_core::{Hub, project};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};

use crate::{tools, wake};

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

    /// Registers this process as an Agent once. A pre-2026-07-28 client names
    /// itself in `initialize`; a newer one has no handshake and names itself in
    /// each request's `_meta`, so the first request registers it. Holding the
    /// hub lock across the check keeps concurrent first requests from
    /// registering twice.
    fn register(&self, client_name: &str) -> Result<(), McpError> {
        let hub = self.hub.lock().expect("hub lock");
        if self.agent.get().is_some() {
            return Ok(());
        }
        let agent = hub
            .register(&agent_kind(client_name), &self.project)
            .map_err(|e| McpError::internal_error(format!("registration failed: {e}"), None))?;
        if let Some(terminal) = wake::current_terminal() {
            let _ = hub.set_terminal(&agent, &terminal);
        }
        let _ = self.agent.set(agent);
        Ok(())
    }

    /// Registers a handshake-less client from the request `_meta`.
    fn register_from(&self, context: &RequestContext<RoleServer>) -> Result<(), McpError> {
        if self.agent.get().is_some() {
            return Ok(());
        }
        let name = context
            .meta
            .client_info()
            .map_or_else(|| UNKNOWN_CLIENT.to_string(), |info| info.name);
        self.register(&name)
    }

    /// Marks the Agent offline once its client disconnects.
    pub fn shutdown(&self) {
        if let Some(agent) = self.agent.get() {
            let _ = self.hub.lock().expect("hub lock").end(agent);
        }
    }

    /// Blocks until a Message is unread, the wait (capped by `MAX_WAIT`) ends,
    /// or the client cancels. Polling the shared file is how a Message from
    /// another process is noticed; it costs one indexed count every 250ms.
    /// The poll never writes: `run` marks the Agent seen once the wait ends.
    async fn wait_for_mail(&self, seconds: u64, cancel: &CancellationToken) {
        let Some(agent) = self.agent.get() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(seconds).min(MAX_WAIT);
        loop {
            {
                let hub = self.hub.lock().expect("hub lock");
                if hub.ensure_supported().is_err() {
                    return;
                }
                if hub.unread_count(agent).is_ok_and(|n| n > 0) {
                    return;
                }
            }
            if Instant::now() >= deadline {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(250)) => {}
                () = cancel.cancelled() => return,
            }
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

/// Longest `inbox` wait. Claude Code, Codex and opencode all allow a tool call
/// at least 60 seconds (opencode's 5-second MCP timeout covers only listing
/// tools); an Agent that needs longer calls `inbox` again.
const MAX_WAIT: Duration = Duration::from_secs(50);

/// Agent Kind for a handshake-less client that omits `clientInfo`.
const UNKNOWN_CLIENT: &str = "unknown";

/// How long a client may reuse the tool list. The list is fixed for the
/// binary's lifetime, but `remember update` swaps the binary under a client
/// that may cache across reconnects, so it is never reused.
const TOOLS_TTL_MS: u64 = 0;

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
        self.register(&request.client_info.name)?;
        Ok(result)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Listing tools is the first request a handshake-less client sends, so
        // the Agent shows up online as soon as it connects. A failure here is
        // retried, and reported, by the first tool call.
        let _ = self.register_from(&context);
        // Protocol 2026-07-28 requires both cache fields on every list result;
        // older clients ignore them.
        Ok(ListToolsResult::with_all_items(self.tools.as_ref().clone())
            .with_ttl_ms(TOOLS_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if let Err(e) = self.register_from(&context) {
            let text = format!("error: {}", e.message);
            return Ok(CallToolResult::error(vec![ContentBlock::text(text)]).into());
        }
        if request.name == "inbox"
            && let Ok(args) = tools::parse::<tools::InboxArgs>(request.arguments.clone())
            && args.wait > 0
        {
            self.wait_for_mail(args.wait, &context.ct).await;
        }
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
