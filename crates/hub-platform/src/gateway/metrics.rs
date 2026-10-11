//! Metrics of the gateway (S10): requests, forwards and superseded streams.
//!
//! Every label comes from a closed set. A swimlane id, a request id or a replica address is never a label (they appear in
//! logs only), so the number of series is fixed.

use ports::GatewayError;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

/// How a request reached its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RequestPath {
    /// The stream is on this replica.
    Local,
    /// The stream is on another replica.
    Forwarded,
    /// The request failed before it was routed (no agent, registry down).
    Unrouted,
}

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RequestOutcome {
    Replied,
    NotConnected,
    Timeout,
    Busy,
    Closed,
    Unavailable,
}

impl From<GatewayError> for RequestOutcome {
    fn from(error: GatewayError) -> Self {
        match error {
            GatewayError::NotConnected => Self::NotConnected,
            GatewayError::Timeout => Self::Timeout,
            GatewayError::Busy => Self::Busy,
            GatewayError::Closed => Self::Closed,
            GatewayError::Unavailable => Self::Unavailable,
        }
    }
}

/// What a forward to a peer came to, on the calling side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ForwardOutcome {
    Replied,
    /// The owner refused the epoch: the agent had moved, and the caller re-read the registry.
    StaleEpoch,
    Failed,
}

/// What a forward from a peer came to, on the serving side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ServeOutcome {
    /// The agent answered, or the stream reported a typed failure.
    Answered,
    /// The caller's epoch is not the epoch of the stream held here, or no stream is held.
    StaleEpoch,
    /// The request did not validate.
    Rejected,
    /// The caller is not a hub replica.
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RequestLabels {
    path: RequestPath,
    outcome: RequestOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ForwardLabels {
    outcome: ForwardOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ServeLabels {
    outcome: ServeOutcome,
}

/// Cloning shares the counters.
#[derive(Debug, Clone, Default)]
pub struct GatewayMetrics {
    requests: Family<RequestLabels, Counter>,
    forwards: Family<ForwardLabels, Counter>,
    served: Family<ServeLabels, Counter>,
    superseded: Counter,
    local_streams: Gauge,
}

impl GatewayMetrics {
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "gateway_requests",
            "Commands sent to agents, by how they were routed and how they ended",
            self.requests.clone(),
        );
        registry.register(
            "gateway_forwards",
            "Commands forwarded to another replica, by outcome",
            self.forwards.clone(),
        );
        registry.register(
            "gateway_forwards_served",
            "Forwarded commands received from other replicas, by outcome",
            self.served.clone(),
        );
        registry.register(
            "gateway_streams_superseded",
            "Agent streams closed because the agent connected elsewhere",
            self.superseded.clone(),
        );
        registry.register(
            "gateway_local_streams",
            "Agent streams this replica holds",
            self.local_streams.clone(),
        );
    }

    pub fn request(&self, path: RequestPath, outcome: RequestOutcome) {
        self.requests
            .get_or_create(&RequestLabels { path, outcome })
            .inc();
    }

    pub fn forward(&self, outcome: ForwardOutcome) {
        self.forwards.get_or_create(&ForwardLabels { outcome }).inc();
    }

    pub fn served(&self, outcome: ServeOutcome) {
        self.served.get_or_create(&ServeLabels { outcome }).inc();
    }

    pub fn superseded(&self) {
        self.superseded.inc();
    }

    pub fn stream_attached(&self) {
        self.local_streams.inc();
    }

    pub fn stream_detached(&self) {
        self.local_streams.dec();
    }

    #[must_use]
    pub fn requests_total(&self, path: RequestPath, outcome: RequestOutcome) -> u64 {
        self.requests
            .get_or_create(&RequestLabels { path, outcome })
            .get()
    }

    #[must_use]
    pub fn forwards_total(&self, outcome: ForwardOutcome) -> u64 {
        self.forwards.get_or_create(&ForwardLabels { outcome }).get()
    }

    #[must_use]
    pub fn served_total(&self, outcome: ServeOutcome) -> u64 {
        self.served.get_or_create(&ServeLabels { outcome }).get()
    }

    #[must_use]
    pub fn superseded_total(&self) -> u64 {
        self.superseded.get()
    }

    #[must_use]
    pub fn local_streams(&self) -> i64 {
        self.local_streams.get()
    }
}
