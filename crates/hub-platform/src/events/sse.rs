//! `GET /api/v1/events`: live updates for the web app over Server-Sent Events (D66, P13).
//!
//! What a client gets, in order:
//!
//! 1. `retry: <ms>`, so the browser knows how long to wait before it reconnects;
//! 2. when it sent `Last-Event-ID`, the events it missed (from the outbox, within the replay window), or a `resync` event if the log
//!    can no longer say what it missed;
//! 3. live events, one frame each, `id: <outbox id>`, `event: <kind>`, `data: <the event as JSON>`;
//! 4. a `: heartbeat` comment every 15 seconds, so that proxies keep the connection open.
//!
//! Events carry ids and hashes, never file content (S12). A frame is one `data:` line, because the JSON is compact and escapes
//! every control character.
//!
//! **Bounds (rule 5).** A user holds at most 5 streams and the replica at most 2,000. Each stream has a small outbound queue; a
//! client that stops reading for the write timeout is cut off, and one that reads slowly lags the bounded broadcast channel and
//! is sent a `resync` instead of making the bus wait. The query is bounded (64 swimlanes, 17 kinds, 8 KiB), parsed by hand into
//! typed values, and refused whole when any part is wrong (S11).
//!
//! **Authorisation (S4).** The route is registered through [`GuardedRouter`] at Viewer, so pending and disabled users never reach the
//! handler. A stream outlives the request that opened it, so on every heartbeat the handler asks [`SessionAuth::still_valid`] and
//! ends the stream when the session has ended or the role dropped below Viewer (within one heartbeat, 15 seconds). A stream also
//! ends at its maximum age (30 minutes by default), so a role change that only narrows what a user may see takes effect when the
//! client reconnects with its `Last-Event-ID`.
//!
//! **Internal events never reach a browser.** The bus carries events that exist only so that hub replicas can coordinate
//! ([`DomainEvent::is_internal`]: today `UserSessionsRevoked`, which names a user). They are global events with an id in them, so the
//! per-user filter would hand them to every Viewer. This edge drops them first, before the user's filter, on live delivery and
//! on replay, and a request cannot ask for their kind (`kinds=user_sessions_revoked` is a 400, like any name the `OpenAPI` list
//! of kinds does not have). Everything else, [`DomainEvent::Resync`] included, is delivered as before.
//!
//! The stream ends with a final `retry:` frame on shutdown and at its maximum age.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Extension;
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use domain::{DomainEvent, SwimlaneId, User, UserId};
use futures::StreamExt;
use futures::stream::BoxStream;
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use ports::{EventFilter, EventKind};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, warn};

use super::metrics::{EventMetrics, ReplayOutcome, SseEnd, SseRefusal};
use super::source::{EventSource, Relayed, Replay, wait_closed};
use super::visibility::EventVisibility;
use super::wire::EventId;
use crate::error::ApiError;
use crate::routes::{EVENTS_PATH, GuardedRouter};
use crate::session_auth::{AuthenticatedUser, SessionAuth};
use domain::Role;

/// The most swimlanes a request may name.
pub const MAX_SWIMLANES: usize = 64;
/// The most kinds a request may name. There are 17 kinds a browser can ask for (the internal kinds are not among them), and
/// `streamEvents` in `api/openapi.yaml` bounds its `kinds` list at the same number.
pub const MAX_KINDS: usize = 17;
/// The longest `Last-Event-ID` the API accepts.
const MAX_LAST_EVENT_ID: usize = 64;
/// The longest a replay query may take.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounds and timings of the endpoint.
#[derive(Debug, Clone)]
pub struct SseSettings {
    /// The time between heartbeats, and between checks that the user is still allowed to listen.
    pub heartbeat: Duration,
    /// The most events one replay may carry. A client further behind than this is told to refetch.
    pub replay_limit: usize,
    /// The streams one user may hold.
    pub per_user: usize,
    /// The streams this replica may hold.
    pub global: usize,
    /// The longest query string, in bytes.
    pub max_query_bytes: usize,
    /// The frames a stream may have waiting for its client.
    pub outbound_queue: usize,
    /// How long a write to a client may wait before the stream is cut off.
    pub write_timeout: Duration,
    /// The longest a stream lives. The client reconnects with its `Last-Event-ID` and loses nothing.
    pub max_age: Duration,
    /// What the client is told to wait before it reconnects.
    pub retry_hint: Duration,
    /// The longest a question to the identity layer may take. No answer counts as "no longer valid".
    pub auth_timeout: Duration,
}

