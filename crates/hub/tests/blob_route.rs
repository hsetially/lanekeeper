//! T8 (P7, S4, S10, S12): the blob endpoint in the composed hub: it is in the route table at Viewer, passes through the same
//! headers, limits and metrics as every route, and keeps its immutable cache header (the one response the header layer lets
//! through).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use domain::{Role, UserStatus};
use http::{Method, StatusCode, header};
use hub::{BlobsModule, Deps, EventsModule, Hub, HubSettings};
use hub_platform::blobs::endpoint::{BlobState, DenyHashes};
use hub_platform::events::fake::FakeEventSource;
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::sse::{SseSettings, SseState};
use hub_platform::events::visibility::RoleVisibility;
use hub_platform::{FakeSessionAuth, RouteGuard, platform_routes};
use ports::fakes::FakeBlobStore;
use ports::{BlobStore, content_hash};
use support::{anonymous, get, header_of, json, request_as, send};
use tokio::sync::watch;

const IMMUTABLE: &str = "private, max-age=31536000, immutable";
const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

struct Rig {
    hub: Hub,
    store: Arc<FakeBlobStore>,
    _closing: watch::Sender<bool>,
    _source: Arc<FakeEventSource>,
}

fn rig(denied: &[domain::ContentHash]) -> Rig {
    let store = Arc::new(FakeBlobStore::new());
    let auth = Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Admin), UserStatus::Disabled),
    );
    let blobs = BlobState::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        Arc::new(DenyHashes::new(denied.iter().copied())),
    );
    let source = Arc::new(FakeEventSource::new(16));
    let (closing, closing_rx) = watch::channel(false);
    let sse = SseState::new(
        Arc::clone(&source) as _,
        Arc::clone(&auth) as _,
        Arc::new(RoleVisibility),
        SseSettings::default(),
        closing_rx,
        EventMetrics::default(),
    );
    let hub = Hub::build(Deps {
        settings: HubSettings::default(),
        auth,
        modules: vec![
            Arc::new(BlobsModule::new(blobs)),
            Arc::new(EventsModule::new(sse)),
        ],
        static_files: None,
        checks: Vec::new(),
    })
    .unwrap();
    Rig {
        hub,
        store,
        _closing: closing,
        _source: source,
    }
}

#[tokio::test]
async fn the_blob_route_is_in_the_hub_route_table_with_the_platform_guard() {
    let rig = rig(&[]);
    let expected: Vec<_> = platform_routes()
        .into_iter()
        .filter(|r| r.path == "/api/v1/blobs/{hash}" || r.path == "/api/v1/events")
        .collect();
    assert_eq!(expected.len(), 2);
    let mut table = rig.hub.routes.clone();
    table.sort_by(|a, b| a.path.cmp(&b.path));
    let mut expected = expected;
    expected.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(table, expected);
    let blob = table.iter().find(|r| r.path == "/api/v1/blobs/{hash}").unwrap();
    assert_eq!(blob.guard, RouteGuard::Role(Role::Viewer));
}

#[tokio::test]
async fn a_blob_through_the_hub_keeps_its_immutable_header_and_gets_the_api_policy() {
    let rig = rig(&[]);
    let hash = rig.store.put(Bytes::from_static(b"a: 1\r\n")).await.unwrap();
    let response = send(&rig.hub.web, get(&format!("/api/v1/blobs/{hash}"))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "cache-control"), Some(IMMUTABLE));
    assert_eq!(header_of(&response, "etag"), Some(format!("\"{hash}\"").as_str()));
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/octet-stream")
    );
    assert!(
        header_of(&response, "content-disposition")
            .unwrap()
            .starts_with("attachment")
    );
    assert_eq!(header_of(&response, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header_of(&response, "content-security-policy"), Some(API_CSP));
    assert_eq!(
        header_of(&response, "strict-transport-security"),
        Some("max-age=63072000; includeSubDomains")
    );
    assert_eq!(
        header_of(&response, "cross-origin-resource-policy"),
        Some("same-origin")
    );
    assert!(header_of(&response, "x-request-id").is_some());
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(&body[..], b"a: 1\r\n");
}

