//! The registry in the composed hub (plan 18, T5; S4, S9, S10, S11, D79, D112, D116, D118): the ingestion that agents and sentinels
//! report to, the pipeline that turns reports and Git moves into drift and findings, the read API, the blob policy, the registrations
//! the write path makes, and the eleven jobs that keep it current.
//!
//! `hub-registry` is a library of parts with no `build`: each service has its own constructor, and D112 left the wiring to the hub.
//! This is the order they are put together in, from the settings and the platform's ports:
//!
//! 1. [`AgentIngest`] (the [`ReportSink`](ports::ReportSink) the agent service hands every report to) and [`SentinelIngest`] (the
//!    sentinel's), over the pool, the blob store, the event bus and the audit log. Every state change commits with its audit event and
//!    its outbox events in one transaction (rule 8, D80); nothing is fanned out before the commit.
//! 2. The settings index: the indexer ingestion offers every structured blob that becomes current to, its worker (a task of the
//!    hub's, stopped with it), and the leased sweeper that makes "every current structured blob is indexed" true whatever the queue
//!    lost.
//! 3. [`Pipeline`] with its four jobs, over the mirrors' reader, and [`Alerts`], which tell the notifier about drift worth knowing
//!    about once a sync window allows it.
//! 4. [`RegistryService`], the read API of 15 routes and the port the write path reads through; [`PgToolWrites`], the port the write
//!    path registers its writes with; [`RegistryBlobPolicy`], which the blob endpoint asks before it serves a hash.
//! 5. The remaining jobs: the tool-write matcher, the served verification, the two attribution jobs, the sync-window watcher, and,
//!    only if a person switches it on, retention.
//!
//! **One configuration.** Every service is built from clones of one [`RegistryConfig`], so the deny globs the agents are told, the
//! read API withholds, the blob policy refuses and the write path rejects are one list (05's evidence: "it must be built from the
//! same `RegistryConfig` as the read service").
//!
//! **Retention (G16).** `registry-retention` deletes blobs, and D116 and D118 are still to be confirmed at review. It is registered
//! only when `LK_RETENTION_ENABLED=true`, and then it reports and deletes nothing until `LK_RETENTION_DRY_RUN=false`.
//!
//! **The warm-up** of the text corpora is a task of each replica, not a leased job, and starts after the listeners are up so that it
//! cannot delay readiness (P11).

use std::sync::Arc;

use hub_platform::GuardedRouter;
use hub_platform::clock::Clock;
use hub_platform::jobs::{Job, JobRunner, JobSpec, SpecError};
use hub_registry::{
    AgentIngest, Alerts, AlertsDeps, AttributionDeps, ConfigError as RegistryConfigError, GRACE_JOB,
    IndexDeps, IngestDeps, PARTITIONS_JOB, PgToolWrites, Pipeline, PipelineDeps, ReadDeps, RegistryApi,
    RegistryBlobPolicy, RegistryConfig, RegistryMetrics, RegistryService, Retention, RetentionDeps,
    SentinelIngest, ServedDeps, ServedVerifier, SettingsIndexer, SettingsSweeper, SettingsWorker,
    SeverityRuleSource, SyncDeps, SyncService, ToolWriteDeps, ToolWriteMatcher, WATCH_JOB,
};
use ports::{AgentGateway, AuditLog, BlobStore, EventBus, GitReader, Notifier};
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::info;

use crate::compose::Module;
use crate::config::RegistrySettings;
use crate::edges::RegistryClock;
use crate::job_wrappers::JobEntry;
use crate::wiring::Tasks;

/// Why the registry could not be set up. Every message is fixed text that names a part, never a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryBuildError {
    /// A service refused the configuration. The settings were validated when they were read, so this is a mismatch between two parts
    /// of the registry and not a deployment's mistake.
    #[error("a registry service refused its configuration ({0})")]
    Config(&'static str),
    #[error("the settings index could not be attached to ingestion")]
    Index,
    /// The name of a job is a constant of the code, never a value.
    #[error("the registry job {0} could not be registered")]
    Job(String),
}

/// What [`Registry::build`] is made from: the platform's ports, and the settings.
pub struct RegistryInputs<'a> {
    pub settings: &'a RegistrySettings,
    pub pool: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub git: Arc<dyn GitReader>,
    pub gateway: Arc<dyn AgentGateway>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    /// Who is told about drift, an overwrite and a sync window that stayed open (Teams, from the write path's crate).
    pub notifier: Arc<dyn Notifier>,
    pub clock: Arc<dyn Clock>,
    /// Turns true when the hub stops: the registry's tasks end then.
    pub closing: watch::Receiver<bool>,
}

