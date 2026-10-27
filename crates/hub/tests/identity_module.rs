//! The identity module in the composed hub (plan 18, T4; S1, S2, S3, S4, S9, S10, P13).
//!
//! The composition is the real one: `Identity::build`, the platform's event bus, audit log and leases, the platform's event stream,
//! and `Hub::build` with its hub-wide `AuthLayer` and `CsrfLayer`, over a real Postgres 16 (a container, TLS 1.3, the app role).
//! Software is only at the edges: identity's own mock Entra, secrets in memory, a KMS with software keys, an in-memory checkpoint
//! bucket and a clock the test moves. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
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
#[path = "../../hub-identity/tests/support/log_capture.rs"]
mod log_capture;
#[path = "../../hub-identity/tests/support/mock_oidc.rs"]
mod mock_oidc;
#[path = "support/identity_rig.rs"]
mod rig;
#[path = "support/secrets.rs"]
mod secrets;
#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;
#[path = "../../hub-platform/tests/support/sse_client.rs"]
mod sse_client;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{DomainEvent, Role, SwimlaneId, UserStatus};
use http::StatusCode;
use hub::identity::{AuditSchedule, IdentityBuildError};
use hub_platform::RouteGuard;
use ports::Leases;
use rig::{Cookies, IdentityRig, Options, Req, concrete, eventually, open, send};
use sse_client::Frames;

fn frames(
    response: http::Response<axum::body::Body>,
) -> Frames<impl futures::Stream<Item = Result<bytes::Bytes, axum::Error>> + Unpin> {
    assert_eq!(response.status(), StatusCode::OK);
    Frames::new(response.into_body().into_data_stream())
}

/// The status of `GET /api/v1/me` for these cookies on `app`.
async fn me(app: &axum::Router, cookies: &Cookies) -> StatusCode {
    send(app, Req::get("/api/v1/me").session(cookies)).await.status
}

/// The value of one of the identity module's counters on replica `n`.
fn counter(rig: &IdentityRig, n: usize, name: &str) -> u64 {
    use hub_identity::metrics::CounterSource;
    rig.replicas[n]
        .identity
        .metrics
        .counters()
        .into_iter()
        .find(|c| c.name == name)
        .map_or(0, |c| c.value)
}

async fn audit_actions(rig: &IdentityRig) -> Vec<String> {
    sqlx::query_scalar("SELECT action FROM audit_events ORDER BY seq LIMIT 1000")
        .fetch_all(rig.pool())
        .await
        .unwrap()
}

// ------------------------------------------------------------------------------------------------------ S1: sign-in

#[tokio::test]
async fn sign_in_against_a_local_identity_provider_gives_a_working_cookie() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    assert_eq!(
        send(app, Req::get("/api/v1/me")).await.status,
        StatusCode::UNAUTHORIZED,
        "no cookie, no access"
    );

    let (callback, cookies) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    assert_eq!(callback.status, StatusCode::FOUND, "{}", callback.text());
    assert_eq!(callback.header("location"), Some("/swimlanes"));
    let cookies = cookies.expect("the callback sets both cookies");

    // The cookie works on a route of identity, and on a route of another module: the platform's event stream, which asks the hub's
    // `SessionAuth` and not identity's guard.
    let me = send(app, Req::get("/api/v1/me").session(&cookies)).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    assert_eq!(me.json()["status"], "active");
    assert_eq!(
        me.json()["role"],
        "admin",
        "the first admin is named in the settings"
    );
    let stream = open(app, Req::get("/api/v1/events").session(&cookies)).await;
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(
        stream.headers().get("content-type").unwrap(),
        "text/event-stream",
        "the platform's guard accepted the identity module's session"
    );

    // The state changes of the sign-in are in the chain, one event each, in the transaction that made them: the first admin got a
    // role, and a session began.
    assert_eq!(audit_actions(&rig).await, ["role_changed", "session_signed_in"]);
}

