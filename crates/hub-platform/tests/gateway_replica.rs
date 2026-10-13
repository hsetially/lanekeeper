//! T4: the replica listener and the `Forward` RPC from the outside (S5, S6, S11, S22). A client speaks real mutual TLS to
//! a replica's listener with whatever certificate the test gives it.
//!
//! The listener trusts the replica CA only, so an agent certificate cannot complete a handshake. Past the handshake, the
//! service checks that the caller's SAN names a hub replica, validates the request, and refuses an epoch that is not the
//! epoch of the stream it holds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::time::Duration;

use bytes::Bytes;
use domain::{AgentReply, Expected, HubCommand, NfsPath, RequestId, SwimlaneId};
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::gateway::local::Epoch;
use hub_platform::gateway::metrics::ServeOutcome;
use hub_platform::gateway::wire::{ForwardCall, pb};
use hub_platform::tls::{ClientTls, roots_from_der};
use support::cluster::{REPLICA_DNS, TestCluster};
use support::fake_agent::Behavior;
use support::grpc::{ClientIdentity, dial_named, identity_for};
use tonic::{Code, Request};

fn replica_client(h2: support::grpc::H2) -> pb::replica_client::ReplicaClient<support::grpc::H2> {
    pb::replica_client::ReplicaClient::new(h2)
}

async fn replica_channel(
    cluster: &TestCluster,
    hub: usize,
    identity: Option<&ClientIdentity>,
) -> std::io::Result<support::grpc::H2> {
    let roots = roots_from_der(&[cluster.replica_ca.ca_der.clone()]).unwrap();
    let tls = ClientTls::new(
        roots,
        identity.map(ClientIdentity::certified),
        cluster.replica_ca.clock.clone(),
    )
    .unwrap();
    dial_named(cluster.hubs[hub].replica_addr, tls.connector(), REPLICA_DNS)
        .await
        .map(|(h2, _)| h2)
}

fn command() -> HubCommand {
    HubCommand::ReadFile {
        request_id: RequestId::parse("r-1").unwrap(),
        path: NfsPath::parse("app/a.yml").unwrap(),
    }
}

fn request_for(swimlane: &str, epoch: i64, command: HubCommand) -> pb::ForwardRequest {
    ForwardCall {
        swimlane: SwimlaneId::parse(swimlane).unwrap(),
        epoch: Epoch::new(epoch),
        timeout: Duration::from_secs(3),
        command,
    }
    .into_proto()
}

async fn hub_identity(cluster: &TestCluster) -> ClientIdentity {
    let profile =
        CertProfile::replica(&PeerName::parse("hub-peer").unwrap(), &[REPLICA_DNS.to_owned()]).unwrap();
    identity_for(&cluster.replica_ca, &profile).await
}

/// A cluster of two replicas with sit1's agent on replica 1 at epoch 1.
async fn with_agent() -> Option<(TestCluster, support::fake_agent::AgentLink)> {
    let cluster = TestCluster::start(2).await?;
    cluster.seed(&["sit1"]).await;
    let agent = cluster.agent("sit1").await;
    let link = agent
        .connect(&cluster.agent_ca, cluster.hubs[1].agent_addr, Behavior::echo())
        .await
        .unwrap();
    Some((cluster, link))
}

