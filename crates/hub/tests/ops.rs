//! T1, S10, S16: `/healthz`, `/readyz`, `/metrics` and the shutdown sequence.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use http::{Method, StatusCode};
use hub::ops::ReadyCheck;
use hub::{Deps, Hub, HubSettings};
use hub_platform::FakeSessionAuth;
use support::{default_hub, get, header_of, request, send, text};

struct Flag(Arc<AtomicBool>);

#[async_trait]
impl ReadyCheck for Flag {
    async fn ready(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

struct Hangs;

#[async_trait]
impl ReadyCheck for Hangs {
    async fn ready(&self) -> bool {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        true
    }
}

fn hub_with(checks: Vec<(&'static str, Arc<dyn ReadyCheck>)>) -> Hub {
    Hub::build(Deps {
        settings: HubSettings::default(),
        auth: Arc::new(FakeSessionAuth::new()),
        modules: Vec::new(),
        static_files: None,
        checks,
    })
    .unwrap()
}

#[tokio::test]
async fn healthz_is_ok_and_names_nothing() {
    let t = default_hub();
    let response = send(&t.hub.ops, get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text(response).await, "ok\n");
}

#[tokio::test]
async fn readyz_follows_the_checks_and_names_the_failing_one() {
    let db = Arc::new(AtomicBool::new(false));
    let hub = hub_with(vec![("postgres", Arc::new(Flag(Arc::clone(&db))))]);
    let response = send(&hub.ops, get("/readyz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(text(response).await, "not ready: postgres\n");
    db.store(true, Ordering::SeqCst);
    let response = send(&hub.ops, get("/readyz")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text(response).await, "ready\n");
}

#[tokio::test(start_paused = true)]
async fn a_check_that_hangs_makes_the_hub_not_ready_after_two_seconds() {
    let hub = hub_with(vec![("mirrors", Arc::new(Hangs))]);
    let ops = hub.ops.clone();
    let task = tokio::spawn(async move { send(&ops, get("/readyz")).await });
    tokio::time::advance(Duration::from_secs(3)).await;
    let response = task.await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(text(response).await, "not ready: timeout\n");
}

#[tokio::test]
async fn readiness_fails_as_soon_as_shutdown_begins_while_health_stays_ok() {
    let hub = hub_with(vec![]);
    assert_eq!(send(&hub.ops, get("/readyz")).await.status(), StatusCode::OK);
    hub.drain.begin_not_ready();
    let response = send(&hub.ops, get("/readyz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(text(response).await, "not ready: shutting_down\n");
    assert_eq!(send(&hub.ops, get("/healthz")).await.status(), StatusCode::OK);
    // New web requests are still served until the stop phase: the load balancer needs the grace period to notice.
    let web = send(&hub.web, get("/anything")).await;
    assert_eq!(
        web.status(),
        StatusCode::NOT_FOUND,
        "served, not refused with a 503"
    );
    hub.drain.begin_stopping();
    let web = send(&hub.web, get("/anything")).await;
    assert_eq!(web.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn metrics_are_labelled_by_route_template_and_never_by_path_user_or_id() {
    let t = default_hub();
    let hash = "f".repeat(64);
    for _ in 0..3 {
        send(&t.hub.web, get(&format!("/api/v1/blobs/{hash}"))).await;
    }
    send(&t.hub.web, get("/api/v1/ping")).await;
    // Many different unmatched paths must not create many series.
    for i in 0..50 {
        send(&t.hub.web, get(&format!("/api/v1/random-{i}"))).await;
    }
    send(&t.hub.web, hub_anonymous()).await;
    let response = send(&t.hub.ops, get("/metrics")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/openmetrics-text; version=1.0.0; charset=utf-8")
    );
    let body = text(response).await;
    assert!(body.contains(r#"route="/api/v1/blobs/{hash}""#), "{body}");
    assert!(body.contains(r#"route="/api/v1/ping""#));
    assert!(body.contains(r#"route="unmatched""#));
    for leak in [hash.as_str(), "random-", "viewer", "x-test-user", "198.51.100"] {
        assert!(!body.contains(leak), "the exposition contains {leak}");
    }
    let series = body
        .lines()
        .filter(|l| l.starts_with("lanekeeper_http_requests_total"))
        .count();
    assert!(series <= 6, "{series} request series for 5 routes");
}

fn hub_anonymous() -> http::Request<Body> {
    support::anonymous(Method::GET, "/api/v1/ping", Body::empty())
}

#[tokio::test]
async fn the_operations_listener_answers_get_only_and_nothing_else() {
    let t = default_hub();
    assert_eq!(
        send(&t.hub.ops, request(Method::POST, "/healthz", Body::empty()))
            .await
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        send(&t.hub.ops, get("/api/v1/ping")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(send(&t.hub.ops, get("/")).await.status(), StatusCode::NOT_FOUND);
}
