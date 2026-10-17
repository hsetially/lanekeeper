//! Reconcile (T3, plan "Pipeline", D63): compare what the pipeline holds with what the database and Git say, and find what an event
//! missed.
//!
//! Events are at least once and not forever: a subscriber that lags gets a `Resync`, a replica that takes over the lease was not
//! listening while nobody held it, a signal can be dropped by a full queue, and a decision made on another replica (a baseline, a mark)
//! publishes nothing. The reconcile is what makes "the state is right" independent of all of that. It runs when a run of the job begins,
//! on a `Resync`, and every `reconcile_interval`, and does three things:
//!
//! 1. **Swimlanes in memory against the database.** One read of the swimlane's rows, baselines and marks, compared with the entry path
//!    by path: the hash of the content on NFS, the class, the baseline, the mark, and the derived columns of the stored row against what
//!    the entry says they should be. A path that differs is queued (the swimlane whole when too many do, or when its rules, tenants or
//!    application names changed since it was computed). The derived columns are compared too, so that a write another replica made
//!    (a nightly repair) or lost is noticed.
//! 2. **The mirror against what was handled.** For each deployable branch whose head is not the one the last handled move left it at, the
//!    move is routed as if its event had just come (`GitIndex::affected`).
//! 3. **Swimlanes that were never computed.** A swimlane whose first inventory is done and that has no stored rules fingerprint has not
//!    been computed: it is returned as *cold* for the caller to load.
//!
//! The function returns what it found; it changes nothing (the pipeline's work then goes through the same batch as any event).

use std::collections::{BTreeMap, BTreeSet};

use domain::{ContentHash, NfsPath, SwimlaneId, TenantSet};

use super::Inner;
use super::coalesce::Work;
use super::git_side::{LiveHeads, live_heads};
use super::lane_loader::Inputs;
use super::lane_store::{LaneEntry, baseline_hash};
use super::{Pipeline, blocking};
use crate::error::RegistryError;
use crate::index::LaneImpact;

/// The most swimlanes the reconcile looks at for ones never computed.
const MAX_LANES: i64 = 10_000;

/// What a reconcile found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// Changes to recompute: paths of swimlanes in memory, whole swimlanes, the paths of moved heads.
    pub work: Vec<(SwimlaneId, Work)>,
    /// Swimlanes with an inventory that have never been computed.
    pub cold: Vec<SwimlaneId>,
    /// Paths where memory and the database disagreed (not counting moved heads).
    pub divergent_paths: usize,
}

/// The paths where the entry and the database disagree about what the inputs are or about what was derived from them.
pub(crate) fn divergent_paths(entry: &LaneEntry, inputs: &Inputs) -> BTreeSet<NfsPath> {
    let mut out = BTreeSet::new();
    let hash_of = |path: &NfsPath| -> Option<(Option<ContentHash>, domain::FileClass)> {
        if let Some(o) = entry.opaque.get(path) {
            return Some((o.nfs, o.class));
        }
        entry
            .file(path)
            .map(|f| (f.nfs.as_ref().map(|c| c.blob), f.class))
    };
    for (path, row) in &inputs.rows {
        let mem = hash_of(path);
        // Opaque files hold the hash for comparing, the engine's the raw one: compare like with like.
        let stored_nfs = if entry.opaque.contains_key(path) {
            row.nfs.and(row.nfs_drift.or(row.nfs))
        } else {
            row.nfs
        };
        let same_inputs = mem.is_some_and(|(nfs, class)| nfs == stored_nfs && class == row.class);
        let baseline = inputs.baselines.get(path).copied();
        let mark = inputs.marks.get(path).copied();
        let (mem_baseline, mem_mark) = match (entry.file(path), entry.opaque.get(path)) {
            (Some(f), _) => (baseline_hash(f.baseline), f.intentional),
            (None, Some(o)) => (baseline_hash(o.baseline), o.intentional),
            (None, None) => (None, None),
        };
        let same_decisions = mem_baseline == baseline && mem_mark == mark;
        let same_derived = entry.sig(path).is_some_and(|sig| sig.matches(&row.persisted));
        if !(same_inputs && same_decisions && same_derived) {
            out.insert(path.clone());
        }
    }
    // A path the entry has that the database has no row for (a row that was lost, or never written).
    for path in entry.paths() {
        if !inputs.rows.contains_key(&path) {
            out.insert(path);
        }
    }
    out
}

impl Inner {
    /// The swimlanes with an inventory that have no stored rules fingerprint.
    pub(crate) async fn never_computed(&self) -> Result<Vec<SwimlaneId>, RegistryError> {
        let ids: Vec<String> = self
            .db
            .run(
                "reconcile_cold",
                sqlx::query_scalar(
                    "SELECT swimlane_id FROM swimlane_registry \
                     WHERE first_inventory_at IS NOT NULL AND rules_fingerprint IS NULL ORDER BY swimlane_id LIMIT $1",
                )
                .bind(MAX_LANES)
                .fetch_all(self.db.pool()),
            )
            .await?;
        ids.iter()
            .map(|id| SwimlaneId::parse(id).map_err(|_| RegistryError::Corrupt))
            .collect()
    }

