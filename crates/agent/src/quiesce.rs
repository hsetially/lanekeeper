//! Quiescence (T10, D75, P1): a change is reported when the tree has been quiet for 3 s, and never later than 30 s after
//! it began.
//!
//! A copy of 600 files is not 600 changes: it is one, and the hub should hear about it once, when it is over. The
//! [`Quiescer`] is the rule for that, and nothing else. It holds no tree and reads no file; the scanner tells it what each
//! walk found and it answers hold or release.
//!
//! # The two clocks of a change
//!
//! - **Quiet** is measured from the moment of the newest change. That moment is not the walk that found it: a file that
//!   was finished before the walk is quiet already, and waiting another 3 s for it would put P1's 15 s at risk (the 10 s
//!   walk is two thirds of it). [`change_moment`] estimates when the changes of a walk happened from the files' own times
//!   (the newer of mtime and ctime, so a copy that preserves mtimes is still dated by the server), never earlier than the
//!   previous walk (the file was unchanged then) and never later than now.
//! - **Deferral** is measured from the moment of the oldest change. After 30 s a delta goes out whatever is still
//!   arriving, so a writer that never stops cannot keep a change from the hub.
//!
//! While changes are held, the scanner walks every second (`Tunables::pending_walk_interval`) so that these two moments
//! are met within a second.

use std::time::Duration;

use domain::Timestamp;
use tokio::time::Instant;

use crate::tree::{StatTime, TreeDiff};

/// When the changes found by one walk most likely happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moments {
    /// The oldest of them.
    pub first: Instant,
    /// The newest of them.
    pub last: Instant,
}

/// What to do with the changes that are waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// There is nothing to report.
    Idle,
    /// Changes are waiting, and the tree is not quiet yet.
    Hold,
    /// Report them now.
    Release,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    first: Instant,
    last: Instant,
}

/// The rule of D75. Cheap, `Copy`-free state: two instants.
#[derive(Debug, Clone)]
pub struct Quiescer {
    quiet: Duration,
    max_defer: Duration,
    pending: Option<Pending>,
}

impl Quiescer {
    pub fn new(quiet: Duration, max_defer: Duration) -> Self {
        Self {
            quiet,
            max_defer,
            pending: None,
        }
    }

    /// The timing in force. The next [`Quiescer::observe`] uses it; changes already held keep their moments.
    pub fn set_timing(&mut self, quiet: Duration, max_defer: Duration) {
        self.quiet = quiet;
        self.max_defer = max_defer;
    }

    /// Changes are being held back.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Forget what is held, because it was reported or no longer exists.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// A walk finished at `now`. `unreported` says the tree differs from what the hub was last told; `found` says when
    /// the changes this walk found happened (`None`: this walk found nothing new).
    pub fn observe(&mut self, now: Instant, unreported: bool, found: Option<Moments>) -> Verdict {
        if !unreported {
            self.pending = None;
            return Verdict::Idle;
        }
        let pending = self.pending.get_or_insert_with(|| {
            // Changes with no known moment (the hub's picture and ours differ for a reason no walk showed) count from now.
            let moments = found.unwrap_or(Moments {
                first: now,
                last: now,
            });
            Pending {
                first: moments.first,
                last: moments.last,
            }
        });
        if let Some(found) = found {
            pending.first = pending.first.min(found.first);
            pending.last = pending.last.max(found.last);
        }
        let quiet = now.saturating_duration_since(pending.last) >= self.quiet;
        let overdue = now.saturating_duration_since(pending.first) >= self.max_defer;
        if quiet || overdue {
            self.pending = None;
            Verdict::Release
        } else {
            Verdict::Hold
        }
    }
}

/// When the changes in `diff`, found by a walk at `now` (wall clock `wall_now`) after one at `previous_walk`, happened.
pub fn change_moment(diff: &TreeDiff, previous_walk: Instant, now: Instant, wall_now: Timestamp) -> Moments {
    let floor = previous_walk.min(now);
    // The instant a file time stands for. A time too old for an `Instant` to hold is as old as the previous walk.
    let instant_of = |time: StatTime| {
        now.checked_sub(age_of(time, wall_now))
            .unwrap_or(floor)
            .clamp(floor, now)
    };
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    let mut note = |moment: Instant| {
        first = Some(first.map_or(moment, |f| f.min(moment)));
        last = Some(last.map_or(moment, |l| l.max(moment)));
    };
    for file in &diff.changed {
        let stat = file.leaf.stat;
        // ctime is set by the server when the file changes and cannot be copied from another file, so it is the one a
        // `cp -p` or `rsync -t` does not hide.
        let newer = if stat.ctime.unix_millis() >= stat.mtime.unix_millis() {
            stat.ctime
        } else {
            stat.mtime
        };
        note(instant_of(newer));
    }
    if !diff.removed.is_empty() || !diff.skipped.is_empty() {
        // When a file vanished is in no stat.
        note(now);
    }
    Moments {
        first: first.unwrap_or(now),
        last: last.unwrap_or(now),
    }
}

