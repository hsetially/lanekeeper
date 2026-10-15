//! S10, S21: whatever the server logs while it authenticates, makes, lists and revokes tokens holds no token, no key, no hash, no
//! header, no label and no body. The filter of the hub keeps `rmcp` at `warn` (`crates/hub/tests/mcp_module.rs`), as rmcp logs whole
//! requests and results at `debug`; the capture here mirrors it.

use std::io;
use std::sync::{Arc, Mutex};

use axum::http::Method;
use domain::{Role, UserStatus};
use ports::AuthError;
use serde_json::json;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

use crate::rig::{ORIGIN, Rig};
use crate::support::json_body;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Everything this crate logs, at every level, and everything else at `warn`: the hub's own filter, widened for this crate.
fn subscriber(capture: &Capture) -> impl tracing::Subscriber + Send + Sync {
    let filter = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target("hub_mcp", LevelFilter::TRACE)
        .with_target("hub_platform", LevelFilter::TRACE);
    let output = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(capture.clone())
        .with_filter(filter);
    tracing_subscriber::Registry::default().with(output)
}

const LABEL: &str = "planted-label-7c1e";

#[tokio::test]
async fn log_capture_has_no_tokens_args_or_output() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(subscriber(&capture));
    tracing::warn!(target: "hub_mcp::test", "the capture works");

    // OAuth mode: good, unknown, expired, pending and unavailable.
    let oauth = Rig::oauth();
    oauth.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    oauth.add_user("pat", Some(Role::Viewer), UserStatus::Pending);
    oauth.verifier.accept("planted-oauth-token-alice", "alice");
    oauth.verifier.accept("planted-oauth-token-pending", "pat");
    oauth
        .verifier
        .refuse("planted-oauth-token-expired", AuthError::Expired);
    oauth
        .verifier
        .refuse("planted-oauth-token-down", AuthError::Unavailable);
    for token in [
        "planted-oauth-token-alice",
        "planted-oauth-token-pending",
        "planted-oauth-token-expired",
        "planted-oauth-token-down",
        "planted-oauth-token-unknown",
    ] {
        let mut client = oauth.client(Some(token));
        let _ = client
            .initialize(&json!({"planted": "planted-body-value-9d2f"}))
            .await;
    }

    // Personal-token mode: make, use, list, revoke, and the failures on the way.
    let personal = Rig::personal();
    personal.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    let body = json!({ "label": LABEL }).to_string();
    let created = personal
        .rest(
            Method::POST,
            "/api/v1/me/mcp-tokens",
            Some("alice"),
            Some(ORIGIN),
            Some(&body),
        )
        .await;
    let created = json_body(created).await;
    let token = created["token"].as_str().unwrap().to_owned();
    let id = created["id"].as_i64().unwrap();
    let mut client = personal.client(Some(&token));
    client.initialize(&json!({})).await.unwrap();
    let _ = personal
        .rest(Method::GET, "/api/v1/me/mcp-tokens", Some("alice"), None, None)
        .await;
    let _ = personal
        .client(Some(&format!("{}x", &token[..48])))
        .post(&json!({}))
        .await;
    let _ = personal
        .rest(
            Method::DELETE,
            &format!("/api/v1/me/mcp-tokens/{id}"),
            Some("alice"),
            Some(ORIGIN),
            None,
        )
        .await;
    let _ = personal.client(Some(&token)).post(&json!({})).await;

    let log = capture.text();
    assert!(log.contains("the capture works"), "{log}");
    let hmac = hex::encode(personal.store.as_ref().unwrap().rows()[0].hmac.as_bytes());
    for secret in [
        "planted-oauth-token",
        "planted-body-value-9d2f",
        &token,
        &token["lkpat_".len()..],
        &token[..20],
        LABEL,
        &hmac,
        "Bearer ",
        "authorization",
    ] {
        assert!(
            !log.to_ascii_lowercase().contains(&secret.to_ascii_lowercase()),
            "the log holds `{secret}`:\n{log}"
        );
    }
}

#[test]
fn debug_output_of_the_secret_bearing_types_is_redacted() {
    use hub_mcp::auth::token::{HmacKey, PersonalToken, generate};
    let token = generate().unwrap();
    let parsed = PersonalToken::parse(&token).unwrap();
    let key = HmacKey::from_bytes(&[3_u8; 32]).unwrap();
    for printed in [
        format!("{parsed:?}"),
        format!("{key:?}"),
        format!("{:?}", key.lookup(&parsed)),
    ] {
        assert!(printed.contains("redacted"));
        assert!(!printed.contains(&token["lkpat_".len()..]));
    }
}
