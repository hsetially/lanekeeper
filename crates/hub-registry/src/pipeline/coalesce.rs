//! Coalescing (T3, D66, P13): a burst of changes to a swimlane is one recompute.
//!
//! The first change to a swimlane opens a **window** of `window` (200 ms); every change to the swimlane inside it joins the window; when it
//! closes, the swimlane's paths are handed over together. A sync that rewrites 600 files, or a hundred events of one scan, then cost one
//! recompute and not a hundred. The windows of different swimlanes are independent, so a swimlane that is busy does not delay another,
//! and the ones that close in the same tick are recomputed in one call (the engine parses a base file that forty swimlanes share once).
//!
//! Everything is bounded. A window keeps at most `max_paths` paths: past that the swimlane's paths are not listed any more and the
//! window becomes **whole**, "recompute the swimlane from the database", which is slower and never wrong. At most `max_lanes` windows are
//! open: a change to one more swimlane is not kept and the coalescer says so ([`Coalescer::overflowed`]), so that the caller looks at
//! everything at its next reconcile. No path an event named is ever lost silently: it is in a window, covered by a whole window, or
//! the overflow flag is set.
//!
//! The coalescer reads no clock: the caller passes the instant, so a test moves time by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use domain::{NfsPath, SwimlaneId};
use tokio::time::Instant;

/// What is to be recomputed for a swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    /// These paths, each once, in path order.
    Paths(BTreeSet<NfsPath>),
    /// The whole swimlane.
    Whole,
}

impl Work {
    /// The paths of the work, or none for a whole swimlane.
    #[cfg(test)]
    pub(crate) fn paths(&self) -> Option<&BTreeSet<NfsPath>> {
        match self {
            Self::Paths(p) => Some(p),
            Self::Whole => None,
        }
    }
}

#[derive(Debug)]
struct Window {
    opened: Instant,
    work: Work,
}

#[derive(Debug)]
pub(crate) struct Coalescer {
    window: Duration,
    max_paths: usize,
    max_lanes: usize,
    open: BTreeMap<SwimlaneId, Window>,
    overflowed: bool,
}

impl Coalescer {
    pub(crate) fn new(window: Duration, max_paths: usize, max_lanes: usize) -> Self {
        Self {
            window,
            max_paths: max_paths.max(1),
            max_lanes: max_lanes.max(1),
            open: BTreeMap::new(),
            overflowed: false,
        }
    }

    fn window_of(&mut self, swimlane: &SwimlaneId, now: Instant) -> Option<&mut Window> {
        if !self.open.contains_key(swimlane) {
            if self.open.len() >= self.max_lanes {
                self.overflowed = true;
                return None;
            }
            self.open.insert(
                swimlane.clone(),
                Window {
                    opened: now,
                    work: Work::Paths(BTreeSet::new()),
                },
            );
        }
        self.open.get_mut(swimlane)
    }

    /// One path of a swimlane changed.
    pub(crate) fn add_path(&mut self, swimlane: &SwimlaneId, path: NfsPath, now: Instant) {
        self.add_paths(swimlane, std::iter::once(path), now);
    }

    /// Several paths of a swimlane changed.
    pub(crate) fn add_paths(
        &mut self,
        swimlane: &SwimlaneId,
        paths: impl IntoIterator<Item = NfsPath>,
        now: Instant,
    ) {
        let max = self.max_paths;
        let Some(window) = self.window_of(swimlane, now) else {
            return;
        };
        let Work::Paths(set) = &mut window.work else {
            return;
        };
        for path in paths {
            if set.len() >= max && !set.contains(&path) {
                window.work = Work::Whole;
                return;
            }
            set.insert(path);
        }
    }

    /// The whole swimlane has to be recomputed.
    pub(crate) fn add_whole(&mut self, swimlane: &SwimlaneId, now: Instant) {
        if let Some(window) = self.window_of(swimlane, now) {
            window.work = Work::Whole;
        }
    }

