//! `AuditLog` over the real `audit_events` table and the real hash chain, for tests that run a state change on Postgres. It stands
//! in for 03a's log: it appends in the caller's transaction (an entry exists only if that transaction commits), serialises the
//! appends with a transaction-level advisory lock held until the commit (as the real log does), and can be told to fail from the
//! Nth call on, to show what a failing audit write rolls back.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use async_trait::async_trait;
use domain::{AuditId, ContentHash};
use ports::{AuditError, AuditEvent, AuditLog, Tx, canonical_json, chain_hash, genesis_hash};
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgTestAudit {
    /// Calls that succeed before every later one fails; negative: never fail.
    fail_after: AtomicI64,
    calls: AtomicU64,
}

impl Default for PgTestAudit {
    fn default() -> Self {
        Self {
            fail_after: AtomicI64::new(-1),
            calls: AtomicU64::new(0),
        }
    }
}

impl PgTestAudit {
    /// Calls that succeed, then every call fails. `None`: never fail.
    pub fn fail_after(&self, calls: Option<u64>) {
        self.fail_after
            .store(calls.map_or(-1, |c| i64::try_from(c).unwrap()), Ordering::SeqCst);
        self.calls.store(0, Ordering::SeqCst);
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AuditLog for PgTestAudit {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let limit = self.fail_after.load(Ordering::SeqCst);
        if limit >= 0 && i64::try_from(n).unwrap() >= limit {
            return Err(AuditError::Unavailable);
        }
        let json = canonical_json(&e)?;
        let value: serde_json::Value = serde_json::from_slice(&json).map_err(|_| AuditError::Encode)?;
        let conn = tx.pg().map_err(|_| AuditError::WrongTx)?;
        // Appends are serialised for the length of the transaction, so the chain order is the commit order.
        sqlx::query("SELECT pg_advisory_xact_lock(7203)")
            .execute(&mut *conn)
            .await
            .map_err(|_| AuditError::Unavailable)?;
        let prev: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT hash::bytea FROM audit_events ORDER BY seq DESC LIMIT 1")
                .fetch_optional(&mut *conn)
                .await
                .map_err(|_| AuditError::Unavailable)?;
        let prev = prev.map_or_else(genesis_hash, |bytes| {
            ContentHash::from_bytes(bytes.try_into().unwrap_or([0; 32]))
        });
        let hash = chain_hash(&prev, &json);
        let text = |v: &serde_json::Value| v.as_str().map(str::to_owned);
        let hex = |v: &serde_json::Value| {
            v.as_str()
                .and_then(|s| ContentHash::parse(s).ok())
                .map(|h| h.as_bytes().to_vec())
        };
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via, swimlane_id, path, \
                                       hash_before, hash_after, attribution_source, attribution_confidence) \
             VALUES ($1::bytea, $2::bytea, $3, timestamptz 'epoch' + ($4::bigint * interval '1 millisecond'), $5, $6, $7, $8, $9, \
                     $10, $11::bytea, $12::bytea, $13, $14) RETURNING seq",
        )
        .bind(prev.as_bytes().as_slice())
        .bind(hash.as_bytes().as_slice())
        .bind(String::from_utf8_lossy(&json).into_owned())
        .bind(e.at.unix_millis())
        .bind(text(&value["actor"]["kind"]))
        .bind(text(&value["actor"]["swimlane"]).or_else(|| text(&value["actor"]["user"])))
        .bind(text(&value["action"]))
        .bind(text(&value["via"]))
        .bind(text(&value["swimlane"]))
        .bind(text(&value["path"]))
        .bind(hex(&value["hash_before"]))
        .bind(hex(&value["hash_after"]))
        .bind(text(&value["attribution"]["source"]))
        .bind(text(&value["attribution"]["confidence"]))
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| AuditError::Unavailable)?;
        Ok(AuditId::from(seq))
    }
}

/// One audit row, for assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    pub action: String,
    pub via: String,
    pub swimlane: Option<String>,
    pub path: Option<String>,
    pub hash_before: Option<ContentHash>,
    pub hash_after: Option<ContentHash>,
    pub attribution_source: Option<String>,
}

pub async fn audit_rows(pool: &PgPool) -> Vec<AuditRow> {
    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT action, via, swimlane_id, path, hash_before::bytea, hash_after::bytea, attribution_source \
             FROM audit_events ORDER BY seq LIMIT 20000",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let h = |b: Option<Vec<u8>>| b.map(|b| ContentHash::from_bytes(b.try_into().unwrap()));
    rows.into_iter()
        .map(|(action, via, swimlane, path, before, after, source)| AuditRow {
            action,
            via,
            swimlane,
            path,
            hash_before: h(before),
            hash_after: h(after),
            attribution_source: source,
        })
        .collect()
}
