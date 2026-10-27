//! The platform's four jobs, registered with the runner (plan 18, T3; D62, D63).
//!
//! Each runs on one replica at a time, under a lease of its own name, with the interval and lease time the platform gives it.
//! The registry's and the write paths' jobs are registered by their tasks (T5); the runner starts after all of them are in.

use std::sync::Arc;

use hub_platform::events::metrics::EventMetrics;
use hub_platform::git::deliveries::{self, DeliveryPrune, PgDeliveryLog};
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::poll::{self, GitPoll};
use hub_platform::git::reader::GixReader;
use hub_platform::git::sync::MirrorSyncer;
use hub_platform::git::tree_index::{self, TreeIndexPersist};
use hub_platform::jobs::outbox_prune::{self, OutboxPrune};
use hub_platform::jobs::{Job, JobRunner, JobSpec, SpecError};
use sqlx::PgPool;

use super::PlatformError;

/// What the four jobs are made of.
#[derive(Debug)]
pub struct JobParts {
    pub pool: PgPool,
    pub events: EventMetrics,
    pub git: GitMetrics,
    pub syncer: Arc<MirrorSyncer>,
    pub reader: Arc<GixReader>,
    pub deliveries: PgDeliveryLog,
}

/// The names the four jobs hold their leases under, in registration order.
pub const PLATFORM_JOBS: [&str; 4] = [
    outbox_prune::PRUNE_JOB,
    deliveries::PRUNE_JOB,
    poll::POLL_JOB,
    tree_index::PERSIST_JOB,
];

fn add(
    runner: &mut JobRunner,
    name: &'static str,
    spec: Result<JobSpec, SpecError>,
    job: Arc<dyn Job>,
) -> Result<(), PlatformError> {
    let spec = spec.map_err(|_| PlatformError::Job(name))?;
    runner.register(spec, job).map_err(|_| PlatformError::Job(name))
}

/// Register the four jobs.
pub fn register(runner: &mut JobRunner, parts: JobParts) -> Result<(), PlatformError> {
    let JobParts {
        pool,
        events,
        git,
        syncer,
        reader,
        deliveries,
    } = parts;
    add(
        runner,
        outbox_prune::PRUNE_JOB,
        OutboxPrune::spec(),
        Arc::new(OutboxPrune::new(pool.clone(), events)),
    )?;
    add(
        runner,
        deliveries::PRUNE_JOB,
        DeliveryPrune::spec(),
        Arc::new(DeliveryPrune::new(deliveries)),
    )?;
    add(
        runner,
        poll::POLL_JOB,
        GitPoll::spec(),
        Arc::new(GitPoll::new(syncer)),
    )?;
    add(
        runner,
        tree_index::PERSIST_JOB,
        TreeIndexPersist::spec(),
        Arc::new(TreeIndexPersist::new(reader, pool, git)),
    )
}
