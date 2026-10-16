//! The property the fuzz target `mcp_http_body` asserts (S4, S11, S22): arbitrary bytes as the body of a `POST /mcp` that carries
//! valid credentials and a live session, through the whole service (the gate, rmcp's decoder, the handler, the tools) on fakes.
//!
//! Included, with `#[path]`, by `fuzz_targets/mcp_http_body.rs` and by `tests/mcp_auth/corpus_replay.rs`. Both depend on the
//! `hub-mcp` crate (the target with the `fakes` feature), so this file names the crate by its path, `hub_mcp::`.
//!
//! # What must hold
//!
//! | Check | Why |
//! |---|---|
//! | the answer starts within 5 seconds | no body makes a worker spin or wait |
//! | the status is one of 200, 202, 400, 404, 406, 413, 415, 422, 429 and 503: never 401 or 403 (the credentials are valid), never another 5xx | a body is the caller's mistake or a call; it never breaks the server, and it never revokes a good session |
//! | a 200 is `text/event-stream` or `application/json` | the shapes a client knows |
//! | at most 512 KiB come back, and the first message (or the end of the stream) comes within 2 seconds | bounded output and time |
//! | after the body the session still works: a `ping` is answered | no body kills the session of the caller or the service |
//!
//! The service is rebuilt every 200 inputs, so that sessions opened by bodies that happen to be valid `initialize` requests do not
//! pile up toward the caps for the fuzzer's whole life.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use domain::{Guid, Role, ShortText, User, UserId, UserStatus};
use http_body_util::BodyExt;
use hub_mcp::auth::oauth::OAuthSettings;
use hub_mcp::config::{AuthMode, McpConfig, PublicOrigin};
use hub_mcp::{Credentials, McpDeps, McpSettings, McpState, mount};
use hub_platform::{FakeSessionAuth, GuardedRouter};
use ports::VerifiedUser;
use ports::fakes::{FakeDocSearch, FakeRegistry, FakeTokenVerifier, FakeUsers, FakeWriteService};
use tower::ServiceExt;

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

const HOST: &str = "hub.example.test";
const TOKEN: &str = "entra-token-for-the-fuzzer";
const REBUILD_EVERY: u32 = 200;
const ALLOWED: [u16; 10] = [200, 202, 400, 404, 406, 413, 415, 422, 429, 503];
const MAX_OUTPUT: usize = 512 * 1024;

fn caller() -> UserId {
    UserId::new(
        Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
        Guid::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
    )
}

struct Harness {
    rt: tokio::runtime::Runtime,
    router: Router,
    session: String,
    served: u32,
}

fn request(session: Option<&str>, body: Vec<u8>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::HOST, HOST)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body)).unwrap()
}

impl Harness {
    fn build() -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("no runtime: {e}"))?;
        let verifier = FakeTokenVerifier::new();
        verifier.accept_entra(
            TOKEN,
            VerifiedUser {
                id: caller(),
                email: None,
                display_name: None,
            },
        );
        let users = FakeUsers::new();
        users.insert(User {
            id: caller(),
            email: ShortText::parse("fuzzer@example.test").unwrap(),
            display_name: ShortText::parse("Fuzzer").unwrap(),
            role: Some(Role::Admin),
            status: UserStatus::Active,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        });
        let oauth = OAuthSettings {
            tenant_id: Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
            api_audience: "api://hub-api".to_owned(),
            authority_host: OAuthSettings::DEFAULT_AUTHORITY_HOST.to_owned(),
            authorization_server_mirror: false,
        };
        let mut settings = McpSettings::new(
            McpConfig {
                auth_mode: AuthMode::Oauth,
                ..McpConfig::default()
            },
            PublicOrigin::parse(&format!("https://{HOST}")).unwrap(),
            Some(oauth),
        );
        settings.sse_keep_alive = None;
        // The rate limit has tests of its own; the fuzzer sends thousands of calls a second.
        settings.limits.read_per_minute = 60_000_000;
        settings.limits.read_burst = 1_000_000;
        settings.limits.write_per_minute = 6_000_000;
        settings.limits.write_burst = 1_000_000;
        settings.limits.sessions_per_user = 1_000;
        let state = McpState::new(
            McpDeps::new(
                Credentials::Oauth(Arc::new(verifier)),
                Arc::new(users),
                Arc::new(FakeRegistry::new()),
                Arc::new(FakeDocSearch::new()),
                Arc::new(FakeWriteService::new()),
            ),
            settings,
        )
        .map_err(|e| format!("no state: {e:?}"))?;
        let (router, _routes) = mount(GuardedRouter::new(Arc::new(FakeSessionAuth::new()), None), &state)
            .into_parts()
            .map_err(|e| format!("no routes: {e:?}"))?;

        // Open the session the bodies are sent to.
        let init = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fuzz","version":"0"}}}"#;
        let session = rt.block_on(async {
            let response = router
                .clone()
                .oneshot(request(None, init.to_vec()))
                .await
                .map_err(|e| format!("initialize failed: {e}"))?;
            if !response.status().is_success() {
                return Err(format!("initialize answered {}", response.status()));
            }
            let session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .ok_or_else(|| "initialize gave no session".to_owned())?;
            first_message(response.into_body()).await?;
            let initialized = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
            let response = router
                .clone()
                .oneshot(request(Some(&session), initialized.to_vec()))
                .await
                .map_err(|e| format!("initialized failed: {e}"))?;
            if response.status() != StatusCode::ACCEPTED {
                return Err(format!(
                    "notifications/initialized answered {}",
                    response.status()
                ));
            }
            Ok::<_, String>(session)
        })?;
        Ok(Self {
            rt,
            router,
            session,
            served: 0,
        })
    }
}