impl Default for SseSettings {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(15),
            replay_limit: 1000,
            per_user: 5,
            global: 2000,
            max_query_bytes: 8 * 1024,
            outbound_queue: 16,
            write_timeout: Duration::from_secs(30),
            max_age: Duration::from_secs(30 * 60),
            retry_hint: Duration::from_secs(3),
            auth_timeout: Duration::from_secs(5),
        }
    }
}

/// Why a stream could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denied {
    User,
    Total,
}

#[derive(Debug, Default)]
struct Held {
    total: usize,
    per_user: HashMap<UserId, usize>,
}

/// How many streams are open, in total and per user. The table has at most `global` entries.
#[derive(Debug)]
struct ConnectionLimiter {
    held: Mutex<Held>,
    per_user: usize,
    global: usize,
}

impl ConnectionLimiter {
    fn new(per_user: usize, global: usize) -> Arc<Self> {
        Arc::new(Self {
            held: Mutex::new(Held::default()),
            per_user,
            global,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn try_acquire(self: &Arc<Self>, user: &UserId) -> Result<Permit, Denied> {
        let mut held = self.lock();
        if held.per_user.get(user).copied().unwrap_or(0) >= self.per_user {
            return Err(Denied::User);
        }
        if held.total >= self.global {
            return Err(Denied::Total);
        }
        held.total += 1;
        *held.per_user.entry(user.clone()).or_insert(0) += 1;
        Ok(Permit {
            limiter: Arc::clone(self),
            user: user.clone(),
        })
    }

    fn total(&self) -> usize {
        self.lock().total
    }
}

/// A place among the open streams, given back when dropped.
#[derive(Debug)]
struct Permit {
    limiter: Arc<ConnectionLimiter>,
    user: UserId,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut held = self.limiter.lock();
        held.total = held.total.saturating_sub(1);
        if let Some(n) = held.per_user.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                held.per_user.remove(&self.user);
            }
        }
    }
}

/// Everything the endpoint needs.
pub struct SseState {
    source: Arc<dyn EventSource>,
    auth: Arc<dyn SessionAuth>,
    visibility: Arc<dyn EventVisibility>,
    settings: SseSettings,
    closing: watch::Receiver<bool>,
    metrics: EventMetrics,
    limiter: Arc<ConnectionLimiter>,
}

impl std::fmt::Debug for SseState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseState")
            .field("streams", &self.limiter.total())
            .finish_non_exhaustive()
    }
}

impl SseState {
    /// `closing` turns true when the hub shuts down: every stream then ends with a final `retry:` frame.
    #[must_use]
    pub fn new(
        source: Arc<dyn EventSource>,
        auth: Arc<dyn SessionAuth>,
        visibility: Arc<dyn EventVisibility>,
        settings: SseSettings,
        closing: watch::Receiver<bool>,
        metrics: EventMetrics,
    ) -> Arc<Self> {
        let limiter = ConnectionLimiter::new(settings.per_user, settings.global);
        Arc::new(Self {
            source,
            auth,
            visibility,
            settings,
            closing,
            metrics,
            limiter,
        })
    }

    /// The streams open on this replica now.
    #[must_use]
    pub fn active_streams(&self) -> usize {
        self.limiter.total()
    }
}

/// Register `GET /api/v1/events` for Viewers and above.
#[must_use]
pub fn mount(router: GuardedRouter, state: Arc<SseState>) -> GuardedRouter {
    router.route(
        &Method::GET,
        EVENTS_PATH,
        Role::Viewer,
        move |Extension(user): Extension<AuthenticatedUser>, uri: Uri, headers: HeaderMap| {
            let state = Arc::clone(&state);
            async move { open(&state, user.0, &uri, &headers) }
        },
    )
}

