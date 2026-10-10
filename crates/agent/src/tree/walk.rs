//! The walker (T4, decision A1, S17, P1, P4): an own parallel walk over the cap-std root.
//!
//! Why not `ignore::WalkParallel`: it walks by path, so a directory swapped for a symlink between the listing and the
//! descent would be followed. Here every directory is opened through its parent's handle ([`Dir::open_dir`]), every file
//! through the directory that holds it, and each handle is checked against the entry that was listed (same inode), so
//! nothing the walker opens can lie outside the root, and a swap in the window is noticed and left for the next walk.
//!
//! What the walk does, per directory:
//!
//! 1. list it and `lstat` each entry (no symlink is followed; hidden files and `.gitignore` mean nothing: D16 says every
//!    file is tracked);
//! 2. leave out names that match the ignore globs (`.nfs*` silly-rename files, the agent's own temp files, anything
//!    configured);
//! 3. give each regular file a leaf: reuse the previous one when size, mtime, ctime and inode are unchanged, hash it
//!    otherwise (a [`ScanMode::Full`] walk hashes every file: NFS attribute caching can hide a change from the stat);
//! 4. recurse into subdirectories in parallel, and share the previous subtree when nothing in it changed.
//!
//! A scan error never becomes a removal. A directory that cannot be listed, or a file that cannot be read, keeps what
//! the tree already had for it and is counted; only an entry that is gone (`NotFound`) disappears. If the root itself
//! cannot be listed the whole scan fails and the tree stays as it was.

use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use cap_std::fs::{Dir, Metadata, MetadataExt};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use tracing::warn;

use super::hash::{HASH_LIMIT, Pool, hash_stream, stat_of};
use super::node::{DirNode, Entry, FileLeaf, MerkleTree, Name, OtherReason};

/// Directories nested deeper than this are not entered. A real config tree is a handful of levels deep; this keeps the
/// recursion, and the paths, bounded (rule 5).
pub const MAX_DEPTH: usize = 64;
/// Entries (files, directories and everything else) in one tree. Past this the scan stops with an error instead of
/// growing the tree without bound. About 150 bytes each, so this is about 15 MB.
pub const MAX_ENTRIES: usize = 100_000;
/// `NfsPath` limits, repeated here because `domain` keeps them private. `valid_names_agree_with_nfs_path` checks them.
const MAX_PATH_BYTES: usize = 1024;
const MAX_COMPONENT_BYTES: usize = 255;
/// Always ignored: NFS silly-rename files and the agent's own temporary files (A9).
pub const BUILT_IN_IGNORE: [&str; 2] = [".nfs*", ".lanekeeper-tmp-*"];

/// How much of the tree a walk reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Rehash only the files whose size, mtime, ctime or inode changed.
    Stat,
    /// Rehash every file.
    Full,
}

/// What a walk is told to leave out, mark and limit.
#[derive(Clone)]
pub struct WalkConfig {
    ignore: GlobSet,
    deny: GlobSet,
    hash_limit: u64,
    max_entries: usize,
    /// Tests only: an I/O error to raise at a named place.
    #[cfg(test)]
    fault: Option<Arc<FaultFn>>,
}

/// Where a test can make the walker fail.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    List,
    Hash,
}

#[cfg(test)]
type FaultFn = dyn Fn(&str, Fault) -> Option<io::ErrorKind> + Send + Sync;

impl std::fmt::Debug for WalkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalkConfig")
            .field("ignore", &self.ignore.len())
            .field("deny", &self.deny.len())
            .field("hash_limit", &self.hash_limit)
            .field("max_entries", &self.max_entries)
            .finish_non_exhaustive()
    }
}

impl Default for WalkConfig {
    /// The built-in ignore globs and nothing denied.
    fn default() -> Self {
        Self::new(&BUILT_IN_IGNORE, &[]).unwrap_or_else(|_| Self::bare())
    }
}

