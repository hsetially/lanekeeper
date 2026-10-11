//! T3 (S1, S2, S3, S9, S10): `/auth/login`, `/auth/callback` and `/auth/logout` behind the real layers, against the mock Entra
//! and a real Postgres. The audit chain is the real `PgAuditLog`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{StatusCode, header};
use domain::{Role, UserStatus};
use hub_identity::session::cookie::RandomToken;
use support::login_rig::Begun;
use support::mock_oidc::{CLIENT_SECRET, TokenSpec};
use support::users_rig::FailingAudit;
use support::web_rig::{ORIGIN, Req, RoutesRig, cookie_line, cookie_value, mock_user, send, whoami_user};

/// A rig with the mock identity already known as an active viewer.
macro_rules! viewer {
    () => {{
        let rig = RoutesRig::new(db!()).await;
        rig.seed_mock_user(Some(Role::Viewer), UserStatus::Active).await;
        rig
    }};
}

async fn session_rows(rig: &RoutesRig) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&rig.replica.pool)
        .await
        .unwrap()
}

async fn audit_actions(rig: &RoutesRig) -> Vec<String> {
    rig.users.audit_actions().await
}

// ---------------------------------------------------------------------------------------------------------------
// login

#[tokio::test]
async fn login_redirects_to_entra_and_sets_the_sealed_login_cookie() {
    let rig = viewer!();
    let a = send(&rig.app, Req::get("/auth/login?return_to=%2Fswimlanes")).await;
    assert_eq!(a.status, StatusCode::FOUND);
    let location = a.headers.get(header::LOCATION).unwrap().to_str().unwrap();
    assert!(location.starts_with("https://login.test/"), "{location}");
    assert!(
        location.contains("code_challenge_method=S256")
            && location.contains("state=")
            && location.contains("nonce=")
    );
    let cookies = a.set_cookies();
    let line = cookie_line(&cookies, "__Host-lk_login").unwrap();
    assert!(
        line.ends_with("; Max-Age=600; Path=/; Secure; HttpOnly; SameSite=Lax"),
        "{line}"
    );
    assert!(
        !line.contains("swimlanes"),
        "the login state is sealed, not readable"
    );
    assert_eq!(a.headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(cookies.len(), 1, "login sets no session");
}

#[tokio::test]
async fn login_with_a_hostile_or_oversize_return_to_still_works_and_goes_home() {
    let rig = viewer!();
    for hostile in [
        "//evil.example.com",
        "https://evil.example.com",
        "/\\evil.example.com",
        "javascript:alert(1)",
    ] {
        let begun = rig.begin(Some(hostile)).await;
        let a = rig
            .callback(&begun, TokenSpec::valid(begun.param("nonce")), &[])
            .await;
        assert_eq!(a.status, StatusCode::FOUND, "{hostile}");
        assert_eq!(
            a.headers.get(header::LOCATION).unwrap(),
            "/",
            "{hostile}: never an absolute URL"
        );
    }
    let huge = send(
        &rig.app,
        Req::get(&format!("/auth/login?return_to=/{}", "a".repeat(5000))),
    )
    .await;
    assert_eq!(huge.status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------------------------------------------
// callback

#[tokio::test]
async fn callback_signs_in_sets_exact_cookies_and_redirects() {
    let rig = viewer!();
    let a = rig.sign_in(&[]).await;
    assert_eq!(a.status, StatusCode::FOUND, "{}", a.body);
    assert_eq!(a.headers.get(header::LOCATION).unwrap(), "/swimlanes");
    assert_eq!(a.headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    let cookies = a.set_cookies();
    assert_eq!(cookies.len(), 3, "{cookies:?}");

    let session = cookie_value(&cookies, "__Host-lk_session").unwrap();
    let csrf = cookie_value(&cookies, "__Host-lk_csrf").unwrap();
    assert_eq!((session.len(), csrf.len()), (43, 43));
    assert_ne!(session, csrf);
    assert_eq!(
        cookie_line(&cookies, "__Host-lk_session").unwrap(),
        format!("__Host-lk_session={session}; Max-Age=43200; Path=/; Secure; HttpOnly; SameSite=Lax")
    );
    assert_eq!(
        cookie_line(&cookies, "__Host-lk_csrf").unwrap(),
        format!("__Host-lk_csrf={csrf}; Max-Age=43200; Path=/; Secure; SameSite=Lax")
    );
    assert_eq!(
        cookie_line(&cookies, "__Host-lk_login").unwrap(),
        "__Host-lk_login=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; HttpOnly; SameSite=Lax",
        "the login state is single use"
    );

    // The session works, and is the mock user's.
    let me = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", &session),
    )
    .await;
    assert_eq!(whoami_user(&me), Some(mock_user()));
    assert_eq!(me.json()["status"], "active");
    // One audit event, by the user, carrying the request id of the callback.
    assert_eq!(audit_actions(&rig).await, ["session_signed_in"]);
    let event: String = sqlx::query_scalar("SELECT event_json FROM audit_events ORDER BY seq DESC LIMIT 1")
        .fetch_one(&rig.replica.pool)
        .await
        .unwrap();
    let event: serde_json::Value = serde_json::from_str(&event).unwrap();
    assert_eq!(event["actor"]["user"], mock_user().to_string());
    assert_eq!(event["via"], "ui");
    assert_eq!(
        event["request_id"],
        a.headers.get("x-request-id").unwrap().to_str().unwrap()
    );
    assert!(!event.to_string().contains(&session) && !event.to_string().contains(&csrf));
}

#[tokio::test]
async fn sign_in_issues_new_session_and_deletes_the_presented_one() {
    let rig = viewer!();
    let first = rig.sign_in(&[]).await;
    let old = cookie_value(&first.set_cookies(), "__Host-lk_session").unwrap();
    assert!(
        whoami_user(&send(&rig.app, Req::get("/whoami").cookie("__Host-lk_session", &old)).await).is_some()
    );

    // Signing in again with the old session cookie in the browser replaces it.
    let second = rig.sign_in(&[("__Host-lk_session", &old)]).await;
    assert_eq!(second.status, StatusCode::FOUND, "{}", second.body);
    let new = cookie_value(&second.set_cookies(), "__Host-lk_session").unwrap();
    assert_ne!(new, old);
    assert_eq!(
        session_rows(&rig).await,
        1,
        "the old row is gone and one new one exists"
    );
    let gone = RandomToken::from_text(&old).unwrap().hash();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE session_hash = $1")
        .bind(gone.as_bytes().as_slice())
        .fetch_one(&rig.replica.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // The cached copy of the old session is evicted too, on this and every replica: it is refused at once.
    rig.replica.wait_for_events(1, Duration::from_secs(1)).await;
    let stale = send(&rig.app, Req::get("/whoami").cookie("__Host-lk_session", &old)).await;
    assert_eq!(stale.json()["anonymous"], true);
    let fresh = send(&rig.app, Req::get("/whoami").cookie("__Host-lk_session", &new)).await;
    assert_eq!(whoami_user(&fresh), Some(mock_user()));
    let diff: String = sqlx::query_scalar("SELECT event_json FROM audit_events ORDER BY seq DESC LIMIT 1")
        .fetch_one(&rig.replica.pool)
        .await
        .unwrap();
    assert!(
        diff.contains("replaced the session the browser presented"),
        "{diff}"
    );
}

#[tokio::test]
async fn session_id_supplied_by_attacker_is_never_adopted() {
    let rig = viewer!();
    // The attacker picks a well-formed session id and plants it in the victim's browser before sign-in.
    let planted = RandomToken::generate().unwrap().to_text();
    let a = rig.sign_in(&[("__Host-lk_session", planted.expose())]).await;
    assert_eq!(a.status, StatusCode::FOUND, "{}", a.body);
    let issued = cookie_value(&a.set_cookies(), "__Host-lk_session").unwrap();
    assert_ne!(
        &issued,
        planted.expose(),
        "the server makes the id; it never takes the browser's"
    );
    // The planted id names nothing, before and after.
    let hash = RandomToken::from_text(planted.expose()).unwrap().hash();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE session_hash = $1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&rig.replica.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let attacker_view = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", planted.expose()),
    )
    .await;
    assert_eq!(attacker_view.json()["anonymous"], true);
    // A planted CSRF cookie is not adopted either: the new token is not the planted one.
    let b = rig.sign_in(&[("__Host-lk_csrf", planted.expose())]).await;
    let csrf = cookie_value(&b.set_cookies(), "__Host-lk_csrf").unwrap();
    assert_ne!(&csrf, planted.expose());
}

#[tokio::test]
async fn an_attackers_own_session_presented_at_sign_in_is_ended() {
    // Session fixation the other way round: the attacker signs in as themselves and plants *that* session in the victim's
    // browser. When the victim signs in, the planted session is deleted, so it cannot be used to ride along.
    let rig = viewer!();
    let attacker = rig.sign_in(&[]).await;
    let attackers_session = cookie_value(&attacker.set_cookies(), "__Host-lk_session").unwrap();
    let victim = rig.sign_in(&[("__Host-lk_session", &attackers_session)]).await;
    assert_ne!(
        cookie_value(&victim.set_cookies(), "__Host-lk_session").unwrap(),
        attackers_session
    );
    rig.replica.wait_for_events(1, Duration::from_secs(1)).await;
    let ride = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", &attackers_session),
    )
    .await;
    assert_eq!(ride.json()["anonymous"], true);
}

#[tokio::test]
async fn callback_failures_are_generic_400s_that_clear_the_login_cookie_and_create_no_session() {
    let rig = viewer!();
    let begun = rig.begin(None).await;
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    let good_state = begun.param("state").to_owned();
    for (what, query, cookie) in [
        (
            "wrong state",
            "code=c&state=not-the-state".to_owned(),
            Some(begun.cookie.clone()),
        ),
        ("no login cookie", format!("code=c&state={good_state}"), None),
        (
            "tampered cookie",
            format!("code=c&state={good_state}"),
            Some(format!("{}A", begun.cookie)),
        ),
        (
            "provider error",
            "error=access_denied&state=x".to_owned(),
            Some(begun.cookie.clone()),
        ),
        (
            "no code",
            format!("state={good_state}"),
            Some(begun.cookie.clone()),
        ),
        (
            "duplicate state",
            format!("code=c&state={good_state}&state={good_state}"),
            Some(begun.cookie.clone()),
        ),
        (
            "oversize query",
            format!("code={}&state=x", "a".repeat(5000)),
            Some(begun.cookie.clone()),
        ),
    ] {
        let a = rig.callback_raw(&query, cookie.as_deref(), &[]).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{what}: {}", a.body);
        assert_eq!(
            a.headers.get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json",
            "{what}"
        );
        let v = a.json();
        assert_eq!(v["type"], "urn:lanekeeper:problem:sign-in-invalid", "{what}");
        assert!(
            !a.body.contains(&good_state) && !a.body.contains("access_denied"),
            "{what}: {}",
            a.body
        );
        let cookies = a.set_cookies();
        assert!(
            cookie_line(&cookies, "__Host-lk_login")
                .unwrap()
                .contains("Max-Age=0"),
            "{what}: the login cookie is cleared on failure too"
        );
        assert_eq!(cookies.len(), 1, "{what}: no session cookie");
    }
    assert_eq!(session_rows(&rig).await, 0);
    assert!(audit_actions(&rig).await.is_empty());
}

#[tokio::test]
async fn a_token_that_fails_verification_is_403_and_creates_no_session() {
    let rig = viewer!();
    for (what, spec) in [
        (
            "wrong tenant",
            TokenSpec::valid("n").claim("tid", "99999999-9999-4999-8999-999999999999".into()),
        ),
        (
            "wrong audience",
            TokenSpec::valid("n").claim("aud", "someone-else".into()),
        ),
        ("expired", TokenSpec::valid("n").claim("exp", 1.into())),
        ("bad signature", TokenSpec::valid("n").signed_by(1)),
    ] {
        let begun = rig.begin(None).await;
        let spec = {
            let mut s = spec;
            s.claims["nonce"] = begun.param("nonce").into();
            s
        };
        let a = rig.callback(&begun, spec, &[]).await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "{what}: {}", a.body);
        assert_eq!(
            a.json()["type"],
            "urn:lanekeeper:problem:sign-in-refused",
            "{what}"
        );
        assert!(
            !a.body.contains("tid") && !a.body.contains("signature"),
            "{what}: {}",
            a.body
        );
    }
    // A replayed nonce: the token was minted for an earlier login.
    let first = rig.begin(None).await;
    let second = rig.begin(None).await;
    let a = rig
        .callback(&second, TokenSpec::valid(first.param("nonce")), &[])
        .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    assert_eq!(session_rows(&rig).await, 0);
}

#[tokio::test]
async fn an_unreachable_identity_provider_is_502() {
    let rig = viewer!();
    let begun = rig.begin(None).await;
    rig.mock.set_token_status(500);
    let a = rig
        .callback(&begun, TokenSpec::valid(begun.param("nonce")), &[])
        .await;
    assert_eq!(a.status, StatusCode::BAD_GATEWAY);
    assert_eq!(a.json()["type"], "urn:lanekeeper:problem:identity-provider");
    assert_eq!(session_rows(&rig).await, 0);
}

#[tokio::test]
async fn first_sign_in_makes_a_pending_user_with_a_session_and_sends_them_to_the_explaining_page() {
    let rig = RoutesRig::new(db!()).await;
    let a = rig.sign_in(&[]).await;
    assert_eq!(a.status, StatusCode::FOUND, "{}", a.body);
    assert_eq!(a.headers.get(header::LOCATION).unwrap(), "/access-requested");
    let session = cookie_value(&a.set_cookies(), "__Host-lk_session").unwrap();
    let me = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", &session),
    )
    .await;
    assert_eq!(
        me.json()["status"],
        "pending",
        "the guard, not the session, keeps them out (D36)"
    );
    assert_eq!(me.json()["role"], serde_json::Value::Null);
    assert_eq!(rig.users.request_rows().await.len(), 1);
    assert_eq!(
        audit_actions(&rig).await,
        ["access_requested", "session_signed_in"]
    );
}

