//! `/healthz`, `/readyz` and `/metrics` (T7, S16): what Kubernetes probes and Prometheus scrapes.
//!
//! - **`/healthz`** (liveness) says whether the agent's loops are alive. A loop is alive while its task has not ended
//!   ([`Health::task`]) and, for the loops that must keep moving, while it has made progress within its allowed age
//!   ([`Health::progress`]). A failing probe makes the kubelet restart the container, so the limits are generous: a hub
//!   that is down is not a reason to restart the agent, and neither is a slow walk.
//! - **`/readyz`** (readiness) says whether the agent is connected to the hub: its configuration has arrived and the
//!   stream is up. It is not ready while it joins, while it reconnects, and from the moment shutdown begins.
//! - **`/metrics`** is the Prometheus text format of [`Metrics`].
//!
//! The server answers `GET` and `HEAD` on those three paths and nothing else. Answers are a few bytes and name no
//! cause beyond the loop's fixed name. Every connection carries one request and has a short deadline, and at most
//! [`MAX_CONNECTIONS`] are open at once, so a slow client cannot hold the port (rule 5).

use std::convert::Infallible;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::time::{Instant, timeout};
use tracing::{debug, warn};

use super::metrics::Metrics;
use crate::clock::Clock;
use crate::transport::session::ConnectionState;

/// Open connections at once. Probes and one scraper need a handful.
pub const MAX_CONNECTIONS: usize = 16;

/// How much of the port one client may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerLimits {
    /// Open connections at once; one more is closed without an answer.
    pub connections: usize,
    /// A client has this long to send its request line and headers.
    pub header_timeout: Duration,
    /// And this long for the whole exchange.
    pub connection_timeout: Duration,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            connections: MAX_CONNECTIONS,
            header_timeout: Duration::from_secs(5),
            connection_timeout: Duration::from_secs(10),
        }
    }
}

enum Check {
    /// Alive until the guard is dropped.
    Task { alive: Arc<AtomicBool> },
    /// Alive while ticks keep coming.
    Progress { last: Arc<AtomicU64>, max_age: Duration },
}

struct Entry {
    name: &'static str,
    check: Check,
}

/// What the probes report. Shared by the loops that update it and the server that reads it.
pub struct Health {
    clock: Arc<dyn Clock>,
    base: Instant,
    checks: Mutex<Vec<Entry>>,
    connection: Mutex<Option<watch::Receiver<ConnectionState>>>,
    shutting_down: AtomicBool,
}

impl fmt::Debug for Health {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Health")
            .field("healthy", &self.unhealthy().is_empty())
            .field("ready", &self.ready().is_ok())
            .finish_non_exhaustive()
    }
}

/// A running task's claim to be alive. Dropping it (the task ended, by returning or by panicking) fails `/healthz`.
#[derive(Debug)]
#[must_use = "the guard is the claim: dropping it at once reports the loop as ended"]
pub struct TaskGuard {
    alive: Arc<AtomicBool>,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

/// Progress of a loop that has to keep moving.
#[derive(Clone)]
pub struct Progress {
    last: Arc<AtomicU64>,
    clock: Arc<dyn Clock>,
    base: Instant,
}

impl fmt::Debug for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Progress").finish_non_exhaustive()
    }
}

impl Progress {
    /// The loop did something.
    pub fn tick(&self) {
        let millis = self
            .clock
            .instant()
            .saturating_duration_since(self.base)
            .as_millis();
        self.last
            .store(u64::try_from(millis).unwrap_or(u64::MAX), Ordering::SeqCst);
    }
}

/// Why `/readyz` says no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotReady {
    ShuttingDown,
    NoSession,
    Disconnected,
    Connecting,
}

impl NotReady {
    fn word(self) -> &'static str {
        match self {
            Self::ShuttingDown => "shutting down",
            Self::NoSession => "starting",
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
        }
    }
}