impl WalkConfig {
    fn bare() -> Self {
        Self {
            ignore: GlobSet::empty(),
            deny: GlobSet::empty(),
            hash_limit: HASH_LIMIT,
            max_entries: MAX_ENTRIES,
            #[cfg(test)]
            fault: None,
        }
    }

    /// `ignore`: names or paths to leave out of the tree. `deny`: files that are tracked but whose bytes are never
    /// sent (D79). A glob without a `/` matches a name anywhere; one with a `/` matches the path from the root. The
    /// caller supplies the whole ignore list, built-ins included ([`BUILT_IN_IGNORE`]).
    pub fn new(ignore: &[&str], deny: &[&str]) -> Result<Self, globset::Error> {
        Ok(Self {
            ignore: glob_set(ignore)?,
            deny: glob_set(deny)?,
            ..Self::bare()
        })
    }

    /// The most bytes of a file that are hashed.
    #[must_use]
    pub fn with_hash_limit(mut self, limit: u64) -> Self {
        self.hash_limit = limit;
        self
    }

    /// The most entries one tree may hold.
    #[must_use]
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    pub fn hash_limit(&self) -> u64 {
        self.hash_limit
    }

    fn ignored(&self, name: &str, rel: &str) -> bool {
        !self.ignore.is_empty() && (self.ignore.is_match(name) || self.ignore.is_match(rel))
    }

    fn denied(&self, name: &str, rel: &str) -> bool {
        !self.deny.is_empty() && (self.deny.is_match(name) || self.deny.is_match(rel))
    }
}

fn glob_set(patterns: &[&str]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(GlobBuilder::new(pattern).backslash_escape(true).build()?);
    }
    builder.build()
}

/// Counters from one walk, for metrics and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub dirs: u64,
    /// Regular files seen.
    pub files: u64,
    /// Files read and hashed.
    pub hashed: u64,
    pub hashed_bytes: u64,
    /// Places where an entry could not be read and the tree kept its earlier state.
    pub errors: u64,
}

/// A finished walk.
#[derive(Debug, Clone)]
pub struct ScanOutcome {
    pub tree: MerkleTree,
    /// Bytes of the directory nodes this walk created; nodes shared with the previous tree are not counted.
    pub new_bytes: usize,
    pub stats: ScanStats,
}

/// Why a walk produced no tree. The tree the agent already has is still right.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// The root could not be listed: the mount is gone or stale.
    #[error("the NFS root cannot be listed ({0:?})")]
    Root(io::ErrorKind),
    /// More entries than a tree may hold.
    #[error("the NFS root holds more than {0} entries")]
    TooManyEntries(usize),
    /// The walk did not finish (the task was cancelled or panicked).
    #[error("the scan did not finish")]
    Interrupted,
}

struct Ctx<'a> {
    cfg: &'a WalkConfig,
    mode: ScanMode,
    entries: AtomicUsize,
    aborted: AtomicBool,
    new_bytes: AtomicUsize,
    dirs: AtomicU64,
    files: AtomicU64,
    hashed: AtomicU64,
    hashed_bytes: AtomicU64,
    errors: AtomicU64,
}

impl Ctx<'_> {
    /// Count a place where the tree kept its earlier state. The log has the path and the kind of error, never the OS
    /// message or any content (S10).
    fn failed(&self, rel: &str, kind: io::ErrorKind) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        warn!(path = rel, error = ?kind, "cannot read; the tree keeps what it had");
    }

    #[cfg(test)]
    fn fault(&self, rel: &str, at: Fault) -> Option<io::ErrorKind> {
        self.cfg.fault.as_ref().and_then(|f| f(rel, at))
    }
}

