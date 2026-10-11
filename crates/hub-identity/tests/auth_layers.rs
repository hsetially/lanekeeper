//! T3 (S2, S3): the real `AuthLayer` and `CsrfLayer` in front of stand-in handlers, with a real Postgres behind the sessions.
//!
//! Every request here is a real HTTP request through the layers (`tower::ServiceExt::oneshot`); the CSRF rules are checked on
//! sessions the store made, so the token hash comparison runs against what the database holds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use axum::http::{Method, StatusCode};
use domain::{Role, UserStatus};
use support::sessions_rig::Replica;
use support::users_rig::{UsersRig, uid};
use support::web_rig::{ORIGIN, Req, app, app_with_exempt_hook, send, whoami_user};

/// A rig with an active editor (user 1), their session, and the app around one replica.
macro_rules! signed_in {
    () => {{
        let rig = UsersRig::new(db!()).await;
        rig.seed(1, Some(Role::Editor), UserStatus::Active).await;
        let replica = Replica::start_deaf(&rig.db, &rig.clock).await;
        let session = replica.create_session(&uid(1)).await;
        let app = app(&replica.sessions);
        (rig, replica, session, app)
    }};
}

// ---------------------------------------------------------------------------------------------------------------
// The auth layer

#[tokio::test]
async fn a_request_without_a_cookie_is_anonymous_and_gets_a_request_id() {
    let (_rig, _replica, _session, app) = signed_in!();
    let a = send(&app, Req::get("/whoami")).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.json()["anonymous"], true);
    let id = a
        .headers
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(a.json()["request_id"], id.as_str());
    let b = send(&app, Req::get("/whoami")).await;
    assert_ne!(
        b.headers.get("x-request-id").unwrap().to_str().unwrap(),
        id,
        "one id per request"
    );
}

#[tokio::test]
async fn a_request_id_sent_by_the_client_is_never_adopted() {
    let (_rig, _replica, _session, app) = signed_in!();
    let a = send(
        &app,
        Req::get("/whoami").header("x-request-id", "attacker-chosen"),
    )
    .await;
    assert_ne!(
        a.headers.get("x-request-id").unwrap().to_str().unwrap(),
        "attacker-chosen"
    );
    assert_ne!(a.json()["request_id"], "attacker-chosen");
}

#[tokio::test]
async fn a_valid_session_cookie_yields_the_users_context() {
    let (_rig, _replica, session, app) = signed_in!();
    let a = send(
        &app,
        Req::get("/whoami").cookie("__Host-lk_session", session.session.expose()),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.json()["anonymous"], false);
    assert_eq!(whoami_user(&a), Some(uid(1)));
    assert_eq!(a.json()["role"], "editor");
    assert_eq!(a.json()["status"], "active");
}

#[tokio::test]
async fn pending_and_disabled_users_keep_their_session_and_carry_their_status() {
    let rig = UsersRig::new(db!()).await;
    rig.seed(1, None, UserStatus::Pending).await;
    rig.seed(2, Some(Role::Viewer), UserStatus::Disabled).await;
    let replica = Replica::start_deaf(&rig.db, &rig.clock).await;
    let app = app(&replica.sessions);
    for (n, status) in [(1, "pending"), (2, "disabled")] {
        let s = replica.create_session(&uid(n)).await;
        let a = send(
            &app,
            Req::get("/whoami").cookie("__Host-lk_session", s.session.expose()),
        )
        .await;
        assert_eq!(
            a.json()["anonymous"],
            false,
            "{status}: the guard, not this layer, answers 403"
        );
        assert_eq!(a.json()["status"], status);
    }
}

