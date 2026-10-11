//! Metrics of the agent endpoint (S10): joins, streams, messages and what the sink refused.
//!
//! Every label comes from a closed set, so the number of series is fixed. A swimlane id, a path, a token or a status text is
//! never a label (swimlane ids appear in logs only).

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

/// How a `Join` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum JoinOutcome {
    Issued,
    Refused,
    RateLimited,
    Unavailable,
}

/// Which credential a join carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum CredentialKind {
    GoogleIdToken,
    JoinToken,
}

/// The kind of message an agent sent on its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum MessageKind {
    Hello,
    Heartbeat,
    ScanDelta,
    ClusterReport,
    Reply,
    CertRenewal,
}

/// Why a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum StreamEnd {
    /// The agent closed it.
    AgentClosed,
    /// The first message was not a valid `Hello`, or named another swimlane.
    BadHello,
    /// Nothing arrived for too long.
    Idle,
    /// A message was malformed or broke the protocol.
    Protocol,
    /// The agent stopped reading what the hub sent.
    Stalled,
    /// The hub is shutting down.
    HubClosing,
    /// Replaced, or closed from the hub's side for another reason.
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct JoinLabels {
    credential: CredentialKind,
    outcome: JoinOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct MessageLabels {
    kind: MessageKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct EndLabels {
    reason: StreamEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct SinkLabels {
    operation: MessageKind,
}

/// Cloning shares the counters.
#[derive(Debug, Clone, Default)]
pub struct AgentMetrics {
    joins: Family<JoinLabels, Counter>,
    streams_active: Gauge,
    messages: Family<MessageLabels, Counter>,
    stream_ends: Family<EndLabels, Counter>,
    unmatched_replies: Counter,
    sink_failures: Family<SinkLabels, Counter>,
    renewals_throttled: Counter,
}

impl AgentMetrics {
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "agent_joins",
            "Agent joins by credential and outcome",
            self.joins.clone(),
        );
        registry.register(
            "agent_streams_active",
            "Agent Connect streams open on this replica",
            self.streams_active.clone(),
        );
        registry.register(
            "agent_messages",
            "Messages received from agents by kind",
            self.messages.clone(),
        );
        registry.register(
            "agent_stream_ends",
            "Agent streams that ended by reason",
            self.stream_ends.clone(),
        );
        registry.register(
            "agent_unmatched_replies",
            "Replies from agents that no request was waiting for",
            self.unmatched_replies.clone(),
        );
        registry.register(
            "agent_sink_failures",
            "Messages the ingestion sink refused or failed on, by kind",
            self.sink_failures.clone(),
        );
        registry.register(
            "agent_renewals_throttled",
            "Certificate renewals ignored because the last one was too recent",
            self.renewals_throttled.clone(),
        );
    }

    pub fn join(&self, credential: CredentialKind, outcome: JoinOutcome) {
        self.joins
            .get_or_create(&JoinLabels { credential, outcome })
            .inc();
    }

    pub fn stream_opened(&self) {
        self.streams_active.inc();
    }

    pub fn stream_closed(&self, reason: StreamEnd) {
        self.streams_active.dec();
        self.stream_ends.get_or_create(&EndLabels { reason }).inc();
    }

    pub fn message(&self, kind: MessageKind) {
        self.messages.get_or_create(&MessageLabels { kind }).inc();
    }

    pub fn unmatched_reply(&self) {
        self.unmatched_replies.inc();
    }

    pub fn sink_failure(&self, operation: MessageKind) {
        self.sink_failures.get_or_create(&SinkLabels { operation }).inc();
    }

    pub fn renewal_throttled(&self) {
        self.renewals_throttled.inc();
    }

    #[must_use]
    pub fn streams_active(&self) -> i64 {
        self.streams_active.get()
    }

    #[must_use]
    pub fn unmatched_replies(&self) -> u64 {
        self.unmatched_replies.get()
    }
}
