//! The poll that backs the webhook up (D64, P2): the holder of the `git-poll` lease fetches both repositories every 60 seconds and
//! announces what moved. A webhook that never arrived, one that was dropped because the queue was full, and one whose fetch kept
//! failing are all caught here, so a push reaches `GitHeadMoved` within the interval plus the runner's jitter (10 %) and the fetch.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::RepoKind;

use crate::jobs::{Job, JobContext, JobError, JobSpec};

use super::sync::RepoSync;

/// The lease and the job's name.
pub const POLL_JOB: &str = "git-poll";
/// How often the leaseholder polls.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// The lease survives two missed renewals, so a dead holder is replaced within this.
pub const POLL_LEASE_TTL: Duration = Duration::from_secs(150);
/// A poll that takes longer than this is cut off; the next one starts afresh.
pub const POLL_MAX_RUN: Duration = Duration::from_secs(5 * 60);

/// The worst delay of the poll path, from a push to `GitHeadMoved`, with a fetch that takes `fetch`: the interval, the runner's
/// 10 % jitter on how soon the holder asks again, and the fetch itself.
#[must_use]
pub fn worst_case_latency(interval: Duration, fetch: Duration) -> Duration {
    interval + interval / 10 + fetch
}

/// Fetches both repositories and announces what moved.
pub struct GitPoll {
    syncer: Arc<dyn RepoSync>,
}

impl std::fmt::Debug for GitPoll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitPoll").finish_non_exhaustive()
    }
}

impl GitPoll {
    #[must_use]
    pub fn new(syncer: Arc<dyn RepoSync>) -> Self {
        Self { syncer }
    }

    /// The spec to register with the `JobRunner`.
    pub fn spec() -> Result<JobSpec, crate::jobs::SpecError> {
        Self::spec_with(POLL_INTERVAL, POLL_LEASE_TTL)
    }

    /// The same with another interval, for tests that run on a real clock.
    pub fn spec_with(interval: Duration, lease_ttl: Duration) -> Result<JobSpec, crate::jobs::SpecError> {
        JobSpec::new(POLL_JOB, interval, lease_ttl)?.with_max_run(POLL_MAX_RUN)
    }
}

#[async_trait]
impl Job for GitPoll {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let mut failed = false;
        for repo in [RepoKind::Base, RepoKind::Tenant] {
            ctx.ensure_held()?;
            if self.syncer.sync(repo).await.is_err() {
                // The other repository is still worth polling.
                failed = true;
            }
        }
        if failed {
            Err(JobError::Failed("git_poll_sync"))
        } else {
            Ok(())
        }
    }
}
