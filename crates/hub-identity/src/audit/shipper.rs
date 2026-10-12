//! The copy of the audit log in Cloud Logging (S9, decision B8).
//!
//! The prompt asks for the event to be emitted "after the commit" as one structured log line. [`ports::AuditLog::record`] cannot know
//! when its caller commits, and a line written before the commit could describe an event that then rolls back. So the copy is
//! made from committed rows: a leased [`AuditLogShipper`] reads events by `seq`, a batch at a time, and writes one line each.
//! A transaction that holds the chain lock commits before any later `seq` can be committed, so reading `seq > last` never skips
//! an event.
//!
//! - **At least once.** On start the shipper resumes from the newest checkpoint's `seq`, so a restart repeats at most an hour of
//!   lines; a replica that takes over the lease does the same. Cloud Logging dedupes by `insertId`, which the line carries as
//!   `audit_event_hash`.
//! - **Lagging by the poll interval** (5 s).
//! - **No content (S10).** The line has the hashes, the ids and the paths of the event. The `diff` text is left out and replaced
//!   by its SHA-256 and length: logs hold hashes, not file content. The table keeps the diff.
//!
//! The lines go to the `tracing` target [`LOG_TARGET`]; the hub's subscriber formats them as JSON for Cloud Logging.

use std::sync::Arc;
use std::time::Duration;

use domain::ContentHash;
use ports::{AuditEvent, LeaseError, LeaseGuard, Leases};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tokio::sync::Mutex;

use super::JobHandle;
use crate::metrics::IdentityMetrics;

/// The `tracing` target of the audit lines.
pub const LOG_TARGET: &str = "lanekeeper::audit";
/// The lease that makes the shipper single-holder.
pub const LEASE_NAME: &str = "audit.shipper";
const LEASE_TTL: Duration = Duration::from_secs(60);
/// How often the table is polled.
pub const POLL_EVERY: Duration = Duration::from_secs(5);
/// Events read per poll unless [`AuditLogShipper::with_batch`] says otherwise.
const DEFAULT_BATCH: i64 = 500;

/// Why a poll failed. No variant carries event content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShipError {
    #[error("lease store failed: {0}")]
    Lease(#[from] LeaseError),
    #[error("the audit table could not be read")]
    Database,
}

/// What one poll did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipOutcome {
    /// Another replica holds the lease.
    NotLeader,
    /// This many lines were written (zero when nothing is new).
    Shipped { lines: usize },
}

struct State {
    lease: Option<LeaseGuard>,
    /// The last `seq` shipped. `None` until the first poll after the lease was taken.
    cursor: Option<i64>,
}

pub struct AuditLogShipper {
    pool: PgPool,
    leases: Arc<dyn Leases>,
    metrics: Arc<IdentityMetrics>,
    batch: i64,
    state: Mutex<State>,
}

impl std::fmt::Debug for AuditLogShipper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLogShipper").finish_non_exhaustive()
    }
}

fn db_error(stage: &'static str, e: &sqlx::Error) -> ShipError {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned));
    tracing::warn!(
        stage,
        sqlstate = code.as_deref().unwrap_or("none"),
        "audit shipper could not read the table"
    );
    ShipError::Database
}

fn hex_digest(text: &str) -> String {
    ContentHash::from_bytes(Sha256::digest(text.as_bytes()).into()).to_string()
}

impl AuditLogShipper {
    pub fn new(pool: PgPool, leases: Arc<dyn Leases>, metrics: Arc<IdentityMetrics>) -> Self {
        Self {
            pool,
            leases,
            metrics,
            batch: DEFAULT_BATCH,
            state: Mutex::new(State {
                lease: None,
                cursor: None,
            }),
        }
    }

    /// Events written per poll (at least one).
    #[must_use]
    pub fn with_batch(mut self, events: u32) -> Self {
        self.batch = i64::from(events.max(1));
        self
    }

