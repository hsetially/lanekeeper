//! The Merkle tree itself (T4, D63, decision A11): persistent directory nodes, the byte encoding, edits that copy a
//! path, and the difference between two trees.
//!
//! # Encoding
//!
//! - A file's hash is the SHA-256 of its bytes.
//! - A directory's hash is the SHA-256 of, for each entry sorted by raw name bytes:
//!   `u32_le(name length) || name || kind byte || 32-byte child hash`.
//! - The kind byte is 0 for a file, 1 for a directory and 2 for anything else. A symlink, a special file, a file too big
//!   to hash and a name that is not a valid path have kind 2 and an all-zero hash.
//! - An empty directory is a node (its hash is the SHA-256 of nothing), so creating one changes the root (C11).
//! - Names are raw bytes. A name that is not UTF-8 is still part of its directory's hash, byte for byte.
//!
//! `merkle_encoding_golden` pins the bytes, because prompt 05 may need to reproduce roots.
//!
//! # Persistence
//!
//! A directory is an `Arc<DirNode>`. An edit rebuilds the directories on the path to the change and shares every other
//! subtree with the tree it came from, so keeping an hour of roots (the [`RootRing`](super::RootRing)) costs a path per
//! change, not a tree. File stat fields (size, mtime, ctime, inode) sit in the leaves for the next stat walk and are not
//! part of any hash.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use domain::ContentHash;
use sha2::{Digest, Sha256};

/// A directory entry's name: raw bytes, shared between the trees that contain it.
pub type Name = Arc<[u8]>;

/// What an entry is, as far as the hash is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    Other,
}

impl Kind {
    const fn byte(self) -> u8 {
        match self {
            Self::File => 0,
            Self::Directory => 1,
            Self::Other => 2,
        }
    }
}

/// A time as the file system reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatTime {
    pub secs: i64,
    pub nanos: u32,
}

impl StatTime {
    pub const fn new(secs: i64, nanos: u32) -> Self {
        Self { secs, nanos }
    }

    /// Milliseconds since the epoch, for the wire.
    pub const fn unix_millis(self) -> i64 {
        self.secs
            .saturating_mul(1000)
            .saturating_add((self.nanos / 1_000_000) as i64)
    }
}

/// The attributes a stat walk compares. A file is rehashed when any of them differs from what the tree remembers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    pub mtime: StatTime,
    pub ctime: StatTime,
    pub ino: u64,
}

impl Stat {
    pub const fn new(size: u64, mtime: StatTime, ctime: StatTime, ino: u64) -> Self {
        Self {
            size,
            mtime,
            ctime,
            ino,
        }
    }
}

/// A regular file in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileLeaf {
    pub hash: ContentHash,
    pub stat: Stat,
    /// The path matches a deny glob (D79): the hash is known, the bytes are never sent.
    pub denied: bool,
}

impl FileLeaf {
    pub const fn new(hash: ContentHash, stat: Stat) -> Self {
        Self {
            hash,
            stat,
            denied: false,
        }
    }

    #[must_use]
    pub const fn with_denied(mut self, denied: bool) -> Self {
        self.denied = denied;
        self
    }
}

/// Why an entry has kind 2 (no content hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtherReason {
    /// A symbolic link. Never followed (S17).
    Symlink,
    /// A socket, a pipe or a device.
    Special,
    /// A file bigger than the most the agent hashes (64 MiB).
    TooLarge,
    /// A new file the agent could not read. It is tried again on every walk.
    Unreadable,
    /// A name that cannot be an [`NfsPath`](domain::NfsPath) component, or a path that would be too long.
    Unrepresentable,
    /// A directory nested deeper than the walker goes.
    TooDeep,
}

/// One entry of a directory.
#[derive(Debug, Clone)]
pub enum Entry {
    File(FileLeaf),
    Other(OtherReason),
    Dir(Arc<DirNode>),
}

impl Entry {
    /// A directory with nothing in it.
    pub fn empty_dir() -> Self {
        Self::Dir(Arc::new(DirNode::new(BTreeMap::new())))
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::File(_) => Kind::File,
            Self::Other(_) => Kind::Other,
            Self::Dir(_) => Kind::Directory,
        }
    }

    /// The hash that goes into the parent: the content hash, the directory hash, or all zeroes.
    pub fn hash(&self) -> ContentHash {
        match self {
            Self::File(leaf) => leaf.hash,
            Self::Other(_) => ContentHash::from_bytes([0; 32]),
            Self::Dir(dir) => dir.hash,
        }
    }
}