/// Walk `root` and build its tree. `previous` is the last tree: files whose stat is unchanged reuse their leaf, and
/// unchanged directories reuse their node. Blocking: run it from `spawn_blocking`. The work runs on `pool`.
pub fn walk(
    pool: &Pool,
    root: &Dir,
    previous: Option<&MerkleTree>,
    mode: ScanMode,
    cfg: &WalkConfig,
) -> Result<ScanOutcome, ScanError> {
    let ctx = Ctx {
        cfg,
        mode,
        entries: AtomicUsize::new(0),
        aborted: AtomicBool::new(false),
        new_bytes: AtomicUsize::new(0),
        dirs: AtomicU64::new(0),
        files: AtomicU64::new(0),
        hashed: AtomicU64::new(0),
        hashed_bytes: AtomicU64::new(0),
        errors: AtomicU64::new(0),
    };
    let prev_root = previous.map(MerkleTree::root);
    let scanned = pool.install(|| scan_dir(&ctx, root, "", 0, prev_root))?;
    match scanned {
        DirScan::Node(node) => Ok(ScanOutcome {
            tree: MerkleTree::from_root(node),
            new_bytes: ctx.new_bytes.load(Ordering::Relaxed),
            stats: ScanStats {
                dirs: ctx.dirs.load(Ordering::Relaxed),
                files: ctx.files.load(Ordering::Relaxed),
                hashed: ctx.hashed.load(Ordering::Relaxed),
                hashed_bytes: ctx.hashed_bytes.load(Ordering::Relaxed),
                errors: ctx.errors.load(Ordering::Relaxed),
            },
        }),
        DirScan::Failed(kind) => Err(ScanError::Root(kind)),
        DirScan::Gone => Err(ScanError::Root(io::ErrorKind::NotFound)),
    }
}

enum DirScan {
    Node(Arc<DirNode>),
    /// The directory could not be listed.
    Failed(io::ErrorKind),
    /// The directory no longer exists.
    Gone,
}

/// Something to do for one listed entry, after the cheap classification.
enum Work<'a> {
    File(FileWork<'a>),
    Dir(DirWork<'a>),
}

struct FileWork<'a> {
    os_name: OsString,
    key: Name,
    rel: String,
    stat: super::node::Stat,
    ino: u64,
    dev: u64,
    denied: bool,
    previous: Option<&'a Entry>,
}

struct DirWork<'a> {
    os_name: OsString,
    key: Name,
    rel: String,
    ino: u64,
    dev: u64,
    depth: usize,
    previous: Option<&'a Entry>,
}

