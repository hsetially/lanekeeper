//! A test double of the `AuditLog` port over the real `audit_events` table (plan A14): the same chain as the real log (the
//! advisory lock, the head read, `ports::canonical_json` and `ports::chain_hash`), so a chain written through it verifies with
//! `ports::verify_chain`, and the table's own CHECK and triggers apply. It is not hub-identity's code: a feature crate's tests
//! never use a sibling's implementation. It can be told to fail, to prove that a failed audit rolls the change back.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use domain::{AuditId, ContentHash};
use ports::{
    AuditActor, AuditEntry, AuditError, AuditEvent, AuditLog, Tx, canonical_json, chain_hash, genesis_hash,
};
use sqlx::{PgPool, Row};

/// The advisory lock key of the chain (the real log's value).
const CHAIN_LOCK_KEY: i64 = 0x004C_4B41_5544_4955;

#[derive(Debug, Default)]
pub struct PgTestAudit {
    fail_next: AtomicBool,
}

impl PgTestAudit {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next `record` fails with `Unavailable`.
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

fn actor_columns(a: &AuditActor) -> (&'static str, String) {
    match a {
        AuditActor::User { user } => ("user", user.to_string()),
        AuditActor::Agent { swimlane } => ("agent", swimlane.as_str().to_owned()),
        AuditActor::Sentinel { name } => ("sentinel", name.as_str().to_owned()),
        AuditActor::System { component } => ("system", component.as_str().to_owned()),
    }
}

fn wire<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

#[async_trait]
impl AuditLog for PgTestAudit {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(AuditError::Unavailable);
        }
        if e.diff
            .as_ref()
            .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
        {
            return Err(AuditError::TooLarge);
        }
        let json = canonical_json(&e)?;
        let conn = tx.pg().map_err(|_| AuditError::WrongTx)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(CHAIN_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(|_| AuditError::Unavailable)?;
        let head: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1")
                .fetch_optional(&mut *conn)
                .await
                .map_err(|_| AuditError::Unavailable)?;
        let prev = match head {
            Some(h) => ContentHash::from_bytes(h.try_into().map_err(|_| AuditError::Unavailable)?),
            None => genesis_hash(),
        };
        let hash = chain_hash(&prev, &json);
        let (kind, actor_ref) = actor_columns(&e.actor);
        let text = String::from_utf8(json).map_err(|_| AuditError::Encode)?;
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO audit_events \
             (prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via, swimlane_id, path, hash_before, hash_after, \
              github_login, approval_id, request_id) \
             VALUES ($1, $2, $3, timestamptz 'epoch' + ($4::bigint::text || ' milliseconds')::interval, $5, $6, $7, $8, $9, $10, \
                     $11, $12, $13, $14, $15) RETURNING seq",
        )
        .bind(prev.as_bytes().as_slice())
        .bind(hash.as_bytes().as_slice())
        .bind(text)
        .bind(e.at.unix_millis())
        .bind(kind)
        .bind(actor_ref)
        .bind(wire(&e.action))
        .bind(wire(&e.via))
        .bind(e.swimlane.as_ref().map(|s| s.as_str().to_owned()))
        .bind(e.path.as_ref().map(|p| p.as_str().to_owned()))
        .bind(e.hash_before.map(|h| h.as_bytes().to_vec()))
        .bind(e.hash_after.map(|h| h.as_bytes().to_vec()))
        .bind(e.github_login.as_ref().map(|g| g.as_str().to_owned()))
        .bind(e.approval.map(domain::ProposalId::get))
        .bind(e.request_id.as_ref().map(|r| r.as_str().to_owned()))
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| AuditError::Unavailable)?;
        Ok(AuditId::from(seq))
    }
}

/// The whole committed chain, in order. For small test chains.
pub async fn committed(pool: &PgPool) -> Vec<AuditEntry> {
    sqlx::query("SELECT seq, prev_hash, hash, event_json FROM audit_events ORDER BY seq LIMIT 100000")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| AuditEntry {
            id: AuditId::from(r.get::<i64, _>("seq")),
            prev_hash: ContentHash::from_bytes(r.get::<Vec<u8>, _>("prev_hash").try_into().unwrap()),
            hash: ContentHash::from_bytes(r.get::<Vec<u8>, _>("hash").try_into().unwrap()),
            event: serde_json::from_str(&r.get::<String, _>("event_json")).unwrap(),
        })
        .collect()
}
