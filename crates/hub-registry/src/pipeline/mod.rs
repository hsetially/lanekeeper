//! The incremental pipeline (T3, D62, D63, D66, D80; P1, P2, P5, P6, P13): drift, effective configuration and findings, kept
//! up to date by events and repaired by a nightly full recompute.
//!
//! # What it does
//!
//! Ingestion (T1) writes what an agent saw: the NFS columns of `file_state`, and a `FileObserved` event for each path that moved. The
//! Git mirror publishes `GitHeadMoved`. The pipeline turns both into **the (swimlane, path) pairs whose drift can have changed**,
//! recomputes those and nothing else with the engine ([`engine::recompute_paths`]), and writes the difference in one transaction for
//! each swimlane ([`persist`]): the derived columns of `file_state`, the findings, the effective tree hashes, the swimlane's counts,
//! and the events `DriftChanged` and `FindingsChanged` (D80: through the outbox, in the same transaction).
//!
//! | Piece | Module |
//! |---|---|
//! | the job: one lease, the event loop, the coalescing timer, the idle warm-up | [`job`] |
//! | events and in-process signals into per-swimlane work; Git heads into paths | [`events`], [`affected`] |
//! | 200 ms windows per swimlane, bounded | [`coalesce`] |
//! | the swimlanes in memory: file metadata and the engine's state, never content | [`lane_store`] |
//! | the engine's input from the database and Git; content only where the engine reads it | [`lane_loader`], [`git_side`] |
//! | the rules a swimlane is computed under and their fingerprint | [`rules`] |
//! | one batch: whole-swimlane loads and incremental updates | `batch` |
//! | baselines set by convergence and stale marks, decided with the build of each path | [`lane_loader`] (`Decision`) |
//! | the difference into the database, by transaction | [`persist`], [`findings_sync`] |
//! | the comparison of memory with the database and Git | [`reconcile`] |
//! | the nightly full recompute that repairs any divergence | [`nightly`] |
//! | a swimlane whose NFS carries the suffix of a branch it does not deploy: the ranking and the admin's review (T4) | [`tenant_rematch`] |
//! | the pickup state of the swimlanes a batch brought up to date (T10): run by the job after the batch is committed, never inside `process` | [`crate::pickup`] |
//!
//! # One lease, one holder of the state
//!
//! The job `registry-pipeline` is leased ([`hub_platform::jobs`]), so one replica at a time holds every swimlane's [`LaneState`]
//! (about 25 MB for 40 swimlanes of 2,000 files: hashes and findings, never content). A replica that takes over starts empty and loads a
//! swimlane the first time something touches it (one full recompute of that swimlane, about 4.5 s at design scale), after a reconcile that
//! finds what was missed while nobody held the lease, and a low-priority warm-up that verifies every other swimlane while the loop is
//! idle. A replica that loses the lease forgets its memory: what it knew is no longer the truth.
//!
//! # Why the result equals a full recompute
//!
//! Everything the pipeline writes is a function of the source facts (the NFS columns, the baselines, the marks), Git and the rules, so the
//! same function applied to all paths is the nightly job. The test `incremental_state_equals_full_recompute_after_random_events`
//! holds the two together after random sequences of events; the nightly job counts what it had to repair
//! (`registry_pipeline_divergences`), which should stay at zero.
//!
//! # What it does not do
//!
//! - C1 in `whole_file` mode needs the base file's history from Git (`LaneFile::history`); no history is given here, so that check
//!   reports only dead overrides (`spring_merge`). C9 to C11 and the "as served" check are written by T10.
//! - It does not audit what it derives, only what it decides (plan A12): baselines set by convergence (one event for a commit, with
//!   the paths) and marks cleared.
//! - It does not decide what is serious: the rules are the engine's defaults and the admin's rows of `severity_rules`
//!   ([`crate::severity`]), read with the rest of the rules of every batch. What they rate is written with the state (`file_state.severity`,
//!   the observation a row points at, the findings), and the set is part of the rules fingerprint, so a change of the rows is a whole recompute.

pub mod affected;
pub(crate) mod batch;
pub mod coalesce;
pub mod events;
pub mod findings_sync;
pub mod git_side;
pub mod job;
pub mod lane_loader;
pub mod lane_store;
pub mod nightly;
pub mod persist;
pub mod reconcile;
pub mod rules;
pub mod tenant_rematch;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use domain::{CommitId, RepoKind, SwimlaneId, TenantId};
use engine::DenyMatcher;
use hub_platform::jobs::{Job, JobSpec, SpecError};
use ports::{AuditLog, BlobStore, EventBus, GitReader};
use sqlx::PgPool;
use tokio::sync::{Mutex, mpsc};

