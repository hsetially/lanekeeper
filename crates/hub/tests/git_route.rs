//! T7: the GitHub webhook in the composed hub: it is in the route table as a public route with its reason, it needs no session,
//! and it passes through the same headers and limits as every route.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;

use axum::body::Body;
use domain::RepoKind;
use http::{Method, Request, StatusCode};
use hub::{Deps, GitModule, Hub, HubSettings, SessionEdge};
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::payload::RepoFullName;
use hub_platform::git::webhook::{MemoryDeliveryLog, WebhookSettings, WebhookState};
use hub_platform::{FakeSessionAuth, RouteGuard, platform_routes};
use ports::fakes::FakeSecretSource;
use support::{header_of, json, peer, send};

const SECRET: &str = "whsec_HubRouteTestSecret";
const REPO: &str = "acme/csp-tenant-data";

struct Rig {
    hub: Hub,
    queue: Arc<FetchQueue>,
}

fn rig() -> Rig {
    let queue = FetchQueue::new(16);
    let state = WebhookState::new(
        WebhookSettings::new(
            "webhook-secret",
            vec![(RepoKind::Tenant, RepoFullName::parse(REPO).unwrap())],
        ),
        Arc::new(FakeSecretSource::new().with("webhook-secret", SECRET)),
        Arc::new(MemoryDeliveryLog::default()),
        Arc::clone(&queue),
        GitMetrics::default(),
    );
    let hub = Hub::build(Deps {
        settings: HubSettings::default(),
        session: SessionEdge::without_sessions(Arc::new(FakeSessionAuth::new())),
        mounted: Vec::new(),
        modules: vec![Arc::new(GitModule::new(Arc::new(state)))],
        static_files: None,
        checks: Vec::new(),
    })
    .unwrap();
    Rig { hub, queue }
}

fn sign(body: &[u8]) -> String {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, SECRET.as_bytes());
    format!(
        "sha256={}",
        hex::encode(aws_lc_rs::hmac::sign(&key, body).as_ref())
    )
}

fn push_body() -> Vec<u8> {
    serde_json::json!({
        "ref": "refs/heads/sit1",
        "before": "89abcdef0123456789abcdef0123456789abcdef",
        "after": "0123456789abcdef0123456789abcdef01234567",
        "repository": {"full_name": REPO},
    })
    .to_string()
    .into_bytes()
}

/// A delivery with no session at all, from a documentation-range address.
fn delivery(body: Vec<u8>, signature: &str, id: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/hooks/github")
        .header("x-github-event", "push")
        .header("x-github-delivery", id)
        .header("x-hub-signature-256", signature)
        .body(Body::from(body))
        .unwrap();
    request.extensions_mut().insert(peer([198, 51, 100, 9]));
    request
}

#[tokio::test]
async fn the_webhook_is_in_the_hub_route_table_as_a_public_route_with_the_platform_reason() {
    let rig = rig();
    let expected: Vec<_> = platform_routes()
        .into_iter()
        .filter(|r| r.path == "/hooks/github")
        .collect();
    assert_eq!(expected.len(), 1);
    assert_eq!(rig.hub.routes, expected);
    assert!(matches!(rig.hub.routes[0].guard, RouteGuard::Public { .. }));
}

#[tokio::test]
async fn a_signed_push_needs_no_session_and_queues_a_fetch() {
    let rig = rig();
    let body = push_body();
    let response = send(&rig.hub.web, delivery(body.clone(), &sign(&body), "d-1")).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(rig.queue.len(), 1);
}

#[tokio::test]
async fn the_webhook_answers_carry_the_api_headers() {
    let rig = rig();
    let body = push_body();
    let ok = send(&rig.hub.web, delivery(body.clone(), &sign(&body), "d-1")).await;
    let bad = send(&rig.hub.web, delivery(body, "sha256=00", "d-2")).await;
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(header_of(&bad, "content-type"), Some("application/problem+json"));
    for response in [&ok, &bad] {
        assert_eq!(header_of(response, "cache-control"), Some("no-store"));
        assert_eq!(header_of(response, "x-content-type-options"), Some("nosniff"));
        assert_eq!(
            header_of(response, "strict-transport-security"),
            Some("max-age=63072000; includeSubDomains")
        );
        assert_eq!(
            header_of(response, "content-security-policy"),
            Some("default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
        );
        assert!(header_of(response, "x-request-id").is_some());
    }
    let problem = json(bad).await;
    assert_eq!(problem["code"], "unauthorized");
    assert!(rig.queue.len() == 1, "only the signed one was queued");
}

#[tokio::test]
async fn a_body_over_the_hub_cap_is_refused_by_the_limits_layer_and_one_over_the_webhook_cap_by_the_handler()
{
    let rig = rig();
    // 5 MiB: over the hub's 4 MiB cap, stopped before the handler.
    let huge = vec![b' '; 5 * 1024 * 1024];
    let response = send(&rig.hub.web, delivery(huge.clone(), &sign(&huge), "big-1")).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    // 2 MiB: inside the hub's cap, over the webhook's own 1 MiB.
    let big = vec![b' '; 2 * 1024 * 1024];
    let response = send(&rig.hub.web, delivery(big.clone(), &sign(&big), "big-2")).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(rig.queue.is_empty());
}
