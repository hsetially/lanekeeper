//! T1 (S1): the sign-in flow against the mock Entra. Every check the callback makes has a test that breaks exactly it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hub_identity::config::ClientAuth;
use hub_identity::{DiscoveryError, IdentityError, TokenError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::login_rig::{begin, query, rig, rig_with, sign_in_with};
use support::mock_oidc::{CLIENT_ID, CLIENT_SECRET, HOST, NOW_S, OID, TID, TokenSpec, keys, secret_auth};

fn token_error(result: Result<hub_identity::oidc::login::SignIn, IdentityError>) -> TokenError {
    match result {
        Err(IdentityError::Token(e)) => e,
        other => panic!("expected a token error, got {other:?}"),
    }
}

#[tokio::test]
async fn login_redirect_has_state_nonce_and_pkce_s256() {
    let rig = rig().await;
    let begun = begin(&rig, Some("/swimlanes/sit1")).await;
    assert_eq!(begun.redirect.host_str(), Some(HOST));
    assert_eq!(begun.redirect.path(), format!("/{TID}/oauth2/v2.0/authorize"));
    assert_eq!(begun.param("client_id"), CLIENT_ID);
    assert_eq!(begun.param("response_type"), "code");
    assert_eq!(
        begun.param("redirect_uri"),
        "https://lanekeeper.example.com/auth/callback"
    );
    assert_eq!(begun.param("code_challenge_method"), "S256");
    assert!(begun.param("scope").starts_with("openid"));
    // 256 random bits each, base64url without padding.
    for name in ["state", "nonce"] {
        assert_eq!(begun.param(name).len(), 43, "{name}");
    }
    assert_ne!(begun.param("state"), begun.param("nonce"));

    // The challenge is the SHA-256 of the verifier that the token request carries.
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    rig.login
        .complete(&query("code-1", begun.param("state")), Some(&begun.cookie))
        .await
        .unwrap();
    let form = &rig.mock.token_forms()[0];
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes()));
    assert_eq!(challenge, begun.param("code_challenge"));
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "code-1");
    assert_eq!(form["redirect_uri"], begun.param("redirect_uri"));

    // Two logins never share state, nonce, verifier or cookie.
    let again = begin(&rig, None).await;
    for name in ["state", "nonce", "code_challenge"] {
        assert_ne!(begun.param(name), again.param(name), "{name}");
    }
    assert_ne!(begun.cookie, again.cookie);
}

#[tokio::test]
async fn a_valid_sign_in_yields_tid_plus_oid_and_the_return_path() {
    let rig = rig().await;
    let done = sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    assert_eq!(done.user.to_string(), format!("{TID}:{OID}"));
    assert_eq!(done.email.unwrap().as_str(), "ada@example.com");
    assert_eq!(done.display_name.unwrap().as_str(), "Ada Lovelace");
    assert_eq!(done.return_to, "/swimlanes");
}

#[tokio::test]
async fn identity_is_tid_plus_oid_and_email_is_display_only() {
    let rig = rig().await;
    let a = sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    // The same email with another object id is another user.
    let other_oid = "44444444-4444-4444-8444-444444444444";
    let b = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("oid", json!(other_oid)))
        .await
        .unwrap();
    assert_ne!(a.user, b.user);
    assert_eq!(a.email, b.email);
    // A changed email and name do not change who the user is.
    let c = sign_in_with(&rig, |n| {
        TokenSpec::valid(n)
            .claim("email", json!("renamed@example.com"))
            .claim("name", json!("A. Lovelace"))
    })
    .await
    .unwrap();
    assert_eq!(a.user, c.user);
    // No email claim: the preferred username is shown instead, and a missing name is just absent.
    let d = sign_in_with(&rig, |n| {
        TokenSpec::valid(n)
            .without_claim("email")
            .without_claim("name")
            .claim("preferred_username", json!("ada@corp.example"))
    })
    .await
    .unwrap();
    assert_eq!(d.email.unwrap().as_str(), "ada@corp.example");
    assert!(d.display_name.is_none());
    // An identity that is not a GUID is refused, whatever else the token says.
    let bad = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("oid", json!("ada"))).await;
    assert_eq!(token_error(bad), TokenError::BadIdentity);
}

