//! One CSRF layer over every state-changing route of every module (plan 18, T4; S3, S4; closes D109).
//!
//! The composition is the real one (`tests/support/identity_rig.rs`): the hub-wide `AuthLayer` and `CsrfLayer` over a real
//! Postgres, with the identity module's routes, the platform's event stream and a probe module that has a state-changing route of
//! every method (plus a public one, the shape of the GitHub webhook) whose handlers count the times they ran. The tests walk
//! `Hub.routes`, the one table of every module, so a module added in a later task, or a route added to one, is covered by the day
//! it is added; none of them lists a route by hand.
//!
//! What is proved: a request that carries a session and lacks the double-submit token, or the exact `Origin`, or has them wrong,
//! never reaches a handler, whatever the method and the module; a request that carries no session is stopped by the guard (`401`)
//! and not by CSRF, which protects ambient authority and a request without a session has none; and a request that has all of it
//! passes, so the layer is not a blanket refusal. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use domain::{Role, UserStatus};
use http::{Method, StatusCode};
use hub::Module;
use hub::config::{LimitSettings, Rate};
use hub_platform::{GuardedRouter, RouteGuard, RouteSpec};
use rig::{Cookies, IdentityRig, Options, Req, concrete, send};

const ORIGIN: &str = mock_oidc::ORIGIN;

/// The state-changing routes of a module that has no CSRF check of its own, and that count the times a handler ran. Every method
/// a browser can make, a role of `Viewer` (so that only CSRF stands between any signed-in user and the handler), and a public route
/// of the shape the GitHub webhook has: authenticated by something other than a session, in the handler.
struct Probe {
    runs: Arc<AtomicUsize>,
}

impl Module for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        let mut router = router;
        for (method, path) in [
            (Method::POST, "/api/v1/probe/post"),
            (Method::PUT, "/api/v1/probe/put"),
            (Method::PATCH, "/api/v1/probe/patch"),
            (Method::DELETE, "/api/v1/probe/delete"),
        ] {
            let runs = Arc::clone(&self.runs);
            router = router.route(&method, path, Role::Viewer, move || {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            });
        }
        let runs = Arc::clone(&self.runs);
        router.public(
            &Method::POST,
            "/hooks/probe",
            "verified by an HMAC in the handler",
            move || {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    StatusCode::ACCEPTED
                }
            },
        )
    }
}

struct Setup {
    rig: IdentityRig,
    runs: Arc<AtomicUsize>,
    /// An admin: nothing about the role can be what refuses a request.
    admin_id: domain::UserId,
    admin: Cookies,
    /// Somebody else's session, for the token that matches itself and not the session.
    other: Cookies,
}

async fn setup() -> Option<Setup> {
    let runs = Arc::new(AtomicUsize::new(0));
    let rig = IdentityRig::start(Options {
        modules: vec![Arc::new(Probe {
            runs: Arc::clone(&runs),
        })],
        ..Options::default()
    })
    .await?;
    let (admin_id, admin) = rig.signed_in(1, Some(Role::Admin), UserStatus::Active).await;
    let (_, other) = rig.signed_in(2, Some(Role::Viewer), UserStatus::Active).await;
    Some(Setup {
        rig,
        runs,
        admin_id,
        admin,
        other,
    })
}

fn method_of(route: &RouteSpec) -> Method {
    Method::from_bytes(route.method.as_bytes()).unwrap()
}

fn is_state_changing(route: &RouteSpec) -> bool {
    !matches!(method_of(route), Method::GET | Method::HEAD | Method::OPTIONS)
}

/// A request to `route` with a body, which a state-changing request of a browser has.
fn to(route: &RouteSpec) -> Req {
    Req::new(method_of(route), &concrete(&route.path)).json(&serde_json::json!({}))
}

