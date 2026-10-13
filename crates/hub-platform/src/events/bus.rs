//! `PgEventBus`: the [`EventBus`] port over the Postgres outbox (D80, P13).
//!
//! ```text
//!   publish / publish_in_tx --INSERT--> outbox --commit--> NOTIFY lanekeeper_outbox
//!                                                              |
//!        every replica:  OutboxListener (one LISTEN connection) wakes OutboxRelay
//!                        OutboxRelay reads rows after its cursor  -->  broadcast channel (bounded)
//!                        subscribers read the channel; one that falls behind gets a Resync
//! ```
//!
//! Every event, plain or inside a transaction, is a row, so every event has one cluster-wide, ordered id (the SSE event id) and can be
//! replayed. A plain `publish` is one autocommit `INSERT`; `publish_in_tx` is the same `INSERT` on the caller's transaction, so it
//! commits or rolls back with the change. Delivery is at least once. Nothing is fanned out before the commit.
//!
//! Events are delivered in id order, except that a row whose transaction commits after a higher id has been read is delivered when
//! the relay first sees it (see [`super::relay`]): two replicas may hear two such events in different orders, and the events of one
//! publisher, which commit one after the other, are always heard in order. A subscriber made just before an event commits may or
//! may not hear it, and one made just after may hear it: "from now" is accurate to the relay's last read.
//!
//! Subscribers share one bounded broadcast channel per replica: the relay never waits for a subscriber, and a subscriber that is
//! `capacity` events behind loses the oldest and is told to refetch ([`DomainEvent::Resync`]) before it sees anything newer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::DomainEvent;
use futures::StreamExt;
use futures::stream::BoxStream;
use ports::{BusError, EventBus, EventFilter, Tx};
use sqlx::PgPool;
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::JoinHandle;

use super::listen::{ListenState, OutboxListener};
use super::metrics::{EventMetrics, PublishOutcome, ResyncReason};
use super::outbox::{self, OutboxError};
use super::relay::{OutboxRelay, RelaySettings};
use super::source::{EventSource, Relayed, Replay, relayed_stream};
use super::wire::{self, EventId, MAX_EVENT_BYTES};

/// Bounds and timings of the bus.
#[derive(Debug, Clone)]
pub struct BusSettings {
    /// How many events a subscriber may fall behind before it gets a `Resync` instead. Must be at least a few relay batches, so
    /// that a subscriber that is running keeps up with a burst.
    pub capacity: usize,
    /// The most an envelope may weigh when it is written. A bigger event is stored as a `Resync`.
    pub max_event_bytes: usize,
    /// How far back a `Last-Event-ID` may be replayed from.
    pub replay_window: Duration,
    pub relay: RelaySettings,
}

impl Default for BusSettings {
    fn default() -> Self {
        Self {
            capacity: 1024,
            max_event_bytes: MAX_EVENT_BYTES,
            replay_window: Duration::from_secs(5 * 60),
            relay: RelaySettings::default(),
        }
    }
}

/// The tasks of a running bus. They end when the `stop` channel given to [`PgEventBus::start`] turns true or is dropped.
#[derive(Debug)]
pub struct BusTasks {
    pub listener: JoinHandle<()>,
    pub relay: JoinHandle<()>,
}

struct Inner {
    pool: PgPool,
    sender: broadcast::Sender<Arc<Relayed>>,
    closed: Arc<watch::Sender<bool>>,
    listen: Arc<ListenState>,
    wake: Arc<Notify>,
    settings: BusSettings,
    metrics: EventMetrics,
}

/// The bus of one replica. Cloning shares it.
#[derive(Clone)]
pub struct PgEventBus {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PgEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgEventBus")
            .field("subscribers", &self.inner.sender.receiver_count())
            .finish_non_exhaustive()
    }
}

/// Decrements the subscriber gauge when the subscription's stream is dropped.
struct SubscriptionGuard(EventMetrics);

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.0.subscriber_removed();
    }
}

impl Inner {
    fn subscribe(&self, filter: EventFilter) -> BoxStream<'static, Arc<Relayed>> {
        // The subscription exists from this call, not from the first poll, so nothing relayed after it is missed.
        let rx = self.sender.subscribe();
        self.metrics.subscriber_added();
        let guard = SubscriptionGuard(self.metrics.clone());
        let metrics = self.metrics.clone();
        relayed_stream(rx, filter, self.closed.subscribe(), move || {
            // The guard lives as long as the stream does: the closure is part of the stream's state.
            let _alive = &guard;
            metrics.resync(ResyncReason::Lagged);
        })
    }
}

impl PgEventBus {
    #[must_use]
    pub fn new(pool: PgPool, settings: BusSettings, metrics: EventMetrics) -> Self {
        let (sender, _) = broadcast::channel(settings.capacity.max(1));
        Self {
            inner: Arc::new(Inner {
                pool,
                sender,
                closed: Arc::new(watch::channel(false).0),
                listen: Arc::new(ListenState::default()),
                wake: Arc::new(Notify::new()),
                settings,
                metrics,
            }),
        }
    }