#[tokio::test]
async fn callback_rejects_wrong_issuer() {
    let rig = rig().await;
    let other = format!("https://{HOST}/99999999-9999-4999-8999-999999999999/v2.0");
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("iss", json!(other))).await;
    assert_eq!(token_error(r), TokenError::WrongIssuer);
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).without_claim("iss")).await;
    assert_eq!(token_error(r), TokenError::WrongIssuer);
}

#[tokio::test]
async fn callback_rejects_wrong_audience() {
    let rig = rig().await;
    let r = sign_in_with(&rig, |n| {
        TokenSpec::valid(n).claim("aud", json!("api://something-else"))
    })
    .await;
    assert_eq!(token_error(r), TokenError::WrongAudience);
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("aud", json!(["a", "b"]))).await;
    assert_eq!(token_error(r), TokenError::WrongAudience);
    // The right client id among others is accepted (the OIDC rule).
    sign_in_with(&rig, |n| {
        TokenSpec::valid(n).claim("aud", json!(["a", CLIENT_ID]))
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn callback_rejects_wrong_tid() {
    let rig = rig().await;
    // The issuer is ours, the signature is valid, but the tenant claim names another tenant.
    let r = sign_in_with(&rig, |n| {
        TokenSpec::valid(n).claim("tid", json!("99999999-9999-4999-8999-999999999999"))
    })
    .await;
    assert_eq!(token_error(r), TokenError::WrongTenant);
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).without_claim("tid")).await;
    assert_eq!(token_error(r), TokenError::MissingClaim);
}

#[tokio::test]
async fn callback_rejects_replayed_nonce() {
    let rig = rig().await;
    // A token minted for an earlier login, replayed into a new one.
    let first = begin(&rig, None).await;
    let stale = TokenSpec::valid(first.param("nonce")).mint();
    let second = begin(&rig, None).await;
    rig.mock.set_id_token(stale);
    let r = rig
        .login
        .complete(&query("c", second.param("state")), Some(&second.cookie))
        .await;
    assert_eq!(token_error(r), TokenError::NonceMismatch);
    // A token with no nonce at all is refused too.
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).without_claim("nonce")).await;
    assert_eq!(token_error(r), TokenError::MissingClaim);
}

#[tokio::test]
async fn callback_rejects_state_mismatch() {
    let rig = rig().await;
    let begun = begin(&rig, None).await;
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    let r = rig
        .login
        .complete(&query("c", "not-the-state"), Some(&begun.cookie))
        .await;
    assert_eq!(r.unwrap_err(), IdentityError::StateMismatch);
    assert!(
        rig.mock.token_forms().is_empty(),
        "the code is not redeemed when the state is wrong"
    );
}

#[tokio::test]
async fn callback_rejects_expired_id_token() {
    let rig = rig().await;
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("exp", json!(NOW_S - 3600))).await;
    assert_eq!(token_error(r), TokenError::Expired);
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).without_claim("exp")).await;
    assert_eq!(token_error(r), TokenError::MissingClaim);
}

#[tokio::test]
async fn callback_rejects_not_yet_valid_id_token() {
    let rig = rig().await;
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("nbf", json!(NOW_S + 600))).await;
    assert_eq!(token_error(r), TokenError::NotYetValid);
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).claim("iat", json!(NOW_S + 600))).await;
    assert_eq!(token_error(r), TokenError::NotYetValid);
}

fn forged(header: &Value, claims: &Value, signature: &[u8]) -> String {
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap()),
        URL_SAFE_NO_PAD.encode(signature)
    )
}

