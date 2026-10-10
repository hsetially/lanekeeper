//! The line between wire messages and typed ones (S11).
//!
//! Every message from the hub is turned into a [`proto::convert::ToAgent`] here, before anything else looks at it.
//! The generated messages (`proto::pb`) are used only in this directory; the rest of the agent sees validated domain
//! values, so a path that is not an `NfsPath` or a hash that is not 32 bytes never gets as far as a handler.

use domain::{AgentReply, OpError, OpResult, RequestId};
use proto::convert::{ConvertError, FromAgent, ToAgent};
use proto::pb;

/// What one message from the hub turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Message(ToAgent),
    /// An empty message, or one from a newer hub that this build does not know. Skipped; the stream stays open.
    Unknown,
    /// The message broke a rule. `problem` names the field and never the value. When the message was a command whose
    /// request id could be read, `request_id` is set, so the hub gets an answer instead of waiting for a timeout.
    Invalid {
        problem: ConvertError,
        request_id: Option<RequestId>,
    },
}

/// Validate one message from the hub.
pub fn decode(message: pb::HubMessage) -> Decoded {
    // Read the id before the message is consumed. Only commands have one, and a command is rare, so the copy is cheap.
    let raw_id = command_request_id(&message).map(str::to_owned);
    match ToAgent::from_proto(message) {
        Ok(Some(decoded)) => Decoded::Message(decoded),
        Ok(None) => Decoded::Unknown,
        Err(problem) => Decoded::Invalid {
            problem,
            request_id: raw_id.and_then(|id| RequestId::parse(&id).ok()),
        },
    }
}

/// The request id of a command, as the hub wrote it, before validation.
fn command_request_id(message: &pb::HubMessage) -> Option<&str> {
    use pb::hub_message::Kind;
    match message.kind.as_ref()? {
        Kind::ReadFile(c) => Some(&c.request_id),
        Kind::WriteFile(c) => Some(&c.request_id),
        Kind::DeleteFile(c) => Some(&c.request_id),
        Kind::RestartDeployment(c) => Some(&c.request_id),
        Kind::RequestClusterReport(c) => Some(&c.request_id),
        Kind::NotifyConfigServer(c) => Some(&c.request_id),
        Kind::FetchServed(c) => Some(&c.request_id),
        Kind::AgentConfig(_)
        | Kind::RequestDelta(_)
        | Kind::RequestFullScan(_)
        | Kind::CertRenewalResponse(_)
        | Kind::Ack(_) => None,
    }
}

/// The answer to a command that failed validation: refused, with no detail. The detail goes to the log.
pub fn refusal(request_id: RequestId) -> FromAgent {
    FromAgent::Reply(AgentReply::Op(OpResult {
        request_id,
        ok: false,
        error: Some(OpError::Denied),
        current_hash: None,
    }))
}