    /// Start the listener and the relay. The relay's cursor is read before this returns, so every event inserted after it is
    /// delivered to subscribers made before it. If the database cannot be reached now, the relay reads the cursor as soon as it
    /// can (nobody can have been told anything in the meantime, since nothing could be written here).
    pub async fn start(&self, stop: watch::Receiver<bool>) -> BusTasks {
        let relay = OutboxRelay {
            pool: self.inner.pool.clone(),
            sender: self.inner.sender.clone(),
            listen: Arc::clone(&self.inner.listen),
            wake: Arc::clone(&self.inner.wake),
            metrics: self.inner.metrics.clone(),
            settings: self.inner.settings.relay.clone(),
            closed: Arc::clone(&self.inner.closed),
        };
        let initial = relay.initial_state().await.ok();
        let listener = OutboxListener {
            pool: self.inner.pool.clone(),
            state: Arc::clone(&self.inner.listen),
            wake: Arc::clone(&self.inner.wake),
            metrics: self.inner.metrics.clone(),
        };
        BusTasks {
            listener: tokio::spawn(listener.run(stop.clone())),
            relay: tokio::spawn(relay.run(initial, stop)),
        }
    }

    /// Whether the `LISTEN` connection is up: part of `/readyz`.
    #[must_use]
    pub fn listener_connected(&self) -> bool {
        self.inner.listen.connected()
    }

    #[must_use]
    pub fn metrics(&self) -> &EventMetrics {
        &self.inner.metrics
    }

    /// The live subscriptions on this replica.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        self.inner.sender.receiver_count()
    }

    /// The bus as the SSE endpoint's source of events (live, with ids, and replay).
    #[must_use]
    pub fn source(&self) -> Arc<dyn EventSource> {
        Arc::new(PgEventSource {
            inner: Arc::clone(&self.inner),
        })
    }
}

impl PgEventBus {
    /// Count a write, and turn its result into the port's error.
    fn written(&self, result: Result<EventId, OutboxError>, replaced: bool) -> Result<(), BusError> {
        // For an event inside a transaction, "written" is not "committed": it may still roll back, and then nobody hears of it.
        let outcome = match (&result, replaced) {
            (Ok(_), false) => PublishOutcome::Stored,
            (Ok(_), true) => PublishOutcome::StoredAsResync,
            (Err(_), _) => PublishOutcome::Failed,
        };
        self.inner.metrics.published(outcome);
        result.map(|_| ()).map_err(|_| BusError::Unavailable)
    }
}

#[async_trait]
impl EventBus for PgEventBus {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
        let (text, replaced) = outbox::prepare(&e, self.inner.settings.max_event_bytes)?;
        let result = outbox::insert_autocommit(&self.inner.pool, &text).await;
        self.written(result, replaced)
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        let (text, replaced) = outbox::prepare(&e, self.inner.settings.max_event_bytes)?;
        let conn = tx.pg().map_err(|_| BusError::WrongTx)?;
        let result = outbox::insert(conn, &text).await;
        self.written(result, replaced)
    }

    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
        self.inner
            .subscribe(f)
            .map(|relayed| relayed.event.clone())
            .boxed()
    }
}

/// [`EventSource`] over the bus's own state, so the bus itself keeps one `subscribe`.
struct PgEventSource {
    inner: Arc<Inner>,
}

#[async_trait]
impl EventSource for PgEventSource {
    fn subscribe(&self, filter: EventFilter) -> BoxStream<'static, Arc<Relayed>> {
        self.inner.subscribe(filter)
    }

    async fn replay(&self, after: EventId, filter: &EventFilter, limit: usize) -> Result<Replay, BusError> {
        let window = self.inner.settings.replay_window;
        // The row the client last saw must exist and be inside the window; otherwise the log cannot say what it missed.
        match outbox::within_window(&self.inner.pool, after, window).await {
            Ok(Some(true)) => {}
            Ok(Some(false) | None) => return Ok(Replay::Resync),
            Err(_) => return Err(BusError::Unavailable),
        }
        let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let Ok(rows) = outbox::read_after(&self.inner.pool, after.get(), fetch).await else {
            return Err(BusError::Unavailable);
        };
        if rows.len() > limit {
            return Ok(Replay::Resync);
        }
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            match wire::decode(&row.text, self.inner.settings.relay.read_cap_bytes) {
                Ok(event) => {
                    if filter.matches(&event) {
                        events.push((row.id, event));
                    }
                }
                // A row that cannot be read means the log cannot be trusted to say what was missed.
                Err(_) => return Ok(Replay::Resync),
            }
        }
        Ok(Replay::Events(events))
    }
}