pub use self::batch::{BatchReport, LaneReport};
pub use self::coalesce::Work;
pub use self::events::PipelineHandle;
pub use self::job::PipelineJob;
use self::lane_loader::LaneLoader;
use self::lane_store::LaneStore;
pub use self::nightly::{NightlyJob, NightlyReport};
use self::persist::Persist;
pub use self::reconcile::ReconcileOutcome;
use self::rules::RuleSource;
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::index::{GitIndex, GitIndexDeps, TreeHashStore};
use crate::metrics::RegistryMetrics;
use crate::pickup::{PickupDeps, Pickups};
use crate::severity::SeverityRuleSource;

/// A job and its spec, as the hub's job runner takes them.
pub type JobEntry = (JobSpec, Arc<dyn Job>);

/// The job that keeps the swimlanes up to date. Its name is its lease's name.
pub const PIPELINE_JOB: &str = "registry-pipeline";
/// The nightly full recompute. Its name is its lease's name.
pub const NIGHTLY_JOB: &str = "registry-nightly";

/// What the pipeline needs from the rest of the hub.
pub struct PipelineDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub git: Arc<dyn GitReader>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// Where a finding that became drift worth knowing about is offered after its commit (T8): held while a sync window is open, sent
    /// otherwise. `None`: the pipeline alerts nobody.
    pub alerts: Option<crate::sync::alerts::Alerts>,
}

impl std::fmt::Debug for PipelineDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineDeps").finish_non_exhaustive()
    }
}

/// Everything one operation of the pipeline works on. Operations take the lock one at a time, so the memory is never seen half changed.
pub(crate) struct Exec {
    pub store: LaneStore,
}

pub(crate) struct Inner {
    pub db: Db,
    pub git: Arc<dyn GitReader>,
    pub git_index: GitIndex,
    pub bus: Arc<dyn EventBus>,
    /// For the audit event of a tenant-suffix review ([`tenant_rematch`]); the rest of the pipeline audits through `persist`.
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub loader: LaneLoader,
    pub persist: Persist,
    pub rules: RuleSource,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// The pickup state (T10): computed for the swimlanes of each batch by the job, after the batch is committed.
    pub pickups: Pickups,
    pub pool: Arc<rayon::ThreadPool>,
    pub limits: Arc<engine::Limits>,
    pub exec: Mutex<Exec>,
    pub signals: Mutex<mpsc::Receiver<events::Signal>>,
    /// A sender of the pipeline's own, so that the queue stays open whether or not anyone holds a [`PipelineHandle`]: a closed queue
    /// would read as the end of the signals and end the run.
    pub _signal_tx: mpsc::Sender<events::Signal>,
    /// The last run ended on its own, with the lease still held, so the memory can be trusted by the next one.
    pub clean_end: AtomicBool,
    /// Where each deployable branch was when the last move was handled: what the reconcile compares the mirror with.
    pub processed: std::sync::Mutex<BTreeMap<(RepoKind, String), CommitId>>,
    /// The suffixes of each adopted swimlane's NFS that its tenants do not include, as the last tenant-suffix review saw them
    /// ([`tenant_rematch`]). Only what saves a batch a query: the review's own rows are the truth.
    pub rematched: std::sync::Mutex<BTreeMap<SwimlaneId, BTreeSet<TenantId>>>,
}

/// The pipeline. Cloning shares it.
#[derive(Clone)]
pub struct Pipeline {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").finish_non_exhaustive()
    }
}

