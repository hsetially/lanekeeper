//! The blobs module (T8): `GET /api/v1/blobs/{hash}` mounted in the composed hub.
//!
//! The endpoint, the store behind it and the cache are in `hub_platform::blobs`. This is the line that puts the route in the
//! hub's route table, so that it passes through the same layers as every other route (headers, limits, metrics, drain) and is
//! checked by the S4 route-table test. The header layer lets this route's `immutable` cache header through on a 200 or 304 and
//! on nothing else.

use std::sync::Arc;

use hub_platform::GuardedRouter;
use hub_platform::blobs::endpoint::{self, BlobState};

use crate::compose::Module;

/// Mounts the blob endpoint.
#[derive(Debug, Clone)]
pub struct BlobsModule {
    state: Arc<BlobState>,
}

impl BlobsModule {
    #[must_use]
    pub fn new(state: Arc<BlobState>) -> Self {
        Self { state }
    }
}

impl Module for BlobsModule {
    fn name(&self) -> &'static str {
        "blobs"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        endpoint::mount(router, Arc::clone(&self.state))
    }
}