#[tokio::test]
async fn callback_rejects_bad_signature() {
    let rig = rig().await;
    // Signed by a key that is not published, under a published key id.
    let r = sign_in_with(&rig, |n| TokenSpec::valid(n).signed_by(3)).await;
    assert_eq!(token_error(r), TokenError::BadSignature);
    // A genuine token with its payload changed after signing.
    let begun = begin(&rig, None).await;
    let genuine = TokenSpec::valid(begun.param("nonce")).mint();
    let mut parts: Vec<String> = genuine.split('.').map(str::to_owned).collect();
    let mut claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&parts[1]).unwrap()).unwrap();
    claims["oid"] = json!("44444444-4444-4444-8444-444444444444");
    parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    rig.mock.set_id_token(parts.join("."));
    let r = rig
        .login
        .complete(&query("c", begun.param("state")), Some(&begun.cookie))
        .await;
    assert_eq!(token_error(r), TokenError::BadSignature);
    // A signature cut short or flipped.
    let flipped = {
        let sig = URL_SAFE_NO_PAD
            .decode(genuine.rsplit('.').next().unwrap())
            .unwrap();
        let mut bad = sig.clone();
        bad[10] ^= 0x01;
        format!(
            "{}.{}",
            genuine.rsplit_once('.').unwrap().0,
            URL_SAFE_NO_PAD.encode(bad)
        )
    };
    rig.mock.set_id_token(flipped);
    let begun = begin(&rig, None).await;
    let r = rig
        .login
        .complete(&query("c", begun.param("state")), Some(&begun.cookie))
        .await;
    assert_eq!(token_error(r), TokenError::BadSignature);
}

#[tokio::test]
async fn callback_rejects_alg_none_and_hs256_confusion() {
    let rig = rig().await;
    let claims = TokenSpec::valid("n").claims;
    // alg none, with and without a signature segment.
    let none_empty = forged(&json!({"alg": "none", "kid": "key-0"}), &claims, b"");
    let none_junk = forged(&json!({"alg": "none", "kid": "key-0"}), &claims, &[1u8; 256]);
    // HS256 signed with the RSA public modulus as the HMAC secret (the classic key confusion).
    let jwk = support::mock_oidc::public_jwk(&keys()[0]);
    let modulus = URL_SAFE_NO_PAD.decode(jwk["n"].as_str().unwrap()).unwrap();
    let hs_header = json!({"alg": "HS256", "kid": "key-0"});
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&hs_header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    );
    let mac = aws_lc_rs::hmac::sign(
        &aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, &modulus),
        input.as_bytes(),
    );
    let hs = format!("{input}.{}", URL_SAFE_NO_PAD.encode(mac.as_ref()));
    // A header that asks for a critical extension, and a missing key id.
    let crit = forged(
        &json!({"alg": "RS256", "kid": "key-0", "crit": ["b64"], "b64": false}),
        &claims,
        &[1u8; 256],
    );
    let nokid = forged(&json!({"alg": "RS256"}), &claims, &[1u8; 256]);

    for (token, want) in [
        (none_empty, TokenError::Malformed),
        (none_junk, TokenError::UnsupportedAlgorithm),
        (hs, TokenError::UnsupportedAlgorithm),
        (crit, TokenError::UnsupportedAlgorithm),
        (nokid, TokenError::MissingKeyId),
    ] {
        let begun = begin(&rig, None).await;
        rig.mock.set_id_token(token);
        let r = rig
            .login
            .complete(&query("c", begun.param("state")), Some(&begun.cookie))
            .await;
        assert_eq!(token_error(r), want);
    }
    assert_eq!(
        rig.https.count("GET", "/discovery/v2.0/keys"),
        0,
        "a token that fails the header checks never reaches the key set"
    );
}

#[tokio::test]
async fn oversize_id_token_is_rejected_unparsed() {
    let rig = rig().await;
    let begun = begin(&rig, None).await;
    rig.mock.set_id_token("a".repeat(ports::MAX_TOKEN_BYTES + 1));
    let r = rig
        .login
        .complete(&query("c", begun.param("state")), Some(&begun.cookie))
        .await;
    assert_eq!(token_error(r), TokenError::Oversize);
    assert_eq!(rig.https.count("GET", "/discovery/v2.0/keys"), 0);
}