/// A directory: its entries by name, its hash, and what is needed to account for it.
#[derive(Debug)]
pub struct DirNode {
    hash: ContentHash,
    entries: BTreeMap<Name, Entry>,
    files: u64,
    own_bytes: usize,
}

impl DirNode {
    pub fn new(entries: BTreeMap<Name, Entry>) -> Self {
        let mut hasher = Sha256::new();
        let mut files = 0_u64;
        for (name, entry) in &entries {
            hasher.update(u32::try_from(name.len()).unwrap_or(u32::MAX).to_le_bytes());
            hasher.update(&**name);
            hasher.update([entry.kind().byte()]);
            hasher.update(entry.hash().as_bytes());
            files += match entry {
                Entry::File(_) => 1,
                Entry::Dir(dir) => dir.files,
                Entry::Other(_) => 0,
            };
        }
        // What this node alone holds on the heap, rounded up: the map's slots and one `Arc` header per name. Names and
        // subtrees that other nodes share are not counted again.
        let own_bytes = size_of::<Self>()
            + entries.len() * (size_of::<Name>() + size_of::<Entry>() + 32)
            + entries.keys().map(|n| n.len()).sum::<usize>();
        Self {
            hash: ContentHash::from_bytes(hasher.finalize().into()),
            entries,
            files,
            own_bytes,
        }
    }

    pub fn hash(&self) -> ContentHash {
        self.hash
    }

    pub fn entries(&self) -> &BTreeMap<Name, Entry> {
        &self.entries
    }

    /// Regular files in this subtree.
    pub fn files(&self) -> u64 {
        self.files
    }

    /// Bytes this node alone takes, for the root ring's budget.
    pub fn own_bytes(&self) -> usize {
        self.own_bytes
    }
}

/// A tree after an edit, and what the edit allocated.
#[derive(Debug, Clone)]
pub struct Edited {
    pub tree: MerkleTree,
    /// Bytes of the directory nodes the edit created. Nodes it shares are not counted.
    pub new_bytes: usize,
}

/// The agent's picture of the NFS root.
#[derive(Debug, Clone)]
pub struct MerkleTree {
    root: Arc<DirNode>,
}

impl MerkleTree {
    pub fn empty() -> Self {
        Self::from_root(Arc::new(DirNode::new(BTreeMap::new())))
    }

    pub fn from_root(root: Arc<DirNode>) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Arc<DirNode> {
        &self.root
    }

    pub fn root_hash(&self) -> ContentHash {
        self.root.hash
    }

    /// Regular files in the tree: what a heartbeat reports.
    pub fn file_count(&self) -> u64 {
        self.root.files
    }

    /// Build a tree from `(path, entry)` pairs, in any order. A path is `a/b/c`; its directories are created. The paths
    /// must not clash (a file and a directory with the same name): the directory wins.
    pub fn from_leaves<P: AsRef<str>>(leaves: impl IntoIterator<Item = (P, Entry)>) -> Self {
        let mut root = DirBuilder::default();
        for (path, entry) in leaves {
            let mut parts = path.as_ref().split('/').filter(|c| !c.is_empty()).peekable();
            let mut at = &mut root;
            while let Some(part) = parts.next() {
                if parts.peek().is_some() {
                    at = at.dirs.entry(part.as_bytes().into()).or_default();
                } else {
                    at.leaves.insert(part.as_bytes().into(), entry);
                    break;
                }
            }
        }
        Self::from_root(root.finish())
    }

    /// The entry at `path` (`a/b/c`). Directories come back as [`Entry::Dir`]. The root has no entry.
    pub fn get(&self, path: &str) -> Option<&Entry> {
        let comps: Vec<&[u8]> = path
            .split('/')
            .filter(|c| !c.is_empty())
            .map(str::as_bytes)
            .collect();
        let (last, parents) = comps.split_last()?;
        let mut node = &self.root;
        for comp in parents {
            let Some(Entry::Dir(next)) = node.entries.get(*comp) else {
                return None;
            };
            node = next;
        }
        node.entries.get(*last)
    }

    /// Set the entry at `path`, creating directories on the way. A file in the way of a directory is replaced.
    #[must_use]
    pub fn put(&self, path: &str, entry: Entry) -> Edited {
        let comps: Vec<&[u8]> = path
            .split('/')
            .filter(|c| !c.is_empty())
            .map(str::as_bytes)
            .collect();
        self.edited(&comps, Op::Put(entry))
    }