    /// When the first open window closes, or `None` when none is open.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.open.values().map(|w| w.opened + self.window).min()
    }

    /// The windows that have closed by `now`, in swimlane order. They are removed.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<(SwimlaneId, Work)> {
        let window = self.window;
        let due: Vec<SwimlaneId> = self
            .open
            .iter()
            .filter(|(_, w)| now >= w.opened + window)
            .map(|(id, _)| id.clone())
            .collect();
        due.into_iter()
            .filter_map(|id| self.open.remove(&id).map(|w| (id, w.work)))
            .collect()
    }

    /// Every open window, closed or not (at the end of a run).
    pub(crate) fn take_all(&mut self) -> Vec<(SwimlaneId, Work)> {
        std::mem::take(&mut self.open)
            .into_iter()
            .map(|(id, w)| (id, w.work))
            .collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.open.len()
    }

    /// A change was not kept because too many windows were open. Reading it clears it.
    pub(crate) fn overflowed(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(id: &str) -> SwimlaneId {
        SwimlaneId::parse(id).unwrap()
    }

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    fn coalescer() -> Coalescer {
        Coalescer::new(Duration::from_millis(200), 3, 2)
    }

    #[test]
    fn a_burst_in_one_window_is_one_unit_of_work() {
        let t0 = Instant::now();
        let mut c = coalescer();
        c.add_path(&lane("a"), path("x.yml"), t0);
        c.add_path(&lane("a"), path("y.yml"), t0 + Duration::from_millis(150));
        c.add_path(&lane("a"), path("x.yml"), t0 + Duration::from_millis(190));
        assert_eq!(c.len(), 1);
        assert!(
            c.take_due(t0 + Duration::from_millis(199)).is_empty(),
            "the window is still open"
        );
        let due = c.take_due(t0 + Duration::from_millis(200));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, lane("a"));
        let paths: Vec<&str> = due[0].1.paths().unwrap().iter().map(NfsPath::as_str).collect();
        assert_eq!(paths, ["x.yml", "y.yml"], "each path once, in order");
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn the_window_opens_at_the_first_change_and_does_not_slide() {
        let t0 = Instant::now();
        let mut c = coalescer();
        c.add_path(&lane("a"), path("x.yml"), t0);
        assert_eq!(c.next_deadline(), Some(t0 + Duration::from_millis(200)));
        c.add_path(&lane("a"), path("y.yml"), t0 + Duration::from_millis(190));
        assert_eq!(
            c.next_deadline(),
            Some(t0 + Duration::from_millis(200)),
            "a steady stream of changes must not keep the swimlane waiting for ever"
        );
    }

    #[test]
    fn swimlanes_have_windows_of_their_own() {
        let t0 = Instant::now();
        let mut c = coalescer();
        c.add_path(&lane("a"), path("x.yml"), t0);
        c.add_path(&lane("b"), path("x.yml"), t0 + Duration::from_millis(100));
        let first = c.take_due(t0 + Duration::from_millis(200));
        assert_eq!(first.len(), 1, "only a's window has closed");
        assert_eq!(first[0].0, lane("a"));
        assert_eq!(c.next_deadline(), Some(t0 + Duration::from_millis(300)));
        let both = {
            c.add_path(&lane("a"), path("z.yml"), t0 + Duration::from_millis(250));
            c.take_due(t0 + Duration::from_millis(500))
        };
        let ids: Vec<&str> = both.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["a", "b"],
            "the ones that close together come out together, in order"
        );
    }

    #[test]
    fn past_the_bound_the_window_is_the_whole_swimlane() {
        let t0 = Instant::now();
        let mut c = coalescer();
        for p in ["a.yml", "b.yml", "c.yml"] {
            c.add_path(&lane("a"), path(p), t0);
        }
        assert!(c.take_all()[0].1.paths().is_some(), "three paths fit");
        for p in ["a.yml", "b.yml", "c.yml", "d.yml"] {
            c.add_path(&lane("a"), path(p), t0);
        }
        let due = c.take_all();
        assert_eq!(due[0].1, Work::Whole, "a fourth path makes the window whole");
        // Once whole, more paths change nothing.
        c.add_whole(&lane("a"), t0);
        c.add_path(&lane("a"), path("e.yml"), t0);
        assert_eq!(c.take_all()[0].1, Work::Whole);
    }

    #[test]
    fn a_path_already_in_a_full_window_does_not_make_it_whole() {
        let t0 = Instant::now();
        let mut c = coalescer();
        for p in ["a.yml", "b.yml", "c.yml", "a.yml", "b.yml"] {
            c.add_path(&lane("a"), path(p), t0);
        }
        assert!(c.take_all()[0].1.paths().is_some());
    }

    #[test]
    fn too_many_open_windows_set_the_overflow_flag_and_lose_nothing_silently() {
        let t0 = Instant::now();
        let mut c = coalescer();
        c.add_path(&lane("a"), path("x.yml"), t0);
        c.add_path(&lane("b"), path("x.yml"), t0);
        assert!(!c.overflowed());
        c.add_path(&lane("c"), path("x.yml"), t0);
        assert!(
            c.overflowed(),
            "the third swimlane did not fit and the caller is told"
        );
        assert!(!c.overflowed(), "reading it clears it");
        assert_eq!(c.len(), 2);
        // A swimlane that already has a window is never refused.
        c.add_path(&lane("a"), path("y.yml"), t0);
        assert!(!c.overflowed());
    }

    #[test]
    fn nothing_is_due_when_nothing_is_open() {
        let mut c = coalescer();
        assert_eq!(c.next_deadline(), None);
        assert!(c.take_due(Instant::now()).is_empty());
        assert!(c.take_all().is_empty());
    }

    proptest::proptest! {
        /// No path that was added is ever lost: after any mix of adds and takes, every added path is in some window that was taken,
        /// is covered by a whole window, or the overflow flag was raised.
        #[test]
        fn coalescing_never_loses_an_affected_path(
            ops in proptest::collection::vec((0_u8..4, 0_u8..8, 0_u16..400, proptest::bool::weighted(0.1)), 1..60)
        ) {
            let t0 = Instant::now();
            let mut c = Coalescer::new(Duration::from_millis(200), 4, 3);
            let mut added: BTreeSet<(String, String)> = BTreeSet::new();
            let mut taken: BTreeSet<(String, String)> = BTreeSet::new();
            let mut whole: BTreeSet<String> = BTreeSet::new();
            let mut overflow = false;
            let mut now = t0;
            for (l, p, dt, take) in ops {
                now += Duration::from_millis(u64::from(dt) / 4);
                let id = lane(&format!("l{l}"));
                if take {
                    for (lane_id, work) in c.take_due(now) {
                        match work {
                            Work::Whole => { whole.insert(lane_id.as_str().to_owned()); }
                            Work::Paths(set) => {
                                for p in set { taken.insert((lane_id.as_str().to_owned(), p.as_str().to_owned())); }
                            }
                        }
                    }
                } else {
                    let name = format!("f{p}.yml");
                    let before = c.len();
                    c.add_path(&id, path(&name), now);
                    overflow |= c.overflowed();
                    let _ = before;
                    added.insert((id.as_str().to_owned(), name));
                }
            }
            for (lane_id, work) in c.take_all() {
                match work {
                    Work::Whole => { whole.insert(lane_id.as_str().to_owned()); }
                    Work::Paths(set) => {
                        for p in set { taken.insert((lane_id.as_str().to_owned(), p.as_str().to_owned())); }
                    }
                }
            }
            for (lane_id, name) in &added {
                proptest::prop_assert!(
                    taken.contains(&(lane_id.clone(), name.clone())) || whole.contains(lane_id) || overflow,
                    "{lane_id}/{name} was lost"
                );
            }
        }
    }
}