#[tokio::test]
async fn a_disabled_user_gets_a_session_and_is_sent_to_the_disabled_page() {
    let rig = RoutesRig::new(db!()).await;
    rig.seed_mock_user(Some(Role::Viewer), UserStatus::Disabled).await;
    let a = rig.sign_in(&[]).await;
    assert_eq!(a.headers.get(header::LOCATION).unwrap(), "/access-disabled");
    let session = cookie_value(&a.set_cookies(), "__Host-lk_session").unwrap();
    let me = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", &session),
    )
    .await;
    assert_eq!(me.json()["status"], "disabled");
}

#[tokio::test]
async fn a_failed_audit_write_rolls_back_the_new_session_and_keeps_the_old_one() {
    let rig = RoutesRig::with_audit(db!(), Arc::new(FailingAudit)).await;
    rig.seed_mock_user(Some(Role::Viewer), UserStatus::Active).await;
    let old = rig.replica.create_session(&mock_user()).await;
    let a = rig.sign_in(&[("__Host-lk_session", old.session.expose())]).await;
    assert_eq!(a.status, StatusCode::SERVICE_UNAVAILABLE, "{}", a.body);
    assert_eq!(a.json()["type"], "urn:lanekeeper:problem:unavailable");
    assert!(
        cookie_value(&a.set_cookies(), "__Host-lk_session").is_none(),
        "no session cookie on a failed sign-in"
    );
    assert_eq!(session_rows(&rig).await, 1, "the new session was not kept");
    let still = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", old.session.expose()),
    )
    .await;
    assert_eq!(
        whoami_user(&still),
        Some(mock_user()),
        "and the presented one was not deleted"
    );
    assert_eq!(
        rig.users.outbox_count().await,
        0,
        "nothing was fanned out for the rolled-back transaction"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// logout

/// Sign in and return the two cookie values.
async fn signed_in(rig: &RoutesRig) -> (String, String) {
    let a = rig.sign_in(&[]).await;
    assert_eq!(a.status, StatusCode::FOUND, "{}", a.body);
    let cookies = a.set_cookies();
    (
        cookie_value(&cookies, "__Host-lk_session").unwrap(),
        cookie_value(&cookies, "__Host-lk_csrf").unwrap(),
    )
}

#[tokio::test]
async fn logout_with_session_requires_csrf() {
    let rig = viewer!();
    let (session, csrf) = signed_in(&rig).await;
    // The session cookie alone, as a forged cross-site POST would arrive.
    let forged = send(
        &rig.app,
        Req::post("/auth/logout")
            .cookie("__Host-lk_session", &session)
            .origin("https://evil.example.com"),
    )
    .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    let no_token = send(
        &rig.app,
        Req::post("/auth/logout")
            .cookie("__Host-lk_session", &session)
            .origin(ORIGIN),
    )
    .await;
    assert_eq!(no_token.status, StatusCode::FORBIDDEN);
    assert_eq!(session_rows(&rig).await, 1, "a refused logout ends nothing");
    assert_eq!(audit_actions(&rig).await, ["session_signed_in"]);

    // With the token and the origin it ends the session, clears both cookies, and records it once.
    let ok = send(&rig.app, Req::post("/auth/logout").with_session(&session, &csrf)).await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert!(ok.body.is_empty());
    let cookies = ok.set_cookies();
    assert_eq!(
        cookie_line(&cookies, "__Host-lk_session").unwrap(),
        "__Host-lk_session=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; HttpOnly; SameSite=Lax"
    );
    assert_eq!(
        cookie_line(&cookies, "__Host-lk_csrf").unwrap(),
        "__Host-lk_csrf=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; SameSite=Lax"
    );
    assert_eq!(session_rows(&rig).await, 0);
    assert_eq!(
        audit_actions(&rig).await,
        ["session_signed_in", "session_signed_out"]
    );
    let after = send(
        &rig.app,
        Req::get("/whoami").cookie("__Host-lk_session", &session),
    )
    .await;
    assert_eq!(
        after.json()["anonymous"],
        true,
        "the session is over at once, from this replica's cache too"
    );
}

#[tokio::test]
async fn idempotent_logout() {
    let rig = viewer!();
    // No cookie at all.
    for _ in 0..2 {
        let a = send(&rig.app, Req::post("/auth/logout")).await;
        assert_eq!(a.status, StatusCode::NO_CONTENT);
        assert_eq!(a.set_cookies().len(), 2, "it still clears the cookies");
    }
    // A cookie that names no session.
    let stranger = RandomToken::generate().unwrap().to_text();
    let a = send(
        &rig.app,
        Req::post("/auth/logout").cookie("__Host-lk_session", stranger.expose()),
    )
    .await;
    assert_eq!(a.status, StatusCode::NO_CONTENT);
    // A real logout, then the same cookies again.
    let (session, csrf) = signed_in(&rig).await;
    let first = send(&rig.app, Req::post("/auth/logout").with_session(&session, &csrf)).await;
    assert_eq!(first.status, StatusCode::NO_CONTENT);
    let again = send(&rig.app, Req::post("/auth/logout").with_session(&session, &csrf)).await;
    assert_eq!(
        again.status,
        StatusCode::NO_CONTENT,
        "the second logout finds nothing and still succeeds"
    );
    assert_eq!(
        audit_actions(&rig).await,
        ["session_signed_in", "session_signed_out"],
        "only the logout that ended a session is audited"
    );
}

#[tokio::test]
async fn logout_is_immediate_on_every_replica() {
    let rig = viewer!();
    let other = support::sessions_rig::Replica::start(&rig.users.db, &rig.users.clock).await;
    let (session, csrf) = signed_in(&rig).await;
    // The other replica serves the session from its cache.
    assert!(other.sessions.authenticate(&session).await.unwrap().is_some());
    let ok = send(&rig.app, Req::post("/auth/logout").with_session(&session, &csrf)).await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    other.wait_for_events(1, Duration::from_secs(1)).await;
    assert!(other.sessions.authenticate(&session).await.unwrap().is_none());
}

#[tokio::test]
async fn a_logout_with_a_database_failure_is_503_and_keeps_the_cookies() {
    // The audit write of the logout fails, so the logout does not happen.
    let broken = RoutesRig::with_audit(db!(), Arc::new(FailingAudit)).await;
    broken
        .seed_mock_user(Some(Role::Viewer), UserStatus::Active)
        .await;
    let s = broken.replica.create_session(&mock_user()).await;
    let a = send(
        &broken.app,
        Req::post("/auth/logout").with_session(s.session.expose(), s.csrf.expose()),
    )
    .await;
    assert_eq!(a.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        a.set_cookies().is_empty(),
        "a logout that did not happen does not claim to have"
    );
    assert_eq!(session_rows(&broken).await, 1, "and the session is still there");
    assert_eq!(
        broken.users.outbox_count().await,
        0,
        "with no revocation published for it"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Logs (S10)

#[tokio::test]
async fn no_cookie_token_or_secret_reaches_the_logs_during_sign_in_failure_and_logout() {
    let (capture, _guard) = support::log_capture::install();
    let rig = viewer!();
    // A failure first, then a good sign-in, then a logout.
    let begun: Begun = rig.begin(None).await;
    let bad = rig
        .callback_raw("code=SECRET-CODE-1&state=wrong", Some(&begun.cookie), &[])
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let a = rig.sign_in(&[]).await;
    let cookies = a.set_cookies();
    let session = cookie_value(&cookies, "__Host-lk_session").unwrap();
    let csrf = cookie_value(&cookies, "__Host-lk_csrf").unwrap();
    let forged = send(
        &rig.app,
        Req::post("/auth/logout")
            .cookie("__Host-lk_session", &session)
            .origin("https://evil.example.com"),
    )
    .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    let out = send(&rig.app, Req::post("/auth/logout").with_session(&session, &csrf)).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);

    let logs = capture.text();
    assert!(!logs.is_empty(), "the capture works");
    for (what, secret) in [
        ("session cookie", session.as_str()),
        ("csrf cookie", csrf.as_str()),
        ("login cookie", begun.cookie.as_str()),
        ("authorization code", "SECRET-CODE-1"),
        ("client secret", CLIENT_SECRET),
    ] {
        assert!(!logs.contains(secret), "{what} appears in the logs");
    }
    // Nor the id token the mock handed out.
    assert!(!logs.contains("eyJ"), "a JWT appears in the logs");
}