#[tokio::test]
async fn every_non_get_route_refuses_a_session_without_token_or_origin() {
    let Some(setup) = setup().await else { return };
    let (app, admin, other) = (setup.rig.app(), &setup.admin, &setup.other);
    let routes: Vec<&RouteSpec> = setup
        .rig
        .one()
        .hub
        .routes
        .iter()
        .filter(|r| is_state_changing(r))
        .collect();
    // A table with nothing in it would pass, so it must hold the identity routes, the probes and the public hook.
    assert!(
        routes.len() >= 10,
        "{} state-changing routes: {routes:?}",
        routes.len()
    );
    for needed in [
        "/auth/logout",
        "/api/v1/admin/users/{id}",
        "/api/v1/probe/delete",
        "/hooks/probe",
    ] {
        assert!(
            routes.iter().any(|r| r.path == needed),
            "{needed} is not in the table"
        );
    }
    assert!(
        routes
            .iter()
            .any(|r| matches!(r.guard, RouteGuard::Public { .. })),
        "the table has a public non-GET route, which a session must not bypass either"
    );

    for route in routes {
        let name = format!("{} {}", route.method, route.path);
        // Each variant lacks, or gets wrong, one thing the check needs. All of them carry a live admin session.
        let variants: Vec<(&str, Req)> = vec![
            ("the cookies only", to(route).session(admin)),
            (
                "a token and no Origin",
                to(route).session(admin).header("x-csrf-token", &admin.csrf),
            ),
            (
                "an Origin and no token",
                to(route).session(admin).header("origin", ORIGIN),
            ),
            (
                "a foreign Origin",
                to(route)
                    .session(admin)
                    .header("x-csrf-token", &admin.csrf)
                    .header("origin", "https://evil.example"),
            ),
            (
                "the Origin `null` of a sandboxed page",
                to(route)
                    .session(admin)
                    .header("x-csrf-token", &admin.csrf)
                    .header("origin", "null"),
            ),
            (
                "the right Origin with a suffix",
                to(route)
                    .session(admin)
                    .header("x-csrf-token", &admin.csrf)
                    .header("origin", &format!("{ORIGIN}.evil.example")),
            ),
            (
                "two Origin headers",
                to(route)
                    .session(admin)
                    .header("x-csrf-token", &admin.csrf)
                    .header("origin", ORIGIN)
                    .header("origin", ORIGIN),
            ),
            (
                "a token that is not the cookie's",
                to(route)
                    .session(admin)
                    .header("x-csrf-token", &other.csrf)
                    .header("origin", ORIGIN),
            ),
            (
                // The attack from a sibling subdomain: it can set a cookie, so the pair matches itself. It does not match the
                // session, which holds the hash of the token it was made with.
                "a token pair planted next to the session",
                Req::new(method_of(route), &concrete(&route.path))
                    .json(&serde_json::json!({}))
                    .cookie("__Host-lk_session", &admin.session)
                    .cookie("__Host-lk_csrf", &other.csrf)
                    .header("x-csrf-token", &other.csrf)
                    .header("origin", ORIGIN),
            ),
        ];
        for (what, request) in variants {
            let reply = send(app, request).await;
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "{name} with {what}: {}",
                reply.text()
            );
            assert_eq!(
                reply.problem_code().as_deref(),
                Some("csrf"),
                "{name} with {what} was refused, but not by CSRF: {}",
                reply.text()
            );
        }
    }
    // And nothing reached a handler of the module that has no check of its own.
    assert_eq!(
        setup.runs.load(Ordering::SeqCst),
        0,
        "a handler ran for a request that failed CSRF"
    );
}

