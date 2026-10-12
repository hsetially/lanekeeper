//! T4 (S1): bearer-token verification for MCP (Entra access tokens) and for agent join (Google ID tokens), against the mock
//! providers. Every bad shape a forger or a mistake can produce has its own test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use domain::{Role, ShortText, UserId, UserStatus};
use hub_identity::Clock;
use hub_identity::config::EntraConfig;
use hub_identity::http::HttpsClient;
use hub_identity::tokens::{
    EntraAccessTokens, GOOGLE_CERTS_URL, GOOGLE_ISSUER, GoogleIdTokens, HubTokenVerifier, MCP_SCOPE,
    McpAuthenticator,
};
use ports::conformance::TokenFixtures;
use ports::{AuthError, GoogleIdentity, TokenVerifier, VerifiedUser};
use serde_json::json;
use support::fake_https::{FakeHttps, json_response};
use support::login_rig::{Rig, rig};
use support::mock_oidc::{CLIENT_ID, NOW_S, OID, TID, TokenSpec, config, keys, public_jwk};

const API_AUD: &str = "api://lanekeeper";
const AGENT_AUD: &str = "https://lanekeeper.example.com/agent";
const AGENT_EMAIL: &str = "sit1-agent@project.iam.gserviceaccount.com";

fn entra_cfg() -> EntraConfig {
    config(support::mock_oidc::secret_auth())
}

/// A valid Entra access token for the hub API with the MCP scope.
fn access_token() -> TokenSpec {
    TokenSpec::valid("unused")
        .without_claim("nonce")
        .claim("aud", json!(API_AUD))
        .claim("scp", json!("mcp.access"))
}

/// A valid Google ID token for an agent.
fn google_token() -> TokenSpec {
    TokenSpec {
        header: json!({"alg": "RS256", "typ": "JWT", "kid": "key-0"}),
        claims: json!({
            "iss": GOOGLE_ISSUER,
            "aud": AGENT_AUD,
            "sub": "108293847561029384756",
            "email": AGENT_EMAIL,
            "email_verified": true,
            "iat": NOW_S, "exp": NOW_S + 3600,
        }),
        signer: 0,
    }
}

/// Google's key set, served for any URL (the host allow-list is the real client's job).
fn google_https() -> Arc<FakeHttps> {
    Arc::new(FakeHttps::new(|req| {
        assert_eq!(
            req.url().as_str(),
            GOOGLE_CERTS_URL,
            "the key set comes from the fixed Google URL"
        );
        let body = json!({"keys": [public_jwk(&keys()[0])]}).to_string();
        Ok(json_response(200, &body, Some("public, max-age=3600")))
    }))
}

struct Verifiers {
    rig: Rig,
    verifier: HubTokenVerifier,
}

async fn verifiers() -> Verifiers {
    let rig = rig().await;
    let clock: Arc<dyn Clock> = Arc::clone(&rig.clock) as Arc<dyn Clock>;
    let verifier = HubTokenVerifier::new(
        EntraAccessTokens::new(&entra_cfg(), rig.login.verifier()),
        GoogleIdTokens::new(google_https() as Arc<dyn HttpsClient>, clock),
    );
    Verifiers { rig, verifier }
}