// ------------------------------------------------------------------------------------------------- request

/// Where the client says it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// A first connection.
    Fresh,
    From(EventId),
    /// A `Last-Event-ID` that is not an id this hub ever sent.
    Unknown,
}

fn last_event_id(headers: &HeaderMap) -> Result<Resume, ()> {
    let Some(value) = headers.get("last-event-id") else {
        return Ok(Resume::Fresh);
    };
    if value.len() > MAX_LAST_EVENT_ID {
        return Err(());
    }
    let text = value.to_str().map_err(|_| ())?;
    Ok(EventId::parse(text).map_or(Resume::Unknown, Resume::From))
}

/// The filter a query string asks for, or `Err` if any part of it is not valid. Swimlane ids and kind names are made of
/// `[a-z0-9-]` and `[a-z_]`, so nothing needs percent-decoding: an escape is simply a bad character.
fn parse_query(query: Option<&str>, max_bytes: usize) -> Result<EventFilter, ()> {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return Ok(EventFilter::all());
    };
    if query.len() > max_bytes {
        return Err(());
    }
    let mut swimlanes: BTreeSet<SwimlaneId> = BTreeSet::new();
    let mut kinds: BTreeSet<EventKind> = BTreeSet::new();
    let (mut swimlane_count, mut kind_count) = (0_usize, 0_usize);
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or(())?;
        match key {
            "swimlane" => {
                swimlane_count += 1;
                if swimlane_count > MAX_SWIMLANES {
                    return Err(());
                }
                swimlanes.insert(SwimlaneId::parse(value).map_err(|_| ())?);
            }
            "kinds" => {
                kind_count += 1;
                if kind_count > MAX_KINDS {
                    return Err(());
                }
                kinds.insert(parse_kind(value)?);
            }
            _ => return Err(()),
        }
    }
    Ok(EventFilter {
        swimlanes: (!swimlanes.is_empty()).then_some(swimlanes),
        kinds,
    })
}

/// Whether a kind is internal to the hub (the kind of an event for which [`DomainEvent::is_internal`] is true). The match has no
/// wildcard, so a kind added to the contract fails to compile here until someone has decided which side of the edge it is on.
fn is_internal_kind(kind: EventKind) -> bool {
    match kind {
        EventKind::UserSessionsRevoked => true,
        EventKind::FileObserved
        | EventKind::DriftChanged
        | EventKind::FindingsChanged
        | EventKind::PendingRestartChanged
        | EventKind::ProposalChanged
        | EventKind::AgentStatusChanged
        | EventKind::GitHeadMoved
        | EventKind::AccessRequestCreated
        | EventKind::DocIndexed
        | EventKind::Resync
        | EventKind::AttributionUpdated
        | EventKind::PrStateChanged
        | EventKind::PrJobProgress
        | EventKind::SyncWindowOpened
        | EventKind::SyncWindowClosed
        | EventKind::PickupChanged
        | EventKind::SwimlaneChanged => false,
    }
}

/// A kind a browser may ask for. An internal kind is refused whole, and is never dropped from the list: an empty list of kinds means
/// every kind, so a filter that was emptied by dropping it would widen the stream.
fn parse_kind(name: &str) -> Result<EventKind, ()> {
    if name.is_empty() || name.len() > 32 || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Err(());
    }
    let kind: EventKind =
        serde_json::from_value(serde_json::Value::String(name.to_owned())).map_err(|_| ())?;
    if is_internal_kind(kind) {
        return Err(());
    }
    Ok(kind)
}

fn refuse(state: &SseState, reason: SseRefusal, error: ApiError) -> Response {
    state.metrics.sse_refused(reason);
    error.into_response()
}

