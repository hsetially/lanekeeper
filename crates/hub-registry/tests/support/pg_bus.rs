//! The real `PgEventBus` (hub-platform, 03b) behind a thin decorator, for the tests that run a state change on Postgres (D80): an event
//! published in the caller's transaction is a row of the real `outbox` table, written by the real bus, so a rolled-back change leaves
//! nothing behind and a committed one leaves exactly one row for each event. The bus is not started (no relay, no listener), so nothing
//! is delivered; the tests that need delivery build the rig `with_event_bus` and start it. What the decorator adds is a switch that
//! makes the publishes fail, and the readers of the outbox for the assertions.
//!
//! **It forwards `publish_in_tx` and `publish_many_in_tx`** to the real bus, so a registry that calls `publish_many_in_tx` takes the
//! real one-statement path. A decorator that forwarded only `publish_in_tx` would still work, through the default `publish_many_in_tx`,
//! and would silently run one statement per event (P15): `tests/batch_writes.rs` counts statements to catch that.
//! [`PgTestBus::per_event`] is that slow loop on purpose: the reference of the equivalence test, what the registry did before it batched.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use domain::DomainEvent;
use futures::stream::BoxStream;
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use ports::{BusError, EventBus, EventFilter, Tx};
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgTestBus {
    pool: PgPool,
    inner: PgEventBus,
    /// `publish_many_in_tx` as the trait's default has it, one real `publish_in_tx` for each event.
    per_event: bool,
    fail: AtomicBool,
    /// Every publish fails with this error, when set.
    fail_with: Mutex<Option<BusError>>,
}

impl PgTestBus {
    pub fn new(pool: PgPool) -> Self {
        Self::build(pool, false)
    }

    /// The registry's reference: `publish_many_in_tx` is one `publish_in_tx` for each event, as before the bus had a batch call.
    pub fn per_event(pool: PgPool) -> Self {
        Self::build(pool, true)
    }

    fn build(pool: PgPool, per_event: bool) -> Self {
        Self {
            inner: PgEventBus::new(pool.clone(), BusSettings::default(), EventMetrics::default()),
            pool,
            per_event,
            fail: AtomicBool::new(false),
            fail_with: Mutex::new(None),
        }
    }

    /// Make the publishes in a transaction fail, to show what an event that cannot be written rolls back.
    pub fn set_failing(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }

    /// Make every publish in a transaction fail with `error` until it is `None` again.
    pub fn fail_with(&self, error: Option<BusError>) {
        *self.fail_with.lock().unwrap() = error;
    }

    fn refused(&self) -> Result<(), BusError> {
        if let Some(error) = *self.fail_with.lock().unwrap() {
            return Err(error);
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(BusError::Unavailable);
        }
        Ok(())
    }

    /// Every event in the outbox, oldest first. Bounded: a test never writes more than a few thousand.
    pub async fn published(&self) -> Vec<DomainEvent> {
        let rows: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT event FROM outbox ORDER BY id LIMIT 20000")
                .fetch_all(&self.pool)
                .await
                .unwrap();
        // The registry's own tests write plain events; the real bus writes `{"v":1,"event":{...}}`. Both are read.
        rows.into_iter()
            .map(|v| match v.get("event") {
                Some(inner) if v.get("v").is_some() => serde_json::from_value(inner.clone()).unwrap(),
                _ => serde_json::from_value(v).unwrap(),
            })
            .collect()
    }

    pub async fn count(&self) -> usize {
        usize::try_from(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM outbox")
                .fetch_one(&self.pool)
                .await
                .unwrap(),
        )
        .unwrap()
    }
}

#[async_trait]
impl EventBus for PgTestBus {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
        self.inner.publish(e).await
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        self.refused()?;
        self.inner.publish_in_tx(tx, e).await
    }

    async fn publish_many_in_tx(&self, tx: &mut Tx<'_>, events: Vec<DomainEvent>) -> Result<(), BusError> {
        if events.is_empty() {
            return Ok(());
        }
        self.refused()?;
        if self.per_event {
            for e in events {
                self.inner.publish_in_tx(tx, e).await?;
            }
            return Ok(());
        }
        self.inner.publish_many_in_tx(tx, events).await
    }

    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
        self.inner.subscribe(f)
    }
}
