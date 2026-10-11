//! T4: the hub-to-hub messages and the replica address (S11, S22). No database and no network.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::time::Duration;

use bytes::Bytes;
use domain::{AgentReply, Expected, HubCommand, NfsPath, OpResult, RequestId, ServiceRef, SwimlaneId};
use hub_platform::gateway::forward::{AddressError, PeerAddress};
use hub_platform::gateway::local::Epoch;
use hub_platform::gateway::wire::{
    ForwardCall, ForwardRejected, MAX_FORWARD_TIMEOUT, error_from_wire, error_to_wire, pb, reply_from_wire,
    reply_to_wire,
};
use ports::GatewayError;
use proptest::prelude::*;
use prost::Message;
use proto::convert::ToAgent;
use proto::pb as agent_pb;

fn rid(s: &str) -> RequestId {
    RequestId::parse(s).unwrap()
}

fn path(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn call(command: HubCommand) -> ForwardCall {
    ForwardCall {
        swimlane: SwimlaneId::parse("sit1").unwrap(),
        epoch: Epoch::new(7),
        timeout: Duration::from_millis(2500),
        command,
    }
}

fn every_command() -> Vec<HubCommand> {
    vec![
        HubCommand::RequestDelta {
            since_root: ports::content_hash(b"root"),
        },
        HubCommand::RequestFullScan,
        HubCommand::ReadFile {
            request_id: rid("r-1"),
            path: path("app/a.yml"),
        },
        HubCommand::WriteFile {
            request_id: rid("r-2"),
            path: path("app/b.yml"),
            expected: Expected::Hash {
                hash: ports::content_hash(b"old"),
            },
            bytes: Bytes::from_static(b"new: 1\n"),
        },
        HubCommand::WriteFile {
            request_id: rid("r-3"),
            path: path("app/c.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from_static(b"x"),
        },
        HubCommand::DeleteFile {
            request_id: rid("r-4"),
            path: path("app/d.yml"),
            expected: ports::content_hash(b"d"),
        },
        HubCommand::RestartDeployment {
            request_id: rid("r-5"),
            service: ServiceRef::new("ns", "svc").unwrap(),
        },
        HubCommand::RequestClusterReport {
            request_id: rid("r-6"),
        },
        HubCommand::NotifyConfigServer {
            request_id: rid("r-7"),
            paths: vec![path("app/a.yml"), path("app/b.yml")],
        },
    ]
}

#[test]
fn a_forward_call_survives_the_wire_for_every_command() {
    for command in every_command() {
        let original = call(command);
        let decoded = ForwardCall::try_from(original.clone().into_proto()).unwrap();
        assert_eq!(decoded, original);
        // And through the bytes, as it travels.
        let bytes = original.clone().into_proto().encode_to_vec();
        let decoded = ForwardCall::try_from(pb::ForwardRequest::decode(bytes.as_slice()).unwrap()).unwrap();
        assert_eq!(decoded, original);
    }
}

fn valid_request() -> pb::ForwardRequest {
    call(HubCommand::ReadFile {
        request_id: rid("r-1"),
        path: path("a.yml"),
    })
    .into_proto()
}

#[test]
fn a_forward_request_that_does_not_validate_is_refused() {
    let refuse = |edit: &dyn Fn(&mut pb::ForwardRequest)| {
        let mut request = valid_request();
        edit(&mut request);
        ForwardCall::try_from(request).unwrap_err()
    };
    assert_eq!(
        refuse(&|r| r.swimlane_id = String::new()),
        ForwardRejected::Swimlane
    );
    assert_eq!(
        refuse(&|r| r.swimlane_id = "../other".into()),
        ForwardRejected::Swimlane
    );
    assert_eq!(
        refuse(&|r| r.swimlane_id = "a".repeat(200)),
        ForwardRejected::Swimlane
    );
    assert_eq!(refuse(&|r| r.epoch = 0), ForwardRejected::Epoch);
    assert_eq!(refuse(&|r| r.epoch = u64::MAX), ForwardRejected::Epoch);
    assert_eq!(refuse(&|r| r.timeout_ms = 0), ForwardRejected::Timeout);
    assert_eq!(refuse(&|r| r.command = None), ForwardRejected::NoCommand);
    // An empty oneof, or a variant this build does not know: not a command.
    assert_eq!(
        refuse(&|r| r.command = Some(agent_pb::HubMessage { kind: None })),
        ForwardRejected::NotACommand
    );
    // Stream-only messages are not commands a peer may send.
    assert_eq!(
        refuse(&|r| r.command = Some(agent_pb::HubMessage {
            kind: Some(agent_pb::hub_message::Kind::Ack(agent_pb::Ack { seq: 1 }))
        })),
        ForwardRejected::NotACommand
    );
    let config = domain::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 5,
        max_file_bytes: 1_000_000,
        deny_globs: Vec::new(),
        env_allowlist: Vec::new(),
        tenants: Vec::new(),
    };
    assert_eq!(
        refuse(&|r| r.command = Some(ToAgent::Config(config.clone()).into_proto())),
        ForwardRejected::NotACommand,
        "a valid configuration message is still not a command"
    );
    assert_eq!(
        refuse(&|r| r.command = Some(agent_pb::HubMessage {
            kind: Some(agent_pb::hub_message::Kind::AgentConfig(
                agent_pb::AgentConfig::default()
            ))
        })),
        ForwardRejected::Command,
        "and an invalid one fails the contract's validation first"
    );
    // A command that fails the contract's own validation (a path that escapes the root).
    assert_eq!(
        refuse(&|r| r.command = Some(agent_pb::HubMessage {
            kind: Some(agent_pb::hub_message::Kind::ReadFile(agent_pb::ReadFile {
                request_id: "r-1".into(),
                path: "../../etc/passwd".into(),
            }))
        })),
        ForwardRejected::Command
    );
}

#[test]
fn the_timeout_a_peer_asks_for_is_capped() {
    let mut request = valid_request();
    request.timeout_ms = u32::MAX;
    assert_eq!(
        ForwardCall::try_from(request).unwrap().timeout,
        MAX_FORWARD_TIMEOUT
    );
}

#[test]
fn gateway_errors_round_trip_and_an_unknown_code_is_unavailable() {
    for error in [
        GatewayError::NotConnected,
        GatewayError::Timeout,
        GatewayError::Busy,
        GatewayError::Closed,
        GatewayError::Unavailable,
    ] {
        assert_eq!(error_from_wire(error_to_wire(error).into()), error);
    }
    assert_eq!(error_from_wire(0), GatewayError::Unavailable, "unspecified");
    assert_eq!(
        error_from_wire(99),
        GatewayError::Unavailable,
        "a code from a newer build"
    );
    assert_eq!(error_from_wire(-1), GatewayError::Unavailable);
}

#[test]
fn a_reply_survives_the_wire_and_a_reply_that_is_not_one_is_refused() {
    let reply = AgentReply::Op(OpResult {
        request_id: rid("r-1"),
        ok: false,
        error: Some(domain::OpError::Conflict),
        current_hash: Some(ports::content_hash(b"current")),
    });
    assert_eq!(reply_from_wire(reply_to_wire(reply.clone())), Ok(reply));

    let heartbeat = pb::ForwardReply {
        result: Some(pb::forward_reply::Result::Reply(agent_pb::AgentMessage {
            kind: Some(agent_pb::agent_message::Kind::Heartbeat(agent_pb::Heartbeat {
                scan_seq: 1,
                merkle_root: Bytes::from(vec![0u8; 32]),
                file_count: 0,
            })),
        })),
    };
    assert_eq!(reply_from_wire(heartbeat), Err(GatewayError::Unavailable));
    assert_eq!(
        reply_from_wire(pb::ForwardReply { result: None }),
        Err(GatewayError::Unavailable)
    );
    let invalid = pb::ForwardReply {
        result: Some(pb::forward_reply::Result::Reply(agent_pb::AgentMessage {
            kind: Some(agent_pb::agent_message::Kind::OpResult(agent_pb::OpResult {
                request_id: "r-1".into(),
                ok: true,
                error_code: 77,
                current_hash: None,
            })),
        })),
    };
    assert_eq!(reply_from_wire(invalid), Err(GatewayError::Unavailable));
}

#[test]
fn a_peer_address_is_a_dns_name_and_a_port() {
    let ok = PeerAddress::parse("hub-1.hub-headless.lanekeeper.svc.cluster.local:8444").unwrap();
    assert_eq!(ok.host(), "hub-1.hub-headless.lanekeeper.svc.cluster.local");
    assert_eq!(ok.port(), 8444);
    assert_eq!(
        ok.as_str(),
        "hub-1.hub-headless.lanekeeper.svc.cluster.local:8444"
    );
    assert!(PeerAddress::parse("localhost:1").is_ok());

    let refuse = |s: &str| PeerAddress::parse(s).unwrap_err();
    assert_eq!(refuse(""), AddressError::Shape);
    assert_eq!(refuse("hub-1"), AddressError::Shape);
    assert_eq!(refuse(":8444"), AddressError::Host);
    assert_eq!(refuse("hub-1:"), AddressError::Port);
    assert_eq!(refuse("hub-1:0"), AddressError::Port);
    assert_eq!(refuse("hub-1:65536"), AddressError::Port);
    assert_eq!(refuse("hub-1:+80"), AddressError::Port);
    assert_eq!(refuse("hub-1:080"), AddressError::Port);
    assert_eq!(refuse("-hub:80"), AddressError::Host);
    assert_eq!(refuse("hub-.x:80"), AddressError::Host);
    assert_eq!(refuse("hub..x:80"), AddressError::Host);
    assert_eq!(refuse("hub_1:80"), AddressError::Host);
    assert_eq!(refuse("user@hub-1:80"), AddressError::Host);
    assert_eq!(refuse("hub-1/path:80"), AddressError::Host);
    assert_eq!(refuse("[::1]:80"), AddressError::Host);
    assert_eq!(refuse("https://hub-1:80"), AddressError::Host);
    assert_eq!(refuse(&format!("{}:80", "a".repeat(64))), AddressError::Host);
    assert_eq!(refuse(&format!("{}:80", "a.".repeat(200))), AddressError::Shape);
}

proptest! {
    /// Whatever bytes a peer sends, decoding and validating them never panics, and what is accepted is well formed.
    #[test]
    fn forward_request_decoding_never_panics_and_accepts_only_well_formed_calls(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        if let Ok(request) = pb::ForwardRequest::decode(bytes.as_slice())
            && let Ok(call) = ForwardCall::try_from(request)
        {
            prop_assert!(call.epoch.get() > 0);
            prop_assert!(!call.timeout.is_zero() && call.timeout <= MAX_FORWARD_TIMEOUT);
            prop_assert!(!call.swimlane.as_str().is_empty());
        }
    }

    #[test]
    fn peer_addresses_that_parse_dial_only_what_they_say(host in "[a-z0-9.-]{0,40}", port in 0u32..70000) {
        let text = format!("{host}:{port}");
        if let Ok(address) = PeerAddress::parse(&text) {
            prop_assert_eq!(address.as_str(), text.as_str());
            prop_assert_eq!(address.host(), host.as_str());
            prop_assert_eq!(u32::from(address.port()), port);
            prop_assert!(!address.host().starts_with('-') && !address.host().contains(".."));
        }
    }
}
