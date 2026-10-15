//! One batch of the pipeline (T3, P5, P6): the swimlanes that have work, brought up to date.
//!
//! A batch is a list of `(swimlane, Work)`: some paths, or the whole swimlane. The steps, with everything that reads the database or the
//! mirror once for the whole batch and not once for each swimlane:
//!
//! 1. which swimlanes have had their first inventory (the others have nothing to compare, and the first inventory recomputes them);
//! 2. the rules of those swimlanes and the heads of the mirror ([`GitView`]), the same for all of them;
//! 3. each swimlane is **whole** (it is not in memory, the work says so, or its rules changed since its state was computed) or
//!    **incremental**;
//! 4. whole swimlanes are loaded and computed from scratch ([`Inner::refresh_lanes`]), incremental ones are brought up to date from their
//!    state in memory ([`Inner::update_lanes`]): the rows of the changed paths are read, the paths rebuilt, the content of the groups the
//!    engine reads fetched, [`engine::recompute_paths`] called once for all of them, and the difference committed, one transaction for
//!    each swimlane.
//!
//! A swimlane whose batch fails is dropped from memory, so that its next batch starts from the database, and is reported in
//! [`BatchReport::failed`] for the caller to try again. It never leaves a half-written swimlane (one transaction) or a wrong one in
//! memory (the entry is put back only after the commit).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use domain::{AppName, ContentHash, LogicalFile, NfsPath, SwimlaneId, TenantSet};
use engine::{
    BatchError, BatchRules, HostRules, LaneFile, LaneInput, LaneState, Limits, recompute_all, recompute_paths,
};
use futures::StreamExt as _;
use tracing::{debug, warn};

use super::coalesce::Work;
use super::findings_sync::{self, FindingRow, rows_of};
use super::git_side::{GitView, live_heads};
use super::lane_loader::{
    BuildCtx, Built, Decisions, Inputs, all_with_content, blobs_of, build_path, clear, files_to_read, fill,
    place,
};
use super::lane_store::{LaneEntry, OpaqueFile, RowSig};
use super::persist::{LaneCommit, RowWrite, TreeMode};
use super::rules::LaneRules;
use super::{Exec, Inner, Pipeline, blocking};
use crate::error::RegistryError;
use crate::metrics::LaneResult;

/// What happened to one swimlane in a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneReport {
    pub swimlane: SwimlaneId,
    pub result: LaneResult,
    /// The paths recomputed: the ones named, or every path of a whole swimlane.
    pub paths: usize,
    pub rows_written: usize,
    pub rows_deleted: usize,
    /// Paths named in `DriftChanged`.
    pub drift_paths: usize,
    pub findings_changed: usize,
    pub baselines_set: usize,
    pub marks_cleared: usize,
    /// For a whole swimlane that was in memory: the paths where memory and the new computation disagreed.
    pub repaired: usize,
}

impl LaneReport {
    fn new(swimlane: SwimlaneId, result: LaneResult, paths: usize) -> Self {
        Self {
            swimlane,
            result,
            paths,
            rows_written: 0,
            rows_deleted: 0,
            drift_paths: 0,
            findings_changed: 0,
            baselines_set: 0,
            marks_cleared: 0,
            repaired: 0,
        }
    }
}

/// What a batch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    pub lanes: Vec<LaneReport>,
    /// Swimlanes whose batch failed. Their memory is dropped; the caller recomputes them whole.
    pub failed: Vec<SwimlaneId>,
    /// Swimlanes with no inventory yet, or that are not swimlanes (any more).
    pub skipped: Vec<SwimlaneId>,
}

/// Every swimlane of `ids` that has no result yet fails with `error`: a step that took all of them with it did not finish.
fn fail_rest(
    results: &mut Vec<(SwimlaneId, Result<LaneReport, RegistryError>)>,
    ids: Vec<SwimlaneId>,
    error: RegistryError,
) {
    let done: BTreeSet<SwimlaneId> = results.iter().map(|(id, _)| id.clone()).collect();
    results.extend(
        ids.into_iter()
            .filter(|id| !done.contains(id))
            .map(|id| (id, Err(error))),
    );
}

/// Work for the same swimlane merged: a whole swimlane covers any paths.
fn merge(work: Vec<(SwimlaneId, Work)>) -> BTreeMap<SwimlaneId, Work> {
    let mut out: BTreeMap<SwimlaneId, Work> = BTreeMap::new();
    for (id, w) in work {
        match (out.remove(&id), w) {
            (None, w) => {
                out.insert(id, w);
            }
            (Some(Work::Paths(mut a)), Work::Paths(b)) => {
                a.extend(b);
                out.insert(id, Work::Paths(a));
            }
            (Some(_), _) => {
                out.insert(id, Work::Whole);
            }
        }
    }
    out
}

