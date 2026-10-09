use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use domain::{
    BranchInfo, BranchKind, CommitId, CommitInfo, GitRef, PathChange, PathChangeKind, RepoKind, RepoPath,
    ShortText, TagInfo, Timestamp, TreeEntry, TreeIndex, VersionLabel,
};
use sha2::{Digest, Sha256};

use super::util::lock;
use crate::conformance::GitExpectation;
use crate::{GitError, GitReader, content_hash};

/// Most commits the fake holds across both repositories.
const MAX_COMMITS: usize = 10_000;

type Tree = Arc<BTreeMap<RepoPath, Bytes>>;

struct Commit {
    repo: RepoKind,
    parent: Option<CommitId>,
    tree: Tree,
    info: CommitInfo,
}

#[derive(Default)]
struct State {
    commits: HashMap<CommitId, Commit>,
    branches: BTreeMap<(RepoKind, String), CommitId>,
    tags: BTreeMap<(RepoKind, String), CommitId>,
    indexes: HashMap<(RepoKind, CommitId), Arc<TreeIndex>>,
    clock: i64,
}

/// A small in-memory Git: linear history per branch, tags, trees of whole files.
#[derive(Clone, Default)]
pub struct FakeGit {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for FakeGit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeGit").finish_non_exhaustive()
    }
}

fn digest_id(parts: &[&[u8]]) -> Result<CommitId, GitError> {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
        h.update([0]);
    }
    let mut hex = String::with_capacity(64);
    for b in h.finalize() {
        // Writing to a String cannot fail.
        let _ = write!(hex, "{b:02x}");
    }
    CommitId::parse(&hex).map_err(|_| GitError::Unavailable)
}

fn branch_kind(repo: RepoKind, name: &str) -> BranchKind {
    match repo {
        RepoKind::Base => BranchKind::Default,
        RepoKind::Tenant if name == "templates/common-docs" => BranchKind::CommonDocs,
        RepoKind::Tenant if name.starts_with("templates/") => BranchKind::Template,
        RepoKind::Tenant => BranchKind::Deployable,
    }
}

