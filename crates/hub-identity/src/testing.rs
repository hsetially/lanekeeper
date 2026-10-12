//! Checks the hub binary runs against its full route table (S4). Behind the `route-table` feature: it pulls the ports
//! conformance suite and its `OpenAPI` reader, which the production build does not need.
//!
//! - [`assert_hub_route_table`]: every route has a guard, the guard's role equals `x-required-role` in `api/openapi.yaml`,
//!   and nothing is registered that the spec does not know (or the spec lists, unless an [`Allowances`] entry says why).
//! - [`assert_default_deny`]: every registered path, called with no cookie (and with a cookie that is not a session), answers
//!   `401`, and so does a path nobody registered. This is the check that a function-based registry needs in place of a macro:
//!   a route that got around the guard answers something else.
//!
//! These functions panic with every problem they find, as test helpers do.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, assert_route_table, spec_operations_from_openapi,
};
use tower::ServiceExt;

use crate::guard::{Access, RouteEntry};

/// A route table in the form the ports' harness takes.
pub fn registered_routes(entries: &[RouteEntry]) -> Vec<RegisteredRoute> {
    entries
        .iter()
        .map(|e| {
            let guard = match &e.access {
                Access::Role(r) => Guard::Role(*r),
                Access::Public { reason } => Guard::Public {
                    reason: (*reason).to_owned(),
                },
            };
            RegisteredRoute::new(&e.method, &e.path, guard)
        })
        .collect()
}

/// Panic unless the route table agrees with the spec. `allow` lists, with reasons, the spec operations this binary does not
/// register and the routes that are not API operations.
///
/// # Panics
/// With every violation found, one per line, or if the spec cannot be read.
pub fn assert_hub_route_table(entries: &[RouteEntry], openapi_yaml: &str, allow: &Allowances) {
    let spec = spec_operations_from_openapi(openapi_yaml).unwrap_or_else(|e| panic!("{e}"));
    assert_route_table(&registered_routes(entries), &spec, allow);
}

/// `{id}` and the like, filled in with something harmless.
fn concrete(path: &str) -> String {
    path.split('/')
        .map(|s| if s.starts_with('{') { "x" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// Panic unless every non-public registered route, and an unregistered path, answers `401` to a caller with no session.
///
/// # Panics
/// With every route that answered something else.
pub async fn assert_default_deny(router: &Router, entries: &[RouteEntry]) {
    let mut failures = Vec::new();
    let mut probes: Vec<(Method, String)> = entries
        .iter()
        .filter(|e| matches!(e.access, Access::Role(_)))
        .map(|e| {
            (
                Method::from_bytes(e.method.as_bytes()).expect("registered methods are valid"),
                concrete(&e.path),
            )
        })
        .collect();
    probes.push((Method::GET, "/__lanekeeper/not-a-route".to_owned()));
    for (method, path) in probes {
        // No cookie at all, and a cookie that is not a session token (answered without a database).
        for cookie in [None, Some("__Host-lk_session=garbage")] {
            let mut builder = Request::builder().method(method.clone()).uri(&path);
            if let Some(c) = cookie {
                builder = builder.header(header::COOKIE, c);
            }
            let request = builder.body(Body::empty()).expect("a valid request");
            let response = router.clone().oneshot(request).await.expect("infallible");
            if response.status() != StatusCode::UNAUTHORIZED {
                failures.push(format!(
                    "{method} {path} (cookie: {}) answered {} instead of 401",
                    cookie.is_some(),
                    response.status()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "default deny violated:\n{}",
        failures.join("\n")
    );
}
