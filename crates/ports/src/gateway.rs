//! The agent gateway port: send a command to the agent of a swimlane, wherever its stream lives (D62).

use std::time::Duration;

use async_trait::async_trait;
use domain::{AgentReply, AgentStatus, HubCommand, SwimlaneId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GatewayError {
    /// No agent stream for the swimlane is open on any replica.
    #[error("agent is not connected")]
    NotConnected,
    /// The agent did not answer within the timeout. The command may still run (idempotent by request id).
    #[error("agent did not answer in time")]
    Timeout,
    /// The agent's command queue is full (every queue is bounded).
    #[error("agent is busy")]
    Busy,
    /// The stream closed while the command was in flight.
    #[error("agent connection closed")]
    Closed,
    #[error("agent gateway is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait AgentGateway: Send + Sync + 'static {
    /// Send `cmd` and wait for the agent's reply, at most `timeout`. Routed to whichever replica holds the
    /// agent's stream. A zero timeout fails with [`GatewayError::Timeout`] without sending.
    ///
    /// An agent-side refusal (for example a hash conflict) is a successful call that returns
    /// [`AgentReply::Op`] with `ok = false`, not a [`GatewayError`].
    async fn request(
        &self,
        s: &SwimlaneId,
        cmd: HubCommand,
        timeout: Duration,
    ) -> Result<AgentReply, GatewayError>;

    async fn status(&self, s: &SwimlaneId) -> AgentStatus;
}
