//! `Persist` (T3, D80, plan A12, A26): one swimlane's computed state into the database, by difference.
//!
//! A commit is **one transaction** that holds everything the recompute changed for a swimlane, so a reader sees the swimlane before
//! or after it and never half of it:
//!
//! 1. `file_state`: the derived columns of the rows whose signature changed (new rows for the files only Git has), and the rows of
//!    paths that are on neither side any more (only a row that has no file on NFS: a guard in the statement, so a file that came
//!    back since the recompute began is never deleted);
//! 2. `baselines`: a path where NFS and Git converged gets its baseline (after adoption only) with the reference that keeps the raw version
//!    behind it from being collected ([`crate::baselines`]), and the stale marks are deleted;
//! 3. `findings`, by fingerprint ([`super::findings_sync`]);
//! 4. `effective_tree_hashes` and `effective_files` ([`TreeHashStore`]);
//! 5. `swimlane_registry`: the version a read's `ETag` uses, the counts per state, the number of open findings and the fingerprint of the
//!    rules this state was computed under;
//! 6. the events, in the outbox in this transaction (D80: nothing is fanned out before the commit), as one batch
//!    (`publish_many_in_tx`, [`crate::batch`]): `DriftChanged` for the paths whose row changed, in chunks of [`DRIFT_EVENT_PATHS`], at
//!    most [`MAX_DRIFT_EVENTS`] per commit, then `FindingsChanged` (also when the paths did not all fit, so that clients refetch the
//!    swimlane instead of waiting for what was never sent);
//! 7. the audit events, last, because the chain's lock is held until the commit, as one batch (`record_many`): `BaselineAdopted` once
//!    for the commit with the paths in its bounded `diff` (A12: a batch of convergences is one decision of the system), and
//!    `DriftMarkCleared` for each mark that no longer holds (rule 8: one audit event for each state change, in the order of the marks).
//!
//! The derived columns, the findings and the tree are all reproducible from the source facts and Git, which is what the nightly job
//! and the reconcile rely on; the baselines and marks are decisions, which is why they are audited.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use domain::{
    CommitId, ContentHash, DomainEvent, DriftState, FileClass, LogicalFile, NfsPath, RepoKind, ShortText,
    SwimlaneId, Timestamp,
};
use engine::LaneState;
use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, EventBus, Tx};
use serde_json::Value;

use super::findings_sync::{FindingRow, FindingsDelta, engine_kind_names};
use super::lane_store::RowSig;
use crate::baselines::{self, RefChange};
use crate::clock::Clock;
use crate::db::{Db, Txn};
use crate::error::RegistryError;
use crate::index::{TreeHashStore, TreeWrite};
use crate::metrics::RegistryMetrics;

/// Paths in one `DriftChanged` (A26). The outbox payload is bounded, so is the list.
pub(crate) const DRIFT_EVENT_PATHS: usize = 500;
/// `DriftChanged` events in one commit (A26). Past it the swimlane is refetched.
pub(crate) const MAX_DRIFT_EVENTS: usize = 20;
/// Paths listed in the `diff` of one `BaselineAdopted` event, and the bytes of it (the audit log takes 64 KiB).
const MAX_LISTED_PATHS: usize = 1_000;
const MAX_LIST_BYTES: usize = 48 * 1024;

/// The component named as the actor of what the pipeline decides.
const COMPONENT: &str = "registry-pipeline";

/// The statements, as constants so that the EXPLAIN snapshot test takes exactly what runs (P12).
pub const UPSERT_ROWS_SQL: &str = "\
INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, role, git_hash, baseline_hash, state, denied, updated_at) \
SELECT $1, x.path, x.logical, x.kind, x.class, x.role, x.git, x.base, x.state, x.denied, \
       (timestamptz 'epoch' + ($10::bigint::text || ' milliseconds')::interval) \
FROM unnest($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::bytea[], $8::bytea[], $9::text[], $11::boolean[]) \
     AS x(path, logical, kind, class, role, git, base, state, denied) \
ON CONFLICT (swimlane_id, path) DO UPDATE SET \
  logical_file = EXCLUDED.logical_file, kind = EXCLUDED.kind, role = EXCLUDED.role, git_hash = EXCLUDED.git_hash, \
  baseline_hash = EXCLUDED.baseline_hash, state = EXCLUDED.state, updated_at = EXCLUDED.updated_at";

pub const DELETE_ROWS_SQL: &str =
    "DELETE FROM file_state WHERE swimlane_id = $1 AND path = ANY($2::text[]) AND nfs_hash IS NULL";

