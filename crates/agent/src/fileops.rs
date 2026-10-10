//! File operations (T5, S11, S17, rule 11): read, write and delete one file, byte-exact and never blind.
//!
//! Everything here goes through the cap-std [`Dir`] of the NFS root. A path is an [`NfsPath`] (already free of `..`,
//! absolute prefixes, NUL and backslashes) and is then walked one component at a time through that handle:
//!
//! - **No symlink is followed**, not even one that stays inside the root (S17). Each directory on the way and the file
//!   itself are `lstat`ed first; a link is refused with [`DeniedReason::Symlink`]. cap-std additionally refuses any
//!   escape, and the handle that is opened is compared with the entry that was listed (same device and inode), so a
//!   directory swapped for a link between the `lstat` and the open is noticed and refused.
//! - **A write is checked before it is made.** The expected hash ("must not exist", or a hash) is compared with the
//!   hash of what is on disk, in constant time. A mismatch is a [`FileError::Conflict`] that carries the current hash
//!   and writes nothing.
//! - **A write is atomic and durable.** The bytes go to a temporary file in the same directory, are fsynced, get the
//!   original file's mode, and then replace the target by rename; the directory is fsynced last. A reader sees the old
//!   file or the new one, never half of either, and a crash at any point leaves the old file. A file that must not
//!   exist is put in place with a hard link instead of a rename, which fails if the name has appeared meanwhile, so a
//!   create never overwrites.
//! - **Bytes are never interpreted.** Line endings, a byte-order mark and binary content come out exactly as they
//!   went in (rule 11).
//! - **Nothing is created beyond the file.** A missing parent directory is `NotFound` (decision A12): a new folder
//!   needs a config-server restart (C11) and must be an explicit act.
//!
//! Operations on one path are serialised through a fixed number of lock stripes ([`PathLocks`]), so memory does not
//! grow with the number of paths, and two writes to the same file never interleave. Every operation is bounded in time:
//! the blocking work runs in `spawn_blocking` and the whole call is cut off after [`OP_TIMEOUT`], so a hung NFS mount
//! turns into an `Io` answer instead of a stuck request. The lock is released only when the blocking work has really
//! finished, so a timed-out write can never overlap the next one.
//!
//! What the tree learns: a successful write or delete tells the [`TreeEdits`] sink at once, with the stat of the file
//! as it is on disk now, so the next walk finds it unchanged and reads nothing. It is not spooled (D80): the hub did it
//! and knows; the root in the next heartbeat shows the difference to any other hub replica.
//!
//! Residual risk, stated in the threat model: between "the hash was checked" and "the file was replaced" another
//! writer on the export can still change the file, because NFS offers no compare-and-swap. The window is the time of
//! one rename. Creates are exempt (hard link); replaces and deletes are not.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use cap_std::fs::{
    Dir, File, Metadata, MetadataExt, OpenOptions, OpenOptionsExt, Permissions, PermissionsExt,
};
use domain::{ContentHash, Expected, NfsPath, OpError};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::time::timeout;
use tracing::warn;

use crate::config::limits::MAX_FILE_BYTES;
use crate::root::NfsRoot;
use crate::tree::hash::{HASH_LIMIT, hash_bytes, hash_stream, stat_of};
use crate::tree::{FileLeaf, Stat};

/// The longest one operation may take, waiting for its lock included.
pub const OP_TIMEOUT: Duration = Duration::from_secs(30);
/// How many locks the paths share. Fixed, so the lock table never grows (rule 5).
pub const LOCK_STRIPES: usize = 64;
/// The mode of a file that did not exist before. An existing file keeps its own.
pub const NEW_FILE_MODE: u32 = 0o644;
/// Temporary files start with this. The walker ignores the name (A9), and the hub may not use it.
pub const TMP_PREFIX: &str = ".lanekeeper-tmp-";
/// NFS silly-rename files: the client's own, never the hub's.
const NFS_SILLY_PREFIX: &str = ".nfs";
/// How often a temporary name is drawn again when it happens to exist.
const TMP_ATTEMPTS: usize = 4;

// ------------------------------------------------------------------------------------------------ errors

/// Why a path was refused outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeniedReason {
    /// A symbolic link on the way or at the end. Never followed (S17).
    #[error("a symbolic link")]
    Symlink,
    /// A name the agent keeps for itself: its temporary files and NFS silly-rename files.
    #[error("a reserved name")]
    ReservedName,
}

/// Why a path is not something this operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnsupportedReason {
    /// A directory, a socket, a device.
    #[error("not a regular file")]
    NotRegular,
    /// Over the size limit (2 MiB for content that travels in a message).
    #[error("over the size limit")]
    TooLarge,
}