impl Pipeline {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: PipelineDeps) -> Result<(Self, PipelineHandle), ConfigError> {
        let PipelineDeps {
            pool,
            blobs,
            git,
            bus,
            audit,
            clock,
            config,
            metrics,
            alerts,
        } = deps;
        config.validate()?;
        let db = Db::new(pool.clone(), config.query_timeout);
        let git_index = GitIndex::new(GitIndexDeps {
            pool: pool.clone(),
            git: Arc::clone(&git),
            config: config.clone(),
            metrics: metrics.clone(),
        })?;
        let severity = SeverityRuleSource::new(pool.clone(), &config, metrics.clone())?;
        let deny = Arc::new(
            DenyMatcher::with_globs(config.extra_deny_globs.iter().map(String::as_str))
                .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?,
        );
        let threads = rayon::ThreadPoolBuilder::new()
            .num_threads(config.pool_threads)
            .thread_name(|i| format!("registry-pipeline-{i}"))
            .build()
            .map_err(|_| ConfigError::Invalid("pool_threads"))?;
        let tree = TreeHashStore::new(db.clone(), Arc::clone(&clock), &config, metrics.clone());
        let persist = Persist::new(
            db.clone(),
            Arc::clone(&bus),
            Arc::clone(&audit),
            Arc::clone(&clock),
            tree,
            metrics.clone(),
            config.rows_per_statement,
        )
        .with_alerts(alerts);
        let (signal_tx, signal_rx) = mpsc::channel(config.signal_queue);
        let pickups = Pickups::new(PickupDeps {
            pool: pool.clone(),
            bus: Arc::clone(&bus),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })?;
        let inner = Inner {
            loader: LaneLoader::new(db.clone(), blobs, config.blob_timeout),
            rules: RuleSource::new(db.clone(), deny, config.extra_deny_globs.clone(), severity),
            db,
            git,
            git_index,
            bus,
            audit,
            clock,
            persist,
            exec: Mutex::new(Exec {
                store: LaneStore::new(config.max_lanes_in_memory),
            }),
            signals: Mutex::new(signal_rx),
            _signal_tx: signal_tx.clone(),
            clean_end: AtomicBool::new(false),
            processed: std::sync::Mutex::new(BTreeMap::new()),
            rematched: std::sync::Mutex::new(BTreeMap::new()),
            pool: Arc::new(threads),
            limits: Arc::new(engine::Limits::default()),
            config,
            pickups,
            metrics: metrics.clone(),
        };
        let handle = PipelineHandle::new(signal_tx, metrics);
        Ok((
            Self {
                inner: Arc::new(inner),
            },
            handle,
        ))
    }

    /// The job that keeps the swimlanes up to date (`registry-pipeline`).
    pub fn job(&self) -> Arc<dyn Job> {
        Arc::new(PipelineJob::new(self.clone()))
    }

    /// The nightly full recompute (`registry-nightly`).
    pub fn nightly_job(&self) -> Arc<dyn Job> {
        Arc::new(NightlyJob::new(self.clone()))
    }

    /// How the pipeline job is scheduled: the holder asks for another run a second after one ends, under a 30 second lease that the
    /// runner renews every 10 seconds; a run lasts `pipeline_run` and then hands over to the next, with the memory kept.
    ///
    /// # Errors
    ///
    /// A spec the runner refuses (the run length is range-checked by `RegistryConfig::validate`).
    pub fn pipeline_spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(PIPELINE_JOB, Duration::from_secs(1), Duration::from_secs(30))?
            .with_max_run(config.pipeline_run + Duration::from_secs(60))
    }

    /// How the nightly job is scheduled: every `nightly_interval`, under a five minute lease, for at most an hour.
    ///
    /// # Errors
    ///
    /// A spec the runner refuses.
    pub fn nightly_spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(NIGHTLY_JOB, config.nightly_interval, Duration::from_secs(300))?
            .with_max_run(Duration::from_secs(3600))
    }

    /// The pickup state of the swimlanes (T10): the computation the job runs after each batch, and the sweeper's job.
    pub fn pickups(&self) -> Pickups {
        self.inner.pickups.clone()
    }

    /// The jobs with their specs, for the hub's job runner: the pipeline, the nightly recompute and the pickup sweeper.
    ///
    /// # Errors
    ///
    /// A spec the runner refuses.
    pub fn jobs(&self) -> Result<Vec<JobEntry>, SpecError> {
        Ok(vec![
            (Self::pipeline_spec(&self.inner.config)?, self.job()),
            (Self::nightly_spec(&self.inner.config)?, self.nightly_job()),
            (Pickups::spec(&self.inner.config)?, self.inner.pickups.job()),
        ])
    }

    /// The engine's state of a swimlane that is in memory, for tests and diagnostics.
    pub async fn lane_state(&self, swimlane: &SwimlaneId) -> Option<engine::LaneState> {
        let exec = self.inner.exec.lock().await;
        exec.store.get(swimlane).and_then(|e| e.state.clone())
    }

    /// The swimlanes in memory, in order.
    pub async fn lanes_in_memory(&self) -> Vec<SwimlaneId> {
        self.inner.exec.lock().await.store.ids()
    }

    /// Forget every swimlane: what a replica that lost the lease does. The next batch loads from the database.
    pub async fn forget_all(&self) {
        let mut exec = self.inner.exec.lock().await;
        exec.store.clear();
        self.inner.metrics.pipeline_lanes_in_memory(0);
    }
}

/// Run a blocking call on the blocking pool under the Git timeout (rule 4, rule 5). The call cannot be cancelled: a timeout frees the
/// caller, not the thread.
pub(crate) async fn blocking<T, F>(timeout: Duration, call: F) -> Result<T, crate::error::RegistryError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, crate::error::RegistryError> + Send + 'static,
{
    match tokio::time::timeout(timeout, tokio::task::spawn_blocking(call)).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(crate::error::RegistryError::Git(ports::GitError::Unavailable)),
        Err(_) => Err(crate::error::RegistryError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jobs_have_the_names_of_their_leases_and_valid_specs() {
        let config = RegistryConfig::default();
        let pipeline = Pipeline::pipeline_spec(&config).unwrap();
        assert_eq!(pipeline.name(), PIPELINE_JOB);
        assert_eq!(PIPELINE_JOB, "registry-pipeline");
        let nightly = Pipeline::nightly_spec(&config).unwrap();
        assert_eq!(nightly.name(), NIGHTLY_JOB);
        assert_eq!(NIGHTLY_JOB, "registry-nightly");
    }
}
