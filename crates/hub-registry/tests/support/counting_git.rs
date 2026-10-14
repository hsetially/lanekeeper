//! A `GitReader` that counts what it is asked, over the in-memory `FakeGit`: a test proves "once" by the count of reads.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bytes::Bytes;
use domain::{
    BranchInfo, CommitId, CommitInfo, PathChange, RepoKind, RepoPath, TagInfo, TreeIndex, VersionLabel,
};
use ports::fakes::FakeGit;
use ports::{GitError, GitReader};

#[derive(Debug, Clone)]
pub struct CountingGit {
    inner: FakeGit,
    reads: Arc<AtomicU64>,
    diffs: Arc<AtomicU64>,
    failing: Arc<AtomicBool>,
}

impl CountingGit {
    pub fn new(inner: FakeGit) -> Self {
        Self {
            inner,
            reads: Arc::new(AtomicU64::new(0)),
            diffs: Arc::new(AtomicU64::new(0)),
            failing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Make every call fail as a mirror that cannot be read does.
    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), GitError> {
        if self.failing.load(Ordering::SeqCst) {
            Err(GitError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub fn git(&self) -> &FakeGit {
        &self.inner
    }

    /// Calls to `read`: each one is a blob taken out of the repository.
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }

    /// Start counting reads again from zero.
    pub fn reset_reads(&self) {
        self.reads.store(0, Ordering::SeqCst);
    }

    pub fn diffs(&self) -> u64 {
        self.diffs.load(Ordering::SeqCst)
    }
}

impl GitReader for CountingGit {
    fn head(&self, r: RepoKind, branch: &str) -> Result<CommitId, GitError> {
        self.inner.head(r, branch)
    }

    fn branches(&self, r: RepoKind) -> Result<Vec<BranchInfo>, GitError> {
        self.check()?;
        self.inner.branches(r)
    }

    fn tags(&self, r: RepoKind) -> Result<Vec<TagInfo>, GitError> {
        self.inner.tags(r)
    }

    fn tree_index(&self, r: RepoKind, c: &CommitId) -> Result<Arc<TreeIndex>, GitError> {
        self.inner.tree_index(r, c)
    }

    fn diff_trees(&self, r: RepoKind, from: &CommitId, to: &CommitId) -> Result<Vec<PathChange>, GitError> {
        self.diffs.fetch_add(1, Ordering::SeqCst);
        self.check()?;
        self.inner.diff_trees(r, from, to)
    }

    fn read(&self, r: RepoKind, c: &CommitId, path: &RepoPath) -> Result<Option<Bytes>, GitError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.check()?;
        self.inner.read(r, c, path)
    }

    fn history(
        &self,
        r: RepoKind,
        branch: &str,
        path: &RepoPath,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, GitError> {
        self.inner.history(r, branch, path, limit)
    }

    fn describe(&self, r: RepoKind, c: &CommitId) -> Result<VersionLabel, GitError> {
        self.inner.describe(r, c)
    }
}
