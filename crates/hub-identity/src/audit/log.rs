//! `PgAuditLog`: the hash-chained audit log over the caller's Postgres transaction (S9, D38, D56).
//!
//! [`AuditLog::record`] runs, inside the caller's transaction:
//!
//! 1. the size check, before anything else (an oversized event never waits for the lock);
//! 2. `pg_advisory_xact_lock`, one lock key for the whole chain, released when the transaction ends;
//! 3. the head of the chain: `SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1` (a backward primary-key scan), or the
//!    genesis hash when the log is empty. This is its own statement: under READ COMMITTED each statement sees what the previous
//!    holder of the lock committed, which a single statement that also took the lock would not;
//! 4. `hash = SHA-256(prev_hash || canonical JSON)` with [`ports::canonical_json`] and [`ports::chain_hash`], the one definition
//!    that the table's `CHECK`, the nightly verifier and the fakes share;
//! 5. the `INSERT`, with the columns that copy the event for querying.
//!
//! # Call `record` last
//!
//! The lock is held until the caller commits or rolls back, so every audited transaction is serialised from `record` to its end.
//! Call `record` as the last statement before `COMMIT`, and do no slow work (a network call, a file write) after it. That is
//! what keeps the cost of the chain to a few milliseconds at the design scale, and it is checked by the contended benchmark
//! `hub-identity/audit_record` (budget `P12.audit_record`).
//!
//! # Failure
//!
//! A wait for the lock longer than the lock timeout (5 s by default) fails the call with [`AuditError::Unavailable`], and the
//! caller rolls back: the action does not happen without its audit event. The timeout is set for the rest of the transaction.
//! Under REPEATABLE READ or SERIALIZABLE the head read can be stale, and the table's `UNIQUE (prev_hash)` refuses the fork; the
//! call fails the same way. Callers run at READ COMMITTED.
//!
//! No error and no log line carries event content, only the stage and the SQLSTATE (S10, S21).

use std::time::Duration;

use async_trait::async_trait;
use domain::{AuditId, ContentHash};
use ports::{AuditActor, AuditError, AuditEvent, AuditLog, Tx, canonical_json, chain_hash, genesis_hash};
use sqlx::Row;

/// The advisory lock that serialises writers of the chain. No other part of the schema takes it. Every replica, of every
/// version, must use this value: it is fixed, and a test pins it.
pub const CHAIN_LOCK_KEY: i64 = 0x004C_4B41_5544_4955;

/// How long a record waits for the chain lock before it gives up.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// The table refuses an `event_json` over this size. The bounded fields of an event cannot reach it; this is the backstop.
const MAX_EVENT_JSON_BYTES: usize = 1024 * 1024;

const INSERT: &str = "INSERT INTO audit_events \
    (prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via, swimlane_id, path, hash_before, hash_after, \
     github_login, approval_id, request_id, attribution_source, attribution_confidence, attribution_actor, attribution_evidence) \
    VALUES ($1, $2, $3, timestamptz 'epoch' + $4::bigint * interval '1 millisecond', $5, $6, $7, $8, $9, $10, $11, $12, \
            $13, $14, $15, $16, $17, $18, $19) \
    RETURNING seq";

/// An event turned into what the table stores: the canonical JSON that is hashed, and the columns copied from the event.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    json: String,
    at_millis: i64,
    actor_kind: &'static str,
    actor_ref: String,
    action: String,
    via: String,
    swimlane: Option<String>,
    path: Option<String>,
    hash_before: Option<ContentHash>,
    hash_after: Option<ContentHash>,
    github_login: Option<String>,
    approval: Option<i64>,
    request_id: Option<String>,
    attribution_source: Option<&'static str>,
    attribution_confidence: Option<&'static str>,
    attribution_actor: Option<serde_json::Value>,
    attribution_evidence: Option<serde_json::Value>,
}

impl Prepared {
    /// The canonical JSON, exactly the text that is hashed and stored.
    pub fn json(&self) -> &str {
        &self.json
    }