fn expected_user() -> VerifiedUser {
    VerifiedUser {
        id: UserId::new(TID.parse().unwrap(), OID.parse().unwrap()),
        email: Some(ShortText::parse("ada@example.com").unwrap()),
        display_name: Some(ShortText::parse("Ada Lovelace").unwrap()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Entra access tokens (MCP)

#[tokio::test]
async fn entra_access_token_valid() {
    let v = verifiers().await;
    let got = v
        .verifier
        .entra_access_token(&access_token().mint())
        .await
        .unwrap();
    assert_eq!(got, expected_user());
    assert_eq!(MCP_SCOPE, "mcp.access");
    // Other scopes next to the one required are fine; the audience may be any configured one.
    let t = access_token()
        .claim("scp", json!("openid mcp.access profile"))
        .mint();
    assert!(v.verifier.entra_access_token(&t).await.is_ok());
}

#[tokio::test]
async fn entra_access_token_rejects_forged_signature() {
    let v = verifiers().await;
    // Right key id, a key nobody published: the signature check fails.
    let t = access_token().signed_by(3).mint();
    assert_eq!(
        v.verifier.entra_access_token(&t).await.unwrap_err(),
        AuthError::InvalidToken
    );
    // A real token with its payload swapped for another user's: the signature no longer covers it.
    let good = access_token().mint();
    let other = access_token()
        .claim("oid", json!("44444444-4444-4444-8444-444444444444"))
        .mint();
    let forged = format!(
        "{}.{}.{}",
        good.split('.').next().unwrap(),
        other.split('.').nth(1).unwrap(),
        good.split('.').nth(2).unwrap()
    );
    assert_eq!(
        v.verifier.entra_access_token(&forged).await.unwrap_err(),
        AuthError::InvalidToken
    );
}

#[tokio::test]
async fn entra_access_token_rejects_expired() {
    let v = verifiers().await;
    let t = access_token().claim("exp", json!(NOW_S - 3600)).mint();
    assert_eq!(
        v.verifier.entra_access_token(&t).await.unwrap_err(),
        AuthError::Expired
    );
    // Within the 60 s of skew it still counts as valid; past it, it does not.
    let edge = access_token().claim("exp", json!(NOW_S - 30)).mint();
    assert!(v.verifier.entra_access_token(&edge).await.is_ok());
    let late = access_token().claim("exp", json!(NOW_S - 61)).mint();
    assert_eq!(
        v.verifier.entra_access_token(&late).await.unwrap_err(),
        AuthError::Expired
    );
}

#[tokio::test]
async fn entra_access_token_rejects_not_yet_valid_and_missing_nbf() {
    let v = verifiers().await;
    let future = access_token().claim("nbf", json!(NOW_S + 3600)).mint();
    assert_eq!(
        v.verifier.entra_access_token(&future).await.unwrap_err(),
        AuthError::InvalidToken
    );
    let none = access_token().without_claim("nbf").mint();
    assert_eq!(
        v.verifier.entra_access_token(&none).await.unwrap_err(),
        AuthError::InvalidToken,
        "nbf is required"
    );
}

#[tokio::test]
async fn entra_access_token_rejects_wrong_audience() {
    let v = verifiers().await;
    let t = access_token().claim("aud", json!("api://some-other-api")).mint();
    assert_eq!(
        v.verifier.entra_access_token(&t).await.unwrap_err(),
        AuthError::WrongAudience
    );
    let none = access_token().without_claim("aud").mint();
    assert_eq!(
        v.verifier.entra_access_token(&none).await.unwrap_err(),
        AuthError::WrongAudience
    );
}

#[tokio::test]
async fn entra_access_token_rejects_wrong_tid() {
    let v = verifiers().await;
    let t = access_token()
        .claim("tid", json!("99999999-9999-4999-8999-999999999999"))
        .mint();
    assert_eq!(
        v.verifier.entra_access_token(&t).await.unwrap_err(),
        AuthError::InvalidToken
    );
    let none = access_token().without_claim("tid").mint();
    assert_eq!(
        v.verifier.entra_access_token(&none).await.unwrap_err(),
        AuthError::InvalidToken
    );
}

#[tokio::test]
async fn entra_access_token_rejects_missing_mcp_scope() {
    let v = verifiers().await;
    for (what, spec) in [
        ("no scp", access_token().without_claim("scp")),
        (
            "another scope",
            access_token().claim("scp", json!("user_impersonation")),
        ),
        (
            "a longer scope",
            access_token().claim("scp", json!("mcp.access.admin")),
        ),
        (
            "a different case",
            access_token().claim("scp", json!("MCP.ACCESS")),
        ),
        ("an empty scope", access_token().claim("scp", json!(""))),
        (
            "an app role instead",
            access_token()
                .without_claim("scp")
                .claim("roles", json!(["mcp.access"])),
        ),
        (
            "scp as an array",
            access_token().claim("scp", json!(["mcp.access"])),
        ),
    ] {
        assert_eq!(
            v.verifier.entra_access_token(&spec.mint()).await.unwrap_err(),
            AuthError::InvalidToken,
            "{what}"
        );
    }
}

#[tokio::test]
async fn an_id_token_is_not_an_access_token() {
    // The ID token of the web sign-in has the hub's client id as audience, which is also accepted for the API. It carries no
    // `scp`, so it cannot be replayed as an MCP credential.
    let v = verifiers().await;
    let mut cfg = entra_cfg();
    cfg.accepted_api_audiences.push(CLIENT_ID.to_owned());
    let tokens = EntraAccessTokens::new(&cfg, v.rig.login.verifier());
    let id_token = TokenSpec::valid("some-nonce").mint();
    assert_eq!(
        tokens.verify(&id_token).await.unwrap_err(),
        AuthError::InvalidToken
    );
}

#[tokio::test]
async fn entra_access_token_rejects_wrong_issuer() {
    let v = verifiers().await;
    for iss in [
        "https://login.test/99999999-9999-4999-8999-999999999999/v2.0",
        "https://sts.windows.net/11111111-1111-4111-8111-111111111111/",
        "https://accounts.google.com",
    ] {
        let t = access_token().claim("iss", json!(iss)).mint();
        assert_eq!(
            v.verifier.entra_access_token(&t).await.unwrap_err(),
            AuthError::InvalidToken,
            "{iss}"
        );
    }
}

#[tokio::test]
async fn entra_access_token_rejects_oversize_token() {
    let v = verifiers().await;
    let big = access_token().claim("pad", json!("a".repeat(20_000))).mint();
    assert!(big.len() > ports::MAX_TOKEN_BYTES);
    assert_eq!(
        v.verifier.entra_access_token(&big).await.unwrap_err(),
        AuthError::InvalidToken
    );
    // Refused without a key lookup: the key set was never fetched.
    assert_eq!(
        v.rig.mock.jwks_fetches.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn entra_access_token_rejects_bad_algorithms_and_missing_identity() {
    let v = verifiers().await;
    for (what, spec) in [
        ("alg none", access_token().header("alg", json!("none"))),
        ("alg HS256", access_token().header("alg", json!("HS256"))),
        ("no oid", access_token().without_claim("oid")),
        (
            "oid not a guid",
            access_token().claim("oid", json!("ada@example.com")),
        ),
        (
            "tid is the oid's shape but wrong type",
            access_token().claim("tid", json!(7)),
        ),
        ("unknown key id", access_token().header("kid", json!("key-9"))),
    ] {
        assert_eq!(
            v.verifier.entra_access_token(&spec.mint()).await.unwrap_err(),
            AuthError::InvalidToken,
            "{what}"
        );
    }
}

#[tokio::test]
async fn entra_access_token_without_email_or_name_still_identifies_the_user() {
    let v = verifiers().await;
    let t = access_token().without_claim("email").without_claim("name").mint();
    let got = v.verifier.entra_access_token(&t).await.unwrap();
    assert_eq!(got.id, expected_user().id);
    assert_eq!((got.email, got.display_name), (None, None));
    let upn = access_token()
        .without_claim("email")
        .claim("preferred_username", json!("ada@corp.example.com"))
        .mint();
    assert_eq!(
        v.verifier
            .entra_access_token(&upn)
            .await
            .unwrap()
            .email
            .unwrap()
            .as_str(),
        "ada@corp.example.com"
    );
}

#[tokio::test]
async fn entra_is_unavailable_when_the_key_set_cannot_be_fetched() {
    let v = verifiers().await;
    v.rig.mock.set_jwks_status(500);
    let err = v
        .verifier
        .entra_access_token(&access_token().mint())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        AuthError::Unavailable,
        "an outage is not the caller's fault and is not an InvalidToken"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Google ID tokens (agent join)

#[tokio::test]
async fn google_id_token_valid_returns_email() {
    let v = verifiers().await;
    let got = v
        .verifier
        .google_id_token(&google_token().mint(), AGENT_AUD)
        .await
        .unwrap();
    assert_eq!(
        got,
        GoogleIdentity {
            subject: ShortText::parse("108293847561029384756").unwrap(),
            email: ShortText::parse(AGENT_EMAIL).unwrap(),
            audience: ShortText::parse(AGENT_AUD).unwrap(),
        }
    );
}

#[tokio::test]
async fn google_id_token_rejects_forged() {
    let v = verifiers().await;
    let t = google_token().signed_by(3).mint();
    assert_eq!(
        v.verifier.google_id_token(&t, AGENT_AUD).await.unwrap_err(),
        AuthError::InvalidToken
    );
}

#[tokio::test]
async fn google_id_token_rejects_expired() {
    let v = verifiers().await;
    let t = google_token().claim("exp", json!(NOW_S - 3600)).mint();
    assert_eq!(
        v.verifier.google_id_token(&t, AGENT_AUD).await.unwrap_err(),
        AuthError::Expired
    );
}

#[tokio::test]
async fn google_id_token_rejects_wrong_audience() {
    let v = verifiers().await;
    let t = google_token().mint();
    assert_eq!(
        v.verifier
            .google_id_token(&t, "https://another.example.com")
            .await
            .unwrap_err(),
        AuthError::WrongAudience
    );
    // An empty audience asked for never matches anything, not even a token with an empty `aud`.
    let empty = google_token().claim("aud", json!("")).mint();
    assert_eq!(
        v.verifier.google_id_token(&empty, "").await.unwrap_err(),
        AuthError::InvalidToken
    );
}

#[tokio::test]
async fn google_id_token_rejects_wrong_issuer() {
    let v = verifiers().await;
    for iss in [
        "https://evil.example.com",
        "accounts.google.com",
        "https://accounts.google.com/",
        &format!("https://login.test/{TID}/v2.0"),
    ] {
        let t = google_token().claim("iss", json!(iss)).mint();
        assert_eq!(
            v.verifier.google_id_token(&t, AGENT_AUD).await.unwrap_err(),
            AuthError::InvalidToken,
            "{iss}"
        );
    }
}

#[tokio::test]
async fn google_id_token_rejects_unverified_email() {
    let v = verifiers().await;
    for (what, spec) in [
        ("false", google_token().claim("email_verified", json!(false))),
        ("absent", google_token().without_claim("email_verified")),
        (
            "a string that is not true",
            google_token().claim("email_verified", json!("yes")),
        ),
        ("no email", google_token().without_claim("email")),
        ("an empty email", google_token().claim("email", json!(""))),
        ("no subject", google_token().without_claim("sub")),
    ] {
        assert_eq!(
            v.verifier
                .google_id_token(&spec.mint(), AGENT_AUD)
                .await
                .unwrap_err(),
            AuthError::InvalidToken,
            "{what}"
        );
    }
}

#[tokio::test]
async fn google_key_set_is_cached_between_tokens() {
    let rig = rig().await;
    let https = google_https();
    let clock: Arc<dyn Clock> = Arc::clone(&rig.clock) as Arc<dyn Clock>;
    let tokens = GoogleIdTokens::new(Arc::clone(&https) as Arc<dyn HttpsClient>, clock);
    for _ in 0..5 {
        tokens.verify(&google_token().mint(), AGENT_AUD).await.unwrap();
    }
    assert_eq!(https.count("GET", "oauth2/v3/certs"), 1);
}

// ---------------------------------------------------------------------------------------------------------------
// Both, and the port contract

#[tokio::test]
async fn an_entra_token_is_not_a_google_token_and_the_other_way_round() {
    let v = verifiers().await;
    assert_eq!(
        v.verifier
            .google_id_token(&access_token().mint(), AGENT_AUD)
            .await
            .unwrap_err(),
        AuthError::InvalidToken
    );
    assert_eq!(
        v.verifier
            .entra_access_token(&google_token().mint())
            .await
            .unwrap_err(),
        AuthError::InvalidToken
    );
}

#[tokio::test]
async fn tokens_never_appear_in_error_text() {
    let v = verifiers().await;
    let attempts = [
        access_token().signed_by(3).mint(),
        access_token().claim("exp", json!(NOW_S - 3600)).mint(),
        access_token().claim("aud", json!("api://other")).mint(),
        access_token().without_claim("scp").mint(),
        "not.a.token".to_owned(),
        "a".repeat(20_000),
    ];
    for t in &attempts {
        let e = v.verifier.entra_access_token(t).await.unwrap_err();
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains(t.as_str()), "the error repeats the token");
        for part in t.split('.').filter(|p| p.len() > 8) {
            assert!(!shown.contains(part), "the error repeats a part of the token");
        }
    }
    let g = google_token().signed_by(3).mint();
    let e = v.verifier.google_id_token(&g, AGENT_AUD).await.unwrap_err();
    assert!(!format!("{e} {e:?}").contains(g.as_str()));
}

#[tokio::test]
async fn hub_token_verifier_passes_port_conformance() {
    let v = verifiers().await;
    let fixtures = TokenFixtures {
        entra_ok: (access_token().mint(), expected_user()),
        entra_expired: access_token().claim("exp", json!(NOW_S - 3600)).mint(),
        google_ok: (
            google_token().mint(),
            AGENT_AUD.to_owned(),
            GoogleIdentity {
                subject: ShortText::parse("108293847561029384756").unwrap(),
                email: ShortText::parse(AGENT_EMAIL).unwrap(),
                audience: ShortText::parse(AGENT_AUD).unwrap(),
            },
        ),
    };
    ports::conformance::token_verifier(&v.verifier, &fixtures).await;
}

// ---------------------------------------------------------------------------------------------------------------
// MCP: the token and the user together (S1: pending and disabled users get 403 everywhere, MCP included)

/// The authenticator over the real user directory, and the rig whose database it reads. The mock identity is the only user.
async fn mcp_over(routes: &support::web_rig::RoutesRig) -> McpAuthenticator {
    let rig = rig().await;
    let clock: Arc<dyn Clock> = Arc::clone(&rig.clock) as Arc<dyn Clock>;
    let tokens = HubTokenVerifier::new(
        EntraAccessTokens::new(&entra_cfg(), rig.login.verifier()),
        GoogleIdTokens::new(google_https() as Arc<dyn HttpsClient>, clock),
    );
    McpAuthenticator::new(Arc::new(tokens), Arc::new(routes.users.users.clone()))
}

/// Set the standing of the mock identity directly (a fixture, not a code path under test).
async fn stand_as(routes: &support::web_rig::RoutesRig, role: Option<Role>, status: UserStatus) {
    sqlx::query("UPDATE users SET role = $1, status = $2 WHERE oid = $3")
        .bind(role.map(Role::as_str))
        .bind(status.as_str())
        .bind(OID)
        .execute(&routes.replica.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn mcp_token_of_pending_or_disabled_user_is_refused() {
    let token = access_token().mint();
    let routes = support::web_rig::RoutesRig::new(db!()).await;
    routes.seed_mock_user(None, UserStatus::Pending).await;
    let mcp = mcp_over(&routes).await;
    for (role, status, want) in [
        (None, UserStatus::Pending, AuthError::Pending),
        (Some(Role::Admin), UserStatus::Pending, AuthError::Pending),
        (Some(Role::Admin), UserStatus::Disabled, AuthError::Disabled),
        (Some(Role::Viewer), UserStatus::Disabled, AuthError::Disabled),
    ] {
        stand_as(&routes, role, status).await;
        for min in Role::ALL {
            assert_eq!(
                mcp.authenticate(&token, min).await.unwrap_err(),
                want,
                "{role:?} {status:?} {min}"
            );
        }
    }
}

#[tokio::test]
async fn mcp_token_of_an_active_user_needs_the_role() {
    let token = access_token().mint();
    let routes = support::web_rig::RoutesRig::new(db!()).await;
    routes
        .seed_mock_user(Some(Role::Editor), UserStatus::Active)
        .await;
    let mcp = mcp_over(&routes).await;
    let user = mcp.authenticate(&token, Role::Editor).await.unwrap();
    assert_eq!(user.id, expected_user().id);
    assert_eq!(
        mcp.authenticate(&token, Role::Viewer).await.unwrap().id,
        expected_user().id
    );
    assert_eq!(
        mcp.authenticate(&token, Role::Operator).await.unwrap_err(),
        AuthError::InsufficientRole
    );
}

#[tokio::test]
async fn mcp_token_of_an_unknown_user_and_a_bad_token() {
    let routes = support::web_rig::RoutesRig::new(db!()).await;
    let mcp = mcp_over(&routes).await;
    // A valid token for a user who never signed in: unknown, which is not "pending". MCP creates nobody.
    assert_eq!(
        mcp.authenticate(&access_token().mint(), Role::Viewer)
            .await
            .unwrap_err(),
        AuthError::UnknownUser
    );
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&routes.replica.pool)
        .await
        .unwrap();
    assert_eq!(users, 0, "a token never creates a user");
    // A forged token is refused before the directory is asked, even if the user exists.
    routes.seed_mock_user(Some(Role::Admin), UserStatus::Active).await;
    assert_eq!(
        mcp.authenticate(&access_token().signed_by(3).mint(), Role::Viewer)
            .await
            .unwrap_err(),
        AuthError::InvalidToken
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Logs (S10)

#[tokio::test]
async fn token_failures_log_no_token_text() {
    let (capture, _guard) = support::log_capture::install();
    let v = verifiers().await;
    let attempts = [
        access_token().signed_by(3).mint(),
        access_token().claim("exp", json!(NOW_S - 3600)).mint(),
        access_token()
            .claim("aud", json!("api://ATTACKER-AUDIENCE"))
            .mint(),
        access_token()
            .without_claim("scp")
            .claim("name", json!("ATTACKER-NAME"))
            .mint(),
        access_token()
            .claim("iss", json!("https://ATTACKER-ISSUER.example.com"))
            .mint(),
    ];
    for t in &attempts {
        let _ = v.verifier.entra_access_token(t).await;
    }
    let g = google_token()
        .claim("email", json!("ATTACKER-EMAIL@example.com"))
        .signed_by(3)
        .mint();
    let _ = v.verifier.google_id_token(&g, AGENT_AUD).await;
    v.rig.mock.set_jwks_status(500);
    let _ = v
        .verifier
        .entra_access_token(&access_token().header("kid", json!("key-2")).mint())
        .await;

    let log = capture.text();
    assert!(!log.is_empty(), "the refusals are logged, so the capture works");
    for needle in attempts.iter().map(String::as_str).chain([g.as_str()]) {
        assert!(!log.contains(needle), "a token reached the log");
        for part in needle.split('.').filter(|p| p.len() > 8) {
            assert!(!log.contains(part), "part of a token reached the log");
        }
    }
    for marker in [
        "ATTACKER-AUDIENCE",
        "ATTACKER-NAME",
        "ATTACKER-ISSUER",
        "ATTACKER-EMAIL",
    ] {
        assert!(!log.contains(marker), "{marker} reached the log");
    }
}