#[tokio::test]
async fn a_request_that_has_the_token_and_the_origin_passes_csrf_on_every_route() {
    let Some(setup) = setup().await else { return };
    let app = setup.rig.app();
    let mut passed = 0;
    for route in setup.rig.one().hub.routes.iter().filter(|r| is_state_changing(r)) {
        // A session of its own for each route: `/auth/logout` ends the one it is given.
        let admin = setup.rig.session_for(&setup.admin_id).await;
        let reply = send(app, to(route).with_token(&admin)).await;
        assert_ne!(
            reply.problem_code().as_deref(),
            Some("csrf"),
            "{} {} refused a request that has the token and the Origin: {}",
            route.method,
            route.path,
            reply.text()
        );
        passed += 1;
    }
    assert!(passed >= 10, "{passed}");
    // The probe's four handlers ran, and so did the public one: it is for a request with no session, but a session does not stop it
    // when the token and the Origin are right.
    assert_eq!(setup.runs.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn a_request_without_a_session_is_stopped_by_the_guard_not_by_csrf() {
    let Some(setup) = setup().await else { return };
    let app = setup.rig.app();
    for route in setup.rig.one().hub.routes.iter().filter(|r| is_state_changing(r)) {
        let name = format!("{} {}", route.method, route.path);
        let RouteGuard::Role(_) = route.guard else {
            continue;
        };
        // No cookie, a garbage cookie and a well-formed cookie that names no session: nobody. There is no ambient authority for CSRF
        // to protect, so it says nothing, and the guard refuses (`401`).
        for (what, request) in [
            ("no cookie", to(route)),
            (
                "a garbage cookie",
                to(route).cookie("__Host-lk_session", "garbage"),
            ),
            (
                "an unknown session",
                to(route).cookie("__Host-lk_session", &rig::unknown_session_cookie()),
            ),
            // The token and the Origin do not make a session.
            (
                "a token and an Origin only",
                to(route).header("x-csrf-token", "x").header("origin", ORIGIN),
            ),
        ] {
            let reply = send(app, request).await;
            assert_eq!(
                reply.status,
                StatusCode::UNAUTHORIZED,
                "{name} with {what}: {}",
                reply.text()
            );
            assert_ne!(
                reply.problem_code().as_deref(),
                Some("csrf"),
                "{name} with {what}"
            );
        }
    }
    assert_eq!(
        setup.runs.load(Ordering::SeqCst),
        0,
        "a guarded handler ran with no session"
    );

    // The public route, which authenticates by itself, is reached without a session: that is what it is for (a webhook has no cookie).
    let hook = send(app, Req::post("/hooks/probe").json(&serde_json::json!({}))).await;
    assert_eq!(hook.status, StatusCode::ACCEPTED, "{}", hook.text());
    assert_eq!(setup.runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_get_needs_only_the_cookie() {
    let Some(setup) = setup().await else { return };
    // A GET with only the cookie is the page loading; CSRF does not touch it (and no GET of this hub changes state).
    let me = send(setup.rig.app(), Req::get("/api/v1/me").session(&setup.admin)).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
}

/// The layers are in the order the composition needs: the per-IP bucket first (so that a flood of unknown cookies cannot cost a
/// database read each), then `AuthLayer`, then `CsrfLayer`, then the routes. `CsrfLayer` does nothing without the `AuthContext`
/// that `AuthLayer` puts in the request, and a session lookup that cannot be answered is a `503` and not an anonymous caller.
#[tokio::test]
async fn the_layers_run_in_the_order_the_composition_needs() {
    let Some(rig) = IdentityRig::start(Options {
        limits: LimitSettings {
            // Three requests from one address, then the bucket is empty for the length of this test.
            per_ip: Rate {
                per_second: 0.001,
                burst: 3,
            },
            ..LimitSettings::default()
        },
        ..Options::default()
    })
    .await
    else {
        return;
    };
    // The database goes away: a request that carries a session cookie cannot be looked up.
    rig.pool().close().await;
    let mut statuses = Vec::new();
    for _ in 0..8 {
        let reply = send(
            rig.app(),
            Req::get("/api/v1/me").cookie("__Host-lk_session", &rig::unknown_session_cookie()),
        )
        .await;
        statuses.push(reply.status);
    }
    // The first three reached `AuthLayer`, which could not answer and said so (not "anonymous", which would be a `401`). The rest
    // never reached it: the bucket, which is outside it, refused them. Were the bucket inside, every one of the eight would be 503.
    assert_eq!(
        statuses[..3],
        [StatusCode::SERVICE_UNAVAILABLE; 3],
        "the session layer runs inside the bucket and says when it cannot answer: {statuses:?}"
    );
    assert!(
        statuses[3..].iter().all(|s| *s == StatusCode::TOO_MANY_REQUESTS),
        "the per-IP bucket must refuse before a cookie is looked up: {statuses:?}"
    );
}
