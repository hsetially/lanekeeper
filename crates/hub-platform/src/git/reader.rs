//! `GixReader`: the [`GitReader`] port over the mirrors (T7, P2).
//!
//! The port is synchronous and every call may read packs and hash blobs, so callers run it inside `spawn_blocking` (rule 4). All
//! results are about commits the mirror already has; the one exception is the **read-through fetch**: a commit the mirror lacks is
//! looked for by one fetch (single-flight, shared with the webhook worker and the poll) before it is reported as not found. A commit
//! that one fetch did not bring is remembered as missing for 30 seconds, so a flood of lookups for ids that do not exist costs one
//! fetch and not a fetch each.
//!
//! Only regular files (modes `100644` and `100755`) exist for this reader. A symlink is never followed and never listed (S11,
//! S17: config text must not lead outside the repository), a submodule is not a file, and a change of mode alone is not a change
//! of content. `RepoPath` and `GitRef` are checked before they reach `gix`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use domain::{
    BranchInfo, BranchKind, CommitId, CommitInfo, ContentHash, GitRef, PathChange, PathChangeKind, RepoKind,
    RepoPath, ShortText, TagInfo, Timestamp, TreeEntry, TreeIndex, VersionLabel,
};
use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::tree::EntryKind;
use moka::sync::Cache;
use ports::{GitError, GitReader, content_hash};
use rayon::prelude::*;
use tracing::warn;

use super::metrics::GitMetrics;
use super::mirror::{Mirror, snapshot};
use super::tree_index::TreeIndexCache;

/// How long a commit that a fetch did not bring is remembered as missing.
const MISSING_TTL: Duration = Duration::from_secs(30);
/// How many missing commits are remembered.
const MISSING_CAPACITY: u64 = 10_000;
/// The most commits `history` looks at before it gives up (a path that changed long ago in a very long history).
const MAX_HISTORY_SCAN: usize = 20_000;
/// The largest file the tree index hashes. Config files are kilobytes; a blob over this is skipped, logged, and not listed.
const DEFAULT_MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;

/// Sizes of the reader's caches and pool.
#[derive(Debug, Clone)]
pub struct ReaderSettings {
    /// Index entries the tree-index cache holds in all (about 200 bytes each).
    pub tree_index_entries: u64,
    /// Blob hashes remembered across commits, so a file that did not change is hashed once however many commits hold it.
    pub blob_hash_entries: u64,
    /// Threads of the hashing pool.
    pub threads: usize,
    pub max_blob_bytes: u64,
}

impl Default for ReaderSettings {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);
        Self {
            tree_index_entries: 400_000,
            blob_hash_entries: 262_144,
            threads: cores.clamp(1, 8),
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
        }
    }
}

pub struct GixReader {
    mirror: Arc<Mirror>,
    trees: TreeIndexCache,
    blob_hashes: Cache<gix::ObjectId, (ContentHash, u64)>,
    hashed: AtomicU64,
    missing: Cache<(RepoKind, gix::ObjectId), ()>,
    pool: rayon::ThreadPool,
    max_blob_bytes: u64,
    metrics: GitMetrics,
}

impl std::fmt::Debug for GixReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GixReader").finish_non_exhaustive()
    }
}

fn unavailable(code: &'static str, e: &dyn std::fmt::Display) -> GitError {
    // The message can name a path or an object id and never a secret.
    warn!(code, error = %e, "git read failed");
    GitError::Unavailable
}

fn object_id(c: &CommitId) -> Result<gix::ObjectId, GitError> {
    // A 64-character id is a SHA-256 commit id, which a SHA-1 mirror cannot hold.
    gix::ObjectId::from_hex(c.as_str().as_bytes()).map_err(|_| GitError::NotFound)
}

fn commit_id(id: gix::ObjectId) -> Result<CommitId, GitError> {
    CommitId::parse(&id.to_hex().to_string()).map_err(|_| GitError::Unavailable)
}

fn branch_kind(repo: RepoKind, name: &GitRef) -> BranchKind {
    let name = name.as_str();
    match repo {
        RepoKind::Base => BranchKind::Default,
        RepoKind::Tenant if name == "templates/common-docs" => BranchKind::CommonDocs,
        RepoKind::Tenant if name.starts_with("templates/") => BranchKind::Template,
        RepoKind::Tenant if name.contains('/') => BranchKind::Other,
        RepoKind::Tenant => BranchKind::Deployable,
    }
}

