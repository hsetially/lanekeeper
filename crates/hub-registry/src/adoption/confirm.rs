//! The confirmation (T5, step 5, D2, D19, D20, D22, rule 8, D80): baselines, marks, the tenant set, and editing unlocked, in one
//! transaction.
//!
//! What it writes, for the plan the admin reviewed (`resolutions`): one baseline for each file that has a Git side at the chosen commits,
//!
//! | File | Baseline | Raw version behind it | Mark |
//! |---|---|---|---|
//! | equal to Git | G (= N) | NFS's | no |
//! | NFS is right | G | Git's, when the registry holds it | no |
//! | Git is right | N | NFS's | no |
//! | Intentional | G | Git's, when the registry holds it | N and G, with the reason |
//!
//! each with the reference that keeps its raw version from being collected ([`crate::baselines`]: the same rule as the pipeline's and the
//! sync report's baselines). A denied file has a baseline and no raw version (D79). The tenant set becomes the chosen one, and for the
//! first pass of a swimlane `editing_unlocked` and `baseline_confirmed_at/by` are set (D2). A tenant-suffix review is on an adopted
//! swimlane already, so it leaves those as they are.
//!
//! **One transaction, in this order** (the order is what keeps the locks short and the result honest):
//!
//! 1. the swimlane's lock, the one every apply of a delta takes, so that no new version of a file arrives in the middle of it;
//! 2. the pass is closed (`confirmed`) with a statement that only an `in_progress` pass at the `confirm` step satisfies: of two
//!    confirmations of the same pass exactly one commits, the other finds it closed;
//! 3. every planned file is read again, and one whose NFS content is not the one the admin reviewed makes the whole confirmation
//!    `Changed` (nothing is written; the transaction is dropped): a decision is only ever applied to what it was made about;
//! 4. the baselines and their references, the marks, the tenant set, and the swimlane's flags;
//! 5. the outbox events (`DriftChanged` for the baselined paths in chunks of 500, at most 20, then `FindingsChanged`, A26);
//! 6. the audit events, **last**, because the chain's lock is held from the first of them to the commit: one `BaselineAdopted`, one
//!    `DriftMarkedIntentional` for each mark, and a `SettingsChanged` when the tenant set changed (D93).
//!
//! After the commit the pipeline is told to recompute the swimlane; what the states become is the pipeline's (`file_state` is derived).

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;

use domain::{
    ContentHash, DomainEvent, NfsPath, RepoKind, SwimlaneId, TenantId, Timestamp, User, VersionLabel,
};
use ports::{AuditAction, AuditEvent, AuditVia, Tx};
use tracing::info;

use super::resolutions::{Entry, Plan, ResolutionKind, Stored};
use super::store::{AdoptionReason, AdoptionStep, Row};
use super::{AdoptionError, Inner, hash_of};
use crate::baselines::{self, RefChange};
use crate::db::{Txn, ts};
use crate::error::RegistryError;
use crate::ingest::apply::LOCK_SQL;
use crate::pipeline::persist::{DRIFT_EVENT_PATHS, MAX_DRIFT_EVENTS};

/// Paths of each kind listed in the `diff` of the `BaselineAdopted` event, and the bytes of the event's text (the audit log takes 64 KiB).
const LISTED_PER_KIND: usize = 200;
const MAX_TEXT_BYTES: usize = 48 * 1024;

/// What a confirmation did, in counts. Nothing in it is content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmOutcome {
    pub adoption: i64,
    pub swimlane: SwimlaneId,
    pub reason: AdoptionReason,
    /// Baselines written: the sum of the four counts after it.
    pub baselines: usize,
    pub equal: usize,
    pub nfs_is_right: usize,
    pub git_is_right: usize,
    pub intentional: usize,
    /// The swimlane's tenants now (the chosen ones).
    pub tenants: Vec<TenantId>,
    /// The chosen set was not the one the swimlane had.
    pub tenants_changed: bool,
    /// The first pass unlocked editing (D2); a review leaves it as it was.
    pub editing_unlocked: bool,
}

/// What is decided for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Equal,
    NfsIsRight,
    GitIsRight,
    Intentional(String),
}

impl Decision {
    /// The resolution this decision came from: `None` for a file that needed none.
    fn kind(&self) -> Option<ResolutionKind> {
        match self {
            Self::Equal => None,
            Self::NfsIsRight => Some(ResolutionKind::NfsIsRight),
            Self::GitIsRight => Some(ResolutionKind::GitIsRight),
            Self::Intentional(_) => Some(ResolutionKind::Intentional),
        }
    }
}

/// A planned file with its decision.
#[derive(Debug, Clone)]
pub(crate) struct Decided<'a> {
    pub entry: &'a Entry,
    pub decision: Decision,
}

impl Decided<'_> {
    /// The hash for drift the baseline holds.
    pub(crate) fn baseline(&self) -> ContentHash {
        match self.decision {
            Decision::GitIsRight => self.entry.nfs,
            Decision::Equal | Decision::NfsIsRight | Decision::Intentional(_) => self.entry.git,
        }
    }

    /// The raw version the baseline would stand on, before the registry looks whether it holds it. A denied file has none.
    pub(crate) fn blob_candidate(&self) -> Option<ContentHash> {
        if self.entry.denied {
            return None;
        }
        match self.decision {
            Decision::Equal | Decision::GitIsRight => Some(self.entry.nfs_raw),
            Decision::NfsIsRight | Decision::Intentional(_) => Some(self.entry.git_raw),
        }
    }
}