#[tokio::test]
async fn unknown_malformed_duplicated_and_expired_cookies_are_anonymous() {
    let (rig, _replica, session, app) = signed_in!();
    let good = session.session.expose().clone();
    let stranger = hub_identity::session::RandomToken::generate().unwrap().to_text();
    for (what, req) in [
        (
            "unknown token",
            Req::get("/whoami").cookie("__Host-lk_session", stranger.expose()),
        ),
        (
            "not a token",
            Req::get("/whoami").cookie("__Host-lk_session", "garbage"),
        ),
        (
            "the same cookie twice",
            Req::get("/whoami")
                .cookie("__Host-lk_session", &good)
                .cookie("__Host-lk_session", &good),
        ),
        (
            "an unprefixed cookie",
            Req::get("/whoami").cookie("lk_session", &good),
        ),
    ] {
        let a = send(&app, req).await;
        assert_eq!(a.status, StatusCode::OK, "{what}");
        assert_eq!(a.json()["anonymous"], true, "{what}");
    }
    // The real one still works, and stops working after 12 hours.
    let ok = send(&app, Req::get("/whoami").cookie("__Host-lk_session", &good)).await;
    assert_eq!(ok.json()["anonymous"], false);
    rig.clock.advance_secs(12 * 3600);
    let late = send(&app, Req::get("/whoami").cookie("__Host-lk_session", &good)).await;
    assert_eq!(late.json()["anonymous"], true);
}

