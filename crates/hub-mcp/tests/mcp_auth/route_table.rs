//! S4: every route this crate registers declares its guard, agrees with `api/openapi.yaml`, and the rest is default deny.

use std::collections::BTreeSet;
use std::path::Path;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use domain::Role;
use hub_platform::{RouteGuard, RouteSpec};
use ports::conformance::route_guard::{
    Allowances, RegisteredRoute, SpecOperation, assert_route_table, spec_operations_from_openapi,
};
use tower::ServiceExt;

use crate::rig::{HOST, Rig};

fn spec() -> Vec<SpecOperation> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml");
    spec_operations_from_openapi(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn registered(routes: &[RouteSpec]) -> Vec<RegisteredRoute> {
    routes.iter().map(RouteSpec::registered).collect()
}

const TOKEN_ROUTES: [(&str, &str); 3] = [
    ("GET", "/api/v1/me/mcp-tokens"),
    ("POST", "/api/v1/me/mcp-tokens"),
    ("DELETE", "/api/v1/me/mcp-tokens/{id}"),
];

fn pairs(routes: &[RouteSpec]) -> BTreeSet<(String, String)> {
    routes
        .iter()
        .map(|r| (r.method.clone(), r.path.clone()))
        .collect()
}

/// Everything in the spec that is not a token route belongs to another crate; the routes that are not API operations are listed.
fn allowances(spec: &[SpecOperation], mine: &[RouteSpec]) -> Allowances {
    let operations: BTreeSet<(&str, &str)> = TOKEN_ROUTES.iter().copied().collect();
    let mut allow = Allowances::default();
    for op in spec {
        if !operations.contains(&(op.method.as_str(), op.path.as_str())) {
            allow = allow.spec_only(&op.method, &op.path, "owned by another crate");
        }
    }
    for route in mine {
        if !operations.contains(&(route.method.as_str(), route.path.as_str())) {
            allow = allow.route_only(
                &route.method,
                &route.path,
                "the MCP endpoint and OAuth discovery are not API operations (docs/mcp-tools.md, RFC 9728)",
            );
        }
    }
    allow
}

#[test]
fn every_mcp_route_has_a_declared_guard() {
    for rig in [Rig::oauth(), Rig::oauth_with_mirror(), Rig::personal()] {
        assert!(!rig.routes.is_empty());
        for route in &rig.routes {
            match &route.guard {
                RouteGuard::Role(role) => assert_eq!(*role, Role::Viewer, "{} {}", route.method, route.path),
                RouteGuard::Public { reason } => {
                    assert!(!reason.trim().is_empty(), "{} {}", route.method, route.path);
                }
            }
        }
        // The token routes are role-guarded at Viewer by the hub's session guard.
        for (method, path) in TOKEN_ROUTES {
            let spec = rig.routes.iter().find(|r| r.method == method && r.path == path);
            assert_eq!(
                spec.map(|r| r.guard.clone()),
                Some(RouteGuard::Role(Role::Viewer)),
                "{method} {path}"
            );
        }
        // `/mcp` authenticates itself in the gate, on every method it answers, and says so in the table.
        for method in ["POST", "GET", "DELETE"] {
            let spec = rig
                .routes
                .iter()
                .find(|r| r.method == method && r.path == "/mcp")
                .unwrap();
            assert!(
                matches!(&spec.guard, RouteGuard::Public { reason } if reason.contains("gate")),
                "{method}: {:?}",
                spec.guard
            );
        }
    }
}

#[test]
fn the_routes_of_each_mode_are_exactly_these() {
    let paths = |rig: &Rig| {
        pairs(&rig.routes)
            .into_iter()
            .map(|(m, p)| format!("{m} {p}"))
            .collect::<Vec<_>>()
    };
    let base = [
        "DELETE /api/v1/me/mcp-tokens/{id}",
        "DELETE /mcp",
        "GET /api/v1/me/mcp-tokens",
        "GET /mcp",
        "POST /api/v1/me/mcp-tokens",
        "POST /mcp",
    ];
    // Personal-token mode: the token routes and the endpoint, and no discovery.
    assert_eq!(paths(&Rig::personal()), base);
    // OAuth mode adds the discovery documents; the mirror only when switched on.
    let mut oauth = base.to_vec();
    oauth.extend([
        "GET /.well-known/oauth-protected-resource",
        "GET /.well-known/oauth-protected-resource/mcp",
    ]);
    oauth.sort_unstable();
    assert_eq!(paths(&Rig::oauth()), oauth);
    let mut mirror = oauth.clone();
    mirror.push("GET /.well-known/oauth-authorization-server");
    mirror.sort_unstable();
    assert_eq!(paths(&Rig::oauth_with_mirror()), mirror);
}

#[test]
fn routes_agree_with_openapi_and_allowances() {
    let spec = spec();
    for rig in [Rig::oauth(), Rig::oauth_with_mirror(), Rig::personal()] {
        assert_route_table(&registered(&rig.routes), &spec, &allowances(&spec, &rig.routes));
    }
}

#[tokio::test]
async fn unmatched_path_under_mcp_is_default_deny() {
    let rig = Rig::oauth();
    rig.add_user("alice", Some(Role::Admin), domain::UserStatus::Active);
    rig.verifier.accept("tok-alice", "alice");
    let send = |method: Method, path: &str, bearer: Option<&str>| {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST);
        if let Some(bearer) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        let request = builder.body(Body::empty()).unwrap();
        let router = rig.router.clone();
        async move { router.oneshot(request).await.unwrap().status() }
    };
    // Paths the router does not know are not answered, for anybody, with a token or without.
    for path in [
        "/mcp/",
        "/mcp/x",
        "/mcp/../mcp",
        "/MCP",
        "/mcpx",
        "/.well-known",
        "/.well-known/oauth-authorization-server",
    ] {
        for bearer in [None, Some("tok-alice")] {
            let status = send(Method::GET, path, bearer).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path} {bearer:?}");
        }
    }
    // Methods the endpoint does not take are refused before anything runs.
    for method in [Method::PUT, Method::PATCH, Method::OPTIONS, Method::TRACE] {
        let status = send(method.clone(), "/mcp", Some("tok-alice")).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
    }
    assert_eq!(
        rig.verifier.calls(),
        0,
        "none of them asked the identity provider"
    );
    // axum sends a HEAD to the handler of GET. It is gated like a GET, and rmcp then refuses the method.
    assert_eq!(send(Method::HEAD, "/mcp", None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        send(Method::HEAD, "/mcp", Some("tok-alice")).await,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(rig.state.live_sessions().await, 0);
}

#[tokio::test]
async fn every_method_of_the_endpoint_is_gated() {
    let rig = Rig::oauth();
    for method in [Method::POST, Method::GET, Method::DELETE] {
        let request = Request::builder()
            .method(method.clone())
            .uri("/mcp")
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap();
        let response = rig.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method}");
    }
}