/// The logical files whose effective hash differs between two states: the ones the effective tables have to follow.
fn touched(previous: &LaneState, now: &LaneState) -> Vec<LogicalFile> {
    let old: BTreeMap<&LogicalFile, ContentHash> = previous.effective_hashes().collect();
    let mut out = BTreeSet::new();
    for (file, hash) in now.effective_hashes() {
        if old.get(file) != Some(&hash) {
            out.insert(file.clone());
        }
    }
    for file in old.keys() {
        if now.effective_hash(file).is_none() {
            out.insert((*file).clone());
        }
    }
    out.into_iter().collect()
}

/// Which swimlanes of a batch share the application names, so that one call of the engine serves them all (the names decide which
/// files are property sources, and the engine takes one list for a call).
fn groups<'a>(apps: impl Iterator<Item = &'a [AppName]>) -> BTreeMap<&'a [AppName], Vec<usize>> {
    let mut out: BTreeMap<&[AppName], Vec<usize>> = BTreeMap::new();
    for (i, a) in apps.enumerate() {
        out.entry(a).or_default().push(i);
    }
    out
}

/// A swimlane that is being loaded whole.
struct FullJob {
    rules: LaneRules,
    inputs: Inputs,
    files: Vec<LaneFile>,
    opaque: BTreeMap<NfsPath, OpaqueFile>,
    decisions: Decisions,
}

/// The engine over whole swimlanes, one call for each group that shares application names. A group that the engine refuses is tried
/// swimlane by swimlane, so that one bad swimlane does not stop the others.
fn compute_full(
    pool: &rayon::ThreadPool,
    hosts: &HostRules,
    limits: &Limits,
    jobs: &[FullJob],
) -> Vec<Result<LaneState, BatchError>> {
    let mut out: Vec<Option<Result<LaneState, BatchError>>> = jobs.iter().map(|_| None).collect();
    let one = |idx: &[usize], apps: &[AppName]| -> Result<Vec<LaneState>, BatchError> {
        let inputs: Vec<LaneInput<'_>> = idx
            .iter()
            .map(|&i| LaneInput {
                swimlane: &jobs[i].rules.swimlane,
                tenants: &jobs[i].rules.tenants,
                tier: jobs[i].rules.tier.as_ref(),
                files: &jobs[i].files,
            })
            .collect();
        recompute_all(
            pool,
            &inputs,
            &BatchRules {
                application_names: apps,
                host_rules: hosts,
                limits,
            },
        )
    };
    for (apps, idx) in groups(jobs.iter().map(|j| j.rules.apps.as_slice())) {
        match one(&idx, apps) {
            Ok(states) => {
                for (i, state) in idx.iter().zip(states) {
                    out[*i] = Some(Ok(state));
                }
            }
            Err(e) if idx.len() == 1 => out[idx[0]] = Some(Err(e)),
            Err(_) => {
                for &i in &idx {
                    out[i] = Some(one(&[i], apps).map(|mut s| s.remove(0)));
                }
            }
        }
    }
    out.into_iter()
        .map(|r| r.unwrap_or(Err(BatchError::UnknownSwimlane)))
        .collect()
}

/// A swimlane that is being updated.
struct IncJob {
    entry: LaneEntry,
    previous: Option<LaneState>,
    affected: BTreeSet<NfsPath>,
}