    /// Set the entry called `name` (raw bytes) inside the directory `parent`. For names that are not valid paths.
    #[must_use]
    pub fn put_raw(&self, parent: &str, name: &[u8], entry: Entry) -> Edited {
        let mut comps: Vec<&[u8]> = parent
            .split('/')
            .filter(|c| !c.is_empty())
            .map(str::as_bytes)
            .collect();
        comps.push(name);
        self.edited(&comps, Op::Put(entry))
    }

    /// Put an empty directory at `path`.
    #[must_use]
    pub fn put_empty_dir(&self, path: &str) -> Edited {
        self.put(path, Entry::empty_dir())
    }

    /// Remove the entry at `path` (and everything below it). Removing what is not there changes nothing.
    #[must_use]
    pub fn remove(&self, path: &str) -> Edited {
        let comps: Vec<&[u8]> = path
            .split('/')
            .filter(|c| !c.is_empty())
            .map(str::as_bytes)
            .collect();
        self.edited(&comps, Op::Remove)
    }

    fn edited(&self, comps: &[&[u8]], op: Op) -> Edited {
        let mut new_bytes = 0;
        let root = if comps.is_empty() {
            Arc::clone(&self.root)
        } else {
            edit(&self.root, comps, op, &mut new_bytes).unwrap_or_else(|| Arc::clone(&self.root))
        };
        Edited {
            tree: Self { root },
            new_bytes,
        }
    }

    /// Every regular file with its leaf, in path order. Paths that are not UTF-8 are shown lossily; for tests and
    /// diagnostics, not for the wire.
    pub fn files(&self) -> Vec<(String, FileLeaf)> {
        let mut out = Vec::new();
        collect_files(&self.root, &mut Vec::new(), &mut |path, leaf| {
            out.push((String::from_utf8_lossy(path).into_owned(), *leaf));
        });
        out
    }

    /// The hash of every directory, the root first (as `""`), in path order.
    pub fn dir_hashes(&self) -> Vec<(String, ContentHash)> {
        fn walk(node: &DirNode, path: &mut Vec<u8>, out: &mut Vec<(String, ContentHash)>) {
            out.push((String::from_utf8_lossy(path).into_owned(), node.hash));
            for (name, entry) in &node.entries {
                if let Entry::Dir(child) = entry {
                    let keep = path.len();
                    if !path.is_empty() {
                        path.push(b'/');
                    }
                    path.extend_from_slice(name);
                    walk(child, path, out);
                    path.truncate(keep);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut out);
        out
    }

    /// Bytes held by this tree's directory nodes, each distinct node once.
    pub fn retained_bytes(&self) -> usize {
        fn walk(node: &Arc<DirNode>, seen: &mut HashSet<*const DirNode>) -> usize {
            if !seen.insert(Arc::as_ptr(node)) {
                return 0;
            }
            node.own_bytes
                + node
                    .entries
                    .values()
                    .map(|e| if let Entry::Dir(d) = e { walk(d, seen) } else { 0 })
                    .sum::<usize>()
        }
        walk(&self.root, &mut HashSet::new())
    }

    /// What changed from `old` to `self`: files to send, files to remove, and entries to report as skipped.
    ///
    /// Subtrees with equal hashes are skipped without looking inside, so the cost follows the change, not the tree.
    /// Only hashes decide: a file whose stat changed and whose content did not is not a change.
    pub fn diff(&self, old: &Self) -> TreeDiff {
        let mut out = TreeDiff::default();
        diff_dir(&self.root, Some(&old.root), &mut Vec::new(), &mut out);
        out
    }

    /// Every file, as it would be reported in a full listing (a diff against nothing).
    pub fn full_listing(&self) -> TreeDiff {
        let mut out = TreeDiff::default();
        diff_dir(&self.root, None, &mut Vec::new(), &mut out);
        out
    }
}

// ------------------------------------------------------------------------------------------ building and editing

#[derive(Default)]
struct DirBuilder {
    leaves: BTreeMap<Name, Entry>,
    dirs: BTreeMap<Name, DirBuilder>,
}

impl DirBuilder {
    fn finish(self) -> Arc<DirNode> {
        let mut entries = self.leaves;
        for (name, dir) in self.dirs {
            entries.insert(name, Entry::Dir(dir.finish()));
        }
        Arc::new(DirNode::new(entries))
    }
}

enum Op {
    Put(Entry),
    Remove,
}

/// Rebuild `node` with `op` applied at `comps`. `None` means nothing changed, so the caller keeps its node.
fn edit(node: &Arc<DirNode>, comps: &[&[u8]], op: Op, new_bytes: &mut usize) -> Option<Arc<DirNode>> {
    let (first, rest) = comps.split_first()?;
    let mut entries = node.entries.clone();
    let key: Name = (*first).into();
    if rest.is_empty() {
        match op {
            Op::Put(entry) => {
                entries.insert(key, entry);
            }
            Op::Remove => {
                entries.remove(&key)?;
            }
        }
    } else {
        let existing = match entries.get(&key) {
            Some(Entry::Dir(dir)) => Some(Arc::clone(dir)),
            _ => None,
        };
        let child = match existing {
            Some(dir) => edit(&dir, rest, op, new_bytes)?,
            None => match op {
                // Nothing to remove below a name that is not a directory.
                Op::Remove => return None,
                Op::Put(_) => edit(&Arc::new(DirNode::new(BTreeMap::new())), rest, op, new_bytes)?,
            },
        };
        entries.insert(key, Entry::Dir(child));
    }
    let rebuilt = Arc::new(DirNode::new(entries));
    *new_bytes += rebuilt.own_bytes;
    Some(rebuilt)
}

fn collect_files(node: &DirNode, path: &mut Vec<u8>, visit: &mut impl FnMut(&[u8], &FileLeaf)) {
    for (name, entry) in &node.entries {
        let keep = path.len();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        match entry {
            Entry::File(leaf) => visit(path, leaf),
            Entry::Dir(child) => collect_files(child, path, visit),
            Entry::Other(_) => {}
        }
        path.truncate(keep);
    }
}

// ------------------------------------------------------------------------------------------ the diff

/// Why something is reported as skipped. The wire reason is [`SkipReason::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Symlink,
    SpecialFile,
    TooLarge,
    Unreadable,
    UnrepresentableName,
    TooDeep,
    EmptyDirectory,
}

impl SkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::SpecialFile => "special_file",
            Self::TooLarge => "too_large",
            Self::Unreadable => "unreadable",
            Self::UnrepresentableName => "unrepresentable_name",
            Self::TooDeep => "too_deep",
            Self::EmptyDirectory => "empty_directory",
        }
    }
}

