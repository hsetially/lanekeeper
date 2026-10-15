//! Findings written outside a pipeline batch (T10): the structure checks (C9 to C11, [`super::c9_c11`]) and the "as served" verification (C12,
//! [`crate::served`]) both keep a set of findings of their own kinds in the `findings` table, and both do it the same way, here.
//!
//! The writer says which kinds it owns and what the table should hold for them ([`reconcile`]); the difference with what the table holds is
//! written **in one transaction under the swimlane's structure lock**: the rows that are new or changed (upserted by fingerprint, so a finding
//! that stays keeps its `detected_at`), the ones that are gone (resolved), the swimlane's new version (an `ETag` over the swimlane moves) and
//! its count of open findings of every kind, and `FindingsChanged` through the outbox (D80). Nothing is written, and no transaction opened,
//! when the table already says what it should. Findings are derived facts, so there is **no audit event** (D93).
//!
//! The table is read before the lock for the cheap answer "nothing to do" and again under it for the write, because another computation may
//! have written between the two. The desired set is a function of what the table holds (a writer that keeps the findings it did not
//! look at this time needs the stored ones to say which), which is why [`reconcile`] takes a function and not a set.

use std::collections::BTreeMap;

use domain::{DomainEvent, SwimlaneId};
use ports::{EventBus, Tx};

use super::findings_sync::{FindingKey, FindingRow, StoredFinding, diff, row_from_stored};
use super::persist::{MAX_FINDINGS, write_findings};
use crate::batch::publish_all;
use crate::clock::Clock;
use crate::db::{Db, Txn, ts};
use crate::error::RegistryError;

/// The statement that serialises the findings writes of one swimlane. A key of its own: it is never held with the lock of ingestion, the pickup
/// computation or the pipeline's commit.
pub const LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.structure.' || $1, 0))";

/// The open findings of some kinds of one swimlane, one index probe of `findings_open_fingerprint (swimlane_id, kind, fingerprint)` per kind.
/// `$1` the swimlane, `$2` the kinds, `$3` the limit (of the rows returned, and of the rows of each kind).
///
/// The pipeline's own statement (`persist::OPEN_FINDINGS_SQL`) says `kind = ANY(..)`, which leaves the planner a choice between the swimlane's
/// whole list and the index by kind of **every** swimlane, by its estimate of the kinds' frequency: with the structure kinds (a swimlane
/// holds hundreds of unresolved placeholders) it takes the second, which reads the open findings of those kinds of every swimlane. A probe
/// per kind, with the kind taken from a list, has no other way to run: it reads the findings it wants and nothing of another swimlane. The
/// rows are not sorted (the caller keys them by fingerprint), so there is no sort to hold them for.
pub const OPEN_OF_KINDS_SQL: &str = "\
SELECT f.kind, f.path, f.severity, f.rule_ids, f.message, f.locations, f.details, f.fingerprint \
FROM unnest($2::text[]) AS k(kind) \
CROSS JOIN LATERAL ( \
  SELECT kind, path, severity, rule_ids, message, locations, details, fingerprint FROM findings \
  WHERE swimlane_id = $1 AND resolved_at IS NULL AND kind = k.kind ORDER BY fingerprint LIMIT $3 \
) f LIMIT $3";

/// The swimlane's row after a change of its findings: a new version (an `ETag` over the swimlane moves), the count of its open findings of
/// every kind, and the time. `$1` the swimlane, `$2` now (Unix milliseconds). Returns the version.
pub fn registry_row_sql() -> String {
    format!(
        "UPDATE swimlane_registry SET version = version + 1, \
           finding_count = (SELECT count(*) FROM findings WHERE swimlane_id = $1 AND resolved_at IS NULL)::int, \
           updated_at = {now} \
         WHERE swimlane_id = $1 RETURNING version",
        now = ts("$2"),
    )
}

/// What a writer works with.
pub(crate) struct Store<'a> {
    pub db: &'a Db,
    pub bus: &'a dyn EventBus,
    pub clock: &'a dyn Clock,
    /// Rows per statement of a write (`RegistryConfig::rows_per_statement`).
    pub rows: usize,
}