    /// Give the lease up, so that another replica can take over at once. Call it on shutdown.
    pub async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        state.cursor = None;
        if let Some(lease) = state.lease.take() {
            if let Err(e) = lease.release().await {
                tracing::debug!(error = %e, "audit shipper lease was not released");
            }
        }
    }

    /// One poll: hold or take the lease, and write a line for each committed event not yet shipped (at most one batch).
    pub async fn tick(&self) -> Result<ShipOutcome, ShipError> {
        let mut state = self.state.lock().await;
        let held = match state.lease.as_mut() {
            Some(lease) if lease.is_valid() => lease.renew(LEASE_TTL).await.is_ok(),
            _ => false,
        };
        if !held {
            state.lease = None;
            match self.leases.try_acquire(LEASE_NAME, LEASE_TTL).await? {
                Some(lease) => {
                    state.lease = Some(lease);
                    // A new holder repeats the last hour rather than risk a gap.
                    state.cursor = None;
                }
                None => return Ok(ShipOutcome::NotLeader),
            }
        }
        let cursor = if let Some(c) = state.cursor {
            c
        } else {
            let from: Option<i64> = sqlx::query_scalar("SELECT max(seq) FROM audit_checkpoints")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| db_error("resume", &e))?;
            from.unwrap_or(0)
        };
        let rows = sqlx::query(
            "SELECT seq, prev_hash, hash, event_json FROM audit_events WHERE seq > $1 ORDER BY seq LIMIT $2",
        )
        .bind(cursor)
        .bind(self.batch)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_error("read", &e))?;
        let mut lines = 0_usize;
        let mut last = cursor;
        for row in rows {
            let seq: i64 = row.try_get("seq").map_err(|e| db_error("read", &e))?;
            let prev: Vec<u8> = row.try_get("prev_hash").map_err(|e| db_error("read", &e))?;
            let hash: Vec<u8> = row.try_get("hash").map_err(|e| db_error("read", &e))?;
            let json: String = row.try_get("event_json").map_err(|e| db_error("read", &e))?;
            last = seq;
            match serde_json::from_str::<AuditEvent>(&json) {
                Ok(event) => {
                    write_line(seq, &hash, &prev, &event);
                    lines += 1;
                }
                // Not a reason to stop: the nightly verification judges the row. The line says only where.
                Err(_) => {
                    tracing::error!(target: LOG_TARGET, seq, "audit event could not be read for shipping");
                }
            }
        }
        state.cursor = Some(last);
        self.metrics
            .lines_shipped(u64::try_from(lines).unwrap_or(u64::MAX));
        Ok(ShipOutcome::Shipped { lines })
    }

    /// Poll every [`POLL_EVERY`] until the handle is dropped.
    pub fn spawn(self: Arc<Self>) -> JobHandle {
        JobHandle::spawn(async move {
            let mut ticker = tokio::time::interval(POLL_EVERY);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(e) = self.tick().await {
                    tracing::warn!(error = %e, "audit shipper poll failed");
                }
            }
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    <[u8; 32]>::try_from(bytes).map_or_else(|_| String::new(), |b| ContentHash::from_bytes(b).to_string())
}

/// One structured line for one event. The diff is replaced by its digest and length.
fn write_line(seq: i64, hash: &[u8], prev: &[u8], e: &AuditEvent) {
    let (diff_sha256, diff_len) = e
        .diff
        .as_deref()
        .map_or((String::new(), 0), |d| (hex_digest(d), d.len()));
    let (actor_kind, actor) = match &e.actor {
        ports::AuditActor::User { user } => ("user", user.to_string()),
        ports::AuditActor::Agent { swimlane } => ("agent", swimlane.to_string()),
        ports::AuditActor::Sentinel { name } => ("sentinel", name.to_string()),
        ports::AuditActor::System { component } => ("system", component.to_string()),
    };
    tracing::info!(
        target: LOG_TARGET,
        audit_event_hash = %hex(hash),
        audit_prev_hash = %hex(prev),
        seq,
        action = ?e.action,
        actor_kind = %actor_kind,
        actor = %actor,
        via = ?e.via,
        at_ms = e.at.unix_millis(),
        swimlane = %e.swimlane.as_ref().map(ToString::to_string).as_deref().unwrap_or(""),
        path = %e.path.as_ref().map(ToString::to_string).as_deref().unwrap_or(""),
        hash_before = %e.hash_before.as_ref().map(ToString::to_string).as_deref().unwrap_or(""),
        hash_after = %e.hash_after.as_ref().map(ToString::to_string).as_deref().unwrap_or(""),
        request_id = %e.request_id.as_ref().map(ToString::to_string).as_deref().unwrap_or(""),
        diff_sha256 = %diff_sha256,
        diff_len,
        "audit event"
    );
}
