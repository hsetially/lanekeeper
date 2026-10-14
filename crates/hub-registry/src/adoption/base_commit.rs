//! The best-matching base commit (T5, D19, D22): which commit of the base repository was deployed to a swimlane?
//!
//! A swimlane's base files (the ones with no tenant suffix) were put there by a sync of some version of the base repository, which is
//! usually not today's head. The commit whose files agree most with NFS is the one the baselines of the base files are taken from, so
//! that a file that equals Git at that commit is `in_sync` and one that Git has changed since is `git_ahead` (the sync of the newer
//! commit has not happened yet), which is the truth and which a baseline at today's head could not say.
//!
//! **Without re-reading Git.** A commit is scored from its tree index (path and SHA-256 of every file, `GitReader::tree_index`, built once
//! per commit by the platform), against the swimlane's own raw hashes. Agreement is the same Jaccard fraction the branch ranking uses
//! (pairs both have over pairs either has, compared as exact fractions).
//!
//! **Candidates.** Scoring every commit of a repository is not an option, so the candidates are the ones that can be the answer:
//!
//! 1. the head of the default branch (the most recent, first so that it wins a tie: nothing changed in what the swimlane has between
//!    two commits that score alike, and the newer one is the better label);
//! 2. the commits that changed the files that differ from the head (`GitReader::history` of up to `adoption_history_paths` of them,
//!    spread over the list, `adoption_history_depth` commits each), newest first: the commit NFS was synced from changed some of them;
//! 3. the tagged commits, because a sync deploys a release (`VersionLabel`: "v2.3.1 + 4 commits").
//!
//! 4. a second look, when the best fit is not exact: the files that changed between the best older candidate and the head and still
//!    have on NFS the content the head has were deployed by some commit in between, which is the newest commit that touched them
//!    (`history` of a few of them, one commit each). The commit the sync deployed usually touched such a file, and this finds it
//!    when no tag does and when it changed none of the files that differ from the head.
//!
//! The list is cut at `adoption_base_candidates`. The best candidate is the answer and the others are not looked at again.
//!
//! # What this cannot do
//!
//! The port has no list of commits and no parent of a commit, so the search finds a commit through the files it changed. A deployed
//! commit that changed none of the sampled files, and is not tagged and not the head, is not a candidate: the answer is then the best
//! of the others, whose files differ a little from NFS and show as files to resolve. Naming the commit (`choose_base_commit`) is the
//! way out, and a commit list on `GitReader` would make this exact.

use std::collections::{BTreeMap, BTreeSet};

use domain::{
    BranchKind, CommitId, ContentHash, PathChangeKind, PathMappingRule, RepoKind, RepoPath, Timestamp,
    VersionLabel,
};
use ports::GitReader;

use crate::error::RegistryError;

/// What the search is allowed to read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bounds {
    pub candidates: usize,
    pub history_paths: usize,
    pub history_depth: usize,
}

/// The commit that matches best, with its score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaseMatch {
    pub commit: CommitId,
    pub label: VersionLabel,
    pub shared: u64,
    pub union: u64,
}

/// A fraction `shared / union` compared exactly. An empty union is zero.
fn better(a: (u64, u64), b: (u64, u64)) -> bool {
    // a > b  <=>  a.shared * b.union > b.shared * a.union, where an empty union is the fraction 0/1.
    let (au, bu) = (u128::from(a.1.max(1)), u128::from(b.1.max(1)));
    u128::from(a.0) * bu > u128::from(b.0) * au
}

/// `shared` and `union` of a commit's files against the swimlane's.
fn score(
    git: &dyn GitReader,
    commit: &CommitId,
    rule: &PathMappingRule,
    ours: &BTreeMap<RepoPath, ContentHash>,
) -> Result<(u64, u64), RegistryError> {
    let index = git.tree_index(RepoKind::Base, commit)?;
    let (mut theirs, mut shared) = (0_u64, 0_u64);
    for entry in index.entries() {
        // Only the files of the base repository's config tree are the swimlane's (a README at the root is not).
        if rule.base_to_nfs(&entry.path).is_err() {
            continue;
        }
        theirs += 1;
        if ours.get(&entry.path) == Some(&entry.sha256) {
            shared += 1;
        }
    }
    let ours_len = u64::try_from(ours.len()).unwrap_or(u64::MAX);
    Ok((shared, ours_len + theirs - shared))
}

