//! The event bus port (D80): fan-out of [`DomainEvent`]s across hub replicas and to SSE clients.

use std::collections::BTreeSet;

use async_trait::async_trait;
use domain::{DomainEvent, SwimlaneId};
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::Tx;

/// The discriminant of a [`DomainEvent`], as named on the wire (`type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    FileObserved,
    DriftChanged,
    FindingsChanged,
    PendingRestartChanged,
    ProposalChanged,
    AgentStatusChanged,
    GitHeadMoved,
    AccessRequestCreated,
    DocIndexed,
    Resync,
    AttributionUpdated,
    PrStateChanged,
    PrJobProgress,
    SyncWindowOpened,
    SyncWindowClosed,
}

impl EventKind {
    pub fn of(e: &DomainEvent) -> Self {
        match e {
            DomainEvent::FileObserved { .. } => Self::FileObserved,
            DomainEvent::DriftChanged { .. } => Self::DriftChanged,
            DomainEvent::FindingsChanged { .. } => Self::FindingsChanged,
            DomainEvent::PendingRestartChanged { .. } => Self::PendingRestartChanged,
            DomainEvent::ProposalChanged { .. } => Self::ProposalChanged,
            DomainEvent::AgentStatusChanged { .. } => Self::AgentStatusChanged,
            DomainEvent::GitHeadMoved { .. } => Self::GitHeadMoved,
            DomainEvent::AccessRequestCreated { .. } => Self::AccessRequestCreated,
            DomainEvent::DocIndexed { .. } => Self::DocIndexed,
            DomainEvent::Resync => Self::Resync,
            DomainEvent::AttributionUpdated { .. } => Self::AttributionUpdated,
            DomainEvent::PrStateChanged { .. } => Self::PrStateChanged,
            DomainEvent::PrJobProgress { .. } => Self::PrJobProgress,
            DomainEvent::SyncWindowOpened { .. } => Self::SyncWindowOpened,
            DomainEvent::SyncWindowClosed { .. } => Self::SyncWindowClosed,
        }
    }
}

/// Which events a subscriber wants. The SSE edge additionally limits `swimlanes` to what the user can see.
///
/// - `swimlanes: None` means every swimlane; `Some(set)` limits swimlane-scoped events to the set.
///   Events that belong to no swimlane (see [`DomainEvent::swimlane`]) pass a swimlane filter.
/// - `kinds` empty means every kind.
/// - [`DomainEvent::Resync`] always passes, so a filtered subscriber still learns that it lagged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub swimlanes: Option<BTreeSet<SwimlaneId>>,
    pub kinds: BTreeSet<EventKind>,
}

impl EventFilter {
    pub fn all() -> Self {
        Self::default()
    }

    pub fn for_swimlane(s: SwimlaneId) -> Self {
        Self {
            swimlanes: Some(BTreeSet::from([s])),
            kinds: BTreeSet::new(),
        }
    }

    pub fn matches(&self, e: &DomainEvent) -> bool {
        if matches!(e, DomainEvent::Resync) {
            return true;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&EventKind::of(e)) {
            return false;
        }
        match (&self.swimlanes, e.swimlane()) {
            (Some(allowed), Some(s)) => allowed.contains(s),
            _ => true,
        }
    }
}

/// Why an event could not be published. Carries no event content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BusError {
    /// `publish_in_tx` was given a transaction of another store.
    #[error("transaction is not backed by the expected store")]
    WrongTx,
    #[error("event bus is unavailable")]
    Unavailable,
}

/// Fan-out of domain events. Subscribers are bounded: a slow subscriber loses events and receives
/// [`DomainEvent::Resync`] in their place, and must refetch state. Nothing blocks the publisher.
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// Fans out to every replica. For events that are not tied to a database change.
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError>;

    /// Writes the event to the outbox inside the caller's transaction (D80). It is fanned out only after
    /// the transaction commits, and never if it rolls back. Delivery is at least once.
    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError>;

    /// A bounded stream of events that match `f`, starting from now. On lag it yields
    /// [`DomainEvent::Resync`] before any later event. It ends when the bus shuts down.
    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent>;
}
