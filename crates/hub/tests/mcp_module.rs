//! Prompt 07 (T1) and plan 18 (T6): the MCP module in the composed hub. The routes pass the hub's route-table check (S4), everything
//! else under `/mcp` is default deny, the challenge of a refused request survives the hub's error layer, a stream that outlives the
//! head timeout is not cut, and the hub's log filter keeps `rmcp` at `warn` (S10).
//!
//! The tests above the line `// the real composition` build the hub from port fakes and no database. The ones below it use the
//! hub's real layers over a real Postgres (`tests/support/identity_rig.rs`: sessions, the hub-wide `AuthLayer` and `CsrfLayer`, the
//! identity module's `HubTokenVerifier`, the real `Mcp::build`, the static web app on the fallback): they are what closes D109's
//! condition for S3 on the personal-token routes, and what shows that a request with a bearer token and no session is not asked
//! for a CSRF token while one that carries both is. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

#[path = "../../hub-platform/tests/support/clock.rs"]
mod clock;
#[path = "../../hub-identity/tests/support/fake_https.rs"]
mod fake_https;
#[path = "../../hub-identity/tests/support/mock_oidc.rs"]
mod mock_oidc;
#[path = "support/identity_rig.rs"]
mod rig;
#[path = "support/secrets.rs"]
mod secrets;
#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

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
use hub::{Deps, Hub, HubSettings, McpModule, SessionEdge};
use hub_mcp::auth::oauth::OAuthSettings;
use hub_mcp::config::{AuthMode, McpConfig, PublicOrigin};
use hub_mcp::{Credentials, McpDeps, McpSettings, McpState};
use hub_platform::{FakeSessionAuth, RouteSpec};
use ports::VerifiedUser;
use ports::conformance::route_guard::{
    Allowances, RegisteredRoute, SpecOperation, assert_route_table, spec_operations_from_openapi,
};
use ports::fakes::{FakeDocSearch, FakeRegistry, FakeTokenVerifier, FakeUsers, FakeWriteService};
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
        McpDeps::new(
            Credentials::Oauth(Arc::new(verifier)),
            Arc::new(users.clone()),
            Arc::new(FakeRegistry::new()),
            Arc::new(FakeDocSearch::new()),
            Arc::new(FakeWriteService::new()),
        ),
        settings,
    )
    .unwrap();
    let mut hub_settings = HubSettings::default();
    hub_settings.limits.request_timeout = request_timeout;
    let hub = Hub::build(Deps {
        settings: hub_settings,
        session: SessionEdge::without_sessions(Arc::new(FakeSessionAuth::new().with_user(
            "alice",
            Some(Role::Viewer),
            UserStatus::Active,
        ))),
        mounted: Vec::new(),
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

// ------------------------------------------------------------------------------------------------ the real composition

const RIG_HOST: &str = "lanekeeper.example.com";

/// A JSON-RPC request to `/mcp` as an MCP client sends it: a bearer token, the host the hub is called by, and no cookie.
fn rpc(token: &str, session: Option<&str>, body: &Value) -> rig::Req {
    let mut request = rig::Req::post("/mcp")
        .header("host", RIG_HOST)
        .header("accept", "application/json, text/event-stream")
        .header("authorization", &format!("Bearer {token}"));
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    request.json(body)
}

/// The JSON-RPC message in the event stream of a reply (rmcp answers a POST with one event, and may send an empty priming event first).
fn rpc_message(reply: &rig::Reply) -> Value {
    let text = reply.text();
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|data| data.starts_with('{'))
        .map_or_else(
            || panic!("no JSON-RPC message in {:?}: {text}", reply.status),
            |data| serde_json::from_str(data).unwrap(),
        )
}

/// An Entra access token for the hub API, as the mock identity provider signs it, for the user `n` of the rig.
fn entra_access_token(n: u32) -> String {
    mock_oidc::TokenSpec::valid("not-an-id-token")
        .claim("aud", json!("api://lanekeeper"))
        .claim("scp", json!("mcp.access"))
        .claim("oid", json!(format!("00000000-0000-4000-8000-{n:012x}")))
        .mint()
}

/// `initialize` and `notifications/initialized`: the session id of an MCP session that is open.
async fn open_session(app: &axum::Router, token: &str) -> String {
    let reply = rig::send(app, rpc(token, None, &initialize())).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let session = reply.header("mcp-session-id").expect("a session id").to_owned();
    let ready = rig::send(
        app,
        rpc(
            token,
            Some(&session),
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        ),
    )
    .await;
    assert_eq!(ready.status, StatusCode::ACCEPTED, "{}", ready.text());
    session
}

/// The rig's clock starts in 2027 so that the mock identity provider's tokens are valid. A personal token's life is set by the database's
/// own clock (`now()`, so that it satisfies the table's constraints whatever the hub's clock says), so a test of tokens made and used
/// puts the hub's clock on the database's, and uses no Entra token.
fn use_the_databases_time(rig: &rig::IdentityRig) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    rig.clock.set_ms(i64::try_from(now.as_millis()).unwrap());
}