#[tokio::test]
async fn a_person_nobody_named_signs_in_pending_and_gets_no_access() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let stranger = "99999999-9999-4999-8999-999999999999";
    let (callback, cookies) = rig.sign_in_on(app, stranger, None).await;
    assert_eq!(callback.status, StatusCode::FOUND, "{}", callback.text());
    let cookies = cookies.expect("a pending user still has a session, to ask for access");
    for path in ["/api/v1/me", "/api/v1/events", "/api/v1/admin/users"] {
        let denied = send(app, Req::get(path).session(&cookies)).await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN, "{path}: {}", denied.text());
    }
}

// ------------------------------------------------------------------------------------------------------ S2: sessions

#[tokio::test]
async fn the_session_cookie_has_the_s2_attributes() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let (callback, cookies) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    let cookies = cookies.unwrap();

    // `__Host-` forbids a Domain and requires Secure and Path=/; HttpOnly keeps the session id from the page; Lax with the
    // token and `Origin` is the CSRF defence; the CSRF cookie is readable on purpose.
    let session = callback.cookie_line("__Host-lk_session").unwrap();
    assert_eq!(
        session,
        format!(
            "__Host-lk_session={}; Max-Age=43200; Path=/; Secure; HttpOnly; SameSite=Lax",
            cookies.session
        )
    );
    let csrf = callback.cookie_line("__Host-lk_csrf").unwrap();
    assert_eq!(
        csrf,
        format!(
            "__Host-lk_csrf={}; Max-Age=43200; Path=/; Secure; SameSite=Lax",
            cookies.csrf
        )
    );
    assert_eq!(callback.header("cache-control"), Some("no-store"));
    assert_eq!(
        (cookies.session.len(), cookies.csrf.len()),
        (43, 43),
        "256 bits each, base64url"
    );
    assert_ne!(cookies.session, cookies.csrf);
    // The database holds the hash of the session id and never the id.
    let rows: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT session_hash FROM sessions")
        .fetch_all(rig.pool())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.len(), 32);
    assert!(!String::from_utf8_lossy(&rows[0].0).contains(&cookies.session));
}

#[tokio::test]
async fn signing_in_again_replaces_the_session_that_was_presented() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let (_, first) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    let first = first.unwrap();
    assert_eq!(
        send(app, Req::get("/api/v1/me").session(&first)).await.status,
        StatusCode::OK
    );

    let (_, second) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, Some(&first)).await;
    let second = second.unwrap();
    assert_ne!(
        first.session, second.session,
        "a new id at every sign-in (S2: fixation)"
    );
    assert_eq!(
        send(app, Req::get("/api/v1/me").session(&first)).await.status,
        StatusCode::UNAUTHORIZED,
        "the presented session was ended"
    );
    assert_eq!(
        send(app, Req::get("/api/v1/me").session(&second)).await.status,
        StatusCode::OK
    );
}

/// The hash that the sessions table keeps for a session cookie.
fn hash_of(cookies: &Cookies) -> Vec<u8> {
    hub_identity::session::cookie::RandomToken::from_text(&cookies.session)
        .expect("a session cookie")
        .hash()
        .as_bytes()
        .to_vec()
}

/// `last_seen_at` of this session, in Unix milliseconds.
async fn last_seen_ms(rig: &IdentityRig, cookies: &Cookies) -> i64 {
    sqlx::query_scalar(
        "SELECT (extract(epoch FROM last_seen_at) * 1000)::bigint FROM sessions WHERE session_hash = $1",
    )
    .bind(hash_of(cookies))
    .fetch_one(rig.pool())
    .await
    .unwrap()
}