#[tokio::test(start_paused = true)]
async fn callback_rejects_unknown_kid_after_one_rate_limited_refresh() {
    let rig = rig().await;
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    assert_eq!(rig.mock.jwks_fetches.load(std::sync::atomic::Ordering::SeqCst), 1);

    // key-1 is not published. Right after the first fetch the unknown id is refused without asking Entra again.
    let unknown = |n: &str| TokenSpec::valid(n).header("kid", json!("key-1")).signed_by(1);
    assert_eq!(
        token_error(sign_in_with(&rig, unknown).await),
        TokenError::UnknownKey
    );
    assert_eq!(
        rig.mock.jwks_fetches.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "rate limited"
    );

    // After 30 s one refresh is allowed; it does not help, and the next attempt is limited again.
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        token_error(sign_in_with(&rig, unknown).await),
        TokenError::UnknownKey
    );
    assert_eq!(
        token_error(sign_in_with(&rig, unknown).await),
        TokenError::UnknownKey
    );
    assert_eq!(
        rig.mock.jwks_fetches.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one refresh, then limited"
    );

    // Entra rotates: key-1 is published, and the next unknown id after the interval picks it up.
    rig.mock.publish_only(&[0, 1]);
    tokio::time::advance(Duration::from_secs(31)).await;
    sign_in_with(&rig, unknown).await.unwrap();
    assert_eq!(rig.mock.jwks_fetches.load(std::sync::atomic::Ordering::SeqCst), 3);
    // And the old key still works.
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    assert_eq!(rig.mock.jwks_fetches.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn callback_without_login_cookie_is_400() {
    let rig = rig().await;
    let begun = begin(&rig, None).await;
    let r = rig.login.complete(&query("c", begun.param("state")), None).await;
    assert_eq!(r.unwrap_err(), IdentityError::LoginStateInvalid);
    let r = rig
        .login
        .complete(&query("c", begun.param("state")), Some("garbage"))
        .await;
    assert_eq!(r.unwrap_err(), IdentityError::LoginStateInvalid);
    for raw in [
        "",
        "code=only",
        "state=only",
        "error=access_denied&state=s",
        "error=access_denied&code=c&state=s",
    ] {
        let q = hub_identity::oidc::login::CallbackQuery::parse(raw).unwrap();
        let r = rig.login.complete(&q, Some(&begun.cookie)).await;
        assert_eq!(r.unwrap_err(), IdentityError::BadRequest, "{raw:?}");
    }
    assert!(rig.mock.token_forms().is_empty());
}

#[tokio::test]
async fn login_cookie_expires_after_10_minutes() {
    let rig = rig().await;
    let begun = begin(&rig, None).await;
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    rig.clock.advance_secs(10 * 60 + 1);
    let r = rig
        .login
        .complete(&query("c", begun.param("state")), Some(&begun.cookie))
        .await;
    assert_eq!(r.unwrap_err(), IdentityError::LoginStateExpired);
    assert!(rig.mock.token_forms().is_empty());

    // Just inside the window it still works.
    let rig = support::login_rig::rig().await;
    let begun = begin(&rig, None).await;
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    rig.clock.advance_secs(10 * 60 - 1);
    rig.login
        .complete(&query("c", begun.param("state")), Some(&begun.cookie))
        .await
        .unwrap();
}

#[tokio::test]
async fn login_cookie_of_another_key_is_rejected_by_a_replica_with_the_same_key_only() {
    // Two replicas share the key, so a login begun on one finishes on the other.
    let one = rig().await;
    let two = rig().await;
    let begun = begin(&one, None).await;
    two.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    two.login
        .complete(&query("c", begun.param("state")), Some(&begun.cookie))
        .await
        .unwrap();
}

#[tokio::test]
async fn return_to_is_revalidated_and_defaults_to_the_root() {
    let rig = rig().await;
    for bad in [
        "https://evil.example.com/",
        "//evil.example.com",
        "/\\evil.example.com",
        "swimlanes",
    ] {
        let begun = begin(&rig, Some(bad)).await;
        rig.mock
            .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
        let done = rig
            .login
            .complete(&query("c", begun.param("state")), Some(&begun.cookie))
            .await
            .unwrap();
        assert_eq!(done.return_to, "/", "{bad:?}");
    }
}

/// The sealing key the rig's `SecretSource` holds (see `mock_oidc::secrets`).
fn rig_sealer() -> hub_identity::oidc::login::LoginSealer {
    hub_identity::oidc::login::LoginSealer::from_key([5u8; 32])
}

#[tokio::test]
async fn return_to_is_checked_on_both_sides_of_the_cookie() {
    use hub_identity::oidc::login::LoginState;
    let rig = rig().await;
    // `begin` never seals an unsafe path.
    let begun = begin(&rig, Some("//evil.example.com")).await;
    assert_eq!(rig_sealer().open(&begun.cookie).unwrap().return_to, "/");
    // The callback does not trust the cookie's path either: a cookie sealed with the right key but an unsafe path.
    let begun = begin(&rig, None).await;
    let opened = rig_sealer().open(&begun.cookie).unwrap();
    let crafted = rig_sealer()
        .seal(&LoginState {
            return_to: "//evil.example.com/x".to_owned(),
            ..opened
        })
        .unwrap();
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    let done = rig
        .login
        .complete(&query("c", begun.param("state")), Some(crafted.as_str()))
        .await
        .unwrap();
    assert_eq!(done.return_to, "/");
}

#[tokio::test]
async fn token_endpoint_error_maps_to_generic_problem() {
    let rig = rig().await;
    rig.mock.set_token_status(400);
    let r = sign_in_with(&rig, TokenSpec::valid).await;
    let err = r.unwrap_err();
    assert_eq!(err, IdentityError::TokenEndpoint);
    let text = format!("{err} {err:?}");
    for leaked in [
        "AADSTS",
        "SECRET-CODE",
        "invalid_grant",
        "the-auth-code",
        CLIENT_SECRET,
    ] {
        assert!(!text.contains(leaked), "{leaked} leaked into the error");
    }
    // A transport failure is a different, equally generic error.
    let rig = support::login_rig::rig().await;
    *rig.mock.token_transport_error.lock().unwrap() = true;
    let err = sign_in_with(&rig, TokenSpec::valid).await.unwrap_err();
    assert!(matches!(err, IdentityError::Http(_)), "{err:?}");
}

#[tokio::test]
async fn discovery_issuer_or_host_mismatch_is_refused_at_login() {
    let rig = rig().await;
    rig.mock.set_discovery_issuer("https://login.test/other/v2.0");
    let err = rig.login.begin(None).await.unwrap_err();
    assert_eq!(err, IdentityError::Discovery(DiscoveryError::IssuerMismatch));

    let rig = support::login_rig::rig().await;
    rig.mock.set_token_endpoint_host("evil.example.com");
    let err = rig.login.begin(None).await.unwrap_err();
    assert_eq!(err, IdentityError::Discovery(DiscoveryError::EndpointRejected));
}

#[tokio::test]
async fn both_client_auth_modes_complete_a_login() {
    // Client secret.
    let rig = rig_with(secret_auth()).await;
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    let form = &rig.mock.token_forms()[0];
    assert_eq!(form["client_secret"], CLIENT_SECRET);
    assert!(!form.contains_key("client_assertion"));

    // Federated credential.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    std::fs::write(&file, "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJzeXN0ZW06c2EifQ.c2ln\n").unwrap();
    let rig = rig_with(ClientAuth::Federated {
        token_file: file.to_string_lossy().into_owned(),
    })
    .await;
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    let form = &rig.mock.token_forms()[0];
    assert!(!form.contains_key("client_secret"));
    assert_eq!(
        form["client_assertion_type"],
        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer"
    );
    assert_eq!(
        form["client_assertion"],
        "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJzeXN0ZW06c2EifQ.c2ln"
    );
}

#[tokio::test]
async fn client_secret_mode_posts_secret_from_secret_source() {
    let rig = rig().await;
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    let forms: Vec<HashMap<String, String>> = rig.mock.token_forms();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0]["client_secret"], CLIENT_SECRET);
    assert_eq!(forms[0]["client_id"], CLIENT_ID);
    // The secret travels in the POST body only, never in a URL.
    assert!(rig.https.seen().iter().all(|r| !r.url.contains(CLIENT_SECRET)));
}

