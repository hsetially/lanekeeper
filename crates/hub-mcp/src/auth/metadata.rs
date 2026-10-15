//! OAuth discovery documents (RFC 9728, RFC 8414), OAuth mode only.
//!
//! Personal-token mode registers none of these routes, so there is no code path that could publish OAuth metadata in that mode
//! (D52, T1). The documents are built once from validated settings and are public by design: they hold URLs and scope names, no
//! secret.

use std::sync::Arc;

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, Method, header};
use hub_platform::GuardedRouter;
use serde_json::{Value, json};

use super::oauth::{
    AUTHORIZATION_SERVER_PATH, OAuthSettings, RESOURCE_METADATA_PATH, RESOURCE_METADATA_ROOT_PATH,
};
use crate::config::PublicOrigin;

/// The reason the route table gives for these routes being open.
const REASON: &str =
    "OAuth discovery metadata (RFC 9728): static, public by design, no secret and no user data";

/// The protected resource metadata of `/mcp`: which authorization server issues its tokens, and the scopes.
#[must_use]
pub fn protected_resource(origin: &PublicOrigin, settings: &OAuthSettings) -> Value {
    json!({
        "resource": origin.url("/mcp"),
        "authorization_servers": [settings.issuer()],
        "scopes_supported": [settings.scope(), "offline_access"],
        "bearer_methods_supported": ["header"],
        "resource_name": "Lanekeeper",
    })
}

/// A static copy of the fields of Entra's v2 discovery document that a client needs to start the code flow with PKCE. Built from
/// the tenant id alone and never fetched, so the hub makes no outbound call for it. Compare it with the live document (T0 runbook).
#[must_use]
pub fn authorization_server(settings: &OAuthSettings) -> Value {
    let base = format!("https://{}/{}", settings.authority_host, settings.tenant_id);
    json!({
        "issuer": settings.issuer(),
        "authorization_endpoint": format!("{base}/oauth2/v2.0/authorize"),
        "token_endpoint": format!("{base}/oauth2/v2.0/token"),
        "jwks_uri": format!("{base}/discovery/v2.0/keys"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [settings.scope(), "offline_access", "openid", "profile"],
    })
}

/// The paths this module registers, for the route table.
#[must_use]
pub fn paths(settings: &OAuthSettings) -> Vec<&'static str> {
    let mut paths = vec![RESOURCE_METADATA_ROOT_PATH, RESOURCE_METADATA_PATH];
    if settings.authorization_server_mirror {
        paths.push(AUTHORIZATION_SERVER_PATH);
    }
    paths
}

/// The reason text used for the open routes.
#[must_use]
pub const fn reason() -> &'static str {
    REASON
}

fn document(body: &Arc<Value>) -> Response {
    let mut response = Json(Value::clone(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Register the discovery routes. Call in OAuth mode only.
#[must_use]
pub fn mount(router: GuardedRouter, origin: &PublicOrigin, settings: &OAuthSettings) -> GuardedRouter {
    let resource = Arc::new(protected_resource(origin, settings));
    let mut router = router;
    for path in [RESOURCE_METADATA_ROOT_PATH, RESOURCE_METADATA_PATH] {
        let body = Arc::clone(&resource);
        router = router.public(&Method::GET, path, REASON, move || {
            let body = Arc::clone(&body);
            async move { document(&body) }
        });
    }
    if settings.authorization_server_mirror {
        let mirror = Arc::new(authorization_server(settings));
        router = router.public(&Method::GET, AUTHORIZATION_SERVER_PATH, REASON, move || {
            let body = Arc::clone(&mirror);
            async move { document(&body) }
        });
    }
    router
}
