//! `NotifyConfigServer` (T12, D86, Q37): the agent POSTs the paths to the config-server's unauthenticated
//! `/update-resources`, with bounded time and bounded retries, and hands the HTTP status back to the hub unchanged.
//!
//! The config-server is a scripted HTTP server that records what it receives. It lives in memory, so the tests wait in
//! virtual time (a paused clock does not work with real sockets: the runtime jumps over the wait for the socket); one test
//! goes over a real loopback socket to cover the production path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::NfsRoot;
use agent::clock::Clock;
use agent::config::BaseUrl;
use agent::configserver::{self, ConfigServerClient, ConfigServerError, Timing};
use agent::deny::DenyList;
use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::FileOps;
use domain::{AgentReply, HubCommand, NfsPath, OpError, OpResult, RequestId};
use proto::convert::FromAgent;
use support::clock::TestClock;
use support::fake_configserver::FakeConfigServer;
use support::raw_server::Reply;
use tempfile::TempDir;
use tokio::time::Instant;

const LIMITS: OpLimits = OpLimits {
    max_file_bytes: 2 * 1024 * 1024,
};

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn nfs(path: &str) -> NfsPath {
    NfsPath::parse(path).unwrap()
}

fn base(url: &str) -> BaseUrl {
    BaseUrl::parse(url, "http").unwrap()
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(TestClock::starting_at(1_791_633_600_000))
}

/// A client for the in-memory server, with the real timing.
fn client(server: &FakeConfigServer) -> ConfigServerClient {
    server.client(DenyList::default(), clock())
}

struct Rig {
    dispatcher: Dispatcher,
    _dir: TempDir,
}

fn rig(server: &FakeConfigServer) -> Rig {
    let dir = TempDir::new().unwrap();
    let ops = FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    );
    let dispatcher = Dispatcher::new(ops).with_config_server(Arc::new(client(server)));
    Rig {
        dispatcher,
        _dir: dir,
    }
}

async fn notify(rig: &Rig, id: &str, paths: &[&str]) -> Option<FromAgent> {
    rig.dispatcher
        .handle(
            HubCommand::NotifyConfigServer {
                request_id: rid(id),
                paths: paths.iter().map(|p| nfs(p)).collect(),
            },
            LIMITS,
        )
        .await
}