/// The decision of every planned file: an equal one needs none, a differing one must have a current resolution.
///
/// # Errors
///
/// `Unresolved(n)` for the `n` differing files without one.
pub(crate) fn decide<'a>(
    plan: &'a Plan,
    stored: &std::collections::BTreeMap<String, Stored>,
) -> Result<Vec<Decided<'a>>, AdoptionError> {
    let mut out: Vec<Decided<'a>> = plan
        .equal
        .iter()
        .map(|entry| Decided {
            entry,
            decision: Decision::Equal,
        })
        .collect();
    let mut unresolved = 0;
    for entry in &plan.differing {
        let Some(found) = stored.get(entry.path.as_str()).filter(|s| s.is_current(entry)) else {
            unresolved += 1;
            continue;
        };
        let decision = match found.kind {
            ResolutionKind::NfsIsRight => Decision::NfsIsRight,
            ResolutionKind::GitIsRight => Decision::GitIsRight,
            // The table only constrains the reason to at most 2,000 characters, so a row without one is read as one with none.
            ResolutionKind::Intentional => Decision::Intentional(found.reason.clone().unwrap_or_default()),
        };
        out.push(Decided { entry, decision });
    }
    if unresolved > 0 {
        return Err(AdoptionError::Unresolved(unresolved));
    }
    out.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
    Ok(out)
}

/// Whether two sets of decisions are the same: the same files, each decided the same way (and for an intentional one, for the same reason).
pub(crate) fn same_decisions(a: &[Decided<'_>], b: &[Decided<'_>]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.entry.path == y.entry.path && x.decision == y.decision)
}

/// The paths whose NFS content is not the one that was planned.
///
/// `current` is `(path, hash for drift)` as read under the lock. A planned file that is not there any more, or whose hash is another,
/// has changed.
pub(crate) fn changed_since<'a>(
    decided: &'a [Decided<'_>],
    current: &std::collections::BTreeMap<String, ContentHash>,
) -> Vec<&'a NfsPath> {
    decided
        .iter()
        .filter(|d| current.get(d.entry.path.as_str()) != Some(&d.entry.nfs))
        .map(|d| &d.entry.path)
        .collect()
}

/// The text of the `BaselineAdopted` event: counts, the tenants, the base commit, and the paths of each kind that took a decision.
fn summary(
    row: &Row,
    label: Option<&VersionLabel>,
    tenants: &[TenantId],
    decided: &[Decided<'_>],
    counts: &Counts,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "adoption {} confirmed ({}) for {}",
        row.id,
        row.reason.as_str(),
        row.swimlane
    );
    let names: Vec<&str> = tenants.iter().map(TenantId::as_str).collect();
    let _ = writeln!(out, "tenants: {}", names.join(", "));
    if let Some(base) = &row.base_commit {
        match label {
            Some(label) => {
                let _ = writeln!(out, "base commit: {base} ({label})");
            }
            None => {
                let _ = writeln!(out, "base commit: {base}");
            }
        }
    }
    let _ = writeln!(
        out,
        "baselines: {} (equal {}, nfs_is_right {}, git_is_right {}, intentional {})",
        counts.total(),
        counts.equal,
        counts.nfs,
        counts.git,
        counts.intentional
    );
    for (title, pick) in [
        ("nfs_is_right", ResolutionKind::NfsIsRight),
        ("git_is_right", ResolutionKind::GitIsRight),
        ("intentional", ResolutionKind::Intentional),
    ] {
        let paths: Vec<&str> = decided
            .iter()
            .filter(|d| d.decision.kind() == Some(pick))
            .map(|d| d.entry.path.as_str())
            .collect();
        if paths.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{title}:");
        for path in paths.iter().take(LISTED_PER_KIND) {
            if out.len() + path.len() + 1 > MAX_TEXT_BYTES {
                break;
            }
            let _ = writeln!(out, "{path}");
        }
        if paths.len() > LISTED_PER_KIND {
            let _ = writeln!(out, "... and {} more", paths.len() - LISTED_PER_KIND);
        }
    }
    out
}

/// How many of each decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub equal: usize,
    pub nfs: usize,
    pub git: usize,
    pub intentional: usize,
}