/// Why a file operation did not happen. None of these carries a path or the text of an OS error: the hub gets a code
/// (see [`FileError::code`]), the log gets the path and the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    /// The expected hash did not match. `current` is the hash found, `None` if the file is absent.
    #[error("the file is not what was expected")]
    Conflict { current: Option<ContentHash> },
    /// The file, or the directory it would be in, does not exist.
    #[error("no such file or directory")]
    NotFound,
    #[error("refused: {0}")]
    Denied(DeniedReason),
    #[error("not supported: {0}")]
    Unsupported(UnsupportedReason),
    /// The file system failed, or the operation timed out.
    #[error("the file system failed ({0:?})")]
    Io(io::ErrorKind),
}

impl FileError {
    /// The answer code the hub sees.
    pub fn code(&self) -> OpError {
        match self {
            Self::Conflict { .. } => OpError::Conflict,
            Self::NotFound => OpError::NotFound,
            Self::Denied(_) => OpError::Denied,
            Self::Unsupported(_) => OpError::Unsupported,
            Self::Io(_) => OpError::Io,
        }
    }

    /// The hash to send back with the answer: only a conflict has one.
    pub fn current_hash(&self) -> Option<ContentHash> {
        match self {
            Self::Conflict { current } => *current,
            _ => None,
        }
    }
}

fn io_error(error: &io::Error) -> FileError {
    FileError::Io(error.kind())
}

/// An error from looking something up: "not there" is its own answer.
fn lookup_error(error: &io::Error) -> FileError {
    if error.kind() == io::ErrorKind::NotFound {
        FileError::NotFound
    } else {
        io_error(error)
    }
}

/// What a write or delete leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpOutcome {
    /// The hash on NFS after the operation; `None` after a delete.
    pub current_hash: Option<ContentHash>,
}

/// What a read returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub bytes: Bytes,
    pub hash: ContentHash,
}

// ------------------------------------------------------------------------------------------------ seams

/// Where a successful write or delete tells the tree what happened. The scanner implements it.
#[async_trait]
pub trait TreeEdits: Send + Sync + fmt::Debug {
    /// `path` now holds the file `leaf` describes.
    async fn written(&self, path: &NfsPath, leaf: FileLeaf);
    /// `path` is gone.
    async fn removed(&self, path: &NfsPath);
}

/// A simulated crash, returned by a [`WriteHooks`] to stop a write dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crashed;

/// Points where a test can interfere with a write. Production uses [`NoHooks`], which does nothing.
pub trait WriteHooks: Send + Sync + 'static {
    /// The new bytes are in a synced temporary file and nothing has replaced the target yet. Returning `Err` ends the
    /// write at once and cleans nothing up, as a killed process would.
    fn before_rename(&self, _path: &NfsPath) -> Result<(), Crashed> {
        Ok(())
    }
}

/// No interference.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl WriteHooks for NoHooks {}

// ------------------------------------------------------------------------------------------------ locks

/// The guard of one lock stripe. The operation holds it until its blocking work has finished.
pub type PathGuard = OwnedMutexGuard<()>;

/// Serialises operations on the same path with a fixed number of locks.
///
/// Two different paths may share a stripe and wait for each other for the length of one operation; that is the price
/// of a table that cannot grow. The stripe is chosen by a hasher with a random key, so a hub cannot pick names that
/// all land on one stripe.
pub struct PathLocks {
    stripes: Box<[Arc<Mutex<()>>]>,
    hasher: RandomState,
}

impl fmt::Debug for PathLocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathLocks")
            .field("stripes", &self.stripes.len())
            .finish_non_exhaustive()
    }
}

impl Default for PathLocks {
    fn default() -> Self {
        Self::new()
    }
}

impl PathLocks {
    pub fn new() -> Self {
        Self {
            stripes: (0..LOCK_STRIPES).map(|_| Arc::new(Mutex::new(()))).collect(),
            hasher: RandomState::new(),
        }
    }

    /// How many locks there are, whatever the number of paths.
    pub fn stripes(&self) -> usize {
        self.stripes.len()
    }

    /// The stripe `path` takes.
    pub fn stripe_of(&self, path: &NfsPath) -> usize {
        let stripes = u64::try_from(self.stripes.len()).unwrap_or(1).max(1);
        usize::try_from(self.hasher.hash_one(path.as_str()) % stripes).unwrap_or(0)
    }

