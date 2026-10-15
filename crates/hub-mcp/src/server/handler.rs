//! The MCP handler rmcp runs for each session (S4, S11, S14).
//!
//! One handler is made per MCP session. It knows who opened the session and refuses anyone else (a session id is never an
//! authenticator), lists the tools the caller's role may call, and runs `tools/call` through one pipeline, in this order:
//!
//! 1. the tool must exist, and `notify_services` must be switched on (a hidden tool is `forbidden`, as the contract says);
//! 2. **the role**: `Users::require_active(user, role of the tool)` on every call, so a role change or a disabling bites on the next
//!    call, and before the arguments are read;
//! 3. the caller's rate and concurrency (`rate_limited` with `retry_after_secs`);
//! 4. the arguments, parsed into typed values (S11);
//! 5. the tool, under the timeout of its class, with the result cut to the cap and every text through the secret scan.
//!
//! A failure is a tool result with `isError: true` and the contract's `ToolError` (rule 3); only a call that cannot be routed
//! (an unknown tool) is a protocol error. One log line says what was called and how it ended: never the arguments, never the result
//! (S10).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use aws_lc_rs::rand::fill;
use domain::{Role, User, UserId};
use ports::AuthError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};

use super::guard::McpCaller;
use super::instructions::INSTRUCTIONS;
use crate::limits::{Class, Refusal};
use crate::output::envelope::Rendered;
use crate::tools::{CallCx, Code, Listed, ToolError, ToolHost, ToolName, parse_args};

/// The name the server announces.
pub const SERVER_NAME: &str = "lanekeeper";

pub(crate) struct McpHandler {
    host: Arc<ToolHost>,
    listing: Arc<Vec<Listed>>,
    /// Set by the first call of the session from the request's `http::request::Parts`; every later call must come from the same
    /// user. This is the second line: the HTTP layer binds the session id to its user first (`guard::bind_sessions`), and also
    /// covers the `GET` and `DELETE` that never reach a handler (D91, rmcp 3.5.0).
    bound: OnceLock<UserId>,
    /// Whether this session has answered `initialize`. The factory makes one handler per session, so this is per session.
    initialized: AtomicBool,
}

fn request_id() -> String {
    let mut bytes = [0_u8; 16];
    if fill(&mut bytes).is_err() {
        // The random source failing is not a reason to fail the call; the id is only for correlation.
        return "unknown".to_owned();
    }
    crate::output::hex(&bytes)
}

fn success(rendered: Rendered) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(rendered.content)]);
    result.structured_content = Some(rendered.structured);
    result
}

fn failure(error: &ToolError, request_id: &str) -> CallToolResult {
    let (structured, message) = error.render(request_id);
    let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
    result.structured_content = Some(structured);
    result
}

fn from_auth(error: AuthError) -> ToolError {
    match error {
        AuthError::Unavailable => ToolError::new(Code::Unavailable),
        _ => ToolError::new(Code::Forbidden),
    }
}

impl McpHandler {
    pub(crate) fn new(host: Arc<ToolHost>, listing: Arc<Vec<Listed>>) -> Self {
        Self {
            host,
            listing,
            bound: OnceLock::new(),
            initialized: AtomicBool::new(false),
        }
    }

