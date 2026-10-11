//! The Git module (T7): `POST /hooks/github` mounted in the composed hub.
//!
//! The handler, the replay guard and the queue are in `hub_platform::git`. This is the line that puts the route in the hub's route
//! table, so that it passes through the same layers as every other route (headers, limits, metrics, drain) and is checked by the
//! S4 route-table test. The webhook is `public` by design: it has no session and proves itself with an HMAC.

use std::sync::Arc;

use hub_platform::GuardedRouter;
use hub_platform::git::webhook::{self, WebhookState};

use crate::compose::Module;

/// Mounts the webhook.
#[derive(Debug, Clone)]
pub struct GitModule {
    state: Arc<WebhookState>,
}

impl GitModule {
    #[must_use]
    pub fn new(state: Arc<WebhookState>) -> Self {
        Self { state }
    }
}

impl Module for GitModule {
    fn name(&self) -> &'static str {
        "git"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        webhook::mount(router, Arc::clone(&self.state))
    }
}