/// Every `n`-th of `items`, at most `want` of them, evenly spread and the same every time.
fn spread<T: Clone>(items: &[T], want: usize) -> Vec<T> {
    if want == 0 || items.is_empty() {
        return Vec::new();
    }
    let step = items.len().div_ceil(want);
    items.iter().step_by(step).take(want).cloned().collect()
}

/// The candidate commits of the first look, in the order they are tried.
fn candidates(
    git: &dyn GitReader,
    branch: &str,
    head: &CommitId,
    head_hashes: &BTreeMap<RepoPath, ContentHash>,
    ours: &BTreeMap<RepoPath, ContentHash>,
    bounds: Bounds,
) -> Result<Vec<CommitId>, RegistryError> {
    let mut order: Vec<CommitId> = vec![head.clone()];
    let mut seen: BTreeSet<CommitId> = BTreeSet::from([head.clone()]);

    // The files that NFS has with another content than the head has (or that the head no longer has).
    let differing: Vec<&RepoPath> = ours
        .iter()
        .filter(|(path, hash)| head_hashes.get(*path) != Some(*hash))
        .map(|(path, _)| path)
        .collect();
    let mut touched: Vec<(Timestamp, CommitId)> = Vec::new();
    for path in spread(&differing, bounds.history_paths) {
        for commit in git.history(RepoKind::Base, branch, path, bounds.history_depth)? {
            touched.push((commit.time, commit.id));
        }
    }
    // Newest first; the id breaks a tie so that the order never depends on how the paths were visited.
    touched.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, id) in touched {
        if seen.insert(id.clone()) {
            order.push(id);
        }
    }
    let mut tags: Vec<CommitId> = git.tags(RepoKind::Base)?.into_iter().map(|t| t.commit).collect();
    tags.sort();
    for id in tags {
        if seen.insert(id.clone()) {
            order.push(id);
        }
    }
    order.truncate(bounds.candidates);
    Ok(order)
}

/// The best commit so far, and the best of those that are not the head.
#[derive(Default)]
struct Best {
    any: Option<(CommitId, (u64, u64))>,
    older: Option<(CommitId, (u64, u64))>,
}

impl Best {
    /// Strictly better only: on a tie the one that was tried first (the more recent) stays.
    fn consider(&mut self, commit: &CommitId, value: (u64, u64), head: &CommitId) {
        if self.any.as_ref().is_none_or(|(_, b)| better(value, *b)) {
            self.any = Some((commit.clone(), value));
        }
        if commit != head && self.older.as_ref().is_none_or(|(_, b)| better(value, *b)) {
            self.older = Some((commit.clone(), value));
        }
    }

    fn is_exact(&self) -> bool {
        self.any
            .as_ref()
            .is_some_and(|(_, (shared, union))| shared == union)
    }
}