    /// The caller the layer in front authenticated, and a refusal if it is not the one this session belongs to. A session id is
    /// never an authenticator.
    fn check_caller(&self, context: &RequestContext<RoleServer>) -> Result<User, ErrorData> {
        let caller = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<McpCaller>())
            .ok_or_else(|| ErrorData::invalid_request("unauthenticated", None))?;
        let owner = self.bound.get_or_init(|| caller.0.id.clone());
        if owner == &caller.0.id {
            Ok(caller.0.clone())
        } else {
            Err(ErrorData::invalid_request(
                "this session belongs to another caller",
                None,
            ))
        }
    }

    async fn call(
        &self,
        caller: &User,
        tool: ToolName,
        request: CallToolRequestParams,
        context: &RequestContext<RoleServer>,
        request_id: &str,
    ) -> Result<Rendered, ToolError> {
        let host = &*self.host;
        let spec = host
            .contract
            .tool(tool.as_str())
            .ok_or_else(|| ToolError::new(Code::Internal))?;
        // 1 and 2. The tool may be called at all, and by this user as the directory has them now.
        if tool == ToolName::NotifyServices && !host.tool_settings.notify_services_enabled {
            return Err(ToolError::new(Code::Forbidden));
        }
        let user = host
            .users
            .require_active(&caller.id, spec.role())
            .await
            .map_err(from_auth)?;
        // 3. The caller's budget.
        let class = ToolHost::class_of(spec);
        let permit = host.limits.begin(&user.id, class).map_err(|refusal| {
            if matches!(refusal, Refusal::RateLimited { .. }) {
                host.metrics.rate_limited(match class {
                    Class::Read => "read",
                    Class::Write => "write",
                });
            }
            ToolError::from(refusal)
        })?;
        // 4. The arguments.
        let args = parse_args(tool, request.arguments)?;
        // 5. The tool.
        let session = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.headers.get(super::guard::SESSION_HEADER))
            .and_then(|v| v.to_str().ok());
        let rpc_id = context.id.to_string();
        let cx = CallCx {
            host,
            user: &user,
            spec,
            request_id,
            session_id: session,
            rpc_id: Some(rpc_id.as_str()),
        };
        let timeout = host.limits.timeout(class);
        let outcome = tokio::select! {
            result = tokio::time::timeout(timeout, host.run(&cx, args)) => {
                result.unwrap_or_else(|_| Err(ToolError::with_detail(Code::Unavailable, "The tool took too long. Try again, or narrow the request.")))
            }
            () = context.ct.cancelled() => Err(ToolError::with_detail(Code::Unavailable, "The call was cancelled.")),
        };
        drop(permit);
        outcome
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.check_caller(&context)?;
        // `initialize` is the first request of a session and is sent once. rmcp's own handler takes whatever a second one declares
        // as the new terms of the session (a version from the future puts it on a lifecycle where `ping` is not answered), so a
        // repeat is refused here and the session keeps what it negotiated. Found by the fuzz target `mcp_http_body`.
        if self.initialized.swap(true, Ordering::AcqRel) {
            return Err(ErrorData::invalid_request("already initialized", None));
        }
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let caller = self.check_caller(&context)?;
        // The list is what this user may call, by the record the directory has now (A16).
        let user = self
            .host
            .users
            .require_active(&caller.id, Role::Viewer)
            .await
            .map_err(|_| ErrorData::invalid_request("not allowed", None))?;
        let tools: Vec<_> = self
            .host
            .visible(&self.listing, &user)
            .into_iter()
            .cloned()
            .collect();
        self.host
            .metrics
            .set_tools_list_bytes(crate::output::cap::json_len(&tools));
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let caller = self.check_caller(&context)?;
        // A tool that does not exist cannot be routed: a protocol error, with fixed text.
        let tool = ToolName::from_name(&request.name)
            .ok_or_else(|| ErrorData::invalid_params("unknown tool", None))?;
        let id = request_id();
        let started = Instant::now();
        let outcome = self.call(&caller, tool, request, &context, &id).await;
        let elapsed = started.elapsed();
        let label = match &outcome {
            Ok(_) => "ok",
            Err(error) => error.code.as_str(),
        };
        self.host.metrics.call(tool.as_str(), label, elapsed);
        // What was called and how it ended. No argument, no result, no path, no text (S10).
        tracing::info!(
            target: "hub_mcp::call",
            tool = tool.as_str(),
            outcome = label,
            elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            request_id = %id,
            "tool call"
        );
        Ok(match outcome {
            Ok(rendered) => success(rendered),
            Err(error) => failure(&error, &id),
        }
        .into())
    }
}