/// Answer the request: refuse it, or start a stream and return its body.
fn open(state: &Arc<SseState>, user: User, uri: &Uri, headers: &HeaderMap) -> Response {
    let Ok(filter) = parse_query(uri.query(), state.settings.max_query_bytes) else {
        return refuse(state, SseRefusal::BadRequest, ApiError::BadRequest);
    };
    let Ok(resume) = last_event_id(headers) else {
        return refuse(state, SseRefusal::BadRequest, ApiError::BadRequest);
    };
    let permit = match state.limiter.try_acquire(&user.id) {
        Ok(permit) => permit,
        Err(Denied::User) => {
            return refuse(state, SseRefusal::TooManyForUser, ApiError::TooManyRequests);
        }
        Err(Denied::Total) => {
            return refuse(state, SseRefusal::TooManyTotal, ApiError::TooManyRequests);
        }
    };
    // Subscribe before anything is replayed, so that no event falls between the replay and the live stream.
    let live = state.source.subscribe(filter.clone());
    let (tx, rx) = mpsc::channel::<Bytes>(state.settings.outbound_queue.max(1));
    state.metrics.sse_started();
    let metrics = state.metrics.clone();
    let stream = Stream {
        state: Arc::clone(state),
        user,
        filter,
        resume,
        out: Out::new(tx),
    };
    tokio::spawn(async move {
        let reason = stream.run(live).await;
        // The permit is given back, and the count dropped, only when the stream is over.
        drop(permit);
        metrics.sse_ended(reason);
    });

    let body = Body::from_stream(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|frame| (Ok::<Bytes, Infallible>(frame), rx))
    }));
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // A reverse proxy that buffers a stream delivers nothing until it fills.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

// --------------------------------------------------------------------------------------------------- frames

/// `retry: <ms>`: how long the client waits before it reconnects.
fn retry_frame(hint: Duration) -> Bytes {
    Bytes::from(format!("retry: {}\n\n", hint.as_millis()))
}

fn heartbeat_frame() -> Bytes {
    Bytes::from_static(b": heartbeat\n\n")
}

/// The wire name of an event's kind, which is also its SSE event name.
fn kind_name(event: &DomainEvent) -> Option<String> {
    match serde_json::to_value(EventKind::of(event)) {
        Ok(serde_json::Value::String(name)) => Some(name),
        _ => None,
    }
}

/// One event as a frame. `None` only if the event cannot be serialised, which no domain event can.
fn event_frame(id: Option<EventId>, event: &DomainEvent) -> Option<Bytes> {
    let name = kind_name(event)?;
    let data = serde_json::to_string(event).ok()?;
    let mut frame = String::with_capacity(data.len() + name.len() + 32);
    if let Some(id) = id {
        frame.push_str("id: ");
        frame.push_str(&id.to_string());
        frame.push('\n');
    }
    frame.push_str("event: ");
    frame.push_str(&name);
    frame.push_str("\ndata: ");
    frame.push_str(&data);
    frame.push_str("\n\n");
    Some(Bytes::from(frame))
}

// ---------------------------------------------------------------------------------------------------- stream

/// The writing end of a stream: a bounded queue to the response body, with a bound on how long a write may wait.
struct Out {
    tx: mpsc::Sender<Bytes>,
}

impl Out {
    fn new(tx: mpsc::Sender<Bytes>) -> Self {
        Self { tx }
    }

    async fn send(&self, frame: Bytes, timeout: Duration) -> Result<(), SseEnd> {
        match tokio::time::timeout(timeout, self.tx.send(frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(SseEnd::ClientGone),
            Err(_) => Err(SseEnd::SlowClient),
        }
    }
}

struct Stream {
    state: Arc<SseState>,
    user: User,
    filter: EventFilter,
    resume: Resume,
    out: Out,
}

impl Stream {
    /// Run the stream until it ends, and say why.
    async fn run(self, mut live: BoxStream<'static, Arc<Relayed>>) -> SseEnd {
        let mut closing = self.state.closing.clone();
        let reason = match self.serve(&mut live, &mut closing).await {
            Ok(reason) | Err(reason) => reason,
        };
        // A clean end of a stream that the hub or the age limit closes carries a final hint to come back.
        if matches!(reason, SseEnd::Shutdown | SseEnd::MaxAge) {
            let hint = retry_frame(self.state.settings.retry_hint);
            let _ = self.out.send(hint, self.state.settings.write_timeout).await;
        }
        debug!(reason = ?reason, "event stream ended");
        reason
    }