async fn rig_with(mcp: rig::RigMcp) -> Option<rig::IdentityRig> {
    rig::IdentityRig::start(rig::Options {
        mcp: Some(mcp),
        ..rig::Options::default()
    })
    .await
}

/// D109's condition for S3 (closes D-2): the personal-token routes are behind the hub's CSRF layer. The route itself checks `Origin`
/// and nothing else, so a request with the cookies and the exact `Origin` but no double-submit token gets through the route's own
/// check, and only the layer stops it.
#[tokio::test]
async fn the_personal_token_routes_need_the_csrf_token() {
    let Some(rig) = rig_with(rig::RigMcp::personal_token()).await else {
        return;
    };
    let app = rig.app();
    let (_, cookies) = rig.signed_in(1, Some(Role::Viewer), UserStatus::Active).await;
    let label = json!({"label": "cursor"});
    let create = || rig::Req::post("/api/v1/me/mcp-tokens").json(&label);

    // The cookies and the right Origin, no token: the route's own check is satisfied and the layer refuses.
    let reply = rig::send(
        app,
        create().session(&cookies).header("origin", mock_oidc::ORIGIN),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    assert_eq!(reply.problem_code().as_deref(), Some("csrf"), "{}", reply.text());
    // A token and no Origin is refused too, by the layer first.
    let reply = rig::send(
        app,
        create().session(&cookies).header("x-csrf-token", &cookies.csrf),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.problem_code().as_deref(), Some("csrf"));
    // Nothing was made by either.
    let listed = rig::send(app, rig::Req::get("/api/v1/me/mcp-tokens").session(&cookies)).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(listed.json()["items"], json!([]));

    // With the token and the Origin the token is made, and shown once.
    let made = rig::send(app, create().with_token(&cookies)).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.text());
    let token = made.json()["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("lkpat_"), "a personal token has its prefix");
    let id = made.json()["id"].as_i64().unwrap();
    // The list never shows it again.
    let listed = rig::send(app, rig::Req::get("/api/v1/me/mcp-tokens").session(&cookies)).await;
    assert_eq!(listed.json()["items"].as_array().unwrap().len(), 1);
    assert!(!listed.text().contains(&token));

    // Revoking is state-changing too.
    let revoke = || rig::Req::new(Method::DELETE, &format!("/api/v1/me/mcp-tokens/{id}"));
    let reply = rig::send(
        app,
        revoke().session(&cookies).header("origin", mock_oidc::ORIGIN),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    assert_eq!(reply.problem_code().as_deref(), Some("csrf"));
    let reply = rig::send(app, revoke().with_token(&cookies)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text());
}

/// The token a signed-in person made is a bearer credential for `/mcp` and nothing else; the cookie that made it is not.
#[tokio::test]
async fn a_personal_token_made_through_a_session_opens_an_mcp_session_and_a_cookie_does_not() {
    let Some(rig) = rig_with(rig::RigMcp::personal_token()).await else {
        return;
    };
    use_the_databases_time(&rig);
    let app = rig.app();
    let (_, cookies) = rig.signed_in(1, Some(Role::Viewer), UserStatus::Active).await;
    let made = rig::send(
        app,
        rig::Req::post("/api/v1/me/mcp-tokens")
            .json(&json!({"label": "cursor"}))
            .with_token(&cookies),
    )
    .await;
    let token = made.json()["token"].as_str().unwrap().to_owned();

    // The bearer token alone: no cookie, no CSRF token, and none is asked for (a bearer request has no ambient authority).
    let session = open_session(app, &token).await;
    let listed = rig::send(
        app,
        rpc(
            &token,
            Some(&session),
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let message = rpc_message(&listed);
    let names: Vec<&str> = message["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    // A Viewer is shown the read tools, the docs tools among them (they answer `unavailable` while there is no search), and no write.
    assert!(
        names.contains(&"list_swimlanes") && names.contains(&"search_docs"),
        "{names:?}"
    );
    assert!(!names.contains(&"apply_change"), "{names:?}");

    // The session cookie is not a credential for `/mcp`: only a bearer token is.
    let cookie_only = rig::send(
        app,
        rig::Req::post("/mcp")
            .header("host", RIG_HOST)
            .header("accept", "application/json, text/event-stream")
            .json(&initialize())
            .with_token(&cookies),
    )
    .await;
    assert_eq!(
        cookie_only.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        cookie_only.text()
    );

    // A token revoked on this replica stops working at once.
    let id = made.json()["id"].as_i64().unwrap();
    let revoked = rig::send(
        app,
        rig::Req::new(Method::DELETE, &format!("/api/v1/me/mcp-tokens/{id}")).with_token(&cookies),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let again = rig::send(app, rpc(&token, None, &initialize())).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED, "{}", again.text());
}

/// OAuth mode, the default: an Entra access token for the hub API, with the `mcp.access` scope, opens a session. No cookie is sent
/// and none is asked for.
#[tokio::test]
async fn an_entra_access_token_opens_an_mcp_session_and_needs_no_csrf_token() {
    let Some(rig) = rig_with(rig::RigMcp::oauth()).await else {
        return;
    };
    let app = rig.app();
    rig.add_user(2, Some(Role::Viewer), UserStatus::Active).await;
    let token = entra_access_token(2);
    let session = open_session(app, &token).await;
    assert!(!session.is_empty());

    // The same token without the scope, for another audience, or for a user the hub does not know is refused at the gate.
    for (what, bad) in [
        (
            "no scope",
            mock_oidc::TokenSpec::valid("n")
                .claim("aud", json!("api://lanekeeper"))
                .claim("oid", json!("00000000-0000-4000-8000-000000000002"))
                .mint(),
        ),
        (
            "another audience",
            mock_oidc::TokenSpec::valid("n")
                .claim("aud", json!("api://another-app"))
                .claim("scp", json!("mcp.access"))
                .claim("oid", json!("00000000-0000-4000-8000-000000000002"))
                .mint(),
        ),
        ("an unknown user", entra_access_token(77)),
    ] {
        let reply = rig::send(app, rpc(&bad, None, &initialize())).await;
        assert!(
            matches!(reply.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "{what}: {} {}",
            reply.status,
            reply.text()
        );
    }
    // A pending user is a 403 whatever the token says (S1, S4).
    rig.add_user(3, None, UserStatus::Pending).await;
    let reply = rig::send(app, rpc(&entra_access_token(3), None, &initialize())).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
}

/// A request that carries a session cookie is a browser's, whatever else it carries: with a bearer token and no CSRF token it is
/// refused by the layer, and does not reach the MCP gate. (A client that sends a bearer token sends no cookie.)
#[tokio::test]
async fn a_bearer_request_that_also_carries_a_session_is_stopped_by_csrf() {
    let Some(rig) = rig_with(rig::RigMcp::oauth()).await else {
        return;
    };
    let app = rig.app();
    let (_, cookies) = rig.signed_in(2, Some(Role::Viewer), UserStatus::Active).await;
    let reply = rig::send(
        app,
        rpc(&entra_access_token(2), None, &initialize()).session(&cookies),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    assert_eq!(reply.problem_code().as_deref(), Some("csrf"));
    // And a state-changing request for a session that ends one (DELETE of an MCP session) is covered the same way.
    let reply = rig::send(
        app,
        rig::Req::new(Method::DELETE, "/mcp")
            .header("host", RIG_HOST)
            .header("authorization", &format!("Bearer {}", entra_access_token(2)))
            .header("mcp-session-id", "x")
            .session(&cookies),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.problem_code().as_deref(), Some("csrf"));
}

/// With the web app on the fallback, as the real hub has it, a path under the reserved prefixes that is not a route is the hub's own
/// `404` and never the app shell (G19): a client that mistypes `/mcp/` must not be handed HTML.
#[tokio::test]
async fn with_the_web_app_on_the_fallback_unknown_paths_under_the_mcp_prefixes_are_404_and_not_the_shell() {
    let Some(rig) = rig_with(rig::RigMcp::oauth()).await else {
        return;
    };
    let app = rig.app();
    rig.add_user(2, Some(Role::Viewer), UserStatus::Active).await;
    let token = entra_access_token(2);
    // The app shell answers an ordinary path (the fallback is really on), and nothing under the prefixes.
    let shell = rig::send(app, rig::Req::get("/swimlanes")).await;
    assert_eq!(shell.status, StatusCode::OK);
    assert!(shell.text().contains("<div id=\"root\">"));
    for path in [
        "/mcp/",
        "/mcp/x",
        "/.well-known/other",
        "/.well-known/oauth-protected-resource/other",
        "/auth/x",
    ] {
        for bearer in [None, Some(&token)] {
            let mut request = rig::Req::get(path).header("host", RIG_HOST);
            if let Some(bearer) = bearer {
                request = request.header("authorization", &format!("Bearer {bearer}"));
            }
            let reply = rig::send(app, request).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}: {}", reply.text());
            assert!(
                !reply.text().contains("<div id=\"root\">"),
                "{path} was answered with the app shell"
            );
        }
    }
    // The documents of OAuth discovery are open (a client reads them before it has a token).
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let reply = rig::send(app, rig::Req::get(path).header("host", RIG_HOST)).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(reply.json()["resource"], "https://lanekeeper.example.com/mcp");
    }
}

/// An open `/mcp` stream ends when the hub starts to stop, and not when the connections have drained: the server's stop task is
/// tied to the hub's `closing`.
#[tokio::test]
async fn an_open_mcp_stream_ends_when_the_hub_stops() {
    let Some(mut rig) = rig_with(rig::RigMcp::oauth()).await else {
        return;
    };
    rig.add_user(2, Some(Role::Viewer), UserStatus::Active).await;
    let token = entra_access_token(2);
    let app = rig.app().clone();
    let session = open_session(&app, &token).await;
    let response = rig::open(
        &app,
        rig::Req::get("/mcp")
            .header("host", RIG_HOST)
            .header("accept", "text/event-stream")
            .header("authorization", &format!("Bearer {token}"))
            .header("mcp-session-id", &session),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    // Idle and open: waiting on it times out and does not end.
    let waited = tokio::time::timeout(Duration::from_millis(500), body.frame()).await;
    match waited {
        Err(_elapsed) => {}
        Ok(Some(Ok(frame))) => assert!(frame.is_data(), "only data (the priming event) may arrive"),
        Ok(other) => panic!("the stream ended before the hub stopped: {other:?}"),
    }
    rig.replicas[0].shut_down().await;
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the stream was still open ten seconds after the hub began to stop"
    );
}
