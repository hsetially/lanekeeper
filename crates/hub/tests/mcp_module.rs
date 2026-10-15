//! Prompt 07 (T1): the MCP module in the composed hub. The routes pass the hub's route-table check (S4), everything else under
//! `/mcp` is default deny, the challenge of a refused request survives the hub's error layer, a stream that outlives the head
//! timeout is not cut, and the hub's log filter keeps `rmcp` at `warn` (S10).
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

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use domain::{Guid, Role, ShortText, User, UserId, UserStatus};
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use hub::client_ip::PeerAddr;
use hub::{Deps, Hub, HubSettings, McpModule};
use hub_mcp::auth::oauth::OAuthSettings;
use hub_mcp::config::{AuthMode, McpConfig, PublicOrigin};
use hub_mcp::{Credentials, McpDeps, McpSettings, McpState};
use hub_platform::{FakeSessionAuth, RouteSpec};
use ports::VerifiedUser;
use ports::conformance::route_guard::{
    Allowances, RegisteredRoute, SpecOperation, assert_route_table, spec_operations_from_openapi,
};
use ports::fakes::{FakeTokenVerifier, FakeUsers};
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "hub.example.test";
const TOKEN: &str = "entra-token-for-alice";

fn alice() -> UserId {
    UserId::new(
        Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
        Guid::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
    )
}

fn user(id: UserId, status: UserStatus) -> User {
    User {
        id,
        email: ShortText::parse("alice@example.test").unwrap(),
        display_name: ShortText::parse("Alice").unwrap(),
        role: Some(Role::Viewer),
        status,
        requires_approval: false,
        github_login: None,
        last_seen_at: None,
    }
}

struct Built {
    hub: Hub,
    users: FakeUsers,
}

fn build(request_timeout: Duration) -> Built {
    let verifier = FakeTokenVerifier::new();
    verifier.accept_entra(
        TOKEN,
        VerifiedUser {
            id: alice(),
            email: None,
            display_name: None,
        },
    );
    let users = FakeUsers::new();
    users.insert(user(alice(), UserStatus::Active));
    let oauth = OAuthSettings {
        tenant_id: Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
        api_audience: "api://hub-api".to_owned(),
        authority_host: OAuthSettings::DEFAULT_AUTHORITY_HOST.to_owned(),
        authorization_server_mirror: true,
    };
    let settings = McpSettings {
        // No keep-alive timer: these tests run on a paused clock.
        sse_keep_alive: None,
        ..McpSettings::new(
            McpConfig {
                auth_mode: AuthMode::Oauth,
                ..McpConfig::default()
            },
            PublicOrigin::parse(&format!("https://{HOST}")).unwrap(),
            Some(oauth),
        )
    };
    let state = McpState::new(
        McpDeps {
            credentials: Credentials::Oauth(Arc::new(verifier)),
            users: Arc::new(users.clone()),
        },
        settings,
    )
    .unwrap();
    let mut hub_settings = HubSettings::default();
    hub_settings.limits.request_timeout = request_timeout;
    let hub = Hub::build(Deps {
        settings: hub_settings,
        auth: Arc::new(FakeSessionAuth::new().with_user("alice", Some(Role::Viewer), UserStatus::Active)),
        modules: vec![Arc::new(McpModule::new(state))],
        static_files: None,
        checks: Vec::new(),
    })
    .unwrap();
    Built { hub, users }
}

fn request(method: Method, path: &str, bearer: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let mut request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .unwrap();
    request
        .extensions_mut()
        .insert(PeerAddr(SocketAddr::from(([198, 51, 100, 1], 40000))));
    request
}

fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}
    })
}

