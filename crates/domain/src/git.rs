//! Types returned by the `GitReader` port.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CommitId, ContentHash, GitRef, RepoKind, RepoPath, ShortText, Timestamp};

/// How a tenant-repo branch is used (domain-model, "Repos and branches").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchKind {
    /// The base repo's single default branch.
    Default,
    /// A swimlane candidate (`sit1` ... `sitN`).
    Deployable,
    /// `templates/*`, never deployed.
    Template,
    /// `templates/common-docs`, a docs source.
    CommonDocs,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: GitRef,
    pub head: CommitId,
    pub kind: BranchKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: GitRef,
    pub commit: CommitId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub id: CommitId,
    pub author: ShortText,
    pub time: Timestamp,
    pub summary: ShortText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One path that differs between two commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathChange {
    pub path: RepoPath,
    pub kind: PathChangeKind,
}

/// One row of the Git tree index: a path at a commit, with its object id and SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: RepoPath,
    pub git_oid: CommitId,
    pub sha256: ContentHash,
    pub size: u64,
}

/// The index of one commit (D-performance: built once per commit). Entries are sorted by path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeIndex {
    pub repo: RepoKind,
    pub commit: CommitId,
    entries: Vec<TreeEntry>,
}

impl TreeIndex {
    /// Sorts the entries so lookups can binary-search. Later duplicates of a path are dropped.
    pub fn new(repo: RepoKind, commit: CommitId, mut entries: Vec<TreeEntry>) -> Self {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);
        Self {
            repo,
            commit,
            entries,
        }
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    pub fn get(&self, path: &RepoPath) -> Option<&TreeEntry> {
        self.entries
            .binary_search_by(|e| e.path.cmp(path))
            .ok()
            .map(|i| &self.entries[i])
    }
}

/// The nearest tag at or before a commit, plus the distance: `v2.3.1 + 4 commits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionLabel {
    pub tag: Option<GitRef>,
    pub distance: u32,
    pub commit: CommitId,
}

impl fmt::Display for VersionLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.tag, self.distance) {
            (None, _) => f.write_str(self.commit.short()),
            (Some(tag), 0) => write!(f, "{tag}"),
            (Some(tag), 1) => write!(f, "{tag} + 1 commit"),
            (Some(tag), n) => write!(f, "{tag} + {n} commits"),
        }
    }
}
