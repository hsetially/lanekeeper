//! Software for the tests, examples and benchmarks of the composition (feature `testing`, plan 18, T4).
//!
//! Nothing here is in a release build: the feature is switched on by this crate's own dev-dependency only, and
//! `tests/release_build.rs` fails if a production dependency switches it on, or if `lib.rs` declares this module without it.
//! The software is the smallest that lets a hub be built without Google: an identity provider that is never reachable, and a
//! checkpoint bucket in memory.

use std::sync::Arc;

use async_trait::async_trait;
use hub_identity::audit::MemoryBucket;
use hub_identity::http::{HttpsClient, HttpsError, HttpsRequest, HttpsResponse};

use crate::edges::IdentityEdges;

/// An HTTPS client that reaches nothing: every call is a connection failure.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefusingHttps;

#[async_trait]
impl HttpsClient for RefusingHttps {
    async fn send(&self, _request: HttpsRequest) -> Result<HttpsResponse, HttpsError> {
        Err(HttpsError::Connect)
    }
}

/// Identity edges with no provider behind them: sign-in cannot complete, and checkpoints go to memory.
#[must_use]
pub fn unreachable_identity() -> IdentityEdges {
    IdentityEdges {
        entra: Arc::new(RefusingHttps),
        google_keys: Arc::new(RefusingHttps),
        checkpoints: Arc::new(MemoryBucket::new()),
    }
}