impl FakeGit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Commit `changes` (a path and its new content, or `None` to delete it) on top of `branch`, creating
    /// the branch with an empty tree if it does not exist. Time advances one minute per commit.
    pub fn commit(
        &self,
        repo: RepoKind,
        branch: &str,
        changes: &[(&str, Option<&[u8]>)],
        message: &str,
    ) -> Result<CommitId, GitError> {
        GitRef::parse(branch).map_err(|_| GitError::InvalidRef)?;
        let mut st = lock(&self.state);
        if st.commits.len() >= MAX_COMMITS {
            return Err(GitError::Unavailable);
        }
        let parent = st.branches.get(&(repo, branch.to_owned())).cloned();
        let mut tree: BTreeMap<RepoPath, Bytes> = parent
            .as_ref()
            .and_then(|p| st.commits.get(p))
            .map(|c| (*c.tree).clone())
            .unwrap_or_default();
        for (path, content) in changes {
            let path = RepoPath::parse(path).map_err(|_| GitError::InvalidRef)?;
            match content {
                Some(b) => {
                    tree.insert(path, Bytes::copy_from_slice(b));
                }
                None => {
                    tree.remove(&path);
                }
            }
        }
        st.clock += 60_000;
        let time = Timestamp::from_unix_millis(1_700_000_000_000 + st.clock);
        let mut tree_digest = Sha256::new();
        for (p, b) in &tree {
            tree_digest.update(p.as_str().as_bytes());
            tree_digest.update(content_hash(b).as_bytes());
        }
        let tree_id = tree_digest.finalize();
        let parent_str = parent.as_ref().map_or("", CommitId::as_str);
        let id = digest_id(&[
            b"commit",
            parent_str.as_bytes(),
            &tree_id,
            message.as_bytes(),
            &time.unix_millis().to_be_bytes(),
        ])?;
        let info = CommitInfo {
            id: id.clone(),
            author: ShortText::parse("Fake Author").map_err(|_| GitError::Unavailable)?,
            time,
            summary: ShortText::parse(message).map_err(|_| GitError::Unavailable)?,
        };
        st.commits.insert(
            id.clone(),
            Commit {
                repo,
                parent,
                tree: Arc::new(tree),
                info,
            },
        );
        st.branches.insert((repo, branch.to_owned()), id.clone());
        Ok(id)
    }

    /// Point a tag at a commit.
    pub fn tag(&self, repo: RepoKind, name: &str, commit: &CommitId) -> Result<(), GitError> {
        GitRef::parse(name).map_err(|_| GitError::InvalidRef)?;
        let mut st = lock(&self.state);
        if !st.commits.contains_key(commit) {
            return Err(GitError::NotFound);
        }
        st.tags.insert((repo, name.to_owned()), commit.clone());
        Ok(())
    }

    /// A repository with three commits on a `sit1` branch, a tag on the first, and the matching
    /// expectation for `conformance::git_reader`.
    pub fn with_conformance_repo() -> Result<(Self, GitExpectation), GitError> {
        let git = Self::new();
        let repo = RepoKind::Tenant;
        let first = git.commit(
            repo,
            "sit1",
            &[
                ("app/core.yml", Some(b"a: 1\r\n")),
                ("app/other.yml", Some(b"b: 1\n")),
            ],
            "initial",
        )?;
        git.tag(repo, "v1.0.0", &first)?;
        git.commit(
            repo,
            "sit1",
            &[("app/core.yml", Some(b"a: 2\r\n"))],
            "change core",
        )?;
        git.commit(
            repo,
            "sit1",
            &[("app/core.yml", Some(b"a: 3\r\n")), ("new.txt", Some(b"hi"))],
            "change core again",
        )?;
        let files = [
            ("app/core.yml", &b"a: 3\r\n"[..]),
            ("app/other.yml", b"b: 1\n"),
            ("new.txt", b"hi"),
        ]
        .into_iter()
        .map(|(p, b)| RepoPath::parse(p).map(|p| (p, Bytes::copy_from_slice(b))))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| GitError::InvalidRef)?;
        let expect = GitExpectation {
            repo,
            branch: "sit1".to_owned(),
            files,
            history_path: RepoPath::parse("app/core.yml").map_err(|_| GitError::InvalidRef)?,
            history_commits: 3,
            tag: Some((GitRef::parse("v1.0.0").map_err(|_| GitError::InvalidRef)?, 2)),
        };
        Ok((git, expect))
    }

    fn commit_of<'a>(st: &'a State, repo: RepoKind, c: &CommitId) -> Result<&'a Commit, GitError> {
        st.commits
            .get(c)
            .filter(|x| x.repo == repo)
            .ok_or(GitError::NotFound)
    }
}

impl GitReader for FakeGit {
    fn head(&self, r: RepoKind, branch: &str) -> Result<CommitId, GitError> {
        GitRef::parse(branch).map_err(|_| GitError::InvalidRef)?;
        lock(&self.state)
            .branches
            .get(&(r, branch.to_owned()))
            .cloned()
            .ok_or(GitError::NotFound)
    }

    fn branches(&self, r: RepoKind) -> Result<Vec<BranchInfo>, GitError> {
        lock(&self.state)
            .branches
            .iter()
            .filter(|((repo, _), _)| *repo == r)
            .map(|((_, name), head)| {
                Ok(BranchInfo {
                    name: GitRef::parse(name).map_err(|_| GitError::InvalidRef)?,
                    head: head.clone(),
                    kind: branch_kind(r, name),
                })
            })
            .collect()
    }

    fn tags(&self, r: RepoKind) -> Result<Vec<TagInfo>, GitError> {
        lock(&self.state)
            .tags
            .iter()
            .filter(|((repo, _), _)| *repo == r)
            .map(|((_, name), commit)| {
                Ok(TagInfo {
                    name: GitRef::parse(name).map_err(|_| GitError::InvalidRef)?,
                    commit: commit.clone(),
                })
            })
            .collect()
    }

