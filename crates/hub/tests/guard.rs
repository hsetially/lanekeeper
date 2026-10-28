//! S4 (T1): the route guard. Pending and disabled users never pass, whatever their role; the role floor is the route's.
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

use axum::body::Body;
use domain::{Role, UserStatus};
use http::{Method, StatusCode};
use hub::{Deps, Hub, HubSettings, Module, SessionEdge};
use hub_platform::{FakeSessionAuth, GuardedRouter, RouteError, RouteGuard};
use support::{anonymous, default_hub, request, request_as, send};

#[tokio::test]
async fn guard_requires_an_active_user_with_the_routes_role() {
    let t = default_hub();
    let w = &t.hub.web;
    let status = |user: &str, method: Method, path: &str| {
        let r = request_as(user, [198, 51, 100, 7], method, path, Body::empty());
        let w = w.clone();
        async move { send(&w, r).await.status() }
    };
    assert_eq!(
        status("viewer", Method::GET, "/api/v1/ping").await,
        StatusCode::OK
    );
    assert_eq!(
        status("viewer", Method::POST, "/api/v1/edit").await,
        StatusCode::FORBIDDEN,
        "a viewer cannot edit"
    );
    assert_eq!(
        status("editor", Method::POST, "/api/v1/edit").await,
        StatusCode::OK
    );
    assert_eq!(
        status("editor", Method::GET, "/api/v1/ping").await,
        StatusCode::OK,
        "a higher role passes a lower floor"
    );
    for user in ["pending", "disabled", "norole"] {
        assert_eq!(
            status(user, Method::GET, "/api/v1/ping").await,
            StatusCode::FORBIDDEN,
            "{user}"
        );
    }
    assert_eq!(
        status("nobody", Method::GET, "/api/v1/ping").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(w, anonymous(Method::GET, "/api/v1/ping", Body::empty()))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_user_who_is_disabled_after_signing_in_is_refused_on_the_next_request() {
    let t = default_hub();
    assert_eq!(
        send(&t.hub.web, request(Method::GET, "/api/v1/ping", Body::empty()))
            .await
            .status(),
        StatusCode::OK
    );
    t.auth.set("viewer", Some(Role::Viewer), UserStatus::Disabled);
    assert_eq!(
        send(&t.hub.web, request(Method::GET, "/api/v1/ping", Body::empty()))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    t.auth.remove("viewer");
    assert_eq!(
        send(&t.hub.web, request(Method::GET, "/api/v1/ping", Body::empty()))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_guard_is_not_reachable_around_through_another_method() {
    let t = default_hub();
    // HEAD and OPTIONS on a guarded route do not reach the handler unauthenticated.
    for method in [Method::HEAD, Method::OPTIONS, Method::PUT, Method::PATCH] {
        let response = send(
            &t.hub.web,
            anonymous(method.clone(), "/api/v1/ping", Body::empty()),
        )
        .await;
        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{method}: {}",
            response.status()
        );
    }
}

struct Registers(fn(GuardedRouter) -> GuardedRouter);

impl Module for Registers {
    fn name(&self) -> &'static str {
        "registers"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        (self.0)(router)
    }
}

fn build_with(f: fn(GuardedRouter) -> GuardedRouter) -> Result<Hub, hub::BuildError> {
    Hub::build(Deps {
        settings: HubSettings::default(),
        session: SessionEdge::without_sessions(Arc::new(FakeSessionAuth::new())),
        mounted: Vec::new(),
        modules: vec![Arc::new(Registers(f))],
        static_files: None,
        checks: Vec::new(),
    })
}

#[test]
fn a_duplicate_route_is_a_build_error_not_a_panic() {
    let error = build_with(|r| {
        r.route(&Method::GET, "/a", Role::Viewer, || async { "1" }).route(
            &Method::GET,
            "/a",
            Role::Admin,
            || async { "2" },
        )
    })
    .unwrap_err();
    assert_eq!(
        error,
        hub::BuildError::Route {
            module: "registers",
            error: RouteError::Duplicate {
                method: "GET".into(),
                path: "/a".into()
            }
        }
    );
}

#[test]
fn a_public_route_without_a_reason_and_a_bad_path_are_build_errors() {
    let error = build_with(|r| r.public(&Method::POST, "/hooks/x", "  ", || async { "1" })).unwrap_err();
    assert!(matches!(
        error,
        hub::BuildError::Route {
            error: RouteError::PublicWithoutReason { .. },
            ..
        }
    ));
    let error =
        build_with(|r| r.route(&Method::GET, "no-slash", Role::Viewer, || async { "1" })).unwrap_err();
    assert!(matches!(
        error,
        hub::BuildError::Route {
            error: RouteError::Path { .. },
            ..
        }
    ));
    // A method axum cannot route is refused when the hub is built.
    let error = build_with(|r| {
        r.route(
            &Method::from_bytes(b"BREW").unwrap(),
            "/c",
            Role::Viewer,
            || async { "1" },
        )
    })
    .unwrap_err();
    assert!(matches!(
        error,
        hub::BuildError::Route {
            error: RouteError::Method { .. },
            ..
        }
    ));
}

#[test]
fn the_route_table_lists_every_mounted_route_with_its_guard() {
    let t = default_hub();
    let find = |m: &str, p: &str| {
        t.hub
            .routes
            .iter()
            .find(|r| r.method == m && r.path == p)
            .unwrap()
    };
    assert_eq!(find("GET", "/api/v1/ping").guard, RouteGuard::Role(Role::Viewer));
    assert_eq!(find("POST", "/api/v1/edit").guard, RouteGuard::Role(Role::Editor));
    assert!(
        matches!(&find("POST", "/hooks/github").guard, RouteGuard::Public { reason } if !reason.is_empty())
    );
    // And every one of them passes the S4 harness's own check for an unguarded or reason-less route.
    let registered: Vec<_> = t
        .hub
        .routes
        .iter()
        .map(hub_platform::RouteSpec::registered)
        .collect();
    let violations = ports::conformance::route_guard::check_route_table(
        &registered,
        &[],
        &t.hub
            .routes
            .iter()
            .fold(ports::conformance::route_guard::Allowances::default(), |a, r| {
                a.route_only(&r.method, &r.path, "test module")
            }),
    );
    assert!(violations.is_empty(), "{violations:?}");
}

// ------------------------------------------------------------------------------------------------ mounted routers (plan 18, T4)

fn mounted(name: &'static str, path: &'static str, fallback: &'static str) -> hub::MountedRouter {
    let router = axum::Router::new()
        .route(path, axum::routing::get(|| async { "mounted" }))
        .fallback(move || async move { fallback });
    hub::MountedRouter {
        name,
        router,
        specs: vec![hub_platform::RouteSpec::new(
            &Method::GET,
            path,
            RouteGuard::Public {
                reason: "a route of a router built elsewhere, for the test".to_owned(),
            },
        )],
    }
}

fn build_mounted(routers: Vec<hub::MountedRouter>) -> Result<Hub, hub::BuildError> {
    Hub::build(Deps {
        settings: HubSettings::default(),
        session: SessionEdge::without_sessions(Arc::new(FakeSessionAuth::new())),
        mounted: routers,
        modules: Vec::new(),
        static_files: None,
        checks: Vec::new(),
    })
}

#[tokio::test]
async fn routers_built_elsewhere_are_merged_and_their_own_fallbacks_never_answer() {
    // Two routers that each answer an unmatched path themselves: `Router::merge` would panic on the second, and adopt the first's
    // for every module. The hub drops both, so its own answer is the only one, whichever module a path was meant for.
    let hub = build_mounted(vec![
        mounted("one", "/one", "one's fallback"),
        mounted("two", "/two", "two's fallback"),
    ])
    .unwrap();
    assert_eq!(hub.routes.len(), 2, "both routers' tables are in the hub's");
    for path in ["/one", "/two"] {
        let reply = send(&hub.web, anonymous(Method::GET, path, Body::empty())).await;
        assert_eq!(reply.status(), StatusCode::OK, "{path}");
    }
    for path in ["/three", "/one/x", "/api/v1/none"] {
        let reply = send(&hub.web, anonymous(Method::GET, path, Body::empty())).await;
        assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{path}");
        let body = support::text(reply).await;
        assert!(
            !body.contains("fallback"),
            "{path}: a mounted router answered: {body}"
        );
    }
}

#[test]
fn a_mounted_router_that_repeats_a_route_is_refused_and_the_module_is_named() {
    let error = build_mounted(vec![mounted("one", "/same", "x"), mounted("two", "/same", "x")]).unwrap_err();
    assert!(
        matches!(
            &error,
            hub::BuildError::Route {
                module: "two",
                error: RouteError::Duplicate { .. }
            }
        ),
        "{error:?}"
    );
    // The same route with a differently named parameter is the same route to the router.
    let error = build_mounted(vec![
        mounted("one", "/a/{id}", "x"),
        mounted("two", "/a/{name}", "x"),
    ])
    .unwrap_err();
    assert!(
        matches!(error, hub::BuildError::Route { module: "two", .. }),
        "{error:?}"
    );
}
