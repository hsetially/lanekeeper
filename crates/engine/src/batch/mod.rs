//! Batch functions (T8, D22, P5, P6): the engine's functions applied to many swimlanes at once, and the kernel of the
//! incremental recompute.
//!
//! - [`recompute_all`] is the nightly full recompute (P6): for every file of every swimlane the drift state, for every logical
//!   file the effective hash, for every directory the tree hash, and the findings of checks C1 to C8;
//! - [`recompute_paths`] is the event-driven kernel (P5): after some paths changed, it brings the earlier [`LaneState`] of each
//!   affected swimlane up to date, touching only what the paths can have changed;
//! - [`findings_for_swimlanes`] is the findings without the hashes, for a caller that only wants those;
//! - [`adoption_rank`] ranks the deployable branches by how much of a swimlane's tenant files they hold (D22).
//!
//! # Calling them
//!
//! Every batch function runs on the rayon pool it is given, never on the global one, and blocks until it is done: call them
//! from a blocking context (`spawn_blocking`), never from async code. The pool is the caller's way of saying how many cores the
//! work may use; the answer does not depend on it.
//!
//! # What they are given
//!
//! The engine reads no file and no repository. A swimlane is a [`LaneInput`]: its tenants, the environment tier for C6 and
//! its files as the caller read them (a [`LaneFile`]: the path and what it is, the class, the content on NFS, the hash of the
//! file in Git, the baseline and the marks). Nothing in it is parsed: the batch parses each distinct blob once, however
//! many swimlanes hold it, on the pool, and drops the parsed form when it returns (a parsed copy of every blob of a design-scale
//! hub is far more than a replica may hold; P11).
//!
//! # What a swimlane's state is
//!
//! A [`LaneState`] is small and complete: a [`FileState`] for every path (the normalised hash and the drift state), the
//! effective hash of every logical file that some tenant of the swimlane reads, the hash of every directory of that tree, and
//! the findings. `recompute_paths` takes one and gives the next; the result equals what `recompute_all` would give on the new
//! files (tests/batch.rs holds the two together).
//!
//! # Groups
//!
//! Everything a path can change is inside its **logical file** (the base file and the files of the tenants, which the checks
//! already compare with each other), except that a root `application.*` file is a layer of every `spring_merge` file. The
//! kernel recomputes the groups of the paths it is told about, and all `spring_merge` groups when one of those paths could be
//! the root file. A group's effective hash and findings do not depend on any other group, so the order in which groups and
//! swimlanes are done cannot matter, and results are collected in order, never pushed into shared state (determinism).
//!
//! # Limits
//!
//! At most [`MAX_BATCH_SWIMLANES`] swimlanes in a call and [`MAX_LANE_FILES`] files in a swimlane. Inputs are checked
//! before any work starts, so an error does not depend on how the work was spread over threads.

mod adoption;
mod lane;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use domain::{ContentHash, FileClass, LogicalFile, NfsPath, ShortText, SwimlaneId, TenantSet};
use rayon::ThreadPool;
use rayon::prelude::*;

pub use self::adoption::{
    AdoptionCandidate, AdoptionScore, MAX_ADOPTION_CANDIDATES, Similarity, adoption_rank,
};
use self::lane::{Blobs, compute_lane, update_lane};
use crate::checks::{BaseVersion, CheckResult, HostRules, Overwrite};
use crate::drift::{Baseline, Drift, IntentionalMark};
use crate::error::BatchError;
use crate::limits::Limits;
use crate::paths::{FileFacts, MergeRules};
use crate::tree_hash::{IncrementalTree, TreeHashes};

/// The most swimlanes in one batch call.
pub const MAX_BATCH_SWIMLANES: usize = 1_000;
/// The most files in one swimlane.
pub const MAX_LANE_FILES: usize = 200_000;

/// What the batch needs besides the swimlanes. Changing any of it changes every result, so a caller that does must recompute
/// everything and not only the paths that changed.
#[derive(Debug, Clone, Copy)]
pub struct BatchRules<'a> {
    /// Which logical file is `whole_file` and which `spring_merge` (interim, until T10).
    pub merge_rules: &'a MergeRules,
    /// C6.
    pub host_rules: &'a HostRules,
    pub limits: &'a Limits,
}