    fn tree_index(&self, r: RepoKind, c: &CommitId) -> Result<Arc<TreeIndex>, GitError> {
        let mut st = lock(&self.state);
        if let Some(ix) = st.indexes.get(&(r, c.clone())) {
            return Ok(ix.clone());
        }
        let commit = Self::commit_of(&st, r, c)?;
        let entries = commit
            .tree
            .iter()
            .map(|(path, bytes)| {
                Ok(TreeEntry {
                    path: path.clone(),
                    git_oid: digest_id(&[b"blob", bytes])?,
                    sha256: content_hash(bytes),
                    size: bytes.len() as u64,
                })
            })
            .collect::<Result<Vec<_>, GitError>>()?;
        let ix = Arc::new(TreeIndex::new(r, c.clone(), entries));
        st.indexes.insert((r, c.clone()), ix.clone());
        Ok(ix)
    }

    fn diff_trees(&self, r: RepoKind, from: &CommitId, to: &CommitId) -> Result<Vec<PathChange>, GitError> {
        let st = lock(&self.state);
        let a = &Self::commit_of(&st, r, from)?.tree;
        let b = &Self::commit_of(&st, r, to)?.tree;
        let mut out = Vec::new();
        for (path, bytes) in b.iter() {
            match a.get(path) {
                None => out.push(PathChange {
                    path: path.clone(),
                    kind: PathChangeKind::Added,
                }),
                Some(old) if old != bytes => out.push(PathChange {
                    path: path.clone(),
                    kind: PathChangeKind::Modified,
                }),
                Some(_) => {}
            }
        }
        for path in a.keys() {
            if !b.contains_key(path) {
                out.push(PathChange {
                    path: path.clone(),
                    kind: PathChangeKind::Deleted,
                });
            }
        }
        out.sort_by(|x, y| x.path.cmp(&y.path));
        Ok(out)
    }

    fn read(&self, r: RepoKind, c: &CommitId, path: &RepoPath) -> Result<Option<Bytes>, GitError> {
        let st = lock(&self.state);
        Ok(Self::commit_of(&st, r, c)?.tree.get(path).cloned())
    }

    fn history(
        &self,
        r: RepoKind,
        branch: &str,
        path: &RepoPath,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, GitError> {
        GitRef::parse(branch).map_err(|_| GitError::InvalidRef)?;
        let st = lock(&self.state);
        let mut cursor = st
            .branches
            .get(&(r, branch.to_owned()))
            .cloned()
            .ok_or(GitError::NotFound)?;
        let mut out = Vec::new();
        loop {
            if out.len() >= limit {
                break;
            }
            let commit = Self::commit_of(&st, r, &cursor)?;
            let before = commit
                .parent
                .as_ref()
                .and_then(|p| st.commits.get(p))
                .and_then(|p| p.tree.get(path));
            if commit.tree.get(path) != before {
                out.push(commit.info.clone());
            }
            match &commit.parent {
                Some(p) => cursor = p.clone(),
                None => break,
            }
        }
        Ok(out)
    }

    fn describe(&self, r: RepoKind, c: &CommitId) -> Result<VersionLabel, GitError> {
        let st = lock(&self.state);
        Self::commit_of(&st, r, c)?;
        let mut cursor = c.clone();
        let mut distance = 0u32;
        loop {
            // First tag by name when several point at one commit.
            let tag = st
                .tags
                .iter()
                .find(|((repo, _), commit)| *repo == r && **commit == cursor)
                .map(|((_, name), _)| name.clone());
            if let Some(name) = tag {
                return Ok(VersionLabel {
                    tag: Some(GitRef::parse(&name).map_err(|_| GitError::InvalidRef)?),
                    distance,
                    commit: c.clone(),
                });
            }
            match Self::commit_of(&st, r, &cursor)?.parent.clone() {
                Some(p) => {
                    cursor = p;
                    distance += 1;
                }
                None => {
                    return Ok(VersionLabel {
                        tag: None,
                        distance,
                        commit: c.clone(),
                    });
                }
            }
        }
    }
}
