//! Hashes of content and of directory trees (T4, D65, P7).
//!
//! A swimlane's effective files are hashed into a Merkle tree, one hash for every directory, so that comparing two
//! swimlanes can skip every subtree whose hash is the same on both sides.
//!
//! # The encoding
//!
//! The hub may need to reproduce a hash that it stored, so the bytes are fixed (pinned by a snapshot in
//! `tests/tree_hash.rs`, and by a test that computes the same hash independently from this description).
//!
//! - The hash of a **file** is the hash the caller gives: the effective hash of the logical file (`Effective::hash`, or for
//!   a file that is not a property source, `normalised_hash` of the file that is served).
//! - The hash of a **directory** is SHA-256 over its entries sorted by raw name bytes, each entry
//!   `u32_le(name length) || name || kind || 32-byte hash`, where the kind is `0` for a file and `1` for a directory.
//!   Names compare as bytes inside their own directory, not as parts of a path: with `a/x.yml`, `a-b.yml` and `a.yml` the
//!   root's entries are `a`, `a-b.yml`, `a.yml`.
//! - The hash of **several tenants** (Q34, `swimlane_hash`) is the common hash if every tenant agrees, and otherwise the
//!   hash of the tenants sorted by name, encoded like directory entries of kind `0`.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use domain::{ContentHash, FileClass, LogicalFile, NfsPath, TenantId};
use sha2::{Digest, Sha256};

use crate::bytes::normalise_eol;
use crate::effective::len32;
use crate::error::TreeHashError;

/// The hash of a file's content for comparing: SHA-256 of the bytes with line endings normalised for a `structured` or
/// `text` file (a file that differs only in line endings is the same file, `docs/domain-model.md`), and of the bytes as
/// they are for a `binary` or `denied` one.
pub fn normalised_hash(data: &Bytes, class: FileClass) -> ContentHash {
    let digest: [u8; 32] = match class {
        FileClass::Structured | FileClass::Text => Sha256::digest(normalise_eol(data)).into(),
        FileClass::Binary | FileClass::Denied => Sha256::digest(data).into(),
    };
    ContentHash::from_bytes(digest)
}

/// The hash of every directory of a tree of files, the root included.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeHashes {
    dirs: BTreeMap<NfsPath, ContentHash>,
}

impl TreeHashes {
    /// The hash of a directory (`NfsPath::root()` for the whole tree), or `None` for one that holds no file.
    pub fn get(&self, dir: &NfsPath) -> Option<ContentHash> {
        self.dirs.get(dir).copied()
    }

    /// The hash of the whole tree, or `None` for a tree with no file.
    pub fn root(&self) -> Option<ContentHash> {
        self.get(&NfsPath::root())
    }

    /// Every directory with its hash, in path order (the root first).
    pub fn iter(&self) -> impl Iterator<Item = (&NfsPath, &ContentHash)> {
        self.dirs.iter()
    }
}

/// One entry of a directory while its hash is built.
struct Entry<'a> {
    name: &'a str,
    directory: bool,
    hash: ContentHash,
}

