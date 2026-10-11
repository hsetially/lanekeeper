//! Plumbing that lets the `ports` conformance suite drive `PgAuditLog`: a `TxFactory` over a pool, and a probe that reads the
//! committed chain back.

use async_trait::async_trait;
use domain::{AuditId, ContentHash};
use ports::conformance::AuditProbe;
use ports::{AuditEntry, AuditError, AuditEvent, AuditLog, OwnedTx, Tx, TxError, TxFactory};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub struct PgTxFactory(pub PgPool);

struct PgOwnedTx(Transaction<'static, Postgres>);

#[async_trait]
impl OwnedTx for PgOwnedTx {
    fn tx(&mut self) -> Tx<'_> {
        Tx::from_pg(&mut self.0)
    }

    async fn commit(self: Box<Self>) -> Result<(), TxError> {
        self.0.commit().await.map_err(|_| TxError::CommitFailed)
    }

    async fn rollback(self: Box<Self>) -> Result<(), TxError> {
        self.0.rollback().await.map_err(|_| TxError::CommitFailed)
    }
}

#[async_trait]
impl TxFactory for PgTxFactory {
    async fn begin(&self) -> Result<Box<dyn OwnedTx>, TxError> {
        let tx = self.0.begin().await.map_err(|_| TxError::Unavailable)?;
        Ok(Box::new(PgOwnedTx(tx)))
    }
}

/// An audit log plus a way to read what it committed.
pub struct PgAuditProbe<L: AuditLog> {
    pub log: L,
    pub pool: PgPool,
}

#[async_trait]
impl<L: AuditLog> AuditLog for PgAuditProbe<L> {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        self.log.record(tx, e).await
    }
}

#[async_trait]
impl<L: AuditLog> AuditProbe for PgAuditProbe<L> {
    async fn committed(&self) -> Vec<AuditEntry> {
        committed(&self.pool).await
    }
}

fn hash(bytes: Vec<u8>) -> ContentHash {
    ContentHash::from_bytes(bytes.try_into().unwrap())
}

/// The whole chain, in order. For small test chains.
pub async fn committed(pool: &PgPool) -> Vec<AuditEntry> {
    sqlx::query("SELECT seq, prev_hash, hash, event_json FROM audit_events ORDER BY seq LIMIT 100000")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| AuditEntry {
            id: AuditId::from(r.get::<i64, _>("seq")),
            prev_hash: hash(r.get("prev_hash")),
            hash: hash(r.get("hash")),
            event: serde_json::from_str(&r.get::<String, _>("event_json")).unwrap(),
        })
        .collect()
}