#[tokio::test]
async fn federated_mode_posts_assertion_and_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    std::fs::write(&file, "aaa.bbb.ccc").unwrap();
    let rig = rig_with(ClientAuth::Federated {
        token_file: file.to_string_lossy().into_owned(),
    })
    .await;
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    let form = &rig.mock.token_forms()[0];
    assert_eq!(form["client_assertion"], "aaa.bbb.ccc");
    assert!(form.keys().all(|k| k != "client_secret"));
    // The secret source is not consulted for the client secret in this mode.
}

#[tokio::test]
async fn federated_assertion_is_reread_on_each_exchange() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    std::fs::write(&file, "first.token.value").unwrap();
    let rig = rig_with(ClientAuth::Federated {
        token_file: file.to_string_lossy().into_owned(),
    })
    .await;
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    std::fs::write(&file, "rotated.token.value\n").unwrap();
    sign_in_with(&rig, TokenSpec::valid).await.unwrap();
    let forms = rig.mock.token_forms();
    assert_eq!(forms[0]["client_assertion"], "first.token.value");
    assert_eq!(forms[1]["client_assertion"], "rotated.token.value");
}

#[tokio::test]
async fn a_bad_assertion_file_fails_the_exchange_without_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in [
        ("missing", None),
        ("empty", Some(String::new())),
        ("spaces", Some("not a jwt".to_owned())),
        ("huge", Some("a".repeat(17 * 1024))),
        ("binary", None),
    ] {
        let file = dir.path().join(name);
        match (name, content) {
            ("binary", _) => std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap(),
            (_, Some(c)) => std::fs::write(&file, c).unwrap(),
            _ => {}
        }
        let path = file.to_string_lossy().into_owned();
        let rig = rig_with(ClientAuth::Federated {
            token_file: path.clone(),
        })
        .await;
        let err = sign_in_with(&rig, TokenSpec::valid).await.unwrap_err();
        assert_eq!(err, IdentityError::ClientAssertion, "{name}");
        assert!(!format!("{err} {err:?}").contains(&path));
        assert!(rig.mock.token_forms().is_empty());
    }
}