pub const UPSERT_BASELINES_SQL: &str = "\
INSERT INTO baselines (swimlane_id, path, hash, repo, branch, commit_id, adopted_at, blob_hash) \
SELECT $1, x.path, x.hash, x.repo, x.branch, x.commit, \
       (timestamptz 'epoch' + ($8::bigint::text || ' milliseconds')::interval), x.blob \
FROM unnest($2::text[], $3::bytea[], $4::text[], $5::text[], $6::text[], $7::bytea[]) AS x(path, hash, repo, branch, commit, blob) \
ON CONFLICT (swimlane_id, path) DO UPDATE SET \
  hash = EXCLUDED.hash, repo = EXCLUDED.repo, branch = EXCLUDED.branch, commit_id = EXCLUDED.commit_id, \
  adopted_at = EXCLUDED.adopted_at, adopted_by_tid = NULL, adopted_by_oid = NULL, blob_hash = EXCLUDED.blob_hash";

pub const CLEAR_MARKS_SQL: &str =
    "DELETE FROM intentional_divergence WHERE swimlane_id = $1 AND path = ANY($2::text[])";

pub const UPSERT_FINDINGS_SQL: &str = "\
INSERT INTO findings (swimlane_id, kind, path, severity, rule_ids, message, locations, details, fingerprint, detected_at) \
SELECT $1, x.kind, x.path, x.sev, ARRAY(SELECT jsonb_array_elements_text(x.rules::jsonb)), x.msg, x.loc::jsonb, x.det::jsonb, x.fp, \
       (timestamptz 'epoch' + ($10::bigint::text || ' milliseconds')::interval) \
FROM unnest($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], $8::text[], $9::text[]) \
     AS x(kind, path, sev, rules, msg, loc, det, fp) \
ON CONFLICT (swimlane_id, kind, fingerprint) WHERE resolved_at IS NULL DO UPDATE SET \
  path = EXCLUDED.path, severity = EXCLUDED.severity, rule_ids = EXCLUDED.rule_ids, message = EXCLUDED.message, \
  locations = EXCLUDED.locations, details = EXCLUDED.details";

pub const RESOLVE_FINDINGS_SQL: &str = "\
UPDATE findings AS f SET resolved_at = (timestamptz 'epoch' + ($4::bigint::text || ' milliseconds')::interval) \
FROM unnest($2::text[], $3::text[]) AS x(k, fp) \
WHERE f.swimlane_id = $1 AND f.resolved_at IS NULL AND f.kind = x.k AND f.fingerprint = x.fp";

/// The open findings of the engine's kinds of one swimlane: what a full load compares its result with.
pub const OPEN_FINDINGS_SQL: &str = "\
SELECT kind, path, severity, rule_ids, message, locations, details, fingerprint FROM findings \
WHERE swimlane_id = $1 AND resolved_at IS NULL AND kind = ANY($2::text[]) ORDER BY kind, fingerprint LIMIT $3";

pub const REGISTRY_SQL: &str = "\
UPDATE swimlane_registry SET version = version + $2::bigint, state_counts = $3::jsonb, finding_count = $4, \
  rules_fingerprint = COALESCE($5::bytea, rules_fingerprint), \
  recomputed_at = (timestamptz 'epoch' + ($6::bigint::text || ' milliseconds')::interval), \
  updated_at = (timestamptz 'epoch' + ($6::bigint::text || ' milliseconds')::interval) \
WHERE swimlane_id = $1";

/// The most findings of one swimlane read back.
pub(crate) const MAX_FINDINGS: i64 = 100_000;

/// A row to write.
#[derive(Debug, Clone)]
pub(crate) struct RowWrite {
    pub path: NfsPath,
    pub sig: RowSig,
}

/// A baseline to set because NFS and Git agree now.
#[derive(Debug, Clone)]
pub(crate) struct BaselineWrite {
    pub path: NfsPath,
    /// The hash for comparing.
    pub hash: ContentHash,
    pub repo: RepoKind,
    /// The tenant branch, or `None` for the base repository's default branch.
    pub branch: Option<String>,
    pub commit: CommitId,
    /// The raw version behind the baseline. A denied file has none: its content is never stored.
    pub blob: Option<ContentHash>,
}

/// How the effective tables follow the state.
#[derive(Debug, Clone)]
pub(crate) enum TreeMode {
    /// Only these logical files and the directories above them can have changed.
    Apply(Vec<LogicalFile>),
    /// Make the tables equal to the state: read what is stored, write the difference.
    Sync,
}

