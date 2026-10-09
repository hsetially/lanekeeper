//! Ingestion ports: what the hub does with what agents and sentinels send (implemented by 05).
//!
//! Every method takes the verified identity of the sender. An identity can only act for itself (S5), and
//! nothing in a payload can name another swimlane.

use async_trait::async_trait;
use domain::{
    AgentConfig, AuditRecordBatch, ClusterReport, Heartbeat, HeartbeatAction, Hello, ScanDelta,
    SentinelConfig, SentinelHello,
};

use crate::{AckSeq, AgentIdentity, SentinelIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// `hello` has not been accepted on this stream yet.
    #[error("hello is required first")]
    HelloRequired,
    /// The payload names a swimlane other than the sender's own.
    #[error("payload does not match the sender identity")]
    IdentityMismatch,
    /// The payload is well formed but not acceptable (for example a denied file carrying bytes, or bytes
    /// whose hash differs from the declared hash).
    #[error("payload is not valid")]
    Invalid,
    #[error("payload is too large")]
    TooLarge,
    #[error("ingestion is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait ReportSink: Send + Sync + 'static {
    /// First message of a stream. `h.swimlane` must equal the identity's swimlane.
    async fn hello(&self, id: &AgentIdentity, h: Hello) -> Result<AgentConfig, SinkError>;

    /// Tells the hub the agent's Merkle root. The answer says whether the hub needs a delta: the first
    /// heartbeat after `hello` asks for a full scan, an unchanged root asks for nothing.
    async fn heartbeat(&self, id: &AgentIdentity, hb: Heartbeat) -> Result<HeartbeatAction, SinkError>;

    /// One message of a scan delta. The hub applies the whole delta only when it receives the message with
    /// `more == false`; a stream that ends before that applies nothing.
    async fn delta(&self, id: &AgentIdentity, d: ScanDelta) -> Result<(), SinkError>;

    async fn cluster(&self, id: &AgentIdentity, c: ClusterReport) -> Result<(), SinkError>;
}

#[async_trait]
pub trait SentinelSink: Send + Sync + 'static {
    async fn hello(&self, id: &SentinelIdentity, h: SentinelHello) -> Result<SentinelConfig, SinkError>;

    /// Durably applies a spooled batch and acknowledges it (D74). Batches are at-least-once: a batch whose
    /// `seq` was already acknowledged is not applied again and is acknowledged with the highest `seq` so
    /// far. A batch larger than the configured `max_batch_records` is [`SinkError::TooLarge`].
    async fn records(&self, id: &SentinelIdentity, batch: AuditRecordBatch) -> Result<AckSeq, SinkError>;
}