impl Counts {
    pub(crate) fn of(decided: &[Decided<'_>]) -> Self {
        let mut counts = Self::default();
        for d in decided {
            match d.decision {
                Decision::Equal => counts.equal += 1,
                Decision::NfsIsRight => counts.nfs += 1,
                Decision::GitIsRight => counts.git += 1,
                Decision::Intentional(_) => counts.intentional += 1,
            }
        }
        counts
    }

    pub(crate) fn total(&self) -> usize {
        self.equal + self.nfs + self.git + self.intentional
    }
}

/// The statement that writes a confirmation's baselines: the pipeline's columns, and the admin who adopted them.
pub const ADOPT_BASELINES_SQL: &str = "\
INSERT INTO baselines (swimlane_id, path, hash, repo, branch, commit_id, adopted_at, adopted_by_tid, adopted_by_oid, blob_hash) \
SELECT $1, x.path, x.hash, x.repo, x.branch, x.commit, \
       (timestamptz 'epoch' + ($8::bigint::text || ' milliseconds')::interval), $9, $10, x.blob \
FROM unnest($2::text[], $3::bytea[], $4::text[], $5::text[], $6::text[], $7::bytea[]) AS x(path, hash, repo, branch, commit, blob) \
ON CONFLICT (swimlane_id, path) DO UPDATE SET \
  hash = EXCLUDED.hash, repo = EXCLUDED.repo, branch = EXCLUDED.branch, commit_id = EXCLUDED.commit_id, \
  adopted_at = EXCLUDED.adopted_at, adopted_by_tid = EXCLUDED.adopted_by_tid, adopted_by_oid = EXCLUDED.adopted_by_oid, \
  blob_hash = EXCLUDED.blob_hash";

/// The marks of the intentional resolutions.
pub const ADOPT_MARKS_SQL: &str = "\
INSERT INTO intentional_divergence (swimlane_id, path, nfs_hash, git_hash, reason, marked_by_tid, marked_by_oid, marked_at) \
SELECT $1, x.path, x.nfs, x.git, x.reason, $6, $7, \
       (timestamptz 'epoch' + ($8::bigint::text || ' milliseconds')::interval) \
FROM unnest($2::text[], $3::bytea[], $4::bytea[], $5::text[]) AS x(path, nfs, git, reason) \
ON CONFLICT (swimlane_id, path) DO UPDATE SET \
  nfs_hash = EXCLUDED.nfs_hash, git_hash = EXCLUDED.git_hash, reason = EXCLUDED.reason, \
  marked_by_tid = EXCLUDED.marked_by_tid, marked_by_oid = EXCLUDED.marked_by_oid, marked_at = EXCLUDED.marked_at";

/// The NFS hashes of the planned files, as they are under the lock.
pub const CURRENT_NFS_SQL: &str = "\
SELECT path, CASE WHEN nfs_hash IS NULL THEN NULL ELSE COALESCE(nfs_drift_hash, nfs_hash) END::bytea FROM file_state \
WHERE swimlane_id = $1 AND path = ANY($2::text[])";

impl Inner {
    /// Confirm a pass. See the module's documentation for what is written and in which order.
    pub(crate) async fn confirm(&self, u: &User, id: i64) -> Result<ConfirmOutcome, AdoptionError> {
        let row = self.working_row(id).await?;
        if row.step != AdoptionStep::Confirm {
            return Err(AdoptionError::WrongStep);
        }
        let lane = self.lane(&row.swimlane).await?;
        match row.reason {
            AdoptionReason::Initial if lane.adopted => return Err(AdoptionError::AlreadyAdopted),
            AdoptionReason::TenantSuffixChanged if !lane.adopted => return Err(AdoptionError::NotAdopted),
            _ => {}
        }
        let plan = self.plan(&row).await?;
        let stored = self.stored_resolutions(id).await?;
        let decided = match decide(&plan, &stored) {
            Ok(decided) => decided,
            Err(error @ AdoptionError::Unresolved(_)) => {
                // What was reviewed is not what is there any more: the pass goes back to the files.
                let _ = self.set_step(id, AdoptionStep::Files).await;
                return Err(error);
            }
            Err(other) => return Err(other),
        };
        let outcome = self.write(u, &row, &plan, &decided).await?;
        if let Some(pipeline) = &self.pipeline {
            pipeline.lane_changed(row.swimlane.clone());
        }
        self.metrics.adoption_baselines(outcome.baselines as u64);
        info!(
            adoption = id,
            swimlane = %row.swimlane,
            baselines = outcome.baselines,
            intentional = outcome.intentional,
            tenants_changed = outcome.tenants_changed,
            "adoption confirmed"
        );
        Ok(outcome)
    }

