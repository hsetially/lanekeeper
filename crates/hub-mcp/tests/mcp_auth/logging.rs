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

#[tokio::test]
async fn tool_calls_log_what_was_called_and_nothing_it_carried() {
    use bytes::Bytes;
    use domain::{NfsPath, SwimlaneId};
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(subscriber(&capture));
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.limits.read_burst = 4;
            s
        },
    );
    let secret = crate::fix::fake_github_token();
    let path = "planted/path-3e9a.yml";
    rig.registry.inner.seed(
        &SwimlaneId::parse("sit1").unwrap(),
        &[(
            NfsPath::parse(path).unwrap(),
            Bytes::from(format!("name: FILE-TEXT-MARKER-77\ntoken: {secret}\n")),
        )],
    );
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    rig.verifier.accept("planted-oauth-token-alice", "alice");
    let mut client = rig.client(Some("planted-oauth-token-alice"));
    client.initialize(&json!({})).await.unwrap();
    // A good read, a search, a call the role forbids, arguments that are wrong, a missing file, and one over the rate.
    let calls = [
        ("read_file", json!({"swimlane": "sit1", "path": path})),
        ("search_text", json!({"pattern": "ARG-PATTERN-5b21"})),
        (
            "restart_service",
            json!({"swimlane": "sit1", "service": {"namespace": "apps", "name": "core"}}),
        ),
        ("get_swimlane", json!({"swimlane": "ARG-VALUE-INVALID-1c44!"})),
        (
            "read_file",
            json!({"swimlane": "sit1", "path": "planted/missing-8d01.yml"}),
        ),
        ("list_swimlanes", json!({})),
        ("list_swimlanes", json!({})),
    ];
    for (tool, args) in &calls {
        let _ = client.call_tool_to_end(tool, args).await;
    }
    let log = capture.text();
    for expected in [
        "tool=\"read_file\"",
        "tool=\"search_text\"",
        "tool=\"restart_service\"",
        "tool=\"get_swimlane\"",
        "outcome=\"ok\"",
        "outcome=\"forbidden\"",
        "outcome=\"invalid_argument\"",
        "outcome=\"not_found\"",
        "outcome=\"rate_limited\"",
    ] {
        assert!(log.contains(expected), "no `{expected}` in the log:\n{log}");
    }
    for never in [
        "planted-oauth-token",
        &secret,
        "ghp_",
        "FILE-TEXT-MARKER-77",
        "ARG-PATTERN-5b21",
        "ARG-VALUE-INVALID",
        "planted/path-3e9a",
        "missing-8d01",
        "bearer",
        "authorization",
    ] {
        assert!(
            !log.to_ascii_lowercase().contains(&never.to_ascii_lowercase()),
            "the log holds `{never}`:\n{log}"
        );
    }
}

