//! The queue between the webhook and the fetch worker (D64, rule 5).
//!
//! The webhook handler must answer fast, so it only records what was pushed. The worker takes everything pending, fetches each
//! repository once (a fetch brings in every branch and tag, so one fetch serves any number of pushes to that repository), and
//! announces what moved. The queue is bounded in distinct pending targets: a flood of pushes to many branches fills it, the rest
//! are dropped and counted, and the 60 s poll catches what was dropped (A19).

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::RepoKind;
use tokio::sync::Notify;

use super::payload::PushedRef;

/// What `push` did with a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueue {
    Queued,
    /// The same target was already pending: nothing more to do.
    Coalesced,
    /// The queue is full. The poll will notice the change.
    Dropped,
}

/// One repository to fetch, and the refs the webhooks named (for logs: the fetch brings in everything).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchJob {
    pub repo: RepoKind,
    pub targets: Vec<PushedRef>,
}

#[derive(Default)]
struct Pending {
    by_repo: BTreeMap<RepoKind, HashSet<PushedRef>>,
    count: usize,
}

/// Bounded, coalescing, and waited on by one worker.
pub struct FetchQueue {
    capacity: usize,
    pending: Mutex<Pending>,
    wake: Notify,
    dropped: AtomicU64,
    coalesced: AtomicU64,
}

impl std::fmt::Debug for FetchQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchQueue")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl FetchQueue {
    /// At most `capacity` distinct targets wait at once.
    #[must_use]
    pub fn new(capacity: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            capacity: capacity.max(1),
            pending: Mutex::new(Pending::default()),
            wake: Notify::new(),
            dropped: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        // No code holds this lock across a panic point; a poisoned lock still holds consistent data.
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn push(&self, repo: RepoKind, target: PushedRef) -> Enqueue {
        let outcome = {
            let mut pending = self.lock();
            let known = pending
                .by_repo
                .get(&repo)
                .is_some_and(|set| set.contains(&target));
            if known {
                Enqueue::Coalesced
            } else if pending.count >= self.capacity {
                Enqueue::Dropped
            } else {
                pending.by_repo.entry(repo).or_default().insert(target);
                pending.count += 1;
                Enqueue::Queued
            }
        };
        match outcome {
            Enqueue::Queued => self.wake.notify_one(),
            Enqueue::Coalesced => {
                self.coalesced.fetch_add(1, Ordering::Relaxed);
            }
            Enqueue::Dropped => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        outcome
    }

    /// Everything pending, one job per repository. Waits while nothing is.
    pub async fn next_batch(&self) -> Vec<FetchJob> {
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            // Register for a wake-up before looking, so a push between the look and the wait is not lost.
            notified.as_mut().enable();
            if let Some(batch) = self.take() {
                return batch;
            }
            notified.await;
        }
    }

    fn take(&self) -> Option<Vec<FetchJob>> {
        let mut pending = self.lock();
        if pending.count == 0 {
            return None;
        }
        pending.count = 0;
        let by_repo = std::mem::take(&mut pending.by_repo);
        Some(
            by_repo
                .into_iter()
                .map(|(repo, targets)| FetchJob {
                    repo,
                    targets: targets.into_iter().collect(),
                })
                .collect(),
        )
    }

    /// Distinct targets waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Targets dropped because the queue was full, since the start.
    #[must_use]
    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Pushes that were already pending, since the start.
    #[must_use]
    pub fn coalesced_total(&self) -> u64 {
        self.coalesced.load(Ordering::Relaxed)
    }
}
