//! The events module (T5): `GET /api/v1/events` mounted in the composed hub.
//!
//! The endpoint itself, and the bus behind it, are in `hub_platform::events`. This is the line that puts the route in the hub's
//! route table, so that it passes through the same layers as every other route (headers, limits, metrics, drain) and is checked
//! by the S4 route-table test.

use std::sync::Arc;

use hub_platform::GuardedRouter;
use hub_platform::events::sse::{self, SseState};

use crate::compose::Module;

/// Mounts the SSE endpoint.
#[derive(Debug, Clone)]
pub struct EventsModule {
    state: Arc<SseState>,
}

impl EventsModule {
    #[must_use]
    pub fn new(state: Arc<SseState>) -> Self {
        Self { state }
    }
}

impl Module for EventsModule {
    fn name(&self) -> &'static str {
        "events"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        sse::mount(router, Arc::clone(&self.state))
    }
}