impl From<OtherReason> for SkipReason {
    fn from(reason: OtherReason) -> Self {
        match reason {
            OtherReason::Symlink => Self::Symlink,
            OtherReason::Special => Self::SpecialFile,
            OtherReason::TooLarge => Self::TooLarge,
            OtherReason::Unreadable => Self::Unreadable,
            OtherReason::Unrepresentable => Self::UnrepresentableName,
            OtherReason::TooDeep => Self::TooDeep,
        }
    }
}

/// A file that is new or whose content changed. The path is raw bytes: it is turned into an
/// [`NfsPath`](domain::NfsPath) where the delta is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: Vec<u8>,
    pub leaf: FileLeaf,
}

/// Something the agent saw and did not send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub path: Vec<u8>,
    pub reason: SkipReason,
}

/// The difference between two trees, in path order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeDiff {
    pub changed: Vec<ChangedFile>,
    pub removed: Vec<Vec<u8>>,
    pub skipped: Vec<Skip>,
}

impl TreeDiff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty() && self.skipped.is_empty()
    }
}

fn diff_dir(new: &Arc<DirNode>, old: Option<&Arc<DirNode>>, path: &mut Vec<u8>, out: &mut TreeDiff) {
    if old.is_some_and(|o| Arc::ptr_eq(new, o) || new.hash == o.hash) {
        return;
    }
    // A directory that is new and empty, or has just been emptied, has no file to carry the news. The root cannot be
    // named as a path, so it is never reported.
    if new.entries.is_empty() && !path.is_empty() && old.is_none_or(|o| !o.entries.is_empty()) {
        out.skipped.push(Skip {
            path: path.clone(),
            reason: SkipReason::EmptyDirectory,
        });
    }
    let mut unrepresentable_reported = false;
    let mut news = new.entries.iter().peekable();
    let mut olds = old.map(|o| o.entries.iter()).into_iter().flatten().peekable();
    loop {
        let order = match (news.peek(), olds.peek()) {
            (None, None) => break,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some((n, _)), Some((o, _))) => n.cmp(o),
        };
        match order {
            std::cmp::Ordering::Less => {
                if let Some((name, entry)) = news.next() {
                    added(name, entry, path, out, &mut unrepresentable_reported);
                }
            }
            std::cmp::Ordering::Greater => {
                if let Some((name, entry)) = olds.next() {
                    removed(name, entry, path, out);
                }
            }
            std::cmp::Ordering::Equal => {
                if let (Some((name, new_entry)), Some((_, old_entry))) = (news.next(), olds.next()) {
                    both(
                        name,
                        new_entry,
                        old_entry,
                        path,
                        out,
                        &mut unrepresentable_reported,
                    );
                }
            }
        }
    }
}