/// The content of a file on NFS.
#[derive(Clone)]
pub struct NfsContent {
    /// SHA-256 of `bytes`, exactly as they are (the identity of the blob). Files with the same value are parsed once, so the
    /// value must be right: the batch does not check it.
    pub blob: ContentHash,
    pub bytes: Bytes,
}

impl fmt::Debug for NfsContent {
    // The content may hold secrets: only its size is printed (S13, S21).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NfsContent")
            .field("blob", &self.blob)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// What C1 needs to know about the base file's history, owned so that it can be kept with the file (`BaseHistory` borrows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneHistory {
    /// The time of the tenant file's last commit, on the tenant's branch.
    pub tenant_commit_time: domain::Timestamp,
    /// The base file as it was when the tenant file was last updated.
    pub at_tenant_commit: Option<BaseVersion>,
    /// The versions of the base file after that time, in any order.
    pub since: Vec<BaseVersion>,
}

/// One path of a swimlane, as the caller read it.
#[derive(Debug, Clone)]
pub struct LaneFile {
    /// Where the file is on NFS.
    pub path: NfsPath,
    /// What the path is (`classify_path`, against all deployable branches).
    pub facts: FileFacts,
    pub class: FileClass,
    /// N: the file on NFS now, or `None` when it is missing there (a file that Git or the baseline has and NFS lacks: it has a
    /// drift state, and C5 reports it, but no service reads it).
    pub nfs: Option<NfsContent>,
    /// G: the hash of the file in Git, made like the drift hash (`drift_hash`), or `None` when Git has no such file.
    pub git: Option<ContentHash>,
    pub baseline: Baseline,
    pub intentional: Option<IntentionalMark>,
    /// The last change, when a sync job made it (C7).
    pub overwrite: Option<Overwrite>,
    /// For a tenant file in `whole_file` mode: the base file's history (C1).
    pub history: Option<Arc<LaneHistory>>,
}

/// One swimlane: the files to compute from, and what the checks need to know about it.
#[derive(Debug, Clone, Copy)]
pub struct LaneInput<'a> {
    pub swimlane: &'a SwimlaneId,
    /// The tenants the swimlane deploys (Q34).
    pub tenants: &'a TenantSet,
    /// The environment tier of the swimlane (C6). `None` turns C6 off.
    pub tier: Option<&'a ShortText>,
    /// Every path of the swimlane, in any order, each once.
    pub files: &'a [LaneFile],
}

/// What is known about one path after a recompute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub path: NfsPath,
    pub logical: LogicalFile,
    pub class: FileClass,
    /// The hash for comparing (`normalised_hash`), or `None` when the file is missing on NFS.
    pub hash: Option<ContentHash>,
    pub drift: Drift,
}

/// The result of a recompute for one swimlane. Small: it holds hashes and findings, never file content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneState {
    swimlane: SwimlaneId,
    files: BTreeMap<NfsPath, FileState>,
    /// The effective hash of every logical file that some tenant reads (`swimlane_hash` of the tenants' effective hashes).
    effective: BTreeMap<LogicalFile, ContentHash>,
    tree: IncrementalTree,
    /// The findings of each logical file's group; a group with none is not listed.
    findings: BTreeMap<LogicalFile, Vec<CheckResult>>,
}

impl LaneState {
    fn empty(swimlane: SwimlaneId) -> Self {
        Self {
            swimlane,
            files: BTreeMap::new(),
            effective: BTreeMap::new(),
            tree: IncrementalTree::default(),
            findings: BTreeMap::new(),
        }
    }

    pub fn swimlane(&self) -> &SwimlaneId {
        &self.swimlane
    }

    /// Every path of the swimlane, in path order.
    pub fn files(&self) -> impl ExactSizeIterator<Item = &FileState> {
        self.files.values()
    }

    pub fn file(&self, path: &NfsPath) -> Option<&FileState> {
        self.files.get(path)
    }

    /// The effective hash of a logical file, or `None` when no tenant of the swimlane reads it.
    pub fn effective_hash(&self, file: &LogicalFile) -> Option<ContentHash> {
        self.effective.get(file).copied()
    }

