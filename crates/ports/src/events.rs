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
    /// Internal (see [`DomainEvent::is_internal`]): never streamed to browsers.
    UserSessionsRevoked,
    /// D85. Supersedes `PendingRestartChanged`.
    PickupChanged,
    /// contract/08-web: the baseline, tier, tenant set or auto-notify setting of a swimlane changed.
    SwimlaneChanged,
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
            DomainEvent::UserSessionsRevoked { .. } => Self::UserSessionsRevoked,
            DomainEvent::PickupChanged { .. } => Self::PickupChanged,
            DomainEvent::SwimlaneChanged { .. } => Self::SwimlaneChanged,
        }
    }
}

/// Which events a subscriber wants. The SSE edge additionally limits `swimlanes` to what the user can see.
///
/// - `swimlanes: None` means every swimlane; `Some(set)` limits swimlane-scoped events to the set.
///   Events that belong to no swimlane (see [`DomainEvent::swimlane`]) pass a swimlane filter.
/// - `kinds` empty means every kind.
/// - [`DomainEvent::Resync`] always passes, so a filtered subscriber still learns that it lagged.
/// - Internal events ([`DomainEvent::is_internal`]) pass like any other: in-process subscribers need them. The
///   SSE edge must drop them before it filters by user.
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

/// The most events one [`EventBus::publish_many_in_tx`] call takes. A bigger batch is refused whole with
/// [`BusError::BatchTooLarge`], never truncated.
///
/// The same number as [`crate::MAX_AUDIT_BATCH`]: a caller that audits a batch publishes the events of the same batch, so the
/// two limits go together. The outbox bus caps an envelope at 4 KiB, so 200 events are under a megabyte in one statement, and
/// there is no byte budget as there is for the audit chain: nothing here is held under a lock that other writers wait for.
pub const MAX_EVENT_BATCH: usize = 200;

/// Why an event could not be published. Carries no event content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BusError {
    /// `publish_in_tx` was given a transaction of another store.
    #[error("transaction is not backed by the expected store")]
    WrongTx,
    #[error("event bus is unavailable")]
    Unavailable,
    /// `publish_many_in_tx` was given more than [`MAX_EVENT_BATCH`] events. Nothing was published; split the batch.
    ///
    /// New with the batch methods: an exhaustive `match` on `BusError` (and on `AuditError`, which gained a variant of the same
    /// name) needs an arm for it.
    #[error("event batch is too large")]
    BatchTooLarge,
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

    /// `publish_in_tx` for each of `events`, in order, in the caller's transaction. An implementation may write them with
    /// one statement, but every event must get what `publish_in_tx` gives it: the same bounds on its payload, the same
    /// rules for who is sent it, an outbox position after the events before it (in this call and in the same
    /// transaction), fan-out only after the commit and never on a rollback, and at-least-once delivery.
    ///
    /// - An empty batch is `Ok`; it writes nothing and does not look at `tx`.
    /// - More than [`MAX_EVENT_BATCH`] events is [`BusError::BatchTooLarge`], and nothing is published.
    /// - On any error the caller rolls the transaction back, as after a failed `publish_in_tx`. A batch that fails
    ///   before its events are written (too many, or a transaction of another store) writes none of them.
    ///
    /// A batch of one event is no faster than `publish_in_tx`: use `publish_in_tx` for a single event. The gain is for many
    /// events, in one statement instead of one round trip for each.
    ///
    /// **A wrapper or decorator of an `EventBus` must forward this method** (and override it too, if it changes what
    /// `publish_in_tx` stores or checks, so that the two give an event the same treatment). The default calls
    /// `publish_in_tx`, so a wrapper that forwards only `publish_in_tx` still works, but it silently falls back to the slow
    /// loop of one statement per event.
    ///
    /// The default calls `publish_in_tx` once per event, after the checks above. Overriding is for the cost of a call,
    /// not for a different result.
    async fn publish_many_in_tx(&self, tx: &mut Tx<'_>, events: Vec<DomainEvent>) -> Result<(), BusError> {
        if events.is_empty() {
            return Ok(());
        }
        if events.len() > MAX_EVENT_BATCH {
            return Err(BusError::BatchTooLarge);
        }
        for e in events {
            self.publish_in_tx(tx, e).await?;
        }
        Ok(())
    }

    /// A bounded stream of events that match `f`, starting from now. On lag it yields
    /// [`DomainEvent::Resync`] before any later event. It ends when the bus shuts down.
    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent>;
}