/// Read a response body until its first message, the end of the stream, or the bound on output. Returns the bytes read.
async fn first_message(mut body: Body) -> Result<Vec<u8>, String> {
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(2), body.frame()).await {
            Err(_) => return Err("no message and no end of the stream within 2 seconds".to_owned()),
            Ok(None) => return Ok(seen),
            Ok(Some(Err(e))) => return Err(format!("the body broke: {e}")),
            Ok(Some(Ok(frame))) => {
                if let Ok(data) = frame.into_data() {
                    seen.extend_from_slice(&data);
                    if seen.len() > MAX_OUTPUT {
                        return Err(format!("more than {MAX_OUTPUT} bytes came back"));
                    }
                    // A message is a `data:` line with something on it.
                    let text = String::from_utf8_lossy(&seen);
                    if text
                        .lines()
                        .filter_map(|l| l.strip_prefix("data:"))
                        .any(|d| !d.trim().is_empty())
                    {
                        return Ok(seen);
                    }
                }
            }
        }
    }
}

thread_local! {
    static HARNESS: RefCell<Option<Harness>> = const { RefCell::new(None) };
}

/// `mcp_http_body`.
pub fn mcp_http_body(data: &[u8]) -> Result<(), String> {
    HARNESS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|h| h.served >= REBUILD_EVERY) {
            *slot = Some(Harness::build()?);
        }
        let harness = slot.as_mut().ok_or_else(|| "no harness".to_owned())?;
        harness.served += 1;
        let (router, session) = (harness.router.clone(), harness.session.clone());
        harness.rt.block_on(async {
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                router.clone().oneshot(request(Some(&session), data.to_vec())),
            )
            .await
            .map_err(|_| "no answer within 5 seconds".to_owned())?
            .map_err(|e| format!("the service failed: {e}"))?;
            let status = response.status();
            if !ALLOWED.contains(&status.as_u16()) {
                return Err(format!("status {status} for a body with valid credentials"));
            }
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            if status == StatusCode::OK
                && !(content_type.starts_with("text/event-stream")
                    || content_type.starts_with("application/json"))
            {
                return Err(format!("a 200 with content type {content_type:?}"));
            }
            if status == StatusCode::OK {
                first_message(response.into_body()).await?;
            } else {
                // A refusal is small: it holds a message and not the request.
                let bytes = tokio::time::timeout(Duration::from_secs(2), response.into_body().collect())
                    .await
                    .map_err(|_| "a refusal whose body does not end".to_owned())?
                    .map_err(|e| format!("the body broke: {e}"))?
                    .to_bytes();
                if bytes.len() > MAX_OUTPUT {
                    return Err(format!("a refusal of {} bytes", bytes.len()));
                }
            }

            // The session still answers.
            let ping = br#"{"jsonrpc":"2.0","id":99999,"method":"ping"}"#;
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                router.oneshot(request(Some(&session), ping.to_vec())),
            )
            .await
            .map_err(|_| "no answer to a ping within 5 seconds".to_owned())?
            .map_err(|e| format!("the service failed on a ping: {e}"))?;
            if response.status() != StatusCode::OK {
                return Err(format!("a ping after the body answered {}", response.status()));
            }
            let seen = first_message(response.into_body()).await?;
            let seen = String::from_utf8_lossy(&seen);
            if !seen.contains("\"result\"") {
                return Err(format!(
                    "a ping after the body got no result: {}",
                    seen.chars().take(300).collect::<String>()
                ));
            }
            Ok(())
        })
    })
}
