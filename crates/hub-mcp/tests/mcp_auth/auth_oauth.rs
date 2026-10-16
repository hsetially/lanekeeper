//! S4, S11, S14 in OAuth mode: the gate in front of `/mcp`. The verifier is scripted to answer each `AuthError`; the audience, scope
//! and tenant checks themselves are 03a's `tokens.rs` tests and are not repeated here.

use axum::body::Body;
use axum::http::{Method, Response, StatusCode, header};
use domain::{Role, UserStatus};
use ports::AuthError;
use serde_json::{Value, json};

use crate::rig::{ORIGIN, Rig, user};
use crate::support::{Client, json_body};

const CHALLENGE: &str = "Bearer resource_metadata=\"https://hub.example.test/.well-known/oauth-protected-resource/mcp\", \
                         scope=\"api://hub-api/mcp.access\"";

fn initialize_body() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }
    })
}

fn challenge(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .map(|v| v.to_str().unwrap().to_owned())
}

async fn code(response: Response<Body>) -> String {
    json_body(response).await["code"].as_str().unwrap().to_owned()
}

/// A rig with one active viewer, `alice`, whose token is `tok-alice`.
fn rig() -> Rig {
    let rig = Rig::oauth();
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    rig.verifier.accept("tok-alice", "alice");
    rig
}

async fn every_method(rig: &Rig, bearer: Option<&str>) -> Vec<Response<Body>> {
    let client = rig.client(bearer);
    vec![
        client.post(&initialize_body()).await,
        client
            .send(client.request(Method::GET).body(Body::empty()).unwrap())
            .await,
        client
            .send(client.request(Method::DELETE).body(Body::empty()).unwrap())
            .await,
    ]
}

#[tokio::test]
async fn missing_header_is_401_with_resource_metadata_challenge() {
    let rig = rig();
    for response in every_method(&rig, None).await {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // No token was presented, so the challenge names the metadata and the scope and no error (RFC 6750 section 3.1).
        assert_eq!(challenge(&response).as_deref(), Some(CHALLENGE));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(code(response).await, "unauthorized");
    }
    assert_eq!(rig.verifier.calls(), 0, "a request without a token asks nobody");
}

#[tokio::test]
async fn invalid_token_is_401() {
    let rig = rig();
    for response in every_method(&rig, Some("tok-nobody")).await {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            challenge(&response).as_deref(),
            Some(format!("{CHALLENGE}, error=\"invalid_token\"").as_str())
        );
    }
}

#[tokio::test]
async fn expired_token_is_401() {
    let rig = rig();
    rig.verifier.refuse("tok-old", AuthError::Expired);
    let response = rig.client(Some("tok-old")).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge(&response).unwrap().contains("invalid_token"));
}