/// The engine over the changed paths of swimlanes in memory, one call for each group that shares application names, tried swimlane by
/// swimlane when a group is refused. Each job's `previous` is taken for the call and put back; the result is in job order.
fn compute_paths(
    pool: &rayon::ThreadPool,
    hosts: &HostRules,
    limits: &Limits,
    jobs: &mut [IncJob],
) -> Vec<Result<LaneState, BatchError>> {
    let mut out: Vec<Option<Result<LaneState, BatchError>>> = jobs.iter().map(|_| None).collect();
    let call = |jobs: &mut [IncJob],
                idx: &[usize],
                apps: &[AppName]|
     -> Result<BTreeMap<SwimlaneId, LaneState>, BatchError> {
        let previous: Vec<LaneState> = idx.iter().filter_map(|&i| jobs[i].previous.take()).collect();
        let affected: Vec<(SwimlaneId, NfsPath)> = idx
            .iter()
            .flat_map(|&i| {
                jobs[i]
                    .affected
                    .iter()
                    .map(|p| (jobs[i].entry.swimlane.clone(), p.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let result = {
            let inputs: Vec<LaneInput<'_>> = idx.iter().map(|&i| jobs[i].entry.input()).collect();
            recompute_paths(
                pool,
                &previous,
                &inputs,
                &affected,
                &BatchRules {
                    application_names: apps,
                    host_rules: hosts,
                    limits,
                },
            )
        };
        // The states go back whether the call worked or not: an entry whose previous state is gone cannot be put back in memory.
        let mut back = previous.into_iter();
        for &i in idx {
            jobs[i].previous = back.next();
        }
        result.map(|states| states.into_iter().map(|s| (s.swimlane().clone(), s)).collect())
    };
    let apps_of: Vec<Vec<AppName>> = jobs.iter().map(|j| j.entry.apps.clone()).collect();
    for (apps, idx) in groups(apps_of.iter().map(Vec::as_slice)) {
        match call(jobs, &idx, apps) {
            Ok(mut states) => {
                for &i in &idx {
                    let id = jobs[i].entry.swimlane.clone();
                    out[i] = Some(states.remove(&id).ok_or(BatchError::UnknownSwimlane));
                }
            }
            Err(e) if idx.len() == 1 => out[idx[0]] = Some(Err(e)),
            Err(_) => {
                for &i in &idx {
                    let id = jobs[i].entry.swimlane.clone();
                    out[i] = Some(
                        call(jobs, &[i], apps)
                            .and_then(|mut s| s.remove(&id).ok_or(BatchError::UnknownSwimlane)),
                    );
                }
            }
        }
    }
    out.into_iter()
        .map(|r| r.unwrap_or(Err(BatchError::UnknownSwimlane)))
        .collect()
}

/// A whole swimlane's paths built: the engine's files, the opaque ones, and what was decided with them.
type LaneBuild = (Vec<LaneFile>, BTreeMap<NfsPath, OpaqueFile>, Decisions);

fn build_lane(lane: &LaneRules, view: &GitView, inputs: &Inputs) -> Result<LaneBuild, RegistryError> {
    let ctx = BuildCtx { lane, view };
    let mut universe: BTreeSet<NfsPath> = inputs
        .rows
        .iter()
        .filter(|(_, row)| row.nfs.is_some())
        .map(|(p, _)| p.clone())
        .collect();
    universe.extend(view.universe(&lane.global.rule, &lane.tenants));
    let mut files = Vec::with_capacity(universe.len());
    let mut opaque = BTreeMap::new();
    let mut decisions = Decisions::default();
    for path in universe {
        let (built, decision) = build_path(
            &ctx,
            &path,
            inputs.rows.get(&path),
            inputs.baselines.get(&path).copied(),
            inputs.marks.get(&path).copied(),
            false,
        )?;
        decisions.add(&path, decision);
        match built {
            Built::Engine(file) => files.push(*file),
            Built::Opaque(file) => {
                opaque.insert(path, *file);
            }
            Built::Absent => {}
        }
    }
    Ok((files, opaque, decisions))
}

/// What an incremental update of one swimlane will commit, worked out before any of it is written.
struct IncrementalPlan {
    swimlane: SwimlaneId,
    entry: LaneEntry,
    /// The paths the update was about.
    affected: usize,
    writes: Vec<RowWrite>,
    deletes: Vec<NfsPath>,
    drift: Vec<NfsPath>,
    baselines: Vec<super::persist::BaselineWrite>,
    mark_clears: Vec<NfsPath>,
    findings: findings_sync::FindingsDelta,
    touched: Vec<LogicalFile>,
    counts: BTreeMap<domain::DriftState, u64>,
    rows: BTreeMap<(domain::FindingKind, String), FindingRow>,
}

/// A swimlane of an incremental batch while its paths are rebuilt.
struct Prepared {
    rules: LaneRules,
    entry: LaneEntry,
    affected: BTreeSet<NfsPath>,
    old: BTreeMap<NfsPath, Option<RowSig>>,
    inputs: Inputs,
    logicals: BTreeSet<LogicalFile>,
    decisions: Decisions,
}

impl Inner {
    /// The swimlanes among `lanes` whose first inventory has been applied.
    pub(crate) async fn with_inventory(
        &self,
        lanes: &[SwimlaneId],
    ) -> Result<BTreeSet<SwimlaneId>, RegistryError> {
        let ids: Vec<&str> = lanes.iter().map(SwimlaneId::as_str).collect();
        let found: Vec<String> = self
            .db
            .run(
                "pipeline_inventory",
                sqlx::query_scalar(
                    "SELECT swimlane_id FROM swimlane_registry \
                     WHERE swimlane_id = ANY($1::text[]) AND first_inventory_at IS NOT NULL ORDER BY swimlane_id LIMIT $2",
                )
                .bind(&ids)
                .bind(i64::try_from(lanes.len().max(1)).unwrap_or(i64::MAX))
                .fetch_all(self.db.pool()),
            )
            .await?;
        found
            .iter()
            .map(|id| SwimlaneId::parse(id).map_err(|_| RegistryError::Corrupt))
            .collect()
    }

    /// The rules of these swimlanes and the heads of the mirror, once for a batch.
    pub(crate) async fn prepare(
        &self,
        lanes: &[SwimlaneId],
    ) -> Result<(BTreeMap<SwimlaneId, LaneRules>, Arc<GitView>), RegistryError> {
        let timeout = self.config.git_timeout;
        let git = Arc::clone(&self.git);
        let live = blocking(timeout, move || live_heads(git.as_ref())).await?;
        let deployable = TenantSet::new(live.tenants.keys().cloned()).map_err(|_| RegistryError::Capacity)?;
        let global = self.rules.global(deployable).await?;
        let rules = self.rules.lanes(&global, lanes).await?;
        let needed: BTreeSet<_> = rules.values().flat_map(|r| r.tenants.iter().cloned()).collect();
        let (git, index) = (Arc::clone(&self.git), self.git_index.clone());
        let view = blocking(timeout, move || {
            GitView::resolve(&index, git.as_ref(), &live, &needed)
        })
        .await?;
        Ok((rules, Arc::new(view)))
    }

    /// Bring the swimlanes with work up to date. Errors that stop the whole batch (the rules, the mirror) are returned; one swimlane's
    /// failure is in the report.
    pub(crate) async fn process(&self, work: Vec<(SwimlaneId, Work)>) -> Result<BatchReport, RegistryError> {
        let started = Instant::now();
        let mut report = BatchReport::default();
        let work = merge(work);
        if work.is_empty() {
            return Ok(report);
        }
        let ids: Vec<SwimlaneId> = work.keys().cloned().collect();
        let inventory = self.with_inventory(&ids).await?;
        let live_ids: Vec<SwimlaneId> = ids.iter().filter(|id| inventory.contains(id)).cloned().collect();
        for id in ids.iter().filter(|id| !inventory.contains(id)) {
            self.metrics.pipeline_lane(LaneResult::NoInventory);
            report.skipped.push(id.clone());
        }
        if live_ids.is_empty() {
            return Ok(report);
        }
        let (rules, view) = self.prepare(&live_ids).await?;
        let mut exec = self.exec.lock().await;

        let mut wholes = Vec::new();
        let mut incrementals = Vec::new();
        for (id, w) in work {
            if !inventory.contains(&id) {
                continue;
            }
            let Some(lane) = rules.get(&id) else {
                exec.store.remove(&id);
                report.skipped.push(id);
                continue;
            };
            let current = exec
                .store
                .get(&id)
                .is_some_and(|e| e.fingerprint == lane.fingerprint);
            match w {
                Work::Paths(paths) if current && !paths.is_empty() => {
                    incrementals.push((lane.clone(), paths));
                }
                Work::Paths(paths) if current && paths.is_empty() => {}
                _ => wholes.push(lane.clone()),
            }
        }
        let mut results = self.refresh_lanes(&mut exec, wholes, &view).await;
        results.extend(self.update_lanes(&mut exec, incrementals, &view).await);
        for (id, result) in results {
            match result {
                Ok(lane) => {
                    self.metrics.pipeline_lane(lane.result);
                    report.lanes.push(lane);
                }
                Err(error) => {
                    self.metrics.pipeline_lane(LaneResult::Failed);
                    warn!(swimlane = %id, ?error, "the pipeline could not bring a swimlane up to date");
                    report.failed.push(id);
                }
            }
        }
        self.metrics.pipeline_lanes_in_memory(exec.store.len());
        // The tenant-suffix review (T4) reads what the batch computed, then lets go of the memory: a ranking over the mirror's trees
        // must not hold up the next batch.
        let checks = self.tenant_checks(&exec, &report);
        drop(exec);
        self.review_tenants(checks).await;
        self.metrics
            .pipeline_batch_seconds(started.elapsed().as_secs_f64());
        debug!(
            lanes = report.lanes.len(),
            failed = report.failed.len(),
            skipped = report.skipped.len(),
            "pipeline batch"
        );
        Ok(report)
    }

    // ------------------------------------------------------------------------------------------------ whole swimlanes

    /// Load and compute these swimlanes from scratch, and make the database say what the computation says. A swimlane that was in
    /// memory is replaced, and the paths where it disagreed with the new computation are counted as repaired.
    #[allow(clippy::too_many_lines)] // one pass in numbered steps, which share their locals
    pub(crate) async fn refresh_lanes(
        &self,
        exec: &mut Exec,
        lanes: Vec<LaneRules>,
        view: &Arc<GitView>,
    ) -> Vec<(SwimlaneId, Result<LaneReport, RegistryError>)> {
        let mut results = Vec::new();
        if lanes.is_empty() {
            return results;
        }
        let ids: Vec<SwimlaneId> = lanes.iter().map(|r| r.swimlane.clone()).collect();
        // What memory held, to count what the new computation disagrees with. The entries leave memory: they come back after the commit.
        let mut previous: BTreeMap<SwimlaneId, LaneEntry> = ids
            .iter()
            .filter_map(|id| exec.store.take(id).map(|e| (id.clone(), e)))
            .collect();
        // 1. What the database holds, for each swimlane.
        let mut loaded = Vec::new();
        for rules in lanes {
            match self.loader.load_lane(&rules.swimlane).await {
                Ok(inputs) => loaded.push((rules, inputs)),
                Err(e) => results.push((rules.swimlane.clone(), Err(e))),
            }
        }
        // 2. The engine's files, built on a blocking thread: a path is mapped, classified and looked up in Git.
        let view_for_build = Arc::clone(view);
        let timeout = self.config.git_timeout.max(std::time::Duration::from_secs(60));
        let built = blocking(timeout, move || {
            let mut out = Vec::with_capacity(loaded.len());
            for (rules, inputs) in loaded {
                let made = build_lane(&rules, &view_for_build, &inputs);
                out.push((rules, inputs, made));
            }
            Ok(out)
        })
        .await;
        let built = match built {
            Ok(b) => b,
            Err(e) => {
                // The blocking call took the inputs with it: every swimlane of the batch fails and is tried again.
                results.extend(ids.into_iter().map(|id| (id, Err(e))));
                return results;
            }
        };
        let mut jobs: Vec<FullJob> = Vec::new();
        for (rules, inputs, made) in built {
            match made {
                Ok((files, opaque, decisions)) => jobs.push(FullJob {
                    rules,
                    inputs,
                    files,
                    opaque,
                    decisions,
                }),
                Err(e) => results.push((rules.swimlane.clone(), Err(e))),
            }
        }
        if jobs.is_empty() {
            return results;
        }
        // 3. The content of every file the engine will read: all of them, once for the batch.
        let mut wanted = BTreeSet::new();
        for job in &jobs {
            wanted.extend(blobs_of(&job.files, &all_with_content(&job.files)));
        }
        let bytes = match self.loader.fetch_bytes(&wanted).await {
            Ok(b) => b,
            Err(e) => {
                results.extend(jobs.iter().map(|j| (j.rules.swimlane.clone(), Err(e))));
                return results;
            }
        };
        drop(wanted);
        for job in &mut jobs {
            let indices = all_with_content(&job.files);
            let missing = fill(&mut job.files, &indices, &bytes);
            if !missing.is_empty() {
                Self::demote(job, &missing, view);
            }
        }
        drop(bytes);
        // 4. The engine, on the registry's pool.
        let (pool, limits) = (Arc::clone(&self.pool), Arc::clone(&self.limits));
        let hosts = Arc::clone(&jobs[0].rules.global.hosts);
        let computed = blocking(
            self.config.git_timeout.max(std::time::Duration::from_secs(600)),
            move || {
                let states = compute_full(&pool, &hosts, &limits, &jobs);
                Ok((jobs, states))
            },
        )
        .await;
        let (jobs, states) = match computed {
            Ok(done) => done,
            Err(e) => {
                fail_rest(&mut results, ids, e);
                return results;
            }
        };
        // 5. Each swimlane's difference into the database.
        for (job, state) in jobs.into_iter().zip(states) {
            let id = job.rules.swimlane.clone();
            let outcome = match state {
                Ok(state) => {
                    self.finish_full(exec, previous.remove(&id), job, state, view)
                        .await
                }
                Err(error) => {
                    warn!(swimlane = %id, ?error, "the engine refused a swimlane");
                    Err(RegistryError::Invalid)
                }
            };
            results.push((id, outcome));
        }
        results
    }

    /// A file whose content the store did not have is computed from its hashes alone, like a denied one.
    fn demote(job: &mut FullJob, missing: &[NfsPath], view: &Arc<GitView>) {
        let ctx = BuildCtx {
            lane: &job.rules,
            view,
        };
        for path in missing {
            let built = build_path(
                &ctx,
                path,
                job.inputs.rows.get(path),
                job.inputs.baselines.get(path).copied(),
                job.inputs.marks.get(path).copied(),
                true,
            );
            if let Ok((Built::Opaque(file), _)) = built {
                if let Ok(i) = job.files.binary_search_by(|f| f.path.cmp(path)) {
                    job.files.remove(i);
                }
                job.opaque.insert(path.clone(), *file);
            }
            warn!(swimlane = %job.rules.swimlane, "a file's content is missing from the blob store");
        }
    }

    /// The end of a whole load: the entry, the decisions, the difference with what the database holds, and the commit.
    async fn finish_full(
        &self,
        exec: &mut Exec,
        previous: Option<LaneEntry>,
        job: FullJob,
        state: LaneState,
        view: &Arc<GitView>,
    ) -> Result<LaneReport, RegistryError> {
        let FullJob {
            rules,
            inputs,
            files,
            opaque,
            decisions,
        } = job;
        let mut entry = LaneEntry::new(
            rules.swimlane.clone(),
            rules.tenants.clone(),
            rules.apps.clone(),
            rules.tier.clone(),
            rules.fingerprint,
            rules.adopted,
            Arc::clone(&rules.global.severity),
            view.heads(&rules.tenants),
            files,
            opaque,
            state,
        );
        clear(&mut entry.files);
        let paths = entry.paths();

        let mut writes = Vec::new();
        let mut drift: BTreeSet<NfsPath> = BTreeSet::new();
        for path in &paths {
            let Some(sig) = entry.sig(path) else { continue };
            let stored = inputs.rows.get(path).map(|r| &r.persisted);
            if stored.is_none_or(|s| !sig.matches(s)) {
                drift.insert(path.clone());
                writes.push(RowWrite {
                    path: path.clone(),
                    sig,
                });
            }
        }
        let in_lane: BTreeSet<&NfsPath> = paths.iter().collect();
        let deletes: Vec<NfsPath> = inputs
            .rows
            .iter()
            .filter(|(p, row)| row.nfs.is_none() && !in_lane.contains(p))
            .map(|(p, _)| p.clone())
            .collect();
        drift.extend(deletes.iter().cloned());

        let Some(state) = entry.state.as_ref() else {
            return Err(RegistryError::Corrupt);
        };
        let rows = rows_of(state, &rules.global.severity);
        let stored: BTreeMap<_, _> = self
            .persist
            .open_findings(&rules.swimlane)
            .await?
            .into_iter()
            .filter_map(findings_sync::row_from_stored)
            .map(|r: FindingRow| ((r.kind, r.fingerprint.clone()), r))
            .collect();
        let delta = findings_sync::diff(&stored, &rows);
        let counts = entry.state_counts();
        let committed = self
            .persist
            .commit(LaneCommit {
                swimlane: &rules.swimlane,
                state,
                writes,
                deletes,
                baselines: decisions.baselines,
                mark_clears: decisions.mark_clears,
                drift_paths: drift.into_iter().collect(),
                findings: delta,
                tree: TreeMode::Sync,
                counts,
                finding_count: rows.len(),
                fingerprint: Some(rules.fingerprint),
            })
            .await?;
        entry.findings = rows;

        let repaired = previous.as_ref().map_or(0, |before| {
            let mut all: BTreeSet<NfsPath> = before.paths().into_iter().collect();
            all.extend(paths.iter().cloned());
            all.iter().filter(|p| before.sig(p) != entry.sig(p)).count()
        });
        self.metrics.pipeline_divergences(repaired as u64);
        self.metrics.pipeline_paths(paths.len() as u64);
        let mut lane = LaneReport::new(rules.swimlane.clone(), LaneResult::Whole, paths.len());
        lane.rows_written = committed.rows_written;
        lane.rows_deleted = committed.rows_deleted;
        lane.drift_paths = committed.drift_paths;
        lane.findings_changed = committed.findings_written;
        lane.baselines_set = committed.baselines;
        lane.marks_cleared = committed.marks_cleared;
        lane.repaired = repaired;
        if exec.store.put(entry).is_some() {
            debug!("a swimlane left memory to make room");
        }
        Ok(lane)
    }

    // ------------------------------------------------------------------------------------------------ incremental

    /// Bring swimlanes that are in memory up to date after some of their paths changed.
    #[allow(clippy::too_many_lines)] // one pass in numbered steps, which share their locals
    pub(crate) async fn update_lanes(
        &self,
        exec: &mut Exec,
        items: Vec<(LaneRules, BTreeSet<NfsPath>)>,
        view: &Arc<GitView>,
    ) -> Vec<(SwimlaneId, Result<LaneReport, RegistryError>)> {
        let mut results: Vec<(SwimlaneId, Result<LaneReport, RegistryError>)> = Vec::new();
        if items.is_empty() {
            return results;
        }
        // 1. What the database holds for the changed paths, of all the swimlanes together.
        let pairs: Vec<(SwimlaneId, NfsPath)> = items
            .iter()
            .flat_map(|(rules, paths)| paths.iter().map(|p| (rules.swimlane.clone(), p.clone())))
            .collect();
        let mut inputs = match self.loader.load_paths(&pairs).await {
            Ok(i) => i,
            Err(e) => {
                for (rules, _) in items {
                    exec.store.remove(&rules.swimlane);
                    results.push((rules.swimlane, Err(e)));
                }
                return results;
            }
        };
        let ids: Vec<SwimlaneId> = items.iter().map(|(r, _)| r.swimlane.clone()).collect();
        // 2. Each swimlane's entry, what it was, and the changed paths rebuilt from what the database says now.
        let mut prepared = Vec::new();
        for (rules, affected) in items {
            let Some(mut entry) = exec.store.take(&rules.swimlane) else {
                results.push((rules.swimlane.clone(), Err(RegistryError::Corrupt)));
                continue;
            };
            let old = affected.iter().map(|p| (p.clone(), entry.sig(p))).collect();
            let logicals = affected.iter().filter_map(|p| entry.logical_of(p)).collect();
            entry.adopted = rules.adopted;
            entry.severity = Arc::clone(&rules.global.severity);
            let lane_inputs = inputs.remove(&rules.swimlane).unwrap_or_default();
            prepared.push(Prepared {
                rules,
                entry,
                affected,
                old,
                inputs: lane_inputs,
                logicals,
                decisions: Decisions::default(),
            });
        }
        let view_for_build = Arc::clone(view);
        let timeout = self.config.git_timeout.max(std::time::Duration::from_secs(60));
        let built = blocking(timeout, move || {
            let mut out = Vec::with_capacity(prepared.len());
            for mut p in prepared {
                let ctx = BuildCtx {
                    lane: &p.rules,
                    view: &view_for_build,
                };
                let mut error = None;
                for path in &p.affected {
                    match build_path(
                        &ctx,
                        path,
                        p.inputs.rows.get(path),
                        p.inputs.baselines.get(path).copied(),
                        p.inputs.marks.get(path).copied(),
                        false,
                    ) {
                        Ok((built, decision)) => {
                            place(&mut p.entry, path, built);
                            p.decisions.add(path, decision);
                        }
                        Err(e) => {
                            error = Some(e);
                            break;
                        }
                    }
                }
                out.push((p, error));
            }
            Ok(out)
        })
        .await;
        let built = match built {
            Ok(b) => b,
            Err(e) => {
                // The entries went with the closure: the swimlanes are loaded again from the database.
                fail_rest(&mut results, ids, e);
                return results;
            }
        };
        let mut ready = Vec::new();
        for (mut p, error) in built {
            if let Some(e) = error {
                results.push((p.rules.swimlane.clone(), Err(e)));
                continue;
            }
            for path in &p.affected {
                if let Some(l) = p.entry.logical_of(path) {
                    p.logicals.insert(l);
                }
            }
            ready.push(p);
        }
        // 3. The content the engine reads, for all the swimlanes together.
        let mut wanted = BTreeSet::new();
        let mut read_sets = Vec::new();
        for p in &ready {
            let indices = files_to_read(&p.entry, &p.logicals, &p.affected);
            wanted.extend(blobs_of(&p.entry.files, &indices));
            read_sets.push(indices);
        }
        let bytes = match self.loader.fetch_bytes(&wanted).await {
            Ok(b) => b,
            Err(e) => {
                results.extend(ready.iter().map(|p| (p.rules.swimlane.clone(), Err(e))));
                return results;
            }
        };
        for (p, indices) in ready.iter_mut().zip(&read_sets) {
            let missing = fill(&mut p.entry.files, indices, &bytes);
            let ctx = BuildCtx { lane: &p.rules, view };
            for path in missing {
                // Not in the store: computed from the hashes alone. Its row is read again, because the file's content is what is
                // missing, not its record.
                let built = build_path(
                    &ctx,
                    &path,
                    p.inputs.rows.get(&path),
                    p.inputs.baselines.get(&path).copied(),
                    p.inputs.marks.get(&path).copied(),
                    true,
                );
                if let Ok((built, _)) = built {
                    place(&mut p.entry, &path, built);
                }
                warn!(swimlane = %p.rules.swimlane, "a file's content is missing from the blob store");
            }
        }
        drop(bytes);
        drop(wanted);
        // 4. The engine.
        let mut jobs: Vec<IncJob> = Vec::with_capacity(ready.len());
        let mut keep: Vec<(LaneRules, BTreeMap<NfsPath, Option<RowSig>>, Decisions)> =
            Vec::with_capacity(ready.len());
        for mut p in ready {
            let previous = p.entry.state.take();
            jobs.push(IncJob {
                entry: p.entry,
                previous,
                affected: p.affected,
            });
            keep.push((p.rules, p.old, p.decisions));
        }
        let (pool, limits) = (Arc::clone(&self.pool), Arc::clone(&self.limits));
        let hosts = keep.first().map_or_else(
            || Arc::new(HostRules::default()),
            |(r, _, _)| Arc::clone(&r.global.hosts),
        );
        let computed = blocking(
            self.config.git_timeout.max(std::time::Duration::from_secs(120)),
            move || {
                let mut jobs = jobs;
                let states = compute_paths(&pool, &hosts, &limits, &mut jobs);
                Ok((jobs, states))
            },
        )
        .await;
        let (jobs, states) = match computed {
            Ok(done) => done,
            Err(e) => {
                fail_rest(&mut results, ids, e);
                return results;
            }
        };
        // 5. Each swimlane's difference into the database: worked out here, one swimlane after the other, and committed `persist_concurrency`
        // at a time (a commit is a transaction on a connection of its own, and the time of a push that concerns every swimlane is
        // mostly the commits' waiting for the disk).
        let mut planned = Vec::with_capacity(jobs.len());
        for ((job, state), (rules, old, decisions)) in jobs.into_iter().zip(states).zip(keep) {
            let id = rules.swimlane.clone();
            match state {
                Ok(state) => match Self::plan_incremental(job, state, &rules, &old, decisions, view) {
                    Ok(plan) => planned.push(plan),
                    Err(error) => results.push((id, Err(error))),
                },
                Err(error) => {
                    warn!(swimlane = %id, ?error, "the engine refused an update");
                    results.push((id, Err(RegistryError::Invalid)));
                }
            }
        }
        let committed: Vec<_> = futures::stream::iter(planned)
            .map(|plan| self.commit_incremental(plan))
            .buffer_unordered(self.config.persist_concurrency)
            .collect()
            .await;
        for (id, outcome) in committed {
            match outcome {
                Ok((entry, lane)) => {
                    exec.store.put(entry);
                    results.push((id, Ok(lane)));
                }
                // The entry is gone with the failed commit: the swimlane is loaded again from the database.
                Err(error) => results.push((id, Err(error))),
            }
        }
        results
    }

    /// The end of an update, first half: the new state, the rows that changed, the findings, and what the commit will carry. Touches
    /// neither the database nor the store.
    fn plan_incremental(
        job: IncJob,
        state: LaneState,
        rules: &LaneRules,
        old: &BTreeMap<NfsPath, Option<RowSig>>,
        decisions: Decisions,
        view: &Arc<GitView>,
    ) -> Result<IncrementalPlan, RegistryError> {
        let IncJob {
            mut entry,
            previous,
            affected,
        } = job;
        let Some(previous) = previous else {
            return Err(RegistryError::Corrupt);
        };
        let touched = touched(&previous, &state);
        entry.state = Some(state);
        clear(&mut entry.files);
        entry.heads = view.heads(&entry.tenants);

        let mut writes = Vec::new();
        let mut deletes = Vec::new();
        let mut drift = Vec::new();
        for path in &affected {
            let before = old.get(path).cloned().flatten();
            match (before, entry.sig(path)) {
                (None, Some(now)) => {
                    drift.push(path.clone());
                    writes.push(RowWrite {
                        path: path.clone(),
                        sig: now,
                    });
                }
                (Some(_), None) => {
                    drift.push(path.clone());
                    deletes.push(path.clone());
                }
                (Some(before), Some(now)) => {
                    if before != now {
                        drift.push(path.clone());
                    }
                    if !before.derived_eq(&now) {
                        writes.push(RowWrite {
                            path: path.clone(),
                            sig: now,
                        });
                    }
                }
                (None, None) => {}
            }
        }
        let Some(state) = entry.state.as_ref() else {
            return Err(RegistryError::Corrupt);
        };
        let rows = rows_of(state, &rules.global.severity);
        let findings = findings_sync::diff(&entry.findings, &rows);
        let counts = entry.state_counts();
        Ok(IncrementalPlan {
            swimlane: rules.swimlane.clone(),
            entry,
            affected: affected.len(),
            writes,
            deletes,
            drift,
            baselines: decisions.baselines,
            mark_clears: decisions.mark_clears,
            findings,
            touched,
            counts,
            rows,
        })
    }

    /// The end of an update, second half: the commit, and the entry back for the store once it is in the database.
    async fn commit_incremental(
        &self,
        plan: IncrementalPlan,
    ) -> (SwimlaneId, Result<(LaneEntry, LaneReport), RegistryError>) {
        let IncrementalPlan {
            swimlane,
            mut entry,
            affected,
            writes,
            deletes,
            drift,
            baselines,
            mark_clears,
            findings,
            touched,
            counts,
            rows,
        } = plan;
        let committed = match entry.state.as_ref() {
            None => Err(RegistryError::Corrupt),
            Some(state) => {
                self.persist
                    .commit(LaneCommit {
                        swimlane: &swimlane,
                        state,
                        writes,
                        deletes,
                        baselines,
                        mark_clears,
                        drift_paths: drift,
                        findings,
                        tree: TreeMode::Apply(touched),
                        counts,
                        finding_count: rows.len(),
                        fingerprint: None,
                    })
                    .await
            }
        };
        let outcome = committed.map(|committed| {
            entry.findings = rows;
            self.metrics.pipeline_paths(affected as u64);
            let mut lane = LaneReport::new(swimlane.clone(), LaneResult::Incremental, affected);
            lane.rows_written = committed.rows_written;
            lane.rows_deleted = committed.rows_deleted;
            lane.drift_paths = committed.drift_paths;
            lane.findings_changed = committed.findings_written;
            lane.baselines_set = committed.baselines;
            lane.marks_cleared = committed.marks_cleared;
            (entry, lane)
        });
        (swimlane, outcome)
    }
}

impl Pipeline {
    /// The work an event means, as the job would queue it, without waiting for a window: for tests and diagnostics.
    ///
    /// # Errors
    ///
    /// The mirror could not say what a moved head changed.
    pub async fn event_work(
        &self,
        event: domain::DomainEvent,
    ) -> Result<Vec<(SwimlaneId, Work)>, RegistryError> {
        let mut scratch = super::coalesce::Coalescer::new(
            std::time::Duration::ZERO,
            self.inner.config.max_affected_paths,
            self.inner.config.max_coalesced_lanes,
        );
        self.inner
            .route(event, &mut scratch, tokio::time::Instant::now())
            .await?;
        Ok(scratch.take_all())
    }

    /// Bring the swimlanes with work up to date, now. The job calls this for the windows that closed; tests and the benchmarks call it
    /// directly.
    ///
    /// # Errors
    ///
    /// The rules or the Git mirror could not be read; nothing was changed.
    pub async fn process(&self, work: Vec<(SwimlaneId, Work)>) -> Result<BatchReport, RegistryError> {
        self.inner.process(work).await
    }

    /// Compute one swimlane from scratch and make the database say what the computation says (a swimlane with no inventory yet is
    /// skipped).
    ///
    /// # Errors
    ///
    /// As [`process`](Self::process).
    pub async fn refresh_lane(&self, swimlane: &SwimlaneId) -> Result<BatchReport, RegistryError> {
        self.inner.process(vec![(swimlane.clone(), Work::Whole)]).await
    }
}