    /// The transaction. `reviewed` is what the admin's resolutions said when the confirmation was prepared; the transaction decides again,
    /// from the resolutions as they are once the pass is closed, and refuses to go on if that is not the same.
    async fn write(
        &self,
        u: &User,
        row: &Row,
        plan: &Plan,
        reviewed: &[Decided<'_>],
    ) -> Result<ConfirmOutcome, AdoptionError> {
        let now = self.clock.now().unix_millis();
        let (tid, oid) = Self::user_ids(u);
        let tenants = row.chosen()?;
        let mut tx = self.db.begin().await?;

        // 1. The lock that every apply of a delta takes.
        self.db
            .run(
                "adoption_lock",
                sqlx::query(LOCK_SQL)
                    .bind(row.swimlane.as_str())
                    .execute(&mut *tx),
            )
            .await?;

        // 2. Close the pass: exactly one confirmation of it can.
        let close = format!(
            "UPDATE adoptions SET status = 'confirmed', confirmed_by_tid = $2, confirmed_by_oid = $3, confirmed_at = {at}, updated_at = {at} \
             WHERE id = $1 AND status = 'in_progress' AND step = 'confirm'",
            at = ts("$4")
        );
        let closed = self
            .db
            .run(
                "adoption_close",
                sqlx::query(&close)
                    .bind(row.id)
                    .bind(&tid)
                    .bind(&oid)
                    .bind(now)
                    .execute(&mut *tx),
            )
            .await?;
        if closed.rows_affected() == 0 {
            return Err(AdoptionError::NotOpen);
        }
        // Closed, so a resolution can no longer be recorded (it stamps the pass first, and a closed pass is refused): these are the
        // resolutions the baselines are written from. A decision that was changed while the confirmation waited is not applied unseen.
        let stored = self.stored_resolutions_in(&mut tx, row.id).await?;
        let decided = decide(plan, &stored)?;
        if !same_decisions(reviewed, &decided) {
            info!(adoption = row.id, swimlane = %row.swimlane, "a resolution changed while the confirmation was prepared");
            return Err(AdoptionError::Changed);
        }
        let decided = decided.as_slice();
        let counts = Counts::of(decided);

        // 3. Nothing the admin reviewed has changed.
        let current = self.current_hashes(&mut tx, &row.swimlane, decided).await?;
        let changed = changed_since(decided, &current);
        if !changed.is_empty() {
            info!(
                adoption = row.id,
                swimlane = %row.swimlane,
                files = changed.len(),
                "a file changed while the confirmation was prepared"
            );
            return Err(AdoptionError::Changed);
        }

        // 4. The baselines with their references, the marks, the tenant set, the swimlane.
        self.write_baselines(&mut tx, row, decided, &tid, &oid, now)
            .await?;
        self.write_marks(&mut tx, row, decided, &tid, &oid, now).await?;
        let (tenants_before, tenants_changed) =
            self.replace_tenants(&mut tx, &row.swimlane, &tenants).await?;
        let editing_unlocked = row.reason == AdoptionReason::Initial;
        if editing_unlocked {
            self.unlock(&mut tx, &row.swimlane, &tid, &oid, now).await?;
        }

        // 5. The events, in this transaction (D80).
        let paths: Vec<NfsPath> = decided.iter().map(|d| d.entry.path.clone()).collect();
        self.queue_events(&mut tx, &row.swimlane, &paths).await?;

        // 6. The audit events, last: the chain's lock is held from here to the commit.
        let events = self.audit_events(
            u,
            row,
            decided,
            &counts,
            &tenants,
            tenants_changed.then_some(tenants_before.as_slice()),
            now,
        );
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::record_all(&*self.audit, &mut handle, events).await?;
        self.db.commit(tx).await?;

        Ok(ConfirmOutcome {
            adoption: row.id,
            swimlane: row.swimlane.clone(),
            reason: row.reason,
            baselines: counts.total(),
            equal: counts.equal,
            nfs_is_right: counts.nfs,
            git_is_right: counts.git,
            intentional: counts.intentional,
            tenants,
            tenants_changed,
            editing_unlocked,
        })
    }

    /// The audit events of a confirmation, in the order they are recorded: one `BaselineAdopted`, one `DriftMarkedIntentional` for each
    /// mark, and a `SettingsChanged` when the tenant set changed (`tenants_before` is `Some` then).
    #[allow(clippy::too_many_arguments)]
    fn audit_events(
        &self,
        u: &User,
        row: &Row,
        decided: &[Decided<'_>],
        counts: &Counts,
        tenants: &[TenantId],
        tenants_before: Option<&[String]>,
        now: i64,
    ) -> Vec<AuditEvent> {
        let label = row
            .base_label
            .as_ref()
            .and_then(|v| serde_json::from_value::<VersionLabel>(v.clone()).ok());
        let mut events: Vec<AuditEvent> = Vec::with_capacity(2 + counts.intentional);
        let mut adopted = AuditEvent::new(
            Timestamp::from_unix_millis(now),
            Self::actor(u),
            AuditAction::BaselineAdopted,
            AuditVia::Ui,
        );
        adopted.swimlane = Some(row.swimlane.clone());
        adopted.diff = Some(summary(row, label.as_ref(), tenants, decided, counts));
        events.push(adopted);
        for d in decided {
            if let Decision::Intentional(reason) = &d.decision {
                let mut mark = AuditEvent::new(
                    Timestamp::from_unix_millis(now),
                    Self::actor(u),
                    AuditAction::DriftMarkedIntentional,
                    AuditVia::Ui,
                );
                mark.swimlane = Some(row.swimlane.clone());
                mark.path = Some(d.entry.path.clone());
                mark.hash_before = Some(d.entry.nfs);
                mark.hash_after = Some(d.entry.git);
                mark.diff = Some(format!("intentional divergence: {reason}"));
                events.push(mark);
            }
        }
        if let Some(before) = tenants_before {
            let after: Vec<&str> = tenants.iter().map(TenantId::as_str).collect();
            events.push(self.lifecycle_event(
                u,
                &row.swimlane,
                format!(
                    "tenants of the swimlane: {} -> {} (adoption {})",
                    before.join(", "),
                    after.join(", "),
                    row.id
                ),
            ));
        }
        events
    }

    /// The hashes for drift of the planned files, as they are under the lock.
    async fn current_hashes(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        decided: &[Decided<'_>],
    ) -> Result<std::collections::BTreeMap<String, ContentHash>, RegistryError> {
        let mut out = std::collections::BTreeMap::new();
        for chunk in decided.chunks(self.config.rows_per_statement) {
            let paths: Vec<&str> = chunk.iter().map(|d| d.entry.path.as_str()).collect();
            let rows: Vec<(String, Option<Vec<u8>>)> = self
                .db
                .run(
                    "adoption_current",
                    sqlx::query_as(CURRENT_NFS_SQL)
                        .bind(swimlane.as_str())
                        .bind(&paths)
                        .fetch_all(&mut **tx),
                )
                .await?;
            for (path, hash) in rows {
                if let Some(hash) = hash {
                    out.insert(path, hash_of(hash)?);
                }
            }
        }
        Ok(out)
    }

    /// The raw versions among `candidates` that the registry holds (the reference's foreign key needs the blob).
    async fn stored_blobs(
        &self,
        tx: &mut Txn,
        candidates: &[Vec<u8>],
    ) -> Result<HashSet<Vec<u8>>, RegistryError> {
        let mut found = HashSet::new();
        for chunk in candidates.chunks(self.config.rows_per_statement) {
            let rows: Vec<Vec<u8>> = self
                .db
                .run(
                    "adoption_blobs",
                    sqlx::query_scalar("SELECT hash::bytea FROM blobs WHERE hash = ANY($1::bytea[])")
                        .bind(chunk)
                        .fetch_all(&mut **tx),
                )
                .await?;
            found.extend(rows);
        }
        Ok(found)
    }

    async fn write_baselines(
        &self,
        tx: &mut Txn,
        row: &Row,
        decided: &[Decided<'_>],
        tid: &str,
        oid: &str,
        now: i64,
    ) -> Result<(), RegistryError> {
        let candidates: Vec<Vec<u8>> = decided
            .iter()
            .filter_map(Decided::blob_candidate)
            .map(|h| h.as_bytes().to_vec())
            .collect();
        let held = self.stored_blobs(tx, &candidates).await?;
        for chunk in decided.chunks(self.config.rows_per_statement) {
            let paths: Vec<&str> = chunk.iter().map(|d| d.entry.path.as_str()).collect();
            let hashes: Vec<Vec<u8>> = chunk.iter().map(|d| d.baseline().as_bytes().to_vec()).collect();
            let repos: Vec<&str> = chunk
                .iter()
                .map(|d| match d.entry.repo {
                    RepoKind::Base => "base",
                    RepoKind::Tenant => "tenant",
                })
                .collect();
            let branches: Vec<Option<&str>> = chunk.iter().map(|d| d.entry.branch.as_deref()).collect();
            let commits: Vec<&str> = chunk.iter().map(|d| d.entry.commit.as_str()).collect();
            let blobs: Vec<Option<Vec<u8>>> = chunk
                .iter()
                .map(|d| {
                    d.blob_candidate()
                        .map(|h| h.as_bytes().to_vec())
                        .filter(|h| held.contains(h))
                })
                .collect();
            // The raw versions the baselines stand on now, before they are replaced (A24, D78).
            let before = baselines::current_blobs(&self.db, tx, &row.swimlane, &paths).await?;
            self.db
                .run(
                    "adoption_baselines",
                    sqlx::query(ADOPT_BASELINES_SQL)
                        .bind(row.swimlane.as_str())
                        .bind(&paths)
                        .bind(&hashes)
                        .bind(&repos)
                        .bind(&branches)
                        .bind(&commits)
                        .bind(&blobs)
                        .bind(now)
                        .bind(tid)
                        .bind(oid)
                        .execute(&mut **tx),
                )
                .await?;
            let changes: Vec<RefChange> = chunk
                .iter()
                .zip(blobs)
                .map(|(d, new)| RefChange {
                    path: d.entry.path.as_str().to_owned(),
                    old: before.get(d.entry.path.as_str()).cloned(),
                    new,
                })
                .collect();
            baselines::replace_refs(&self.db, tx, &row.swimlane, &changes).await?;
        }
        Ok(())
    }

    async fn write_marks(
        &self,
        tx: &mut Txn,
        row: &Row,
        decided: &[Decided<'_>],
        tid: &str,
        oid: &str,
        now: i64,
    ) -> Result<(), RegistryError> {
        let marked: Vec<(&Decided<'_>, &str)> = decided
            .iter()
            .filter_map(|d| match &d.decision {
                Decision::Intentional(reason) => Some((d, reason.as_str())),
                _ => None,
            })
            .collect();
        for chunk in marked.chunks(self.config.rows_per_statement) {
            let paths: Vec<&str> = chunk.iter().map(|(d, _)| d.entry.path.as_str()).collect();
            let nfs: Vec<Vec<u8>> = chunk
                .iter()
                .map(|(d, _)| d.entry.nfs.as_bytes().to_vec())
                .collect();
            let git: Vec<Vec<u8>> = chunk
                .iter()
                .map(|(d, _)| d.entry.git.as_bytes().to_vec())
                .collect();
            let reasons: Vec<&str> = chunk.iter().map(|(_, r)| *r).collect();
            self.db
                .run(
                    "adoption_marks",
                    sqlx::query(ADOPT_MARKS_SQL)
                        .bind(row.swimlane.as_str())
                        .bind(&paths)
                        .bind(&nfs)
                        .bind(&git)
                        .bind(&reasons)
                        .bind(tid)
                        .bind(oid)
                        .bind(now)
                        .execute(&mut **tx),
                )
                .await?;
        }
        Ok(())
    }

    /// Make the swimlane's tenants the chosen ones. Returns what they were and whether that is a change.
    async fn replace_tenants(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        chosen: &[TenantId],
    ) -> Result<(Vec<String>, bool), RegistryError> {
        let before: Vec<String> = self
            .db
            .run(
                "adoption_tenants_before",
                sqlx::query_scalar(
                    "SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id LIMIT 200",
                )
                .bind(swimlane.as_str())
                .fetch_all(&mut **tx),
            )
            .await?;
        let wanted: Vec<String> = chosen.iter().map(|t| t.as_str().to_owned()).collect();
        let same = before.iter().collect::<BTreeSet<_>>() == wanted.iter().collect::<BTreeSet<_>>();
        if same {
            return Ok((before, false));
        }
        self.db
            .run(
                "adoption_tenants_drop",
                sqlx::query(
                    "DELETE FROM swimlane_tenants WHERE swimlane_id = $1 AND tenant_id <> ALL($2::text[])",
                )
                .bind(swimlane.as_str())
                .bind(&wanted)
                .execute(&mut **tx),
            )
            .await?;
        self.db
            .run(
                "adoption_tenants_add",
                sqlx::query(
                    "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) SELECT $1, x FROM unnest($2::text[]) AS x \
                     ON CONFLICT DO NOTHING",
                )
                .bind(swimlane.as_str())
                .bind(&wanted)
                .execute(&mut **tx),
            )
            .await?;
        Ok((before, true))
    }

    /// The first pass confirms the swimlane's baseline and unlocks editing (D2).
    async fn unlock(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        tid: &str,
        oid: &str,
        now: i64,
    ) -> Result<(), AdoptionError> {
        let sql = format!(
            "UPDATE swimlanes SET editing_unlocked = true, baseline_confirmed_at = {}, baseline_confirmed_by_tid = $3, \
             baseline_confirmed_by_oid = $4 WHERE id = $1 AND baseline_confirmed_at IS NULL",
            ts("$2")
        );
        let done = self
            .db
            .run(
                "adoption_unlock",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(now)
                    .bind(tid)
                    .bind(oid)
                    .execute(&mut **tx),
            )
            .await?;
        if done.rows_affected() == 0 {
            return Err(AdoptionError::AlreadyAdopted);
        }
        Ok(())
    }

    /// `DriftChanged` for the baselined paths, in chunks of [`DRIFT_EVENT_PATHS`] and at most [`MAX_DRIFT_EVENTS`], and `FindingsChanged`
    /// (also when the paths did not all fit, so that clients refetch the swimlane, A26).
    async fn queue_events(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[NfsPath],
    ) -> Result<(), RegistryError> {
        let mut events: Vec<DomainEvent> = paths
            .chunks(DRIFT_EVENT_PATHS)
            .take(MAX_DRIFT_EVENTS)
            .map(|chunk| DomainEvent::DriftChanged {
                swimlane: swimlane.clone(),
                paths: chunk.to_vec(),
            })
            .collect();
        events.push(DomainEvent::FindingsChanged {
            swimlane: swimlane.clone(),
        });
        let mut handle = Tx::from_pg(tx);
        crate::batch::publish_all(&*self.bus, &mut handle, events).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use domain::CommitId;
    use ports::content_hash;

    use super::*;

    fn h(s: &str) -> ContentHash {
        content_hash(s.as_bytes())
    }

    fn entry(path: &str, nfs: &str, git: &str, denied: bool) -> Entry {
        Entry {
            path: NfsPath::parse(path).unwrap(),
            denied,
            nfs_raw: h(&format!("raw-{nfs}")),
            nfs: h(nfs),
            git: h(git),
            git_raw: h(&format!("raw-{git}")),
            repo: RepoKind::Base,
            branch: None,
            commit: CommitId::parse(&"a".repeat(40)).unwrap(),
        }
    }

    fn stored_for(kind: ResolutionKind, e: &Entry, reason: Option<&str>) -> Stored {
        Stored {
            kind,
            reason: reason.map(str::to_owned),
            nfs: Some(e.nfs),
            git: Some(e.git),
        }
    }

    fn plan() -> Plan {
        Plan {
            equal: vec![entry("a.yml", "same", "same", false)],
            differing: vec![
                entry("b.yml", "n1", "g1", false),
                entry("c.yml", "n2", "g2", false),
                entry("d.yml", "n3", "g3", false),
                entry("k.key", "n4", "g4", true),
            ],
            missing_on_nfs: 0,
        }
    }

    fn resolutions(plan: &Plan) -> BTreeMap<String, Stored> {
        let d = &plan.differing;
        BTreeMap::from([
            (
                "b.yml".to_owned(),
                stored_for(ResolutionKind::NfsIsRight, &d[0], None),
            ),
            (
                "c.yml".to_owned(),
                stored_for(ResolutionKind::GitIsRight, &d[1], None),
            ),
            (
                "d.yml".to_owned(),
                stored_for(ResolutionKind::Intentional, &d[2], Some("the batch job")),
            ),
            (
                "k.key".to_owned(),
                stored_for(ResolutionKind::NfsIsRight, &d[3], None),
            ),
        ])
    }

    #[test]
    fn each_resolution_chooses_the_baseline_and_the_raw_version_behind_it() {
        let plan = plan();
        let decided = decide(&plan, &resolutions(&plan)).unwrap();
        let by: BTreeMap<&str, &Decided<'_>> = decided.iter().map(|d| (d.entry.path.as_str(), d)).collect();

        // Equal: B = N = G, the raw version is NFS's.
        assert_eq!(by["a.yml"].baseline(), h("same"));
        assert_eq!(by["a.yml"].blob_candidate(), Some(h("raw-same")));
        // NFS is right: Git is behind, so B is Git's (the state becomes nfs_ahead), and the raw version is Git's.
        assert_eq!(by["b.yml"].baseline(), h("g1"));
        assert_eq!(by["b.yml"].blob_candidate(), Some(h("raw-g1")));
        // Git is right: NFS is behind, so B is NFS's (the state becomes git_ahead), and the raw version is NFS's.
        assert_eq!(by["c.yml"].baseline(), h("n2"));
        assert_eq!(by["c.yml"].blob_candidate(), Some(h("raw-n2")));
        // Intentional: B is Git's, and the mark holds N and G.
        assert_eq!(by["d.yml"].baseline(), h("g3"));
        assert_eq!(
            by["d.yml"].decision,
            Decision::Intentional("the batch job".to_owned())
        );
        // A denied file has a baseline and never a raw version (D79).
        assert_eq!(by["k.key"].baseline(), h("g4"));
        assert_eq!(by["k.key"].blob_candidate(), None);
        assert_eq!(decided.len(), 5);
        assert!(
            decided.windows(2).all(|w| w[0].entry.path < w[1].entry.path),
            "in path order"
        );
    }

    #[test]
    fn a_file_without_a_current_resolution_refuses_the_confirmation_with_the_count() {
        let plan = plan();
        let mut stored = resolutions(&plan);
        stored.remove("b.yml");
        // A resolution made for another version of the file does not count.
        stored.insert(
            "c.yml".to_owned(),
            Stored {
                nfs: Some(h("older")),
                ..stored["c.yml"].clone()
            },
        );
        assert_eq!(decide(&plan, &stored).unwrap_err(), AdoptionError::Unresolved(2));
        assert_eq!(
            decide(&plan, &BTreeMap::new()).unwrap_err(),
            AdoptionError::Unresolved(4)
        );
        // A plan with nothing that differs needs none.
        let equal_only = Plan {
            equal: plan.equal.clone(),
            ..Plan::default()
        };
        assert_eq!(decide(&equal_only, &BTreeMap::new()).unwrap().len(), 1);
    }

    #[test]
    fn a_file_that_changed_or_went_since_the_review_is_found() {
        let plan = plan();
        let decided = decide(&plan, &resolutions(&plan)).unwrap();
        let mut now: BTreeMap<String, ContentHash> = decided
            .iter()
            .map(|d| (d.entry.path.as_str().to_owned(), d.entry.nfs))
            .collect();
        assert!(changed_since(&decided, &now).is_empty());
        now.insert("b.yml".to_owned(), h("somebody edited it"));
        now.remove("d.yml");
        let changed: Vec<&str> = changed_since(&decided, &now)
            .into_iter()
            .map(NfsPath::as_str)
            .collect();
        assert_eq!(changed, vec!["b.yml", "d.yml"]);
    }

    #[test]
    fn the_counts_add_up_and_the_summary_names_paths_never_content() {
        let plan = plan();
        let decided = decide(&plan, &resolutions(&plan)).unwrap();
        let counts = Counts::of(&decided);
        assert_eq!(
            counts,
            Counts {
                equal: 1,
                nfs: 2,
                git: 1,
                intentional: 1
            }
        );
        assert_eq!(counts.total(), 5);
        let row = Row {
            id: 7,
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            reason: AdoptionReason::Initial,
            status: super::super::store::AdoptionStatus::InProgress,
            step: AdoptionStep::Confirm,
            ranking: serde_json::json!([]),
            hints: serde_json::json!({}),
            tenants: vec!["sit1".to_owned()],
            tenant_commits: serde_json::json!({}),
            base_commit: Some("b".repeat(40)),
            base_label: None,
            secret_scan: serde_json::json!([]),
            started_at_ms: 0,
            updated_at_ms: 0,
        };
        let text = summary(&row, None, &[TenantId::parse("sit1").unwrap()], &decided, &counts);
        assert!(
            text.starts_with("adoption 7 confirmed (initial) for sit1\n"),
            "{text}"
        );
        assert!(text.contains("tenants: sit1"));
        assert!(text.contains("baselines: 5 (equal 1, nfs_is_right 2, git_is_right 1, intentional 1)"));
        assert!(text.contains("nfs_is_right:\nb.yml\nk.key\n"), "{text}");
        assert!(text.contains("intentional:\nd.yml\n"));
        assert!(!text.contains("a.yml"), "equal files are counted, not listed");
        assert!(
            !text.contains("the batch job"),
            "the reason is in the mark's own event"
        );
    }

    #[test]
    fn a_long_list_of_paths_is_cut_and_counted() {
        let many = Plan {
            equal: Vec::new(),
            differing: (0..500)
                .map(|i| entry(&format!("dir/file-{i:03}.yml"), "n", "g", false))
                .collect(),
            missing_on_nfs: 0,
        };
        let stored: BTreeMap<String, Stored> = many
            .differing
            .iter()
            .map(|e| {
                (
                    e.path.as_str().to_owned(),
                    stored_for(ResolutionKind::NfsIsRight, e, None),
                )
            })
            .collect();
        let decided = decide(&many, &stored).unwrap();
        let row = Row {
            id: 1,
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            reason: AdoptionReason::TenantSuffixChanged,
            status: super::super::store::AdoptionStatus::InProgress,
            step: AdoptionStep::Confirm,
            ranking: serde_json::json!([]),
            hints: serde_json::json!({}),
            tenants: Vec::new(),
            tenant_commits: serde_json::json!({}),
            base_commit: None,
            base_label: None,
            secret_scan: serde_json::json!([]),
            started_at_ms: 0,
            updated_at_ms: 0,
        };
        let text = summary(&row, None, &[], &decided, &Counts::of(&decided));
        assert!(text.contains("... and 300 more"), "{text}");
        assert!(text.len() <= MAX_TEXT_BYTES + 128);
        assert!(text.len() <= ports::AuditEvent::MAX_DIFF_BYTES);
    }

    #[test]
    fn the_statements_name_every_bind_they_use() {
        for (sql, last) in [
            (ADOPT_BASELINES_SQL, 10),
            (ADOPT_MARKS_SQL, 8),
            (CURRENT_NFS_SQL, 2),
        ] {
            assert!(sql.contains(&format!("${last}")), "{sql}");
            assert!(!sql.contains(&format!("${}", last + 1)), "{sql}");
            assert!(!sql.contains("OFFSET"));
        }
    }

    mod property {
        use proptest::prelude::*;

        use super::*;

        /// How a differing file stands: not resolved, resolved for another version, or resolved now, by each kind.
        #[derive(Debug, Clone, Copy)]
        enum Stand {
            None,
            Stale,
            Nfs,
            Git,
            Intentional,
        }

        fn stand() -> impl Strategy<Value = Stand> {
            prop_oneof![
                Just(Stand::None),
                Just(Stand::Stale),
                Just(Stand::Nfs),
                Just(Stand::Git),
                Just(Stand::Intentional)
            ]
        }

        proptest! {
            /// A confirmation decides every file or refuses with the exact number that lack a current resolution; the decisions come in
            /// path order whatever order the plan had, and each is the resolution the admin made.
            #[test]
            fn every_file_is_decided_or_the_confirmation_is_refused_with_the_count(
                stands in prop::collection::vec(stand(), 0..12),
                equal in 0_usize..4,
            ) {
                let plan = Plan {
                    equal: (0..equal).map(|i| entry(&format!("eq/f{i}.yml"), "same", "same", false)).collect(),
                    differing: stands
                        .iter()
                        .enumerate()
                        .map(|(i, _)| entry(&format!("diff/f{i:02}.yml"), &format!("n{i}"), &format!("g{i}"), false))
                        .collect(),
                    missing_on_nfs: 0,
                };
                let mut stored = BTreeMap::new();
                for (e, how) in plan.differing.iter().zip(&stands) {
                    let kind = match how {
                        Stand::None => continue,
                        Stand::Stale => {
                            stored.insert(
                                e.path.as_str().to_owned(),
                                Stored { kind: ResolutionKind::NfsIsRight, reason: None, nfs: Some(h("older")), git: Some(e.git) },
                            );
                            continue;
                        }
                        Stand::Nfs => ResolutionKind::NfsIsRight,
                        Stand::Git => ResolutionKind::GitIsRight,
                        Stand::Intentional => ResolutionKind::Intentional,
                    };
                    let reason = (kind == ResolutionKind::Intentional).then_some("on purpose");
                    stored.insert(e.path.as_str().to_owned(), stored_for(kind, e, reason));
                }
                let lacking = stands.iter().filter(|s| matches!(s, Stand::None | Stand::Stale)).count();
                match decide(&plan, &stored) {
                    Ok(decided) => {
                        prop_assert_eq!(lacking, 0);
                        prop_assert_eq!(decided.len(), plan.equal.len() + plan.differing.len());
                        prop_assert!(decided.windows(2).all(|w| w[0].entry.path < w[1].entry.path));
                        let counts = Counts::of(&decided);
                        prop_assert_eq!(counts.equal, equal);
                        prop_assert_eq!(counts.total(), decided.len());
                    }
                    Err(e) => prop_assert_eq!(e, AdoptionError::Unresolved(lacking)),
                }
            }
        }
    }
}