fn spec() -> Vec<SpecOperation> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml");
    spec_operations_from_openapi(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn mcp_routes_pass_the_hub_route_table_check() {
    let built = build(Duration::from_secs(30));
    let spec = spec();
    let token_routes: BTreeSet<(&str, &str)> = [
        ("GET", "/api/v1/me/mcp-tokens"),
        ("POST", "/api/v1/me/mcp-tokens"),
        ("DELETE", "/api/v1/me/mcp-tokens/{id}"),
    ]
    .into_iter()
    .collect();
    let mut allow = Allowances::default();
    for op in &spec {
        if !token_routes.contains(&(op.method.as_str(), op.path.as_str())) {
            allow = allow.spec_only(&op.method, &op.path, "owned by another crate");
        }
    }
    let registered: Vec<RegisteredRoute> = built.hub.routes.iter().map(RouteSpec::registered).collect();
    for route in &built.hub.routes {
        if !token_routes.contains(&(route.method.as_str(), route.path.as_str())) {
            allow = allow.route_only(
                &route.method,
                &route.path,
                "the MCP endpoint and OAuth discovery are not API operations",
            );
        }
    }
    assert_route_table(&registered, &spec, &allow);
    // The hub's table holds exactly what the module registers: the three token routes, `/mcp` on three methods, three documents.
    assert_eq!(built.hub.routes.len(), 9);
}

#[tokio::test]
async fn mcp_default_deny_holds() {
    let built = build(Duration::from_secs(30));
    let web = &built.hub.web;
    let status = |method: Method, path: &str, bearer: Option<&str>| {
        let web = web.clone();
        let request = request(method, path, bearer, None);
        async move { web.oneshot(request).await.unwrap().status() }
    };
    // Unknown paths under /mcp are the hub's own 404, for a caller with a token or without.
    for path in ["/mcp/", "/mcp/x", "/mcp/../mcp", "/mcpx"] {
        assert_eq!(
            status(Method::GET, path, None).await,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        assert_eq!(
            status(Method::GET, path, Some(TOKEN)).await,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for method in [Method::PUT, Method::PATCH, Method::OPTIONS] {
        assert_eq!(
            status(method.clone(), "/mcp", Some(TOKEN)).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method}"
        );
    }
    // Every method of the endpoint is gated.
    for method in [Method::POST, Method::GET, Method::DELETE] {
        assert_eq!(
            status(method.clone(), "/mcp", None).await,
            StatusCode::UNAUTHORIZED,
            "{method}"
        );
    }
    // The documents of OAuth discovery are open, and nothing else under /.well-known is.
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
    ] {
        assert_eq!(status(Method::GET, path, None).await, StatusCode::OK, "{path}");
    }
    assert_eq!(
        status(Method::GET, "/.well-known/other", None).await,
        StatusCode::NOT_FOUND
    );
    // A personal-token route is the session's: a bearer token is not a session, and OAuth mode has no tokens to list.
    assert_eq!(
        status(Method::GET, "/api/v1/me/mcp-tokens", Some(TOKEN)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_challenge_and_the_problem_body_survive_the_hubs_error_layer() {
    let built = build(Duration::from_secs(30));
    let response = built
        .hub
        .web
        .clone()
        .oneshot(request(Method::POST, "/mcp", None, Some(initialize())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // The hub rewrites every error body into a problem; the header the OAuth client reads is not part of the body.
    assert_eq!(
        response.headers()[header::WWW_AUTHENTICATE],
        "Bearer resource_metadata=\"https://hub.example.test/.well-known/oauth-protected-resource/mcp\", scope=\"api://hub-api/mcp.access\""
    );
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["code"], "unauthorized");
    assert!(body["request_id"].is_string());
    // A pending user is a 403 with no challenge.
    let built = build(Duration::from_secs(30));
    built.users.insert(user(alice(), UserStatus::Pending));
    let response = built
        .hub
        .web
        .clone()
        .oneshot(request(Method::POST, "/mcp", Some(TOKEN), Some(initialize())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get(header::WWW_AUTHENTICATE).is_none());
}

#[tokio::test(start_paused = true)]
async fn a_mcp_response_that_streams_longer_than_the_head_timeout_is_not_cut() {
    // The hub's limits layer times out the response head (here 2 seconds), not the body. A server-to-client stream is a response
    // whose body lasts as long as the session.
    let built = build(Duration::from_secs(2));
    let web = built.hub.web.clone();
    let response = web
        .clone()
        .oneshot(request(Method::POST, "/mcp", Some(TOKEN), Some(initialize())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"].to_str().unwrap().to_owned();
    let _ = response.into_body().collect().await.unwrap();
    let mut initialized = request(
        Method::POST,
        "/mcp",
        Some(TOKEN),
        Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
    );
    initialized
        .headers_mut()
        .insert("mcp-session-id", session.parse().unwrap());
    assert_eq!(
        web.clone().oneshot(initialized).await.unwrap().status(),
        StatusCode::ACCEPTED
    );

    let mut open = request(Method::GET, "/mcp", Some(TOKEN), None);
    open.headers_mut()
        .insert("mcp-session-id", session.parse().unwrap());
    let response = web.clone().oneshot(open).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/event-stream");
    let mut body = response.into_body();
    // Far past the head timeout, the stream is open and idle: waiting on it times out, and does not end.
    tokio::time::sleep(Duration::from_secs(120)).await;
    let next = tokio::time::timeout(Duration::from_secs(30), body.frame()).await;
    match next {
        Err(_elapsed) => {}
        Ok(Some(Ok(frame))) => assert!(frame.is_data(), "only data (the priming event) may arrive"),
        Ok(other) => panic!("the stream was cut: {other:?}"),
    }
}

#[test]
fn the_hubs_log_filter_keeps_rmcp_at_warn() {
    // rmcp 3.5.0 logs every request it receives at `debug` (tool arguments included), every result at `debug` (file text included),
    // and notifications at `info` (D91). The hub's filter holds every library at `warn`; this keeps it that way for rmcp.
    let filter = hub::telemetry::filter();
    for target in [
        "rmcp",
        "rmcp::service",
        "rmcp::transport::streamable_http_server::tower",
        "rmcp::transport::streamable_http_server::session::local",
    ] {
        for level in [tracing::Level::TRACE, tracing::Level::DEBUG, tracing::Level::INFO] {
            assert!(!filter.would_enable(target, &level), "{target} at {level}");
        }
        assert!(
            filter.would_enable(target, &tracing::Level::WARN),
            "{target} at warn"
        );
    }
    // This crate's own target is the hub's (`hub` is a prefix of `hub_mcp`), at `info`; what it logs is ids and classes only.
    for target in ["hub_mcp", "hub_mcp::server::guard"] {
        assert!(filter.would_enable(target, &tracing::Level::INFO), "{target}");
        assert!(!filter.would_enable(target, &tracing::Level::DEBUG), "{target}");
    }
}
