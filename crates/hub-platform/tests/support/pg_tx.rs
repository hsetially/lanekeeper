//! A `TxFactory` over a real Postgres pool, for `ports::conformance::event_bus` and the audit and outbox tests: `begin` opens a
//! transaction that owns its connection, `tx()` lends it to a port call as a `Tx`, and `commit` and `rollback` end it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use ports::{OwnedTx, Tx, TxError, TxFactory};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone)]
pub struct PgTxFactory {
    pool: PgPool,
}

impl PgTxFactory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct PgOwnedTx {
    tx: Transaction<'static, Postgres>,
}

#[async_trait]
impl OwnedTx for PgOwnedTx {
    fn tx(&mut self) -> Tx<'_> {
        Tx::from_pg(&mut self.tx)
    }

    async fn commit(self: Box<Self>) -> Result<(), TxError> {
        self.tx.commit().await.map_err(|_| TxError::CommitFailed)
    }

    async fn rollback(self: Box<Self>) -> Result<(), TxError> {
        self.tx.rollback().await.map_err(|_| TxError::Unavailable)
    }
}

#[async_trait]
impl TxFactory for PgTxFactory {
    async fn begin(&self) -> Result<Box<dyn OwnedTx>, TxError> {
        let tx = self.pool.begin().await.map_err(|_| TxError::Unavailable)?;
        Ok(Box::new(PgOwnedTx { tx }))
    }
}