#[tokio::test]
async fn wrong_audience_is_401() {
    let rig = rig();
    rig.verifier.refuse("tok-other-api", AuthError::WrongAudience);
    let response = rig.client(Some("tok-other-api")).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_without_mcp_scope_is_401() {
    // The verifier answers `InvalidToken` for a token whose `scp` lacks `mcp.access`, as it does for every token it cannot accept.
    let rig = rig();
    rig.verifier.refuse("tok-no-scope", AuthError::InvalidToken);
    let response = rig.client(Some("tok-no-scope")).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unavailable_verifier_is_503_not_401() {
    let rig = rig();
    rig.verifier.otherwise(AuthError::Unavailable);
    let response = rig.client(Some("tok-anyone")).post(&initialize_body()).await;
    // The keys of the identity provider could not be fetched: not a verdict on the token, so no sign-in is started over it.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(challenge(&response).is_none());
    assert_eq!(code(response).await, "unavailable");
}

#[tokio::test]
async fn unknown_user_is_401() {
    let rig = rig();
    rig.verifier.accept("tok-stranger", "stranger");
    let response = rig.client(Some("tok-stranger")).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pending_user_is_403() {
    let rig = rig();
    rig.add_user("pat", Some(Role::Viewer), UserStatus::Pending);
    rig.verifier.accept("tok-pat", "pat");
    for response in every_method(&rig, Some("tok-pat")).await {
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            challenge(&response).is_none(),
            "a signed-in user who may not is not asked to sign in again"
        );
        assert_eq!(code(response).await, "forbidden");
    }
}

#[tokio::test]
async fn disabled_user_is_403() {
    let rig = rig();
    rig.add_user("dave", Some(Role::Admin), UserStatus::Disabled);
    rig.verifier.accept("tok-dave", "dave");
    let response = rig.client(Some("tok-dave")).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "whatever the role");
}

#[tokio::test]
async fn a_user_with_no_role_is_403() {
    let rig = rig();
    rig.add_user("norole", None, UserStatus::Active);
    rig.verifier.accept("tok-norole", "norole");
    let response = rig.client(Some("tok-norole")).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn role_change_takes_effect_on_next_call() {
    let rig = rig();
    let mut alice = rig.client(Some("tok-alice"));
    alice.initialize(&json!({})).await.unwrap();
    assert_eq!(alice.open_event_stream().await, StatusCode::OK);

    // An admin takes her access away. Nothing is cached on this side, so her next call is refused.
    rig.users.insert(user("alice", None, UserStatus::Active));
    let (_, refused) = alice.call_tool("anything", &json!({})).await;
    assert_eq!(refused.err(), Some(StatusCode::FORBIDDEN));
    rig.users
        .insert(user("alice", Some(Role::Viewer), UserStatus::Disabled));
    assert_eq!(alice.open_event_stream().await, StatusCode::FORBIDDEN);
    // And back.
    rig.users
        .insert(user("alice", Some(Role::Viewer), UserStatus::Active));
    assert_eq!(alice.open_event_stream().await, StatusCode::OK);
}

#[tokio::test]
async fn token_longer_than_max_is_refused_before_parsing() {
    let rig = rig();
    let long = "a".repeat(16 * 1024 + 1);
    let response = rig.client(Some(&long)).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(rig.verifier.calls(), 0, "the verifier never saw it");
    // At the limit it is looked at (and refused by the verifier, which does not know it).
    let at_limit = "a".repeat(16 * 1024);
    let response = rig.client(Some(&at_limit)).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(rig.verifier.calls(), 1);
}

#[tokio::test]
async fn two_credentials_are_refused_and_neither_is_used() {
    let rig = rig();
    let client = rig.client(None);
    let request = client
        .request(Method::POST)
        .header(header::AUTHORIZATION, "Bearer tok-alice")
        .header(header::AUTHORIZATION, "Bearer tok-alice")
        .body(Body::from(initialize_body().to_string()))
        .unwrap();
    let response = client.send(request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(rig.verifier.calls(), 0);
}

#[tokio::test]
async fn another_scheme_is_refused() {
    let rig = rig();
    let client = rig.client(None);
    let request = client
        .request(Method::POST)
        .header(header::AUTHORIZATION, "Basic YWxpY2U6c2VjcmV0")
        .body(Body::from(initialize_body().to_string()))
        .unwrap();
    assert_eq!(client.send(request).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(rig.verifier.calls(), 0);
}

#[tokio::test]
async fn a_personal_token_in_oauth_mode_is_401_and_never_reaches_the_verifier() {
    let rig = rig();
    let personal = format!("lkpat_{}", "A".repeat(43));
    let response = rig.client(Some(&personal)).post(&initialize_body()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        rig.verifier.calls(),
        0,
        "OAuth mode has no code path for the other mode's tokens"
    );
}

#[tokio::test]
async fn an_authenticated_session_works_end_to_end() {
    let rig = rig();
    let mut alice = rig.client(Some("tok-alice"));
    let result = alice.initialize(&json!({})).await.unwrap();
    assert_eq!(result["serverInfo"]["name"], "lanekeeper");
    assert!(alice.session.is_some());
    assert_eq!(rig.state.live_sessions().await, 1);
    assert!(alice.delete_session().await.is_success());
    assert_eq!(rig.state.live_sessions().await, 0);
}

#[tokio::test]
async fn session_id_is_bound_to_the_user() {
    let rig = rig();
    rig.add_user("bob", Some(Role::Admin), UserStatus::Active);
    rig.verifier.accept("tok-bob", "bob");
    let mut alice = rig.client(Some("tok-alice"));
    alice.initialize(&json!({})).await.unwrap();
    let session = alice.session.clone().unwrap();

    // Bob is a valid, active user with an Admin role and Alice's session id. A session id is never an authenticator: every method
    // is answered as rmcp answers an id it does not know, which does not reveal that the id exists.
    let mut bob = rig.client(Some("tok-bob"));
    bob.session = Some(session);
    let (_, post) = bob.call_tool("anything", &json!({})).await;
    assert_eq!(post.err(), Some(StatusCode::NOT_FOUND));
    assert_eq!(bob.open_event_stream().await, StatusCode::NOT_FOUND);
    assert_eq!(bob.delete_session().await, StatusCode::NOT_FOUND);
    assert_eq!(
        rig.state.live_sessions().await,
        1,
        "Bob did not end Alice's session"
    );

    // The owner is unaffected: she can open the stream and end her own session.
    assert_eq!(alice.open_event_stream().await, StatusCode::OK);
    assert!(alice.delete_session().await.is_success());
    assert_eq!(rig.state.live_sessions().await, 0);
}

#[tokio::test]
async fn a_session_whose_owner_is_not_remembered_is_not_served() {
    let rig = rig();
    let mut alice = rig.client(Some("tok-alice"));
    alice.initialize(&json!({})).await.unwrap();
    assert_eq!(alice.open_event_stream().await, StatusCode::OK);
    // The memory of who opened which session is bounded and expires. When it is gone the gate fails closed: the client gets the
    // answer for an unknown session and opens a new one.
    rig.state.forget_session_owners();
    assert_eq!(alice.open_event_stream().await, StatusCode::NOT_FOUND);
    let (_, post) = alice.call_tool("anything", &json!({})).await;
    assert_eq!(post.err(), Some(StatusCode::NOT_FOUND));
    assert_eq!(alice.delete_session().await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unknown_or_malformed_session_id_is_404() {
    let rig = rig();
    for id in [
        "not-a-session",
        "00000000-0000-4000-8000-000000000000",
        &"x".repeat(200),
    ] {
        let mut alice = rig.client(Some("tok-alice"));
        alice.session = Some(id.to_owned());
        assert_eq!(alice.open_event_stream().await, StatusCode::NOT_FOUND, "{id}");
    }
    // A header that is not text names no session either.
    let client = rig.client(Some("tok-alice"));
    let request = client
        .request(Method::GET)
        .header(
            "mcp-session-id",
            axum::http::HeaderValue::from_bytes(b"\xff\xfe").unwrap(),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(client.send(request).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn foreign_origin_is_refused() {
    let rig = rig();
    let mut alice: Client = rig.client(Some("tok-alice"));
    // A page on another site that has got hold of a token still cannot talk to the server from a browser.
    alice.origin = Some("https://evil.example".to_owned());
    assert_eq!(
        alice.post(&initialize_body()).await.status(),
        StatusCode::FORBIDDEN
    );
    alice.origin = Some("null".to_owned());
    assert_eq!(
        alice.post(&initialize_body()).await.status(),
        StatusCode::FORBIDDEN
    );
    // The token is not looked at for a request that is refused for its origin.
    assert_eq!(rig.verifier.calls(), 0);
    // The public origin is fine, and so is no origin at all (a desktop client sends none).
    alice.origin = Some(ORIGIN.to_owned());
    assert_eq!(alice.post(&initialize_body()).await.status(), StatusCode::OK);
    alice.origin = None;
    assert_eq!(alice.post(&initialize_body()).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_extra_allowed_origin_is_accepted_and_nothing_else() {
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.allowed_origins = vec!["vscode-file://vscode-app".to_owned()];
            s
        },
    );
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    rig.verifier.accept("tok-alice", "alice");
    let mut alice = rig.client(Some("tok-alice"));
    alice.origin = Some("vscode-file://vscode-app".to_owned());
    assert_eq!(alice.post(&initialize_body()).await.status(), StatusCode::OK);
    alice.origin = Some("vscode-file://other".to_owned());
    assert_eq!(
        alice.post(&initialize_body()).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_request_the_gate_refuses_never_reaches_a_handler() {
    // 401, 403 and 503 all leave the session count where it was.
    let rig = rig();
    rig.add_user("pat", Some(Role::Viewer), UserStatus::Pending);
    rig.verifier.accept("tok-pat", "pat");
    let _ = every_method(&rig, None).await;
    let _ = every_method(&rig, Some("tok-pat")).await;
    assert_eq!(rig.state.live_sessions().await, 0);
}