impl std::fmt::Debug for RegistryInputs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryInputs").finish_non_exhaustive()
    }
}

fn config_error(error: RegistryConfigError) -> RegistryBuildError {
    let (RegistryConfigError::OutOfRange(field) | RegistryConfigError::Invalid(field)) = error;
    RegistryBuildError::Config(field)
}

/// The registry, built. The jobs are held until [`Registry::register_jobs`], so that the runner starts after everything that
/// registers one has.
pub struct Registry {
    /// The report sink of agents.
    pub ingest: AgentIngest,
    /// The report sink of sentinels.
    pub sentinels: SentinelIngest,
    /// The read API and the port the write path reads through.
    pub service: RegistryService,
    /// The port the write path registers its writes with.
    pub tool_writes: PgToolWrites,
    /// What the blob endpoint asks before it serves a hash.
    pub blob_policy: Arc<RegistryBlobPolicy>,
    /// The counters of every part, registered once on the hub's registry.
    pub metrics: RegistryMetrics,
    jobs: Vec<JobEntry>,
    closing: watch::Receiver<bool>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("jobs", &self.jobs.len())
            .finish_non_exhaustive()
    }
}

/// Resolves when `closing` is true (or its sender is gone).
async fn closed(mut closing: watch::Receiver<bool>) {
    let _ = closing.wait_for(|stopping| *stopping).await;
}

fn add(
    jobs: &mut Vec<JobEntry>,
    name: &str,
    spec: Result<JobSpec, SpecError>,
    job: Arc<dyn Job>,
) -> Result<(), RegistryBuildError> {
    jobs.push((spec.map_err(|_| RegistryBuildError::Job(name.to_owned()))?, job));
    Ok(())
}

impl Registry {
    /// Build every service and adopt the tasks it needs into `tasks`. Makes no call: the first query is the first report.
    // One construction for each service, in the order of the module's notes; splitting it would hide which depends on which.
    #[allow(clippy::too_many_lines)]
    pub fn build(inputs: RegistryInputs<'_>, tasks: &mut Tasks) -> Result<Self, RegistryBuildError> {
        let RegistryInputs {
            settings,
            pool,
            blobs,
            git,
            gateway,
            bus,
            audit,
            notifier,
            clock,
            closing,
        } = inputs;
        let config: RegistryConfig = settings.config.clone();
        let clock: Arc<dyn hub_registry::Clock> = Arc::new(RegistryClock(clock));
        let metrics = RegistryMetrics::default();

        // 1. Ingestion: what an agent's and a sentinel's reports are applied by.
        let ingest = AgentIngest::new(IngestDeps {
            pool: pool.clone(),
            blobs: Arc::clone(&blobs),
            bus: Arc::clone(&bus),
            audit: Arc::clone(&audit),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })
        .map_err(config_error)?;
        let sentinels = SentinelIngest::new(AttributionDeps {
            pool: pool.clone(),
            bus: Arc::clone(&bus),
            audit: Arc::clone(&audit),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })
        .map_err(config_error)?;

        // 2. The settings index: the queue ingestion offers to, its worker, and the sweeper that catches what the queue lost.
        let (indexer, worker) = SettingsIndexer::new(IndexDeps {
            pool: pool.clone(),
            blobs: Arc::clone(&blobs),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })
        .map_err(config_error)?;
        ingest
            .attach_settings_index(indexer.clone())
            .map_err(|_| RegistryBuildError::Index)?;
        tasks.adopt(
            "registry-index-worker",
            spawn_index_worker(worker, closing.clone()),
        );

        // 3. The pipeline and the alerts it offers drift to.
        let alerts = Alerts::new(AlertsDeps {
            pool: pool.clone(),
            notifier: Arc::clone(&notifier),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })
        .map_err(config_error)?;
        let (pipeline, handle) = Pipeline::new(PipelineDeps {
            pool: pool.clone(),
            blobs: Arc::clone(&blobs),
            git: Arc::clone(&git),
            bus: Arc::clone(&bus),
            audit: Arc::clone(&audit),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
            alerts: Some(alerts),
        })
        .map_err(config_error)?;

        // 4. The read API, the write path's two ports, and the blob policy.
        let service = RegistryService::new(ReadDeps {
            pool: pool.clone(),
            blobs: Arc::clone(&blobs),
            git: Arc::clone(&git),
            gateway: Arc::clone(&gateway),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })
        .map_err(config_error)?;
        let tool_writes = PgToolWrites::new(ToolWriteDeps {
            pool: pool.clone(),
            bus: Arc::clone(&bus),
            audit: Arc::clone(&audit),
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
        })
        .map_err(config_error)?;
        let blob_policy =
            Arc::new(RegistryBlobPolicy::new(pool.clone(), &config, metrics.clone()).map_err(config_error)?);

        // 5. The other jobs.
        let sync = SyncService::new(SyncDeps {
            pool: pool.clone(),
            git: Arc::clone(&git),
            bus: Arc::clone(&bus),
            audit: Arc::clone(&audit),
            gateway: Arc::clone(&gateway),
            notifier,
            clock: Arc::clone(&clock),
            config: config.clone(),
            metrics: metrics.clone(),
            pipeline: Some(handle),
        })
        .map_err(config_error)?;
        let severity =
            SeverityRuleSource::new(pool.clone(), &config, metrics.clone()).map_err(config_error)?;
        let served = ServedVerifier::new(ServedDeps {
            service: service.clone(),
            bus: Arc::clone(&bus),
            severity,
        });

        let mut jobs: Vec<JobEntry> = Vec::new();
        for (spec, job) in pipeline
            .jobs()
            .map_err(|_| RegistryBuildError::Job("registry-pipeline".to_owned()))?
        {
            jobs.push((spec, job));
        }
        add(
            &mut jobs,
            "registry-index-sweeper",
            SettingsSweeper::spec(&config),
            Arc::new(SettingsSweeper::new(indexer)),
        )?;
        add(
            &mut jobs,
            "registry-tool-write-matcher",
            ToolWriteMatcher::spec(&config),
            tool_writes.matcher().job(),
        )?;
        add(
            &mut jobs,
            "registry-served-verify",
            ServedVerifier::spec(&config),
            served.job(),
        )?;
        add(
            &mut jobs,
            GRACE_JOB,
            hub_registry::AttributionGrace::spec(&config),
            sentinels.grace().job(),
        )?;
        add(
            &mut jobs,
            PARTITIONS_JOB,
            hub_registry::SentinelPartitions::spec(&config),
            sentinels.partitions().job(),
        )?;
        add(
            &mut jobs,
            WATCH_JOB,
            SyncService::watch_spec(&config),
            sync.watch_job(),
        )?;
        if settings.retention_enabled {
            let retention = Retention::new(RetentionDeps {
                pool,
                ingest: ingest.clone(),
                clock,
                config: config.clone(),
                metrics: metrics.clone(),
            })
            .map_err(config_error)?;
            add(
                &mut jobs,
                "registry-retention",
                Retention::spec(&config),
                retention.job(),
            )?;
            info!(
                dry_run = config.retention_dry_run,
                "the registry's retention job is registered (LK_RETENTION_ENABLED)"
            );
        }
        Ok(Self {
            ingest,
            sentinels,
            service,
            tool_writes,
            blob_policy,
            metrics,
            jobs,
            closing,
        })
    }