/// How old a file time is, relative to `wall_now`. A time in the future is zero old.
fn age_of(time: StatTime, wall_now: Timestamp) -> Duration {
    let millis = wall_now.unix_millis().saturating_sub(time.unix_millis()).max(0);
    Duration::from_millis(u64::try_from(millis).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const QUIET: Duration = Duration::from_secs(3);
    const DEFER: Duration = Duration::from_secs(30);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn quiescer() -> Quiescer {
        Quiescer::new(QUIET, DEFER)
    }

    fn at(base: Instant, n: u64) -> Instant {
        base + secs(n)
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the argument it is passed as is an Option"
    )]
    fn moments(first: Instant, last: Instant) -> Option<Moments> {
        Some(Moments { first, last })
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_unreported_is_idle() {
        let t0 = Instant::now();
        let mut q = quiescer();
        assert_eq!(q.observe(t0, false, None), Verdict::Idle);
        assert!(!q.is_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_for_3s_before_delta() {
        let t0 = Instant::now();
        let mut q = quiescer();
        // A change at t0 + 10, found by the walk at t0 + 10.
        let change = at(t0, 10);
        assert_eq!(q.observe(change, true, moments(change, change)), Verdict::Hold);
        assert!(q.is_pending());
        assert_eq!(q.observe(at(t0, 11), true, None), Verdict::Hold);
        assert_eq!(q.observe(at(t0, 12), true, None), Verdict::Hold);
        assert_eq!(q.observe(at(t0, 13), true, None), Verdict::Release);
        assert!(!q.is_pending(), "a release clears what was held");
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_that_finished_before_the_walk_is_released_at_once() {
        let t0 = Instant::now();
        let mut q = quiescer();
        let found_at = at(t0, 10);
        // It happened at t0 + 2, long before the walk at t0 + 10.
        assert_eq!(
            q.observe(found_at, true, moments(at(t0, 2), at(t0, 2))),
            Verdict::Release
        );
    }

    #[tokio::test(start_paused = true)]
    async fn every_new_change_restarts_the_quiet_period() {
        let t0 = Instant::now();
        let mut q = quiescer();
        assert_eq!(
            q.observe(at(t0, 1), true, moments(at(t0, 1), at(t0, 1))),
            Verdict::Hold
        );
        assert_eq!(
            q.observe(at(t0, 3), true, moments(at(t0, 3), at(t0, 3))),
            Verdict::Hold
        );
        // Four seconds after the first change, one after the last.
        assert_eq!(q.observe(at(t0, 5), true, None), Verdict::Hold);
        assert_eq!(q.observe(at(t0, 6), true, None), Verdict::Release);
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_at_most_30s_under_constant_change() {
        let t0 = Instant::now();
        let mut q = quiescer();
        // A change every second from t0 on: never quiet.
        let mut released = None;
        for n in 0..=40 {
            let now = at(t0, n);
            if q.observe(now, true, moments(now, now)) == Verdict::Release {
                released = Some(n);
                break;
            }
        }
        assert_eq!(
            released,
            Some(30),
            "released 30 s after the first change, not before"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_deferral_counts_from_the_oldest_change_not_from_the_walk() {
        let t0 = Instant::now();
        let mut q = quiescer();
        // The first walk finds changes that began 20 s ago and are still going.
        let now = at(t0, 20);
        assert_eq!(q.observe(now, true, moments(t0, now)), Verdict::Hold);
        assert_eq!(
            q.observe(at(t0, 29), true, moments(at(t0, 29), at(t0, 29))),
            Verdict::Hold
        );
        assert_eq!(
            q.observe(at(t0, 30), true, moments(at(t0, 30), at(t0, 30))),
            Verdict::Release,
            "30 s after the oldest change"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_that_is_undone_leaves_nothing_to_report() {
        let t0 = Instant::now();
        let mut q = quiescer();
        assert_eq!(
            q.observe(at(t0, 1), true, moments(at(t0, 1), at(t0, 1))),
            Verdict::Hold
        );
        // The next walk finds the tree as the hub knows it.
        assert_eq!(q.observe(at(t0, 2), false, None), Verdict::Idle);
        assert!(!q.is_pending());
        // A later change starts again from nothing.
        assert_eq!(
            q.observe(at(t0, 20), true, moments(at(t0, 20), at(t0, 20))),
            Verdict::Hold
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unreported_with_no_known_moment_counts_from_now() {
        let t0 = Instant::now();
        let mut q = quiescer();
        assert_eq!(q.observe(t0, true, None), Verdict::Hold);
        assert_eq!(q.observe(at(t0, 3), true, None), Verdict::Release);
    }

    #[tokio::test(start_paused = true)]
    async fn the_timing_can_change_while_changes_are_held() {
        let t0 = Instant::now();
        let mut q = quiescer();
        assert_eq!(q.observe(t0, true, moments(t0, t0)), Verdict::Hold);
        q.set_timing(secs(1), DEFER);
        assert_eq!(q.observe(at(t0, 1), true, None), Verdict::Release);
    }

    // ---------------------------------------------------------------------------------------- the moment of a change

    use crate::tree::{ChangedFile, FileLeaf, Skip, SkipReason, Stat};
    use domain::ContentHash;

    fn leaf(mtime_ms: i64, ctime_ms: i64) -> FileLeaf {
        let time = |ms: i64| {
            StatTime::new(
                ms.div_euclid(1000),
                u32::try_from(ms.rem_euclid(1000) * 1_000_000).unwrap(),
            )
        };
        FileLeaf::new(
            ContentHash::from_bytes([1; 32]),
            Stat::new(1, time(mtime_ms), time(ctime_ms), 7),
        )
    }

    fn diff_of(files: &[(i64, i64)]) -> TreeDiff {
        TreeDiff {
            changed: files
                .iter()
                .enumerate()
                .map(|(i, (m, c))| ChangedFile {
                    path: format!("f{i}").into_bytes(),
                    leaf: leaf(*m, *c),
                })
                .collect(),
            ..TreeDiff::default()
        }
    }

    const WALL: i64 = 1_800_000_000_000;

    #[tokio::test(start_paused = true)]
    async fn a_file_is_dated_by_its_newer_time() {
        let t0 = Instant::now();
        let now = at(t0, 10);
        let wall = Timestamp::from_unix_millis(WALL);
        // mtime preserved from years ago (cp -p), ctime four seconds ago.
        let diff = diff_of(&[(WALL - 90_000_000_000, WALL - 4_000)]);
        let m = change_moment(&diff, t0, now, wall);
        assert_eq!(m.last, now - Duration::from_secs(4));
        assert_eq!(m.first, m.last);
    }

    #[tokio::test(start_paused = true)]
    async fn the_moment_is_never_before_the_previous_walk() {
        let t0 = Instant::now();
        let previous = at(t0, 5);
        let now = at(t0, 15);
        let wall = Timestamp::from_unix_millis(WALL);
        // Both times are a year old: the file cannot have changed before the walk that saw it unchanged.
        let diff = diff_of(&[(WALL - 31_536_000_000, WALL - 31_536_000_000)]);
        let m = change_moment(&diff, previous, now, wall);
        assert_eq!((m.first, m.last), (previous, previous));
    }

    #[tokio::test(start_paused = true)]
    async fn a_time_in_the_future_is_now() {
        let t0 = Instant::now();
        let now = at(t0, 10);
        let wall = Timestamp::from_unix_millis(WALL);
        let diff = diff_of(&[(WALL + 60_000, WALL + 60_000)]);
        let m = change_moment(&diff, t0, now, wall);
        assert_eq!((m.first, m.last), (now, now));
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_and_last_moment_span_all_the_files() {
        let t0 = Instant::now();
        let now = at(t0, 10);
        let wall = Timestamp::from_unix_millis(WALL);
        let diff = diff_of(&[
            (WALL - 8_000, WALL - 8_000),
            (WALL - 1_000, WALL - 1_000),
            (WALL - 5_000, WALL - 5_000),
        ]);
        let m = change_moment(&diff, t0, now, wall);
        assert_eq!(m.first, now - Duration::from_secs(8));
        assert_eq!(m.last, now - Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_removal_has_no_time_and_counts_as_now() {
        let t0 = Instant::now();
        let now = at(t0, 10);
        let wall = Timestamp::from_unix_millis(WALL);
        let mut diff = diff_of(&[(WALL - 9_000, WALL - 9_000)]);
        diff.removed.push(b"gone".to_vec());
        let m = change_moment(&diff, t0, now, wall);
        assert_eq!(m.last, now, "when a file vanished is not in any stat");
        assert_eq!(m.first, now - Duration::from_secs(9));
    }

    #[tokio::test(start_paused = true)]
    async fn a_skipped_entry_counts_as_now() {
        let t0 = Instant::now();
        let now = at(t0, 10);
        let wall = Timestamp::from_unix_millis(WALL);
        let diff = TreeDiff {
            skipped: vec![Skip {
                path: b"link".to_vec(),
                reason: SkipReason::Symlink,
            }],
            ..TreeDiff::default()
        };
        let m = change_moment(&diff, t0, now, wall);
        assert_eq!((m.first, m.last), (now, now));
    }

    #[tokio::test(start_paused = true)]
    async fn an_empty_diff_counts_as_now() {
        let t0 = Instant::now();
        let now = at(t0, 10);
        let m = change_moment(&TreeDiff::default(), t0, now, Timestamp::from_unix_millis(WALL));
        assert_eq!((m.first, m.last), (now, now));
    }
}
