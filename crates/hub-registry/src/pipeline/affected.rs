//! From an event to the paths it affects (T3, plan A21, P2): the first step of "recompute only what can have changed".
//!
//! | Event | Affected |
//! |---|---|
//! | `FileObserved { swimlane, path }` | that path of that swimlane |
//! | `GitHeadMoved { repo, branch, from, to }` | what [`crate::index::GitIndex::affected`] says: the paths of the tree diff, mapped to NFS names, for each swimlane the branch concerns (the base default branch concerns every swimlane with an inventory, a tenant branch the swimlanes that deploy it, a template or the docs none); the whole swimlane when the diff is too large to list or its old side is gone |
//! | `Resync` | everything may have been missed: the caller reconciles |
//! | anything else | nothing |
//!
//! The pipeline remembers where each branch was when its last move was handled ([`super::Inner::processed`]) **only after** the move
//! was routed, so a move that could not be routed (the mirror did not answer) is found again by the reconcile, which compares the
//! mirror's heads with those and routes the difference as if the event had just come.

use domain::DomainEvent;
use tokio::time::Instant;

use super::Inner;
use super::coalesce::{Coalescer, Work};
use crate::error::RegistryError;
use crate::index::{Affected, LaneImpact};

/// The most branches the pipeline remembers a head of. There are hundreds of branches in the tenant repository at most.
const MAX_REMEMBERED_BRANCHES: usize = 4_096;

/// What routing an event did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Routed {
    /// One path of one swimlane was queued.
    Path,
    /// A moved head concerned this many swimlanes.
    Moved {
        lanes: usize,
    },
    /// The subscriber lagged: reconcile.
    Resync,
    Ignored,
}

/// Queue what an [`Affected`] names.
pub(crate) fn queue(coalescer: &mut Coalescer, affected: &Affected, now: Instant) {
    for (swimlane, impact) in &affected.lanes {
        match impact {
            LaneImpact::Paths(paths) => coalescer.add_paths(swimlane, paths.iter().cloned(), now),
            LaneImpact::Whole => coalescer.add_whole(swimlane, now),
        }
    }
}

/// Queue work that is already in the form the coalescer holds.
pub(crate) fn requeue(coalescer: &mut Coalescer, work: Vec<(domain::SwimlaneId, Work)>, now: Instant) {
    for (swimlane, work) in work {
        match work {
            Work::Paths(paths) => coalescer.add_paths(&swimlane, paths, now),
            Work::Whole => coalescer.add_whole(&swimlane, now),
        }
    }
}

impl Inner {
    /// Route one event into the coalescer.
    ///
    /// # Errors
    ///
    /// The mirror could not say what a moved head changed. Nothing was queued and the branch is not marked as handled, so the
    /// reconcile tries again.
    pub(crate) async fn route(
        &self,
        event: DomainEvent,
        coalescer: &mut Coalescer,
        now: Instant,
    ) -> Result<Routed, RegistryError> {
        match event {
            DomainEvent::FileObserved { swimlane, path, .. } => {
                coalescer.add_path(&swimlane, path, now);
                Ok(Routed::Path)
            }
            DomainEvent::GitHeadMoved {
                repo,
                ref branch,
                ref from,
                ref to,
            } => {
                let affected = self
                    .git_index
                    .affected(repo, branch.as_str(), from.as_ref(), to)
                    .await?;
                queue(coalescer, &affected, now);
                self.remember(repo, branch.as_str(), to);
                Ok(Routed::Moved {
                    lanes: affected.lanes.len(),
                })
            }
            DomainEvent::Resync => Ok(Routed::Resync),
            _ => Ok(Routed::Ignored),
        }
    }

    /// Note that the move of `branch` to `to` has been handled.
    pub(crate) fn remember(&self, repo: domain::RepoKind, branch: &str, to: &domain::CommitId) {
        let Ok(mut processed) = self.processed.lock() else {
            return;
        };
        let key = (repo, branch.to_owned());
        if processed.contains_key(&key) || processed.len() < MAX_REMEMBERED_BRANCHES {
            processed.insert(key, to.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::time::Duration;

    use domain::{NfsPath, SwimlaneId};

    fn lane(id: &str) -> SwimlaneId {
        SwimlaneId::parse(id).unwrap()
    }

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    #[test]
    fn an_affected_set_is_queued_per_swimlane_and_a_whole_one_is_whole() {
        let shared: Arc<BTreeSet<NfsPath>> = Arc::new([path("a.yml"), path("b.yml")].into());
        let affected = Affected {
            lanes: BTreeMap::from([
                (lane("a"), LaneImpact::Paths(Arc::clone(&shared))),
                (lane("b"), LaneImpact::Paths(shared)),
                (lane("c"), LaneImpact::Whole),
            ]),
            changed: 2,
            unmapped: 0,
        };
        let t0 = Instant::now();
        let mut c = Coalescer::new(Duration::from_millis(200), 10, 10);
        queue(&mut c, &affected, t0);
        let due = c.take_all();
        assert_eq!(due.len(), 3);
        assert_eq!(due[0].1.paths().map(BTreeSet::len), Some(2));
        assert_eq!(due[1].1.paths().map(BTreeSet::len), Some(2));
        assert_eq!(due[2].1, Work::Whole);
    }

    #[test]
    fn requeued_work_keeps_its_shape() {
        let t0 = Instant::now();
        let mut c = Coalescer::new(Duration::from_millis(200), 10, 10);
        requeue(
            &mut c,
            vec![
                (lane("a"), Work::Paths([path("x.yml")].into())),
                (lane("b"), Work::Whole),
            ],
            t0,
        );
        let due = c.take_all();
        assert_eq!(due[0].1.paths().map(BTreeSet::len), Some(1));
        assert_eq!(due[1].1, Work::Whole);
    }
}
