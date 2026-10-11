//! T4 (S1, S4): the role gate, the extractors, the status of the caller, and the identity routes, through real HTTP requests
//! and a real Postgres.
//!
//! Every request goes through the real `AuthLayer`, `CsrfLayer` and role gate of `GuardedRouter`, so what is proven here is the
//! behaviour a browser sees.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;

use axum::Router;
use axum::http::{Method, StatusCode};
use domain::{Role, User, UserStatus};
use hub_identity::ManualClock;
use hub_identity::guard::{Access, Admin, Denied, RequireRole, Viewer, authorize, authorize_status};
use support::guard_rig::{App, assert_problem, identity_guarded, problem_kind, send_json};
use support::sessions_rig::Replica;
use support::users_rig::{UsersRig, uid};
use support::web_rig::{ORIGIN, Req, send};

// ---------------------------------------------------------------------------------------------------------------
// The gate

#[test]
fn route_gate_denies_pending_and_disabled_for_every_role() {
    // The conformance harness of the ports: pending and disabled never pass, whatever the role; active passes iff role >= min.
    ports::conformance::route_guard::assert_user_gate(|user: &User, min| authorize(user, min).is_ok());
    for status in [UserStatus::Pending, UserStatus::Disabled] {
        for role in Role::ALL {
            for min in Role::ALL {
                assert!(
                    authorize_status(status, Some(role), min).is_err(),
                    "{status:?} {role} {min}"
                );
            }
        }
    }
}