    /// The names of the jobs, in registration order.
    #[must_use]
    pub fn job_names(&self) -> Vec<String> {
        self.jobs.iter().map(|(spec, _)| spec.name().to_owned()).collect()
    }

    /// Register every job with `runner`. The runner starts later, when everything that registers a job has.
    pub fn register_jobs(&mut self, runner: &mut JobRunner) -> Result<(), RegistryBuildError> {
        for (spec, job) in std::mem::take(&mut self.jobs) {
            let name = spec.name().to_owned();
            runner
                .register(spec, job)
                .map_err(|_| RegistryBuildError::Job(name))?;
        }
        Ok(())
    }

    /// The read API's 15 routes, all `GET` at Viewer.
    #[must_use]
    pub fn module(&self) -> Arc<dyn Module> {
        Arc::new(RegistryModule {
            api: Arc::new(self.service.clone()),
        })
    }

    /// Start the warm-up of the text corpora on this replica and stop it with the hub. Call after the listeners are up.
    pub fn start_warmup(&self, tasks: &mut Tasks) {
        let warmup = self.service.spawn_warmup();
        let closing = self.closing.clone();
        tasks.adopt(
            "registry-warmup",
            tokio::spawn(async move {
                closed(closing).await;
                warmup.shutdown().await;
            }),
        );
    }
}

/// The settings index's worker, until the hub stops. (`SettingsWorker::run` ends when every indexer handle is dropped, which is after
/// the tasks have been waited for.)
fn spawn_index_worker(worker: SettingsWorker, closing: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::select! {
            () = worker.run() => {}
            () = closed(closing) => {}
        }
    })
}

/// Mounts the registry's read routes.
pub struct RegistryModule {
    api: Arc<dyn RegistryApi>,
}

impl std::fmt::Debug for RegistryModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryModule").finish_non_exhaustive()
    }
}

impl Module for RegistryModule {
    fn name(&self) -> &'static str {
        "registry"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        hub_registry::registry_routes(router, Arc::clone(&self.api))
    }
}
