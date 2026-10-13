//! `POST /hooks/github` (T7, D64, S3, S4, S11, S18).
//!
//! The one HTTP route of the hub with no session. It proves itself, in this fixed order, and stops at the first failure:
//!
//! 1. **Body cap.** At most [`WebhookSettings::max_body`] bytes (1 MiB, the contract's number) are read, as raw bytes. A
//!    declared length over the cap is refused without reading.
//! 2. **HMAC.** `X-Hub-Signature-256` is `sha256=` and the HMAC-SHA256 of those raw bytes under the webhook secret (from
//!    [`SecretSource`], cached there for five minutes), compared in constant time. A missing, malformed or wrong signature is a
//!    bare 401: the answer never says which, and nothing is recorded.
//! 3. **Event type.** `push` goes on, `pull_request` goes on when a [`PrLinks`] is configured, `ping` answers 200, and every
//!    other event is acknowledged with 202 and ignored.
//! 4. **Replay guard.** The delivery's key is inserted into `webhook_deliveries`; one seen before is a 202 that does nothing. The
//!    key is the HMAC-SHA256 of the body that step 2 just verified (see [`replay_key`]), not `X-GitHub-Delivery`: that header is
//!    not covered by the signature, so whoever holds one captured signed body could send it again under any id. GitHub's own
//!    redelivery carries the same body, so it has the same key. The header is still required (and checked for form), as GitHub
//!    always sends it.
//! 5. **Strict parse.** The payload becomes a typed [`PushEvent`] or [`PullRequestEvent`], or the answer is 400.
//!
//! For a push, a fetch is *queued*, and the answer is 202 without waiting for it (the [`FetchWorker`](super::sync::FetchWorker)
//! fetches, then announces `GitHeadMoved`). For a pull request, the link is updated (with its audit event and its
//! `PrStateChanged`, in one transaction, [`super::pr_webhook`]) before the answer; if that fails the answer is 503 and the delivery
//! key is forgotten, so that GitHub's redelivery of the same delivery is applied and not taken for a replay.
//!
//! The hub does no source-address check: the load balancer's allowlist is that control (S18, prompt 09), and the HMAC is the proof
//! of origin. The threat model says so.

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
use super::payload::{PullRequestEvent, PushEvent, RepoFullName};
use super::pr_webhook::{PrLinks, PrOutcome};
use crate::error::ApiError;
use crate::routes::{GuardedRouter, WEBHOOK_PATH};

/// What the contract says about the body: `api/openapi.yaml` limits it to 1 MiB.
pub const DEFAULT_MAX_BODY: usize = 1024 * 1024;
const MAX_DELIVERY_ID: usize = 128;
/// The reason on the route table.
const PUBLIC_REASON: &str =
    "no session: verified by an HMAC over the raw body and a replay guard keyed on that HMAC";

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

/// The record of deliveries already seen (`webhook_deliveries`). The "id" is the delivery's [`replay_key`].
#[async_trait]
pub trait DeliveryLog: Send + Sync + 'static {
    /// Records `id`. `true` if it was new, `false` if it had been seen.
    async fn first_time(&self, id: &str, event: &str) -> Result<bool, DeliveryError>;

    /// Takes `id` back out, for a delivery that was recorded and then could not be applied, so that its redelivery is not
    /// mistaken for a replay.
    async fn forget(&self, id: &str) -> Result<(), DeliveryError>;
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

    async fn forget(&self, id: &str) -> Result<(), DeliveryError> {
        self.seen
            .lock()
            .map(|mut s| {
                s.remove(id);
            })
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
    pull_requests: Option<Arc<PrLinks>>,
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
            pull_requests: None,
        }
    }

    /// Act on `pull_request` events with `links` (T11). Without it they are acknowledged and ignored, as every other event is.
    #[must_use]
    pub fn with_pull_requests(mut self, links: Arc<PrLinks>) -> Self {
        self.pull_requests = Some(links);
        self
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

/// The HMAC-SHA256 of `body` under `secret`: what a genuine `X-Hub-Signature-256` holds.
fn body_digest(secret: &[u8], body: &[u8]) -> [u8; 32] {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret);
    let tag = aws_lc_rs::hmac::sign(&key, body);
    let mut digest = [0_u8; 32];
    // SHA-256's tag is 32 bytes; a tag of another length would leave `digest` zeroed, which no header can match.
    if tag.as_ref().len() == digest.len() {
        digest.copy_from_slice(tag.as_ref());
    }
    digest
}