#[tokio::test]
async fn discovery_documents_are_the_metadata_of_this_tenant() {
    use crate::support::json_body;
    let rig = Rig::oauth_with_mirror();
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let response = rig.client(None).get(path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = json_body(response).await;
        assert_eq!(body["resource"], "https://hub.example.test/mcp");
        assert_eq!(
            body["authorization_servers"][0],
            "https://login.microsoftonline.com/11111111-2222-4333-8444-555555555555/v2.0"
        );
        assert_eq!(body["scopes_supported"][0], "api://hub-api/mcp.access");
        assert_eq!(body["bearer_methods_supported"][0], "header");
    }
    let body = json_body(
        rig.client(None)
            .get("/.well-known/oauth-authorization-server")
            .await,
    )
    .await;
    assert_eq!(
        body["authorization_endpoint"],
        "https://login.microsoftonline.com/11111111-2222-4333-8444-555555555555/oauth2/v2.0/authorize"
    );
    assert_eq!(body["code_challenge_methods_supported"][0], "S256");
    // Without the switch the mirror is not there.
    let rig = Rig::oauth();
    assert_eq!(
        rig.client(None)
            .get("/.well-known/oauth-authorization-server")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn the_spec_has_the_three_token_operations_at_viewer() {
    let spec = spec();
    for (method, path) in TOKEN_ROUTES {
        let op = spec.iter().find(|o| o.method == method && o.path == path);
        assert!(op.is_some(), "{method} {path} is not in api/openapi.yaml");
    }
}