/// Everything one swimlane's recompute changed.
pub(crate) struct LaneCommit<'a> {
    pub swimlane: &'a SwimlaneId,
    pub state: &'a LaneState,
    pub writes: Vec<RowWrite>,
    pub deletes: Vec<NfsPath>,
    pub baselines: Vec<BaselineWrite>,
    /// Marks that no longer hold, with the hash the file had when they were set (for the audit event).
    pub mark_clears: Vec<NfsPath>,
    /// The paths named in `DriftChanged`.
    pub drift_paths: Vec<NfsPath>,
    pub findings: FindingsDelta,
    pub tree: TreeMode,
    pub counts: BTreeMap<DriftState, u64>,
    pub finding_count: usize,
    /// Store the rules the state was computed under (the first time, and when they changed).
    pub fingerprint: Option<ContentHash>,
}

/// What a commit did, in counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommitReport {
    pub rows_written: usize,
    pub rows_deleted: usize,
    pub drift_paths: usize,
    pub drift_events: usize,
    pub findings_written: usize,
    pub baselines: usize,
    pub marks_cleared: usize,
    pub tree: TreeWrite,
    /// A transaction was opened and committed.
    pub committed: bool,
}

impl CommitReport {
    /// Something other than the registry row changed.
    pub(crate) fn changed(&self) -> bool {
        self.rows_written > 0
            || self.rows_deleted > 0
            || self.findings_written > 0
            || self.baselines > 0
            || self.marks_cleared > 0
            || !self.tree.is_empty()
    }
}

#[derive(Clone)]
pub(crate) struct Persist {
    db: Db,
    bus: Arc<dyn EventBus>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
    tree: TreeHashStore,
    metrics: RegistryMetrics,
    rows: usize,
}

impl std::fmt::Debug for Persist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Persist").finish_non_exhaustive()
    }
}

fn raw(hash: Option<ContentHash>) -> Option<Vec<u8>> {
    hash.map(|h| h.as_bytes().to_vec())
}

fn json_text(value: &Value) -> String {
    value.to_string()
}

impl Persist {
    pub(crate) fn new(
        db: Db,
        bus: Arc<dyn EventBus>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        tree: TreeHashStore,
        metrics: RegistryMetrics,
        rows: usize,
    ) -> Self {
        Self {
            db,
            bus,
            audit,
            clock,
            tree,
            metrics,
            rows: rows.max(1),
        }
    }

    /// The open findings of the engine's kinds of a swimlane, as the table holds them.
    pub(crate) async fn open_findings(
        &self,
        swimlane: &SwimlaneId,
    ) -> Result<Vec<super::findings_sync::StoredFinding>, RegistryError> {
        self.db
            .run(
                "findings_open",
                sqlx::query_as(OPEN_FINDINGS_SQL)
                    .bind(swimlane.as_str())
                    .bind(engine_kind_names())
                    .bind(MAX_FINDINGS)
                    .fetch_all(self.db.pool()),
            )
            .await
    }

