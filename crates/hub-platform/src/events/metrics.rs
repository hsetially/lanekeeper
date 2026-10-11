//! Metrics of the event bus and the SSE endpoint (S10).
//!
//! Every label comes from a closed set. A user, a swimlane, a path or an event id is never a label, so the number of series is
//! fixed. The relay lag (how many outbox ids behind the newest row the relay is) is the number to alert on for P13.

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

/// How a publish ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum PublishOutcome {
    Stored,
    /// The event was over the size cap and a `Resync` was stored in its place.
    StoredAsResync,
    Failed,
}

/// Why subscribers were told to refetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ResyncReason {
    /// A subscriber fell behind the bounded channel.
    Lagged,
    /// The listener's connection was lost and made again: notifications may have been missed.
    Reconnect,
    /// Reading the outbox failed for longer than the overlap window.
    Outage,
    /// A row could not be read as an event.
    Undecodable,
}

/// How an SSE stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SseEnd {
    ClientGone,
    /// The user's session ended or their role dropped below Viewer.
    Revoked,
    Shutdown,
    /// The client stopped reading for longer than the write timeout.
    SlowClient,
    BusClosed,
    /// The stream reached its maximum age; the client reconnects with its `Last-Event-ID`.
    MaxAge,
}

/// Why an SSE request was refused after it passed the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum SseRefusal {
    BadRequest,
    TooManyForUser,
    TooManyTotal,
}

/// How a replay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum ReplayOutcome {
    Events,
    /// The id was outside the window, unknown, or too many events matched.
    Resync,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct PublishLabels {
    outcome: PublishOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ResyncLabels {
    reason: ResyncReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct EndLabels {
    reason: SseEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RefusalLabels {
    reason: SseRefusal,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ReplayLabels {
    outcome: ReplayOutcome,
}

/// Cloning shares the counters.
#[derive(Debug, Clone, Default)]
pub struct EventMetrics {
    published: Family<PublishLabels, Counter>,
    relayed: Counter,
    relay_errors: Counter,
    relay_lag: Gauge,
    resyncs: Family<ResyncLabels, Counter>,
    listener_connected: Gauge,
    subscribers: Gauge,
    sse_streams: Gauge,
    sse_ended: Family<EndLabels, Counter>,
    sse_refused: Family<RefusalLabels, Counter>,
    replays: Family<ReplayLabels, Counter>,
}

impl EventMetrics {
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "events_published",
            "Events written to the outbox, by outcome",
            self.published.clone(),
        );
        registry.register(
            "events_relayed",
            "Outbox rows this replica read and handed to its subscribers",
            self.relayed.clone(),
        );
        registry.register(
            "events_relay_errors",
            "Reads of the outbox that failed",
            self.relay_errors.clone(),
        );
        registry.register(
            "events_relay_lag",
            "Outbox ids between the relay's cursor and the newest row, when a read filled its batch",
            self.relay_lag.clone(),
        );
        registry.register(
            "events_resyncs",
            "Times subscribers were told to refetch, by reason",
            self.resyncs.clone(),
        );
        registry.register(
            "events_listener_connected",
            "1 while the LISTEN connection of this replica is up",
            self.listener_connected.clone(),
        );
        registry.register(
            "events_subscribers",
            "Live subscriptions on this replica",
            self.subscribers.clone(),
        );
        registry.register(
            "events_sse_streams",
            "SSE streams open on this replica",
            self.sse_streams.clone(),
        );
        registry.register(
            "events_sse_ended",
            "SSE streams that ended, by reason",
            self.sse_ended.clone(),
        );
        registry.register(
            "events_sse_refused",
            "SSE requests refused after authorisation, by reason",
            self.sse_refused.clone(),
        );
        registry.register("events_replays", "SSE replays, by outcome", self.replays.clone());
    }

    pub fn published(&self, outcome: PublishOutcome) {
        self.published.get_or_create(&PublishLabels { outcome }).inc();
    }

    pub fn relayed(&self) {
        self.relayed.inc();
    }

    pub fn relay_error(&self) {
        self.relay_errors.inc();
    }

    pub fn set_relay_lag(&self, lag: i64) {
        self.relay_lag.set(lag);
    }

    pub fn resync(&self, reason: ResyncReason) {
        self.resyncs.get_or_create(&ResyncLabels { reason }).inc();
    }

    pub fn set_listener_connected(&self, connected: bool) {
        self.listener_connected.set(i64::from(connected));
    }

    pub fn subscriber_added(&self) {
        self.subscribers.inc();
    }

    pub fn subscriber_removed(&self) {
        self.subscribers.dec();
    }

    pub fn sse_started(&self) {
        self.sse_streams.inc();
    }

    pub fn sse_ended(&self, reason: SseEnd) {
        self.sse_streams.dec();
        self.sse_ended.get_or_create(&EndLabels { reason }).inc();
    }

    pub fn sse_refused(&self, reason: SseRefusal) {
        self.sse_refused.get_or_create(&RefusalLabels { reason }).inc();
    }

    pub fn replay(&self, outcome: ReplayOutcome) {
        self.replays.get_or_create(&ReplayLabels { outcome }).inc();
    }

    #[must_use]
    pub fn published_total(&self, outcome: PublishOutcome) -> u64 {
        self.published.get_or_create(&PublishLabels { outcome }).get()
    }

    #[must_use]
    pub fn relayed_total(&self) -> u64 {
        self.relayed.get()
    }

    #[must_use]
    pub fn resyncs_total(&self, reason: ResyncReason) -> u64 {
        self.resyncs.get_or_create(&ResyncLabels { reason }).get()
    }

    #[must_use]
    pub fn sse_streams(&self) -> i64 {
        self.sse_streams.get()
    }

    #[must_use]
    pub fn sse_ended_total(&self, reason: SseEnd) -> u64 {
        self.sse_ended.get_or_create(&EndLabels { reason }).get()
    }

    #[must_use]
    pub fn sse_refused_total(&self, reason: SseRefusal) -> u64 {
        self.sse_refused.get_or_create(&RefusalLabels { reason }).get()
    }

    #[must_use]
    pub fn replays_total(&self, outcome: ReplayOutcome) -> u64 {
        self.replays.get_or_create(&ReplayLabels { outcome }).get()
    }

    #[must_use]
    pub fn relay_lag(&self) -> i64 {
        self.relay_lag.get()
    }
}