    /// Every logical file that some tenant reads, with its effective hash, in path order. These are the leaves of the tree.
    pub fn effective_hashes(&self) -> impl ExactSizeIterator<Item = (&LogicalFile, ContentHash)> {
        self.effective.iter().map(|(file, hash)| (file, *hash))
    }

    /// The hash of the whole tree of effective files, or `None` for a swimlane that serves nothing. It is the identity of the
    /// swimlane's configuration: two swimlanes with the same root hash serve the same files.
    pub fn tree_root(&self) -> Option<ContentHash> {
        self.tree.root()
    }

    /// The hash of one directory (`NfsPath::root()` for the whole tree), or `None` for one that holds no served file.
    pub fn directory_hash(&self, dir: &NfsPath) -> Option<ContentHash> {
        self.tree.get(dir)
    }

    /// The hash of every directory, as `effective_tree_hash` gives them for [`effective_hashes`](Self::effective_hashes).
    pub fn tree_hashes(&self) -> TreeHashes {
        self.tree.to_hashes()
    }

    /// The findings of checks C1 to C8, sorted by check, path and message like `checks_c1_to_c8` sorts them.
    pub fn findings(&self) -> Vec<CheckResult> {
        let mut all: Vec<CheckResult> = self.findings.values().flatten().cloned().collect();
        sort_findings(&mut all);
        all
    }

    fn apply_group(
        &mut self,
        logical: &LogicalFile,
        effective: Option<ContentHash>,
        findings: Vec<CheckResult>,
    ) -> Result<(), BatchError> {
        match effective {
            Some(hash) => {
                self.effective.insert(logical.clone(), hash);
            }
            None => {
                self.effective.remove(logical);
            }
        }
        self.tree
            .set(logical, effective)
            .map_err(|_| BatchError::InconsistentTree)?;
        if findings.is_empty() {
            self.findings.remove(logical);
        } else {
            self.findings.insert(logical.clone(), findings);
        }
        Ok(())
    }
}

/// The order of `checks_c1_to_c8`.
pub(crate) fn sort_findings(findings: &mut [CheckResult]) {
    findings.sort_by(|a, b| (a.kind, &a.path, &a.message).cmp(&(b.kind, &b.path, &b.message)));
}

/// The findings of one swimlane (C1 to C8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwimlaneFindings {
    pub swimlane: SwimlaneId,
    pub findings: Vec<CheckResult>,
}

/// The first error among results that were computed in parallel, in input order: the same whatever the thread schedule.
fn first_error<T>(results: Vec<Result<T, BatchError>>) -> Result<Vec<T>, BatchError> {
    results.into_iter().collect()
}

/// Checks that make an error independent of the schedule: sizes and duplicate swimlanes, before any parallel work.
fn check_batch(lanes: &[LaneInput<'_>]) -> Result<(), BatchError> {
    if lanes.len() > MAX_BATCH_SWIMLANES {
        return Err(BatchError::TooManySwimlanes);
    }
    if lanes.iter().any(|lane| lane.files.len() > MAX_LANE_FILES) {
        return Err(BatchError::TooManyFiles);
    }
    let mut seen = BTreeSet::new();
    if lanes.iter().any(|lane| !seen.insert(lane.swimlane)) {
        return Err(BatchError::DuplicateSwimlane);
    }
    Ok(())
}

/// The distinct blobs of the files that will be read, parsed once each on the pool.
fn parse_blobs<'a>(pool: &ThreadPool, files: impl Iterator<Item = &'a LaneFile>, limits: &Limits) -> Blobs {
    let mut keys = BTreeMap::new();
    for file in files {
        lane::collect_key(file, &mut keys);
    }
    Blobs::build(Some(pool), keys, limits)
}