#[tokio::test]
async fn a_database_failure_is_503_and_not_anonymous() {
    let (_rig, replica, session, app) = signed_in!();
    replica.pool.close().await;
    let a = send(
        &app,
        Req::get("/whoami").cookie("__Host-lk_session", session.session.expose()),
    )
    .await;
    assert_eq!(
        a.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "fail closed: not an anonymous 200"
    );
    assert_eq!(a.headers.get("content-type").unwrap(), "application/problem+json");
    assert!(
        !a.body.to_lowercase().contains("sql") && !a.body.contains("pool"),
        "{}",
        a.body
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The CSRF layer (S3)

#[tokio::test]
async fn a_complete_state_changing_request_passes() {
    let (_rig, _replica, s, app) = signed_in!();
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let a = send(
            &app,
            Req::new(method.clone(), "/change").with_session(s.session.expose(), s.csrf.expose()),
        )
        .await;
        assert_eq!(a.status, StatusCode::NO_CONTENT, "{method}");
    }
}

#[tokio::test]
async fn post_without_csrf_header_is_403() {
    let (_rig, _replica, s, app) = signed_in!();
    let a = send(
        &app,
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .cookie("__Host-lk_csrf", s.csrf.expose())
            .origin(ORIGIN),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    // And without the cookie as well.
    let b = send(
        &app,
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .header("x-csrf-token", s.csrf.expose())
            .origin(ORIGIN),
    )
    .await;
    assert_eq!(
        b.status,
        StatusCode::FORBIDDEN,
        "a header with no cookie is not a double submit"
    );
}

#[tokio::test]
async fn post_with_header_not_matching_cookie_is_403() {
    let (_rig, _replica, s, app) = signed_in!();
    let other = hub_identity::session::RandomToken::generate().unwrap().to_text();
    let a = send(
        &app,
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .cookie("__Host-lk_csrf", s.csrf.expose())
            .header("x-csrf-token", other.expose())
            .origin(ORIGIN),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_with_cookie_and_header_equal_but_not_session_bound_is_403() {
    // An attacker who can plant a cookie (a sibling subdomain) plants a CSRF cookie of their own and sends the same value in
    // the header. The two match, but the hash is not the one stored with the victim's session.
    let (_rig, _replica, s, app) = signed_in!();
    let planted = hub_identity::session::RandomToken::generate().unwrap().to_text();
    let a = send(
        &app,
        Req::post("/change").with_session(s.session.expose(), planted.expose()),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    // The session's own token still works.
    let b = send(
        &app,
        Req::post("/change").with_session(s.session.expose(), s.csrf.expose()),
    )
    .await;
    assert_eq!(b.status, StatusCode::NO_CONTENT);
    // Another session's token is no better.
    let (_r2, _rep2, s2, _app2) = signed_in!();
    let c = send(
        &app,
        Req::post("/change").with_session(s.session.expose(), s2.csrf.expose()),
    )
    .await;
    assert_eq!(c.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_malformed_csrf_value_is_403() {
    let (_rig, _replica, s, app) = signed_in!();
    for bad in ["short", &"A".repeat(44), &"A".repeat(500), "not-base64-!!"] {
        let a = send(&app, Req::post("/change").with_session(s.session.expose(), bad)).await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "{bad:?}");
    }
    // An empty cookie of ours is a malformed `Cookie` header, and a request whose cookies cannot be read is anonymous: it
    // has no session to attack, and the guard answers 401.
    let empty = send(&app, Req::get("/whoami").with_session(s.session.expose(), "")).await;
    assert_eq!(empty.json()["anonymous"], true);
}

#[tokio::test]
async fn cross_origin_post_is_403() {
    let (_rig, _replica, s, app) = signed_in!();
    for origin in [
        "https://evil.example.com",
        "http://lanekeeper.example.com",
        "https://lanekeeper.example.com:8443",
        "https://lanekeeper.example.com.evil.example.com",
        "https://evil.example.com/https://lanekeeper.example.com",
        "https://lanekeeper.example.com/",
        "https://LANEKEEPER.example.com",
        "",
    ] {
        let a = send(
            &app,
            Req::post("/change")
                .cookie("__Host-lk_session", s.session.expose())
                .cookie("__Host-lk_csrf", s.csrf.expose())
                .header("x-csrf-token", s.csrf.expose())
                .origin(origin),
        )
        .await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "Origin {origin:?}");
    }
}

#[tokio::test]
async fn missing_origin_on_post_is_403() {
    let (_rig, _replica, s, app) = signed_in!();
    let a = send(
        &app,
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .cookie("__Host-lk_csrf", s.csrf.expose())
            .header("x-csrf-token", s.csrf.expose()),
    )
    .await;
    assert_eq!(
        a.status,
        StatusCode::FORBIDDEN,
        "a valid token alone is not enough: Origin is required too"
    );
}

#[tokio::test]
async fn null_origin_is_403() {
    let (_rig, _replica, s, app) = signed_in!();
    let a = send(
        &app,
        Req::post("/change")
            .with_session(s.session.expose(), s.csrf.expose())
            .origin("null"),
    )
    .await;
    // `with_session` set a good Origin first; two Origin headers are ambiguous and refused as well.
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let b = send(
        &app,
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .cookie("__Host-lk_csrf", s.csrf.expose())
            .header("x-csrf-token", s.csrf.expose())
            .origin("null"),
    )
    .await;
    assert_eq!(
        b.status,
        StatusCode::FORBIDDEN,
        "a sandboxed or redirected request sends Origin: null"
    );
}

#[tokio::test]
async fn two_csrf_headers_are_403() {
    let (_rig, _replica, s, app) = signed_in!();
    let a = send(
        &app,
        Req::post("/change")
            .with_session(s.session.expose(), s.csrf.expose())
            .header("x-csrf-token", s.csrf.expose()),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn put_patch_delete_are_checked_too() {
    let (_rig, _replica, s, app) = signed_in!();
    for method in [Method::PUT, Method::PATCH, Method::DELETE, Method::POST] {
        let no_token = send(
            &app,
            Req::new(method.clone(), "/change")
                .cookie("__Host-lk_session", s.session.expose())
                .origin(ORIGIN),
        )
        .await;
        assert_eq!(no_token.status, StatusCode::FORBIDDEN, "{method} without a token");
        let foreign = send(
            &app,
            Req::new(method.clone(), "/change")
                .cookie("__Host-lk_session", s.session.expose())
                .cookie("__Host-lk_csrf", s.csrf.expose())
                .header("x-csrf-token", s.csrf.expose())
                .origin("https://evil.example.com"),
        )
        .await;
        assert_eq!(
            foreign.status,
            StatusCode::FORBIDDEN,
            "{method} from another origin"
        );
    }
    // Methods the app does not use are not "safe" by default.
    for method in ["TRACE", "CONNECT", "PROPFIND"] {
        let a = send(
            &app,
            Req::new(Method::from_bytes(method.as_bytes()).unwrap(), "/change")
                .cookie("__Host-lk_session", s.session.expose()),
        )
        .await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "{method}");
    }
}

#[tokio::test]
async fn get_is_not_checked_but_never_changes_state() {
    let (_rig, _replica, s, app) = signed_in!();
    // Safe methods carry no CSRF token and may come from anywhere: reading is not what CSRF protects.
    for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
        let a = send(
            &app,
            Req::new(method.clone(), "/whoami")
                .cookie("__Host-lk_session", s.session.expose())
                .origin("https://evil.example.com"),
        )
        .await;
        assert!(
            a.status.is_success() || a.status == StatusCode::METHOD_NOT_ALLOWED,
            "{method}: {}",
            a.status
        );
    }
    // The crate has no GET route that writes: the auth routes that change state are POST (logout) or end in a redirect that
    // changes nothing before the state, nonce and code have been checked (login, callback).
    let routes =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes/auth.rs"))
            .unwrap();
    assert!(routes.contains("post(logout)"), "logout is a POST");
}

#[tokio::test]
async fn an_anonymous_state_changing_request_reaches_the_guard() {
    // With no session there is no ambient authority for CSRF to protect: the request goes on to the guard, which answers 401
    // (T4). Both an absent cookie and a stale one are anonymous.
    let (_rig, _replica, _s, app) = signed_in!();
    let a = send(&app, Req::post("/change")).await;
    assert_eq!(
        a.status,
        StatusCode::NO_CONTENT,
        "the stand-in handler stands for the guard"
    );
    let stale = hub_identity::session::RandomToken::generate().unwrap().to_text();
    let b = send(
        &app,
        Req::post("/change").cookie("__Host-lk_session", stale.expose()),
    )
    .await;
    assert_eq!(b.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn csrf_failures_are_one_generic_problem_whatever_failed() {
    let (_rig, _replica, s, app) = signed_in!();
    let mut bodies = Vec::new();
    for req in [
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .origin(ORIGIN),
        Req::post("/change").with_session(s.session.expose(), "x"),
        Req::post("/change")
            .cookie("__Host-lk_session", s.session.expose())
            .origin("https://evil.example.com"),
    ] {
        let a = send(&app, req).await;
        assert_eq!(a.status, StatusCode::FORBIDDEN);
        assert_eq!(a.headers.get("content-type").unwrap(), "application/problem+json");
        assert!(a.headers.get("x-request-id").is_some());
        let mut v: serde_json::Value = serde_json::from_str(&a.body).unwrap();
        assert!(
            v["request_id"].is_string(),
            "the problem quotes the request id for the logs"
        );
        v.as_object_mut().unwrap().remove("request_id");
        bodies.push(v.to_string());
    }
    assert!(
        bodies.windows(2).all(|w| w[0] == w[1]),
        "the answer does not tell an attacker which check failed: {bodies:?}"
    );
    let v: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(v["status"], 403);
    assert_eq!(v["type"], "urn:lanekeeper:problem:csrf");
    assert!(
        !bodies[0].contains(s.csrf.expose().as_str()) && !bodies[0].contains(s.session.expose().as_str())
    );
}

#[tokio::test]
async fn an_exempt_route_skips_the_check_and_the_others_do_not() {
    let rig = UsersRig::new(db!()).await;
    rig.seed(1, Some(Role::Editor), UserStatus::Active).await;
    let replica = Replica::start_deaf(&rig.db, &rig.clock).await;
    let app = app_with_exempt_hook(&replica.sessions);
    // A webhook from another origin, with no token: let through (it authenticates itself with an HMAC).
    let hook = send(&app, Req::post("/hook").origin("https://github.com")).await;
    assert_eq!(hook.status, StatusCode::NO_CONTENT);
    // Even when the sender happens to carry a session cookie.
    let s = replica.create_session(&uid(1)).await;
    let hook = send(
        &app,
        Req::post("/hook").cookie("__Host-lk_session", s.session.expose()),
    )
    .await;
    assert_eq!(hook.status, StatusCode::NO_CONTENT);
    // The route next to it is still protected.
    let change = send(
        &app,
        Req::post("/change").cookie("__Host-lk_session", s.session.expose()),
    )
    .await;
    assert_eq!(change.status, StatusCode::FORBIDDEN);
}