fn scan_dir(
    ctx: &Ctx<'_>,
    dir: &Dir,
    rel: &str,
    depth: usize,
    previous: Option<&Arc<DirNode>>,
) -> Result<DirScan, ScanError> {
    if ctx.aborted.load(Ordering::Relaxed) {
        return Err(ScanError::TooManyEntries(ctx.cfg.max_entries));
    }
    #[cfg(test)]
    if let Some(kind) = ctx.fault(rel, Fault::List) {
        return Ok(DirScan::Failed(kind));
    }
    let listing = match dir.entries() {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DirScan::Gone),
        Err(e) => return Ok(DirScan::Failed(e.kind())),
    };
    ctx.dirs.fetch_add(1, Ordering::Relaxed);

    let mut fixed: Vec<(Name, Entry)> = Vec::new();
    let mut work: Vec<Work<'_>> = Vec::new();
    for item in listing {
        let item = match item {
            Ok(item) => item,
            // A listing that fails half way is not a listing: keep the directory as it was.
            Err(e) => return Ok(DirScan::Failed(e.kind())),
        };
        let os_name = item.file_name();
        let bytes = os_name.as_bytes();
        let valid = valid_component(bytes);
        let lossy;
        let name_for_globs = if let Some(name) = valid {
            name
        } else {
            lossy = String::from_utf8_lossy(bytes);
            &*lossy
        };
        let rel_child = child_rel(rel, name_for_globs);
        if ctx.cfg.ignored(name_for_globs, &rel_child) {
            continue;
        }
        if ctx.entries.fetch_add(1, Ordering::Relaxed) >= ctx.cfg.max_entries {
            ctx.aborted.store(true, Ordering::Relaxed);
            return Err(ScanError::TooManyEntries(ctx.cfg.max_entries));
        }
        let previous_entry = previous.and_then(|p| p.entries().get(bytes));
        let key: Name = previous
            .and_then(|p| p.entries().get_key_value(bytes))
            .map_or_else(|| Name::from(bytes), |(k, _)| Arc::clone(k));
        let md = match item.metadata() {
            Ok(md) => md,
            // Deleted between the listing and the stat.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                ctx.failed(&rel_child, e.kind());
                fixed.push((key, keep_previous(previous_entry)));
                continue;
            }
        };
        let kind = md.file_type();
        if valid.is_none() || rel_child.len() > MAX_PATH_BYTES {
            fixed.push((key, Entry::Other(OtherReason::Unrepresentable)));
        } else if kind.is_symlink() {
            fixed.push((key, Entry::Other(OtherReason::Symlink)));
        } else if kind.is_dir() {
            if depth + 1 >= MAX_DEPTH {
                fixed.push((key, Entry::Other(OtherReason::TooDeep)));
            } else {
                work.push(Work::Dir(DirWork {
                    os_name,
                    key,
                    rel: rel_child,
                    ino: md.ino(),
                    dev: md.dev(),
                    depth,
                    previous: previous_entry,
                }));
            }
        } else if kind.is_file() {
            ctx.files.fetch_add(1, Ordering::Relaxed);
            let denied = ctx.cfg.denied(name_for_globs, &rel_child);
            work.push(Work::File(FileWork {
                os_name,
                key,
                rel: rel_child,
                stat: stat_of(&md),
                ino: md.ino(),
                dev: md.dev(),
                denied,
                previous: previous_entry,
            }));
        } else {
            fixed.push((key, Entry::Other(OtherReason::Special)));
        }
    }

    let done: Result<Vec<Option<(Name, Entry)>>, ScanError> = work
        .into_par_iter()
        .map(|w| match w {
            Work::File(f) => Ok(scan_file(ctx, dir, f)),
            Work::Dir(d) => scan_child_dir(ctx, dir, d),
        })
        .collect();
    fixed.extend(done?.into_iter().flatten());
    Ok(DirScan::Node(assemble(ctx, previous, fixed)))
}

/// Build the directory node, or hand back the previous one when nothing in it changed.
fn assemble(ctx: &Ctx<'_>, previous: Option<&Arc<DirNode>>, mut items: Vec<(Name, Entry)>) -> Arc<DirNode> {
    items.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(previous) = previous {
        let same = previous.entries().len() == items.len()
            && previous
                .entries()
                .iter()
                .zip(&items)
                .all(|((pn, pe), (n, e))| pn == n && same_entry(pe, e));
        if same {
            return Arc::clone(previous);
        }
    }
    let node = Arc::new(DirNode::new(items.into_iter().collect()));
    ctx.new_bytes.fetch_add(node.own_bytes(), Ordering::Relaxed);
    node
}

