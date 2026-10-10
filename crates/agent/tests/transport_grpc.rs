//! The gRPC transport against a real tonic hub behind real TLS (T3, S6): who is asked for a certificate, what is
//! compressed, and what each way of failing looks like to the caller.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::config::BaseUrl;
use agent::identity::joiner::JoinClient;
use agent::identity::{JoinError, KeyMaterial};
use agent::transport::tls::HubRoots;
use agent::transport::wire;
use agent::transport::{GrpcTransport, HubTransport, TlsFailure, TransportError};
use bytes::Bytes;
use domain::{AgentReply, ContentHash, Expected, NfsPath, RequestId, Secret};
use futures::StreamExt;
use proto::convert::{FromAgent, JoinCredential, JoinParams, JoinSubject, ToAgent};
use support::clock::TestClock;
use support::fake_hub::{HUB_URL, HubServer};
use support::rig::{Rig, swimlane};
use support::test_ca::{T0_MS, TestCa};
use support::tls::HubVersions;

fn file_reply(len: usize) -> FromAgent {
    FromAgent::Reply(AgentReply::File {
        request_id: RequestId::parse("req-1").unwrap(),
        path: NfsPath::parse("a/b.yml").unwrap(),
        hash: ContentHash::from_bytes([1; 32]),
        bytes: Bytes::from(vec![0_u8; len]),
    })
}

#[tokio::test(start_paused = true)]
async fn join_has_no_client_cert_connect_has() {
    let rig = Rig::new();
    let identity = rig.join().await;

    let joins = rig.server.joins();
    assert_eq!(joins.len(), 1);
    assert!(
        !joins[0].has_client_cert,
        "Join must work with no certificate at all"
    );

    let _connection = rig.connect(&identity).await.unwrap();
    let conn = rig.server.wait_for_connection(1).await;
    assert_eq!(
        conn.client_cert().as_deref(),
        Some(identity.chain_der()[0].as_ref()),
        "Connect presents the certificate the join returned"
    );
}

#[tokio::test(start_paused = true)]
async fn hello_is_the_first_message_and_a_hub_that_reads_it_before_answering_does_not_deadlock() {
    let rig = Rig::new();
    let identity = rig.join().await;
    // The fake hub reads `Hello` before it answers. If the agent waited for the answer first, this would never return.
    let connection = tokio::time::timeout(Duration::from_secs(60), rig.connect(&identity))
        .await
        .expect("connect hung: Hello was not sent before the response was awaited")
        .unwrap();
    let conn = rig.server.wait_for_connection(1).await;
    assert_eq!(conn.hello().swimlane, swimlane());
    assert_eq!(conn.received_count(), 1);
    drop(connection);
}

#[tokio::test(start_paused = true)]
async fn stream_uses_zstd() {
    let rig = Rig::new();
    let identity = rig.join().await;
    let mut connection = rig.connect(&identity).await.unwrap();
    let conn = rig.server.wait_for_connection(1).await;

    assert_eq!(conn.grpc_encoding().as_deref(), Some("zstd"), "agent to hub");
    assert_eq!(
        rig.server.joins()[0].grpc_encoding.as_deref(),
        Some("zstd"),
        "Join too"
    );

    // And it really compresses, both ways: a mebibyte of zeros each way costs a few kilobytes on the wire.
    let _ = connection.inbound.next().await; // the AgentConfig
    let before = rig.server.net.counts();
    connection
        .outbound
        .send(wire::encode(file_reply(1024 * 1024)))
        .await
        .unwrap();
    conn.wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::File { .. })))
        .await;
    conn.send(ToAgent::Command(domain::HubCommand::WriteFile {
        request_id: RequestId::parse("req-2").unwrap(),
        path: NfsPath::parse("a/b.yml").unwrap(),
        expected: Expected::Absent,
        bytes: Bytes::from(vec![0_u8; 1024 * 1024]),
    }))
    .await;
    let message = connection.inbound.next().await.unwrap().unwrap();
    match wire::decode(message) {
        wire::Decoded::Message(ToAgent::Command(domain::HubCommand::WriteFile { bytes, .. })) => {
            assert_eq!(bytes.len(), 1024 * 1024, "the content arrives whole");
        }
        other => panic!("{other:?}"),
    }
    let used = rig.server.net.counts().since(before);
    assert!(used.to_hub < 64 * 1024, "to the hub: {used:?}");
    assert!(used.from_hub < 64 * 1024, "from the hub: {used:?}");
}