    /// `SHA-256(prev || json)`.
    pub fn hash_after(&self, prev: &ContentHash) -> ContentHash {
        chain_hash(prev, self.json.as_bytes())
    }
}

/// The wire name of a serde unit enum.
fn wire<T: serde::Serialize>(v: &T) -> Result<String, AuditError> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        _ => Err(AuditError::Encode),
    }
}

/// Check and encode an event, without touching the database.
///
/// # Errors
/// [`AuditError::TooLarge`] when the diff is over [`AuditEvent::MAX_DIFF_BYTES`] (or the whole event over the table's limit),
/// [`AuditError::Encode`] when it cannot be turned into JSON.
pub fn prepare(e: &AuditEvent) -> Result<Prepared, AuditError> {
    if e.diff
        .as_ref()
        .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
    {
        return Err(AuditError::TooLarge);
    }
    let json = String::from_utf8(canonical_json(e)?).map_err(|_| AuditError::Encode)?;
    if json.len() > MAX_EVENT_JSON_BYTES {
        return Err(AuditError::TooLarge);
    }
    let (actor_kind, actor_ref) = match &e.actor {
        AuditActor::User { user } => ("user", user.to_string()),
        AuditActor::Agent { swimlane } => ("agent", swimlane.to_string()),
        AuditActor::Sentinel { name } => ("sentinel", name.to_string()),
        AuditActor::System { component } => ("system", component.to_string()),
    };
    let attribution = e.attribution.as_ref();
    Ok(Prepared {
        json,
        at_millis: e.at.unix_millis(),
        actor_kind,
        actor_ref,
        action: wire(&e.action)?,
        via: wire(&e.via)?,
        swimlane: e.swimlane.as_ref().map(ToString::to_string),
        path: e.path.as_ref().map(ToString::to_string),
        hash_before: e.hash_before,
        hash_after: e.hash_after,
        github_login: e.github_login.as_ref().map(ToString::to_string),
        approval: e.approval.map(domain::ProposalId::get),
        request_id: e.request_id.as_ref().map(ToString::to_string),
        attribution_source: attribution.map(|a| a.source.as_str()),
        attribution_confidence: attribution.map(|a| a.confidence.as_str()),
        attribution_actor: attribution
            .and_then(|a| a.actor.as_ref())
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| AuditError::Encode)?,
        attribution_evidence: attribution
            .map(|a| serde_json::to_value(&a.evidence))
            .transpose()
            .map_err(|_| AuditError::Encode)?,
    })
}

/// Log the stage and SQLSTATE of a database failure, and turn it into the port's error. Nothing from the message is kept: it
/// can quote a row.
fn db_error(stage: &'static str, e: &sqlx::Error) -> AuditError {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned));
    if code.as_deref() == Some("23505") {
        tracing::error!(stage, "audit chain fork refused by the table");
    } else {
        tracing::warn!(
            stage,
            sqlstate = code.as_deref().unwrap_or("none"),
            "audit record failed"
        );
    }
    AuditError::Unavailable
}

/// The audit log over Postgres. Stateless: all state is in the table and the caller's transaction.
#[derive(Debug, Clone, Copy)]
pub struct PgAuditLog {
    lock_timeout: Duration,
}

impl Default for PgAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl PgAuditLog {
    pub const fn new() -> Self {
        Self {
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Wait at most `timeout` for the chain lock (rounded down to whole milliseconds, at least one).
    #[must_use]
    pub const fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }
}

#[async_trait]
impl AuditLog for PgAuditLog {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        // 1. Before the lock: an event that cannot be stored must not make anyone wait.
        let p = prepare(&e)?;
        let conn = tx.pg().map_err(|_| AuditError::WrongTx)?;

