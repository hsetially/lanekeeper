//! T1 (S10): nothing sensitive reaches the logs while a user signs in, fails to sign in, or the identity provider misbehaves.
//! Later tasks add their flows (sessions, admin operations, the audit chain) to the same assertion.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use serde_json::json;
use support::log_capture;
use support::login_rig::{begin, query, rig, sign_in_with};
use support::mock_oidc::{CLIENT_SECRET, TokenSpec};

#[tokio::test]
async fn log_capture_has_no_token_cookie_secret_or_diff() {
    let (capture, _guard) = log_capture::install();
    let rig = rig().await;

    // A successful sign-in, keeping the pieces that must not be logged.
    let begun = begin(&rig, Some("/swimlanes")).await;
    let id_token = TokenSpec::valid(begun.param("nonce")).mint();
    rig.mock.set_id_token(id_token.clone());
    rig.login
        .complete(
            &query("AUTH-CODE-12345", begun.param("state")),
            Some(&begun.cookie),
        )
        .await
        .unwrap();
    let verifier = rig.mock.token_forms()[0]["code_verifier"].clone();

    // Failures: a bad token, a refused code, an unreachable provider, a bad key set.
    let _ = sign_in_with(&rig, |n| {
        TokenSpec::valid(n).claim("tid", json!("99999999-9999-4999-8999-999999999999"))
    })
    .await;
    rig.mock.set_token_status(400);
    let _ = sign_in_with(&rig, TokenSpec::valid).await;
    rig.mock.set_token_status(200);
    *rig.mock.token_transport_error.lock().unwrap() = true;
    let _ = sign_in_with(&rig, TokenSpec::valid).await;
    *rig.mock.token_transport_error.lock().unwrap() = false;
    rig.mock.set_jwks_status(500);

    let log = capture.text();
    assert!(
        log.contains("token endpoint refused"),
        "the refusal is logged by status, so the capture works: {log:?}"
    );
    for secret in [
        id_token.as_str(),
        "AUTH-CODE-12345",
        CLIENT_SECRET,
        begun.cookie.as_str(),
        verifier.as_str(),
        "ACCESS-TOKEN-DO-NOT-KEEP",
        begun.param("nonce"),
        begun.param("state"),
        "AADSTS",
    ] {
        assert!(!log.contains(secret), "{secret} reached the log");
    }
}
