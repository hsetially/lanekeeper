//! The warm-up of the text corpora (T6, P7): a task on each replica that keeps them current, so that a search is normally a warm one.
//!
//! Every replica serves reads, so every replica has its own corpora and its own warm-up (this is not a leased job). The task builds
//! the snapshot of every swimlane after start-up, then looks at the swimlanes' versions every `warmup_poll` (one cheap statement) and,
//! when one has moved, waits `warmup_debounce` (a burst of changes is taken in once) and brings the snapshot up to date: the moved
//! swimlanes' lists are read and only the shards whose places changed are made again (see [`super::corpora`]).
//!
//! # What it may and may not do
//!
//! - **It never makes a read wait for more than a read would wait for itself.** It builds through the same path a search does,
//!   [`Inner::corpus_snapshot`]: one build at a time on the replica, under one lock. A search that needs what the warm-up is building
//!   takes the result when it is done (and would have built the same); a search that needs more waits for that build, then builds its
//!   own, which is the shards the warm-up did not cover and so little. A search never waits for the next poll: it checks the versions
//!   itself.
//! - **A search never gets data older than the swimlane's current version** whether or not the warm-up has run: the snapshot is only
//!   used when it covers the versions the search read, and is made again from the database otherwise.
//! - **It is bounded.** At most one build at a time, each at most once per `warmup_poll + warmup_debounce`; the build's own work is
//!   bounded (the lists of the moved swimlanes, the changed shards, the blobs they lack, each call with a timeout), and the memory is
//!   the snapshot's, inside `corpus_cache_bytes`. A failed build is logged by kind (never a name or a content) and tried again at the
//!   next poll; until then searches build for themselves.
//! - **A viewer cannot make it build**: nothing a viewer sends reaches it. It builds when the data changed, which is the writers'
//!   doing, and at most as often as above.
//! - **It stops on request and when dropped**, even in the middle of a build (every await point of a build can be abandoned: the
//!   snapshot is replaced only when a build is whole).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use ports::ReadError;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::service::{Inner, RegistryService};

impl Inner {
    /// The (swimlane, version) of every swimlane, and the names of them.
    async fn all_versions(&self) -> Result<(Vec<(String, i64)>, BTreeSet<String>), ReadError> {
        let scope = self.search_scope(&[]).await?;
        let names = scope.iter().map(|(lane, _)| lane.clone()).collect();
        Ok((scope, names))
    }

    /// Whether the snapshot covers every swimlane at its current version.
    pub async fn corpora_are_current(&self) -> Result<bool, ReadError> {
        let (scope, _) = self.all_versions().await?;
        Ok(scope.is_empty()
            || self
                .caches
                .corpora
                .current()
                .is_some_and(|snapshot| snapshot.covers(&scope)))
    }

    /// Bring the snapshot up to date for every swimlane.
    pub async fn warm_corpora(&self) -> Result<(), ReadError> {
        let (scope, names) = self.all_versions().await?;
        if scope.is_empty() {
            return Ok(());
        }
        self.corpus_snapshot(&scope, Some(&names)).await.map(|_| ())
    }
}

/// A running warm-up. Dropping it stops the task; [`shutdown`](Self::shutdown) stops it and waits.
#[derive(Debug)]
pub struct WarmupTask {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl WarmupTask {
    /// Stop the task, in the middle of a build if need be, and wait until it has.
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    /// Whether the task has ended (it only does when asked to).
    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for WarmupTask {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Wait `d`, or until the task is asked to stop; `false` for stop.
async fn pause(d: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(d) => true,
        _ = stop.changed() => false,
    }
}

async fn run(inner: Arc<Inner>, mut stop: watch::Receiver<bool>) {
    let (poll, debounce) = (inner.config.warmup_poll, inner.config.warmup_debounce);
    loop {
        // Build, or abandon the build at a stop.
        let built = tokio::select! {
            built = inner.warm_corpora() => built,
            _ = stop.changed() => return,
        };
        match built {
            Ok(()) => debug!(op = "warmup", "the text corpora are current"),
            Err(e) => {
                warn!(op = "warmup", error = ?e, "the text corpora could not be brought up to date; trying again");
            }
        }
        // Wait for a change (or for the build to be possible again).
        loop {
            if !pause(poll, &mut stop).await {
                return;
            }
            let stale = tokio::select! {
                stale = inner.corpora_are_current() => stale.map_or(true, |current| !current),
                _ = stop.changed() => return,
            };
            if stale {
                break;
            }
        }
        // Let a burst of changes finish: the next build takes them all.
        if !pause(debounce, &mut stop).await {
            return;
        }
    }
}

impl RegistryService {
    /// Start the warm-up of the text corpora on this replica. The composition calls it once; the task ends when it is dropped or shut
    /// down. It needs a Tokio runtime.
    pub fn spawn_warmup(&self) -> WarmupTask {
        let (stop, signal) = watch::channel(false);
        let task = tokio::spawn(run(Arc::clone(&self.inner), signal));
        WarmupTask {
            stop,
            task: Some(task),
        }
    }

    /// The bytes the text corpora hold now (the blobs, their places and the file lists they were made from), or `None` when there is no
    /// snapshot. What `corpus_cache_bytes` bounds; for tests, benchmarks and the memory budget (P11).
    pub fn text_corpora_weight(&self) -> Option<u64> {
        self.inner.caches.corpora.current().map(|s| s.weight)
    }

    /// Whether the text corpora cover every swimlane at its current version: what the warm-up is for. For tests and benchmarks.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the database cannot be read.
    pub async fn text_corpora_are_current(&self) -> Result<bool, ReadError> {
        self.inner.corpora_are_current().await
    }
}