    /// Wait for the lock of `path`'s stripe.
    pub async fn lock(&self, path: &NfsPath) -> PathGuard {
        // `stripes` is never empty, and `stripe_of` is below its length, so the fallback is never taken.
        let stripe = self
            .stripes
            .get(self.stripe_of(path))
            .or_else(|| self.stripes.first())
            .map(Arc::clone)
            .unwrap_or_default();
        stripe.lock_owned().await
    }
}

// ------------------------------------------------------------------------------------------------ the operations

/// Reads, writes and deletes files under the NFS root.
pub struct FileOps<H: WriteHooks = NoHooks> {
    root: NfsRoot,
    locks: PathLocks,
    edits: Arc<dyn TreeEdits>,
    hooks: Arc<H>,
}

impl<H: WriteHooks> fmt::Debug for FileOps<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileOps")
            .field("locks", &self.locks)
            .finish_non_exhaustive()
    }
}

impl FileOps<NoHooks> {
    pub fn new(root: NfsRoot, edits: Arc<dyn TreeEdits>) -> Self {
        Self::with_hooks(root, edits, NoHooks)
    }
}

impl<H: WriteHooks> FileOps<H> {
    pub fn with_hooks(root: NfsRoot, edits: Arc<dyn TreeEdits>, hooks: H) -> Self {
        Self {
            root,
            locks: PathLocks::new(),
            edits,
            hooks: Arc::new(hooks),
        }
    }

    pub fn locks(&self) -> &PathLocks {
        &self.locks
    }

    /// Read a file: its bytes and their hash. A file over `max_bytes` is refused without being read. The caller's limit
    /// can only lower the 2 MiB cap ([`MAX_FILE_BYTES`]), never raise it.
    pub async fn read(&self, path: &NfsPath, max_bytes: u64) -> Result<FileContent, FileError> {
        let max_bytes = max_bytes.min(MAX_FILE_BYTES);
        let target = path.clone();
        self.blocking(path, move |root| read_blocking(root, &target, max_bytes))
            .await
    }

    /// Write `bytes` to `path` if the file is what `expected` says. `max_bytes` is the largest file the agent accepts;
    /// like for a read, it can only lower the 2 MiB cap.
    pub async fn write(
        &self,
        path: &NfsPath,
        expected: Expected,
        bytes: Bytes,
        max_bytes: u64,
    ) -> Result<OpOutcome, FileError> {
        // Before any lock and any I/O: the size is on the message.
        if bytes.len() as u64 > max_bytes.min(MAX_FILE_BYTES) {
            return Err(FileError::Unsupported(UnsupportedReason::TooLarge));
        }
        let target = path.clone();
        let hooks = Arc::clone(&self.hooks);
        let done = self
            .blocking(path, move |root| {
                write_blocking(root, hooks.as_ref(), &target, expected, &bytes)
            })
            .await?;
        // The file has changed, whether or not the directory could be synced: the tree is told either way.
        self.edits.written(path, done.leaf).await;
        if let Some(kind) = done.dir_sync_failed {
            return Err(FileError::Io(kind));
        }
        Ok(OpOutcome {
            current_hash: Some(done.leaf.hash),
        })
    }

    /// Delete `path` if its hash is `expected`. There is no "delete whatever is there".
    pub async fn delete(&self, path: &NfsPath, expected: ContentHash) -> Result<OpOutcome, FileError> {
        let target = path.clone();
        let dir_sync_failed = self
            .blocking(path, move |root| delete_blocking(root, &target, expected))
            .await?;
        self.edits.removed(path).await;
        if let Some(kind) = dir_sync_failed {
            return Err(FileError::Io(kind));
        }
        Ok(OpOutcome { current_hash: None })
    }

