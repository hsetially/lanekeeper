//! The Merkle tree and how it is built (T4, D63, P1, P4).
//!
//! - [`node`]: the tree, its byte encoding, edits and diffs;
//! - [`ring`]: the roots of the last hour;
//! - [`walk`]: the own parallel walker over the cap-std root (decision A1);
//! - [`hash`]: the worker pool and streaming hashes;
//! - [`source`]: the file system behind the [`TreeSource`] trait.
//!
//! Building deltas from tree differences is in [`delta`].

pub mod delta;
pub mod hash;
pub mod node;
pub mod ring;
pub mod source;
#[cfg(test)]
mod testfs;
pub mod walk;

pub use hash::{HASH_LIMIT, Pool, PoolError};
pub use node::{
    ChangedFile, DirNode, Edited, Entry, FileLeaf, Kind, MerkleTree, Name, OtherReason, Skip, SkipReason,
    Stat, StatTime, TreeDiff,
};
pub use ring::{RingLimits, RootRing};
pub use source::{FileRead, FsSource, ReadError, ReadRequest, RefreshRequest, Refreshed, TreeSource};
pub use walk::{ScanError, ScanMode, ScanOutcome, ScanStats, WalkConfig};
