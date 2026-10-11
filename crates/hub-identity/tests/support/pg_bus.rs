//! A test-only `EventBus` over the real `outbox` table (D80). The fake bus only accepts an in-memory `Tx`, so tests that
//! use a real Postgres transaction need this one.
//!
//! `publish_in_tx` inserts the row in the caller's transaction, so a rolled-back change leaves nothing behind and a committed
//! one leaves exactly one row. The table's trigger sends `NOTIFY lanekeeper_outbox` when the transaction commits, and a bus
//! made with [`PgTestBus::listening`] receives it on its own `LISTEN` connection, reads the row and hands the event to its
//! subscribers. Two buses on one database are two hub replicas: a change committed through one reaches the other's
//! subscribers, which is how the tests prove that a revocation reaches every replica.

use std::sync::Arc;

use async_trait::async_trait;
use domain::DomainEvent;
use futures::StreamExt;
use futures::stream::BoxStream;
use ports::{BusError, EventBus, EventFilter, Tx};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

/// The channel the outbox trigger notifies (`db/migrations/0001_init.sql`).
const CHANNEL: &str = "lanekeeper_outbox";

/// Stops the listener task when the last clone of the bus is dropped.
struct StopOnDrop(tokio::task::JoinHandle<()>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
pub struct PgTestBus {
    pool: PgPool,
    sender: broadcast::Sender<DomainEvent>,
    _listener: Option<Arc<StopOnDrop>>,
}

impl std::fmt::Debug for PgTestBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgTestBus").finish_non_exhaustive()
    }
}

impl PgTestBus {
    /// A bus that only writes the outbox. Its `subscribe` yields nothing.
    pub fn new(pool: PgPool) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            pool,
            sender,
            _listener: None,
        }
    }

    /// A bus that also listens: it returns once `LISTEN` is active, so no event committed after it returns is missed.
    pub async fn listening(pool: PgPool) -> Self {
        let (sender, _) = broadcast::channel(256);
        let mut listener = PgListener::connect_with(&pool).await.unwrap();
        listener.listen(CHANNEL).await.unwrap();
        let task = {
            let (pool, sender) = (pool.clone(), sender.clone());
            tokio::spawn(async move {
                while let Ok(note) = listener.recv().await {
                    let Ok(id) = note.payload().parse::<i64>() else {
                        continue;
                    };
                    let row: Option<serde_json::Value> =
                        sqlx::query_scalar("SELECT event FROM outbox WHERE id = $1")
                            .bind(id)
                            .fetch_optional(&pool)
                            .await
                            .ok()
                            .flatten();
                    if let Some(event) = row.and_then(|v| serde_json::from_value::<DomainEvent>(v).ok()) {
                        // No receiver is fine: nobody subscribed yet.
                        let _ = sender.send(event);
                    }
                }
            })
        };
        Self {
            pool,
            sender,
            _listener: Some(Arc::new(StopOnDrop(task))),
        }
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

    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
        // Subscribe now, not on first poll, so that nothing published before the first poll is lost.
        let rx = self.sender.subscribe();
        futures::stream::unfold((rx, f), |(mut rx, f)| async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        if f.matches(&e) {
                            return Some((e, (rx, f)));
                        }
                    }
                    Err(RecvError::Lagged(_)) => return Some((DomainEvent::Resync, (rx, f))),
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .boxed()
    }
}
