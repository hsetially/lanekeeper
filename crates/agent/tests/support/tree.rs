//! Small constructors for tree tests: a file leaf whose hash really is the SHA-256 of its content.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use agent::tree::{Entry, FileLeaf, Stat, StatTime};
use domain::ContentHash;
use sha2::{Digest, Sha256};

/// SHA-256 of `content`.
pub fn sha(content: &[u8]) -> [u8; 32] {
    Sha256::digest(content).into()
}

/// A leaf for a file with this content. The stat is a function of the content, so equal content gives an equal leaf.
pub fn leaf(content: &[u8]) -> FileLeaf {
    let hash = ContentHash::from_bytes(sha(content));
    FileLeaf::new(
        hash,
        Stat::new(
            content.len() as u64,
            StatTime::new(1_700_000_000, 0),
            StatTime::new(1_700_000_000, 0),
            u64::from(hash.as_bytes()[0]) + 1,
        ),
    )
}

pub fn file(content: &[u8]) -> Entry {
    Entry::File(leaf(content))
}

/// The same leaf with another modification time: a stat change that leaves the content alone.
pub fn touched(mut leaf: FileLeaf, mtime_secs: i64) -> FileLeaf {
    leaf.stat.mtime = StatTime::new(mtime_secs, 0);
    leaf
}
