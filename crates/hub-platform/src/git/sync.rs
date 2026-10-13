//! Keeping the mirrors current and telling the other replicas (D64, P2).
//!
//! ```text
//!   webhook --enqueue--> FetchQueue --> FetchWorker --fetch--> Mirror --announce--> bus: GitHeadMoved
//!   poll (leaseholder, every 60 s) ------fetch-------^                                    |
//!                                                                          every replica: MirrorSync
//!                                                                          fetches when it lacks `to`
//! ```
//!
//! **Who announces.** The replica that fetched because of a webhook, and the poll's leaseholder, publish `GitHeadMoved` for every
//! branch whose head differs from what was last announced. The [`Announcer`] compares with its own memory of the last announced
//! heads, not with who did the fetching, so a fetch made by a read-through for a missing commit cannot swallow an announcement. A
//! replica that fetches *because of* an announcement never announces (it would only repeat what it just heard); it records what it
//! heard, so that if it becomes the poll's leaseholder it does not announce old moves again.
//!
//! Delivery is at least once, and every consumer is idempotent (tree diffs between two commits).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{CommitId, DomainEvent, GitRef, RepoKind};
use futures::StreamExt;
use ports::{EventBus, EventFilter, EventKind};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::fetch_queue::FetchQueue;
use super::metrics::GitMetrics;
use super::mirror::{Mirror, MirrorError, RefSnapshot};

const REPOS: [RepoKind; 2] = [RepoKind::Base, RepoKind::Tenant];

/// Why a sync did not complete. Closed, and without detail: the cause is in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error("the fetch failed")]
    Fetch(#[source] MirrorError),
    #[error("the mirror could not be read")]
    Read,
}

/// Fetch a repository and announce what moved. The webhook worker and the poll both do this; the seam lets the poll's schedule
/// be tested on a paused clock without a real fetch.
#[async_trait]
pub trait RepoSync: Send + Sync + 'static {
    async fn sync(&self, repo: RepoKind) -> Result<(), SyncError>;
}

type Heads = BTreeMap<GitRef, CommitId>;

/// Publishes `GitHeadMoved` for branches whose head moved since the last announcement.
pub struct Announcer {
    mirror: Arc<Mirror>,
    bus: Arc<dyn EventBus>,
    last: Mutex<HashMap<RepoKind, Heads>>,
    metrics: GitMetrics,
}

impl std::fmt::Debug for Announcer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Announcer").finish_non_exhaustive()
    }
}

async fn heads_of(mirror: &Arc<Mirror>, repo: RepoKind) -> Result<RefSnapshot, SyncError> {
    let mirror = Arc::clone(mirror);
    tokio::task::spawn_blocking(move || mirror.refs(repo))
        .await
        .map_err(|_| SyncError::Read)?
        .map_err(|_| SyncError::Read)
}

impl Announcer {
    /// Takes the current heads as the baseline, so that starting up announces nothing. Call after [`Mirror::initialize`].
    pub async fn start(mirror: Arc<Mirror>, bus: Arc<dyn EventBus>, metrics: GitMetrics) -> Arc<Self> {
        let mut last = HashMap::new();
        for repo in REPOS {
            // A mirror that cannot be read yet has no baseline; its first announcement then covers every branch.
            if let Ok(snapshot) = heads_of(&mirror, repo).await {
                last.insert(repo, snapshot.branches);
            }
        }
        Arc::new(Self {
            mirror,
            bus,
            last: Mutex::new(last),
            metrics,
        })
    }

