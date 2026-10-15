//! Plan 07, A19: the three rmcp 3.5.0 APIs the design leans on, shown to exist and to behave as T1 to T3 need.
//!
//! 1. A session is bound to the user that opened it, and the request's `http::request::Parts` (with what the layer in front
//!    added) reach the handler on every call.
//! 2. A question that is open when the call is cancelled, the session ends or the server stops is withdrawn, and nothing proceeds.
//! 3. A session manager can be wrapped and capped.
//!
//! Each test names the finding it pins. A test that starts failing after an rmcp bump says that the design must be looked at
//! again; it is not a reason to loosen the test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

#[allow(dead_code)]
#[path = "../examples/t0_stub.rs"]
mod t0_stub;

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use support::{Client, elicitation_capabilities, good_token, other_token, result_text};
use t0_stub::{Outcome, StubConfig, build};
use tracing::Level;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;

/// No SSE keep-alive: several of these tests run on a paused clock, and the keep-alive timer of the `sse-stream` crate is re-armed
/// from the real clock, which would turn every wait on a stream into a real-time wait.
fn config() -> StubConfig {
    StubConfig {
        sse_keep_alive: None,
        ..StubConfig::default()
    }
}

/// Wait for the elicitation request on a `tools/call` stream.
async fn question_on(sse: &mut support::Sse) -> Value {
    loop {
        let message = sse.next().await.expect("the server asks a question");
        if message["method"] == "elicitation/create" {
            return message;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 1. Sessions bound to a user; the request's Parts reach the handler
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a19_request_parts_reach_the_handler_and_bind_the_session_to_one_caller() {
    let stub = build(&config());
    let mut alice = Client::new(stub.router.clone(), Some(good_token()));
    alice.initialize(&json!({})).await.unwrap();
    let session = alice.session.clone().unwrap();

    // The caller the layer in front put into the request extensions is what the handler sees: Alice can use her session.
    let response = alice.call_tool_to_end("t0_read", &json!({})).await;
    assert!(result_text(&response).contains("alpha-7f3a"), "{response}");

    // Bob has a valid token of his own and Alice's session id. The HTTP layer answers him as it answers an unknown session.
    let mut bob = Client::new(stub.router.clone(), Some(other_token()));
    bob.session = Some(session);
    let (_, refused) = bob.call_tool("t0_read", &json!({})).await;
    assert_eq!(refused.err(), Some(StatusCode::NOT_FOUND));

    // The layer's memory is bounded and expires. Once it is gone the request reaches the handler, which holds the binding too:
    // the request's `Parts` carry Bob's identity there, and the handler refuses him. A session id is never an authenticator.
    stub.forget_session_owners();
    let (id, reached) = bob.call_tool("t0_read", &json!({})).await;
    let (_, response) = reached
        .expect("the request reaches the handler")
        .until_response(id)
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], -32600, "{response}");
    assert!(response.get("result").is_none(), "{response}");

    // Alice is unaffected.
    let response = alice.call_tool_to_end("t0_read", &json!({})).await;
    assert!(response.get("result").is_some(), "{response}");
}

#[tokio::test]
async fn a19_binding_must_hold_for_get_and_delete_too_because_they_never_reach_the_handler() {
    let stub = build(&config());
    let mut alice = Client::new(stub.router.clone(), Some(good_token()));
    alice.initialize(&json!({})).await.unwrap();
    let session = alice.session.clone().unwrap();

    // The server-to-client stream (GET) and the end of the session (DELETE) are handled inside rmcp's transport and never
    // call the handler, so the handler's check cannot protect them. The stub's HTTP layer (`bind_sessions`) does; it answers
    // a different caller as rmcp answers an unknown session id, so the id's existence is not revealed.
    let mut bob = Client::new(stub.router.clone(), Some(other_token()));
    bob.session = Some(session.clone());
    assert_eq!(bob.open_event_stream().await, StatusCode::NOT_FOUND);
    assert_eq!(bob.delete_session().await, StatusCode::NOT_FOUND);
    assert_eq!(stub.sessions.live(), 1, "Bob did not end Alice's session");

    // The owner can open the stream and can end her own session.
    assert_eq!(alice.open_event_stream().await, StatusCode::OK);
    assert!(alice.delete_session().await.is_success());
    assert_eq!(stub.sessions.live(), 0);

    // A session that is gone is gone for everybody, and rmcp says 404 for it (A5: what Cursor does on this 404 is observed).
    assert_eq!(alice.open_event_stream().await, StatusCode::NOT_FOUND);
    let (_, sse) = alice.call_tool("t0_read", &json!({})).await;
    assert_eq!(sse.err(), Some(StatusCode::NOT_FOUND));
}

#[tokio::test]
async fn a19_an_initialize_request_gets_a_stateful_session_whatever_version_it_names() {
    // rmcp 3.5.0 speaks 2026-07-28, which has no sessions and uses multi-round-trip requests for questions (MRTR). A client
    // that opens with `initialize` still gets the classic stateful session, answered with a version that has the handshake, so
    // the design (a session, `elicitation/create`) holds for it. A client that skips `initialize` (`server/discover`) is the
    // case T0 must observe: `t0/README.md` records the negotiated version.
    let stub = build(&config());
    for asked in [
        "2025-03-26",
        "2025-06-18",
        "2025-11-25",
        "2026-07-28",
        "2099-01-01",
    ] {
        let mut client = Client::new(stub.router.clone(), Some(good_token()));
        let result = client
            .initialize_as(asked, &elicitation_capabilities())
            .await
            .unwrap_or_else(|s| panic!("{asked}: {s}"));
        let negotiated = result["protocolVersion"].as_str().unwrap();
        assert!(client.session.is_some(), "{asked}: no session id");
        assert!(
            ["2025-03-26", "2025-06-18", "2025-11-25"].contains(&negotiated),
            "{asked} negotiated {negotiated}"
        );
        assert!(
            negotiated <= asked || asked > "2025-11-25",
            "{asked} negotiated {negotiated}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 2. A question that is open when the call is cancelled, or the session ends, is withdrawn and nothing proceeds
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a19_a_cancelled_call_withdraws_the_open_question_and_nothing_proceeds() {
    let stub = build(&config());
    let mut client = Client::new(stub.router.clone(), Some(good_token()));
    client.initialize(&elicitation_capabilities()).await.unwrap();
    let (id, sse) = client.call_tool("t0_write", &json!({"note": "demo"})).await;
    let mut sse = sse.unwrap();
    let question = question_on(&mut sse).await;

    // The user closes the prompt in the client: the client cancels the tool call. The handler stops waiting.
    let status = client
        .notify(
            "notifications/cancelled",
            &json!({"requestId": id, "reason": "user cancelled"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let outcomes = tokio::time::timeout(Duration::from_secs(30), stub.probe.wait_for(1))
        .await
        .expect("the handler must stop waiting");
    assert_eq!(outcomes, [Outcome::Aborted]);

    // Nothing proceeds even if a late answer arrives for the withdrawn question.
    let _ = client
        .answer(
            &question["id"],
            &json!({"action": "accept", "content": {"confirm": true}}),
        )
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(stub.probe.outcomes(), [Outcome::Aborted]);
}

#[tokio::test(start_paused = true)]
async fn a19_ending_the_session_wakes_the_open_question_and_nothing_proceeds() {
    let stub = build(&config());
    let mut client = Client::new(stub.router.clone(), Some(good_token()));
    client.initialize(&elicitation_capabilities()).await.unwrap();
    let (_, sse) = client.call_tool("t0_write", &json!({})).await;
    let mut sse = sse.unwrap();
    question_on(&mut sse).await;

    // `DELETE` closes the session. rmcp drops the pending request, so the handler's wait fails at once (it does not sit out the
    // 120 s timeout) and the call ends without a confirmation. The handler sees `TransportClosed`, which the stub counts as cancelled.
    assert!(client.delete_session().await.is_success());
    let outcomes = tokio::time::timeout(Duration::from_secs(30), stub.probe.wait_for(1))
        .await
        .expect("the handler must stop waiting");
    assert!(
        matches!(outcomes.as_slice(), [Outcome::Aborted | Outcome::Cancelled]),
        "{outcomes:?}"
    );
    assert_eq!(stub.sessions.live(), 0);
}

#[tokio::test(start_paused = true)]
async fn a19_the_servers_own_stop_signal_wakes_the_open_question_and_nothing_proceeds() {
    let stub = build(&config());
    let mut client = Client::new(stub.router.clone(), Some(good_token()));
    client.initialize(&elicitation_capabilities()).await.unwrap();
    let (_, sse) = client.call_tool("t0_write", &json!({})).await;
    let mut sse = sse.unwrap();
    question_on(&mut sse).await;

    stub.shutdown();
    let outcomes = tokio::time::timeout(Duration::from_secs(5), stub.probe.wait_for(1))
        .await
        .expect("the handler must stop waiting");
    assert_eq!(outcomes, [Outcome::Aborted]);
}

#[tokio::test(start_paused = true)]
async fn a19_stopping_rmcp_alone_does_not_wake_an_open_question() {
    // The finding behind the test above: `StreamableHttpServerConfig::cancellation_token` ends rmcp's streams but a handler that
    // is waiting for an answer keeps waiting until its own timeout. Nothing proceeds in the meantime, but a confirmation must
    // have a server-wide stop of its own (T3) and a timeout (always).
    let stub = build(&StubConfig {
        elicit_timeout: Duration::from_secs(60),
        ..config()
    });
    let mut client = Client::new(stub.router.clone(), Some(good_token()));
    client.initialize(&elicitation_capabilities()).await.unwrap();
    let (_, sse) = client.call_tool("t0_write", &json!({})).await;
    let mut sse = sse.unwrap();
    question_on(&mut sse).await;

    stub.stop_transport();
    let woken = tokio::time::timeout(Duration::from_secs(30), stub.probe.wait_for(1)).await;
    assert!(
        woken.is_err(),
        "rmcp now wakes handlers when its token is cancelled; revisit T3's shutdown handling: {woken:?}"
    );
    let outcomes = tokio::time::timeout(Duration::from_secs(120), stub.probe.wait_for(1))
        .await
        .expect("the timeout ends the wait");
    assert_eq!(outcomes, [Outcome::TimedOut]);
}

#[tokio::test(start_paused = true)]
async fn a19_a_dropped_stream_leaves_the_question_to_the_timeout_and_nothing_is_applied() {
    // A client that vanishes without cancelling (a dead connection) is not noticed by rmcp's stateful transport until the
    // handler's own timeout, which is why every confirmation has one (120 s by default, plan "Confirmation").
    let stub = build(&StubConfig {
        elicit_timeout: Duration::from_secs(30),
        ..config()
    });
    let mut client = Client::new(stub.router.clone(), Some(good_token()));
    client.initialize(&elicitation_capabilities()).await.unwrap();
    let (_, sse) = client.call_tool("t0_write", &json!({})).await;
    let mut sse = sse.unwrap();
    question_on(&mut sse).await;
    drop(sse);

    let outcomes = tokio::time::timeout(Duration::from_secs(120), stub.probe.wait_for(1))
        .await
        .expect("the timeout must end the wait");
    assert_eq!(outcomes, [Outcome::TimedOut]);
}

#[tokio::test(start_paused = true)]
async fn a19_an_answer_from_another_session_is_ignored() {
    let stub = build(&StubConfig {
        elicit_timeout: Duration::from_secs(30),
        ..config()
    });
    let mut asking = Client::new(stub.router.clone(), Some(good_token()));
    asking.initialize(&elicitation_capabilities()).await.unwrap();
    let mut other = Client::new(stub.router.clone(), Some(good_token()));
    other.initialize(&elicitation_capabilities()).await.unwrap();

    let (id, sse) = asking.call_tool("t0_write", &json!({})).await;
    let mut sse = sse.unwrap();
    let question = question_on(&mut sse).await;

    // The other session answers "yes" to the first session's question (same caller, other session id). It must not confirm it.
    let _ = other
        .answer(
            &question["id"],
            &json!({"action": "accept", "content": {"confirm": true}}),
        )
        .await;
    let (_, response) = tokio::time::timeout(Duration::from_secs(300), sse.until_response(id))
        .await
        .unwrap()
        .unwrap();
    assert!(result_text(&response).starts_with("TIMED_OUT"), "{response}");
    assert_eq!(stub.probe.outcomes(), [Outcome::TimedOut]);
}

// ---------------------------------------------------------------------------------------------------------------------
// 3. A session manager can be wrapped and capped
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a19_session_manager_can_be_capped_and_a_closed_session_frees_its_slot() {
    let stub = build(&StubConfig {
        session_cap: 2,
        ..config()
    });
    let mut first = Client::new(stub.router.clone(), Some(good_token()));
    let mut second = Client::new(stub.router.clone(), Some(good_token()));
    let mut third = Client::new(stub.router.clone(), Some(good_token()));
    first.initialize(&json!({})).await.unwrap();
    second.initialize(&json!({})).await.unwrap();
    assert_eq!(stub.sessions.live(), 2);

    // The third is refused. rmcp turns a session-manager error into a 500; `T2` puts a per-user count and a 503 with a
    // Retry-After in front of this, and keeps the manager as the second line.
    let refused = third.initialize(&json!({})).await.unwrap_err();
    assert!(refused.is_server_error(), "{refused}");
    assert_eq!(stub.sessions.live(), 2, "a refused session holds no slot");
    assert!(third.session.is_none());

    // Ending a session frees its slot at once.
    assert!(first.delete_session().await.is_success());
    assert_eq!(stub.sessions.live(), 1);
    third.initialize(&json!({})).await.unwrap();
    assert_eq!(stub.sessions.live(), 2);
}

#[tokio::test]
async fn a19_many_concurrent_initializes_never_exceed_the_cap() {
    let stub = build(&StubConfig {
        session_cap: 3,
        ..config()
    });
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let router = stub.router.clone();
        tasks.spawn(async move {
            let mut client = Client::new(router, Some(good_token()));
            client.initialize(&json!({})).await.is_ok()
        });
    }
    let mut accepted = 0;
    while let Some(done) = tasks.join_next().await {
        accepted += usize::from(done.unwrap());
    }
    assert_eq!(accepted, 3);
    assert_eq!(stub.sessions.live(), 3);
}

// ---------------------------------------------------------------------------------------------------------------------
// S10 and S21: what rmcp itself logs
// ---------------------------------------------------------------------------------------------------------------------

/// Log lines per test thread, from one global subscriber installed once (see `stub_protocol.rs` for why not `set_default`). It
/// is deliberately permissive: the stub's filter plus `rmcp` at debug, so that the finding below can be shown. What the stub's own
/// filter lets through is tested on the filter itself.
#[derive(Clone, Default)]
struct PerThread(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

struct ThreadWriter(PerThread);

fn thread_name() -> String {
    std::thread::current().name().unwrap_or("unnamed").to_owned()
}

impl io::Write for ThreadWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .0
            .lock()
            .unwrap()
            .entry(thread_name())
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for PerThread {
    type Writer = ThreadWriter;
    fn make_writer(&'a self) -> ThreadWriter {
        ThreadWriter(self.clone())
    }
}

static LOGS: OnceLock<PerThread> = OnceLock::new();

fn capture_logs() -> &'static PerThread {
    let logs = LOGS.get_or_init(|| {
        let logs = PerThread::default();
        let filter = Targets::new()
            .with_default(LevelFilter::WARN)
            .with_target("t0", LevelFilter::INFO)
            .with_target("rmcp", LevelFilter::DEBUG);
        tracing::subscriber::set_global_default(t0_stub::log_subscriber(logs.clone(), filter)).unwrap();
        logs
    });
    logs.0.lock().unwrap().remove(&thread_name());
    logs
}

impl PerThread {
    fn text(&self) -> String {
        String::from_utf8_lossy(
            self.0
                .lock()
                .unwrap()
                .get(&thread_name())
                .map_or(&[][..], Vec::as_slice),
        )
        .into_owned()
    }
}

const SECRET_NOTE: &str = "planted-secret-note-4f9d2c";

#[tokio::test]
async fn a19_rmcp_logs_tool_arguments_when_its_target_is_allowed_below_warn() {
    // The finding: rmcp 3.5.0 writes `?request` (the whole request, tool arguments included) at debug and `?result` at debug.
    // A hub that lets the `rmcp` target log at debug would write file content into its logs.
    let logs = capture_logs();
    let stub = build(&config());
    let mut client = Client::new(stub.router.clone(), Some(good_token()));
    client.initialize(&elicitation_capabilities()).await.unwrap();
    let (id, sse) = client.call_tool("t0_write", &json!({"note": SECRET_NOTE})).await;
    let mut sse = sse.unwrap();
    let question = question_on(&mut sse).await;
    client
        .answer(&question["id"], &json!({"action": "decline"}))
        .await;
    sse.until_response(id).await.unwrap();

    let log = logs.text();
    assert!(
        log.contains(SECRET_NOTE),
        "rmcp no longer logs arguments at debug; re-read the finding in t0/README.md:\n{log}"
    );
    assert!(log.contains("received request"), "{log}");
}

#[test]
fn a19_the_stubs_filter_keeps_rmcp_below_warn_so_arguments_and_results_never_reach_the_log() {
    // The hub's filter must do the same (T1): `rmcp` never above `warn`. rmcp 3.5.0 logs requests and results at debug,
    // notifications at info, and every SSE message at trace.
    let filter = t0_stub::log_filter();
    for level in [Level::DEBUG, Level::INFO, Level::TRACE] {
        for target in [
            "rmcp::service",
            "rmcp::transport::streamable_http_server::tower",
            "rmcp",
        ] {
            assert!(!filter.would_enable(target, &level), "{target} at {level}");
        }
    }
    assert!(filter.would_enable("rmcp::service", &Level::WARN));
    // The stub's own lines are what T0 reads.
    for target in ["t0::claims", "t0::request", "t0::initialize"] {
        assert!(filter.would_enable(target, &Level::INFO), "{target}");
    }
    assert!(!filter.would_enable("t0::claims", &Level::DEBUG));
}
