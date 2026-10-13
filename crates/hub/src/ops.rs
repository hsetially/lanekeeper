//! The operations listener (T1, S16): `/healthz`, `/readyz` and `/metrics` on an internal plain-HTTP port.
//!
//! No secrets and nothing mutable here: three read-only routes, answered in a few bytes, that name no cause beyond a fixed
//! check name. The port is cluster-internal (the `NetworkPolicy` in `HARDENING.md` lets only the kubelet and the metrics
//! scraper reach it), so none of these routes is on the web listener and none is an API operation (S4: they are listed as
//! route-only allowances in the route-table test).
//!
//! - `/healthz` is liveness: the process is up and serving.
//! - `/readyz` is readiness: shutdown has not begun and every registered check passes (Postgres reachable; mirrors opened
//!   and the outbox listener connected, once those modules register their checks).

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::metrics::Metrics;
use crate::shutdown::{Drain, Phase};

/// One readiness condition. It must answer quickly: the probe gives all checks together a deadline.
#[async_trait]
pub trait ReadyCheck: Send + Sync + 'static {
    async fn ready(&self) -> bool;
}

/// The longest the probe waits for all checks together; a check that has not answered by then counts as failed.
pub const READY_DEADLINE: Duration = Duration::from_secs(2);

type NamedCheck = (&'static str, Arc<dyn ReadyCheck>);

/// The checks behind `/readyz`, and the shutdown phase.
#[derive(Clone)]
pub struct Readiness {
    checks: Arc<RwLock<Vec<NamedCheck>>>,
    drain: Drain,
}

impl std::fmt::Debug for Readiness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readiness").finish_non_exhaustive()
    }
}

impl Readiness {
    #[must_use]
    pub fn new(drain: Drain) -> Self {
        Self {
            checks: Arc::new(RwLock::new(Vec::new())),
            drain,
        }
    }

    /// Add a condition. `name` is a fixed word; it is what `/readyz` prints when the check fails.
    pub fn register(&self, name: &'static str, check: Arc<dyn ReadyCheck>) {
        self.checks
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name, check));
    }

    /// The names of the failing conditions. Empty means ready.
    pub async fn failing(&self) -> Vec<&'static str> {
        if self.drain.phase() != Phase::Running {
            return vec!["shutting_down"];
        }
        let checks: Vec<_> = self.checks.read().unwrap_or_else(PoisonError::into_inner).clone();
        let run = async {
            let mut failing = Vec::new();
            for (name, check) in &checks {
                if !check.ready().await {
                    failing.push(*name);
                }
            }
            failing
        };
        match tokio::time::timeout(READY_DEADLINE, run).await {
            Ok(failing) => failing,
            Err(_) => vec!["timeout"],
        }
    }
}

#[derive(Clone)]
struct OpsState {
    readiness: Readiness,
    metrics: Arc<Metrics>,
}

fn plain(status: StatusCode, content_type: &'static str, body: String) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn healthz() -> Response {
    plain(StatusCode::OK, "text/plain; charset=utf-8", "ok\n".to_owned())
}

async fn readyz(State(state): State<OpsState>) -> Response {
    let failing = state.readiness.failing().await;
    if failing.is_empty() {
        plain(StatusCode::OK, "text/plain; charset=utf-8", "ready\n".to_owned())
    } else {
        plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "text/plain; charset=utf-8",
            format!("not ready: {}\n", failing.join(", ")),
        )
    }
}

async fn metrics(State(state): State<OpsState>) -> Response {
    plain(
        StatusCode::OK,
        "application/openmetrics-text; version=1.0.0; charset=utf-8",
        state.metrics.encode(),
    )
}

/// The three routes. Only `GET` (and `HEAD`, which axum derives) are answered.
pub fn router(readiness: Readiness, metrics_registry: Arc<Metrics>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .fallback(|| async {
            plain(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                "not found\n".to_owned(),
            )
        })
        .with_state(OpsState {
            readiness,
            metrics: metrics_registry,
        })
}