#[tokio::test]
async fn a_304_through_the_hub_keeps_the_immutable_header() {
    let rig = rig(&[]);
    let hash = rig
        .store
        .put(Bytes::from_static(b"cached by the browser"))
        .await
        .unwrap();
    let mut request = get(&format!("/api/v1/blobs/{hash}"));
    request
        .headers_mut()
        .insert(header::IF_NONE_MATCH, format!("\"{hash}\"").parse().unwrap());
    let response = send(&rig.hub.web, request).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_of(&response, "cache-control"), Some(IMMUTABLE));
    assert_eq!(header_of(&response, "etag"), Some(format!("\"{hash}\"").as_str()));
}

#[tokio::test]
async fn blob_errors_are_never_cached_and_are_problems() {
    let rig = rig(&[]);
    let unknown = content_hash(b"unknown");
    for (path, status, code) in [
        (
            format!("/api/v1/blobs/{unknown}"),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "/api/v1/blobs/not-a-hash".to_owned(),
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
    ] {
        let response = send(&rig.hub.web, get(&path)).await;
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(header_of(&response, "cache-control"), Some("no-store"), "{path}");
        assert_eq!(
            header_of(&response, "content-type"),
            Some("application/problem+json")
        );
        assert!(header_of(&response, "etag").is_none());
        let problem = json(response).await;
        assert_eq!(problem["code"], code);
        assert!(
            problem["request_id"].is_string(),
            "the edge adds the request id to the problem"
        );
    }
}

#[tokio::test]
async fn a_withheld_blob_through_the_hub_is_indistinguishable_from_an_unknown_one() {
    let secret = content_hash(b"password: hunter2\n");
    let rig = rig(&[secret]);
    rig.store
        .put(Bytes::from_static(b"password: hunter2\n"))
        .await
        .unwrap();
    let withheld = send(&rig.hub.web, get(&format!("/api/v1/blobs/{secret}"))).await;
    let unknown = send(
        &rig.hub.web,
        get(&format!("/api/v1/blobs/{}", content_hash(b"never stored"))),
    )
    .await;
    assert_eq!(withheld.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let (a, b) = (json(withheld).await, json(unknown).await);
    assert_eq!(a["code"], b["code"]);
    assert_eq!(a["title"], b["title"]);
    assert_eq!(a["status"], b["status"]);
}

#[tokio::test]
async fn sse_and_blob_require_viewer_and_active_user() {
    let rig = rig(&[]);
    let hash = rig.store.put(Bytes::from_static(b"content")).await.unwrap();
    let blob = format!("/api/v1/blobs/{hash}");
    for path in [blob.as_str(), "/api/v1/events"] {
        for (user, expected) in [
            ("pending", StatusCode::FORBIDDEN),
            ("disabled", StatusCode::FORBIDDEN),
        ] {
            let response = send(
                &rig.hub.web,
                request_as(user, [198, 51, 100, 1], Method::GET, path, Body::empty()),
            )
            .await;
            assert_eq!(response.status(), expected, "{user} {path}");
            assert_eq!(
                header_of(&response, "content-type"),
                Some("application/problem+json")
            );
        }
        let no_session = send(&rig.hub.web, anonymous(Method::GET, path, Body::empty())).await;
        assert_eq!(no_session.status(), StatusCode::UNAUTHORIZED, "{path}");
        let viewer = send(&rig.hub.web, get(path)).await;
        assert_eq!(viewer.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn blob_metrics_name_the_route_template_and_never_the_hash() {
    let rig = rig(&[]);
    let hash = rig.store.put(Bytes::from_static(b"metric")).await.unwrap();
    let _ = send(&rig.hub.web, get(&format!("/api/v1/blobs/{hash}"))).await;
    let text = rig.hub.metrics.encode();
    assert!(text.contains(r#"route="/api/v1/blobs/{hash}""#), "{text}");
    assert!(!text.contains(&hash.to_string()), "a hash is never a label");
}
