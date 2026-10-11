//! A test-only `AuditLog` over a real Postgres transaction, for T2 and T3 (plan decision B7).
//!
//! It is the smallest honest chain writer: the same `ports::canonical_json` and `ports::chain_hash` as the real one, an
//! advisory lock, the head read, the INSERT. It is replaced by `PgAuditLog` in T5; every T2 and T3 test that uses it then
//! runs against the real one.

use async_trait::async_trait;
use domain::AuditId;
use ports::{AuditActor, AuditError, AuditEvent, AuditLog, Tx, canonical_json, chain_hash, genesis_hash};
use sqlx::Row;

/// A key no other part of the schema uses.
const CHAIN_LOCK: i64 = 7_160_001;

#[derive(Debug, Default, Clone, Copy)]
pub struct SimpleChainAudit;

fn actor_ref(a: &AuditActor) -> String {
    match a {
        AuditActor::User { user } => user.to_string(),
        AuditActor::Agent { swimlane } => swimlane.to_string(),
        AuditActor::Sentinel { name } => name.to_string(),
        AuditActor::System { component } => component.to_string(),
    }
}

fn actor_kind(a: &AuditActor) -> &'static str {
    match a {
        AuditActor::User { .. } => "user",
        AuditActor::Agent { .. } => "agent",
        AuditActor::Sentinel { .. } => "sentinel",
        AuditActor::System { .. } => "system",
    }
}

/// The wire name of a serde unit enum.
fn wire<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[async_trait]
impl AuditLog for SimpleChainAudit {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        if e.diff
            .as_ref()
            .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
        {
            return Err(AuditError::TooLarge);
        }
        let conn = tx.pg().map_err(|_| AuditError::WrongTx)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(CHAIN_LOCK)
            .execute(&mut *conn)
            .await
            .map_err(|_| AuditError::Unavailable)?;
        let head: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1")
                .fetch_optional(&mut *conn)
                .await
                .map_err(|_| AuditError::Unavailable)?;
        let prev = match head {
            Some(bytes) => {
                domain::ContentHash::from_bytes(bytes.try_into().map_err(|_| AuditError::Unavailable)?)
            }
            None => genesis_hash(),
        };
        let json = canonical_json(&e)?;
        let hash = chain_hash(&prev, &json);
        let row = sqlx::query(
            "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via, swimlane_id, path, github_login, request_id) \
             VALUES ($1, $2, $3, to_timestamp($4::bigint / 1000.0), $5, $6, $7, $8, $9, $10, $11, $12) RETURNING seq",
        )
        .bind(prev.as_bytes().as_slice())
        .bind(hash.as_bytes().as_slice())
        .bind(String::from_utf8(json).map_err(|_| AuditError::Encode)?)
        .bind(e.at.unix_millis())
        .bind(actor_kind(&e.actor))
        .bind(Some(actor_ref(&e.actor)))
        .bind(wire(&e.action))
        .bind(wire(&e.via))
        .bind(e.swimlane.as_ref().map(ToString::to_string))
        .bind(e.path.as_ref().map(ToString::to_string))
        .bind(e.github_login.as_ref().map(ToString::to_string))
        .bind(e.request_id.as_ref().map(ToString::to_string))
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| AuditError::Unavailable)?;
        let seq: i64 = row.try_get("seq").map_err(|_| AuditError::Unavailable)?;
        Ok(AuditId::from(seq))
    }
}
