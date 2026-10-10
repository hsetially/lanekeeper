//! Debouncing of cluster reports (T6): changes that arrive within one window leave as one report.
//!
//! The first change after a quiet spell opens a window; everything marked until it closes joins the same batch. So a
//! rollout that touches twenty pods in a second is one report, and no report leaves more than one second after the
//! change that caused it (plus the time to build it).

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;

/// A set of changed keys and the clock that decides when to release them.
#[derive(Debug)]
pub struct ReportDebouncer<K: Ord> {
    window: Duration,
    pending: Mutex<BTreeSet<K>>,
    wake: Notify,
}

impl<K: Ord> ReportDebouncer<K> {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: Mutex::new(BTreeSet::new()),
            wake: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeSet<K>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Note that these keys changed.
    pub fn mark(&self, keys: impl IntoIterator<Item = K>) {
        let mut pending = self.lock();
        let before = pending.len();
        pending.extend(keys);
        let changed = pending.len() != before;
        drop(pending);
        if changed {
            self.wake.notify_one();
        }
    }

    /// Forget everything marked so far: a full report has just covered it.
    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Wait for a change, then for the window to pass, then take every key marked meanwhile.
    pub async fn next_batch(&self) -> BTreeSet<K> {
        loop {
            if !self.is_empty() {
                break;
            }
            // `notify_one` keeps a permit when nobody waits, so a mark between the check and this line is not lost.
            self.wake.notified().await;
        }
        tokio::time::sleep(self.window).await;
        std::mem::take(&mut *self.lock())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::time::{advance, timeout};

    use super::*;

    const WINDOW: Duration = Duration::from_secs(1);

    fn debouncer() -> Arc<ReportDebouncer<u32>> {
        Arc::new(ReportDebouncer::new(WINDOW))
    }

    /// Run `next_batch` in the background and let it start waiting.
    fn start(d: &Arc<ReportDebouncer<u32>>) -> tokio::task::JoinHandle<BTreeSet<u32>> {
        let d = Arc::clone(d);
        tokio::spawn(async move { d.next_batch().await })
    }

    #[tokio::test(start_paused = true)]
    async fn marks_inside_one_window_leave_as_one_batch_after_a_second() {
        let d = debouncer();
        let batch = start(&d);
        d.mark([1]);
        advance(Duration::from_millis(400)).await;
        d.mark([2, 1]);
        advance(Duration::from_millis(400)).await;
        d.mark([3]);
        assert!(!batch.is_finished(), "nothing leaves before the window closes");
        advance(Duration::from_millis(199)).await;
        assert!(
            !batch.is_finished(),
            "999 ms after the first mark is still inside the window"
        );
        advance(Duration::from_millis(2)).await;
        assert_eq!(batch.await.unwrap(), BTreeSet::from([1, 2, 3]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_mark_after_the_batch_opens_the_next_window() {
        let d = debouncer();
        let first = start(&d);
        d.mark([1]);
        advance(WINDOW).await;
        assert_eq!(first.await.unwrap(), BTreeSet::from([1]));
        let second = start(&d);
        advance(Duration::from_secs(30)).await;
        assert!(!second.is_finished(), "no mark, no batch, however long it waits");
        d.mark([2]);
        advance(Duration::from_millis(999)).await;
        assert!(!second.is_finished());
        advance(Duration::from_millis(1)).await;
        assert_eq!(second.await.unwrap(), BTreeSet::from([2]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_mark_made_before_anyone_waits_is_not_lost() {
        let d = debouncer();
        d.mark([7]);
        let batch = start(&d);
        advance(WINDOW).await;
        assert_eq!(batch.await.unwrap(), BTreeSet::from([7]));
    }

    #[tokio::test(start_paused = true)]
    async fn clearing_drops_what_a_full_report_covered() {
        let d = debouncer();
        d.mark([1, 2]);
        d.clear();
        assert!(d.is_empty());
        let batch = start(&d);
        advance(Duration::from_secs(5)).await;
        assert!(!batch.is_finished());
        batch.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn marking_nothing_wakes_nobody() {
        let d = debouncer();
        let batch = start(&d);
        d.mark(std::iter::empty());
        assert!(timeout(Duration::from_secs(10), batch).await.is_err());
    }
}
