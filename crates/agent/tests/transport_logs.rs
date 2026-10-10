//! What must never reach a log line from the transport (S10, S21): tokens, the private key, file content, and text
//! the hub chose. The log is captured at every level, including the libraries' own (`h2`, `hyper`, `rustls`), across
//! a join, a session with traffic in both directions, and each way of failing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use agent::config::BaseUrl;
use agent::identity::JoinError;
use agent::identity::joiner::{IdentityHandle, JoinClient};
use agent::transport::session::{ConnectionState, Session, SessionConfig};
use agent::transport::tls::HubRoots;
use agent::transport::{GrpcTransport, HubTransport};
use bytes::Bytes;
use domain::{AgentReply, ContentHash, Expected, HubCommand, NfsPath, RequestId, Secret};
use proto::convert::{FromAgent, JoinCredential, JoinParams, JoinSubject, ToAgent};
use proto::pb;
use support::fake_hub::HUB_URL;
use support::log_capture::LogCapture;
use support::rig::{Recorder, Rig, hello, hello_message, swimlane};
use support::test_ca::TestCa;
use tokio::task::JoinHandle;
use tokio::time::sleep;

const ID_TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.UEFZTE9BRC1NQVJLRVItV0k.U0lHTkFUVVJFLU1BUktFUi1XSQ";
const JOIN_TOKEN: &str = "lk_join_MARKER_9f8e7d6c5b4a";
const FILE_MARKER: &str = "FILE-CONTENT-MARKER-5c1d";
const HOSTILE_PATH_MARKER: &str = "HOSTILE-PATH-MARKER-77aa";

fn join_params(credential: JoinCredential) -> JoinParams {
    JoinParams {
        subject: JoinSubject::Agent(swimlane()),
        csr_der: agent::identity::KeyMaterial::generate()
            .unwrap()
            .csr_der()
            .unwrap(),
        credential,
    }
}

/// Join with each kind of credential, once accepted and once rejected.
async fn joins(rig: &Rig) {
    for credential in [
        JoinCredential::GoogleIdToken(Secret::new(ID_TOKEN.to_owned())),
        JoinCredential::JoinToken(Secret::new(JOIN_TOKEN.to_owned())),
    ] {
        JoinClient::join(&*rig.transport, join_params(credential))
            .await
            .unwrap();
    }
    rig.server.hub.reject_next_joins(1);
    let rejected = JoinClient::join(
        &*rig.transport,
        join_params(JoinCredential::JoinToken(Secret::new(JOIN_TOKEN.to_owned()))),
    )
    .await;
    assert_eq!(rejected.err(), Some(JoinError::Rejected));
}

/// A session with traffic in both directions: file content each way, a hostile path, and an empty message.
async fn session_with_traffic(rig: &Rig, handle: Arc<IdentityHandle>) -> JoinHandle<Infallible> {
    let session = Arc::new(Session::new(
        hello(),
        rig.transport.clone(),
        handle,
        SessionConfig::default(),
    ));
    let recorder = Recorder::new();
    let task = {
        let (session, recorder) = (session.clone(), recorder.clone());
        tokio::spawn(async move { session.run(recorder).await })
    };
    let conn = rig.server.wait_for_connection(1).await;
    conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
    session
        .state()
        .wait_for(|s| *s == ConnectionState::Connected)
        .await
        .unwrap();
    conn.send(ToAgent::Command(HubCommand::WriteFile {
        request_id: RequestId::parse("req-1").unwrap(),
        path: NfsPath::parse("a/b.yml").unwrap(),
        expected: Expected::Absent,
        bytes: Bytes::from(FILE_MARKER),
    }))
    .await;
    conn.send_raw(pb::HubMessage {
        kind: Some(pb::hub_message::Kind::ReadFile(pb::ReadFile {
            request_id: "req-2".into(),
            path: format!("../{HOSTILE_PATH_MARKER}"),
        })),
    })
    .await;
    conn.send_raw(pb::HubMessage { kind: None }).await;
    recorder.outboxes()[0]
        .send(FromAgent::Reply(AgentReply::File {
            request_id: RequestId::parse("req-3").unwrap(),
            path: NfsPath::parse("a/b.yml").unwrap(),
            hash: ContentHash::from_bytes([3; 32]),
            bytes: Bytes::from(FILE_MARKER),
        }))
        .await
        .unwrap();
    conn.wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::File { .. })))
        .await;
    sleep(Duration::from_secs(1)).await;
    task
}

/// The connection is cut, the hub refuses, the network is gone, and the hub's certificate is not the pinned one.
async fn ways_of_failing(rig: &Rig) {
    rig.server.net.kill_connections();
    rig.server.wait_for_connection(2).await;
    rig.server.set_refuse_connect(true);
    rig.server.net.kill_connections();
    sleep(Duration::from_secs(120)).await;
    rig.server.net.set_reachable(false);
    sleep(Duration::from_secs(120)).await;

    let wrong_ca = GrpcTransport::new(
        &BaseUrl::parse(HUB_URL, "https").unwrap(),
        HubRoots::from_pem(TestCa::new().pem().as_bytes()).unwrap(),
        rig.server.net.dialer(),
    )
    .unwrap();
    rig.server.net.set_reachable(true);
    let identity = rig.join().await;
    let _ = wrong_ca.connect(&identity, hello_message()).await;
}

#[tokio::test(start_paused = true)]
async fn transport_logs_hold_no_secrets_content_or_hub_text() {
    let capture = LogCapture::new();
    let _guard = capture.install();
    let rig = Rig::new();

    joins(&rig).await;
    let handle = rig.identity_handle().await;
    let key_pem = handle.current().key().to_pem();
    let task = session_with_traffic(&rig, handle).await;
    ways_of_failing(&rig).await;
    task.abort();

    let log = capture.text();
    assert!(
        log.contains("connected to the hub"),
        "the capture works and the session logged: {log}"
    );
    let key_body = key_pem
        .expose()
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<String>();
    let forbidden: [(&str, &str); 7] = [
        ("the Google ID token", ID_TOKEN),
        ("the join token", JOIN_TOKEN),
        ("file content", FILE_MARKER),
        ("a path the hub chose", HOSTILE_PATH_MARKER),
        ("the private key (PEM header)", "PRIVATE KEY"),
        ("the private key (body)", &key_body[..40]),
        ("the payload of the ID token", "UEFZTE9BRC1NQVJLRVItV0k"),
    ];
    for (what, needle) in forbidden {
        assert!(!log.contains(needle), "{what} is in the log:\n{log}");
    }
}
