//! The real `PgAuditLog` (hub-identity, 03a) behind a thin decorator, for the tests that run a state change on Postgres: the entries are
//! the real chain, written by the real log in the caller's transaction, under the real chain lock. What the decorator adds is the
//! hooks a test needs: it can be told to fail from the Nth event on, or with one given error, and it counts the calls.
//!
//! **It forwards both `record` and `record_many`** to the real log, so a registry that calls `record_many` takes the real one-lock,
//! one-head-read, one-insert path. A decorator that forwarded only `record` would still work, through the default `record_many`, and
//! would silently run the slow loop of one lock, head read and insert per event (P15): the test that counts statements
//! (`tests/batch_writes.rs`) exists to catch that. [`PgTestAudit::per_event`] is that slow loop on purpose: it is the reference of the
//! equivalence test, what the registry did before it batched.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use async_trait::async_trait;
use domain::{AuditId, ContentHash};
use hub_identity::audit::PgAuditLog;
use ports::{AuditError, AuditEvent, AuditLog, Tx};
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgTestAudit {
    inner: PgAuditLog,
    /// `record_many` as the trait's default has it, one real `record` for each event: the registry before it batched.
    per_event: bool,
    /// Events that succeed before every later one fails; negative: never fail.
    fail_after: AtomicI64,
    /// Every call fails with this error, when set.
    fail_with: Mutex<Option<AuditError>>,
    /// Events asked for so far, in `record` and in `record_many` alike (what `fail_after` counts).
    events: AtomicU64,
    singles: AtomicU64,
    batches: AtomicU64,
}

impl Default for PgTestAudit {
    fn default() -> Self {
        Self::new(false)
    }
}

impl PgTestAudit {
    fn new(per_event: bool) -> Self {
        Self {
            inner: PgAuditLog::new(),
            per_event,
            fail_after: AtomicI64::new(-1),
            fail_with: Mutex::new(None),
            events: AtomicU64::new(0),
            singles: AtomicU64::new(0),
            batches: AtomicU64::new(0),
        }
    }

    /// The registry's reference: `record_many` is one `record` for each event, as before the audit log had a batch call.
    pub fn per_event() -> Self {
        Self::new(true)
    }

    /// Events that succeed, then every event fails (a batch that does not fit in what is left fails whole, written nowhere).
    /// `None`: never fail. Resets the count.
    pub fn fail_after(&self, events: Option<u64>) {
        self.fail_after
            .store(events.map_or(-1, |c| i64::try_from(c).unwrap()), Ordering::SeqCst);
        self.events.store(0, Ordering::SeqCst);
    }

    /// Every call fails with `error` until it is `None` again.
    pub fn fail_with(&self, error: Option<AuditError>) {
        *self.fail_with.lock().unwrap() = error;
    }

    /// The events asked for since the last `fail_after`.
    pub fn calls(&self) -> u64 {
        self.events.load(Ordering::SeqCst)
    }

    /// How many calls of `record` and of `record_many` reached the real log.
    pub fn single_calls(&self) -> u64 {
        self.singles.load(Ordering::SeqCst)
    }

    pub fn batch_calls(&self) -> u64 {
        self.batches.load(Ordering::SeqCst)
    }

    /// Count `n` more events, and fail if they do not fit in what `fail_after` allows or if `fail_with` is set.
    fn admit(&self, n: u64) -> Result<(), AuditError> {
        if let Some(error) = *self.fail_with.lock().unwrap() {
            return Err(error);
        }
        let before = self.events.fetch_add(n, Ordering::SeqCst);
        let limit = self.fail_after.load(Ordering::SeqCst);
        if limit >= 0 && i64::try_from(before + n).unwrap() > limit {
            return Err(AuditError::Unavailable);
        }
        Ok(())
    }
}

#[async_trait]
impl AuditLog for PgTestAudit {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        self.admit(1)?;
        self.singles.fetch_add(1, Ordering::SeqCst);
        self.inner.record(tx, e).await
    }

    async fn record_many(
        &self,
        tx: &mut Tx<'_>,
        events: Vec<AuditEvent>,
    ) -> Result<Vec<AuditId>, AuditError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        self.admit(u64::try_from(events.len()).unwrap())?;
        if self.per_event {
            let mut ids = Vec::with_capacity(events.len());
            for e in events {
                self.singles.fetch_add(1, Ordering::SeqCst);
                ids.push(self.inner.record(tx, e).await?);
            }
            return Ok(ids);
        }
        self.batches.fetch_add(1, Ordering::SeqCst);
        self.inner.record_many(tx, events).await
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