/// Whether `header` holds `expected`. The comparison is `subtle`'s, so its time does not depend on where the two digests first
/// differ.
fn digest_matches(expected: &[u8; 32], header: Option<&str>) -> bool {
    let (provided, well_formed) = parse_signature(header);
    let same = expected[..].ct_eq(&provided[..]);
    bool::from(same & Choice::from(u8::from(well_formed)))
}

/// Whether `header` is the HMAC-SHA256 of `body` under `secret`.
#[must_use]
pub fn verify_signature(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    digest_matches(&body_digest(secret, body), header)
}

/// The replay-guard key of a body whose signature has been verified: the HMAC-SHA256 of the body, in hex, behind a label (76
/// characters, inside the `webhook_deliveries.delivery_id` limit of 128). Everything the sender chooses and the signature does not
/// cover (the delivery header) is left out, so a captured signed body cannot be made to look new. The digest is what GitHub put
/// in a header already, and an HMAC under a secret the table's readers do not have, so storing it gives nothing away.
fn replay_key(digest: &[u8; 32]) -> String {
    format!("hmac-sha256:{}", hex::encode(digest))
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
    let digest = body_digest(secret.expose().as_bytes(), &bytes);
    if !digest_matches(&digest, text(headers, "x-hub-signature-256")) {
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
        "pull_request" if state.pull_requests.is_some() => {}
        _ => return accept(state, WebhookOutcome::Ignored),
    }

    // 4. The replay guard.
    if !text(headers, "x-github-delivery").is_some_and(is_delivery_id) {
        return refuse(state, WebhookOutcome::BadRequest, ApiError::BadRequest);
    }
    let delivery = replay_key(&digest);
    match state.deliveries.first_time(&delivery, event).await {
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

    // 5. The strict parse, then what the event is for.
    if event == "pull_request" {
        return pull_request(state, &delivery, &bytes).await;
    }
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

/// A `pull_request` delivery that passed the signature and the replay guard, recorded under `delivery` (its [`replay_key`]).
async fn pull_request(state: &WebhookState, delivery: &str, body: &[u8]) -> Response {
    let Some(links) = state.pull_requests.as_ref() else {
        return accept(state, WebhookOutcome::Ignored);
    };
    let Ok(event) = PullRequestEvent::parse(body) else {
        return refuse(state, WebhookOutcome::BadRequest, ApiError::BadRequest);
    };
    let Some(repo) = state
        .settings
        .repos
        .iter()
        .find(|(_, name)| name.matches(&event.repository))
        .map(|(kind, _)| *kind)
    else {
        return accept(state, WebhookOutcome::NotApplicable);
    };
    match links.apply(repo, &event).await {
        Ok(PrOutcome::Applied { .. }) => {
            info!(
                code = "webhook_pull_request_applied",
                repo = repo.as_str(),
                "a pull request event moved a link"
            );
            accept(state, WebhookOutcome::PrApplied)
        }
        Ok(PrOutcome::UnknownPr) => accept(state, WebhookOutcome::NotApplicable),
        Ok(PrOutcome::Ignored | PrOutcome::Unchanged) => accept(state, WebhookOutcome::Ignored),
        Err(_) => {
            // Nothing was written, and the key must not stay recorded: GitHub's redelivery carries the same body.
            if state.deliveries.forget(delivery).await.is_err() {
                warn!(
                    code = "webhook_delivery_forget_failed",
                    "a delivery could not be released after a failed update; a redelivery will look like a replay"
                );
            }
            refuse(state, WebhookOutcome::Unavailable, ApiError::Unavailable)
        }
    }
}