    /// Publish `GitHeadMoved` for each branch of `repo` that is new or moved since the last announcement. Returns how many.
    /// A branch that was deleted is dropped from the memory and announced as nothing.
    pub async fn announce(&self, repo: RepoKind) -> Result<usize, SyncError> {
        let now = heads_of(&self.mirror, repo).await?.branches;
        let mut last = self.last.lock().await;
        let before = last.entry(repo).or_default();
        let mut announced = 0;
        let mut kept = Heads::new();
        for (branch, to) in &now {
            let from = before.get(branch);
            if from == Some(to) {
                kept.insert(branch.clone(), to.clone());
                continue;
            }
            let event = DomainEvent::GitHeadMoved {
                repo,
                branch: branch.clone(),
                from: from.cloned(),
                to: to.clone(),
            };
            if self.bus.publish(event).await.is_ok() {
                self.metrics.announced();
                announced += 1;
                kept.insert(branch.clone(), to.clone());
            } else {
                // Not announced: keep the old head so the next call tries again.
                warn!(
                    code = "git_announce_failed",
                    repo = repo.as_str(),
                    "GitHeadMoved could not be published"
                );
                if let Some(from) = from {
                    kept.insert(branch.clone(), from.clone());
                }
            }
        }
        *before = kept;
        Ok(announced)
    }

    /// Record a move another replica announced.
    pub async fn observe(&self, repo: RepoKind, branch: &GitRef, to: &CommitId) {
        self.last
            .lock()
            .await
            .entry(repo)
            .or_default()
            .insert(branch.clone(), to.clone());
    }
}

/// Fetch, then announce.
pub struct MirrorSyncer {
    mirror: Arc<Mirror>,
    announcer: Arc<Announcer>,
}

impl std::fmt::Debug for MirrorSyncer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MirrorSyncer").finish_non_exhaustive()
    }
}

impl MirrorSyncer {
    #[must_use]
    pub fn new(mirror: Arc<Mirror>, announcer: Arc<Announcer>) -> Self {
        Self { mirror, announcer }
    }
}

#[async_trait]
impl RepoSync for MirrorSyncer {
    async fn sync(&self, repo: RepoKind) -> Result<(), SyncError> {
        self.mirror.fetch(repo).await.map_err(SyncError::Fetch)?;
        self.announcer.announce(repo).await.map(|_| ())
    }
}

// ------------------------------------------------------------------------------------------------ webhook worker

/// How the worker retries a failed fetch (rule 5): a few attempts, exponential backoff, full jitter.
#[derive(Debug, Clone, Copy)]
pub struct RetrySettings {
    pub attempts: u32,
    pub base: Duration,
    pub cap: Duration,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(5),
        }
    }
}

impl RetrySettings {
    /// A random wait between zero and `min(cap, base * 2^attempt)`.
    fn wait(&self, attempt: u32) -> Duration {
        let ceiling = self.base.saturating_mul(1 << attempt.min(16)).min(self.cap);
        ceiling.mul_f64(fastrand::f64())
    }
}

/// Takes what the webhook queued and syncs each repository once.
pub struct FetchWorker {
    queue: Arc<FetchQueue>,
    syncer: Arc<dyn RepoSync>,
    retry: RetrySettings,
}

impl std::fmt::Debug for FetchWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchWorker").finish_non_exhaustive()
    }
}

impl FetchWorker {
    #[must_use]
    pub fn new(queue: Arc<FetchQueue>, syncer: Arc<dyn RepoSync>, retry: RetrySettings) -> Self {
        Self { queue, syncer, retry }
    }

    /// Runs until `closing` turns true.
    #[must_use]
    pub fn spawn(self, mut closing: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let batch = tokio::select! {
                    batch = self.queue.next_batch() => batch,
                    () = closed(&mut closing) => return,
                };
                for job in batch {
                    self.sync_with_retry(job.repo).await;
                }
            }
        })
    }

    async fn sync_with_retry(&self, repo: RepoKind) {
        for attempt in 0..self.retry.attempts.max(1) {
            match self.syncer.sync(repo).await {
                Ok(()) => return,
                Err(_) if attempt + 1 < self.retry.attempts => {
                    tokio::time::sleep(self.retry.wait(attempt)).await;
                }
                Err(_) => {
                    // Dropped: the poll will fetch it within its interval.
                    warn!(
                        code = "git_sync_gave_up",
                        repo = repo.as_str(),
                        "the fetch after a webhook failed; the poll will retry"
                    );
                }
            }
        }
    }
}