/// The hash of every directory of the tree made by `files`, each a logical file with its effective hash. The order of the
/// input does not matter.
///
/// # Errors
///
/// `Inconsistent` if a logical file is listed twice, or a path is both a file and a directory.
pub fn effective_tree_hash(files: &[(LogicalFile, ContentHash)]) -> Result<TreeHashes, TreeHashError> {
    // The files of each directory, and the directories of each directory. A directory is named by its path as text
    // (the root is "").
    let mut file_entries: BTreeMap<&str, Vec<Entry<'_>>> = BTreeMap::new();
    let mut subdirs: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (file, hash) in files {
        let path = file.as_str();
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        file_entries.entry(dir).or_default().push(Entry {
            name,
            directory: false,
            hash: *hash,
        });
        // Every ancestor is a directory of its parent. Stop at the first one that is known: its ancestors are too.
        let mut child = dir;
        while !child.is_empty() {
            let (parent, child_name) = child.rsplit_once('/').unwrap_or(("", child));
            if !subdirs.entry(parent).or_default().insert(child_name) {
                break;
            }
            child = parent;
        }
    }

    // Deepest directories first: a directory's hash needs the hashes of its subdirectories.
    let mut dirs: BTreeSet<&str> = file_entries.keys().copied().collect();
    dirs.extend(subdirs.keys().copied());
    let mut order: Vec<&str> = dirs.into_iter().collect();
    order.sort_by_key(|d| std::cmp::Reverse(depth(d)));

    let mut hashes: BTreeMap<&str, ContentHash> = BTreeMap::new();
    for dir in order {
        let mut entries: Vec<Entry<'_>> = file_entries.remove(dir).unwrap_or_default();
        if let Some(children) = subdirs.get(dir) {
            for name in children {
                let path = if dir.is_empty() {
                    (*name).to_owned()
                } else {
                    format!("{dir}/{name}")
                };
                let hash = hashes
                    .get(path.as_str())
                    .copied()
                    .ok_or(TreeHashError::Inconsistent)?;
                entries.push(Entry {
                    name,
                    directory: true,
                    hash,
                });
            }
        }
        entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        // A name twice in one directory: a file listed twice, or a file and a directory of the same name.
        if entries.windows(2).any(|w| w[0].name == w[1].name) {
            return Err(TreeHashError::Inconsistent);
        }
        let mut sha = Sha256::new();
        for e in &entries {
            update_entry(&mut sha, e.name, e.directory, &e.hash);
        }
        hashes.insert(dir, ContentHash::from_bytes(sha.finalize().into()));
    }

    let mut out = BTreeMap::new();
    for (dir, hash) in hashes {
        let path = NfsPath::parse_prefix(dir).map_err(|_| TreeHashError::Inconsistent)?;
        out.insert(path, hash);
    }
    Ok(TreeHashes { dirs: out })
}

fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.bytes().filter(|&b| b == b'/').count() + 1
    }
}

fn update_entry(hasher: &mut Sha256, name: &str, directory: bool, hash: &ContentHash) {
    hasher.update(len32(name.len()).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update([u8::from(directory)]);
    hasher.update(hash.as_bytes());
}

/// A node of a directory of an [`IncrementalTree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    File(ContentHash),
    /// A subdirectory; its hash is in `IncrementalTree::hashes`.
    Dir,
}

/// The tree of `effective_tree_hash`, kept between runs so that a change to a few files rehashes only their directories and
/// the directories above them (P5). Every directory keeps its entries and its hash. `set` changes a file and marks the
/// directories above it, and `finish` rehashes the marked ones, deepest first, so that a directory is hashed once however
/// many of its files changed. The result is the same as `effective_tree_hash` of the same files (a property test in
/// `tests/batch.rs` holds the two together), whatever order the changes were made in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct IncrementalTree {
    /// Directory (as text, the root is `""`) to its entries by name. A directory with no entries is not kept.
    entries: BTreeMap<String, BTreeMap<String, Node>>,
    /// Directory to its hash, for every directory in `entries`.
    hashes: BTreeMap<String, ContentHash>,
    /// Directories whose hash is out of date: after `finish` there are none.
    dirty: BTreeSet<String>,
}

impl IncrementalTree {
    /// Sets (`Some`) or removes (`None`) the effective hash of one logical file. The directories above it are rehashed by the
    /// next `finish`.
    ///
    /// # Errors
    ///
    /// `Inconsistent` if a name is both a file and a directory.
    pub(crate) fn set(&mut self, file: &LogicalFile, hash: Option<ContentHash>) -> Result<(), TreeHashError> {
        let path = file.as_str();
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        if let Some(hash) = hash {
            self.link(dir)?;
            let slot = self.entries.entry(dir.to_owned()).or_default();
            if matches!(slot.get(name), Some(Node::Dir)) {
                return Err(TreeHashError::Inconsistent);
            }
            slot.insert(name.to_owned(), Node::File(hash));
        } else {
            // A file that is not there is already removed, and marks nothing.
            let Some(slot) = self.entries.get_mut(dir) else {
                return Ok(());
            };
            if !matches!(slot.get(name), Some(Node::File(_))) {
                return Ok(());
            }
            slot.remove(name);
        }
        self.mark(dir);
        Ok(())
    }