/// The commit of the base repository that the swimlane's base files match best. `ours` is the swimlane's base files as repository paths
/// and raw hashes. Synchronous like the mirror: call it inside `spawn_blocking`.
pub(crate) fn best_base_commit(
    git: &dyn GitReader,
    rule: &PathMappingRule,
    ours: &[(RepoPath, ContentHash)],
    bounds: Bounds,
) -> Result<BaseMatch, RegistryError> {
    let branches = git.branches(RepoKind::Base)?;
    let default = branches
        .iter()
        .find(|b| b.kind == BranchKind::Default)
        .or_else(|| branches.first())
        .ok_or(RegistryError::Git(ports::GitError::NotFound))?;
    let ours: BTreeMap<RepoPath, ContentHash> = ours.iter().cloned().collect();
    let head_index = git.tree_index(RepoKind::Base, &default.head)?;
    let head_hashes: BTreeMap<RepoPath, ContentHash> = head_index
        .entries()
        .iter()
        .map(|e| (e.path.clone(), e.sha256))
        .collect();
    let order = candidates(
        git,
        default.name.as_str(),
        &default.head,
        &head_hashes,
        &ours,
        bounds,
    )?;
    let mut tried: BTreeSet<CommitId> = order.iter().cloned().collect();
    let mut best = Best::default();
    for commit in &order {
        best.consider(commit, score(git, commit, rule, &ours)?, &default.head);
    }

    // The second look (see the module's list): only when the fit is not exact, and from the best older candidate.
    if !best.is_exact() && tried.len() < bounds.candidates {
        if let Some((older, _)) = best.older.clone() {
            let q: Vec<RepoPath> = git
                .diff_trees(RepoKind::Base, &older, &default.head)?
                .into_iter()
                .filter(|change| change.kind != PathChangeKind::Deleted)
                .map(|change| change.path)
                .filter(|path| ours.contains_key(path) && ours.get(path) == head_hashes.get(path))
                .collect();
            for path in spread(&q, bounds.history_paths) {
                if tried.len() >= bounds.candidates {
                    break;
                }
                let newest = git.history(RepoKind::Base, default.name.as_str(), &path, 1)?;
                if let Some(commit) = newest.first() {
                    if tried.insert(commit.id.clone()) {
                        best.consider(&commit.id, score(git, &commit.id, rule, &ours)?, &default.head);
                    }
                }
            }
        }
    }

    let (commit, (shared, union)) = best.any.ok_or(RegistryError::Git(ports::GitError::NotFound))?;
    let label = git.describe(RepoKind::Base, &commit)?;
    Ok(BaseMatch {
        commit,
        label,
        shared,
        union,
    })
}

#[cfg(test)]
mod tests {
    use ports::content_hash;
    use ports::fakes::FakeGit;

    use super::*;

    const BOUNDS: Bounds = Bounds {
        candidates: 64,
        history_paths: 16,
        history_depth: 32,
    };

    fn root() -> PathMappingRule {
        PathMappingRule::default()
    }

    fn ours(files: &[(&str, &str)]) -> Vec<(RepoPath, ContentHash)> {
        files
            .iter()
            .map(|(p, c)| {
                (
                    RepoPath::parse(&format!("config/{p}")).unwrap(),
                    content_hash(c.as_bytes()),
                )
            })
            .collect()
    }

    /// Four commits on master: c1 (a1, b1), c2 (a2, b1), c3 (a2, b2) and c4 (a3, b2, extra). Tags: v1 on c1 and v2 on c2.
    fn repo() -> (FakeGit, Vec<CommitId>) {
        let git = FakeGit::new();
        let commit = |changes: &[(&str, Option<&[u8]>)], msg: &str| {
            git.commit(RepoKind::Base, "master", changes, msg).unwrap()
        };
        let c1 = commit(
            &[
                ("config/app/a.yml", Some(b"a: 1\n")),
                ("config/app/b.yml", Some(b"b: 1\n")),
            ],
            "v1",
        );
        git.tag(RepoKind::Base, "v1.0.0", &c1).unwrap();
        let c2 = commit(&[("config/app/a.yml", Some(b"a: 2\n"))], "v2");
        git.tag(RepoKind::Base, "v2.0.0", &c2).unwrap();
        let c3 = commit(&[("config/app/b.yml", Some(b"b: 2\n"))], "c3");
        let c4 = commit(
            &[
                ("config/app/a.yml", Some(b"a: 3\n")),
                ("config/app/extra.yml", Some(b"e: 1\n")),
            ],
            "c4",
        );
        (git, vec![c1, c2, c3, c4])
    }

    #[test]
    fn a_fraction_is_compared_exactly_and_an_empty_union_is_zero() {
        assert!(better((2, 3), (1, 2)));
        assert!(!better((1, 2), (2, 4)), "equal fractions are not better");
        assert!(!better((0, 0), (0, 5)));
        assert!(better((1, 7), (0, 0)));
    }

    #[test]
    fn the_commit_whose_files_agree_most_is_the_one() {
        let (git, c) = repo();
        // NFS has a2 and b1: exactly the tree of v2.
        let found = best_base_commit(
            &git,
            &root(),
            &ours(&[("app/a.yml", "a: 2\n"), ("app/b.yml", "b: 1\n")]),
            BOUNDS,
        )
        .unwrap();
        assert_eq!(found.commit, c[1]);
        assert_eq!((found.shared, found.union), (2, 2));
        assert_eq!(
            found.label.tag.as_ref().map(domain::GitRef::as_str),
            Some("v2.0.0")
        );
        assert_eq!(found.label.distance, 0);

        // NFS has a1 and b1: v1.
        let found = best_base_commit(
            &git,
            &root(),
            &ours(&[("app/a.yml", "a: 1\n"), ("app/b.yml", "b: 1\n")]),
            BOUNDS,
        )
        .unwrap();
        assert_eq!(found.commit, c[0]);
        assert_eq!(
            found.label.tag.as_ref().map(domain::GitRef::as_str),
            Some("v1.0.0")
        );
    }