fn op(reply: Option<FromAgent>) -> OpResult {
    match reply {
        Some(FromAgent::Reply(AgentReply::Op(result))) => result,
        other => panic!("an OpResult, got {other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------ the request

#[tokio::test(start_paused = true)]
async fn notify_posts_form_header_and_relative_paths() {
    let server = FakeConfigServer::start();
    let status = client(&server)
        .notify(&[
            nfs("tx-infinity-api/tx-infinity-core-sit1.yml"),
            nfs("tx-infinity-api/tx-infinity-core.yml"),
            nfs("remote-itm-teller/receipt-ci_1.bmp"),
        ])
        .await
        .unwrap();
    assert_eq!(status, 200);

    let seen = server.requests();
    assert_eq!(seen.len(), 1, "one POST for the whole list");
    // The golden request: the exact bytes the config-server receives.
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/update-resources");
    assert_eq!(request.header("backend"), Some("filesystem"));
    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        request.header("content-length"),
        Some(request.body.len().to_string().as_str())
    );
    assert_eq!(request.header("host"), Some(server.authority()));
    assert_eq!(
        String::from_utf8(request.body.clone()).unwrap(),
        "path=tx-infinity-api%2Ftx-infinity-core-sit1.yml\
         &path=tx-infinity-api%2Ftx-infinity-core.yml\
         &path=remote-itm-teller%2Freceipt-ci_1.bmp"
    );

    // Decoded by an independent reader: the field is repeated, in order, and every path is relative.
    let notification = &server.notifications()[0];
    assert_eq!(
        notification.paths,
        [
            "tx-infinity-api/tx-infinity-core-sit1.yml",
            "tx-infinity-api/tx-infinity-core.yml",
            "remote-itm-teller/receipt-ci_1.bmp"
        ]
    );
    assert!(notification.other_fields.is_empty());
    assert!(notification.paths.iter().all(|p| !p.starts_with('/')));
}

#[tokio::test(start_paused = true)]
async fn notify_form_encodes_what_would_break_a_form() {
    let server = FakeConfigServer::start();
    let awkward = [
        "a b/c&d=e.yml",
        "x;y/%41.yml",
        "é/ü.yml",
        "plus+sign/q?r#s.yml",
        "line\u{2028}sep.yml",
    ];
    let paths: Vec<NfsPath> = awkward.iter().map(|p| nfs(p)).collect();
    client(&server).notify(&paths).await.unwrap();
    let notification = &server.notifications()[0];
    assert_eq!(notification.paths, awkward, "every path comes out as it went in");
    assert!(
        notification.other_fields.is_empty(),
        "a path must not smuggle in a field of its own: {:?}",
        notification.other_fields
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn notify_with_no_paths_posts_an_empty_form() {
    let server = FakeConfigServer::start();
    let status = client(&server).notify(&[]).await.unwrap();
    assert_eq!(status, 200);
    let notification = &server.notifications()[0];
    assert!(notification.paths.is_empty());
    assert_eq!(notification.backend.as_deref(), Some("filesystem"));
}

#[test]
fn notify_form_is_built_from_the_paths_only() {
    assert_eq!(configserver::notify_body(&[]), "");
    assert_eq!(
        configserver::notify_body(&[nfs("a.yml"), nfs("b/c d.yml")]),
        "path=a.yml&path=b%2Fc%20d.yml"
    );
}

// ------------------------------------------------------------------------------------------------ time and retries

#[test]
fn the_timeouts_and_retries_are_the_documented_ones() {
    let timing = Timing::default();
    assert_eq!(timing.timeout, Duration::from_secs(5));
    assert_eq!(timing.attempts, 3, "bounded retries: three attempts in all");
    assert!(timing.backoff_cap <= Duration::from_secs(2));
    assert!(timing.backoff_base <= timing.backoff_cap);
    assert_eq!(configserver::MAX_RESPONSE_BYTES, 2 * 1024 * 1024);
}

#[tokio::test(start_paused = true)]
async fn notify_timeout_5s_with_bounded_retries() {
    let server = FakeConfigServer::start();
    server.script_notify([Reply::Hang]);
    let started = Instant::now();
    let error = client(&server).notify(&[nfs("a.yml")]).await.unwrap_err();
    let took = started.elapsed();
    assert_eq!(error, ConfigServerError::Unreachable);
    assert_eq!(server.requests().len(), 3, "three attempts and no more");
    // Three waits of five seconds, and two jittered pauses of at most 0.2 s and 0.4 s between them.
    assert!(took >= Duration::from_secs(15), "took {took:?}");
    assert!(
        took <= Duration::from_secs(15) + Duration::from_millis(600),
        "took {took:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn notify_retries_transport_failures_until_the_server_answers() {
    let server = FakeConfigServer::start();
    server.script_notify([
        Reply::Close,
        Reply::Raw(b"not http".to_vec()),
        Reply::status(202, ""),
    ]);
    let status = client(&server).notify(&[nfs("a.yml")]).await.unwrap();
    assert_eq!(status, 202);
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn an_answer_with_an_error_status_is_not_retried() {
    for status in [400, 404, 500, 503] {
        let server = FakeConfigServer::start();
        server.notify_status(status);
        let got = client(&server).notify(&[nfs("a.yml")]).await.unwrap();
        assert_eq!(got, status);
        assert_eq!(
            server.requests().len(),
            1,
            "the config-server answered {status}: that is an answer, not a failure"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_status_the_hub_would_reject_is_a_failure_not_an_answer() {
    // The wire accepts 100 to 599 only; a hostile server sending 700 must not make the agent send a message the hub drops.
    let server = FakeConfigServer::start();
    server.script_notify([Reply::Raw(
        b"HTTP/1.1 700 Odd\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    )]);
    let error = client(&server).notify(&[nfs("a.yml")]).await.unwrap_err();
    assert_eq!(error, ConfigServerError::Unreachable);
}

#[tokio::test(start_paused = true)]
async fn the_notify_answer_body_is_capped_and_never_used() {
    let server = FakeConfigServer::start();
    server.script_notify([Reply::status(200, vec![b'x'; 1024 * 1024])]);
    let error = client(&server).notify(&[nfs("a.yml")]).await.unwrap_err();
    assert_eq!(
        error,
        ConfigServerError::Unreachable,
        "a body this large is not an answer to a refresh"
    );
}

// ------------------------------------------------------------------------------------------------ the hub's view

#[tokio::test(start_paused = true)]
async fn notify_answers_with_notify_result_and_http_status() {
    for status in [200_u16, 204, 301, 400, 404, 500, 503] {
        let server = FakeConfigServer::start();
        server.notify_status(status);
        let rig = rig(&server);
        let reply = notify(&rig, "n1", &["tx-infinity-api/tx-infinity-core.yml"]).await;
        let Some(FromAgent::Reply(AgentReply::Notify {
            request_id,
            status: got,
        })) = reply
        else {
            panic!("a Notify reply for {status}, got {reply:?}");
        };
        assert_eq!(request_id.as_str(), "n1");
        assert_eq!(got, status, "the status comes back unchanged");
        // And it is a message the hub accepts.
        let wire = FromAgent::Reply(AgentReply::Notify {
            request_id,
            status: got,
        });
        assert_eq!(
            FromAgent::from_proto(wire.clone().into_proto()).unwrap(),
            Some(wire)
        );
        assert_eq!(server.notifications().len(), 1, "{status}: exactly one POST");
    }
}

#[tokio::test(start_paused = true)]
async fn notify_connect_failure_answers_op_result_io() {
    let server = FakeConfigServer::absent();
    let rig = rig(&server);
    let result = op(notify(&rig, "n2", &["a.yml"]).await);
    assert!(!result.ok);
    assert_eq!(result.error, Some(OpError::Io));
    assert_eq!(result.current_hash, None);
    assert_eq!(result.request_id.as_str(), "n2");
}

#[tokio::test(start_paused = true)]
async fn notify_timeout_answers_op_result_io() {
    let server = FakeConfigServer::start();
    server.script_notify([Reply::Hang]);
    let rig = rig(&server);
    let result = op(notify(&rig, "n3", &["a.yml"]).await);
    assert_eq!((result.ok, result.error), (false, Some(OpError::Io)));
    assert_eq!(server.requests().len(), 3, "retries are used up first");
}

#[tokio::test(start_paused = true)]
async fn notify_without_a_configured_config_server_is_unsupported() {
    let dir = TempDir::new().unwrap();
    let ops = FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    );
    let dispatcher = Dispatcher::new(ops);
    let reply = dispatcher
        .handle(
            HubCommand::NotifyConfigServer {
                request_id: rid("n4"),
                paths: vec![nfs("a.yml")],
            },
            LIMITS,
        )
        .await;
    assert_eq!(op(reply).error, Some(OpError::Unsupported));
}

#[tokio::test(start_paused = true)]
async fn a_notify_reply_carries_no_path_or_server_text() {
    // A failure says a code; it never repeats a path, the URL or what the server wrote.
    let server = FakeConfigServer::start();
    server.script_notify([Reply::Raw(
        b"HTTP/1.1 500 SECRET-SERVER-TEXT\r\nContent-Length: 0\r\n\r\n".to_vec(),
    )]);
    let rig = rig(&server);
    let reply = notify(&rig, "n5", &["keys/MARKER-PATH.yml"]).await.unwrap();
    let text = format!("{reply:?}");
    assert!(!text.contains("SECRET-SERVER-TEXT"), "{text}");
    assert!(!text.contains("MARKER-PATH"), "{text}");
    assert!(!text.contains(server.authority()), "{text}");
}

// ------------------------------------------------------------------------------------------------ the production path

/// The same request over a real loopback socket, through the TCP dialer the agent runs with.
#[tokio::test]
async fn notify_over_a_real_socket() {
    let server = FakeConfigServer::start_tcp().await;
    let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(1_791_633_600_000));
    let client = ConfigServerClient::new(base(server.url()), DenyList::default(), clock);
    let status = client.notify(&[nfs("svc/a.yml")]).await.unwrap();
    assert_eq!(status, 200);
    let request = &server.requests()[0];
    assert_eq!(request.header("host"), Some(server.authority()));
    assert_eq!(request.header("backend"), Some("filesystem"));
    assert_eq!(server.notifications()[0].paths, ["svc/a.yml"]);

    // And a connection that is refused is an error, not a hang.
    let absent = FakeConfigServer::start_tcp().await;
    let url = absent.url().to_owned();
    drop(absent);
    let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(1_791_633_600_000));
    let refused = ConfigServerClient::new(base(&url), DenyList::default(), clock).with_timing(Timing {
        backoff_base: Duration::from_millis(1),
        backoff_cap: Duration::from_millis(2),
        ..Timing::default()
    });
    assert_eq!(
        refused.notify(&[nfs("a.yml")]).await,
        Err(ConfigServerError::Unreachable)
    );
}
