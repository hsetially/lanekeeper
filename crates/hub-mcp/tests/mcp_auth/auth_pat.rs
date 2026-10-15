//! S7, S9, S14 in personal-token mode, over the in-memory store. The same behaviour against a real Postgres is `auth_pat_pg`.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use domain::{Role, UserStatus};
use hub_mcp::auth::pat::{PersonalTokens, TokenError, TokenSettings};
use hub_mcp::auth::store::{Label, TokenId};
use hub_mcp::auth::token::PersonalToken;
use hub_mcp::clock::Clock;
use ports::{AuditAction, AuthError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::rig::{EIGHT_DAYS_SECS, Rig, key, uid};
use crate::support::json_body;

fn label(text: &str) -> Label {
    Label::parse(text).unwrap()
}

/// A personal-token rig with one active viewer, `alice`.
fn rig() -> Rig {
    let rig = Rig::personal();
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    rig
}

async fn make(rig: &Rig, name: &str) -> (String, hub_mcp::auth::store::TokenRecord) {
    let created = rig
        .tokens
        .as_ref()
        .unwrap()
        .create(&uid("alice"), label(name), None)
        .await
        .unwrap();
    (created.token.expose().clone(), created.record)
}

async fn status_with(rig: &Rig, token: &str) -> StatusCode {
    let client = rig.client(Some(token));
    client.post(&initialize()).await.status()
}

fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}
    })
}

#[tokio::test]
async fn pat_roundtrip_creates_and_authenticates() {
    let rig = rig();
    let (token, record) = make(&rig, "laptop").await;
    assert!(token.starts_with("lkpat_"));
    assert_eq!(record.label.as_str(), "laptop");
    assert_eq!(record.owner, uid("alice"));
    // The lifetime is seven days from the moment it was written.
    assert_eq!(
        record.expires_at.unix_millis() - record.created_at.unix_millis(),
        7 * 24 * 60 * 60 * 1000
    );
    let mut client = rig.client(Some(&token));
    let result = client.initialize(&json!({})).await.unwrap();
    assert_eq!(result["serverInfo"]["name"], "lanekeeper");
    assert!(client.session.is_some());
}

#[tokio::test]
async fn pat_is_stored_as_hmac_not_plaintext() {
    let rig = rig();
    let (token, _) = make(&rig, "laptop").await;
    let rows = rig.store.as_ref().unwrap().rows();
    assert_eq!(rows.len(), 1);
    let parsed = PersonalToken::parse(&token).unwrap();
    // What is stored is the keyed hash of the token text, and it is not a plain hash of it.
    assert_eq!(rows[0].hmac, key().lookup(&parsed));
    let plain_sha = Sha256::digest(token.as_bytes());
    assert_ne!(rows[0].hmac.as_bytes()[..], plain_sha[..]);
    // Nothing the store holds contains the token or the random part of it.
    let dump = format!("{rows:?}");
    assert!(!dump.contains(&token));
    assert!(!dump.contains(&token["lkpat_".len()..]));
    let events = format!("{:?}", rig.audit.entries());
    assert!(!events.contains(&token["lkpat_".len()..]));
    assert!(!events.contains(&hex::encode(rows[0].hmac.as_bytes())));
}

#[tokio::test]
async fn wrong_key_does_not_authenticate() {
    let rig = rig();
    let (token, _) = make(&rig, "laptop").await;
    // Another deployment's key, the same table: the hash it computes finds nothing.
    let other = PersonalTokens::new(
        rig.store.clone().unwrap(),
        hub_mcp::auth::token::HmacKey::from_bytes(&[1_u8; 32]).unwrap(),
        Arc::new(rig.audit.clone()),
        rig.clock.clone() as Arc<dyn Clock>,
        TokenSettings::default(),
    );
    assert_eq!(other.authenticate(&token).await, Err(AuthError::InvalidToken));
    assert_eq!(
        rig.tokens.as_ref().unwrap().authenticate(&token).await,
        Ok(uid("alice"))
    );
}