#[tokio::test]
async fn replica_forward_accepts_a_hub_certificate_and_delivers_the_command() {
    let Some((cluster, link)) = with_agent().await else {
        return;
    };
    let identity = hub_identity(&cluster).await;
    let mut client = replica_client(replica_channel(&cluster, 1, Some(&identity)).await.unwrap());
    let reply = client
        .forward(Request::new(request_for("sit1", 1, command())))
        .await
        .unwrap()
        .into_inner();
    match hub_platform::gateway::wire::reply_from_wire(reply).unwrap() {
        AgentReply::Op(op) => assert_eq!(op.request_id.as_str(), "r-1"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(link.received(), vec![command()]);
    assert_eq!(
        cluster.hubs[1]
            .gateway_metrics
            .served_total(ServeOutcome::Answered),
        1
    );
}

#[tokio::test]
async fn replica_listener_refuses_agent_cert() {
    let Some((cluster, _link)) = with_agent().await else {
        return;
    };
    // A valid agent certificate, from the agent CA: not a certificate the replica listener trusts.
    let agent = cluster.agent("sit1").await;
    let replica_stats = &cluster.hubs[1].replica_stats;
    let before = replica_stats.handshakes_failed();
    let outcome = async {
        let mut client = replica_client(replica_channel(&cluster, 1, Some(&agent.identity)).await?);
        client
            .forward(Request::new(request_for("sit1", 1, command())))
            .await
            .map_err(|status| std::io::Error::other(status.to_string()))
    }
    .await;
    assert!(
        outcome.is_err(),
        "an agent certificate must not reach the replica service: {outcome:?}"
    );
    for _ in 0..100 {
        if replica_stats.handshakes_failed() > before {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the replica listener did not count a failed handshake");
}

#[tokio::test]
async fn replica_forward_requires_a_client_certificate() {
    let Some((cluster, link)) = with_agent().await else {
        return;
    };
    let mut client = replica_client(replica_channel(&cluster, 1, None).await.unwrap());
    let status = client
        .forward(Request::new(request_for("sit1", 1, command())))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::Unauthenticated);
    assert!(status.message().is_empty(), "no reason is given");
    assert!(link.received().is_empty(), "nothing reached the agent");
    assert_eq!(
        cluster.hubs[1].gateway_metrics.served_total(ServeOutcome::Denied),
        1
    );
}

#[tokio::test]
async fn replica_forward_requires_hub_san() {
    let Some((cluster, link)) = with_agent().await else {
        return;
    };
    // Certificates the replica CA signed, but for another kind of identity: the SAN decides, not the issuer.
    let agent_kind = identity_for(
        &cluster.replica_ca,
        &CertProfile::agent(&SwimlaneId::parse("sit1").unwrap()),
    )
    .await;
    let sentinel_kind = identity_for(
        &cluster.replica_ca,
        &CertProfile::sentinel(&PeerName::parse("nfs-vm-1").unwrap()),
    )
    .await;
    for (name, identity) in [("agent", &agent_kind), ("sentinel", &sentinel_kind)] {
        let mut client = replica_client(replica_channel(&cluster, 1, Some(identity)).await.unwrap());
        let status = client
            .forward(Request::new(request_for("sit1", 1, command())))
            .await
            .unwrap_err();
        assert_eq!(
            status.code(),
            Code::PermissionDenied,
            "a {name} SAN is not a replica"
        );
    }
    assert!(link.received().is_empty(), "nothing reached the agent");
}

#[tokio::test]
async fn replica_forward_with_an_epoch_that_is_not_the_streams_is_failed_precondition() {
    let Some((cluster, link)) = with_agent().await else {
        return;
    };
    let identity = hub_identity(&cluster).await;
    let mut client = replica_client(replica_channel(&cluster, 1, Some(&identity)).await.unwrap());
    for epoch in [2, 99] {
        let status = client
            .forward(Request::new(request_for("sit1", epoch, command())))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::FailedPrecondition, "epoch {epoch}");
    }
    // A swimlane this replica holds no stream for is the same answer.
    let status = client
        .forward(Request::new(request_for("sit9", 1, command())))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert!(link.received().is_empty());
    assert_eq!(
        cluster.hubs[1]
            .gateway_metrics
            .served_total(ServeOutcome::StaleEpoch),
        3
    );
}

#[tokio::test]
async fn replica_forward_refuses_requests_that_do_not_validate() {
    let Some((cluster, link)) = with_agent().await else {
        return;
    };
    let identity = hub_identity(&cluster).await;
    let mut client = replica_client(replica_channel(&cluster, 1, Some(&identity)).await.unwrap());
    let good = request_for("sit1", 1, command());
    let bad: Vec<(&str, pb::ForwardRequest)> = vec![
        (
            "empty swimlane",
            pb::ForwardRequest {
                swimlane_id: String::new(),
                ..good.clone()
            },
        ),
        (
            "path-like swimlane",
            pb::ForwardRequest {
                swimlane_id: "../sit1".into(),
                ..good.clone()
            },
        ),
        (
            "zero epoch",
            pb::ForwardRequest {
                epoch: 0,
                ..good.clone()
            },
        ),
        (
            "zero timeout",
            pb::ForwardRequest {
                timeout_ms: 0,
                ..good.clone()
            },
        ),
        (
            "no command",
            pb::ForwardRequest {
                command: None,
                ..good.clone()
            },
        ),
        (
            "an acknowledgement is not a command",
            pb::ForwardRequest {
                command: Some(proto::pb::HubMessage {
                    kind: Some(proto::pb::hub_message::Kind::Ack(proto::pb::Ack { seq: 1 })),
                }),
                ..good.clone()
            },
        ),
        (
            "a path that escapes the root",
            pb::ForwardRequest {
                command: Some(proto::pb::HubMessage {
                    kind: Some(proto::pb::hub_message::Kind::ReadFile(proto::pb::ReadFile {
                        request_id: "r-1".into(),
                        path: "../../etc/passwd".into(),
                    })),
                }),
                ..good.clone()
            },
        ),
    ];
    for (what, request) in bad {
        let status = client.forward(Request::new(request)).await.unwrap_err();
        assert_eq!(status.code(), Code::InvalidArgument, "{what}");
        assert!(status.message().is_empty(), "{what}: no reason is given");
    }
    assert!(link.received().is_empty(), "nothing reached the agent");
    assert_eq!(
        cluster.hubs[1]
            .gateway_metrics
            .served_total(ServeOutcome::Rejected),
        7
    );
}

#[tokio::test]
async fn replica_forward_refuses_a_message_larger_than_the_limit_before_decoding_it() {
    let Some((cluster, link)) = with_agent().await else {
        return;
    };
    let identity = hub_identity(&cluster).await;
    let mut client = replica_client(replica_channel(&cluster, 1, Some(&identity)).await.unwrap());
    let huge = HubCommand::WriteFile {
        request_id: RequestId::parse("r-big").unwrap(),
        path: NfsPath::parse("app/a.yml").unwrap(),
        expected: Expected::Absent,
        bytes: Bytes::from(vec![b'x'; 3 * 1024 * 1024]),
    };
    // Within the contract's limit: accepted and delivered.
    let ok = client.forward(Request::new(request_for("sit1", 1, huge))).await;
    assert!(ok.is_ok(), "{ok:?}");
    // Over it: a request that is not a valid message at all (5 MiB of one field).
    let mut oversized = request_for("sit1", 1, command());
    oversized.swimlane_id = "s".repeat(5 * 1024 * 1024);
    let refused = client.forward(Request::new(oversized)).await.unwrap_err();
    assert_eq!(
        refused.code(),
        Code::OutOfRange,
        "the transport's size limit, before any of the message is decoded"
    );
    assert_eq!(link.received().len(), 1, "only the first reached the agent");
}

#[tokio::test]
async fn the_agent_listener_refuses_a_replica_certificate() {
    let Some((cluster, _link)) = with_agent().await else {
        return;
    };
    let identity = hub_identity(&cluster).await;
    // The agent listener trusts the agent CA: a replica certificate does not complete a handshake there either.
    let roots = roots_from_der(&[cluster.agent_ca.ca_der.clone()]).unwrap();
    let tls = ClientTls::new(roots, Some(identity.certified()), cluster.agent_ca.clock.clone()).unwrap();
    let before = cluster.hubs[1].replica_stats.handshakes_failed();
    let result = async {
        let (h2, _) = dial_named(
            cluster.hubs[1].agent_addr,
            tls.connector(),
            support::grpc::HUB_NAME,
        )
        .await?;
        let mut client = proto::grpc::agent_client(h2);
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        client
            .connect(Request::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
            .await
            .map(|_| ())
            .map_err(|status| std::io::Error::other(status.to_string()))
    }
    .await;
    assert!(
        result.is_err(),
        "a replica certificate must not open an agent stream: {result:?}"
    );
    assert_eq!(
        cluster.hubs[1].replica_stats.handshakes_failed(),
        before,
        "the replica listener was not involved"
    );
}