/// What the batcher writes once a session has been used: `last_seen_at` is the clock's now. The batcher flushes in real time every
/// few seconds, which a test that moves the clock by hours cannot wait for at every step, so after the first step (which does wait,
/// below) this stands in for it.
async fn touch(rig: &IdentityRig, cookies: &Cookies) {
    use hub_platform::clock::Clock;
    let now = rig.clock.now().unix_millis();
    #[allow(clippy::cast_precision_loss)]
    sqlx::query("UPDATE sessions SET last_seen_at = to_timestamp($1::double precision / 1000.0) WHERE session_hash = $2")
        .bind(now as f64)
        .bind(hash_of(cookies))
        .execute(rig.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_session_ends_at_its_absolute_and_its_idle_limit() {
    use hub_platform::clock::Clock;
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    // Three browsers of one person, all signed in at the instant the mock identity provider's tokens are valid for. (A token is
    // only good for an hour, so every sign-in is before the clock moves.)
    let mut sessions = Vec::new();
    for _ in 0..3 {
        sessions.push(rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await.1.unwrap());
    }
    let [busy, near_idle, idle] = <[Cookies; 3]>::try_from(sessions).unwrap();
    let t0 = rig.clock.now().unix_millis();

    // A request in the composed hub marks its session used: the batcher writes `last_seen_at` (really, in a few seconds).
    rig.clock.advance_secs(120);
    assert_eq!(me(app, &busy).await, StatusCode::OK);
    eventually(
        Duration::from_secs(15),
        "the batcher writing last_seen_at",
        || async { last_seen_ms(&rig, &busy).await >= t0 + 119_000 },
    )
    .await;

    // Left alone, a session ends after 8 hours (the idle limit), though it has four hours left; just under, it is still good.
    rig.clock.set_ms(t0 + 8 * 3_600_000 - 60_000);
    assert_eq!(
        me(app, &near_idle).await,
        StatusCode::OK,
        "just under 8 hours idle"
    );
    rig.clock.set_ms(t0 + 8 * 3_600_000 + 60_000);
    assert_eq!(me(app, &idle).await, StatusCode::UNAUTHORIZED, "8 hours idle");

    // Used every few hours, a session lives to its absolute end (12 hours after sign-in) and no further.
    for hours in [9, 11] {
        rig.clock.set_ms(t0 + hours * 3_600_000);
        touch(&rig, &busy).await;
        assert_eq!(
            me(app, &busy).await,
            StatusCode::OK,
            "{hours} hours after sign-in, in use"
        );
    }
    rig.clock.set_ms(t0 + 12 * 3_600_000 + 60_000);
    touch(&rig, &busy).await;
    assert_eq!(
        me(app, &busy).await,
        StatusCode::UNAUTHORIZED,
        "12 hours after sign-in, however busy"
    );
}

#[tokio::test]
async fn sign_out_ends_the_session_and_its_live_stream() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    // The same person on two browsers: two sessions, one user.
    let (_, laptop) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    let (_, phone) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    let (laptop, phone) = (laptop.unwrap(), phone.unwrap());
    let mut laptop_stream = frames(open(app, Req::get("/api/v1/events").session(&laptop)).await);
    let mut phone_stream = frames(open(app, Req::get("/api/v1/events").session(&phone)).await);
    assert!(laptop_stream.next().await.unwrap().starts_with("retry:"));
    assert!(phone_stream.next().await.unwrap().starts_with("retry:"));

    let out = send(app, Req::post("/auth/logout").with_token(&laptop)).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.text());
    assert_eq!(
        send(app, Req::get("/api/v1/me").session(&laptop)).await.status,
        StatusCode::UNAUTHORIZED,
        "the cookie is dead"
    );

    // The laptop's stream ends, within a beat or two; the phone's goes on, though it is the same user (G2: per session).
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while laptop_stream.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the signed-out browser's stream is still open");
    let beat = tokio::time::timeout(Duration::from_secs(2), phone_stream.next()).await;
    assert!(
        matches!(beat, Ok(Some(_))),
        "the user's other browser lost its stream: {beat:?}"
    );
    assert_eq!(
        send(app, Req::get("/api/v1/me").session(&phone)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        audit_actions(&rig).await.last().map(String::as_str),
        Some("session_signed_out")
    );
}

#[tokio::test]
async fn a_logout_without_the_token_does_not_end_the_session() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let (_, cookies) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    let cookies = cookies.unwrap();
    // A page on another site can make the browser send the cookie, but not the token or the `Origin`.
    let forged = send(app, Req::post("/auth/logout").session(&cookies)).await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    assert_eq!(
        send(app, Req::get("/api/v1/me").session(&cookies)).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_role_change_on_one_replica_ends_sessions_on_the_other_within_a_second() {
    let Some(rig) = IdentityRig::start(Options {
        replicas: 2,
        ..Options::default()
    })
    .await
    else {
        return;
    };
    let (a, b) = (rig.replicas[0].web(), rig.replicas[1].web());
    let (_, admin) = rig.sign_in_on(a, rig::FIRST_ADMIN_OID, None).await;
    let admin = admin.unwrap();
    let (user, member) = rig.signed_in(7, Some(Role::Viewer), UserStatus::Active).await;

    // The member's session is warm in B's cache, and in a stream on B.
    assert_eq!(me(b, &member).await, StatusCode::OK);
    let mut stream = frames(open(b, Req::get("/api/v1/events").session(&member)).await);
    assert!(stream.next().await.unwrap().starts_with("retry:"));

    // The admin disables the member on A.
    let patch = send(
        a,
        Req::new(http::Method::PATCH, &format!("/api/v1/admin/users/{user}"))
            .with_token(&admin)
            .json(&serde_json::json!({"status": "disabled"})),
    )
    .await;
    assert_eq!(patch.status, StatusCode::OK, "{}", patch.text());

    // B hears of it through the event bus: the next request is refused, and the stream is closed.
    let told = Instant::now();
    let took = eventually(
        Duration::from_secs(1),
        "B refusing the disabled user's cookie",
        || async { me(b, &member).await != StatusCode::OK },
    )
    .await;
    assert!(took < Duration::from_secs(1), "{took:?}");
    let refused = me(b, &member).await;
    assert!(
        refused == StatusCode::FORBIDDEN || refused == StatusCode::UNAUTHORIZED,
        "{refused}"
    );
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        while stream.next().await.is_some() {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "the stream of a disabled user stayed open for {:?}",
        told.elapsed()
    );
}

// ------------------------------------------------------------------------------------------------------ S1/S4: every route

/// The sessions of three people who must get nothing from a guarded route, whatever it is.
async fn people_without_access(rig: &IdentityRig) -> Vec<(&'static str, Cookies)> {
    let (_, pending) = rig.signed_in(11, None, UserStatus::Pending).await;
    let (_, disabled) = rig.signed_in(12, Some(Role::Admin), UserStatus::Disabled).await;
    let (_, no_role) = rig.signed_in(13, None, UserStatus::Active).await;
    vec![
        ("pending", pending),
        ("disabled admin", disabled),
        ("active without a role", no_role),
    ]
}

#[tokio::test]
async fn pending_and_disabled_users_get_403_on_every_composed_route() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let routes = &rig.one().hub.routes;
    let guarded: Vec<_> = routes
        .iter()
        .filter(|route| matches!(route.guard, RouteGuard::Role(_)))
        .collect();
    assert!(
        guarded.len() >= 8,
        "the table is the identity routes and the event stream: {}",
        guarded.len()
    );
    for (who, cookies) in people_without_access(&rig).await {
        for route in &guarded {
            let method = http::Method::from_bytes(route.method.as_bytes()).unwrap();
            // A browser's own request, token and `Origin` and all, so that only the guard is left to refuse it.
            let request = Req::new(method.clone(), &concrete(&route.path))
                .with_token(&cookies)
                .json(&serde_json::json!({}));
            let reply = send(app, request).await;
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "{who} on {method} {}: {}",
                route.path,
                reply.text()
            );
        }
    }
}