/// The full recompute (P6): every swimlane from scratch. The results are in the order of `lanes`.
///
/// Distinct blobs are decoded, hashed and parsed once, in parallel, and then each swimlane is computed in parallel from the
/// shared table. Call it from a blocking context.
///
/// # Errors
///
/// `TooManySwimlanes`, `TooManyFiles`, `DuplicateSwimlane`, `DuplicatePath` for inputs that are not what the function takes,
/// and `InconsistentTree` when the effective files of a swimlane have a name that is both a file and a directory.
pub fn recompute_all(
    pool: &ThreadPool,
    lanes: &[LaneInput<'_>],
    rules: &BatchRules<'_>,
) -> Result<Vec<LaneState>, BatchError> {
    check_batch(lanes)?;
    let blobs = parse_blobs(
        pool,
        lanes.iter().flat_map(|lane| lane.files.iter()),
        rules.limits,
    );
    let results: Vec<Result<LaneState, BatchError>> = pool.install(|| {
        lanes
            .par_iter()
            .map(|lane| compute_lane(lane, rules, &blobs, true))
            .collect()
    });
    first_error(results)
}

/// The findings of checks C1 to C8 for many swimlanes, in the order of `lanes`. Each equals what the same swimlane gives
/// alone; the effective hashes and the tree are not computed.
///
/// # Errors
///
/// As [`recompute_all`].
pub fn findings_for_swimlanes(
    pool: &ThreadPool,
    lanes: &[LaneInput<'_>],
    rules: &BatchRules<'_>,
) -> Result<Vec<SwimlaneFindings>, BatchError> {
    check_batch(lanes)?;
    let blobs = parse_blobs(
        pool,
        lanes.iter().flat_map(|lane| lane.files.iter()),
        rules.limits,
    );
    let results: Vec<Result<SwimlaneFindings, BatchError>> = pool.install(|| {
        lanes
            .par_iter()
            .map(|lane| {
                compute_lane(lane, rules, &blobs, false).map(|state| SwimlaneFindings {
                    swimlane: lane.swimlane.clone(),
                    findings: state.findings(),
                })
            })
            .collect()
    });
    first_error(results)
}

/// The incremental recompute (P5): the next state of each swimlane that has an affected path, from its earlier state, its
/// files now, and the paths that changed. The result has one state per affected swimlane, in swimlane order; the others are
/// untouched and not returned.
///
/// For each affected path it drops or recomputes the path's state, then recomputes the group of its logical file: the
/// effective hash, the tree above it (only the directories on the way up are rehashed), and the findings of the checks that
/// depend on the file. If an affected path could be a root `application.*` file, every `spring_merge` group is recomputed, since
/// the root is a layer of all of them. Paths that are in `lanes` but not in `affected` are taken to be unchanged and are not
/// read again unless they share a group with an affected one. A swimlane's tenants, tier and the rules are taken to be what
/// they were: when they change, the caller recomputes everything.
///
/// Call it from a blocking context.
///
/// # Errors
///
/// As [`recompute_all`], and `UnknownSwimlane` for an affected swimlane that has no state in `previous` or no input in `lanes`.
pub fn recompute_paths(
    pool: &ThreadPool,
    previous: &[LaneState],
    lanes: &[LaneInput<'_>],
    affected: &[(SwimlaneId, NfsPath)],
    rules: &BatchRules<'_>,
) -> Result<Vec<LaneState>, BatchError> {
    check_batch(lanes)?;
    let mut by_swimlane: BTreeMap<&SwimlaneId, BTreeSet<&NfsPath>> = BTreeMap::new();
    for (swimlane, path) in affected {
        by_swimlane.entry(swimlane).or_default().insert(path);
    }
    let states: BTreeMap<&SwimlaneId, &LaneState> = previous.iter().map(|s| (s.swimlane(), s)).collect();
    let inputs: BTreeMap<&SwimlaneId, &LaneInput<'_>> = lanes.iter().map(|l| (l.swimlane, l)).collect();
    let mut work = Vec::with_capacity(by_swimlane.len());
    for (swimlane, paths) in by_swimlane {
        let (Some(state), Some(input)) = (states.get(swimlane), inputs.get(swimlane)) else {
            return Err(BatchError::UnknownSwimlane);
        };
        work.push((*state, *input, paths));
    }
    let results: Vec<Result<LaneState, BatchError>> = pool.install(|| {
        work.par_iter()
            .map(|(state, input, paths)| update_lane(state, input, rules, paths))
            .collect()
    });
    first_error(results)
}
