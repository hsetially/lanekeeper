//! Shared helpers for the hub's integration tests: a hub built from fakes with one module that has a route of every class.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use domain::{Role, UserStatus};
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use hub::client_ip::PeerAddr;
use hub::static_files::StaticFiles;
use hub::{Deps, Hub, HubSettings, Module};
use hub_platform::{FakeSessionAuth, GuardedRouter, TEST_USER_HEADER};
use tower::ServiceExt;

pub mod pki;

pub const INDEX_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Lanekeeper</title>
<link rel="stylesheet" nonce="__CSP_NONCE__" href="/assets/app-abc123.css"></head>
<body><div id="root"></div><script type="module" nonce="__CSP_NONCE__" src="/assets/app-abc123.js"></script></body></html>"#;

/// How many times each handler body ran.
#[derive(Debug, Default)]
pub struct Counters {
    pub raw: AtomicUsize,
    pub echo: AtomicUsize,
}

struct TestModule {
    counters: Arc<Counters>,
}

impl Module for TestModule {
    fn name(&self) -> &'static str {
        "test"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        let raw = Arc::clone(&self.counters);
        let echo = Arc::clone(&self.counters);
        router
            .route(&Method::GET, "/api/v1/ping", Role::Viewer, || async {
                axum::Json(serde_json::json!({"ok": true}))
            })
            .route(
                &Method::POST,
                "/api/v1/echo",
                Role::Viewer,
                move |axum::Json(v): axum::Json<serde_json::Value>| {
                    let echo = Arc::clone(&echo);
                    async move {
                        echo.echo.fetch_add(1, Ordering::SeqCst);
                        axum::Json(v)
                    }
                },
            )
            .route(&Method::POST, "/api/v1/raw", Role::Viewer, move |body: Bytes| {
                let raw = Arc::clone(&raw);
                async move {
                    raw.raw.fetch_add(1, Ordering::SeqCst);
                    body.len().to_string()
                }
            })
            .route(&Method::GET, "/api/v1/events", Role::Viewer, || async {
                let mut response = Response::new(Body::from("retry: 1000\n\n"));
                response
                    .headers_mut()
                    .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
                response
            })
            .route(
                &Method::GET,
                "/api/v1/blobs/{hash}",
                Role::Viewer,
                |Path(hash): Path<String>| async move {
                    let mut response = Response::new(Body::from(vec![1u8, 2, 3]));
                    let headers = response.headers_mut();
                    headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
                    headers.insert(header::ETAG, format!("\"{hash}\"").parse().unwrap());
                    headers.insert(
                        header::CACHE_CONTROL,
                        "private, max-age=31536000, immutable".parse().unwrap(),
                    );
                    response
                },
            )
            .route(&Method::GET, "/api/v1/boom", Role::Viewer, || async {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ERROR: relation \"secrets\" does not exist at /srv/hub/db.rs:42",
                )
                    .into_response()
            })
            .route(&Method::GET, "/api/v1/slow", Role::Viewer, || async {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                "late"
            })
            .route(&Method::GET, "/api/v1/cache-me", Role::Viewer, || async {
                let mut response = Response::new(Body::from("x"));
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".parse().unwrap(),
                );
                response
            })
            .route(&Method::GET, "/api/v1/logs", Role::Viewer, || async {
                tracing::info!(target: "hub::test", "handled a request");
                "logged"
            })
            .route(&Method::POST, "/api/v1/edit", Role::Editor, || async { "edited" })
            .route(&Method::GET, "/api/v1/conflict", Role::Viewer, || async {
                let body = serde_json::json!({
                    "type": "urn:lanekeeper:problem:conflict",
                    "title": "Conflict",
                    "status": 409,
                    "code": "conflict",
                    "current": "a".repeat(64),
                });
                let mut response = Response::new(Body::from(body.to_string()));
                *response.status_mut() = StatusCode::CONFLICT;
                response
                    .headers_mut()
                    .insert(header::CONTENT_TYPE, "application/problem+json".parse().unwrap());
                response
            })
            .public(&Method::POST, "/hooks/github", "verified by HMAC", || async {
                StatusCode::ACCEPTED
            })
    }
}

pub struct TestHub {
    pub hub: Hub,
    pub dist: tempfile::TempDir,
    pub auth: Arc<FakeSessionAuth>,
    pub counters: Arc<Counters>,
}

pub fn write_dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), INDEX_HTML).unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/app-abc123.js"), "console.log(1)").unwrap();
    std::fs::write(dir.path().join("assets/app-abc123.css"), "body{}").unwrap();
    std::fs::write(dir.path().join("favicon.ico"), [0u8, 1, 2]).unwrap();
    dir
}

pub fn build(settings: HubSettings) -> TestHub {
    let dist = write_dist();
    let auth = Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("viewer2", Some(Role::Viewer), UserStatus::Active)
            .with_user("editor", Some(Role::Editor), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Admin), UserStatus::Disabled)
            .with_user("norole", None, UserStatus::Active),
    );
    let counters = Arc::new(Counters::default());
    let hub = Hub::build(Deps {
        settings,
        auth: auth.clone(),
        modules: vec![Arc::new(TestModule {
            counters: Arc::clone(&counters),
        })],
        static_files: Some(Arc::new(StaticFiles::open(dist.path()).unwrap())),
        checks: Vec::new(),
    })
    .unwrap();
    TestHub {
        hub,
        dist,
        auth,
        counters,
    }
}

pub fn default_hub() -> TestHub {
    build(HubSettings::default())
}

pub fn peer(ip: [u8; 4]) -> PeerAddr {
    PeerAddr(SocketAddr::from((ip, 40000)))
}

/// A request as `viewer`, from 198.51.100.1.
pub fn request(method: Method, path: &str, body: Body) -> Request<Body> {
    request_as("viewer", [198, 51, 100, 1], method, path, body)
}

/// A request as `user` from `ip`.
pub fn request_as(user: &str, ip: [u8; 4], method: Method, path: &str, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(TEST_USER_HEADER, user)
        .body(body)
        .unwrap();
    request.extensions_mut().insert(peer(ip));
    request
}

pub fn get(path: &str) -> Request<Body> {
    request(Method::GET, path, Body::empty())
}

/// A request with no session.
pub fn anonymous(method: Method, path: &str, body: Body) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(path).body(body).unwrap();
    request.extensions_mut().insert(peer([198, 51, 100, 1]));
    request
}

pub async fn send(router: &Router, request: Request<Body>) -> Response<Body> {
    router.clone().oneshot(request).await.unwrap()
}

pub async fn text(response: Response<Body>) -> String {
    String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes()).into_owned()
}

pub async fn json(response: Response<Body>) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

pub fn header_of<'a>(response: &'a Response<Body>, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}