    async fn write(&self, frame: Bytes) -> Result<(), SseEnd> {
        self.out.send(frame, self.state.settings.write_timeout).await
    }

    /// Whether this user's stream carries `event`. Internal events come first and are never delivered, whoever the user is;
    /// a [`DomainEvent::Resync`] always is; the rest follow the visibility rule.
    fn deliverable(&self, event: &DomainEvent) -> bool {
        if event.is_internal() {
            return false;
        }
        matches!(event, DomainEvent::Resync) || self.state.visibility.visible(&self.user, event)
    }

    async fn write_event(&self, id: Option<EventId>, event: &DomainEvent) -> Result<(), SseEnd> {
        match event_frame(id, event) {
            Some(frame) => self.write(frame).await,
            None => Ok(()),
        }
    }

    async fn write_resync(&self) -> Result<(), SseEnd> {
        self.write_event(None, &DomainEvent::Resync).await
    }

    /// The events the client missed. Returns the ids sent, so that the live stream does not send them again.
    async fn replay(&self) -> Result<HashSet<EventId>, SseEnd> {
        let mut sent = HashSet::new();
        let after = match self.resume {
            Resume::Fresh => return Ok(sent),
            Resume::Unknown => {
                self.state.metrics.replay(ReplayOutcome::Resync);
                self.write_resync().await?;
                return Ok(sent);
            }
            Resume::From(id) => id,
        };
        let replay = tokio::time::timeout(
            REPLAY_TIMEOUT,
            self.state
                .source
                .replay(after, &self.filter, self.state.settings.replay_limit),
        )
        .await;
        match replay {
            Ok(Ok(Replay::Events(events))) => {
                self.state.metrics.replay(ReplayOutcome::Events);
                for (id, event) in events {
                    if self.deliverable(&event) {
                        self.write_event(Some(id), &event).await?;
                    }
                    sent.insert(id);
                }
            }
            Ok(Ok(Replay::Resync)) => {
                self.state.metrics.replay(ReplayOutcome::Resync);
                self.write_resync().await?;
            }
            Ok(Err(_)) | Err(_) => {
                // The log could not be read: the client cannot be brought up to date from it, so it refetches.
                warn!("event replay failed");
                self.state.metrics.replay(ReplayOutcome::Failed);
                self.write_resync().await?;
            }
        }
        Ok(sent)
    }

    /// Everything between the first frame and the end. `Ok` is a clean end, `Err` one forced by the client.
    async fn serve(
        &self,
        live: &mut BoxStream<'static, Arc<Relayed>>,
        closing: &mut watch::Receiver<bool>,
    ) -> Result<SseEnd, SseEnd> {
        let settings = self.state.settings.clone();
        let started = Instant::now();
        self.write(retry_frame(settings.retry_hint)).await?;
        let replayed = self.replay().await?;

        let mut heartbeat = tokio::time::interval_at(started + settings.heartbeat, settings.heartbeat);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let max_age = tokio::time::sleep_until(started + settings.max_age);
        tokio::pin!(max_age);

        loop {
            tokio::select! {
                biased;
                // The hub is shutting down (or is gone).
                () = wait_closed(closing) => return Ok(SseEnd::Shutdown),
                () = self.out.tx.closed() => return Ok(SseEnd::ClientGone),
                () = &mut max_age => return Ok(SseEnd::MaxAge),
                _ = heartbeat.tick() => {
                    if !self.still_valid().await {
                        return Ok(SseEnd::Revoked);
                    }
                    self.write(heartbeat_frame()).await?;
                }
                item = live.next() => {
                    let Some(item) = item else { return Ok(SseEnd::BusClosed) };
                    if item.id.is_some_and(|id| replayed.contains(&id)) {
                        continue;
                    }
                    if self.deliverable(&item.event) {
                        self.write_event(item.id, &item.event).await?;
                    }
                }
            }
        }
    }