#[tokio::test]
async fn default_deny_holds_with_static_files_on() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let routes = &rig.one().hub.routes;

    // Every registered route that is not public refuses a caller with no cookie and a caller with a garbage cookie.
    for route in routes.iter().filter(|r| matches!(r.guard, RouteGuard::Role(_))) {
        let method = http::Method::from_bytes(route.method.as_bytes()).unwrap();
        for (what, request) in [
            ("no cookie", Req::new(method.clone(), &concrete(&route.path))),
            (
                "a garbage cookie",
                Req::new(method.clone(), &concrete(&route.path)).cookie("__Host-lk_session", "garbage"),
            ),
            (
                "a well-formed cookie that names no session",
                Req::new(method.clone(), &concrete(&route.path))
                    .cookie("__Host-lk_session", &rig::unknown_session_cookie()),
            ),
        ] {
            let reply = send(app, request).await;
            assert_eq!(
                reply.status,
                StatusCode::UNAUTHORIZED,
                "{what} on {method} {}",
                route.path
            );
        }
    }

    // A path under a prefix the server owns that no route answers is a 404 and never the web app (S4).
    let (_, cookies) = rig.signed_in(21, Some(Role::Admin), UserStatus::Active).await;
    for path in [
        "/api/v1/nonexistent",
        "/api/v2/me",
        "/api",
        "/auth/nonexistent",
        "/auth",
        "/hooks/nonexistent",
        "/mcp",
        "/mcp/nonexistent",
        "/.well-known/nonexistent",
        "/.well-known/oauth-protected-resource",
    ] {
        let reply = send(app, Req::get(path)).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        assert!(
            !reply.text().contains(r#"<div id="root">"#),
            "{path} answered with the web app"
        );
        // With a cookie too: a session does not turn a missing route into the app.
        let with = send(app, Req::get(path).session(&cookies)).await;
        assert_eq!(with.status, StatusCode::NOT_FOUND, "{path} with a session");
    }

    // The web app itself is for everyone: the shell for a client-side route, with the page's own headers.
    for path in ["/", "/swimlanes", "/swimlanes/sl-01/files"] {
        let page = send(app, Req::get(path)).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(page.text().contains(r#"<div id="root">"#));
        assert!(
            page.header("content-security-policy").is_some(),
            "{path}: the page has a CSP"
        );
    }
}

/// The composed table, held against `api/openapi.yaml` by the repository's own S4 harness: every route has a declared guard, and for
/// every route that the contract also names, the role the contract gives it is the role it was registered with. The routes the
/// contract names and no module mounts yet, and the routes no operation names, are allowances here by kind; the explicit list, with
/// a reason and an owner for each, is T8's (`route_table_full`).
#[tokio::test]
async fn the_composed_table_passes_the_route_table_check() {
    use ports::conformance::route_guard::{
        Allowances, RegisteredRoute, assert_route_table, spec_operations_from_openapi,
    };

    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml");
    let spec = spec_operations_from_openapi(&std::fs::read_to_string(path).unwrap()).unwrap();
    let routes = &rig.one().hub.routes;
    let mounted: std::collections::BTreeSet<(String, String)> = routes
        .iter()
        .map(|r| (r.method.clone(), r.path.clone()))
        .collect();
    let named: std::collections::BTreeSet<(String, String)> =
        spec.iter().map(|o| (o.method.clone(), o.path.clone())).collect();
    let mut allow = Allowances::default();
    for op in &spec {
        if !mounted.contains(&(op.method.clone(), op.path.clone())) {
            allow = allow.spec_only(
                &op.method,
                &op.path,
                "mounted by a later task or another prompt (T8 lists each)",
            );
        }
    }
    for route in routes {
        if !named.contains(&(route.method.clone(), route.path.clone())) {
            allow = allow.route_only(
                &route.method,
                &route.path,
                "sign-in and the event stream are not operations of the contract",
            );
        }
    }
    let registered: Vec<RegisteredRoute> = routes.iter().map(hub_platform::RouteSpec::registered).collect();
    assert_route_table(&registered, &spec, &allow);
    // What identity registered is all there, with the guard identity declared.
    for (method, path) in [
        ("GET", "/api/v1/me"),
        ("POST", "/api/v1/me/access-request"),
        ("GET", "/api/v1/admin/users"),
        ("PATCH", "/api/v1/admin/users/{id}"),
        ("POST", "/auth/logout"),
        ("GET", "/auth/login"),
        ("GET", "/auth/callback"),
        ("GET", "/api/v1/events"),
    ] {
        assert!(
            mounted.contains(&(method.to_owned(), path.to_owned())),
            "{method} {path} is not mounted"
        );
    }
}

// ------------------------------------------------------------------------------------------------------ S9: audit jobs

#[tokio::test]
async fn the_checkpoint_and_verify_jobs_run_on_one_replica_and_stop_on_shutdown() {
    let Some(mut rig) = IdentityRig::start(Options {
        replicas: 2,
        schedule: AuditSchedule {
            checkpoint_every: Duration::from_millis(150),
            verify_first_delay: Duration::from_millis(200),
            verify_every: Duration::from_millis(300),
        },
        ..Options::default()
    })
    .await
    else {
        return;
    };
    // Somebody else holds the checkpoint lease: neither replica writes a checkpoint, though both have a head to sign.
    let outsider = hub_platform::leases::PgLeases::new(
        rig.pool().clone(),
        &hub_platform::ca::profile::PeerName::parse("hub-9").unwrap(),
    );
    // The replicas' own jobs hold the lease for a moment at each tick, so wait for a moment when it is free.
    let deadline = Instant::now() + Duration::from_secs(10);
    let held = loop {
        if let Some(guard) = outsider
            .try_acquire(
                hub_identity::audit::checkpoint::LEASE_NAME,
                Duration::from_secs(60),
            )
            .await
            .unwrap()
        {
            break guard;
        }
        assert!(Instant::now() < deadline, "the checkpoint lease was never free");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let (_, _) = rig.sign_in_on(rig.app(), rig::FIRST_ADMIN_OID, None).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        rig.checkpoints.len(),
        0,
        "a replica wrote a checkpoint without the lease"
    );

    // Released, exactly one object appears for the head, and exactly one replica wrote it.
    held.release().await.unwrap();
    eventually(Duration::from_secs(10), "a checkpoint in the bucket", || async {
        rig.checkpoints.len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        rig.checkpoints.len(),
        1,
        "the head did not move, so nothing more is written"
    );
    let written = [
        counter(&rig, 0, "audit_checkpoints_written_total"),
        counter(&rig, 1, "audit_checkpoints_written_total"),
    ];
    assert_eq!(
        written.iter().sum::<u64>(),
        1,
        "written by one replica: {written:?}"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_checkpoints")
        .fetch_one(rig.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);

    // The nightly verification ran and found the chain whole, and it ran once per round, not once per replica.
    eventually(Duration::from_secs(10), "a verification run", || async {
        counter(&rig, 0, "audit_verify_runs_total") + counter(&rig, 1, "audit_verify_runs_total") >= 1
    })
    .await;
    assert_eq!(
        counter(&rig, 0, "audit_verify_failures_total") + counter(&rig, 1, "audit_verify_failures_total"),
        0
    );

    // When the hub stops, the jobs stop: nothing is written or verified afterwards, and a signed-in head is left unsigned.
    for replica in &mut rig.replicas {
        replica.shut_down().await;
    }
    let (runs, count) = (
        counter(&rig, 0, "audit_verify_runs_total") + counter(&rig, 1, "audit_verify_runs_total"),
        rig.checkpoints.len(),
    );
    sqlx::query("SELECT 1").execute(rig.pool()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        counter(&rig, 0, "audit_verify_runs_total") + counter(&rig, 1, "audit_verify_runs_total"),
        runs,
        "a verification ran after shutdown"
    );
    assert_eq!(rig.checkpoints.len(), count);
}

// ------------------------------------------------------------------------------------------------------ S10, P13

#[tokio::test]
async fn metrics_labels_stay_closed_after_composition() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let (_, cookies) = rig.signed_in(31, Some(Role::Admin), UserStatus::Active).await;
    // A hundred distinct paths, ids and cookies: none of them may become a series.
    for n in 0..100 {
        let _ = send(
            app,
            Req::get(&format!("/api/v1/admin/users/{}", rig::user_id(n))).session(&cookies),
        )
        .await;
        let _ = send(app, Req::get(&format!("/api/v1/never/{n}/{}", "q".repeat(8)))).await;
        let _ = send(
            app,
            Req::get("/api/v1/me").cookie("__Host-lk_session", &rig::unknown_session_cookie()),
        )
        .await;
    }
    let text = rig.one().hub.metrics.encode();
    let paths: std::collections::BTreeSet<&str> = text
        .lines()
        .filter_map(|line| line.split("route=\"").nth(1))
        .filter_map(|rest| rest.split('"').next())
        .collect();
    assert!(paths.len() <= 40, "{} route labels: {paths:?}", paths.len());
    for path in &paths {
        assert!(
            !path.contains("qqqq") && !path.contains("never") && !path.contains("0000"),
            "{path}"
        );
    }
    // The identity module's own counters are exported, with no label at all.
    assert!(
        text.contains("lanekeeper_audit_checkpoints_written_total"),
        "{text}"
    );
    for line in text.lines().filter(|l| l.starts_with("lanekeeper_audit_")) {
        assert!(!line.contains('{'), "{line}");
    }
    // And no cookie value, token or session id reaches the exposition.
    assert!(!text.contains(&cookies.session) && !text.contains(&cookies.csrf));
}

/// S10, S21: through the hub's own layers, as through identity's, nothing that authenticates and nothing about a person is written to
/// a log. The flows are the ones that log most: a sign-in, a refused state-changing request, a cookie that names nothing, a callback
/// that fails, a role change, a sign-out. The capture is of everything `tracing` records on this thread, every level.
#[tokio::test]
async fn no_secret_and_no_personal_data_reach_the_logs_through_the_composed_hub() {
    let (log, _guard) = log_capture::install();
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();

    let (_, admin) = rig.sign_in_on(app, rig::FIRST_ADMIN_OID, None).await;
    let admin = admin.unwrap();
    let stranger_oid = "99999999-9999-4999-8999-999999999999";
    let (_, stranger) = rig.sign_in_on(app, stranger_oid, None).await;
    let stranger = stranger.unwrap();
    let (member_id, member) = rig.signed_in(51, Some(Role::Viewer), UserStatus::Active).await;

    // A refused state-changing request, in several ways; a cookie that names nothing; a callback that fails.
    for request in [
        Req::post("/api/v1/me/access-request").session(&member),
        Req::post("/api/v1/me/access-request")
            .session(&member)
            .header("origin", "https://evil.example"),
        Req::post("/api/v1/me/access-request")
            .session(&member)
            .header("x-csrf-token", &admin.csrf)
            .header("origin", mock_oidc::ORIGIN),
    ] {
        assert_eq!(send(app, request).await.status, StatusCode::FORBIDDEN);
    }
    let unknown = rig::unknown_session_cookie();
    let _ = send(app, Req::get("/api/v1/me").cookie("__Host-lk_session", &unknown)).await;
    let _ = send(
        app,
        Req::get("/auth/callback?code=the-auth-code&state=tampered").cookie("__Host-lk_login", "tampered"),
    )
    .await;
    let _ = send(app, Req::get("/api/v1/me").session(&stranger)).await;

    // A role change, and a sign-out.
    let patch = send(
        app,
        Req::new(http::Method::PATCH, &format!("/api/v1/admin/users/{member_id}"))
            .with_token(&admin)
            .json(&serde_json::json!({"role": "editor"})),
    )
    .await;
    assert_eq!(patch.status, StatusCode::OK, "{}", patch.text());
    assert_eq!(
        send(app, Req::post("/auth/logout").with_token(&admin))
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let text = log.text();
    // The capture works: the refusal of a state-changing request is logged, by its kind and not by its content.
    assert!(
        text.contains("state-changing request refused"),
        "nothing was captured:\n{text}"
    );

    let mut secrets: Vec<String> = rig.seen.lock().unwrap().clone();
    for cookies in [&admin, &stranger, &member] {
        secrets.extend([cookies.session.clone(), cookies.csrf.clone()]);
    }
    secrets.extend([
        unknown,
        mock_oidc::CLIENT_SECRET.to_owned(),
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, [5u8; 32]),
        "the-auth-code".to_owned(),
    ]);
    for secret in &secrets {
        assert!(
            secret.len() >= 8,
            "a planted value too short to mean anything: {secret}"
        );
        assert!(
            !text.contains(secret.as_str()),
            "a secret reached a log: {secret}"
        );
    }
    // Nor does anything about the person: the e-mail address and the name the identity provider vouched for.
    for personal in [
        format!("{stranger_oid}@example.com"),
        format!("{}@example.com", rig::FIRST_ADMIN_OID),
        "Ada Lovelace".to_owned(),
    ] {
        assert!(
            !text.contains(&personal),
            "personal data reached a log: {personal}"
        );
    }
}

#[tokio::test]
async fn an_sse_event_reaches_a_subscriber_within_one_second_through_the_composed_hub() {
    use ports::EventBus;
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    let app = rig.app();
    let (_, cookies) = rig.signed_in(41, Some(Role::Viewer), UserStatus::Active).await;
    let mut stream = frames(open(app, Req::get("/api/v1/events").session(&cookies)).await);
    assert!(stream.next().await.unwrap().starts_with("retry:"));

    // The event is published by the bus of the same replica, the way a write path does after its commit.
    let swimlane = SwimlaneId::parse("sit1").unwrap();
    let sent = Instant::now();
    rig.one()
        .bus
        .publish(DomainEvent::FindingsChanged {
            swimlane: swimlane.clone(),
        })
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(1), stream.next_event())
        .await
        .expect("the event arrived within one second")
        .unwrap();
    assert!(frame.contains("sit1"), "{frame}");
    assert!(sent.elapsed() < Duration::from_secs(1), "{:?}", sent.elapsed());
}

// ------------------------------------------------------------------------------------------------------ refusals

/// A value that is not a key, planted to see that no message repeats it.
const PLANTED: &str = "planted-cookie-key-98765";

#[tokio::test]
async fn a_missing_or_malformed_secret_stops_the_identity_module_and_names_the_variable() {
    let Some(rig) = IdentityRig::start(Options::default()).await else {
        return;
    };
    for (secret, value, want) in [
        ("entra-client-secret", None, IdentityBuildError::ClientSecret),
        ("login-cookie-key", None, IdentityBuildError::CookieKeyUnavailable),
        (
            "login-cookie-key",
            Some(PLANTED),
            IdentityBuildError::CookieKeyMalformed,
        ),
    ] {
        let secrets = Arc::new(secrets::MemorySecrets::default());
        secrets.put("entra-client-secret", mock_oidc::CLIENT_SECRET);
        secrets.put("login-cookie-key", &"A".repeat(43));
        match value {
            Some(value) => secrets.put(secret, value),
            None => secrets.remove(secret),
        }
        let error = rig.try_identity(secrets).await.unwrap_err();
        assert_eq!(error, want, "{secret}");
        // Fixed text that names the setting and nothing else.
        let message = error.to_string();
        assert!(message.contains("LK_ENTRA_"), "{message}");
        assert!(!message.contains(mock_oidc::CLIENT_SECRET) && !message.contains(PLANTED));
    }
}
