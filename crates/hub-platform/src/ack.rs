//! Acknowledging scan deltas (T11, D74, P15): `Ack{seq}` goes to an agent only after `ReportSink::delta` returned `Ok` for that
//! message, in the order the deltas arrived, once for each message the sink applied.
//!
//! The agent keeps every version in its durable spool until the hub acknowledges it, so an acknowledgement that came before the
//! sink committed would let the agent delete a version the hub then lost. Nothing else is promised by a missing ack: the agent
//! keeps the version and sends it again.
//!
//! The ordering is structural. [`AckWriter::apply`] is the only way to send an ack, and it sends one only as its last step, after
//! the sink call it wraps returned `Ok`; the stream handler awaits each `apply` before it reads the next message, so the acks
//! leave in the order the deltas arrived. A message of a multi-part delta is acknowledged as the sink accepts that message, which
//! is what the sink's contract says (the sink applies the whole delta on the part with `more = false`).

use std::sync::Arc;
use std::time::Duration;

use domain::ScanDelta;
use ports::{AgentIdentity, ReportSink, SinkError};
use proto::convert::ToAgent;

use crate::agent_metrics::AgentMetrics;
use crate::stream::{ConnectedAgent, SendError};

/// What became of one scan delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The sink applied it and the ack is queued for the agent.
    Acked,
    /// The sink refused it or did not answer in time: no ack, and the agent resends.
    Refused(SinkError),
    /// The sink applied it, but the agent's stream cannot take the ack (it ended, or the agent stopped reading).
    StreamLost(SendError),
}

/// Sends acknowledgements on one agent's stream.
#[derive(Debug)]
pub struct AckWriter {
    agent: Arc<ConnectedAgent>,
    send_timeout: Duration,
    metrics: AgentMetrics,
}

impl AckWriter {
    #[must_use]
    pub fn new(agent: Arc<ConnectedAgent>, send_timeout: Duration, metrics: AgentMetrics) -> Self {
        Self {
            agent,
            send_timeout,
            metrics,
        }
    }

    /// Hand `delta` to the sink, and acknowledge it if, and only after, the sink returned `Ok` within `sink_timeout`.
    pub async fn apply(
        &self,
        sink: &dyn ReportSink,
        identity: &AgentIdentity,
        delta: ScanDelta,
        sink_timeout: Duration,
    ) -> Applied {
        let seq = delta.seq;
        match tokio::time::timeout(sink_timeout, sink.delta(identity, delta)).await {
            Err(_elapsed) => Applied::Refused(SinkError::Unavailable),
            Ok(Err(error)) => Applied::Refused(error),
            Ok(Ok(())) => match self.agent.send(ToAgent::Ack(seq), self.send_timeout).await {
                Ok(()) => {
                    self.metrics.ack_sent();
                    Applied::Acked
                }
                Err(error) => Applied::StreamLost(error),
            },
        }
    }
}