fn is_file(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::Blob | EntryKind::BlobExecutable)
}

fn has_commit(repo: &gix::Repository, id: gix::ObjectId) -> bool {
    repo.find_header(id)
        .is_ok_and(|h| h.kind() == gix::object::Kind::Commit)
}

/// The longest text a `ShortText` holds.
const SHORT_TEXT_BYTES: usize = 256;

/// Text from a commit for a `ShortText`: control characters become spaces, and it is cut at a character boundary.
fn short_text(raw: &BStr) -> Result<ShortText, GitError> {
    let mut text: String = raw
        .to_str_lossy()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    while text.len() > SHORT_TEXT_BYTES {
        text.pop();
    }
    ShortText::parse(text.trim()).map_err(|_| GitError::Unavailable)
}

/// The first line of a commit message: its summary.
fn first_line(text: &BStr) -> &BStr {
    text.split(|b| *b == b'\n').next().unwrap_or_default().as_bstr()
}

/// The root tree of a commit.
fn tree_of(repo: &gix::Repository, id: gix::ObjectId) -> Result<gix::Tree<'_>, GitError> {
    let commit = repo
        .find_commit(id)
        .map_err(|e| unavailable("git_commit_unreadable", &e))?;
    commit.tree().map_err(|e| unavailable("git_tree_unreadable", &e))
}

impl GixReader {
    /// Builds the reader and its hashing pool.
    pub fn new(
        mirror: Arc<Mirror>,
        settings: &ReaderSettings,
        metrics: GitMetrics,
    ) -> Result<Self, GitError> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(settings.threads.max(1))
            .thread_name(|i| format!("git-hash-{i}"))
            .build()
            .map_err(|e| unavailable("git_pool_failed", &e))?;
        Ok(Self {
            mirror,
            trees: TreeIndexCache::new(settings.tree_index_entries, metrics.clone()),
            blob_hashes: Cache::builder()
                .max_capacity(settings.blob_hash_entries.max(1))
                .build(),
            hashed: AtomicU64::new(0),
            missing: Cache::builder()
                .max_capacity(MISSING_CAPACITY)
                .time_to_live(MISSING_TTL)
                .build(),
            pool,
            max_blob_bytes: settings.max_blob_bytes,
            metrics,
        })
    }

    /// The cache of tree indexes, for the job that persists them.
    #[must_use]
    pub fn tree_indexes(&self) -> &TreeIndexCache {
        &self.trees
    }

    /// The mirrors this reader reads.
    #[must_use]
    pub fn mirror(&self) -> &Arc<Mirror> {
        &self.mirror
    }

    /// Blobs hashed since the start (a blob already in the hash cache is not counted).
    #[must_use]
    pub fn blob_hashes_computed(&self) -> u64 {
        self.hashed.load(Ordering::Relaxed)
    }

    /// A handle on the mirror, and the id of `c` in it, after the read-through fetch if the mirror lacked the commit.
    fn locate(&self, r: RepoKind, c: &CommitId) -> Result<(gix::Repository, gix::ObjectId), GitError> {
        let id = object_id(c)?;
        let repo = self.mirror.repository(r);
        if has_commit(&repo, id) {
            return Ok((repo, id));
        }
        if self.missing.contains_key(&(r, id)) {
            return Err(GitError::NotFound);
        }
        self.metrics.readthrough(r);
        // One fetch, shared with every other caller that needs one at the same time.
        self.mirror
            .fetch_blocking(r)
            .map_err(|e| unavailable("git_readthrough_failed", &e))?;
        let repo = self.mirror.repository(r);
        if has_commit(&repo, id) {
            Ok((repo, id))
        } else {
            self.missing.insert((r, id), ());
            Err(GitError::NotFound)
        }
    }

    fn tip(&self, r: RepoKind, branch: &str) -> Result<(gix::Repository, gix::ObjectId), GitError> {
        let name = GitRef::parse(branch).map_err(|_| GitError::InvalidRef)?;
        let repo = self.mirror.repository(r);
        let full = format!("refs/heads/{name}");
        let mut reference = repo
            .try_find_reference(full.as_str())
            .map_err(|e| unavailable("git_ref_unreadable", &e))?
            .ok_or(GitError::NotFound)?;
        let id = reference
            .peel_to_id()
            .map_err(|e| unavailable("git_ref_unreadable", &e))?
            .detach();
        Ok((repo, id))
    }

    /// SHA-256 and size of a blob, from the cache or by reading it. `None` for a blob over the size limit.
    fn blob_hash(
        &self,
        repo: &gix::Repository,
        oid: gix::ObjectId,
    ) -> Result<Option<(ContentHash, u64)>, GitError> {
        if let Some(hit) = self.blob_hashes.get(&oid) {
            return Ok(Some(hit));
        }
        let header = repo
            .find_header(oid)
            .map_err(|e| unavailable("git_blob_unreadable", &e))?;
        if header.size() > self.max_blob_bytes {
            warn!(
                code = "git_blob_too_large",
                "a blob over the size limit is not indexed"
            );
            return Ok(None);
        }
        let blob = repo
            .find_blob(oid)
            .map_err(|e| unavailable("git_blob_unreadable", &e))?;
        let value = (content_hash(&blob.data), blob.data.len() as u64);
        self.hashed.fetch_add(1, Ordering::Relaxed);
        self.blob_hashes.insert(oid, value);
        Ok(Some(value))
    }

    fn build_index(&self, r: RepoKind, c: &CommitId) -> Result<Arc<TreeIndex>, GitError> {
        let (repo, id) = self.locate(r, c)?;
        let tree = tree_of(&repo, id)?;
        let mut recorder = gix::traverse::tree::Recorder::default();
        tree.traverse()
            .breadthfirst(&mut recorder)
            .map_err(|e| unavailable("git_tree_unreadable", &e))?;
        let files: Vec<(RepoPath, gix::ObjectId)> = recorder
            .records
            .into_iter()
            .filter(|e| is_file(e.mode.kind()))
            .filter_map(|e| {
                let path = e.filepath.to_str().ok().and_then(|p| RepoPath::parse(p).ok());
                if path.is_none() {
                    warn!(
                        code = "git_path_skipped",
                        "a path that is not a valid repository path was left out of the index"
                    );
                }
                path.map(|p| (p, e.oid))
            })
            .collect();
        let shared = self.mirror.thread_safe(r);
        let hashed: Result<Vec<Option<TreeEntry>>, GitError> = self.pool.install(|| {
            files
                .par_iter()
                .map_init(
                    || shared.to_thread_local(),
                    |repo, (path, oid)| {
                        let Some((sha256, size)) = self.blob_hash(repo, *oid)? else {
                            return Ok(None);
                        };
                        Ok(Some(TreeEntry {
                            path: path.clone(),
                            git_oid: commit_id(*oid)?,
                            sha256,
                            size,
                        }))
                    },
                )
                .collect()
        });
        let entries = hashed?.into_iter().flatten().collect();
        Ok(Arc::new(TreeIndex::new(r, c.clone(), entries)))
    }
}

