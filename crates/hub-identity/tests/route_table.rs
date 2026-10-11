//! T4 (S4): the route table. Every route the identity component registers has a declared guard, the guard agrees with the
//! `x-required-role` of `api/openapi.yaml`, and the check is exported for the hub binary to run against its whole table.
//!
//! None of these tests needs a database: the router's pool never connects, and a request without a cookie never reaches it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use domain::{Role, Timestamp};
use hub_identity::csrf::{CsrfExemption, CsrfPolicy};
use hub_identity::guard::{Access, GuardedRouter, RegisterError, RouteEntry, Verb};
use hub_identity::session::{SessionPolicy, Sessions};
use hub_identity::testing::{assert_default_deny, assert_hub_route_table, registered_routes};
use hub_identity::{Clock, ManualClock};
use ports::conformance::route_guard::{
    Allowances, Guard, RegisteredRoute, SpecOperation, Violation, check_route_table,
    spec_operations_from_openapi,
};
use support::guard_rig::{assert_problem, lazy_app, lazy_pool};
use support::mock_oidc::NOW_MS;
use support::web_rig::{ORIGIN, Req, send};

const OPENAPI: &str = include_str!("../../../api/openapi.yaml");

/// The operations this crate owns: method, path and the role the spec requires.
const IDENTITY_OPERATIONS: [(&str, &str, Option<Role>); 10] = [
    ("GET", "/auth/login", None),
    ("GET", "/auth/callback", None),
    ("POST", "/auth/logout", None),
    ("GET", "/api/v1/me", Some(Role::Viewer)),
    ("POST", "/api/v1/me/access-request", Some(Role::Viewer)),
    ("GET", "/api/v1/admin/users", Some(Role::Admin)),
    ("PATCH", "/api/v1/admin/users/{id}", Some(Role::Admin)),
    ("GET", "/api/v1/admin/access-requests", Some(Role::Admin)),
    (
        "POST",
        "/api/v1/admin/access-requests/{id}/approve",
        Some(Role::Admin),
    ),
    (
        "POST",
        "/api/v1/admin/access-requests/{id}/reject",
        Some(Role::Admin),
    ),
];

/// Everything in the spec that is not one of ours is somebody else's: allow it to be missing from this table.
fn allowances_for_foreign_operations(
    spec: &[SpecOperation],
    ours: &BTreeSet<(String, String)>,
) -> Allowances {
    spec.iter()
        .filter(|o| !ours.contains(&(o.method.clone(), o.path.clone())))
        .fold(Allowances::default(), |a, o| {
            a.spec_only(
                &o.method,
                &o.path,
                "owned by another prompt; its crate registers it",
            )
        })
}

fn ours() -> BTreeSet<(String, String)> {
    IDENTITY_OPERATIONS
        .iter()
        .map(|(m, p, _)| ((*m).to_owned(), (*p).to_owned()))
        .collect()
}

#[tokio::test]
async fn route_table_matches_openapi_and_has_no_unguarded_route() {
    let (_router, table) = lazy_app().await;
    let spec = spec_operations_from_openapi(OPENAPI).unwrap();
    let allow = allowances_for_foreign_operations(&spec, &ours());
    // Panics with every violation if a route is unguarded, has another role than the spec, or is missing or extra.
    assert_hub_route_table(&table, OPENAPI, &allow);

    let registered: BTreeSet<(String, String)> =
        table.iter().map(|r| (r.method.clone(), r.path.clone())).collect();
    assert_eq!(
        registered,
        ours(),
        "the identity component registers exactly its ten operations"
    );
}

#[tokio::test]
async fn registered_role_equals_x_required_role_for_every_identity_operation() {
    let (_router, table) = lazy_app().await;
    for (method, path, role) in IDENTITY_OPERATIONS {
        let entry = table
            .iter()
            .find(|r| r.method == method && r.path == path)
            .unwrap_or_else(|| panic!("{method} {path} is not registered"));
        match (role, &entry.access) {
            (Some(want), Access::Role(have)) => assert_eq!(*have, want, "{method} {path}"),
            (None, Access::Public { reason }) => {
                assert!(reason.len() >= 10, "{method} {path}: a public route says why");
            }
            (want, have) => panic!("{method} {path}: spec {want:?}, registered {have:?}"),
        }
    }
}

