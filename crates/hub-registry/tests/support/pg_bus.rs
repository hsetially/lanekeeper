//! A test-only `EventBus` over the real `outbox` table (D80). `publish_in_tx` inserts the row in the caller's transaction, so a
//! rolled-back change leaves nothing behind and a committed one leaves exactly one row. The fake bus accepts only an in-memory
//! `Tx`, so tests that run a real Postgres transaction need this one.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use domain::DomainEvent;
use futures::stream::BoxStream;
use ports::{BusError, EventBus, EventFilter, Tx};
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgTestBus {
    pool: PgPool,
    fail: AtomicBool,
}

impl PgTestBus {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            fail: AtomicBool::new(false),
        }
    }

    /// Make `publish_in_tx` fail, to show what an event that cannot be written rolls back.
    pub fn set_failing(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }

    /// Every event in the outbox, oldest first. Bounded: a test never writes more than a few thousand.
    pub async fn published(&self) -> Vec<DomainEvent> {
        let rows: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT event FROM outbox ORDER BY id LIMIT 20000")
                .fetch_all(&self.pool)
                .await
                .unwrap();
        rows.into_iter()
            .map(|v| serde_json::from_value(v).unwrap())
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
        let json = serde_json::to_value(&e).map_err(|_| BusError::Unavailable)?;
        sqlx::query("INSERT INTO outbox (event) VALUES ($1)")
            .bind(json)
            .execute(&self.pool)
            .await
            .map_err(|_| BusError::Unavailable)?;
        Ok(())
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(BusError::Unavailable);
        }
        let conn = tx.pg().map_err(|_| BusError::WrongTx)?;
        let json = serde_json::to_value(&e).map_err(|_| BusError::Unavailable)?;
        sqlx::query("INSERT INTO outbox (event) VALUES ($1)")
            .bind(json)
            .execute(&mut *conn)
            .await
            .map_err(|_| BusError::Unavailable)?;
        Ok(())
    }

    fn subscribe(&self, _: EventFilter) -> BoxStream<'static, DomainEvent> {
        Box::pin(futures::stream::empty())
    }
}