        // 2. The lock. `set_config(..., true)` lasts to the end of the transaction. The two calls are one round trip.
        let timeout_ms = self.lock_timeout.as_millis().clamp(1, 600_000).to_string();
        sqlx::query("SELECT set_config('lock_timeout', $1, true), pg_advisory_xact_lock($2)")
            .bind(timeout_ms)
            .bind(CHAIN_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(|e| db_error("lock", &e))?;

        // 3. The head, as of now: a statement of its own, so it sees what the previous holder committed.
        let head: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1")
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| db_error("head", &e))?;
        let prev = match head {
            Some(bytes) => ContentHash::from_bytes(<[u8; 32]>::try_from(bytes).map_err(|_| {
                tracing::warn!(stage = "head", "audit head hash has the wrong length");
                AuditError::Unavailable
            })?),
            None => genesis_hash(),
        };

        // 4 and 5.
        let hash = p.hash_after(&prev);
        let row = sqlx::query(INSERT)
            .bind(prev.as_bytes().as_slice())
            .bind(hash.as_bytes().as_slice())
            .bind(&p.json)
            .bind(p.at_millis)
            .bind(p.actor_kind)
            .bind(&p.actor_ref)
            .bind(&p.action)
            .bind(&p.via)
            .bind(&p.swimlane)
            .bind(&p.path)
            .bind(p.hash_before.as_ref().map(|h| h.as_bytes().as_slice()))
            .bind(p.hash_after.as_ref().map(|h| h.as_bytes().as_slice()))
            .bind(&p.github_login)
            .bind(p.approval)
            .bind(&p.request_id)
            .bind(p.attribution_source)
            .bind(p.attribution_confidence)
            .bind(&p.attribution_actor)
            .bind(&p.attribution_evidence)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| db_error("insert", &e))?;
        let seq: i64 = row.try_get("seq").map_err(|e| db_error("insert", &e))?;
        Ok(AuditId::from(seq))
    }
}

#[cfg(test)]
mod tests {
    use domain::{Attribution, ProposalId, ShortText, SwimlaneId, Timestamp};
    use ports::{AuditAction, AuditVia};

    use super::*;

    fn event() -> AuditEvent {
        AuditEvent::new(
            Timestamp::from_unix_millis(1_700_000_000_123),
            AuditActor::System {
                component: ShortText::parse("test").unwrap(),
            },
            AuditAction::SettingsChanged,
            AuditVia::System,
        )
    }

    #[test]
    fn lock_key_is_stable() {
        // Every replica must use the same key; a change would let two versions of the hub write a forked chain.
        assert_eq!(CHAIN_LOCK_KEY, 0x004C_4B41_5544_4955);
    }

    #[test]
    fn prepare_copies_the_columns_from_the_event() {
        let mut e = event();
        e.swimlane = Some(SwimlaneId::parse("sit1").unwrap());
        e.approval = Some(ProposalId::from(42));
        e.attribution = Some(Attribution::unknown());
        let p = prepare(&e).unwrap();
        assert_eq!(p.actor_kind, "system");
        assert_eq!(p.actor_ref, "test");
        assert_eq!(p.action, "settings_changed");
        assert_eq!(p.via, "system");
        assert_eq!(p.swimlane.as_deref(), Some("sit1"));
        assert_eq!(p.approval, Some(42));
        assert_eq!(p.attribution_source, Some("unknown"));
        assert_eq!(p.attribution_confidence, Some("low"));
        assert!(p.attribution_evidence.is_some());
        assert_eq!(p.at_millis, 1_700_000_000_123);
        assert_eq!(p.json.as_bytes(), canonical_json(&e).unwrap().as_slice());
    }

    #[test]
    fn prepare_refuses_an_oversize_diff_without_encoding_it() {
        let mut e = event();
        e.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
        assert_eq!(prepare(&e), Err(AuditError::TooLarge));
        e.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES));
        assert!(prepare(&e).is_ok());
    }

    #[test]
    fn hash_after_is_the_ports_chain_hash() {
        let p = prepare(&event()).unwrap();
        let prev = genesis_hash();
        assert_eq!(p.hash_after(&prev), chain_hash(&prev, p.json().as_bytes()));
    }
}
