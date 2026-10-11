//! A test-only `EventBus` over the real `outbox` table (D80). The fake bus only accepts an in-memory `Tx`, so tests that
//! use a real Postgres transaction need this one.
//!
//! `publish_in_tx` inserts the row in the caller's transaction, so a rolled-back change leaves nothing behind and a committed
//! one leaves exactly one row. T2 only reads the table back (`published`); T3 adds the `LISTEN lanekeeper_outbox` side
//! that delivers to subscribers on every replica.

use async_trait::async_trait;
use domain::DomainEvent;
use futures::stream::BoxStream;
use ports::{BusError, EventBus, EventFilter, Tx};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct PgTestBus {
    pool: PgPool,
}

impl PgTestBus {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every event in the outbox, oldest first. Bounded: a test never writes more than a few hundred.
    pub async fn published(&self) -> Vec<DomainEvent> {
        let rows: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT event FROM outbox ORDER BY id LIMIT 1000")
                .fetch_all(&self.pool)
                .await
                .unwrap();
        rows.into_iter()
            .map(|v| serde_json::from_value(v).unwrap())
            .collect()
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
        let conn = tx.pg().map_err(|_| BusError::WrongTx)?;
        let json = serde_json::to_value(&e).map_err(|_| BusError::Unavailable)?;
        sqlx::query("INSERT INTO outbox (event) VALUES ($1)")
            .bind(json)
            .execute(&mut *conn)
            .await
            .map_err(|_| BusError::Unavailable)?;
        Ok(())
    }

    fn subscribe(&self, _f: EventFilter) -> BoxStream<'static, DomainEvent> {
        Box::pin(futures::stream::empty())
    }
}
