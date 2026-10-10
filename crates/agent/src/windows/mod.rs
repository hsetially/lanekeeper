//! Sync windows (T10, D72, D75): the spans of time in which a sync Job was running, as the agent saw them.
//!
//! The Kubernetes watchers say when a sync Job starts and stops ([`WindowTransition`]); this ledger turns that into
//! windows. Two things read it:
//!
//! - **the scanner**, which tags the changes it finds with the Job that was running when they happened
//!   ([`WindowLedger::overlapping`]), so that the hub holds its drift alerts until the Job is done;
//! - **the announcer** ([`announce`]), which tells the hub that a window opened and closed.
//!
//! # Order is by counter, not by clock
//!
//! Every transition takes the next number of the ledger's generation. A walk reads the generation before it looks at the
//! file system, so "this walk started after that window closed" is a comparison of two integers, with no clock skew and no
//! two events at the same instant in a test running in virtual time.
//!
//! # What is kept
//!
//! At most [`MAX_WINDOWS`] windows: the ones that are open, and the ones that closed in the last [`RETENTION`]. It is what
//! the first report after a reconnect repeats (A21), because window events have no acknowledgement and a hub outage can
//! lose them. Names and uids of Jobs only; nothing else about a Job is kept.

pub mod announce;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use domain::{JobRef, Timestamp};
use tokio::sync::watch;
use tracing::warn;

use crate::clock::Clock;

/// The most windows kept, open and closed together.
pub const MAX_WINDOWS: usize = 64;
/// How long a closed window is kept for a hub that missed its events.
pub const RETENTION: Duration = Duration::from_secs(60 * 60);

/// What the Job watcher saw happen to a sync Job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowTransition {
    /// The Job is running. `started` is the start time the API server gives, when it gives one.
    Running { job: JobRef, started: Option<Timestamp> },
    /// The Job completed, failed, was suspended or was deleted. `finished` is the completion time, when there is one.
    Finished {
        job: JobRef,
        finished: Option<Timestamp>,
    },
}

/// A window as the announcer reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowView {
    /// Names the window for as long as the ledger keeps it. A Job that is resumed after a suspension opens a new one.
    pub id: u64,
    pub job: JobRef,
    pub opened_at: Timestamp,
    /// `None` while the Job runs.
    pub closed_at: Option<Timestamp>,
    /// The ledger generation at which the close was seen. `None` while the Job runs.
    pub closed_gen: Option<u64>,
}

#[derive(Debug, Clone)]
struct Window {
    id: u64,
    job: JobRef,
    opened_at: Timestamp,
    opened_gen: u64,
    closed_at: Option<Timestamp>,
    closed_gen: Option<u64>,
    /// The agent's wall clock when the close was seen, for retention.
    closed_seen: Option<Timestamp>,
}

impl Window {
    fn is_open(&self) -> bool {
        self.closed_gen.is_none()
    }