/// Equal in every way a later walk could see: the leaf with its stat, the reason, or the very same subtree.
fn same_entry(a: &Entry, b: &Entry) -> bool {
    match (a, b) {
        (Entry::File(x), Entry::File(y)) => x == y,
        (Entry::Other(x), Entry::Other(y)) => x == y,
        (Entry::Dir(x), Entry::Dir(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

/// What to show for an entry that could not be read this time: what the tree had, or "unreadable" for a new one.
fn keep_previous(previous: Option<&Entry>) -> Entry {
    previous.cloned().unwrap_or(Entry::Other(OtherReason::Unreadable))
}

fn scan_file(ctx: &Ctx<'_>, dir: &Dir, w: FileWork<'_>) -> Option<(Name, Entry)> {
    if ctx.mode == ScanMode::Stat {
        if let Some(Entry::File(leaf)) = w.previous {
            if leaf.stat == w.stat && leaf.denied == w.denied {
                return Some((w.key, Entry::File(*leaf)));
            }
        }
    }
    match hash_file(ctx, dir, &w) {
        Ok(Some(entry)) => Some((w.key, entry)),
        // Gone since the listing.
        Ok(None) => None,
        Err(kind) => {
            ctx.failed(&w.rel, kind);
            Some((w.key, keep_previous(w.previous)))
        }
    }
}

/// Open the file through its directory, check that it is the file that was listed, and hash it.
/// `Ok(None)`: the file is gone. `Err`: it could not be read this time.
fn hash_file(ctx: &Ctx<'_>, dir: &Dir, w: &FileWork<'_>) -> Result<Option<Entry>, io::ErrorKind> {
    #[cfg(test)]
    if let Some(kind) = ctx.fault(&w.rel, Fault::Hash) {
        return Err(kind);
    }
    let mut file = match dir.open(&w.os_name) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.kind()),
    };
    let md = file.metadata().map_err(|e| e.kind())?;
    if !is_same_regular_file(&md, w.ino, w.dev) {
        // Swapped for something else between the listing and the open. The next walk sees what is there now.
        return Err(io::ErrorKind::InvalidData);
    }
    // The stat taken before the read is the one that is remembered: a write during the read leaves a newer stat on
    // the file, so the next walk reads it again.
    let stat = stat_of(&md);
    if stat.size > ctx.cfg.hash_limit {
        return Ok(Some(Entry::Other(OtherReason::TooLarge)));
    }
    let Some((hash, read)) = hash_stream(&mut file, ctx.cfg.hash_limit).map_err(|e| e.kind())? else {
        return Ok(Some(Entry::Other(OtherReason::TooLarge)));
    };
    ctx.hashed.fetch_add(1, Ordering::Relaxed);
    ctx.hashed_bytes.fetch_add(read, Ordering::Relaxed);
    Ok(Some(Entry::File(FileLeaf::new(hash, stat).with_denied(w.denied))))
}

/// True when `md` is a regular file with the inode and device that were listed.
fn is_same_regular_file(md: &Metadata, ino: u64, dev: u64) -> bool {
    md.is_file() && md.ino() == ino && md.dev() == dev
}

fn scan_child_dir(ctx: &Ctx<'_>, dir: &Dir, w: DirWork<'_>) -> Result<Option<(Name, Entry)>, ScanError> {
    let child = match dir.open_dir(&w.os_name) {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            ctx.failed(&w.rel, e.kind());
            return Ok(Some((w.key, keep_previous(w.previous))));
        }
    };
    // It was a directory when it was listed; make sure this is the same one.
    let same = child
        .dir_metadata()
        .is_ok_and(|md| md.is_dir() && md.ino() == w.ino && md.dev() == w.dev);
    if !same {
        ctx.failed(&w.rel, io::ErrorKind::InvalidData);
        return Ok(Some((w.key, keep_previous(w.previous))));
    }
    let previous_dir = match w.previous {
        Some(Entry::Dir(d)) => Some(d),
        _ => None,
    };
    match scan_dir(ctx, &child, &w.rel, w.depth + 1, previous_dir)? {
        DirScan::Node(node) => Ok(Some((w.key, Entry::Dir(node)))),
        DirScan::Failed(kind) => {
            ctx.failed(&w.rel, kind);
            Ok(Some((w.key, keep_previous(w.previous))))
        }
        DirScan::Gone => Ok(None),
    }
}

fn child_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        let mut rel = String::with_capacity(parent.len() + 1 + name.len());
        rel.push_str(parent);
        rel.push('/');
        rel.push_str(name);
        rel
    }
}