#[tokio::test]
async fn expired_pat_is_401() {
    let rig = rig();
    let (token, _) = make(&rig, "laptop").await;
    assert_eq!(status_with(&rig, &token).await, StatusCode::OK);
    rig.clock.advance_secs(7 * 24 * 60 * 60 - 1);
    assert_eq!(
        status_with(&rig, &token).await,
        StatusCode::OK,
        "one second before its end"
    );
    rig.clock.advance_secs(1);
    assert_eq!(status_with(&rig, &token).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        rig.tokens.as_ref().unwrap().authenticate(&token).await,
        Err(AuthError::Expired)
    );
}

#[tokio::test]
async fn a_cached_token_is_still_checked_for_expiry() {
    // Four seconds before its end the entry is cached for five more; the token still ends on time.
    let rig = rig();
    let (token, _) = make(&rig, "laptop").await;
    rig.clock.advance_secs(7 * 24 * 60 * 60 - 4);
    assert_eq!(status_with(&rig, &token).await, StatusCode::OK);
    rig.clock.advance_secs(4);
    assert_eq!(status_with(&rig, &token).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_pat_is_401_at_once_on_the_same_replica() {
    let rig = rig();
    let (token, record) = make(&rig, "laptop").await;
    assert_eq!(status_with(&rig, &token).await, StatusCode::OK, "cached now");
    rig.tokens
        .as_ref()
        .unwrap()
        .revoke(&uid("alice"), record.id, None)
        .await
        .unwrap();
    // No time has passed: the revoking replica does not wait for its cache to expire.
    assert_eq!(status_with(&rig, &token).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_pat_is_401_within_the_cache_ttl_on_another_replica() {
    let rig = rig();
    let (token, record) = make(&rig, "laptop").await;
    // A second replica: its own cache, the same table.
    let other = PersonalTokens::new(
        rig.store.clone().unwrap(),
        key(),
        Arc::new(rig.audit.clone()),
        rig.clock.clone() as Arc<dyn Clock>,
        TokenSettings::default(),
    );
    assert_eq!(other.authenticate(&token).await, Ok(uid("alice")));
    rig.tokens
        .as_ref()
        .unwrap()
        .revoke(&uid("alice"), record.id, None)
        .await
        .unwrap();
    // It has not heard yet. This is the documented window: at most five seconds.
    rig.clock.advance_secs(4);
    assert_eq!(other.authenticate(&token).await, Ok(uid("alice")));
    rig.clock.advance_secs(1);
    assert_eq!(other.authenticate(&token).await, Err(AuthError::InvalidToken));
}

#[tokio::test]
async fn pat_of_pending_user_is_403() {
    let rig = rig();
    let (token, _) = make(&rig, "laptop").await;
    rig.users
        .insert(crate::rig::user("alice", Some(Role::Viewer), UserStatus::Pending));
    assert_eq!(status_with(&rig, &token).await, StatusCode::FORBIDDEN);
    rig.users.insert(crate::rig::user(
        "alice",
        Some(Role::Viewer),
        UserStatus::Disabled,
    ));
    assert_eq!(status_with(&rig, &token).await, StatusCode::FORBIDDEN);
    rig.users
        .insert(crate::rig::user("alice", Some(Role::Viewer), UserStatus::Active));
    assert_eq!(
        status_with(&rig, &token).await,
        StatusCode::OK,
        "a token is not lost with a pending spell"
    );
}

#[tokio::test]
async fn pat_in_oauth_mode_and_jwt_in_pat_mode_are_both_401() {
    // A JWT in personal-token mode: refused by its shape, and the store is not asked.
    let rig = rig();
    let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJhcGk6Ly9odWItYXBpIn0.c2ln";
    assert_eq!(status_with(&rig, jwt).await, StatusCode::UNAUTHORIZED);
    assert_eq!(rig.store.as_ref().unwrap().lookups(), 0);
    // A personal token in OAuth mode is `auth_oauth::a_personal_token_in_oauth_mode_is_401_and_never_reaches_the_verifier`.
    // The challenge of personal-token mode points at no metadata.
    let response = rig.client(None).post(&initialize()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["www-authenticate"], "Bearer");
    let response = rig.client(Some(jwt)).post(&initialize()).await;
    assert_eq!(
        response.headers()["www-authenticate"],
        "Bearer error=\"invalid_token\""
    );
}

#[tokio::test]
async fn pat_mode_publishes_no_discovery_metadata() {
    let rig = rig();
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
        "/.well-known/openid-configuration",
    ] {
        let response = rig.client(None).get(path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(response.headers().get("www-authenticate").is_none(), "{path}");
    }
    assert!(
        rig.routes.iter().all(|r| !r.path.starts_with("/.well-known")),
        "no discovery route is registered in this mode"
    );
    // The mirror switch belongs to OAuth mode: settings that carry it in personal-token mode do not start.
    let mut settings = crate::rig::settings(hub_mcp::config::AuthMode::PersonalToken, false);
    settings.oauth = Some(crate::rig::oauth_settings(true));
    let started = hub_mcp::McpState::new(
        hub_mcp::McpDeps::new(
            hub_mcp::Credentials::PersonalToken(rig.tokens.clone().unwrap()),
            Arc::new(rig.users.clone()),
            Arc::new(rig.registry.clone()),
            Arc::new(rig.docs.clone()),
            Arc::new(rig.writes.clone()),
        ),
        settings,
    );
    assert!(started.is_err());
}

#[tokio::test]
async fn at_most_five_live_tokens_per_user() {
    let rig = rig();
    let tokens = rig.tokens.as_ref().unwrap();
    let mut made = Vec::new();
    for n in 0..5 {
        made.push(make(&rig, &format!("token {n}")).await);
    }
    let sixth = tokens.create(&uid("alice"), label("sixth"), None).await;
    assert!(matches!(sixth, Err(TokenError::LimitReached)));
    // Revoking one makes room.
    tokens.revoke(&uid("alice"), made[0].1.id, None).await.unwrap();
    make(&rig, "replacement").await;
    assert!(matches!(
        tokens.create(&uid("alice"), label("again"), None).await,
        Err(TokenError::LimitReached)
    ));
    // Another user has their own five.
    rig.add_user("bob", Some(Role::Viewer), UserStatus::Active);
    assert!(tokens.create(&uid("bob"), label("bob's"), None).await.is_ok());
    // Tokens that have ended do not count.
    rig.clock.advance_secs(EIGHT_DAYS_SECS);
    assert!(
        tokens
            .create(&uid("alice"), label("after a week"), None)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_token_that_was_not_made_with_the_limit_in_mind_still_counts_the_limit_from_settings() {
    let rig = Rig::personal_with(TokenSettings {
        max_live: 1,
        ..TokenSettings::default()
    });
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    let tokens = rig.tokens.as_ref().unwrap();
    tokens.create(&uid("alice"), label("one"), None).await.unwrap();
    assert!(matches!(
        tokens.create(&uid("alice"), label("two"), None).await,
        Err(TokenError::LimitReached)
    ));
}

#[tokio::test]
async fn token_is_shown_once_and_never_listed() {
    let rig = rig();
    let (token, record) = make(&rig, "laptop").await;
    let tokens = rig.tokens.as_ref().unwrap();
    let listed = tokens.list(&uid("alice"), None, 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    // The list route's body, as the web UI gets it: the label and the times, and neither the token nor anything derived from it.
    let response = rig
        .rest(Method::GET, "/api/v1/me/mcp-tokens", Some("alice"), None, None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let text = body.to_string();
    assert!(!text.contains(&token["lkpat_".len()..]), "{text}");
    assert!(!text.contains("lkpat_"), "{text}");
    assert!(!text.contains("token_hmac") && !text.contains("hmac"), "{text}");
    assert_eq!(body["items"][0]["id"], record.id.get());
    assert_eq!(body["items"][0]["label"], "laptop");
    assert_eq!(body["next"], Value::Null);
}

#[tokio::test]
async fn create_and_revoke_write_exactly_one_audit_event_each() {
    let rig = rig();
    assert!(rig.audit.entries().is_empty());
    let (token, record) = make(&rig, "laptop").await;
    let entries = rig.audit.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event.action, AuditAction::McpTokenCreated);
    let tokens = rig.tokens.as_ref().unwrap();
    tokens.revoke(&uid("alice"), record.id, None).await.unwrap();
    assert_eq!(rig.audit.entries().len(), 2);
    assert_eq!(rig.audit.entries()[1].event.action, AuditAction::McpTokenRevoked);
    // Revoking again changes nothing and writes nothing.
    tokens.revoke(&uid("alice"), record.id, None).await.unwrap();
    assert_eq!(rig.audit.entries().len(), 2);
    // The events name the owner and the label, and never the token or its hash.
    for entry in rig.audit.entries() {
        assert_eq!(entry.event.actor, ports::AuditActor::User { user: uid("alice") });
        let json = serde_json::to_string(&entry.event).unwrap();
        assert!(json.contains("laptop"), "{json}");
        assert!(!json.contains(&token["lkpat_".len()..]), "{json}");
        assert!(!json.contains(&hex::encode(record.hmac.as_bytes())), "{json}");
    }
}

#[tokio::test]
async fn revoking_someone_elses_token_or_a_missing_one_is_not_found() {
    let rig = rig();
    rig.add_user("bob", Some(Role::Viewer), UserStatus::Active);
    let (token, record) = make(&rig, "laptop").await;
    let tokens = rig.tokens.as_ref().unwrap();
    assert!(matches!(
        tokens.revoke(&uid("bob"), record.id, None).await,
        Err(TokenError::NotFound)
    ));
    assert!(matches!(
        tokens
            .revoke(&uid("alice"), TokenId::new(999).unwrap(), None)
            .await,
        Err(TokenError::NotFound)
    ));
    assert_eq!(
        rig.audit.entries().len(),
        1,
        "a refused revocation writes nothing"
    );
    assert_eq!(
        status_with(&rig, &token).await,
        StatusCode::OK,
        "and does not revoke"
    );
}

#[tokio::test]
async fn rollback_leaves_no_token_and_no_event() {
    // An audit log that cannot write: the token is not made either (rule 8). The Postgres version of this is in `auth_pat_pg`.
    struct Broken;
    #[async_trait::async_trait]
    impl ports::AuditLog for Broken {
        async fn record(
            &self,
            _tx: &mut ports::Tx<'_>,
            _e: ports::AuditEvent,
        ) -> Result<domain::AuditId, ports::AuditError> {
            Err(ports::AuditError::Unavailable)
        }
    }
    let rig = rig();
    let tokens = PersonalTokens::new(
        rig.store.clone().unwrap(),
        key(),
        Arc::new(Broken),
        rig.clock.clone() as Arc<dyn Clock>,
        TokenSettings::default(),
    );
    let made = tokens.create(&uid("alice"), label("laptop"), None).await;
    assert!(matches!(made, Err(TokenError::Unavailable)), "{made:?}");
    assert!(rig.store.as_ref().unwrap().rows().is_empty());
    assert!(rig.audit.entries().is_empty());
    // Nor is a token revoked when its event cannot be written.
    let (token, record) = make(&rig, "laptop").await;
    let revoked = tokens.revoke(&uid("alice"), record.id, None).await;
    assert!(matches!(revoked, Err(TokenError::Unavailable)));
    assert_eq!(status_with(&rig, &token).await, StatusCode::OK);
}

#[tokio::test]
async fn the_store_is_not_asked_again_within_the_cache_window() {
    let rig = rig();
    let (token, _) = make(&rig, "laptop").await;
    let store = rig.store.clone().unwrap();
    for _ in 0..10 {
        assert_eq!(status_with(&rig, &token).await, StatusCode::OK);
    }
    assert_eq!(store.lookups(), 1);
    rig.clock.advance_secs(5);
    assert_eq!(status_with(&rig, &token).await, StatusCode::OK);
    assert_eq!(
        store.lookups(),
        2,
        "after the window the store is asked once more"
    );
}

#[tokio::test]
async fn a_token_that_is_not_known_is_never_cached() {
    let rig = rig();
    let unknown = hub_mcp::auth::token::generate().unwrap();
    for _ in 0..3 {
        assert_eq!(status_with(&rig, &unknown).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        rig.store.as_ref().unwrap().lookups(),
        3,
        "a made-up token cannot fill the cache"
    );
    // Nor is one that is revoked.
    let (token, record) = make(&rig, "laptop").await;
    rig.tokens
        .as_ref()
        .unwrap()
        .revoke(&uid("alice"), record.id, None)
        .await
        .unwrap();
    let before = rig.store.as_ref().unwrap().lookups();
    for _ in 0..3 {
        assert_eq!(status_with(&rig, &token).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(rig.store.as_ref().unwrap().lookups(), before + 3);
}

#[tokio::test]
async fn a_cache_of_zero_seconds_asks_the_store_every_time() {
    let rig = Rig::personal_with(TokenSettings {
        cache_ttl: Duration::ZERO,
        ..TokenSettings::default()
    });
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    let (token, _) = make(&rig, "laptop").await;
    for _ in 0..3 {
        assert_eq!(status_with(&rig, &token).await, StatusCode::OK);
    }
    assert_eq!(rig.store.as_ref().unwrap().lookups(), 3);
}

#[tokio::test]
async fn last_used_is_moved_at_most_every_ten_seconds() {
    let rig = Rig::personal_with(TokenSettings {
        cache_ttl: Duration::ZERO,
        ..TokenSettings::default()
    });
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    let (token, _) = make(&rig, "laptop").await;
    let used = || rig.store.as_ref().unwrap().rows()[0].last_used_at;
    assert_eq!(used(), None);
    status_with(&rig, &token).await;
    let first = used().unwrap();
    rig.clock.advance_secs(9);
    status_with(&rig, &token).await;
    assert_eq!(used(), Some(first), "nine seconds later nothing was written");
    rig.clock.advance_secs(2);
    status_with(&rig, &token).await;
    assert!(used().unwrap() > first);
}

#[tokio::test]
async fn labels_are_bounded_and_have_no_control_characters() {
    for bad in [
        "",
        "   ",
        "\t",
        &"x".repeat(65),
        "line\nbreak",
        "bell\u{7}",
        "nul\0",
    ] {
        assert!(Label::parse(bad).is_err(), "{bad:?}");
    }
    assert!(Label::parse(&"x".repeat(64)).is_ok());
    assert!(
        Label::parse(&"\u{e9}".repeat(64)).is_ok(),
        "64 characters, not 64 bytes"
    );
    assert!(Label::parse("work laptop (2026)").is_ok());
}

#[test]
fn settings_out_of_range_are_refused() {
    assert!(TokenSettings::default().validate().is_ok());
    for bad in [
        TokenSettings {
            lifetime: Duration::ZERO,
            ..TokenSettings::default()
        },
        TokenSettings {
            lifetime: Duration::from_secs(32 * 24 * 60 * 60),
            ..TokenSettings::default()
        },
        TokenSettings {
            max_live: 0,
            ..TokenSettings::default()
        },
        TokenSettings {
            max_live: 101,
            ..TokenSettings::default()
        },
        TokenSettings {
            cache_ttl: Duration::from_secs(61),
            ..TokenSettings::default()
        },
        TokenSettings {
            cache_capacity: 0,
            ..TokenSettings::default()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}
