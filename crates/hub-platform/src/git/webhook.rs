//! `POST /hooks/github` (T7, D64, S3, S4, S11, S18).
//!
//! The one HTTP route of the hub with no session. It proves itself, in this fixed order, and stops at the first failure:
//!
//! 1. **Body cap.** At most [`WebhookSettings::max_body`] bytes (1 MiB, the contract's number) are read, as raw bytes. A
//!    declared length over the cap is refused without reading.
//! 2. **HMAC.** `X-Hub-Signature-256` is `sha256=` and the HMAC-SHA256 of those raw bytes under the webhook secret (from
//!    [`SecretSource`], cached there for five minutes), compared in constant time. A missing, malformed or wrong signature is a
//!    bare 401: the answer never says which, and nothing is recorded.
//! 3. **Event type.** `push` goes on, `ping` answers 200, and every other event is acknowledged with 202 and ignored.
//! 4. **Replay guard.** The delivery id is inserted into `webhook_deliveries`; one seen before is a 202 that does nothing.
//! 5. **Strict parse.** The payload becomes a typed [`PushEvent`], or the answer is 400.
//!
//! Only then is a fetch *queued*, and the answer is 202 without waiting for it (the [`FetchWorker`](super::sync::FetchWorker)
//! fetches, then announces `GitHeadMoved`). The hub does no source-address check: the load balancer's allowlist is that control
//! (S18, prompt 09), and the HMAC is the proof of origin. The threat model says so.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use domain::RepoKind;
use http::{HeaderMap, Method, StatusCode};
use ports::SecretSource;
use subtle::{Choice, ConstantTimeEq};
use tracing::{info, warn};

use super::fetch_queue::{Enqueue, FetchQueue};
use super::metrics::{GitMetrics, WebhookOutcome};
use super::payload::{PushEvent, RepoFullName};
use crate::error::ApiError;
use crate::routes::{GuardedRouter, WEBHOOK_PATH};

/// What the contract says about the body: `api/openapi.yaml` limits it to 1 MiB.
pub const DEFAULT_MAX_BODY: usize = 1024 * 1024;
const MAX_DELIVERY_ID: usize = 128;
/// The reason on the route table.
const PUBLIC_REASON: &str =
    "no session: verified by an HMAC over the raw body and a delivery-id replay guard";

/// What the webhook accepts.
#[derive(Debug, Clone)]
pub struct WebhookSettings {
    /// The name of the webhook secret in the secret source.
    pub secret_name: String,
    /// The repositories the hub mirrors, by the name GitHub reports in `repository.full_name`.
    pub repos: Vec<(RepoKind, RepoFullName)>,
    pub max_body: usize,
}

impl WebhookSettings {
    #[must_use]
    pub fn new(secret_name: &str, repos: Vec<(RepoKind, RepoFullName)>) -> Self {
        Self {
            secret_name: secret_name.to_owned(),
            repos,
            max_body: DEFAULT_MAX_BODY,
        }
    }
}

/// Why the replay guard could not be consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryError {
    #[error("the delivery log is unavailable")]
    Unavailable,
}

/// The record of delivery ids already seen (`webhook_deliveries`).
#[async_trait]
pub trait DeliveryLog: Send + Sync + 'static {
    /// Records `id`. `true` if it was new, `false` if it had been seen.
    async fn first_time(&self, id: &str, event: &str) -> Result<bool, DeliveryError>;
}

/// An in-memory log for tests.
#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Default)]
pub struct MemoryDeliveryLog {
    seen: std::sync::Mutex<std::collections::HashSet<String>>,
}

#[cfg(any(test, feature = "fakes"))]
impl MemoryDeliveryLog {
    /// How many delivery ids are recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.lock().map_or(0, |s| s.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(any(test, feature = "fakes"))]
#[async_trait]
impl DeliveryLog for MemoryDeliveryLog {
    async fn first_time(&self, id: &str, _event: &str) -> Result<bool, DeliveryError> {
        self.seen
            .lock()
            .map(|mut s| s.insert(id.to_owned()))
            .map_err(|_| DeliveryError::Unavailable)
    }
}

/// Everything the handler needs.
pub struct WebhookState {
    settings: WebhookSettings,
    secrets: Arc<dyn SecretSource>,
    deliveries: Arc<dyn DeliveryLog>,
    queue: Arc<FetchQueue>,
    metrics: GitMetrics,
}

impl std::fmt::Debug for WebhookState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookState").finish_non_exhaustive()
    }
}

impl WebhookState {
    #[must_use]
    pub fn new(
        settings: WebhookSettings,
        secrets: Arc<dyn SecretSource>,
        deliveries: Arc<dyn DeliveryLog>,
        queue: Arc<FetchQueue>,
        metrics: GitMetrics,
    ) -> Self {
        Self {
            settings,
            secrets,
            deliveries,
            queue,
            metrics,
        }
    }
}