fn child_path(path: &mut Vec<u8>, name: &[u8]) -> usize {
    let keep = path.len();
    if !path.is_empty() {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    keep
}

/// Report an entry that is new, or has taken the place of one of another kind.
fn added(name: &Name, entry: &Entry, path: &mut Vec<u8>, out: &mut TreeDiff, unrep_done: &mut bool) {
    match entry {
        Entry::File(leaf) => {
            let keep = child_path(path, name);
            out.changed.push(ChangedFile {
                path: path.clone(),
                leaf: *leaf,
            });
            path.truncate(keep);
        }
        Entry::Other(OtherReason::Unrepresentable) => {
            // Its own path cannot be written down, so say it once, on the directory that holds it. The root cannot be
            // named as a path, so there the entry's name is shown with every byte that is not valid made printable.
            if !*unrep_done {
                *unrep_done = true;
                let shown = if path.is_empty() {
                    printable(name)
                } else {
                    path.clone()
                };
                out.skipped.push(Skip {
                    path: shown,
                    reason: SkipReason::UnrepresentableName,
                });
            }
        }
        Entry::Other(reason) => {
            let keep = child_path(path, name);
            out.skipped.push(Skip {
                path: path.clone(),
                reason: (*reason).into(),
            });
            path.truncate(keep);
        }
        Entry::Dir(dir) => {
            let keep = child_path(path, name);
            diff_dir(dir, None, path, out);
            path.truncate(keep);
        }
    }
}

/// Report an entry that is gone, or was replaced by one of another kind. Only files are reported: the hub never heard
/// of directories, symlinks or special files.
fn removed(name: &Name, entry: &Entry, path: &mut Vec<u8>, out: &mut TreeDiff) {
    match entry {
        Entry::File(_) => {
            let keep = child_path(path, name);
            out.removed.push(path.clone());
            path.truncate(keep);
        }
        Entry::Dir(dir) => {
            let keep = child_path(path, name);
            collect_below(dir, path, &mut out.removed);
            path.truncate(keep);
        }
        Entry::Other(_) => {}
    }
}

/// Paths of every file under `node`, each prefixed with `prefix` (the path of `node` itself).
fn collect_below(node: &DirNode, prefix: &mut Vec<u8>, out: &mut Vec<Vec<u8>>) {
    for (name, entry) in &node.entries {
        let keep = child_path(prefix, name);
        match entry {
            Entry::File(_) => out.push(prefix.clone()),
            Entry::Dir(child) => collect_below(child, prefix, out),
            Entry::Other(_) => {}
        }
        prefix.truncate(keep);
    }
}

fn both(
    name: &Name,
    new: &Entry,
    old: &Entry,
    path: &mut Vec<u8>,
    out: &mut TreeDiff,
    unrep_done: &mut bool,
) {
    match (new, old) {
        (Entry::File(n), Entry::File(o)) => {
            if n.hash != o.hash || n.denied != o.denied {
                let keep = child_path(path, name);
                out.changed.push(ChangedFile {
                    path: path.clone(),
                    leaf: *n,
                });
                path.truncate(keep);
            }
        }
        (Entry::Dir(n), Entry::Dir(o)) => {
            let keep = child_path(path, name);
            diff_dir(n, Some(o), path, out);
            path.truncate(keep);
        }
        (Entry::Other(n), Entry::Other(o)) => {
            if n != o {
                added(name, new, path, out, unrep_done);
            }
        }
        _ => {
            removed(name, old, path, out);
            added(name, new, path, out, unrep_done);
        }
    }
}

/// A name with every byte that is not part of a valid, printable character replaced by `?`. Only for reporting.
fn printable(name: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(name);
    text.chars()
        .map(|c| {
            if c.is_control() || c == '\u{fffd}' || c == '\\' || c == '/' {
                '?'
            } else {
                c
            }
        })
        .collect::<String>()
        .into_bytes()
}
