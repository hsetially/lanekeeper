//! The MCP handler rmcp runs for each session (S4, S14).
//!
//! T1 gives it what authentication needs and nothing else: it knows who opened the session, refuses anyone else, and has no tools
//! yet (T2 adds them). One handler is made per MCP session, so the first caller it sees is the session's owner.

use std::sync::OnceLock;

use domain::UserId;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, InitializeRequestParams, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};

use super::guard::McpCaller;

/// The name the server announces.
pub const SERVER_NAME: &str = "lanekeeper";

pub(crate) struct McpHandler {
    /// Set by the first call of the session from the request's `http::request::Parts`; every later call must come from the same
    /// user. This is the second line: the HTTP layer binds the session id to its user first (`guard::bind_sessions`), and also
    /// covers the `GET` and `DELETE` that never reach a handler (D91, rmcp 3.5.0).
    bound: OnceLock<UserId>,
}

impl McpHandler {
    pub(crate) fn new() -> Self {
        Self {
            bound: OnceLock::new(),
        }
    }

    /// The caller the layer in front authenticated, and a refusal if it is not the one this session belongs to. A session id is
    /// never an authenticator.
    fn check_caller(&self, context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
        let caller = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<McpCaller>())
            .ok_or_else(|| ErrorData::invalid_request("unauthenticated", None))?;
        let owner = self.bound.get_or_init(|| caller.0.id.clone());
        if owner == &caller.0.id {
            Ok(())
        } else {
            Err(ErrorData::invalid_request(
                "this session belongs to another caller",
                None,
            ))
        }
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.check_caller(&context)?;
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.check_caller(&context)?;
        Ok(ListToolsResult::with_all_items(Vec::new()))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.check_caller(&context)?;
        Err(ErrorData::invalid_params("unknown tool", None))
    }
}