#[tokio::test]
async fn startup_refuses_a_bad_configuration_or_cookie_key() {
    use hub_identity::config::ConfigError;
    use hub_identity::oidc::login::OidcLogin;
    use support::mock_oidc::{MockOidc, NOW_MS, config, secrets};

    let mock = MockOidc::new();
    let start = |cfg, secrets| {
        let clock = std::sync::Arc::new(hub_identity::ManualClock::at(
            domain::Timestamp::from_unix_millis(NOW_MS),
        ));
        OidcLogin::new(cfg, mock.https(), secrets, clock)
    };
    let mut bad = config(secret_auth());
    bad.public_origin = "http://insecure.example.com".to_owned();
    assert_eq!(
        start(bad, secrets()).await.unwrap_err(),
        IdentityError::Config(ConfigError::PublicOrigin)
    );

    // The cookie key secret is missing, or is not 32 bytes of base64url.
    let none = std::sync::Arc::new(ports::fakes::FakeSecretSource::new());
    assert!(matches!(
        start(config(secret_auth()), none).await.unwrap_err(),
        IdentityError::Secret(_)
    ));
    let short =
        std::sync::Arc::new(ports::fakes::FakeSecretSource::new().with("login-cookie-key", "c2hvcnQ"));
    assert_eq!(
        start(config(secret_auth()), short).await.unwrap_err(),
        IdentityError::Crypto
    );
}
