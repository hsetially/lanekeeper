//! Adoption ranking (D22): which deployable branch does a swimlane's NFS look like?
//!
//! Adoption proposes the branch a swimlane was deployed from, so that an admin can confirm it. The evidence is the tenant
//! files on NFS: each deployable branch is mapped forward (`data/config/<rel>/<name>.<ext>` on branch B to
//! `<rel>/<name>-B.<ext>`), and the branch whose mapped files agree most with the swimlane's wins. Agreement is the Jaccard
//! similarity of two sets of (NFS path, normalised hash) pairs: the pairs both have, over the pairs either has. A file that is
//! in the swimlane and in the branch with other content counts against both, which is what tells `sit1` from `sit2` when
//! almost every file of the two is the same.
//!
//! Similarity is an exact fraction, compared by cross-multiplying, so that no floating-point rounding can reorder two branches.
//! Ties are broken by the branch name. Template branches (`templates/*`), the docs branch and every branch that is not a
//! deployable tenant branch are never ranked: a template is never deployed (D22).

use std::cmp::Ordering;

use domain::{BranchKind, ContentHash, GitRef, NfsPath, PathMappingRule, RepoPath, TenantId};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::error::BatchError;
use crate::paths::{RepoSource, map_path};

/// The most branches `adoption_rank` takes.
pub const MAX_ADOPTION_CANDIDATES: usize = 10_000;
/// The most files in one list (a swimlane's or a branch's).
const MAX_FILES: usize = super::MAX_LANE_FILES;

/// A branch of the tenant repository and its files.
#[derive(Debug, Clone, Copy)]
pub struct AdoptionCandidate<'a> {
    pub branch: &'a GitRef,
    pub kind: BranchKind,
    /// The tenant that the branch is: the branch name is the tenant id, so the caller has it already.
    pub tenant: &'a TenantId,
    /// The files of the branch at its head: repository path and the normalised hash of the content.
    pub files: &'a [(RepoPath, ContentHash)],
}

/// The Jaccard similarity of two sets, as the fraction `shared / union`. Two empty sets have similarity zero.
///
/// Two similarities are equal when they are the same fraction (`1/2` and `2/4`; any `0/n` is zero), the way they compare, so
/// that equality and order agree. The counts they were made from are `shared` and `union`.
#[derive(Debug, Clone, Copy)]
pub struct Similarity {
    shared: u64,
    union: u64,
}

impl Similarity {
    /// Pairs both sides have.
    pub fn shared(self) -> u64 {
        self.shared
    }

    /// Pairs either side has.
    pub fn union(self) -> u64 {
        self.union
    }

    /// For display only: ranking never uses it.
    #[allow(clippy::cast_precision_loss)]
    pub fn ratio(self) -> f64 {
        if self.union == 0 {
            0.0
        } else {
            self.shared as f64 / self.union as f64
        }
    }
}

impl PartialEq for Similarity {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Similarity {}

impl Ord for Similarity {
    fn cmp(&self, other: &Self) -> Ordering {
        // a/b against c/d: a*d against c*b, where an empty union is the fraction 0/1.
        let (b, d) = (u128::from(self.union.max(1)), u128::from(other.union.max(1)));
        (u128::from(self.shared) * d).cmp(&(u128::from(other.shared) * b))
    }
}

impl PartialOrd for Similarity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// How well one branch fits the swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionScore {
    pub branch: GitRef,
    pub tenant: TenantId,
    pub similarity: Similarity,
}

/// Sorted and without repeats.
fn sorted_pairs(mut pairs: Vec<(NfsPath, ContentHash)>) -> Vec<(NfsPath, ContentHash)> {
    pairs.sort();
    pairs.dedup();
    pairs
}

