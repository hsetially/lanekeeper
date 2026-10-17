//! The index sweeper (T2): the leased job `registry-index-sweeper`.
//!
//! The queue ([`super::SettingsIndexer::offer`]) is the fast path, and it can lose a hash: the queue was full, the replica died with
//! hashes waiting, or the blob store was down for a batch. The sweeper is what makes "every current structured blob is indexed" true
//! whatever happened. For each swimlane it reads the current structured blobs of `file_state` that have no marker in
//! `settings_index_blobs` (one query per swimlane, in hash order, served by the covering index on `(swimlane_id, nfs_hash)`), and
//! indexes them in batches. Blobs that are not the current version of any file are not its business: settings search joins the
//! index to the current hash, and a history version is read from its bytes.
//!
//! A run is idempotent and bounded: at most `index_sweep_max_blobs` blobs, so it ends well inside its `max_run`, and the next run goes
//! on. It asks [`JobContext::ensure_held`] before each unit of work that writes and stops when the lease is lost.

use std::time::Duration;

use async_trait::async_trait;
use domain::NfsPath;
use engine::Syntax;
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use tracing::{debug, warn};

use super::settings::{IndexJob, SettingsIndexer};
use crate::config::RegistryConfig;

/// The job's name, which is also its lease's name.
pub const JOB_NAME: &str = "registry-index-sweeper";

/// The most swimlanes one run walks.
const MAX_SWIMLANES: i64 = 10_000;

/// What one sweep did, in counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Blobs found without a marker.
    pub found: u64,
    pub indexed: u64,
    pub unparseable: u64,
    pub missing: u64,
    pub failed: u64,
    /// The run stopped at `index_sweep_max_blobs` and the next one will go on.
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SettingsSweeper {
    indexer: SettingsIndexer,
}

impl SettingsSweeper {
    pub fn new(indexer: SettingsIndexer) -> Self {
        Self { indexer }
    }

    /// How the job is scheduled: every `index_sweep_interval`, under a 90 second lease that the runner renews every 30.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes (the interval is range-checked by `RegistryConfig::validate`).
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(JOB_NAME, config.index_sweep_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(15 * 60))
    }

    /// One sweep. `held` is asked before each unit of work and ends the sweep with its error.
    pub async fn sweep(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<SweepReport, JobError> {
        let inner = self.indexer.inner();
        let mut report = SweepReport::default();
        held()?;
        let swimlanes: Vec<String> = inner
            .db
            .run(
                "sweep_swimlanes",
                sqlx::query_scalar("SELECT id FROM swimlanes ORDER BY id LIMIT $1")
                    .bind(MAX_SWIMLANES)
                    .fetch_all(inner.db.pool()),
            )
            .await
            .map_err(|_| JobError::Failed("registry_index_sweep"))?;
        let batch = i64::try_from(inner.batch_size()).unwrap_or(i64::MAX);
        let budget = inner.sweep_max_blobs() as u64;
        'swimlanes: for swimlane in swimlanes {
            // The empty string sorts before every hash.
            let mut after: Vec<u8> = Vec::new();
            loop {
                held()?;
                let found = inner
                    .unindexed(&swimlane, &after, batch)
                    .await
                    .map_err(|_| JobError::Failed("registry_index_sweep"))?;
                let Some((last, _)) = found.last() else { break };
                after = last.as_bytes().to_vec();
                // A page that is not full is the end of the swimlane.
                let full = i64::try_from(found.len()).is_ok_and(|n| n >= batch);
                let jobs: Vec<IndexJob> = found
                    .iter()
                    .filter_map(|(hash, path)| {
                        let syntax = NfsPath::parse(path).ok().and_then(|p| Syntax::of_path(&p))?;
                        Some(IndexJob { hash: *hash, syntax })
                    })
                    .collect();
                report.found += found.len() as u64;
                match self.indexer.index_now(jobs).await {
                    Ok(done) => {
                        report.indexed += done.indexed;
                        report.unparseable += done.unparseable;
                        report.missing += done.missing;
                        report.failed += done.failed;
                    }
                    Err(error) => {
                        // The database or the blob store is not answering: end the run, the next one tries again.
                        warn!(%error, "settings index sweep stopped");
                        return Err(JobError::Failed("registry_index_sweep"));
                    }
                }
                if report.found >= budget && full {
                    report.truncated = true;
                    break 'swimlanes;
                }
                if !full {
                    break;
                }
            }
        }
        debug!(
            found = report.found,
            indexed = report.indexed,
            unparseable = report.unparseable,
            missing = report.missing,
            failed = report.failed,
            truncated = report.truncated,
            "settings index sweep"
        );
        Ok(report)
    }
}

#[async_trait]
impl Job for SettingsSweeper {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec =
            SettingsSweeper::spec(&RegistryConfig::default()).expect("the default config gives a valid spec");
        assert_eq!(spec.name(), JOB_NAME);
        assert_eq!(JOB_NAME, "registry-index-sweeper");
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = RegistryConfig {
            index_sweep_interval: Duration::ZERO,
            ..RegistryConfig::default()
        };
        assert!(SettingsSweeper::spec(&config).is_err());
    }
}