#[tokio::test]
async fn exported_assert_hub_route_table_is_callable_with_foreign_routes() {
    // The hub binary composes the routes of several crates and calls the exported check on the whole table. Here a router
    // that is not ours registers a read route and a webhook, and the check judges them against the same spec.
    let pool = lazy_pool();
    let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(NOW_MS)));
    let sessions = Arc::new(Sessions::new(
        pool,
        Arc::clone(&clock) as Arc<dyn Clock>,
        &SessionPolicy::default(),
    ));
    let hook = CsrfExemption::new("HMAC over the raw body, checked in the handler").unwrap();
    let g = GuardedRouter::new(sessions, ORIGIN)
        .get("/api/v1/blobs/{hash}", Role::Viewer, (), || async { "blob" })
        .public(
            Verb::Post,
            "/hooks/github",
            "authenticated by an HMAC over the body, not by a session",
            CsrfPolicy::Exempt(hook),
            (),
            || async { "ok" },
        );
    let table = g.route_table().to_vec();
    let spec = spec_operations_from_openapi(OPENAPI).unwrap();
    let theirs: BTreeSet<(String, String)> = [
        ("GET".to_owned(), "/api/v1/blobs/{hash}".to_owned()),
        ("POST".to_owned(), "/hooks/github".to_owned()),
    ]
    .into();
    let allow = allowances_for_foreign_operations(&spec, &theirs);
    assert_hub_route_table(&table, OPENAPI, &allow);
    assert!(g.into_router().is_ok());
}

#[test]
#[should_panic(expected = "guarded as admin but the spec says viewer")]
fn a_role_that_disagrees_with_the_spec_fails_the_check() {
    let table = vec![RouteEntry {
        method: "GET".to_owned(),
        path: "/api/v1/me".to_owned(),
        access: Access::Role(Role::Admin),
    }];
    let spec = spec_operations_from_openapi(OPENAPI).unwrap();
    let allow = allowances_for_foreign_operations(
        &spec,
        &BTreeSet::from([("GET".to_owned(), "/api/v1/me".to_owned())]),
    );
    assert_hub_route_table(&table, OPENAPI, &allow);
}

#[test]
fn an_unguarded_route_is_reported() {
    // `GuardedRouter` cannot make an unguarded entry; the harness still must catch one that arrives some other way.
    let routes = [RegisteredRoute::new("GET", "/api/v1/me", Guard::Unguarded)];
    let spec = spec_operations_from_openapi(OPENAPI).unwrap();
    let allow = allowances_for_foreign_operations(
        &spec,
        &BTreeSet::from([("GET".to_owned(), "/api/v1/me".to_owned())]),
    );
    let found = check_route_table(&routes, &spec, &allow);
    assert!(
        found
            .iter()
            .any(|v| matches!(v, Violation::Unguarded { path, .. } if path == "/api/v1/me")),
        "{found:?}"
    );
    // And our own conversion of a real table produces no unguarded entry.
    let converted = registered_routes(&[RouteEntry {
        method: "GET".to_owned(),
        path: "/x".to_owned(),
        access: Access::Role(Role::Viewer),
    }]);
    assert_eq!(converted[0].guard, Guard::Role(Role::Viewer));
}

#[tokio::test]
async fn default_deny_every_registered_path_without_cookie_is_401() {
    let (router, table) = lazy_app().await;
    // Checks every non-public route with no cookie and the unregistered path; panics on any other answer.
    assert_default_deny(&router, &table).await;
}

#[tokio::test]
async fn unmatched_path_without_session_is_401_not_404() {
    let (router, _table) = lazy_app().await;
    for (method, path) in [
        (Method::GET, "/api/v1/nothing-here"),
        (Method::POST, "/api/v1/nothing-here"),
        (Method::GET, "/"),
        (Method::DELETE, "/auth/login"),
    ] {
        let a = send(&router, Req::new(method.clone(), path)).await;
        // The wrong method on a public route is a 405 from the method router, never a 404 that maps the site.
        if path == "/auth/login" {
            assert!(
                a.status == StatusCode::UNAUTHORIZED || a.status == StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path}: {}",
                a.status
            );
        } else {
            assert_problem(&a, StatusCode::UNAUTHORIZED, "unauthenticated");
        }
    }
}