/// Register `POST /hooks/github`, public by design, with the reason on the route table.
#[must_use]
pub fn mount(router: GuardedRouter, state: Arc<WebhookState>) -> GuardedRouter {
    router.public(
        &Method::POST,
        WEBHOOK_PATH,
        PUBLIC_REASON,
        move |headers: HeaderMap, body: Body| {
            let state = Arc::clone(&state);
            async move { receive(&state, &headers, body).await }
        },
    )
}

/// The 32 bytes of a `sha256=<64 hex>` header, and whether it was well formed. A malformed header gives zeros and `false`, so the
/// comparison below runs the same way whatever arrived.
fn parse_signature(header: Option<&str>) -> ([u8; 32], bool) {
    let mut digest = [0_u8; 32];
    let parsed = header
        .and_then(|h| h.strip_prefix("sha256="))
        .is_some_and(|hex_part| hex::decode_to_slice(hex_part, &mut digest).is_ok());
    if parsed {
        (digest, true)
    } else {
        ([0_u8; 32], false)
    }
}

/// Whether `header` is the HMAC-SHA256 of `body` under `secret`. The comparison is `subtle`'s, so its time does not depend on
/// where the two digests first differ.
#[must_use]
pub fn verify_signature(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret);
    let expected = aws_lc_rs::hmac::sign(&key, body);
    let (provided, well_formed) = parse_signature(header);
    let same = expected.as_ref().ct_eq(&provided[..]);
    bool::from(same & Choice::from(u8::from(well_formed)))
}

fn text<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn is_delivery_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_DELIVERY_ID
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

fn refuse(state: &WebhookState, outcome: WebhookOutcome, error: ApiError) -> Response {
    state.metrics.webhook(outcome);
    error.into_response()
}

fn accept(state: &WebhookState, outcome: WebhookOutcome) -> Response {
    state.metrics.webhook(outcome);
    StatusCode::ACCEPTED.into_response()
}

async fn receive(state: &WebhookState, headers: &HeaderMap, body: Body) -> Response {
    // 1. The body cap, before anything is read.
    let declared = text(headers, "content-length").and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > state.settings.max_body) {
        return refuse(state, WebhookOutcome::TooLarge, ApiError::PayloadTooLarge);
    }
    let Ok(bytes) = axum::body::to_bytes(body, state.settings.max_body).await else {
        return refuse(state, WebhookOutcome::TooLarge, ApiError::PayloadTooLarge);
    };

    // 2. The HMAC over the exact bytes.
    let Ok(secret) = state.secrets.get(&state.settings.secret_name).await else {
        warn!(
            code = "webhook_secret_unavailable",
            "the webhook secret could not be read"
        );
        return refuse(state, WebhookOutcome::Unavailable, ApiError::Unavailable);
    };
    if !verify_signature(
        secret.expose().as_bytes(),
        &bytes,
        text(headers, "x-hub-signature-256"),
    ) {
        return refuse(state, WebhookOutcome::BadSignature, ApiError::Unauthorized);
    }

    // 3. The event type.
    let Some(event) = text(headers, "x-github-event") else {
        return refuse(state, WebhookOutcome::BadRequest, ApiError::BadRequest);
    };
    match event {
        "ping" => {
            state.metrics.webhook(WebhookOutcome::Ignored);
            return StatusCode::OK.into_response();
        }
        "push" => {}
        _ => return accept(state, WebhookOutcome::Ignored),
    }

    // 4. The replay guard.
    let Some(delivery) = text(headers, "x-github-delivery").filter(|d| is_delivery_id(d)) else {
        return refuse(state, WebhookOutcome::BadRequest, ApiError::BadRequest);
    };
    match state.deliveries.first_time(delivery, event).await {
        Ok(true) => {}
        Ok(false) => return accept(state, WebhookOutcome::Duplicate),
        Err(_) => {
            warn!(
                code = "webhook_delivery_log_unavailable",
                "the delivery log could not be written"
            );
            return refuse(state, WebhookOutcome::Unavailable, ApiError::Unavailable);
        }
    }

    // 5. The strict parse, then the queue.
    let Ok(push) = PushEvent::parse(&bytes) else {
        return refuse(state, WebhookOutcome::BadRequest, ApiError::BadRequest);
    };
    let Some(repo) = state
        .settings
        .repos
        .iter()
        .find(|(_, name)| name.matches(&push.repository))
        .map(|(kind, _)| *kind)
    else {
        return accept(state, WebhookOutcome::NotApplicable);
    };
    if push.deleted {
        return accept(state, WebhookOutcome::NotApplicable);
    }
    match state.queue.push(repo, push.target) {
        Enqueue::Queued => {
            info!(
                code = "webhook_push_queued",
                repo = repo.as_str(),
                "a push was queued for fetching"
            );
            accept(state, WebhookOutcome::Queued)
        }
        Enqueue::Coalesced => accept(state, WebhookOutcome::Coalesced),
        Enqueue::Dropped => {
            state.metrics.queue_dropped();
            warn!(
                code = "webhook_queue_full",
                repo = repo.as_str(),
                "the fetch queue is full; the poll will catch this push"
            );
            accept(state, WebhookOutcome::Dropped)
        }
    }
}