    /// Whether the user may still listen. No answer in time counts as no: a stream stays open only on a positive answer.
    async fn still_valid(&self) -> bool {
        tokio::time::timeout(
            self.state.settings.auth_timeout,
            self.state.auth.still_valid(&self.user.id),
        )
        .await
        .unwrap_or(false)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const KINDS: [&str; 17] = [
        "file_observed",
        "drift_changed",
        "findings_changed",
        "pending_restart_changed",
        "proposal_changed",
        "agent_status_changed",
        "git_head_moved",
        "access_request_created",
        "doc_indexed",
        "resync",
        "attribution_updated",
        "pr_state_changed",
        "pr_job_progress",
        "sync_window_opened",
        "sync_window_closed",
        "pickup_changed",
        "swimlane_changed",
    ];

    #[test]
    fn every_kind_name_parses_and_names_itself_back() {
        for name in KINDS {
            let kind = parse_kind(name).unwrap();
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(name.to_owned())
            );
        }
    }

    #[test]
    fn an_internal_kind_cannot_be_asked_for_and_agrees_with_the_event() {
        assert!(parse_kind("user_sessions_revoked").is_err());
        let event = DomainEvent::UserSessionsRevoked {
            user: crate::session_auth::user_id("someone"),
        };
        assert!(event.is_internal());
        assert!(is_internal_kind(EventKind::of(&event)));
        for name in KINDS {
            assert!(!is_internal_kind(parse_kind(name).unwrap()), "{name}");
        }
    }

    proptest! {
        /// A query is untrusted text: whatever it holds, parsing it never panics, and what is accepted is within bounds.
        #[test]
        fn an_accepted_query_is_within_the_bounds(query in "[a-z0-9=&_%+.-]{0,300}") {
            if let Ok(filter) = parse_query(Some(&query), 8192) {
                prop_assert!(filter.swimlanes.as_ref().is_none_or(|s| s.len() <= MAX_SWIMLANES && !s.is_empty()));
                prop_assert!(filter.kinds.len() <= MAX_KINDS);
            }
        }

        #[test]
        fn any_text_at_all_is_handled(query in ".{0,600}", max in 0usize..400) {
            let parsed = parse_query(Some(&query), max);
            if query.len() > max && !query.is_empty() {
                prop_assert!(parsed.is_err());
            }
        }

        #[test]
        fn a_valid_query_round_trips(
            lanes in prop::collection::btree_set("[a-z0-9][a-z0-9-]{0,62}", 0..=MAX_SWIMLANES),
            kinds in prop::collection::btree_set(prop::sample::select(KINDS.to_vec()), 0..=MAX_KINDS),
        ) {
            let mut pairs: Vec<String> = lanes.iter().map(|l| format!("swimlane={l}")).collect();
            pairs.extend(kinds.iter().map(|k| format!("kinds={k}")));
            let query = pairs.join("&");
            let filter = parse_query(Some(&query), 1 << 20).unwrap();
            let got: BTreeSet<String> = filter
                .swimlanes
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect();
            prop_assert_eq!(got, lanes);
            let got_kinds: BTreeSet<String> = filter
                .kinds
                .iter()
                .map(|k| match serde_json::to_value(k).unwrap() {
                    serde_json::Value::String(s) => s,
                    other => panic!("{other}"),
                })
                .collect();
            let want: BTreeSet<String> = kinds.iter().map(|k| (*k).to_owned()).collect();
            prop_assert_eq!(got_kinds, want);
        }

        #[test]
        fn last_event_id_is_a_number_or_unknown_and_never_longer_than_64(text in "[ -~]{0,100}") {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&text) {
                headers.insert("last-event-id", value);
                match last_event_id(&headers) {
                    Err(()) => prop_assert!(text.len() > MAX_LAST_EVENT_ID),
                    Ok(Resume::From(id)) => prop_assert_eq!(id.to_string(), text),
                    Ok(_) => {}
                }
            }
        }
    }
}