#[tokio::test(start_paused = true)]
async fn messages_from_the_hub_arrive_in_order_and_the_end_of_the_stream_is_reported() {
    let rig = Rig::new();
    let identity = rig.join().await;
    let mut connection = rig.connect(&identity).await.unwrap();
    let conn = rig.server.wait_for_connection(1).await;

    let first = connection.inbound.next().await.unwrap().unwrap();
    assert!(matches!(
        wire::decode(first),
        wire::Decoded::Message(ToAgent::Config(_))
    ));
    for seq in [1, 2, 3] {
        conn.send(ToAgent::Ack(seq)).await;
    }
    for seq in [1, 2, 3] {
        let message = connection.inbound.next().await.unwrap().unwrap();
        assert_eq!(wire::decode(message), wire::Decoded::Message(ToAgent::Ack(seq)));
    }
    conn.end_stream();
    // An orderly end is the end of the stream, not an error.
    assert!(connection.inbound.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_connection_closes_the_stream_toward_the_hub() {
    let rig = Rig::new();
    let identity = rig.join().await;
    let connection = rig.connect(&identity).await.unwrap();
    let conn = rig.server.wait_for_connection(1).await;
    drop(connection);
    tokio::time::timeout(Duration::from_secs(60), conn.wait_closed())
        .await
        .expect("the hub never saw the stream close");
}

// ------------------------------------------------------------------ ways of failing

#[tokio::test(start_paused = true)]
async fn an_unreachable_hub_is_reported_as_unreachable() {
    let rig = Rig::new();
    let identity = rig.join().await;
    rig.server.net.set_reachable(false);
    assert_eq!(
        rig.connect(&identity).await.err(),
        Some(TransportError::Unreachable)
    );
    let params = JoinParams {
        subject: JoinSubject::Agent(swimlane()),
        csr_der: KeyMaterial::generate().unwrap().csr_der().unwrap(),
        credential: JoinCredential::JoinToken(Secret::new("t".to_owned())),
    };
    assert_eq!(
        JoinClient::join(&*rig.transport, params).await.err(),
        Some(JoinError::Unavailable)
    );
}

#[tokio::test(start_paused = true)]
async fn a_tls12_only_hub_is_refused_before_anything_is_sent() {
    let rig = Rig::with(HubVersions::Tls12Only);
    let identity = {
        // The join needs a hub that works, so mint the identity from a second, ordinary hub of the same CA.
        let good = Rig::new();
        good.join().await
    };
    assert_eq!(
        rig.connect(&identity).await.err(),
        Some(TransportError::Tls(TlsFailure::Version))
    );
    assert_eq!(rig.server.connection_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_hub_certificate_from_another_ca_is_refused() {
    let rig = Rig::new();
    let identity = rig.join().await;
    let other = TestCa::new();
    let pinned_elsewhere = GrpcTransport::new(
        &BaseUrl::parse(HUB_URL, "https").unwrap(),
        HubRoots::from_pem(other.pem().as_bytes()).unwrap(),
        rig.server.net.dialer(),
    )
    .unwrap();
    let error = pinned_elsewhere
        .connect(&identity, wire::encode(FromAgent::Hello(support::rig::hello())))
        .await
        .err();
    assert_eq!(error, Some(TransportError::Tls(TlsFailure::Certificate)));
    assert_eq!(rig.server.connection_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_refusal_from_the_hub_is_reported_as_a_refusal() {
    let rig = Rig::new();
    let identity = rig.join().await;
    rig.server.set_refuse_connect(true);
    assert_eq!(rig.connect(&identity).await.err(), Some(TransportError::Refused));
}

#[tokio::test(start_paused = true)]
async fn a_rejected_join_credential_is_reported_as_rejected() {
    let rig = Rig::new();
    rig.server.hub.reject_next_joins(1);
    let params = JoinParams {
        subject: JoinSubject::Agent(swimlane()),
        csr_der: KeyMaterial::generate().unwrap().csr_der().unwrap(),
        credential: JoinCredential::JoinToken(Secret::new("wrong".to_owned())),
    };
    assert_eq!(
        JoinClient::join(&*rig.transport, params).await.err(),
        Some(JoinError::Rejected)
    );
}

#[test]
fn the_hub_address_must_be_https_and_name_a_host() {
    let roots = HubRoots::from_pem(TestCa::new().pem().as_bytes()).unwrap();
    let dialer = || {
        HubServer::start(Arc::new(TestClock::starting_at(T0_MS)))
            .net
            .dialer()
    };
    let plain = BaseUrl::parse("http://hub.example.com:8080", "http").unwrap();
    assert!(matches!(
        GrpcTransport::new(&plain, roots.clone(), dialer()),
        Err(TransportError::Address(_))
    ));
    // Names, addresses and bracketed IPv6 addresses are all fine; the port defaults to 443.
    for url in [
        "https://hub.example.com",
        "https://10.1.2.3:8443",
        "https://[::1]:8443",
    ] {
        let url = BaseUrl::parse(url, "https").unwrap();
        GrpcTransport::new(&url, roots.clone(), dialer()).unwrap_or_else(|e| panic!("{url}: {e}"));
    }
}