/// The name as a path component, if it can be one: UTF-8, no control characters, no `/` or `\`, not `.`, `..` or empty,
/// at most 255 bytes. These are the rules of [`domain::NfsPath`]; the hub would refuse any other path.
fn valid_component(name: &[u8]) -> Option<&str> {
    if name.is_empty() || name.len() > MAX_COMPONENT_BYTES {
        return None;
    }
    let text = std::str::from_utf8(name).ok()?;
    if text == "." || text == ".." || text.chars().any(|c| c.is_control() || c == '\\' || c == '/') {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use domain::NfsPath;
    use proptest::prelude::*;
    use tempfile::TempDir;

    use super::*;
    use crate::root::NfsRoot;

    fn pool() -> Pool {
        Pool::new(2).unwrap()
    }

    use crate::tree::testfs::write;

    fn faulty(rules: Vec<(&'static str, Fault, io::ErrorKind)>) -> WalkConfig {
        let mut cfg = WalkConfig::default();
        let rules = Mutex::new(rules);
        cfg.fault = Some(Arc::new(move |rel, at| {
            rules
                .lock()
                .unwrap()
                .iter()
                .find(|(r, a, _)| *r == rel && *a == at)
                .map(|(_, _, k)| *k)
        }));
        cfg
    }

    fn run(
        dir: &TempDir,
        prev: Option<&MerkleTree>,
        mode: ScanMode,
        cfg: &WalkConfig,
    ) -> Result<ScanOutcome, ScanError> {
        let root = NfsRoot::open(dir.path()).unwrap();
        walk(&pool(), root.dir(), prev, mode, cfg)
    }

    #[test]
    fn a_directory_that_cannot_be_listed_keeps_its_earlier_subtree() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "svc/a.yml", b"a");
        write(dir.path(), "svc/sub/b.yml", b"b");
        write(dir.path(), "other/c.yml", b"c");
        let first = run(&dir, None, ScanMode::Full, &WalkConfig::default()).unwrap();

        // `svc` is still on disk, but listing it fails this time. `other` changes, and the walk goes on.
        write(dir.path(), "other/new.yml", b"n");
        let cfg = faulty(vec![("svc", Fault::List, io::ErrorKind::PermissionDenied)]);
        let out = run(&dir, Some(&first.tree), ScanMode::Stat, &cfg).unwrap();
        assert_eq!(out.stats.errors, 1);
        let files: Vec<_> = out.tree.files().into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            files,
            ["other/c.yml", "other/new.yml", "svc/a.yml", "svc/sub/b.yml"]
        );
        // The failed directory is exactly the node the tree already had: no removal, no rebuild.
        match (first.tree.get("svc"), out.tree.get("svc")) {
            (Some(Entry::Dir(a)), Some(Entry::Dir(b))) => assert!(Arc::ptr_eq(a, b)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failing_subdirectory_with_nothing_earlier_is_unreadable_not_empty() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "svc/a.yml", b"a");
        let cfg = faulty(vec![("svc", Fault::List, io::ErrorKind::PermissionDenied)]);
        let out = run(&dir, None, ScanMode::Full, &cfg).unwrap();
        assert!(matches!(
            out.tree.get("svc"),
            Some(Entry::Other(OtherReason::Unreadable))
        ));
        assert_eq!(out.stats.errors, 1);
    }

    #[test]
    fn scan_error_never_reports_removal() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.yml", b"one");
        write(dir.path(), "b.yml", b"two");
        let first = run(&dir, None, ScanMode::Full, &WalkConfig::default()).unwrap();
        // b.yml changed, but cannot be read this time: the tree must keep the old leaf, not drop the file.
        write(dir.path(), "b.yml", b"two, changed");
        let cfg = faulty(vec![("b.yml", Fault::Hash, io::ErrorKind::TimedOut)]);
        let out = run(&dir, Some(&first.tree), ScanMode::Stat, &cfg).unwrap();
        assert_eq!(out.stats.errors, 1);
        assert_eq!(out.tree.root_hash(), first.tree.root_hash());
        let diff = out.tree.diff(&first.tree);
        assert!(diff.removed.is_empty() && diff.changed.is_empty(), "{diff:?}");
        // A new file that cannot be read is tracked as unreadable and tried again next time.
        write(dir.path(), "c.yml", b"three");
        let cfg = faulty(vec![("c.yml", Fault::Hash, io::ErrorKind::TimedOut)]);
        let second = run(&dir, Some(&first.tree), ScanMode::Stat, &cfg).unwrap();
        assert!(matches!(
            second.tree.get("c.yml"),
            Some(Entry::Other(OtherReason::Unreadable))
        ));
        let third = run(&dir, Some(&second.tree), ScanMode::Stat, &WalkConfig::default()).unwrap();
        assert!(matches!(third.tree.get("c.yml"), Some(Entry::File(_))));
    }

    #[test]
    fn an_unlistable_root_fails_the_scan() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.yml", b"a");
        let cfg = faulty(vec![("", Fault::List, io::ErrorKind::StaleNetworkFileHandle)]);
        assert_eq!(
            run(&dir, None, ScanMode::Full, &cfg).unwrap_err(),
            ScanError::Root(io::ErrorKind::StaleNetworkFileHandle)
        );
    }

    #[test]
    fn only_the_listed_regular_file_is_accepted() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.yml", b"a");
        write(dir.path(), "b.yml", b"b");
        std::os::unix::fs::symlink("a.yml", dir.path().join("link")).unwrap();
        let root = NfsRoot::open(dir.path()).unwrap();
        let a = root.dir().symlink_metadata("a.yml").unwrap();
        let b = root.dir().symlink_metadata("b.yml").unwrap();
        let link = root.dir().symlink_metadata("link").unwrap();
        let top = root.dir().dir_metadata().unwrap();
        assert!(is_same_regular_file(&a, a.ino(), a.dev()));
        // Another file now at the name (a different inode), a link, and a directory are all refused.
        assert!(!is_same_regular_file(&b, a.ino(), a.dev()));
        assert!(!is_same_regular_file(&link, link.ino(), link.dev()));
        assert!(!is_same_regular_file(&top, top.ino(), top.dev()));
    }

    #[test]
    fn a_directory_nested_too_deep_is_kind_other() {
        let dir = TempDir::new().unwrap();
        let deep = (0..(MAX_DEPTH + 3))
            .map(|i| format!("d{i}"))
            .collect::<Vec<_>>()
            .join("/")
            + "/";
        write(dir.path(), &format!("{deep}f.yml"), b"x");
        let out = run(&dir, None, ScanMode::Full, &WalkConfig::default()).unwrap();
        assert!(out.tree.files().is_empty());
        let mut at = out.tree.root().clone();
        let mut levels = 0;
        loop {
            let next = at.entries().values().next().cloned();
            match next {
                Some(Entry::Dir(d)) => {
                    levels += 1;
                    at = d;
                }
                Some(Entry::Other(OtherReason::TooDeep)) => break,
                other => panic!("level {levels}: {other:?}"),
            }
        }
        assert_eq!(levels, MAX_DEPTH - 1);
    }

    #[test]
    fn valid_names_agree_with_nfs_path() {
        for name in [
            &b"plain.yml"[..],
            b"with space",
            "ünï.yml".as_bytes(),
            b".hidden",
            b"a\\b",
            b"a\nb",
            b"a\x7fb",
            b"..",
            b".",
            b"",
            b"caf\xe9",
            b"tab\there",
            &[b'x'; 255],
            &[b'x'; 256],
        ] {
            let ours = valid_component(name).is_some();
            let theirs = std::str::from_utf8(name)
                .ok()
                .is_some_and(|s| NfsPath::parse(s).is_ok() && !s.contains('/'));
            // "." and ".." parse as an empty or traversing path, so NfsPath rejects them too.
            assert_eq!(ours, theirs, "{:?}", String::from_utf8_lossy(name));
        }
    }

    proptest! {
        #[test]
        fn a_valid_component_is_always_a_valid_nfs_path(name in ".{0,40}") {
            if let Some(valid) = valid_component(name.as_bytes()) {
                let parsed = NfsPath::parse(valid);
                prop_assert!(parsed.is_ok());
                let path = parsed.unwrap();
                prop_assert_eq!(path.as_str(), valid);
            }
        }
    }
}