    fn view(&self) -> WindowView {
        WindowView {
            id: self.id,
            job: self.job.clone(),
            opened_at: self.opened_at,
            closed_at: self.closed_at,
            closed_gen: self.closed_gen,
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    /// In the order they opened.
    windows: Vec<Window>,
    next_id: u64,
}

/// The windows of the sync Jobs. Shared by `Arc`; every method takes `&self`.
pub struct WindowLedger {
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
    /// The generation: raised under `inner`'s lock after each change, and what subscribers wait on.
    generation: watch::Sender<u64>,
}

impl std::fmt::Debug for WindowLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowLedger")
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl WindowLedger {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            inner: Mutex::new(Inner::default()),
            generation: watch::channel(0).0,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned lock means a thread panicked mid-update; the list is still whole, and an agent that stops tagging is
        // worse than one that carries on.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The number of changes so far. A reader that sees `g` also sees every change numbered `g` or less.
    pub fn generation(&self) -> u64 {
        *self.generation.borrow()
    }

    /// Wakes when the ledger changes.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// Record what the Job watcher saw. Saying the same thing twice changes nothing.
    pub fn apply(&self, transition: WindowTransition) {
        let observed = self.clock.now();
        let mut inner = self.lock();
        Self::expire(&mut inner, observed);
        let changed = match transition {
            WindowTransition::Running { job, started } => Self::open(
                &mut inner,
                job,
                started.unwrap_or(observed),
                self.generation() + 1,
            ),
            WindowTransition::Finished { job, finished } => Self::close(
                &mut inner,
                &job,
                finished.unwrap_or(observed),
                observed,
                self.generation() + 1,
            ),
        };
        if changed {
            self.generation.send_modify(|g| *g += 1);
        }
    }

    fn open(inner: &mut Inner, job: JobRef, opened_at: Timestamp, number: u64) -> bool {
        if inner.windows.iter().any(|w| w.is_open() && w.job == job) {
            return false;
        }
        if inner.windows.len() >= MAX_WINDOWS {
            // Closed windows go first, the one closed longest ago first; a ledger of open windows only is full of Jobs
            // and says so.
            let oldest_closed = inner
                .windows
                .iter()
                .enumerate()
                .filter_map(|(at, w)| w.closed_gen.map(|closed| (closed, at)))
                .min();
            if let Some((_, at)) = oldest_closed {
                inner.windows.remove(at);
            } else {
                warn!("too many sync Jobs are running; one is not tracked");
                return false;
            }
        }
        let id = inner.next_id;
        inner.next_id += 1;
        inner.windows.push(Window {
            id,
            job,
            opened_at,
            opened_gen: number,
            closed_at: None,
            closed_gen: None,
            closed_seen: None,
        });
        true
    }

    fn close(
        inner: &mut Inner,
        job: &JobRef,
        closed_at: Timestamp,
        observed: Timestamp,
        number: u64,
    ) -> bool {
        // A Job that finished before the agent saw it run has no window: nothing was tagged, so nothing is owed.
        let Some(window) = inner.windows.iter_mut().find(|w| w.is_open() && w.job == *job) else {
            return false;
        };
        window.closed_at = Some(closed_at);
        window.closed_gen = Some(number);
        window.closed_seen = Some(observed);
        true
    }

    /// Forget the windows that closed more than [`RETENTION`] ago.
    fn expire(inner: &mut Inner, now: Timestamp) {
        let keep_after = now
            .unix_millis()
            .saturating_sub(i64::try_from(RETENTION.as_millis()).unwrap_or(i64::MAX));
        inner
            .windows
            .retain(|w| w.closed_seen.is_none_or(|seen| seen.unix_millis() >= keep_after));
    }

    /// The Job whose window is open now: the one that opened first, if several are.
    pub fn active(&self) -> Option<JobRef> {
        self.lock()
            .windows
            .iter()
            .find(|w| w.is_open())
            .map(|w| w.job.clone())
    }

    /// The Job whose window overlaps the span between two walks, which started when the ledger was at generations `after`
    /// and `upto`: opened by `upto`, and not closed by `after`. The oldest, if several do.
    pub fn overlapping(&self, after: u64, upto: u64) -> Option<JobRef> {
        self.lock()
            .windows
            .iter()
            .find(|w| w.opened_gen <= upto && w.closed_gen.is_none_or(|closed| closed > after))
            .map(|w| w.job.clone())
    }

    /// The windows a hub that knows nothing needs to hear about: the open ones, and those closed in the last hour.
    pub fn views(&self) -> Vec<WindowView> {
        let now = self.clock.now();
        let mut inner = self.lock();
        Self::expire(&mut inner, now);
        inner.windows.iter().map(Window::view).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use tokio::time::Instant;

    use super::*;

    #[derive(Debug)]
    struct ManualClock(AtomicI64);

    impl ManualClock {
        fn advance(&self, by: Duration) {
            self.0
                .fetch_add(i64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0.load(Ordering::SeqCst))
        }

        fn instant(&self) -> Instant {
            Instant::now()
        }
    }

    const T0: i64 = 1_800_000_000_000;

    fn ledger() -> (Arc<ManualClock>, WindowLedger) {
        let clock = Arc::new(ManualClock(AtomicI64::new(T0)));
        let ledger = WindowLedger::new(clock.clone());
        (clock, ledger)
    }

    fn job(name: &str) -> JobRef {
        JobRef::new(name, &format!("uid-{name}")).unwrap()
    }

    fn running(name: &str) -> WindowTransition {
        WindowTransition::Running {
            job: job(name),
            started: None,
        }
    }

    fn finished(name: &str) -> WindowTransition {
        WindowTransition::Finished {
            job: job(name),
            finished: None,
        }
    }

    #[test]
    fn a_running_job_opens_one_window_however_often_it_is_seen() {
        let (_, ledger) = ledger();
        assert_eq!(ledger.active(), None);
        ledger.apply(running("dataload-1"));
        ledger.apply(running("dataload-1"));
        assert_eq!(ledger.active(), Some(job("dataload-1")));
        assert_eq!(ledger.views().len(), 1);
        assert_eq!(ledger.generation(), 1, "the second sighting changed nothing");
    }

    #[test]
    fn the_start_time_the_api_gives_is_the_one_reported() {
        let (_, ledger) = ledger();
        let started = Timestamp::from_unix_millis(T0 - 90_000);
        ledger.apply(WindowTransition::Running {
            job: job("a"),
            started: Some(started),
        });
        assert_eq!(ledger.views()[0].opened_at, started);
    }

    #[test]
    fn finishing_closes_the_window_and_ends_the_activity() {
        let (clock, ledger) = ledger();
        ledger.apply(running("a"));
        clock.advance(Duration::from_secs(30));
        ledger.apply(finished("a"));
        assert_eq!(ledger.active(), None);
        let views = ledger.views();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].closed_at, Some(Timestamp::from_unix_millis(T0 + 30_000)));
        assert_eq!(views[0].closed_gen, Some(2));
        ledger.apply(finished("a"));
        assert_eq!(ledger.generation(), 2, "closing twice changes nothing");
    }

    #[test]
    fn the_completion_time_the_api_gives_is_the_one_reported() {
        let (clock, ledger) = ledger();
        ledger.apply(running("a"));
        clock.advance(Duration::from_secs(30));
        let done = Timestamp::from_unix_millis(T0 + 25_000);
        ledger.apply(WindowTransition::Finished {
            job: job("a"),
            finished: Some(done),
        });
        assert_eq!(ledger.views()[0].closed_at, Some(done));
    }

    #[test]
    fn a_job_that_finished_before_it_was_seen_has_no_window() {
        let (_, ledger) = ledger();
        ledger.apply(finished("never-seen"));
        assert!(ledger.views().is_empty());
        assert_eq!(ledger.generation(), 0);
    }

    #[test]
    fn a_resumed_job_opens_a_new_window() {
        let (_, ledger) = ledger();
        ledger.apply(running("a"));
        ledger.apply(finished("a"));
        ledger.apply(running("a"));
        let views = ledger.views();
        assert_eq!(views.len(), 2);
        assert_ne!(views[0].id, views[1].id);
        assert_eq!(ledger.active(), Some(job("a")));
    }

    #[test]
    fn overlapping_asks_whether_the_window_was_open_between_two_walks() {
        let (_, ledger) = ledger();
        // generation 1: opened; generation 2: closed.
        ledger.apply(running("a"));
        ledger.apply(finished("a"));
        // A walk that started at generation 0 (before it opened) followed by one at 1: the window was open between.
        assert_eq!(ledger.overlapping(0, 1), Some(job("a")));
        // Walks at 1 and 2: it closed between them, so the second walk is the one that settles it.
        assert_eq!(ledger.overlapping(1, 2), Some(job("a")));
        // Both walks after the close: the window is none of their business.
        assert_eq!(ledger.overlapping(2, 3), None);
        // Both walks before it opened.
        assert_eq!(ledger.overlapping(0, 0), None);
    }

    #[test]
    fn the_oldest_open_window_is_the_one_that_tags() {
        let (_, ledger) = ledger();
        ledger.apply(running("b"));
        ledger.apply(running("a"));
        assert_eq!(ledger.active(), Some(job("b")));
        assert_eq!(ledger.overlapping(0, 2), Some(job("b")));
    }

    #[test]
    fn a_closed_window_is_forgotten_after_an_hour() {
        let (clock, ledger) = ledger();
        ledger.apply(running("a"));
        ledger.apply(running("still-running"));
        ledger.apply(finished("a"));
        clock.advance(RETENTION - Duration::from_secs(1));
        assert_eq!(ledger.views().len(), 2);
        clock.advance(Duration::from_secs(2));
        let views = ledger.views();
        assert_eq!(views.len(), 1);
        assert_eq!(
            views[0].job,
            job("still-running"),
            "an open window is never forgotten"
        );
    }

    #[test]
    fn the_ledger_is_bounded_and_drops_the_oldest_closed_window_first() {
        let (_, ledger) = ledger();
        for n in 0..MAX_WINDOWS {
            ledger.apply(running(&format!("j{n}")));
        }
        ledger.apply(finished("j3"));
        ledger.apply(finished("j1"));
        ledger.apply(running("extra"));
        let views = ledger.views();
        assert_eq!(views.len(), MAX_WINDOWS);
        assert!(views.iter().any(|v| v.job == job("extra")));
        assert!(
            views.iter().all(|v| v.job != job("j3")),
            "the oldest closed one went"
        );
        assert!(views.iter().any(|v| v.job == job("j1")));
    }

    #[test]
    fn a_ledger_full_of_open_windows_refuses_another() {
        let (_, ledger) = ledger();
        for n in 0..MAX_WINDOWS {
            ledger.apply(running(&format!("j{n}")));
        }
        let before = ledger.generation();
        ledger.apply(running("one-too-many"));
        assert_eq!(ledger.views().len(), MAX_WINDOWS);
        assert_eq!(ledger.generation(), before);
    }

    #[test]
    fn subscribers_wake_on_every_change() {
        let (_, ledger) = ledger();
        let mut rx = ledger.subscribe();
        rx.borrow_and_update();
        assert!(!rx.has_changed().unwrap());
        ledger.apply(running("a"));
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), 1);
    }
}
