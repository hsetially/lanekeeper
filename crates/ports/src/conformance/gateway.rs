use std::time::Duration;

use async_trait::async_trait;
use domain::{AgentReply, AgentStatus, HubCommand, SwimlaneId};

use super::sample::{hash, nfs, request_id, swimlane};
use crate::{AgentGateway, GatewayError};

/// What the gateway conformance suite needs from the test harness: agents that behave in a known way.
#[async_trait]
pub trait AgentGatewayScenario: AgentGateway {
    /// Connect an agent for `s` that acknowledges every command with `AgentReply::Op { ok: true, .. }`,
    /// keeping the command's `request_id`.
    async fn attach_echo_agent(&self, s: &SwimlaneId);
    /// Connect an agent that never answers.
    async fn attach_silent_agent(&self, s: &SwimlaneId);
    /// Close the agent's stream.
    async fn detach_agent(&self, s: &SwimlaneId);
}

fn cluster_cmd(id: &str) -> HubCommand {
    HubCommand::RequestClusterReport {
        request_id: request_id(id),
    }
}

/// - No agent ever seen: status `NeverSeen`, requests fail with `NotConnected`.
/// - A connected agent answers, and the reply carries the request id.
/// - A zero timeout, or an agent that stays silent, fails with `Timeout`; one silent agent does not block
///   another swimlane.
/// - After the stream closes the status is `Disconnected` and requests fail with `NotConnected`.
pub async fn agent_gateway<G: AgentGatewayScenario + ?Sized>(g: &G) {
    let s1 = swimlane("sit1");
    let s2 = swimlane("sit2");
    let second = Duration::from_secs(1);

    assert_eq!(g.status(&s1).await, AgentStatus::NeverSeen, "unknown swimlane");
    assert_eq!(
        g.request(&s1, cluster_cmd("r-0"), second).await,
        Err(GatewayError::NotConnected),
        "request to a swimlane with no agent"
    );

    g.attach_echo_agent(&s1).await;
    assert_eq!(g.status(&s1).await, AgentStatus::Connected);
    let reply = g
        .request(
            &s1,
            HubCommand::DeleteFile {
                request_id: request_id("r-1"),
                path: nfs("a/b.yml"),
                expected: hash(b"x"),
            },
            second,
        )
        .await
        .expect("a connected agent answers");
    match reply {
        AgentReply::Op(op) => {
            assert_eq!(op.request_id, request_id("r-1"), "reply keeps the request id");
            assert!(op.ok);
        }
        other => panic!("expected an Op reply, got {other:?}"),
    }

    assert_eq!(
        g.request(&s1, cluster_cmd("r-2"), Duration::ZERO).await,
        Err(GatewayError::Timeout),
        "a zero timeout fails at once"
    );

    g.attach_silent_agent(&s2).await;
    assert_eq!(
        g.request(&s2, cluster_cmd("r-3"), Duration::from_millis(50))
            .await,
        Err(GatewayError::Timeout),
        "a silent agent times out"
    );
    assert!(
        g.request(&s1, cluster_cmd("r-4"), second).await.is_ok(),
        "a silent agent on sit2 does not affect sit1"
    );

    g.detach_agent(&s1).await;
    assert_eq!(g.status(&s1).await, AgentStatus::Disconnected);
    assert_eq!(
        g.request(&s1, cluster_cmd("r-5"), second).await,
        Err(GatewayError::NotConnected),
        "request after the stream closed"
    );
}