    /// Take the path's lock, then run `work` on the blocking pool, all within [`OP_TIMEOUT`]. The lock moves into the
    /// closure, so it is released when the work has finished and not when the call gives up.
    async fn blocking<T: Send + 'static>(
        &self,
        path: &NfsPath,
        work: impl FnOnce(&Dir) -> Result<T, FileError> + Send + 'static,
    ) -> Result<T, FileError> {
        let timed_out = || FileError::Io(io::ErrorKind::TimedOut);
        let guard = timeout(OP_TIMEOUT, self.locks.lock(path))
            .await
            .map_err(|_| timed_out())?;
        let dir = self.root.shared();
        let task = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            work(&dir)
        });
        match timeout(OP_TIMEOUT, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_panicked_or_cancelled)) => Err(FileError::Io(io::ErrorKind::Other)),
            Err(_) => {
                warn!(path = %path, "a file operation timed out; the blocking work may still be running");
                Err(timed_out())
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ blocking parts

/// Names the hub may not name: the agent's temporary files and NFS silly-rename files.
fn is_reserved(component: &str) -> bool {
    component.starts_with(TMP_PREFIX) || component.starts_with(NFS_SILLY_PREFIX)
}

/// The directory that holds `path`, opened one component at a time without following a link, and the file's name.
fn locate<'p>(root: &Dir, path: &'p NfsPath) -> Result<(Dir, &'p str), FileError> {
    let text = path.as_str();
    if text.split('/').any(is_reserved) {
        return Err(FileError::Denied(DeniedReason::ReservedName));
    }
    let (parents, name) = text
        .rsplit_once('/')
        .map_or(("", text), |(dirs, name)| (dirs, name));
    let mut dir = root.try_clone().map_err(|e| io_error(&e))?;
    if parents.is_empty() {
        return Ok((dir, name));
    }
    for component in parents.split('/') {
        let listed = dir.symlink_metadata(component).map_err(|e| lookup_error(&e))?;
        if listed.file_type().is_symlink() {
            return Err(FileError::Denied(DeniedReason::Symlink));
        }
        if !listed.is_dir() {
            // A file where a directory would have to be: the parent directory does not exist.
            return Err(FileError::NotFound);
        }
        let next = dir.open_dir(component).map_err(|e| lookup_error(&e))?;
        let opened = next.dir_metadata().map_err(|e| io_error(&e))?;
        if !opened.is_dir() || opened.ino() != listed.ino() || opened.dev() != listed.dev() {
            // Swapped for something else between the lstat and the open. Whatever it is, do not use it.
            return Err(FileError::Io(io::ErrorKind::InvalidData));
        }
        dir = next;
    }
    Ok((dir, name))
}

/// What is at the end of a path.
enum Target {
    Absent,
    Regular(Metadata),
}

/// Look at `name` in `dir` without following it.
fn inspect(dir: &Dir, name: &str) -> Result<Target, FileError> {
    match dir.symlink_metadata(name) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Target::Absent),
        Err(e) => Err(io_error(&e)),
        Ok(md) if md.file_type().is_symlink() => Err(FileError::Denied(DeniedReason::Symlink)),
        Ok(md) if md.is_file() => Ok(Target::Regular(md)),
        Ok(_) => Err(FileError::Unsupported(UnsupportedReason::NotRegular)),
    }
}

/// Open the regular file that `listed` described, and check that it is that file.
fn open_listed(dir: &Dir, name: &str, listed: &Metadata) -> Result<File, FileError> {
    let file = dir.open(name).map_err(|e| lookup_error(&e))?;
    let now = file.metadata().map_err(|e| io_error(&e))?;
    if !now.is_file() || now.ino() != listed.ino() || now.dev() != listed.dev() {
        return Err(FileError::Io(io::ErrorKind::InvalidData));
    }
    Ok(file)
}

/// The hash of a regular file on disk, streamed so the file is never held in memory.
fn hash_listed(dir: &Dir, name: &str, listed: &Metadata) -> Result<ContentHash, FileError> {
    let mut file = open_listed(dir, name, listed)?;
    match hash_stream(&mut file, HASH_LIMIT) {
        Ok(Some((hash, _))) => Ok(hash),
        Ok(None) => Err(FileError::Unsupported(UnsupportedReason::TooLarge)),
        Err(e) => Err(io_error(&e)),
    }
}

/// True when the file is what the caller expected. The hashes are compared in constant time.
fn expectation_met(expected: Expected, current: Option<ContentHash>) -> bool {
    match (expected, current) {
        (Expected::Absent, None) => true,
        (Expected::Hash { hash }, Some(current)) => {
            bool::from(hash.as_bytes().as_slice().ct_eq(current.as_bytes().as_slice()))
        }
        _ => false,
    }
}

fn read_blocking(root: &Dir, path: &NfsPath, max_bytes: u64) -> Result<FileContent, FileError> {
    let (dir, name) = locate(root, path)?;
    let Target::Regular(listed) = inspect(&dir, name)? else {
        return Err(FileError::NotFound);
    };
    if listed.len() > max_bytes {
        return Err(FileError::Unsupported(UnsupportedReason::TooLarge));
    }
    let mut file = open_listed(&dir, name, &listed)?;
    // One byte more than the limit, so a file that grew since the stat is noticed instead of truncated.
    let mut bytes = Vec::with_capacity(usize::try_from(listed.len()).unwrap_or(0));
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(&e))?;
    if bytes.len() as u64 > max_bytes {
        return Err(FileError::Unsupported(UnsupportedReason::TooLarge));
    }
    let hash = hash_bytes(&bytes);
    Ok(FileContent {
        bytes: Bytes::from(bytes),
        hash,
    })
}