impl Health {
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            base: clock.instant(),
            clock,
            checks: Mutex::new(Vec::new()),
            connection: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
        })
    }

    fn checks(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.checks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Register a loop that must not end. Hold the guard for as long as the loop runs.
    pub fn task(&self, name: &'static str) -> TaskGuard {
        let alive = Arc::new(AtomicBool::new(true));
        self.checks().push(Entry {
            name,
            check: Check::Task {
                alive: Arc::clone(&alive),
            },
        });
        TaskGuard { alive }
    }

    /// Register a loop that must make progress at least every `max_age`. The clock starts now.
    pub fn progress(&self, name: &'static str, max_age: Duration) -> Progress {
        let progress = Progress {
            last: Arc::new(AtomicU64::new(0)),
            clock: Arc::clone(&self.clock),
            base: self.base,
        };
        progress.tick();
        self.checks().push(Entry {
            name,
            check: Check::Progress {
                last: Arc::clone(&progress.last),
                max_age,
            },
        });
        progress
    }

    /// Follow the session: ready means [`ConnectionState::Connected`].
    pub fn follow_connection(&self, state: watch::Receiver<ConnectionState>) {
        *self.connection.lock().unwrap_or_else(PoisonError::into_inner) = Some(state);
    }

    /// From now on the agent is not ready, so that traffic and probes move away while it flushes and exits.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// The names of the loops that have ended or stalled.
    pub fn unhealthy(&self) -> Vec<&'static str> {
        let now = self.clock.instant().saturating_duration_since(self.base);
        self.checks()
            .iter()
            .filter(|entry| match &entry.check {
                Check::Task { alive } => !alive.load(Ordering::SeqCst),
                Check::Progress { last, max_age } => {
                    let last = Duration::from_millis(last.load(Ordering::SeqCst));
                    now.saturating_sub(last) > *max_age
                }
            })
            .map(|entry| entry.name)
            .collect()
    }

    pub fn ready(&self) -> Result<(), NotReady> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(NotReady::ShuttingDown);
        }
        let connection = self.connection.lock().unwrap_or_else(PoisonError::into_inner);
        match connection.as_ref().map(|rx| *rx.borrow()) {
            None => Err(NotReady::NoSession),
            Some(ConnectionState::Connected) => Ok(()),
            Some(ConnectionState::Connecting) => Err(NotReady::Connecting),
            Some(ConnectionState::Disconnected) => Err(NotReady::Disconnected),
        }
    }
}

/// What the server needs.
#[derive(Clone)]
struct Endpoints {
    health: Arc<Health>,
    metrics: Arc<Metrics>,
}

fn reply(status: StatusCode, content_type: &'static str, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}

fn text(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    reply(status, "text/plain; charset=utf-8", body)
}

/// Answer one request. Public so a test can ask without a socket.
pub fn respond<B>(health: &Health, metrics: &Metrics, request: &Request<B>) -> Response<Full<Bytes>> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n".to_owned());
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let mut response = match request.uri().path() {
        "/healthz" => {
            let bad = health.unhealthy();
            if bad.is_empty() {
                text(StatusCode::OK, "ok\n".to_owned())
            } else {
                text(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("unhealthy: {}\n", bad.join(", ")),
                )
            }
        }
        "/readyz" => match health.ready() {
            Ok(()) => text(StatusCode::OK, "ready\n".to_owned()),
            Err(why) => text(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("not ready: {}\n", why.word()),
            ),
        },
        "/metrics" => reply(
            StatusCode::OK,
            "text/plain; version=0.0.4; charset=utf-8",
            metrics.render(),
        ),
        _ => text(StatusCode::NOT_FOUND, "not found\n".to_owned()),
    };
    if request.method() == Method::HEAD {
        // A HEAD answer has the headers of the GET answer and no body.
        *response.body_mut() = Full::new(Bytes::new());
    }
    response
}

/// Serve the three endpoints on `listener` until the future is dropped.
pub async fn serve(listener: TcpListener, health: Arc<Health>, metrics: Arc<Metrics>) -> Infallible {
    serve_with(listener, health, metrics, ServerLimits::default()).await
}

/// As [`serve`], with chosen limits.
pub async fn serve_with(
    listener: TcpListener,
    health: Arc<Health>,
    metrics: Arc<Metrics>,
    limits: ServerLimits,
) -> Infallible {
    let endpoints = Endpoints { health, metrics };
    let slots = Arc::new(Semaphore::new(limits.connections.max(1)));
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // Out of file descriptors, or the like. Back off briefly instead of spinning.
                warn!(kind = ?error.kind(), "the health port could not accept a connection");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        // A connection beyond the limit is closed at once, not queued.
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            debug!("too many connections to the health port; closed one");
            continue;
        };
        let endpoints = endpoints.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                let response = respond(&endpoints.health, &endpoints.metrics, &request);
                async move { Ok::<_, Infallible>(response) }
            });
            let connection = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.header_timeout)
                .keep_alive(false)
                .serve_connection(TokioIo::new(stream), service);
            // An error is a client that went away or sent nonsense: nothing to report.
            let _ = timeout(limits.connection_timeout, connection).await;
        });
    }
}
