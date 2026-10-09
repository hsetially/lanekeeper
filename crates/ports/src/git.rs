//! The Git reader port (synchronous: call it inside `spawn_blocking`).

use std::sync::Arc;

use bytes::Bytes;
use domain::{
    BranchInfo, CommitId, CommitInfo, PathChange, RepoKind, RepoPath, TagInfo, TreeIndex, VersionLabel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("branch, tag or commit not found")]
    NotFound,
    /// A branch name that is not a valid ref.
    #[error("ref is not valid")]
    InvalidRef,
    #[error("repository is unavailable")]
    Unavailable,
}

/// Read access to the mirrored base and tenant repositories. All results are about commits that already
/// exist locally; fetching is the mirror's job (03b).
pub trait GitReader: Send + Sync + 'static {
    fn head(&self, r: RepoKind, branch: &str) -> Result<CommitId, GitError>;
    fn branches(&self, r: RepoKind) -> Result<Vec<BranchInfo>, GitError>;
    fn tags(&self, r: RepoKind) -> Result<Vec<TagInfo>, GitError>;
    /// The index of every file at `c`, built once per commit and cached.
    fn tree_index(&self, r: RepoKind, c: &CommitId) -> Result<Arc<TreeIndex>, GitError>;
    /// Paths that differ between two commits, sorted by path.
    fn diff_trees(&self, r: RepoKind, from: &CommitId, to: &CommitId) -> Result<Vec<PathChange>, GitError>;
    /// `Ok(None)` when the commit exists but the path does not.
    fn read(&self, r: RepoKind, c: &CommitId, path: &RepoPath) -> Result<Option<Bytes>, GitError>;
    /// Commits that touched `path`, newest first, at most `limit`.
    fn history(
        &self,
        r: RepoKind,
        branch: &str,
        path: &RepoPath,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, GitError>;
    /// The nearest tag at or before `c`, and the distance.
    fn describe(&self, r: RepoKind, c: &CommitId) -> Result<VersionLabel, GitError>;
}