    /// Write one swimlane's changes in one transaction. A commit with nothing to write opens no transaction.
    pub(crate) async fn commit(&self, c: LaneCommit<'_>) -> Result<CommitReport, RegistryError> {
        let nothing = c.writes.is_empty()
            && c.deletes.is_empty()
            && c.baselines.is_empty()
            && c.mark_clears.is_empty()
            && c.drift_paths.is_empty()
            && c.findings.is_empty()
            && c.fingerprint.is_none()
            && matches!(&c.tree, TreeMode::Apply(t) if t.is_empty());
        if nothing {
            return Ok(CommitReport::default());
        }
        let now_ms = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;
        let mut report = CommitReport {
            rows_written: c.writes.len(),
            rows_deleted: 0,
            drift_paths: c.drift_paths.len(),
            baselines: c.baselines.len(),
            marks_cleared: c.mark_clears.len(),
            committed: true,
            ..CommitReport::default()
        };

        for chunk in c.writes.chunks(self.rows) {
            self.upsert_rows(&mut tx, c.swimlane, chunk, now_ms).await?;
        }
        for chunk in c.deletes.chunks(self.rows) {
            report.rows_deleted += self.delete_rows(&mut tx, c.swimlane, chunk).await?;
        }
        for chunk in c.baselines.chunks(self.rows) {
            self.upsert_baselines(&mut tx, c.swimlane, chunk, now_ms).await?;
        }
        for chunk in c.mark_clears.chunks(self.rows) {
            self.clear_marks(&mut tx, c.swimlane, chunk).await?;
        }
        report.findings_written = c.findings.total();
        self.write_findings(&mut tx, c.swimlane, &c.findings, now_ms)
            .await?;
        report.tree = match &c.tree {
            TreeMode::Apply(touched) => self.tree.apply(&mut tx, c.state, touched).await?,
            TreeMode::Sync => self.tree.sync(&mut tx, c.state).await?,
        };
        let bump = i64::from(report.changed());
        let counts: serde_json::Map<String, Value> = c
            .counts
            .iter()
            .map(|(state, n)| (state.as_str().to_owned(), Value::from(*n)))
            .collect();
        self.db
            .run(
                "registry_row",
                sqlx::query(REGISTRY_SQL)
                    .bind(c.swimlane.as_str())
                    .bind(bump)
                    .bind(json_text(&Value::Object(counts)))
                    .bind(i32::try_from(c.finding_count).unwrap_or(i32::MAX))
                    .bind(c.fingerprint.map(|f| f.as_bytes().to_vec()))
                    .bind(now_ms)
                    .execute(&mut *tx),
            )
            .await?;

        report.drift_events = self
            .publish_events(&mut tx, c.swimlane, &c.drift_paths, &c.findings)
            .await?;
        self.audit(&mut tx, c.swimlane, &c.baselines, &c.mark_clears, now_ms)
            .await?;
        self.db.commit(tx).await?;

        self.metrics
            .pipeline_rows((report.rows_written + report.rows_deleted) as u64);
        self.metrics.pipeline_findings(report.findings_written as u64);
        self.metrics.pipeline_drift_paths(report.drift_paths as u64);
        self.metrics.pipeline_baselines(report.baselines as u64);
        self.metrics.pipeline_marks_cleared(report.marks_cleared as u64);
        Ok(report)
    }

