//! The MCP module (prompt 07): `/mcp`, the OAuth discovery documents, and the personal-token routes, mounted in the composed hub.
//!
//! The server itself, its gate and its authentication are in `hub_mcp`. This is the line that puts its routes in the hub's route
//! table, so that they pass through the same layers as every other route (headers, limits, metrics, drain) and are checked by the
//! S4 route-table test. `/mcp` answers a long-lived stream, which the limits layer allows: it times out the response *head*, not
//! the body (`/api/v1/events` is the precedent). `crates/hub/tests/mcp_module.rs` proves both, and that the hub's log filter keeps
//! `rmcp` at `warn`, since rmcp logs whole requests and results at `debug` (S10).

use std::sync::Arc;

use hub_mcp::McpState;
use hub_platform::GuardedRouter;

use crate::compose::Module;

/// Mounts the MCP server.
#[derive(Debug, Clone)]
pub struct McpModule {
    state: Arc<McpState>,
}

impl McpModule {
    #[must_use]
    pub fn new(state: Arc<McpState>) -> Self {
        Self { state }
    }
}

impl Module for McpModule {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        hub_mcp::mount(router, &self.state)
    }
}