#[tokio::test]
async fn fallback_route_is_guarded() {
    // The fallback sits behind the auth layer: its answer carries the request id the layer stamps, and it never says 404 to
    // a caller who is not signed in.
    let (router, _table) = lazy_app().await;
    let a = send(&router, Req::get("/definitely/not/registered")).await;
    assert_problem(&a, StatusCode::UNAUTHORIZED, "unauthenticated");
    assert!(
        a.headers.contains_key("x-request-id"),
        "the auth layer wraps the fallback"
    );
    assert_eq!(
        a.json()["request_id"],
        a.headers["x-request-id"].to_str().unwrap()
    );
}

#[tokio::test]
async fn a_wrong_method_on_a_guarded_path_without_session_is_401() {
    let (router, _table) = lazy_app().await;
    for (method, path) in [
        (Method::DELETE, "/api/v1/me"),
        (Method::PUT, "/api/v1/admin/users"),
        (Method::GET, "/api/v1/admin/access-requests/1/approve"),
    ] {
        let a = send(&router, Req::new(method.clone(), path)).await;
        assert_problem(&a, StatusCode::UNAUTHORIZED, "unauthenticated");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The registration rules

fn empty_guarded() -> GuardedRouter {
    let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(NOW_MS)));
    let sessions = Arc::new(Sessions::new(
        lazy_pool(),
        Arc::clone(&clock) as Arc<dyn Clock>,
        &SessionPolicy::default(),
    ));
    GuardedRouter::new(sessions, ORIGIN)
}

#[tokio::test]
async fn public_route_needs_a_reason() {
    for reason in ["", "   ", "open", "short"] {
        let g = empty_guarded().public(Verb::Get, "/open", reason, CsrfPolicy::Required, (), || async {
            "x"
        });
        assert_eq!(
            g.into_router().unwrap_err(),
            RegisterError::PublicWithoutReason,
            "{reason:?}"
        );
    }
    let g = empty_guarded().public(
        Verb::Get,
        "/open",
        "the health probe has no session to check",
        CsrfPolicy::Required,
        (),
        || async { "x" },
    );
    assert!(g.into_router().is_ok());
}

#[tokio::test]
async fn a_route_registered_twice_is_refused_not_a_panic() {
    let g = empty_guarded()
        .get("/a/{id}", Role::Viewer, (), || async { "1" })
        .get("/a/{id}", Role::Admin, (), || async { "2" });
    assert_eq!(g.into_router().unwrap_err(), RegisterError::Duplicate);
    // The same shape under another parameter name would make axum panic: it is refused too.
    let g = empty_guarded()
        .get("/a/{id}", Role::Viewer, (), || async { "1" })
        .post("/a/{name}", Role::Viewer, (), || async { "2" });
    assert_eq!(g.into_router().unwrap_err(), RegisterError::Duplicate);
    // Different methods on one path are fine, each with its own role.
    let g = empty_guarded()
        .get("/a/{id}", Role::Viewer, (), || async { "1" })
        .patch("/a/{id}", Role::Admin, (), || async { "2" });
    assert_eq!(g.route_table().len(), 2);
    assert!(g.into_router().is_ok());
}

#[tokio::test]
async fn paths_that_would_make_axum_panic_are_refused() {
    for path in [
        "",
        "a",
        "/a//b",
        "/a/{}",
        "/a/{x",
        "/a/x}",
        "/a/{*rest}",
        "/a/*",
        "/a b",
        "/a?b",
        "/a/{id}x",
        "/ä",
    ] {
        let g = empty_guarded().get(path, Role::Viewer, (), || async { "x" });
        assert_eq!(g.into_router().unwrap_err(), RegisterError::BadPath, "{path:?}");
    }
}

#[tokio::test]
async fn the_first_registration_error_is_the_one_reported() {
    let g = empty_guarded()
        .get("bad", Role::Viewer, (), || async { "x" })
        .public(Verb::Get, "/open", "", CsrfPolicy::Required, (), || async { "x" });
    assert_eq!(g.into_router().unwrap_err(), RegisterError::BadPath);
}