/// A message for the hub, as the generated type the transport sends.
pub fn encode(message: FromAgent) -> pb::AgentMessage {
    message.into_proto()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use proptest::prelude::*;
    use prost::Message;

    use super::*;

    fn hub(kind: pb::hub_message::Kind) -> pb::HubMessage {
        pb::HubMessage { kind: Some(kind) }
    }

    fn write_file(request_id: &str, path: &str) -> pb::HubMessage {
        hub(pb::hub_message::Kind::WriteFile(pb::WriteFile {
            request_id: request_id.to_owned(),
            path: path.to_owned(),
            expected: Some(pb::Expected {
                state: Some(pb::expected::State::Absent(pb::expected::Absent {})),
            }),
            content: Bytes::from_static(b"x"),
        }))
    }

    #[test]
    fn a_valid_message_is_decoded() {
        let decoded = decode(hub(pb::hub_message::Kind::Ack(pb::Ack { seq: 7 })));
        assert_eq!(decoded, Decoded::Message(ToAgent::Ack(7)));
    }

    #[test]
    fn an_empty_or_unknown_message_is_skipped_not_refused() {
        assert_eq!(decode(pb::HubMessage { kind: None }), Decoded::Unknown);
    }

    #[test]
    fn a_command_with_a_hostile_path_is_invalid_and_keeps_its_request_id() {
        for path in ["../etc/passwd", "/abs", "a/../b", "a\0b", "", "a\\b", "a/./b/.."] {
            let Decoded::Invalid { problem, request_id } = decode(write_file("req-1", path)) else {
                panic!("{path:?} must be refused");
            };
            assert_eq!(problem, ConvertError::Invalid("write_file.path"), "{path:?}");
            assert_eq!(request_id.as_ref().map(RequestId::as_str), Some("req-1"));
        }
    }

    #[test]
    fn an_unreadable_request_id_gets_no_answer() {
        for id in ["", "has space", &"x".repeat(1000)] {
            let Decoded::Invalid { request_id, .. } = decode(write_file(id, "../x")) else {
                panic!("must be refused");
            };
            assert_eq!(request_id, None, "{id:?}");
        }
    }

    #[test]
    fn every_command_kind_keeps_its_request_id() {
        use pb::hub_message::Kind;
        let id = "req-9".to_owned();
        let commands = [
            Kind::ReadFile(pb::ReadFile {
                request_id: id.clone(),
                path: "../x".into(),
            }),
            Kind::DeleteFile(pb::DeleteFile {
                request_id: id.clone(),
                path: "../x".into(),
                expected_hash: Bytes::from_static(b"short"),
            }),
            Kind::RestartDeployment(pb::RestartDeployment {
                request_id: id.clone(),
                service: None,
            }),
            Kind::RequestClusterReport(pb::RequestClusterReport {
                request_id: String::new(),
            }),
            Kind::NotifyConfigServer(pb::NotifyConfigServer {
                request_id: id.clone(),
                paths: vec!["../x".into()],
            }),
            Kind::FetchServed(pb::FetchServed {
                request_id: id.clone(),
                application: "../x".into(),
                tenant: "sit1".into(),
                channel: None,
                file: "a.yml".into(),
            }),
        ];
        let seen: Vec<Option<String>> = commands
            .into_iter()
            .map(|kind| match decode(hub(kind)) {
                Decoded::Invalid { request_id, .. } => request_id.map(|r| r.as_str().to_owned()),
                other => panic!("must be refused: {other:?}"),
            })
            .collect();
        // The cluster-report request had an empty id, which is not a request id.
        assert_eq!(
            seen,
            vec![
                Some(id.clone()),
                Some(id.clone()),
                Some(id.clone()),
                None,
                Some(id.clone()),
                Some(id)
            ]
        );
    }

    #[test]
    fn a_broken_config_has_no_request_id_to_answer() {
        let message = hub(pb::hub_message::Kind::AgentConfig(pb::AgentConfig {
            scan_interval_secs: 0,
            heartbeat_interval_secs: 10,
            ..pb::AgentConfig::default()
        }));
        assert_eq!(
            decode(message),
            Decoded::Invalid {
                problem: ConvertError::Invalid("agent_config.scan_interval_secs"),
                request_id: None
            }
        );
    }

    #[test]
    fn a_too_long_notify_is_refused_with_its_request_id() {
        let message = hub(pb::hub_message::Kind::NotifyConfigServer(
            pb::NotifyConfigServer {
                request_id: "req-2".into(),
                paths: (0..=proto::limits::MAX_NOTIFY_PATHS)
                    .map(|i| format!("a/{i}.yml"))
                    .collect(),
            },
        ));
        let Decoded::Invalid { problem, request_id } = decode(message) else {
            panic!("must be refused");
        };
        assert_eq!(problem, ConvertError::TooLarge("notify_config_server.paths"));
        assert_eq!(
            request_id.map(|r| r.as_str().to_owned()),
            Some("req-2".to_owned())
        );
    }

    #[test]
    fn the_refusal_is_a_denied_op_result_with_no_detail() {
        let id = RequestId::parse("req-3").unwrap();
        let wire = encode(refusal(id.clone()));
        let Some(pb::agent_message::Kind::OpResult(result)) = wire.kind else {
            panic!("an OpResult");
        };
        assert_eq!(result.request_id, "req-3");
        assert!(!result.ok);
        assert_eq!(result.error_code, pb::OpErrorCode::Denied as i32);
        assert_eq!(result.current_hash, None);
        // And the hub's own conversion accepts it.
        let back = FromAgent::from_proto(encode(refusal(id))).unwrap().unwrap();
        assert!(
            matches!(back, FromAgent::Reply(AgentReply::Op(r)) if !r.ok && r.error == Some(OpError::Denied))
        );
    }

    proptest! {
        /// S11: whatever bytes arrive, `decode` returns; it never panics and never allocates from a length it has not
        /// checked (the conversions bound every list and every byte string).
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
            if let Ok(message) = pb::HubMessage::decode(bytes.as_slice()) {
                let _ = decode(message);
            }
        }
    }
}