#[test]
fn the_gate_names_why_it_denied() {
    assert_eq!(
        authorize_status(UserStatus::Pending, None, Role::Viewer),
        Err(Denied::Pending)
    );
    assert_eq!(
        authorize_status(UserStatus::Disabled, Some(Role::Admin), Role::Viewer),
        Err(Denied::Disabled)
    );
    assert_eq!(
        authorize_status(UserStatus::Active, Some(Role::Viewer), Role::Editor),
        Err(Denied::InsufficientRole)
    );
    assert_eq!(
        authorize_status(UserStatus::Active, None, Role::Viewer),
        Err(Denied::InsufficientRole),
        "an active user with no role passes nothing"
    );
    assert_eq!(
        authorize_status(UserStatus::Active, Some(Role::Admin), Role::Admin),
        Ok(())
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The registered routes, with a pending or disabled caller (D36)

/// The path of a route with its parameters filled in.
fn concrete(path: &str) -> String {
    path.replace("{id}", "1")
}

async fn every_guarded_route_answers(status: UserStatus, kind: &str) {
    let app = App::new(db!()).await;
    let (_id, s) = app
        .user(
            1,
            if status == UserStatus::Pending {
                None
            } else {
                Some(Role::Admin)
            },
            status,
        )
        .await;
    let mut checked = 0;
    for route in &app.table {
        let Access::Role(_) = route.access else { continue };
        let method = Method::from_bytes(route.method.as_bytes()).unwrap();
        let a = app.send(app.as_user(method, &concrete(&route.path), &s)).await;
        assert_problem(&a, StatusCode::FORBIDDEN, kind);
        checked += 1;
    }
    assert_eq!(checked, 7, "the seven guarded identity routes");
}

#[tokio::test]
async fn pending_user_gets_403_on_every_guarded_route() {
    every_guarded_route_answers(UserStatus::Pending, "access-pending").await;
}

#[tokio::test]
async fn disabled_user_gets_403_on_every_guarded_route() {
    // Even a disabled admin: the status, not the role, decides.
    every_guarded_route_answers(UserStatus::Disabled, "access-disabled").await;
}

#[tokio::test]
async fn pending_and_disabled_users_are_denied_on_unmatched_paths_too() {
    let app = App::new(db!()).await;
    let (_p, pending) = app.user(1, None, UserStatus::Pending).await;
    let (_d, disabled) = app.user(2, Some(Role::Admin), UserStatus::Disabled).await;
    let (_a, active) = app.user(3, Some(Role::Viewer), UserStatus::Active).await;
    let a = app
        .send(app.as_user(Method::GET, "/api/v1/nothing", &pending))
        .await;
    assert_problem(&a, StatusCode::FORBIDDEN, "access-pending");
    let a = app
        .send(app.as_user(Method::GET, "/api/v1/nothing", &disabled))
        .await;
    assert_problem(&a, StatusCode::FORBIDDEN, "access-disabled");
    let a = app
        .send(app.as_user(Method::GET, "/api/v1/nothing", &active))
        .await;
    assert_problem(&a, StatusCode::NOT_FOUND, "not-found");
}

#[tokio::test]
async fn problem_types_distinguish_pending_disabled_and_insufficient_role() {
    let app = App::new(db!()).await;
    let (_p, pending) = app.user(1, None, UserStatus::Pending).await;
    let (_d, disabled) = app.user(2, Some(Role::Viewer), UserStatus::Disabled).await;
    let (_v, viewer) = app.user(3, Some(Role::Viewer), UserStatus::Active).await;
    let kinds = [
        problem_kind(
            &app.send(app.as_user(Method::GET, "/api/v1/admin/users", &pending))
                .await,
        ),
        problem_kind(
            &app.send(app.as_user(Method::GET, "/api/v1/admin/users", &disabled))
                .await,
        ),
        problem_kind(
            &app.send(app.as_user(Method::GET, "/api/v1/admin/users", &viewer))
                .await,
        ),
        problem_kind(&app.send(Req::get("/api/v1/admin/users")).await),
    ];
    assert_eq!(
        kinds,
        [
            "access-pending",
            "access-disabled",
            "insufficient-role",
            "unauthenticated"
        ]
    );
}

#[tokio::test]
async fn problem_json_carries_no_internals() {
    let app = App::new(db!()).await;
    let (_v, viewer) = app.user(1, Some(Role::Viewer), UserStatus::Active).await;
    let denied = app
        .send(app.as_user(Method::GET, "/api/v1/admin/users", &viewer))
        .await;
    let anonymous = app.send(Req::get("/api/v1/me")).await;
    for a in [&denied, &anonymous] {
        let v = a.json();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["request_id", "status", "title", "type"], "{}", a.body);
        for needle in [
            "admin", "viewer", "role", "select ", "sql", "postgres", "/home", "panicked",
        ] {
            let title = v["title"].as_str().unwrap().to_lowercase();
            assert!(!title.contains(needle), "the title leaks {needle:?}: {title}");
        }
    }
    // A database that fails is a generic 503, and the body says nothing of it.
    app.rig.replica.pool.close().await;
    let a = app.send(app.as_user(Method::GET, "/api/v1/me", &viewer)).await;
    assert_problem(&a, StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    let text = a.body.to_lowercase();
    assert!(
        !text.contains("sql") && !text.contains("pool") && !text.contains("connection"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The role matrix, through the router and through an extractor

/// A router with one route per minimum role, all with stand-in handlers, on the same sessions and database as `rig`.
async fn matrix_router(rig: &UsersRig, replica: &Replica) -> Router {
    let clock: &Arc<ManualClock> = &rig.clock;
    let g = identity_guarded(
        replica.pool.clone(),
        Arc::clone(&replica.sessions),
        Arc::new(support::simple_audit::SimpleChainAudit),
        Arc::clone(&replica.bus) as Arc<_>,
        clock,
    )
    .await
    .get("/t/viewer", Role::Viewer, (), || async { "ok" })
    .get("/t/editor", Role::Editor, (), || async { "ok" })
    .get("/t/operator", Role::Operator, (), || async { "ok" })
    .get("/t/admin", Role::Admin, (), || async { "ok" })
    .post("/t/changes", Role::Operator, (), || async { "changed" })
    // Registered as a viewer route, but the handler asks for more: the extractor checks again and can only make it stricter.
    .get("/t/strict", Role::Viewer, (), |_who: RequireRole<Admin>| async {
        "ok"
    })
    .get("/t/loose", Role::Admin, (), |_who: RequireRole<Viewer>| async {
        "ok"
    });
    g.into_router().unwrap()
}

#[tokio::test]
async fn role_matrix_viewer_editor_operator_admin() {
    let rig = UsersRig::new(db!()).await;
    let replica = Replica::start_deaf(&rig.db, &rig.clock).await;
    let router = matrix_router(&rig, &replica).await;
    for (n, role) in (1u32..).zip(Role::ALL) {
        rig.seed(n, Some(role), UserStatus::Active).await;
        let s = replica.create_session(&uid(n)).await;
        for (path, min) in [
            ("/t/viewer", Role::Viewer),
            ("/t/editor", Role::Editor),
            ("/t/operator", Role::Operator),
            ("/t/admin", Role::Admin),
        ] {
            let a = send(
                &router,
                Req::get(path).with_session(s.session.expose(), s.csrf.expose()),
            )
            .await;
            if role >= min {
                assert_eq!(a.status, StatusCode::OK, "{role} on {path}");
            } else {
                assert_problem(&a, StatusCode::FORBIDDEN, "insufficient-role");
            }
        }
    }
}

#[tokio::test]
async fn extractor_rechecks_role() {
    let rig = UsersRig::new(db!()).await;
    let replica = Replica::start_deaf(&rig.db, &rig.clock).await;
    let router = matrix_router(&rig, &replica).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    rig.seed(2, Some(Role::Admin), UserStatus::Active).await;
    let viewer = replica.create_session(&uid(1)).await;
    let admin = replica.create_session(&uid(2)).await;
    let with = |s: &hub_identity::session::store::NewSession, path| {
        Req::get(path).with_session(s.session.expose(), s.csrf.expose())
    };
    // The route says viewer; the handler needs an admin. The viewer is stopped by the extractor.
    assert_problem(
        &send(&router, with(&viewer, "/t/strict")).await,
        StatusCode::FORBIDDEN,
        "insufficient-role",
    );
    assert_eq!(
        send(&router, with(&admin, "/t/strict")).await.status,
        StatusCode::OK
    );
    // The route says admin; a handler that would let a viewer in is still behind the route's own gate.
    assert_problem(
        &send(&router, with(&viewer, "/t/loose")).await,
        StatusCode::FORBIDDEN,
        "insufficient-role",
    );
    assert_eq!(
        send(&router, with(&admin, "/t/loose")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_state_changing_route_checks_csrf_before_the_role() {
    let rig = UsersRig::new(db!()).await;
    let replica = Replica::start_deaf(&rig.db, &rig.clock).await;
    let router = matrix_router(&rig, &replica).await;
    rig.seed(1, Some(Role::Operator), UserStatus::Active).await;
    let s = replica.create_session(&uid(1)).await;
    let ok = send(
        &router,
        Req::post("/t/changes").with_session(s.session.expose(), s.csrf.expose()),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    // A cross-site request carries the cookies but not the token or the origin: refused as CSRF, whatever the role.
    let forged = send(
        &router,
        Req::post("/t/changes")
            .cookie("__Host-lk_session", s.session.expose())
            .cookie("__Host-lk_csrf", s.csrf.expose())
            .origin("https://evil.example.com"),
    )
    .await;
    assert_problem(&forged, StatusCode::FORBIDDEN, "csrf");
    // No cookie at all: nothing to forge, so 401 and not CSRF.
    let anon = send(&router, Req::post("/t/changes").origin(ORIGIN)).await;
    assert_problem(&anon, StatusCode::UNAUTHORIZED, "unauthenticated");
}

#[tokio::test]
async fn a_role_change_takes_effect_on_the_next_request_after_the_event() {
    // The gate reads the role from the session cache; the revocation event (T3) is what makes a demotion immediate. A replica
    // that listens sees the demoted admin as insufficient on the next request.
    let rig = UsersRig::new(db!()).await;
    let replica = Replica::start(&rig.db, &rig.clock).await;
    let router = matrix_router(&rig, &replica).await;
    rig.seed_admin(1).await;
    rig.seed_admin(2).await;
    let s = replica.create_session(&uid(2)).await;
    let before = send(
        &router,
        Req::get("/t/admin").with_session(s.session.expose(), s.csrf.expose()),
    )
    .await;
    assert_eq!(before.status, StatusCode::OK);
    rig.admin
        .update_user(
            &support::users_rig::actor(1),
            &uid(2),
            hub_identity::users::rules::Change {
                role: Some(Some(Role::Viewer)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    replica
        .wait_for_events(1, std::time::Duration::from_secs(5))
        .await;
    let after = send(
        &router,
        Req::get("/t/admin").with_session(s.session.expose(), s.csrf.expose()),
    )
    .await;
    // The sessions of the demoted user are gone: they must sign in again.
    assert_problem(&after, StatusCode::UNAUTHORIZED, "unauthenticated");
}

// ---------------------------------------------------------------------------------------------------------------
// The identity routes

#[tokio::test]
async fn me_returns_the_user_and_the_onboarding_flag() {
    let app = App::new(db!()).await;
    let (_e, editor) = app.user(1, Some(Role::Editor), UserStatus::Active).await;
    let (_v, viewer) = app.user(2, Some(Role::Viewer), UserStatus::Active).await;
    let a = app.send(app.as_user(Method::GET, "/api/v1/me", &editor)).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let me = a.json();
    assert_eq!(me["id"], uid(1).to_string());
    assert_eq!(me["role"], "editor");
    assert_eq!(me["status"], "active");
    assert_eq!(me["swimlane_access"], "all");
    assert_eq!(
        me["needs_github_token"], true,
        "an editor with no stored token is prompted"
    );
    assert_eq!(me["email"], "user1@example.com");
    assert_eq!(me["requires_approval"], false);
    assert_eq!(me["github_login"], serde_json::Value::Null);
    let v = app
        .send(app.as_user(Method::GET, "/api/v1/me", &viewer))
        .await
        .json();
    assert_eq!(v["needs_github_token"], false, "a viewer never edits");
    assert_eq!(a.headers["cache-control"], "no-store");
}

#[tokio::test]
async fn me_never_contains_a_credential() {
    let app = App::new(db!()).await;
    let (_e, editor) = app.user(1, Some(Role::Editor), UserStatus::Active).await;
    let a = app.send(app.as_user(Method::GET, "/api/v1/me", &editor)).await;
    let text = a.body.to_lowercase();
    for needle in [
        "token_ciphertext",
        "ciphertext",
        "session",
        "csrf",
        "secret",
        "hash",
    ] {
        assert!(!text.contains(needle), "{needle}: {text}");
    }
}

#[tokio::test]
async fn an_active_user_asks_for_a_role_and_gets_202() {
    let app = App::new(db!()).await;
    let (_v, viewer) = app.user(1, Some(Role::Viewer), UserStatus::Active).await;
    let body = r#"{"requested_role":"editor","reason":"I maintain the sit1 swimlane"}"#;
    let a = send_json(
        &app.router,
        Method::POST,
        "/api/v1/me/access-request",
        Some(&viewer),
        body,
    )
    .await;
    assert_eq!(a.status, StatusCode::ACCEPTED, "{}", a.body);
    let r = a.json();
    assert_eq!(r["requested_role"], "editor");
    assert_eq!(r["status"], "pending");
    assert_eq!(r["user"], uid(1).to_string());
    // A second request at once is refused, and says so without echoing anything back.
    let again = send_json(
        &app.router,
        Method::POST,
        "/api/v1/me/access-request",
        Some(&viewer),
        body,
    )
    .await;
    assert_problem(&again, StatusCode::CONFLICT, "conflict");
    assert!(!again.body.contains("maintain"), "{}", again.body);
}

#[tokio::test]
async fn request_bodies_are_strict() {
    let app = App::new(db!()).await;
    let (_v, viewer) = app.user(1, Some(Role::Viewer), UserStatus::Active).await;
    let path = "/api/v1/me/access-request";
    for (what, body) in [
        ("empty", ""),
        ("not json", "role=editor"),
        (
            "an unknown field",
            r#"{"requested_role":"editor","reason":"x","admin":true}"#,
        ),
        ("a missing field", r#"{"requested_role":"editor"}"#),
        ("a bad role", r#"{"requested_role":"root","reason":"x"}"#),
        (
            "a number for the reason",
            r#"{"requested_role":"editor","reason":7}"#,
        ),
        (
            "an oversize reason",
            &format!(r#"{{"requested_role":"editor","reason":"{}"}}"#, "a".repeat(300)),
        ),
    ] {
        let a = send_json(&app.router, Method::POST, path, Some(&viewer), body).await;
        assert_problem(&a, StatusCode::BAD_REQUEST, "bad-request");
        assert!(
            !a.body.contains("root") && !a.body.contains("aaaa"),
            "{what}: the body is not echoed: {}",
            a.body
        );
    }
    // A body over the limit is refused without being read whole.
    let huge = format!(
        r#"{{"requested_role":"editor","reason":"{}"}}"#,
        "a".repeat(100_000)
    );
    let a = send_json(&app.router, Method::POST, path, Some(&viewer), &huge).await;
    assert_problem(&a, StatusCode::PAYLOAD_TOO_LARGE, "payload-too-large");
    // Not a JSON content type.
    let wrong_type = {
        use axum::body::Body;
        use axum::http::{Request, header};
        use tower::ServiceExt;
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::CONTENT_TYPE, "text/plain")
            .header(
                header::COOKIE,
                format!(
                    "__Host-lk_session={}; __Host-lk_csrf={}",
                    viewer.session.expose(),
                    viewer.csrf.expose()
                ),
            )
            .header("x-csrf-token", viewer.csrf.expose().as_str())
            .header(header::ORIGIN, ORIGIN)
            .body(Body::from(r#"{"requested_role":"editor","reason":"x"}"#))
            .unwrap();
        app.router.clone().oneshot(req).await.unwrap().status()
    };
    assert_eq!(wrong_type, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn admin_lists_users_with_keyset_pages() {
    let app = App::new(db!()).await;
    let (_a, admin) = app.user(1, Some(Role::Admin), UserStatus::Active).await;
    for n in 2..=6 {
        app.rig
            .users
            .seed(n, Some(Role::Viewer), UserStatus::Active)
            .await;
    }
    let first = app
        .send(app.as_user(Method::GET, "/api/v1/admin/users?limit=4", &admin))
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let page = first.json();
    assert_eq!(page["items"].as_array().unwrap().len(), 4);
    let next = page["next"].as_str().unwrap().to_owned();
    let second = app
        .send(app.as_user(
            Method::GET,
            &format!("/api/v1/admin/users?limit=4&cursor={next}"),
            &admin,
        ))
        .await
        .json();
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["next"], serde_json::Value::Null);
    // Bad parameters are 400, never a 500 and never an echo.
    for q in [
        "limit=0",
        "limit=abc",
        "limit=1&limit=2",
        "status=root",
        "cursor=%00%00",
        &format!("q={}", "a".repeat(300)),
    ] {
        let a = app
            .send(app.as_user(Method::GET, &format!("/api/v1/admin/users?{q}"), &admin))
            .await;
        assert_problem(&a, StatusCode::BAD_REQUEST, "bad-request");
    }
    // A limit above the cap is clamped, not refused.
    let big = app
        .send(app.as_user(Method::GET, "/api/v1/admin/users?limit=100000", &admin))
        .await;
    assert_eq!(big.status, StatusCode::OK);
    let filtered = app
        .send(app.as_user(Method::GET, "/api/v1/admin/users?status=disabled", &admin))
        .await
        .json();
    assert!(filtered["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn admin_approves_and_rejects_requests_and_updates_users() {
    let app = App::new(db!()).await;
    let (_a, admin) = app.user(1, Some(Role::Admin), UserStatus::Active).await;
    // Two pending users with an open request each (the sign-in made them; here they are seeded).
    let p2 = app.rig.users.seed(2, None, UserStatus::Pending).await;
    let p3 = app.rig.users.seed(3, None, UserStatus::Pending).await;
    for p in [&p2, &p3] {
        sqlx::query("INSERT INTO access_requests (user_tid, user_oid, role, reason) VALUES ($1, $2, 'viewer', 'new here')")
            .bind(p.tid().as_str())
            .bind(p.oid().as_str())
            .execute(&app.rig.replica.pool)
            .await
            .unwrap();
    }
    let list = app
        .send(app.as_user(
            Method::GET,
            "/api/v1/admin/access-requests?status=pending",
            &admin,
        ))
        .await
        .json();
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    let request_of = |user: domain::UserId| {
        let pool = app.rig.replica.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM access_requests WHERE user_tid = $1 AND user_oid = $2",
            )
            .bind(user.tid().as_str())
            .bind(user.oid().as_str())
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let ids = [request_of(p2.clone()).await, request_of(p3.clone()).await];

    let approve = send_json(
        &app.router,
        Method::POST,
        &format!("/api/v1/admin/access-requests/{}/approve", ids[0]),
        Some(&admin),
        r#"{"role":"editor"}"#,
    )
    .await;
    assert_eq!(approve.status, StatusCode::OK, "{}", approve.body);
    assert_eq!(approve.json()["role"], "editor");
    assert_eq!(approve.json()["status"], "active");

    let reject = send_json(
        &app.router,
        Method::POST,
        &format!("/api/v1/admin/access-requests/{}/reject", ids[1]),
        Some(&admin),
        r#"{"reason":"not on the team"}"#,
    )
    .await;
    assert_eq!(reject.status, StatusCode::OK, "{}", reject.body);
    assert_eq!(reject.json()["status"], "rejected");

    // Deciding twice is a conflict; an unknown request is not found.
    let twice = send_json(
        &app.router,
        Method::POST,
        &format!("/api/v1/admin/access-requests/{}/approve", ids[0]),
        Some(&admin),
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_problem(&twice, StatusCode::CONFLICT, "conflict");
    let missing = send_json(
        &app.router,
        Method::POST,
        "/api/v1/admin/access-requests/99999/reject",
        Some(&admin),
        r#"{"reason":"x"}"#,
    )
    .await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not-found");

    // Update: promote the approved editor to operator, then disable them.
    let promote = send_json(
        &app.router,
        Method::PATCH,
        &format!("/api/v1/admin/users/{p2}"),
        Some(&admin),
        r#"{"role":"operator","requires_approval":true}"#,
    )
    .await;
    assert_eq!(promote.status, StatusCode::OK, "{}", promote.body);
    assert_eq!(promote.json()["role"], "operator");
    assert_eq!(promote.json()["requires_approval"], true);
    let disable = send_json(
        &app.router,
        Method::PATCH,
        &format!("/api/v1/admin/users/{p2}"),
        Some(&admin),
        r#"{"status":"disabled"}"#,
    )
    .await;
    assert_eq!(disable.json()["status"], "disabled");
    assert_eq!(disable.json()["role"], "operator", "disabling keeps the role");
}

#[tokio::test]
async fn the_last_admin_and_self_changes_are_refused_with_a_reason() {
    let app = App::new(db!()).await;
    let (admin_id, admin) = app.user(1, Some(Role::Admin), UserStatus::Active).await;
    let path = format!("/api/v1/admin/users/{admin_id}");
    let own_role = send_json(
        &app.router,
        Method::PATCH,
        &path,
        Some(&admin),
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_problem(&own_role, StatusCode::UNPROCESSABLE_ENTITY, "unprocessable");
    assert!(
        own_role.json()["detail"].as_str().unwrap().contains("own role"),
        "{}",
        own_role.body
    );
    let own_status = send_json(
        &app.router,
        Method::PATCH,
        &path,
        Some(&admin),
        r#"{"status":"disabled"}"#,
    )
    .await;
    assert_eq!(
        own_status.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        own_status.body
    );
    // Another admin demoting the only other admin is fine; demoting the last one is a conflict.
    let (_b, other) = app.user(2, Some(Role::Admin), UserStatus::Active).await;
    let demote = send_json(
        &app.router,
        Method::PATCH,
        &path,
        Some(&other),
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_eq!(demote.status, StatusCode::OK, "{}", demote.body);
    let last = send_json(
        &app.router,
        Method::PATCH,
        &format!("/api/v1/admin/users/{}", uid(2)),
        Some(&other),
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_eq!(
        last.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "self change is checked first: {}",
        last.body
    );
    // An update that asks for nothing, with an id that is no user, and with a malformed id.
    let empty = send_json(
        &app.router,
        Method::PATCH,
        &format!("/api/v1/admin/users/{}", uid(2)),
        Some(&other),
        "{}",
    )
    .await;
    assert_problem(&empty, StatusCode::BAD_REQUEST, "bad-request");
    let unknown = send_json(
        &app.router,
        Method::PATCH,
        &format!("/api/v1/admin/users/{}", uid(77)),
        Some(&other),
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_problem(&unknown, StatusCode::NOT_FOUND, "not-found");
    let bad_id = send_json(
        &app.router,
        Method::PATCH,
        "/api/v1/admin/users/not-an-id",
        Some(&other),
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_problem(&bad_id, StatusCode::BAD_REQUEST, "bad-request");
}

#[tokio::test]
async fn null_role_removes_the_role_and_absent_role_leaves_it() {
    let app = App::new(db!()).await;
    let (_a, admin) = app.user(1, Some(Role::Admin), UserStatus::Active).await;
    let target = app
        .rig
        .users
        .seed(2, Some(Role::Editor), UserStatus::Disabled)
        .await;
    let path = format!("/api/v1/admin/users/{target}");
    // Only the flag: the role stays.
    let flag = send_json(
        &app.router,
        Method::PATCH,
        &path,
        Some(&admin),
        r#"{"requires_approval":true}"#,
    )
    .await;
    assert_eq!(flag.json()["role"], "editor", "{}", flag.body);
    // `null` is a request to remove it.
    let none = send_json(
        &app.router,
        Method::PATCH,
        &path,
        Some(&admin),
        r#"{"role":null}"#,
    )
    .await;
    assert_eq!(none.status, StatusCode::OK, "{}", none.body);
    assert_eq!(none.json()["role"], serde_json::Value::Null);
}

#[tokio::test]
async fn every_admin_route_answers_403_to_every_lower_role() {
    let app = App::new(db!()).await;
    for (n, role) in (1u32..).zip([Role::Viewer, Role::Editor, Role::Operator]) {
        let (_id, s) = app.user(n, Some(role), UserStatus::Active).await;
        for route in app
            .table
            .iter()
            .filter(|r| matches!(r.access, Access::Role(Role::Admin)))
        {
            let method = Method::from_bytes(route.method.as_bytes()).unwrap();
            let a = app.send(app.as_user(method, &concrete(&route.path), &s)).await;
            assert_problem(&a, StatusCode::FORBIDDEN, "insufficient-role");
        }
    }
}

#[tokio::test]
async fn the_csrf_policy_of_the_public_routes_is_required() {
    // The three sign-in routes are open, but logout with a session still has to pass the CSRF check.
    let app = App::new(db!()).await;
    let (_v, s) = app.user(1, Some(Role::Viewer), UserStatus::Active).await;
    let forged = send(
        &app.router,
        Req::post("/auth/logout")
            .cookie("__Host-lk_session", s.session.expose())
            .cookie("__Host-lk_csrf", s.csrf.expose())
            .origin("https://evil.example.com"),
    )
    .await;
    assert_problem(&forged, StatusCode::FORBIDDEN, "csrf");
    let ok = send(
        &app.router,
        Req::post("/auth/logout").with_session(s.session.expose(), s.csrf.expose()),
    )
    .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
}

// ---------------------------------------------------------------------------------------------------------------
// Logs (S10)

#[tokio::test]
async fn identity_routes_log_no_cookie_token_reason_or_body() {
    let (capture, _guard) = support::log_capture::install();
    let app = App::new(db!()).await;
    let (_a, admin) = app.user(1, Some(Role::Admin), UserStatus::Active).await;
    let (_v, viewer) = app.user(2, Some(Role::Viewer), UserStatus::Active).await;
    let (_p, pending) = app.user(3, None, UserStatus::Pending).await;

    let reason = "REASON-TEXT-I-MAINTAIN-SIT1";
    let body = format!(r#"{{"requested_role":"editor","reason":"{reason}"}}"#);
    send_json(
        &app.router,
        Method::POST,
        "/api/v1/me/access-request",
        Some(&viewer),
        &body,
    )
    .await;
    send_json(
        &app.router,
        Method::POST,
        "/api/v1/me/access-request",
        Some(&viewer),
        &body,
    )
    .await;
    send_json(
        &app.router,
        Method::POST,
        "/api/v1/me/access-request",
        Some(&viewer),
        "{not json REASON-TEXT-BAD",
    )
    .await;
    send_json(
        &app.router,
        Method::POST,
        "/api/v1/admin/access-requests/1/reject",
        Some(&admin),
        r#"{"reason":"REJECT-REASON-TEXT"}"#,
    )
    .await;
    send_json(
        &app.router,
        Method::PATCH,
        &format!("/api/v1/admin/users/{}", uid(1)),
        Some(&admin),
        r#"{"role":"viewer"}"#,
    )
    .await;
    // Refusals: the role gate, the pending gate, CSRF, a cookie that is not a session.
    app.send(app.as_user(Method::GET, "/api/v1/admin/users", &viewer))
        .await;
    app.send(app.as_user(Method::GET, "/api/v1/me", &pending)).await;
    app.send(
        Req::post("/api/v1/me/access-request")
            .cookie("__Host-lk_session", viewer.session.expose())
            .cookie("__Host-lk_csrf", viewer.csrf.expose())
            .origin("https://evil.example.com"),
    )
    .await;
    app.send(Req::get("/api/v1/me").cookie("__Host-lk_session", "COOKIE-GARBAGE-VALUE"))
        .await;
    app.rig.replica.pool.close().await;
    app.send(app.as_user(Method::GET, "/api/v1/me", &viewer)).await;

    let log = capture.text();
    assert!(!log.is_empty(), "refusals are logged, so the capture works");
    for secret in [
        admin.session.expose().as_str(),
        admin.csrf.expose().as_str(),
        viewer.session.expose().as_str(),
        viewer.csrf.expose().as_str(),
        pending.session.expose().as_str(),
        reason,
        "REASON-TEXT-BAD",
        "REJECT-REASON-TEXT",
        "COOKIE-GARBAGE-VALUE",
        "evil.example.com",
    ] {
        assert!(!log.contains(secret), "{secret} reached the log");
    }
}