// ------------------------------------------------------------------------------------------------------- diff

#[derive(Clone, Copy)]
enum Side {
    Tree(gix::ObjectId),
    File(gix::ObjectId),
}

struct Differ<'r> {
    repo: &'r gix::Repository,
    out: Vec<PathChange>,
}

impl Differ<'_> {
    fn children(&self, tree: Option<gix::ObjectId>) -> Result<BTreeMap<BString, Side>, GitError> {
        let mut map = BTreeMap::new();
        let Some(oid) = tree else { return Ok(map) };
        let tree = self
            .repo
            .find_tree(oid)
            .map_err(|e| unavailable("git_tree_unreadable", &e))?;
        for entry in tree.iter() {
            let entry = entry.map_err(|e| unavailable("git_tree_unreadable", &e))?;
            let kind = entry.mode().kind();
            let side = match kind {
                EntryKind::Tree => Side::Tree(entry.oid().to_owned()),
                k if is_file(k) => Side::File(entry.oid().to_owned()),
                // A symlink or a submodule is not a file of the repository.
                _ => continue,
            };
            map.insert(entry.filename().to_owned(), side);
        }
        Ok(map)
    }

    fn push(&mut self, prefix: &str, name: &BStr, kind: PathChangeKind) {
        let Ok(name) = name.to_str() else { return };
        let full = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if let Ok(path) = RepoPath::parse(&full) {
            self.out.push(PathChange { path, kind });
        }
    }

    /// Everything under `tree` as `kind`.
    fn all(&mut self, prefix: &str, tree: gix::ObjectId, kind: PathChangeKind) -> Result<(), GitError> {
        for (name, side) in self.children(Some(tree))? {
            match side {
                Side::File(_) => self.push(prefix, name.as_ref(), kind),
                Side::Tree(sub) => {
                    let Ok(n) = name.to_str() else { continue };
                    let next = if prefix.is_empty() {
                        n.to_owned()
                    } else {
                        format!("{prefix}/{n}")
                    };
                    self.all(&next, sub, kind)?;
                }
            }
        }
        Ok(())
    }

    fn side(&mut self, prefix: &str, name: &BStr, side: Side, kind: PathChangeKind) -> Result<(), GitError> {
        match side {
            Side::File(_) => {
                self.push(prefix, name, kind);
                Ok(())
            }
            Side::Tree(oid) => {
                let Ok(n) = name.to_str() else { return Ok(()) };
                let next = if prefix.is_empty() {
                    n.to_owned()
                } else {
                    format!("{prefix}/{n}")
                };
                self.all(&next, oid, kind)
            }
        }
    }

    fn diff(
        &mut self,
        prefix: &str,
        old: Option<gix::ObjectId>,
        new: Option<gix::ObjectId>,
    ) -> Result<(), GitError> {
        if old == new {
            return Ok(());
        }
        let (left, right) = (self.children(old)?, self.children(new)?);
        let names: std::collections::BTreeSet<&BString> = left.keys().chain(right.keys()).collect();
        for name in names {
            let entry: &BStr = name.as_ref();
            match (left.get(name).copied(), right.get(name).copied()) {
                (Some(Side::Tree(before)), Some(Side::Tree(after))) => {
                    if before != after {
                        let Ok(text) = entry.to_str() else { continue };
                        let next = if prefix.is_empty() {
                            text.to_owned()
                        } else {
                            format!("{prefix}/{text}")
                        };
                        self.diff(&next, Some(before), Some(after))?;
                    }
                }
                (Some(Side::File(before)), Some(Side::File(after))) => {
                    if before != after {
                        self.push(prefix, entry, PathChangeKind::Modified);
                    }
                }
                (gone, added) => {
                    if let Some(gone) = gone {
                        self.side(prefix, entry, gone, PathChangeKind::Deleted)?;
                    }
                    if let Some(added) = added {
                        self.side(prefix, entry, added, PathChangeKind::Added)?;
                    }
                }
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------------ port

impl GitReader for GixReader {
    fn head(&self, r: RepoKind, branch: &str) -> Result<CommitId, GitError> {
        let (_, id) = self.tip(r, branch)?;
        commit_id(id)
    }

    fn branches(&self, r: RepoKind) -> Result<Vec<BranchInfo>, GitError> {
        let refs =
            snapshot(&self.mirror.repository(r)).map_err(|e| unavailable("git_refs_unreadable", &e))?;
        Ok(refs
            .branches
            .into_iter()
            .map(|(name, head)| BranchInfo {
                kind: branch_kind(r, &name),
                name,
                head,
            })
            .collect())
    }

    fn tags(&self, r: RepoKind) -> Result<Vec<TagInfo>, GitError> {
        let refs =
            snapshot(&self.mirror.repository(r)).map_err(|e| unavailable("git_refs_unreadable", &e))?;
        Ok(refs
            .tags
            .into_iter()
            .map(|(name, commit)| TagInfo { name, commit })
            .collect())
    }

    fn tree_index(&self, r: RepoKind, c: &CommitId) -> Result<Arc<TreeIndex>, GitError> {
        if let Some(hit) = self.trees.get(r, c) {
            return Ok(hit);
        }
        self.trees
            .get_or_build(r, c, || self.build_index(r, c))
            .map_err(|e| *e)
    }

    fn diff_trees(&self, r: RepoKind, from: &CommitId, to: &CommitId) -> Result<Vec<PathChange>, GitError> {
        let (repo, a) = self.locate(r, from)?;
        let (_, b) = self.locate(r, to)?;
        let tree_of = |id: gix::ObjectId| {
            repo.find_commit(id)
                .map_err(|e| unavailable("git_tree_unreadable", &e))?
                .tree_id()
                .map(gix::Id::detach)
                .map_err(|e| unavailable("git_tree_unreadable", &e))
        };
        let (ta, tb) = (tree_of(a)?, tree_of(b)?);
        let mut differ = Differ {
            repo: &repo,
            out: Vec::new(),
        };
        differ.diff("", Some(ta), Some(tb))?;
        let mut out = differ.out;
        out.sort_by(|x, y| x.path.cmp(&y.path));
        Ok(out)
    }

    fn read(&self, r: RepoKind, c: &CommitId, path: &RepoPath) -> Result<Option<Bytes>, GitError> {
        let (repo, id) = self.locate(r, c)?;
        let tree = tree_of(&repo, id)?;
        let entry = tree
            .lookup_entry_by_path(path.as_str())
            .map_err(|e| unavailable("git_tree_unreadable", &e))?;
        let Some(entry) = entry else { return Ok(None) };
        if !is_file(entry.mode().kind()) {
            return Ok(None);
        }
        let blob = repo
            .find_blob(entry.oid())
            .map_err(|e| unavailable("git_blob_unreadable", &e))?;
        Ok(Some(Bytes::from(blob.detach().data)))
    }

    fn history(
        &self,
        r: RepoKind,
        branch: &str,
        path: &RepoPath,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, GitError> {
        let (repo, tip) = self.tip(r, branch)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        // The blob id at `path` in a commit (or `None`), remembered, because each commit is asked about once as a child and once
        // for each child that names it a parent.
        let mut memo: HashMap<gix::ObjectId, Option<gix::ObjectId>> = HashMap::new();
        let mut blob_at = |id: gix::ObjectId| -> Result<Option<gix::ObjectId>, GitError> {
            if let Some(known) = memo.get(&id) {
                return Ok(*known);
            }
            let tree = tree_of(&repo, id)?;
            let found = tree
                .lookup_entry_by_path(path.as_str())
                .map_err(|e| unavailable("git_tree_unreadable", &e))?
                .filter(|e| is_file(e.mode().kind()))
                .map(|e| e.oid().to_owned());
            memo.insert(id, found);
            Ok(found)
        };
        let walk = repo
            .rev_walk([tip])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(|e| unavailable("git_walk_failed", &e))?;
        let mut out = Vec::new();
        for (scanned, info) in walk.enumerate() {
            if scanned >= MAX_HISTORY_SCAN || out.len() >= limit {
                break;
            }
            let info = info.map_err(|e| unavailable("git_walk_failed", &e))?;
            let here = blob_at(info.id)?;
            // The commit changed the path when the path differs from every parent (a root commit has none).
            let parents: Vec<gix::ObjectId> = info.parent_ids().map(gix::Id::detach).collect();
            let mut same_as_a_parent = false;
            for parent in &parents {
                if blob_at(*parent)? == here {
                    same_as_a_parent = true;
                    break;
                }
            }
            let changed = if parents.is_empty() {
                here.is_some()
            } else {
                !same_as_a_parent
            };
            if !changed {
                continue;
            }
            let commit = repo
                .find_commit(info.id)
                .map_err(|e| unavailable("git_commit_unreadable", &e))?;
            let author = commit
                .author()
                .map_err(|e| unavailable("git_commit_unreadable", &e))?;
            let message = commit
                .message()
                .map_err(|e| unavailable("git_commit_unreadable", &e))?;
            let seconds = commit
                .time()
                .map_err(|e| unavailable("git_commit_unreadable", &e))?
                .seconds;
            out.push(CommitInfo {
                id: commit_id(info.id)?,
                author: short_text(author.name)?,
                time: Timestamp::from_unix_millis(seconds.saturating_mul(1000)),
                summary: short_text(first_line(message.title))?,
            });
        }
        Ok(out)
    }

    fn describe(&self, r: RepoKind, c: &CommitId) -> Result<VersionLabel, GitError> {
        let (repo, id) = self.locate(r, c)?;
        let commit = repo
            .find_commit(id)
            .map_err(|e| unavailable("git_commit_unreadable", &e))?;
        let resolution = commit
            .describe()
            .names(gix::commit::describe::SelectRef::AllTags)
            .try_resolve()
            .map_err(|e| unavailable("git_describe_failed", &e))?;
        let (tag, distance) = match resolution {
            Some(found) => {
                let tag = found
                    .outcome
                    .name
                    .as_ref()
                    .and_then(|n| n.to_str().ok())
                    .and_then(|n| GitRef::parse(n.strip_prefix("refs/tags/").unwrap_or(n)).ok());
                (tag, found.outcome.depth)
            }
            None => (None, 0),
        };
        Ok(VersionLabel {
            tag,
            distance,
            commit: c.clone(),
        })
    }
}
