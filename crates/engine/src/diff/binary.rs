//! Binary comparison (T5): a binary file is never shown, only compared by its hash and its size.

use domain::{BinaryCompare, ContentHash};

/// What is known about one side of a binary comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinaryFacts {
    pub hash: ContentHash,
    pub size: u64,
}

/// The result of `binary_compare`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryDiff {
    /// Both sides are the same file: the same hash and the same size, or neither side exists.
    pub identical: bool,
    /// The contract's description of the comparison (`Comparison::binary`).
    pub compare: BinaryCompare,
}

/// Compares two binary files by hash and size. `None` is a side where the file does not exist.
///
/// The hash decides. A pair with the same hash and different sizes cannot be a real pair of files; it is not called
/// identical, because a wrong "identical" is worse than a wrong "different".
pub fn binary_compare(left: Option<BinaryFacts>, right: Option<BinaryFacts>) -> BinaryDiff {
    let identical = match (left, right) {
        (Some(l), Some(r)) => l.hash == r.hash && l.size == r.size,
        (None, None) => true,
        _ => false,
    };
    BinaryDiff {
        identical,
        compare: BinaryCompare {
            left: left.map(|f| f.hash),
            right: right.map(|f| f.hash),
            left_size: left.map(|f| f.size),
            right_size: right.map(|f| f.size),
        },
    }
}