/// What a reconciliation wrote, in counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub inserted: usize,
    pub updated: usize,
    pub resolved: usize,
    /// The version of the swimlane's row after the write, when there was one.
    pub version: Option<i64>,
}

type Rows = BTreeMap<FindingKey, FindingRow>;

fn parse(stored: Vec<StoredFinding>) -> Rows {
    stored
        .into_iter()
        .filter_map(row_from_stored)
        .map(|r| ((r.kind, r.fingerprint.clone()), r))
        .collect()
}

/// The open findings of these kinds of a swimlane, as the table holds them.
pub(crate) async fn stored(db: &Db, swimlane: &SwimlaneId, kinds: &[&str]) -> Result<Rows, RegistryError> {
    let rows: Vec<StoredFinding> = db
        .run(
            "findings_open",
            sqlx::query_as(OPEN_OF_KINDS_SQL)
                .bind(swimlane.as_str())
                .bind(kinds)
                .bind(MAX_FINDINGS)
                .fetch_all(db.pool()),
        )
        .await?;
    Ok(parse(rows))
}

async fn stored_in(
    db: &Db,
    tx: &mut Txn,
    swimlane: &SwimlaneId,
    kinds: &[&str],
) -> Result<Rows, RegistryError> {
    let rows: Vec<StoredFinding> = db
        .run(
            "findings_open",
            sqlx::query_as(OPEN_OF_KINDS_SQL)
                .bind(swimlane.as_str())
                .bind(kinds)
                .bind(MAX_FINDINGS)
                .fetch_all(&mut **tx),
        )
        .await?;
    Ok(parse(rows))
}

/// Make the findings of `kinds` that the table holds for the swimlane say what `desired` says of them (given what the table holds).
pub(crate) async fn reconcile(
    store: &Store<'_>,
    swimlane: &SwimlaneId,
    kinds: &[&str],
    desired: impl Fn(&Rows) -> Rows,
) -> Result<Reconciled, RegistryError> {
    let before = stored(store.db, swimlane, kinds).await?;
    if diff(&before, &desired(&before)).is_empty() {
        return Ok(Reconciled::default());
    }
    let now_ms = store.clock.now().unix_millis();
    let mut tx = store.db.begin().await?;
    store
        .db
        .run(
            "findings_lock",
            sqlx::query(LOCK_SQL).bind(swimlane.as_str()).execute(&mut *tx),
        )
        .await?;
    // Read again under the lock: another computation may have written since.
    let held = stored_in(store.db, &mut tx, swimlane, kinds).await?;
    let delta = diff(&held, &desired(&held));
    if delta.is_empty() {
        return Ok(Reconciled::default());
    }
    write_findings(store.db, &mut tx, swimlane, &delta, now_ms, store.rows).await?;
    let version: i64 = store
        .db
        .run(
            "findings_registry_row",
            sqlx::query_scalar(&registry_row_sql())
                .bind(swimlane.as_str())
                .bind(now_ms)
                .fetch_one(&mut *tx),
        )
        .await?;
    let mut handle = Tx::from_pg(&mut tx);
    publish_all(
        store.bus,
        &mut handle,
        vec![DomainEvent::FindingsChanged {
            swimlane: swimlane.clone(),
        }],
    )
    .await?;
    store.db.commit(tx).await?;
    Ok(Reconciled {
        inserted: delta.insert.len(),
        updated: delta.update.len(),
        resolved: delta.resolve.len(),
        version: Some(version),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_statements_name_every_bind_they_use() {
        assert!(LOCK_SQL.contains("$1") && !LOCK_SQL.contains("$2"));
        assert!(OPEN_OF_KINDS_SQL.contains("$3") && !OPEN_OF_KINDS_SQL.contains("$4"));
        assert!(!OPEN_OF_KINDS_SQL.contains("OFFSET"));
        let row = registry_row_sql();
        assert!(row.contains("$2") && !row.contains("$3") && row.contains("RETURNING version"));
    }
}