    /// Every swimlane with an inventory.
    pub(crate) async fn all_with_inventory(&self) -> Result<Vec<SwimlaneId>, RegistryError> {
        let ids: Vec<String> = self
            .db
            .run(
                "pipeline_all_lanes",
                sqlx::query_scalar(
                    "SELECT swimlane_id FROM swimlane_registry WHERE first_inventory_at IS NOT NULL \
                     ORDER BY swimlane_id LIMIT $1",
                )
                .bind(MAX_LANES)
                .fetch_all(self.db.pool()),
            )
            .await?;
        ids.iter()
            .map(|id| SwimlaneId::parse(id).map_err(|_| RegistryError::Corrupt))
            .collect()
    }

    /// Remember where the branches are now, if nothing has been remembered. What a run begins with: whatever happens from now on is an
    /// event or a difference from this.
    pub(crate) async fn remember_heads(&self) -> Result<(), RegistryError> {
        let git = std::sync::Arc::clone(&self.git);
        let live = blocking(self.config.git_timeout, move || live_heads(git.as_ref())).await?;
        if let Ok(mut processed) = self.processed.lock() {
            if processed.is_empty() {
                *processed = live.branches();
            }
        }
        Ok(())
    }

    pub(crate) async fn reconcile(&self) -> Result<ReconcileOutcome, RegistryError> {
        self.metrics.pipeline_reconcile();
        let mut out = ReconcileOutcome {
            cold: self.never_computed().await?,
            ..ReconcileOutcome::default()
        };
        let git = std::sync::Arc::clone(&self.git);
        let live: LiveHeads = blocking(self.config.git_timeout, move || live_heads(git.as_ref())).await?;

        // 1. The swimlanes in memory against the database.
        let ids = self.exec.lock().await.store.ids();
        if !ids.is_empty() {
            let deployable =
                TenantSet::new(live.tenants.keys().cloned()).map_err(|_| RegistryError::Capacity)?;
            let global = self.rules.global(deployable).await?;
            let rules = self.rules.lanes(&global, &ids).await?;
            for id in &ids {
                let Some(lane) = rules.get(id) else {
                    // The swimlane is gone.
                    self.exec.lock().await.store.remove(id);
                    continue;
                };
                let changed_rules = {
                    let exec = self.exec.lock().await;
                    exec.store
                        .get(id)
                        .is_none_or(|e| e.fingerprint != lane.fingerprint)
                };
                if changed_rules {
                    out.work.push((id.clone(), Work::Whole));
                    continue;
                }
                let inputs = self.loader.load_lane(id).await?;
                let exec = self.exec.lock().await;
                let Some(entry) = exec.store.get(id) else { continue };
                let diff = divergent_paths(entry, &inputs);
                if diff.is_empty() {
                    continue;
                }
                out.divergent_paths += diff.len();
                if diff.len() > self.config.max_affected_paths {
                    out.work.push((id.clone(), Work::Whole));
                } else {
                    out.work.push((id.clone(), Work::Paths(diff)));
                }
            }
        }

        // 2. The mirror against what was handled.
        let now = live.branches();
        let processed: BTreeMap<_, _> = self.processed.lock().map(|p| p.clone()).unwrap_or_default();
        for ((repo, branch), head) in &now {
            let from = processed.get(&(*repo, branch.clone()));
            if from == Some(head) {
                continue;
            }
            let affected = self.git_index.affected(*repo, branch, from, head).await?;
            for (lane, impact) in &affected.lanes {
                match impact {
                    LaneImpact::Paths(paths) => {
                        out.work.push((lane.clone(), Work::Paths(paths.as_ref().clone())));
                    }
                    LaneImpact::Whole => out.work.push((lane.clone(), Work::Whole)),
                }
            }
            self.remember(*repo, branch, head);
        }
        if let Ok(mut processed) = self.processed.lock() {
            // A branch that is gone is not remembered.
            processed.retain(|key, _| now.contains_key(key));
        }
        Ok(out)
    }
}

impl Pipeline {
    /// Compare what the pipeline holds with the database and the mirror, and say what differs. It changes nothing: the work it finds
    /// is for [`Pipeline::process`].
    ///
    /// # Errors
    ///
    /// The database or the mirror could not be read.
    pub async fn reconcile(&self) -> Result<ReconcileOutcome, RegistryError> {
        self.inner.reconcile().await
    }

    /// Remember where the branches are now, if nothing has been remembered (what a run of the job does first).
    ///
    /// # Errors
    ///
    /// The mirror could not be read.
    pub async fn remember_heads(&self) -> Result<(), RegistryError> {
        self.inner.remember_heads().await
    }
}
