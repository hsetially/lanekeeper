//! The ring of recent roots (T4): a delta can be computed from any root the hub has seen in the last hour.
//!
//! Each slot holds a whole [`MerkleTree`], which is cheap because trees share every subtree an edit did not touch. The
//! ring is bounded three ways (rule 5): by age (1 hour), by count (512 roots) and by the bytes of the directory nodes
//! the slots created (16 MiB). Eviction is oldest first, and the newest root is never evicted, so a single tree larger
//! than the byte budget still works.
//!
//! The byte budget counts the nodes each root *created*. A node that an evicted root created and a newer root still
//! shares stays alive, so the real memory is the retained bytes plus at most one tree: still bounded, and the bound is
//! what the P4 memory budget is measured against.

use std::collections::VecDeque;
use std::time::Duration;

use domain::ContentHash;
use tokio::time::Instant;

use super::node::MerkleTree;

/// How much history the ring keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingLimits {
    pub max_age: Duration,
    pub max_roots: usize,
    pub max_bytes: usize,
}

impl Default for RingLimits {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(60 * 60),
            max_roots: 512,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
struct Slot {
    tree: MerkleTree,
    at: Instant,
    new_bytes: usize,
}

/// Recent roots, oldest first.
#[derive(Debug)]
pub struct RootRing {
    slots: VecDeque<Slot>,
    limits: RingLimits,
    bytes: usize,
}

impl RootRing {
    pub fn new(limits: RingLimits) -> Self {
        Self {
            slots: VecDeque::new(),
            limits,
            bytes: 0,
        }
    }

    /// Remember `tree`. A tree whose root is the newest one is not added again. `new_bytes` is what building it
    /// allocated (see [`Edited`](super::Edited)). Returns whether the ring changed.
    pub fn push(&mut self, tree: MerkleTree, new_bytes: usize, now: Instant) -> bool {
        if self
            .slots
            .back()
            .is_some_and(|newest| newest.tree.root_hash() == tree.root_hash())
        {
            return false;
        }
        self.bytes += new_bytes;
        self.slots.push_back(Slot {
            tree,
            at: now,
            new_bytes,
        });
        self.evict(now);
        true
    }

    /// Drop what has aged out or does not fit. Call it from the scan tick, so an idle agent does not hold an hour-old
    /// tree for ever.
    pub fn evict(&mut self, now: Instant) {
        while self.slots.len() > 1 {
            let Some(oldest) = self.slots.front() else { break };
            let aged = now.saturating_duration_since(oldest.at) > self.limits.max_age;
            if !(aged || self.slots.len() > self.limits.max_roots || self.bytes > self.limits.max_bytes) {
                break;
            }
            if let Some(gone) = self.slots.pop_front() {
                self.bytes = self.bytes.saturating_sub(gone.new_bytes);
            }
        }
    }

    /// The tree whose root is `root`, if it is still remembered. The newest match wins.
    pub fn get(&self, root: &ContentHash) -> Option<&MerkleTree> {
        self.slots
            .iter()
            .rev()
            .find(|slot| slot.tree.root_hash() == *root)
            .map(|slot| &slot.tree)
    }

    pub fn newest(&self) -> Option<&MerkleTree> {
        self.slots.back().map(|slot| &slot.tree)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Bytes the slots created, the number the byte limit applies to.
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

impl Default for RootRing {
    fn default() -> Self {
        Self::new(RingLimits::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::node::Entry;

    fn tree(n: usize) -> MerkleTree {
        MerkleTree::from_leaves([(
            format!("f{n}"),
            Entry::Other(crate::tree::node::OtherReason::Symlink),
        )])
        .put(&format!("d{n}/x"), Entry::empty_dir())
        .tree
    }

    #[tokio::test(start_paused = true)]
    async fn root_ring_is_bounded() {
        // By count.
        let mut ring = RootRing::new(RingLimits {
            max_roots: 5,
            ..RingLimits::default()
        });
        let now = Instant::now();
        for n in 0..50 {
            ring.push(tree(n), 100, now);
            assert!(ring.len() <= 5);
        }
        assert_eq!(ring.len(), 5);
        assert!(ring.get(&tree(49).root_hash()).is_some());
        assert!(ring.get(&tree(44).root_hash()).is_none(), "the oldest went first");

        // By bytes: the newest always stays, even when it alone is over the budget.
        let mut ring = RootRing::new(RingLimits {
            max_bytes: 1000,
            ..RingLimits::default()
        });
        for n in 0..10 {
            ring.push(tree(n), 400, now);
        }
        assert!(ring.retained_bytes() <= 1000, "{}", ring.retained_bytes());
        assert_eq!(ring.len(), 2);
        ring.push(tree(99), 5000, now);
        assert_eq!(ring.len(), 1);
        assert!(ring.get(&tree(99).root_hash()).is_some());

        // By age.
        let mut ring = RootRing::default();
        ring.push(tree(1), 10, Instant::now());
        tokio::time::advance(Duration::from_secs(30 * 60)).await;
        ring.push(tree(2), 10, Instant::now());
        tokio::time::advance(Duration::from_secs(31 * 60)).await;
        ring.evict(Instant::now());
        assert!(ring.get(&tree(1).root_hash()).is_none(), "older than an hour");
        assert!(ring.get(&tree(2).root_hash()).is_some());
        tokio::time::advance(Duration::from_secs(2 * 60 * 60)).await;
        ring.evict(Instant::now());
        assert_eq!(ring.len(), 1, "the newest root is kept however old");
    }

    #[tokio::test(start_paused = true)]
    async fn the_same_root_is_not_added_twice() {
        let mut ring = RootRing::default();
        let now = Instant::now();
        assert!(ring.push(tree(1), 10, now));
        assert!(!ring.push(tree(1), 10, now));
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.retained_bytes(), 10);
    }
}