    /// Makes `dir` and each directory above it a known directory, with a `Dir` node in its parent.
    fn link(&mut self, dir: &str) -> Result<(), TreeHashError> {
        self.entries.entry(dir.to_owned()).or_default();
        let mut child = dir;
        while !child.is_empty() {
            let (parent, name) = child.rsplit_once('/').unwrap_or(("", child));
            let slot = self.entries.entry(parent.to_owned()).or_default();
            match slot.get(name) {
                Some(Node::File(_)) => return Err(TreeHashError::Inconsistent),
                Some(Node::Dir) => break,
                None => {
                    slot.insert(name.to_owned(), Node::Dir);
                }
            }
            child = parent;
        }
        Ok(())
    }

    /// Marks `dir` and each directory above it. A directory that is marked has its ancestors marked, so the walk stops at the
    /// first one that already is.
    fn mark(&mut self, dir: &str) {
        let mut current = dir;
        loop {
            if !self.dirty.insert(current.to_owned()) || current.is_empty() {
                break;
            }
            current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }

    /// Rehashes the marked directories, deepest first, and drops those that no longer hold a file.
    ///
    /// # Errors
    ///
    /// `Inconsistent` if a directory node has no hash (a bug; the tree is left as it is).
    pub(crate) fn finish(&mut self) -> Result<(), TreeHashError> {
        let mut order: Vec<String> = std::mem::take(&mut self.dirty).into_iter().collect();
        order.sort_by_key(|d| std::cmp::Reverse(depth(d)));
        for dir in order {
            let empty = self.entries.get(&dir).is_none_or(BTreeMap::is_empty);
            if empty {
                self.entries.remove(&dir);
                self.hashes.remove(&dir);
                // Its entry in the parent goes too (the parent is marked, as every directory above a marked one is).
                if !dir.is_empty() {
                    let (parent, name) = dir.rsplit_once('/').unwrap_or(("", dir.as_str()));
                    if let Some(slot) = self.entries.get_mut(parent) {
                        if slot.get(name) == Some(&Node::Dir) {
                            slot.remove(name);
                        }
                    }
                }
                continue;
            }
            let mut sha = Sha256::new();
            if let Some(entries) = self.entries.get(&dir) {
                for (name, node) in entries {
                    match node {
                        Node::File(hash) => update_entry(&mut sha, name, false, hash),
                        Node::Dir => {
                            let child = if dir.is_empty() {
                                name.clone()
                            } else {
                                format!("{dir}/{name}")
                            };
                            let hash = self.hashes.get(&child).ok_or(TreeHashError::Inconsistent)?;
                            update_entry(&mut sha, name, true, hash);
                        }
                    }
                }
            }
            self.hashes
                .insert(dir, ContentHash::from_bytes(sha.finalize().into()));
        }
        Ok(())
    }

    /// The hash of the whole tree, or `None` for a tree with no file.
    pub(crate) fn root(&self) -> Option<ContentHash> {
        self.hashes.get("").copied()
    }

    /// The hash of one directory, or `None` for one that holds no file.
    pub(crate) fn get(&self, dir: &NfsPath) -> Option<ContentHash> {
        self.hashes.get(dir.as_str()).copied()
    }

    /// The hashes as `effective_tree_hash` returns them.
    pub(crate) fn to_hashes(&self) -> TreeHashes {
        TreeHashes {
            dirs: self
                .hashes
                .iter()
                .filter_map(|(dir, hash)| Some((NfsPath::parse_prefix(dir).ok()?, *hash)))
                .collect(),
        }
    }
}

/// The value of one logical file in a swimlane that has several tenants (Q34): the hash all of them agree on, or, when
/// they do not, a hash over the tenants and their hashes, sorted by tenant. `None` when there is no tenant.
pub fn swimlane_hash(per_tenant: &[(TenantId, ContentHash)]) -> Option<ContentHash> {
    let (first, rest) = per_tenant.split_first()?;
    if rest.iter().all(|(_, hash)| *hash == first.1) {
        return Some(first.1);
    }
    let mut sorted: Vec<&(TenantId, ContentHash)> = per_tenant.iter().collect();
    sorted.sort();
    let mut hasher = Sha256::new();
    for (tenant, hash) in sorted {
        update_entry(&mut hasher, tenant.as_str(), false, hash);
    }
    Some(ContentHash::from_bytes(hasher.finalize().into()))
}