/// The size of the intersection of two sorted lists without repeats.
fn shared_count(a: &[(NfsPath, ContentHash)], b: &[(NfsPath, ContentHash)]) -> usize {
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    shared
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Ranks the deployable branches among `candidates` by their similarity to `swimlane`, the best first. `swimlane` is the
/// tenant-suffixed files of the swimlane on NFS (path and normalised hash); the caller leaves out the others, which no branch
/// holds. The result lists every deployable candidate (a candidate with similarity zero last) and none of the others.
/// Equal similarities are ordered by branch name. The files of a branch that do not map to a path below the tenant root
/// (a README) are not part of it. Call it from a blocking context.
///
/// # Errors
///
/// `TooManyCandidates` for more than [`MAX_ADOPTION_CANDIDATES`] candidates and `TooManyFiles` for a list of files over the
/// limit of a swimlane.
pub fn adoption_rank(
    pool: &ThreadPool,
    rule: &PathMappingRule,
    swimlane: &[(NfsPath, ContentHash)],
    candidates: &[AdoptionCandidate<'_>],
) -> Result<Vec<AdoptionScore>, BatchError> {
    if candidates.len() > MAX_ADOPTION_CANDIDATES {
        return Err(BatchError::TooManyCandidates);
    }
    if swimlane.len() > MAX_FILES || candidates.iter().any(|c| c.files.len() > MAX_FILES) {
        return Err(BatchError::TooManyFiles);
    }
    let ours = sorted_pairs(swimlane.to_vec());
    let deployable: Vec<&AdoptionCandidate<'_>> = candidates
        .iter()
        .filter(|c| c.kind == BranchKind::Deployable)
        .collect();
    let mut scores: Vec<AdoptionScore> = pool.install(|| {
        deployable
            .par_iter()
            .map(|candidate| {
                let source = RepoSource::Tenant(candidate.tenant.clone());
                let theirs = sorted_pairs(
                    candidate
                        .files
                        .iter()
                        .filter_map(|(path, hash)| Some((map_path(rule, &source, path).ok()?, *hash)))
                        .collect(),
                );
                let shared = shared_count(&ours, &theirs);
                AdoptionScore {
                    branch: candidate.branch.clone(),
                    tenant: candidate.tenant.clone(),
                    similarity: Similarity {
                        shared: as_u64(shared),
                        union: as_u64(ours.len() + theirs.len() - shared),
                    },
                }
            })
            .collect()
    });
    scores.sort_by(|a, b| {
        b.similarity
            .cmp(&a.similarity)
            .then_with(|| a.branch.cmp(&b.branch))
            .then_with(|| a.tenant.cmp(&b.tenant))
    });
    Ok(scores)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fraction(shared: u64, union: u64) -> Similarity {
        Similarity { shared, union }
    }

    /// `Eq` and `Ord` say the same thing: two similarities are equal exactly when they are the same fraction, whatever the
    /// counts they were made from, so that a sorted list of them, or a set, can never put two equal ones apart.
    #[test]
    fn equal_fractions_are_equal_and_compare_equal() {
        let (half, two_quarters, third) = (fraction(1, 2), fraction(2, 4), fraction(1, 3));
        assert_eq!(half, two_quarters);
        assert_eq!(half.cmp(&two_quarters), Ordering::Equal);
        assert_ne!(half, third);
        assert_eq!(half.cmp(&third), Ordering::Greater);
        // Nothing in common is zero whatever the size of the union, and an empty union is zero too.
        assert_eq!(fraction(0, 5), fraction(0, 9));
        assert_eq!(fraction(0, 0), fraction(0, 7));
        assert_ne!(fraction(0, 0), fraction(1, 7));
        // The whole is one.
        assert_eq!(fraction(3, 3), fraction(10, 10));
    }

    #[test]
    fn eq_agrees_with_ord_for_every_pair() {
        let all: Vec<Similarity> = (0..=6)
            .flat_map(|union| (0..=union).map(move |shared| fraction(shared, union)))
            .collect();
        for a in &all {
            for b in &all {
                assert_eq!(a == b, a.cmp(b) == Ordering::Equal, "{a:?} {b:?}");
                assert_eq!(a.cmp(b), b.cmp(a).reverse(), "{a:?} {b:?}");
            }
        }
    }
}