    #[test]
    fn a_commit_that_is_not_tagged_and_not_the_head_is_found_by_the_second_look() {
        let (git, c) = repo();
        // c3 (a2, b2) is neither tagged nor the head, and it changed only `b.yml`, which still has on NFS what the head has. The first
        // look finds c2 (a2, b1) as the best of the others; the files that changed between c2 and the head and still match NFS are
        // `b.yml`, and the newest commit that touched it is c3.
        let nfs = ours(&[("app/a.yml", "a: 2\n"), ("app/b.yml", "b: 2\n")]);
        let found = best_base_commit(&git, &root(), &nfs, BOUNDS).unwrap();
        assert_eq!(found.commit, c[2]);
        assert_eq!((found.shared, found.union), (2, 2), "an exact fit");
        assert_eq!(found.label.to_string(), "v2.0.0 + 1 commit");

        // With no room for the second look it stays with the best of the first.
        let first_look_only = Bounds {
            candidates: 3,
            ..BOUNDS
        };
        let found = best_base_commit(&git, &root(), &nfs, first_look_only).unwrap();
        assert_eq!(
            found.commit, c[1],
            "head, then the history of a.yml (c2, c1): c3 is not among them"
        );
    }

    #[test]
    fn a_commit_that_changed_a_file_that_differs_from_the_head_is_found_by_its_history() {
        let (git, c) = repo();
        // NFS has a1 and b2, a tree that no commit has. `a.yml` differs from the head, and its history is c4, c2, c1, so c1 (a1, b1) is
        // a candidate and shares one file; the head shares `b.yml` but has two files that NFS lacks. The second look finds c3 (a2, b2)
        // through `b.yml`, which shares one file as well: a tie that c1, tried first, keeps.
        let nfs = ours(&[("app/a.yml", "a: 1\n"), ("app/b.yml", "b: 2\n")]);
        let found = best_base_commit(&git, &root(), &nfs, BOUNDS).unwrap();
        assert_eq!(found.commit, c[0]);
        assert_eq!((found.shared, found.union), (1, 3));
        assert_eq!(found.label.to_string(), "v1.0.0");
    }

    #[test]
    fn a_tie_goes_to_the_candidate_that_was_tried_first() {
        let (git, c) = repo();
        // Nothing in common with any commit: every score is zero and the head, tried first, stays.
        let found = best_base_commit(&git, &root(), &ours(&[("app/zzz.yml", "z\n")]), BOUNDS).unwrap();
        assert_eq!(found.commit, c[3]);
        assert_eq!(found.shared, 0);
        // A swimlane with no base files at all.
        assert_eq!(best_base_commit(&git, &root(), &[], BOUNDS).unwrap().commit, c[3]);
        // `b: 1` is in c1 and c2 alike. The list is the head, the commits that changed `b.yml` (c3, c1) and then the tags (c1, c2), so c1
        // is tried before c2 and keeps the tie.
        let found = best_base_commit(&git, &root(), &ours(&[("app/b.yml", "b: 1\n")]), BOUNDS).unwrap();
        assert_eq!(found.commit, c[0]);
        assert_eq!((found.shared, found.union), (1, 2));
    }

    #[test]
    fn the_search_is_bounded_by_the_candidate_count() {
        let (git, c) = repo();
        let nfs = ours(&[("app/a.yml", "a: 1\n"), ("app/b.yml", "b: 1\n")]);
        let one = Bounds {
            candidates: 1,
            ..BOUNDS
        };
        assert_eq!(
            best_base_commit(&git, &root(), &nfs, one).unwrap().commit,
            c[3],
            "with room for the head only, the head is the answer, however poor"
        );
        let no_history = Bounds {
            history_paths: 0,
            ..BOUNDS
        };
        // Without the history the tags are still tried, so v1 is found.
        assert_eq!(
            best_base_commit(&git, &root(), &nfs, no_history).unwrap().commit,
            c[0]
        );
    }