    async fn upsert_rows(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        rows: &[RowWrite],
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        let logical: Vec<&str> = rows.iter().map(|r| r.sig.logical.as_str()).collect();
        let kinds: Vec<&str> = rows.iter().map(|r| r.sig.kind.as_str()).collect();
        let classes: Vec<&str> = rows.iter().map(|r| r.sig.class.as_str()).collect();
        let roles: Vec<&str> = rows.iter().map(|r| r.sig.role.as_str()).collect();
        let git: Vec<Option<Vec<u8>>> = rows.iter().map(|r| raw(r.sig.git)).collect();
        let base: Vec<Option<Vec<u8>>> = rows.iter().map(|r| raw(r.sig.baseline)).collect();
        let states: Vec<&str> = rows.iter().map(|r| r.sig.state.as_str()).collect();
        let denied: Vec<bool> = rows.iter().map(|r| r.sig.class == FileClass::Denied).collect();
        self.db
            .run(
                "persist_rows",
                sqlx::query(UPSERT_ROWS_SQL)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(&logical)
                    .bind(&kinds)
                    .bind(&classes)
                    .bind(&roles)
                    .bind(&git)
                    .bind(&base)
                    .bind(&states)
                    .bind(now_ms)
                    .bind(&denied)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }

    async fn delete_rows(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[NfsPath],
    ) -> Result<usize, RegistryError> {
        let names: Vec<&str> = paths.iter().map(NfsPath::as_str).collect();
        let done = self
            .db
            .run(
                "persist_delete_rows",
                sqlx::query(DELETE_ROWS_SQL)
                    .bind(swimlane.as_str())
                    .bind(&names)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(usize::try_from(done.rows_affected()).unwrap_or(usize::MAX))
    }

    async fn upsert_baselines(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        rows: &[BaselineWrite],
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        let hashes: Vec<Vec<u8>> = rows.iter().map(|r| r.hash.as_bytes().to_vec()).collect();
        let repos: Vec<&str> = rows
            .iter()
            .map(|r| match r.repo {
                RepoKind::Base => "base",
                RepoKind::Tenant => "tenant",
            })
            .collect();
        let branches: Vec<Option<&str>> = rows.iter().map(|r| r.branch.as_deref()).collect();
        let commits: Vec<&str> = rows.iter().map(|r| r.commit.as_str()).collect();
        let blobs: Vec<Option<Vec<u8>>> = rows.iter().map(|r| raw(r.blob)).collect();
        // The raw versions the baselines stand on now, read before they are replaced (A24, D78).
        let before = baselines::current_blobs(&self.db, tx, swimlane, &paths).await?;
        self.db
            .run(
                "persist_baselines",
                sqlx::query(UPSERT_BASELINES_SQL)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(&hashes)
                    .bind(&repos)
                    .bind(&branches)
                    .bind(&commits)
                    .bind(&blobs)
                    .bind(now_ms)
                    .execute(&mut **tx),
            )
            .await?;
        // The reference that keeps the raw version behind each baseline from being collected, and the one of the version it replaced
        // dropped: the same rule as a sync report's baselines (`record_sync`), through the same code.
        let changes: Vec<RefChange> = rows
            .iter()
            .zip(blobs)
            .map(|(r, new)| RefChange {
                path: r.path.as_str().to_owned(),
                old: before.get(r.path.as_str()).cloned(),
                new,
            })
            .collect();
        baselines::replace_refs(&self.db, tx, swimlane, &changes).await
    }

    async fn clear_marks(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[NfsPath],
    ) -> Result<(), RegistryError> {
        let names: Vec<&str> = paths.iter().map(NfsPath::as_str).collect();
        self.db
            .run(
                "persist_clear_marks",
                sqlx::query(CLEAR_MARKS_SQL)
                    .bind(swimlane.as_str())
                    .bind(&names)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }

    async fn write_findings(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        delta: &FindingsDelta,
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let upserts: Vec<&FindingRow> = delta.insert.iter().chain(&delta.update).collect();
        for chunk in upserts.chunks(self.rows) {
            let kinds: Vec<&str> = chunk.iter().map(|r| r.kind.as_str()).collect();
            let paths: Vec<Option<&str>> = chunk.iter().map(|r| r.path.as_deref()).collect();
            let severities: Vec<&str> = chunk.iter().map(|r| r.severity.as_str()).collect();
            let rules: Vec<String> = chunk
                .iter()
                .map(|r| serde_json::to_string(&r.rule_ids).unwrap_or_else(|_| "[]".to_owned()))
                .collect();
            let messages: Vec<&str> = chunk.iter().map(|r| r.message.as_str()).collect();
            let locations: Vec<String> = chunk.iter().map(|r| json_text(&r.locations)).collect();
            let details: Vec<String> = chunk.iter().map(|r| json_text(&r.details)).collect();
            let fingerprints: Vec<&str> = chunk.iter().map(|r| r.fingerprint.as_str()).collect();
            self.db
                .run(
                    "persist_findings",
                    sqlx::query(UPSERT_FINDINGS_SQL)
                        .bind(swimlane.as_str())
                        .bind(&kinds)
                        .bind(&paths)
                        .bind(&severities)
                        .bind(&rules)
                        .bind(&messages)
                        .bind(&locations)
                        .bind(&details)
                        .bind(&fingerprints)
                        .bind(now_ms)
                        .execute(&mut **tx),
                )
                .await?;
        }
        for chunk in delta.resolve.chunks(self.rows) {
            let kinds: Vec<&str> = chunk.iter().map(|(k, _)| k.as_str()).collect();
            let fingerprints: Vec<&str> = chunk.iter().map(|(_, f)| f.as_str()).collect();
            self.db
                .run(
                    "persist_resolve_findings",
                    sqlx::query(RESOLVE_FINDINGS_SQL)
                        .bind(swimlane.as_str())
                        .bind(&kinds)
                        .bind(&fingerprints)
                        .bind(now_ms)
                        .execute(&mut **tx),
                )
                .await?;
        }
        Ok(())
    }

    /// The events of the commit, in the commit's transaction. Returns how many `DriftChanged` there were.
    async fn publish_events(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[NfsPath],
        findings: &FindingsDelta,
    ) -> Result<usize, RegistryError> {
        let mut events: Vec<DomainEvent> = Vec::new();
        let mut dropped = false;
        for chunk in paths.chunks(DRIFT_EVENT_PATHS) {
            if events.len() == MAX_DRIFT_EVENTS {
                dropped = true;
                break;
            }
            events.push(DomainEvent::DriftChanged {
                swimlane: swimlane.clone(),
                paths: chunk.to_vec(),
            });
        }
        let sent = events.len();
        // The paths that did not fit are still news: the swimlane is refetched, which is what this event says.
        if dropped || !findings.is_empty() {
            events.push(DomainEvent::FindingsChanged {
                swimlane: swimlane.clone(),
            });
        }
        // All of them, in order, as one batch (P15): a commit has one `DriftChanged` for each 500 paths and at most 21 events.
        let mut handle = Tx::from_pg(tx);
        crate::batch::publish_all(&*self.bus, &mut handle, events).await?;
        Ok(sent)
    }

    async fn audit(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        baselines: &[BaselineWrite],
        marks: &[NfsPath],
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let actor = || {
            ShortText::parse(COMPONENT)
                .map(|component| AuditActor::System { component })
                .map_err(|_| RegistryError::Invalid)
        };
        let mut events: Vec<AuditEvent> = Vec::with_capacity(1 + marks.len());
        if !baselines.is_empty() {
            let mut event = AuditEvent::new(
                Timestamp::from_unix_millis(now_ms),
                actor()?,
                AuditAction::BaselineAdopted,
                AuditVia::System,
            );
            event.swimlane = Some(swimlane.clone());
            event.diff = Some(listed(baselines.iter().map(|b| b.path.as_str())));
            events.push(event);
        }
        for path in marks {
            let mut event = AuditEvent::new(
                Timestamp::from_unix_millis(now_ms),
                actor()?,
                AuditAction::DriftMarkCleared,
                AuditVia::System,
            );
            event.swimlane = Some(swimlane.clone());
            event.path = Some(path.clone());
            events.push(event);
        }
        // One event for each decision (rule 8), recorded as one batch (P15): one chain lock and one insert for all of them.
        let mut handle = Tx::from_pg(tx);
        crate::batch::record_all(&*self.audit, &mut handle, events).await
    }
}

/// The paths of one decision, one per line, within the audit log's bound: how many there are, then as many as fit.
pub(crate) fn listed<'a>(paths: impl ExactSizeIterator<Item = &'a str>) -> String {
    let total = paths.len();
    let mut out = format!("{total} paths converged\n");
    let mut shown = 0;
    for path in paths {
        if shown == MAX_LISTED_PATHS || out.len() + path.len() + 1 > MAX_LIST_BYTES {
            break;
        }
        out.push_str(path);
        out.push('\n');
        shown += 1;
    }
    if shown < total {
        let _ = writeln!(out, "... and {} more", total - shown);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listed_paths_are_counted_and_bounded() {
        let few = listed(["a.yml", "b/c.yml"].into_iter());
        assert_eq!(few, "2 paths converged\na.yml\nb/c.yml\n");
        let many: Vec<String> = (0..5_000).map(|i| format!("dir/file-{i}.yml")).collect();
        let text = listed(many.iter().map(String::as_str));
        assert!(text.starts_with("5000 paths converged\n"));
        assert!(
            text.len() <= ports::AuditEvent::MAX_DIFF_BYTES,
            "{} bytes",
            text.len()
        );
        assert!(text.ends_with("more\n"), "the rest is counted, not listed");
        assert!(text.lines().count() <= MAX_LISTED_PATHS + 2);
    }

    #[test]
    fn a_long_path_list_stops_at_the_byte_bound() {
        let long: Vec<String> = (0..200).map(|i| format!("{}{i}", "x".repeat(1000))).collect();
        let text = listed(long.iter().map(String::as_str));
        assert!(text.len() <= MAX_LIST_BYTES + 64, "{} bytes", text.len());
        assert!(text.contains(" more"));
    }

    #[test]
    fn the_event_bounds_are_the_plan_s() {
        assert_eq!(DRIFT_EVENT_PATHS, 500, "A26: chunks of 500 paths");
        assert_eq!(MAX_DRIFT_EVENTS, 20, "A26: at most 20 per batch and swimlane");
    }

    #[test]
    fn the_statements_name_every_bind_they_use() {
        // A statement that uses $n must be given n binds; the numbers below are what `persist` passes.
        for (sql, last) in [
            (UPSERT_ROWS_SQL, 11),
            (DELETE_ROWS_SQL, 2),
            (UPSERT_BASELINES_SQL, 8),
            (CLEAR_MARKS_SQL, 2),
            (UPSERT_FINDINGS_SQL, 10),
            (RESOLVE_FINDINGS_SQL, 4),
            (REGISTRY_SQL, 6),
            (OPEN_FINDINGS_SQL, 3),
        ] {
            assert!(sql.contains(&format!("${last}")), "{sql}");
            assert!(!sql.contains(&format!("${}", last + 1)), "{sql}");
        }
        assert!(!UPSERT_ROWS_SQL.contains("OFFSET") && !OPEN_FINDINGS_SQL.contains("OFFSET"));
    }
}