pub(crate) async fn closed(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

// --------------------------------------------------------------------------------------------------- start-up

/// How a replica with an empty mirror keeps trying to fill it at start-up: exponential backoff with full jitter, never giving up
/// (a replica that cannot reach GitHub is not ready, and that is the truth), at most `cap` between tries.
#[derive(Debug, Clone, Copy)]
pub struct StartupRetry {
    pub base: Duration,
    pub cap: Duration,
}

impl Default for StartupRetry {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(60),
        }
    }
}

/// Fills any empty mirror ([`Mirror::initialize`]) and tries again, with backoff, until it works or `closing` turns true. Without
/// this, a replica that started while GitHub was unreachable would stay unready until the next push: only the poll's leaseholder
/// fetches on a timer.
#[must_use]
pub fn spawn_initialize(
    mirror: Arc<Mirror>,
    retry: StartupRetry,
    mut closing: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempt: u32 = 0;
        loop {
            match mirror.initialize().await {
                Ok(()) => {
                    info!(code = "git_mirrors_ready", "both mirrors are filled");
                    return;
                }
                Err(e) => {
                    warn!(code = "git_initialize_failed", error = %e, attempt, "the mirrors could not be filled; trying again");
                }
            }
            let ceiling = retry.base.saturating_mul(1 << attempt.min(16)).min(retry.cap);
            attempt = attempt.saturating_add(1);
            tokio::select! {
                () = tokio::time::sleep(ceiling.mul_f64(fastrand::f64())) => {}
                () = closed(&mut closing) => return,
            }
        }
    })
}

// ------------------------------------------------------------------------------------------- other replicas

/// Fetches when another replica announced a head this one does not have.
pub struct MirrorSync {
    mirror: Arc<Mirror>,
    announcer: Arc<Announcer>,
    bus: Arc<dyn EventBus>,
}

impl std::fmt::Debug for MirrorSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MirrorSync").finish_non_exhaustive()
    }
}

impl MirrorSync {
    #[must_use]
    pub fn new(mirror: Arc<Mirror>, announcer: Arc<Announcer>, bus: Arc<dyn EventBus>) -> Self {
        Self {
            mirror,
            announcer,
            bus,
        }
    }

    /// Subscribes to `GitHeadMoved` and runs until `closing` turns true. A `Resync` means events were missed, so every repository
    /// is fetched.
    #[must_use]
    pub fn spawn(self, mut closing: watch::Receiver<bool>) -> JoinHandle<()> {
        let filter = EventFilter {
            swimlanes: None,
            kinds: [EventKind::GitHeadMoved].into(),
        };
        let mut events = self.bus.subscribe(filter);
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    event = events.next() => event,
                    () = closed(&mut closing) => return,
                };
                match event {
                    Some(DomainEvent::GitHeadMoved { repo, branch, to, .. }) => {
                        self.announcer.observe(repo, &branch, &to).await;
                        self.catch_up(repo, &to).await;
                    }
                    Some(DomainEvent::Resync) => {
                        for repo in REPOS {
                            if let Err(e) = self.mirror.fetch(repo).await {
                                warn!(code = "git_resync_fetch_failed", repo = repo.as_str(), error = %e, "fetch after a resync failed");
                            }
                        }
                    }
                    Some(_) => {}
                    None => return,
                }
            }
        })
    }

    async fn catch_up(&self, repo: RepoKind, to: &CommitId) {
        let mirror = Arc::clone(&self.mirror);
        let target = to.clone();
        let have = tokio::task::spawn_blocking(move || mirror.has_commit(repo, &target))
            .await
            .unwrap_or(false);
        if have {
            debug!(
                code = "git_head_already_here",
                repo = repo.as_str(),
                "the announced commit is already in the mirror"
            );
            return;
        }
        match self.mirror.fetch(repo).await {
            Ok(_) => info!(
                code = "git_fetched_on_announcement",
                repo = repo.as_str(),
                "fetched after GitHeadMoved"
            ),
            Err(e) => {
                warn!(code = "git_announcement_fetch_failed", repo = repo.as_str(), error = %e, "fetch after GitHeadMoved failed");
            }
        }
    }
}