    #[test]
    fn the_paths_whose_history_is_read_are_spread_over_the_list() {
        let items: Vec<u32> = (0..100).collect();
        let picked = spread(&items, 10);
        assert_eq!(picked.len(), 10);
        assert_eq!(picked[0], 0);
        assert!(picked.last().is_some_and(|l| *l >= 90), "{picked:?}");
        assert_eq!(spread(&items, 1000).len(), 100, "no more than there are");
        assert!(spread(&items, 0).is_empty());
        assert!(spread::<u32>(&[], 5).is_empty());
        assert_eq!(spread(&items, 10), spread(&items, 10), "the same every time");
    }

    #[test]
    fn a_repository_without_a_branch_is_an_error_and_not_a_guess() {
        let git = FakeGit::new();
        assert_eq!(
            best_base_commit(&git, &root(), &[], BOUNDS),
            Err(RegistryError::Git(ports::GitError::NotFound))
        );
    }

    mod property {
        use proptest::prelude::*;

        use super::*;

        const FILES: usize = 5;

        /// A history of full trees (each commit gives every file a value) and the NFS content of one of its commits.
        fn history() -> impl Strategy<Value = (Vec<[u8; FILES]>, usize, Vec<bool>)> {
            (2_usize..8).prop_flat_map(|commits| {
                (
                    prop::collection::vec(prop::array::uniform5(0_u8..3), commits),
                    0..commits,
                    prop::collection::vec(any::<bool>(), commits),
                )
            })
        }

        fn name(i: usize) -> String {
            format!("config/app/f{i}.yml")
        }

        proptest! {
            /// Whatever the history, the answer is never a worse fit than the head, and when NFS has exactly what the head or a tagged
            /// commit has the answer is an exact fit: the cases the search promises. It is the same answer every time.
            #[test]
            fn the_answer_is_never_worse_than_the_head_and_exact_for_the_head_and_the_tags((trees, deployed, tagged) in history()) {
                let git = FakeGit::new();
                let mut ids = Vec::new();
                for (n, tree) in trees.iter().enumerate() {
                    let contents: Vec<String> = tree.iter().map(|v| format!("v{v}\n")).collect();
                    let changes: Vec<(String, Option<&[u8]>)> = (0..FILES)
                        .map(|i| (name(i), Some(contents[i].as_bytes())))
                        .collect();
                    let changes: Vec<(&str, Option<&[u8]>)> = changes.iter().map(|(p, c)| (p.as_str(), *c)).collect();
                    let id = git.commit(RepoKind::Base, "master", &changes, &format!("c{n}")).unwrap();
                    if tagged[n] {
                        git.tag(RepoKind::Base, &format!("v{n}.0.0"), &id).unwrap();
                    }
                    ids.push(id);
                }
                let nfs: Vec<(RepoPath, ContentHash)> = (0..FILES)
                    .map(|i| (RepoPath::parse(&name(i)).unwrap(), content_hash(format!("v{}\n", trees[deployed][i]).as_bytes())))
                    .collect();
                let rule = PathMappingRule::default();
                let found = best_base_commit(&git, &rule, &nfs, BOUNDS).unwrap();
                let ours: BTreeMap<RepoPath, ContentHash> = nfs.iter().cloned().collect();
                let head = ids.last().unwrap();
                let (hs, hu) = score(&git, head, &rule, &ours).unwrap();
                prop_assert!(!better((hs, hu), (found.shared, found.union)), "the head fits better than the answer");
                // The score the answer reports is the score of the commit it names.
                prop_assert_eq!(score(&git, &found.commit, &rule, &ours).unwrap(), (found.shared, found.union));
                let is_head = deployed == trees.len() - 1;
                if is_head || tagged[deployed] {
                    prop_assert_eq!(found.shared, found.union, "NFS is exactly the tree of the head or of a tagged commit");
                }
                prop_assert_eq!(best_base_commit(&git, &rule, &nfs, BOUNDS).unwrap(), found);
            }
        }
    }
}