/// S10 and the crate conventions: one line per call, and the line says who made it. The stable internal user id (`tid:oid`) and
/// nothing else about the caller: not the email, not the display name; and still no argument and no text. A read and a refused call
/// are as attributable as a write (reads are not audited, so this line is their only trace).
#[tokio::test]
async fn every_call_line_carries_the_stable_user_id_and_nothing_else_about_the_caller() {
    use bytes::Bytes;
    use domain::{NfsPath, SwimlaneId};
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(subscriber(&capture));
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.limits.read_burst = 4;
            s
        },
    );
    let secret = crate::fix::fake_github_token();
    let path = "planted/path-3e9a.yml";
    rig.registry.inner.seed(
        &SwimlaneId::parse("sit1").unwrap(),
        &[(
            NfsPath::parse(path).unwrap(),
            Bytes::from(format!("name: FILE-TEXT-MARKER-77\ntoken: {secret}\n")),
        )],
    );
    // The key becomes the email and the display name too, so one word finds all three.
    let alice_key = "zanzibar-quokka-4417";
    let bob_key = "marmalade-heron-8830";
    let alice = rig.add_user(alice_key, Some(Role::Viewer), UserStatus::Active);
    let bob = rig.add_user(bob_key, Some(Role::Viewer), UserStatus::Active);
    rig.verifier.accept("planted-oauth-token-alice", alice_key);
    rig.verifier.accept("planted-oauth-token-bob", bob_key);
    let mut a = rig.client(Some("planted-oauth-token-alice"));
    a.initialize(&json!({})).await.unwrap();
    let mut b = rig.client(Some("planted-oauth-token-bob"));
    b.initialize(&json!({})).await.unwrap();

    // Alice: a good read, a search, a call her role forbids, wrong arguments, a missing file, a tool that does not exist, and
    // one call over the rate. Bob: one good read.
    let calls = [
        ("read_file", json!({"swimlane": "sit1", "path": path})),
        ("search_text", json!({"pattern": "ARG-PATTERN-5b21"})),
        (
            "restart_service",
            json!({"swimlane": "sit1", "service": {"namespace": "apps", "name": "core"}}),
        ),
        ("get_swimlane", json!({"swimlane": "ARG-VALUE-INVALID-1c44!"})),
        (
            "read_file",
            json!({"swimlane": "sit1", "path": "planted/missing-8d01.yml"}),
        ),
        ("ARG-TOOL-NAME-9c1d", json!({"swimlane": "sit1"})),
        ("list_swimlanes", json!({})),
        ("list_swimlanes", json!({})),
    ];
    for (tool, args) in &calls {
        let _ = a.call_tool_to_end(tool, args).await;
    }
    let _ = b.call_tool_to_end("list_swimlanes", &json!({})).await;

    let log = capture.text();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains("tool call")).collect();
    assert_eq!(lines.len(), calls.len() + 1, "one line per call:\n{log}");
    let alice_field = format!("user_id={alice}");
    let bob_field = format!("user_id={bob}");
    for line in &lines[..calls.len()] {
        assert!(
            line.contains(&alice_field),
            "alice's call is not attributed: {line}"
        );
        assert!(!line.contains(&bob_field), "{line}");
    }
    let last = lines[calls.len()];
    assert!(last.contains(&bob_field), "bob's call is not attributed: {last}");
    assert!(!last.contains(&alice_field), "{last}");
    // A read, a refusal by role, by arguments, by rate and by the name of the tool: each is a line with its outcome and its user.
    for outcome in [
        "ok",
        "forbidden",
        "invalid_argument",
        "not_found",
        "rate_limited",
        "unknown_tool",
    ] {
        let wanted = format!("outcome=\"{outcome}\"");
        assert!(
            lines
                .iter()
                .any(|l| l.contains(&wanted) && l.contains(&alice_field)),
            "no attributed line for `{outcome}`:\n{log}"
        );
    }
    // The id is `tid:oid`, which is not an address of a person; the email, the name, the arguments and the text are not in the log.
    for never in [
        "zanzibar",
        "marmalade",
        "@example.test",
        "planted-oauth-token",
        &secret,
        "FILE-TEXT-MARKER-77",
        "ARG-PATTERN-5b21",
        "ARG-VALUE-INVALID",
        "ARG-TOOL-NAME-9c1d",
        "planted/path-3e9a",
        "missing-8d01",
    ] {
        assert!(
            !log.to_ascii_lowercase().contains(&never.to_ascii_lowercase()),
            "the log holds `{never}`:\n{log}"
        );
    }
}

#[test]
fn debug_of_write_args_prints_no_content() {
    use hub_mcp::tools::args::{ToolName, parse_args};
    let marker = "CONTENT-MARKER-0f5e";
    let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    for (tool, args) in [
        (
            ToolName::ProposeChange,
            json!({"swimlane": "s", "path": "a", "expected": {"state": "absent"}, "content": marker}),
        ),
        (
            ToolName::UploadFile,
            json!({"swimlane": "s", "path": "a", "expected": {"state": "hash", "hash": hash}, "content": marker}),
        ),
        (
            ToolName::RaisePr,
            json!({"swimlane": "s", "paths": ["a"], "title": "t", "body": marker}),
        ),
        (ToolName::SearchText, json!({"pattern": marker})),
        (ToolName::GrepDocs, json!({"pattern": marker})),
        (ToolName::SearchDocs, json!({"q": marker})),
    ] {
        let parsed = parse_args(tool, args.as_object().cloned()).unwrap();
        let printed = format!("{parsed:?} {parsed:#?}");
        assert!(!printed.contains(marker), "{tool}: {printed}");
        assert!(printed.contains("Opaque("), "{tool}: {printed}");
    }
}