/// What a finished write reports back to the async side.
struct Written {
    leaf: FileLeaf,
    /// The kind of the error if the directory could not be synced after the file was in place.
    dir_sync_failed: Option<io::ErrorKind>,
}

fn write_blocking<H: WriteHooks>(
    root: &Dir,
    hooks: &H,
    path: &NfsPath,
    expected: Expected,
    bytes: &[u8],
) -> Result<Written, FileError> {
    let (dir, name) = locate(root, path)?;
    let (current, permissions) = match inspect(&dir, name)? {
        Target::Absent => (None, Permissions::from_mode(NEW_FILE_MODE)),
        Target::Regular(listed) => (Some(hash_listed(&dir, name, &listed)?), listed.permissions()),
    };
    if !expectation_met(expected, current) {
        return Err(FileError::Conflict { current });
    }
    let new_hash = hash_bytes(bytes);

    let (tmp_name, mut tmp) = create_temp(&dir)?;
    if let Err(error) = stage(&mut tmp, bytes, permissions) {
        drop(tmp);
        // Best effort: if this fails too, the walker ignores the name.
        let _ = dir.remove_file(&tmp_name);
        return Err(io_error(&error));
    }
    drop(tmp);

    if hooks.before_rename(path).is_err() {
        // A crash cleans nothing up. The temporary file stays, and the target is as it was.
        return Err(FileError::Io(io::ErrorKind::Interrupted));
    }

    let placed = match expected {
        // A create must not replace a file that appeared since the check: linking fails if the name exists.
        Expected::Absent => dir.hard_link(&tmp_name, &dir, name),
        Expected::Hash { .. } => dir.rename(&tmp_name, &dir, name),
    };
    match placed {
        Ok(()) => {}
        Err(error) => {
            let _ = dir.remove_file(&tmp_name);
            if error.kind() == io::ErrorKind::AlreadyExists {
                return Err(FileError::Conflict {
                    current: current_hash(&dir, name),
                });
            }
            return Err(io_error(&error));
        }
    }
    if matches!(expected, Expected::Absent) {
        // The name is linked; the temporary one has done its job.
        let _ = dir.remove_file(&tmp_name);
    }
    let dir_sync_failed = sync_dir(&dir).err().map(|e| e.kind());

    // The stat of what is there now, so the tree's leaf matches the next walk.
    let placed_md = dir.symlink_metadata(name).map_err(|e| io_error(&e))?;
    let stat: Stat = stat_of(&placed_md);
    Ok(Written {
        leaf: FileLeaf::new(new_hash, stat),
        dir_sync_failed,
    })
}

fn delete_blocking(
    root: &Dir,
    path: &NfsPath,
    expected: ContentHash,
) -> Result<Option<io::ErrorKind>, FileError> {
    let (dir, name) = locate(root, path)?;
    let Target::Regular(listed) = inspect(&dir, name)? else {
        return Err(FileError::NotFound);
    };
    let current = hash_listed(&dir, name, &listed)?;
    if !expectation_met(Expected::Hash { hash: expected }, Some(current)) {
        return Err(FileError::Conflict {
            current: Some(current),
        });
    }
    dir.remove_file(name).map_err(|e| lookup_error(&e))?;
    Ok(sync_dir(&dir).err().map(|e| e.kind()))
}

/// The hash of the regular file at `name`, if there is one and it can be read. For the answer to a lost race.
fn current_hash(dir: &Dir, name: &str) -> Option<ContentHash> {
    match inspect(dir, name) {
        Ok(Target::Regular(listed)) => hash_listed(dir, name, &listed).ok(),
        _ => None,
    }
}

/// A new, empty temporary file in `dir`, readable by its owner only until the content and mode are in place.
fn create_temp(dir: &Dir) -> Result<(String, File), FileError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut last = io::ErrorKind::AlreadyExists;
    for _ in 0..TMP_ATTEMPTS {
        let name = format!("{TMP_PREFIX}{:032x}", fastrand::u128(..));
        match dir.open_with(&name, &options) {
            Ok(file) => return Ok((name, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e.kind(),
            Err(e) => return Err(lookup_error(&e)),
        }
    }
    Err(FileError::Io(last))
}

/// Put the content into the temporary file, give it the final mode and make it durable.
fn stage(file: &mut File, bytes: &[u8], permissions: Permissions) -> io::Result<()> {
    file.write_all(bytes)?;
    file.set_permissions(permissions)?;
    file.sync_all()
}

/// Make the directory entries durable.
fn sync_dir(dir: &Dir) -> io::Result<()> {
    dir.open(".")?.sync_all()
}
