//! Retention (T9, D78, D116): the leased nightly job `registry-retention`.
//!
//! One job, three tidy-ups, in this order, each bounded and each safe to repeat:
//!
//! 1. **Stale `delta_staging` rows** (parts of a delta that never completed, older than `staging_ttl`): ingestion's own sweep
//!    ([`crate::AgentIngest::sweep_staging_batch`]), called here in bounded batches.
//! 2. **Processed `sync_windows` rows** older than `sync_window_retention`, which is always longer than the hold budget reads back.
//! 3. **Blobs that nothing needs**: at most `gc_batch` of them and `gc_max_bytes` bytes a run (A34), deleted by SQL under the app role's
//!    own `DELETE` grant on `blobs` (the `BlobStore` port has no delete and could not do the reference check, which needs the registry's
//!    tables). What may go, and what each column that can hold a hash means to that question, is in [`plan`]. A blob that is deleted can
//!    linger in one replica's `PgBlobStore` cache until it is evicted: content is immutable, so that is harmless.
//!
//! A run with `retention_dry_run` set does the same looking and changes nothing: no row, no lock, no metric but its own counters. It says
//! what it would have done in the log and in `registry_retention_*`.
//!
//! **GC deletes are not audited** (A12, human-approved): a blob is a cache of a version that Git, the audit chain's own diff text and the
//! history keep a record of; the run's counts and bytes are in the log and the metrics. **Logs and metrics carry counts and bytes only**
//! (S10): never a hash, a path or a name.
//!
//! The job is leased (`registry-retention`, one holder), checks its lease before every statement that writes, and a run is idempotent: a
//! second run finds nothing new to do. It never sets a session setting: the transaction that deletes sets `lock_timeout` for itself only.

mod job;
pub mod plan;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use sqlx::PgPool;

use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::ingest::AgentIngest;
use crate::metrics::RegistryMetrics;

pub use job::Collected;
pub use plan::{Candidate, DECISIONS, Decision, KeepRules, Plan};

/// The job's name, which is also its lease's name.
pub const JOB_NAME: &str = "registry-retention";

/// What one run did, in counts and bytes (never which blobs, paths or names).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// The run only reported: nothing was changed.
    pub dry_run: bool,
    /// The keep rules the run applied (the `retention_settings` row, or Q33's defaults).
    pub keep_versions: i64,
    pub keep_days: i64,
    /// Blobs the plan holds, and their bytes.
    pub planned_blobs: usize,
    pub planned_bytes: u64,
    /// The plan was cut by the batch or the byte limit: more is collectable than this run took.
    pub truncated: bool,
    /// Blobs deleted, and their bytes (zero in a dry run).
    pub deleted_blobs: usize,
    pub deleted_bytes: u64,
    /// Blobs left for the next run because they changed between the plan and the delete: a put refreshed them, a writer held their
    /// row, or a reference appeared.
    pub skipped_blobs: usize,
    /// Blobs of a delete transaction that a reference or a lock stopped (the foreign key, `lock_timeout`).
    pub contended_blobs: usize,
    /// Processed `sync_windows` rows deleted (in a dry run: that would be).
    pub windows: usize,
    /// Stale `delta_staging` rows deleted (in a dry run: that would be).
    pub staging: usize,
}

/// The state every part of the job shares.
pub(crate) struct Inner {
    pub(crate) db: Db,
    pub(crate) ingest: AgentIngest,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) config: RegistryConfig,
    pub(crate) metrics: RegistryMetrics,
}

/// What the job needs from the rest of the hub.
pub struct RetentionDeps {
    /// The pool of the `lanekeeper_app` role: the job asks for no privilege that the app does not already have.
    pub pool: PgPool,
    /// Ingestion, whose staging sweep the job calls (it is not repeated here).
    pub ingest: AgentIngest,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for RetentionDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetentionDeps").finish_non_exhaustive()
    }
}

/// The retention job. Cloning shares everything.
#[derive(Clone)]
pub struct Retention {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Retention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Retention").finish_non_exhaustive()
    }
}

impl Retention {
    /// # Errors
    ///
    /// The configuration is not valid (the grace period below its floor, a retention of `sync_windows` that the hold budget would still
    /// read, a limit out of range), or `config.staging_ttl` is not the staging lifetime of the ingestion that is given (the grace
    /// period is checked against one and the sweep uses the other, so they must be the same number).
    pub fn new(deps: RetentionDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        if deps.ingest.staging_ttl() != deps.config.staging_ttl {
            return Err(ConfigError::Invalid("staging_ttl"));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                ingest: deps.ingest,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
            }),
        })
    }

    /// How the job is scheduled: every `retention_interval`, under a 90 second lease that the runner renews every 30, for at most half an
    /// hour (a run is the plan, a few hundred blobs in short transactions, and two bounded prunes).
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes.
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(JOB_NAME, config.retention_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(30 * 60))
    }

    /// The job (`registry-retention`), for the hub's job runner.
    pub fn job(&self) -> Arc<dyn Job> {
        Arc::new(Run {
            retention: self.clone(),
        })
    }
}

struct Run {
    retention: Retention,
}

#[async_trait]
impl Job for Run {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.retention.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec =
            Retention::spec(&RegistryConfig::default()).expect("the default config gives a valid spec");
        assert_eq!(spec.name(), JOB_NAME);
        assert_eq!(JOB_NAME, "registry-retention");
        assert!(ports::is_valid_lease_name(JOB_NAME));
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = RegistryConfig {
            retention_interval: Duration::ZERO,
            ..RegistryConfig::default()
        };
        assert!(Retention::spec(&config).is_err());
    }
}
