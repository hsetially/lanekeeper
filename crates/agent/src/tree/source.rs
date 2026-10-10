//! Where the scanner gets its facts from (T4): the file system, behind a trait.
//!
//! [`TreeSource`] is the seam between the scan loop and the disk. The loop only knows "scan", "check these files again"
//! and "read these files"; [`FsSource`] answers from the cap-std root on a [`Pool`], and tests answer from memory, so a
//! test of the 10 s / 3 s / 30 s timing runs in virtual time and never waits on `spawn_blocking`.

use std::fmt;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cap_std::fs::Dir;
use domain::{ContentHash, NfsPath};
use rayon::prelude::*;

use super::hash::{Pool, hash_stream, stat_of};
use super::node::{Entry, FileLeaf, MerkleTree, Stat};
use super::walk::{ScanError, ScanMode, ScanOutcome, WalkConfig, walk};
use crate::deny::{DenyList, DenySnapshot};
use crate::root::NfsRoot;

/// A file whose leaf is about to be sent: check that it is still what the tree says.
#[derive(Debug, Clone)]
pub struct RefreshRequest {
    pub path: NfsPath,
    pub leaf: FileLeaf,
}

/// What a file is now, compared with the leaf the tree has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refreshed {
    /// Same size, times and inode: nothing to do.
    Same,
    /// The file changed since the walk. This is its leaf now, read just now.
    Changed(FileLeaf),
    /// The file is gone.
    Gone,
    /// The file could not be checked (not a regular file any more, or an I/O error).
    Failed,
}

/// A file to read for a delta.
#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub path: NfsPath,
    /// The most bytes to accept; a bigger file is refused without being read.
    pub max_bytes: u64,
}

/// The bytes of a file as they were read, with their hash and the stat taken just before the read.
#[derive(Debug, Clone)]
pub struct FileRead {
    pub bytes: Bytes,
    pub hash: ContentHash,
    pub stat: Stat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("the file is gone")]
    NotFound,
    #[error("the file is over the size limit")]
    TooLarge,
    #[error("the path is not a regular file")]
    NotRegular,
    #[error("the file could not be read")]
    Io,
    /// The path matches a deny glob (D79): its bytes are never read for a delta, whoever asks.
    #[error("the path is denied")]
    Denied,
}

/// What the scan loop needs from the disk.
#[async_trait]
pub trait TreeSource: Send + Sync + fmt::Debug + 'static {
    /// The deny globs this source walks and reads by (D79, T11). It is the one list in the agent: the scanner puts the
    /// hub's globs into it, and the file operations and the spool follow the same handle.
    fn deny(&self) -> DenyList;

    /// Walk the tree. `previous` lets the walk skip what has not changed.
    async fn scan(&self, previous: Option<MerkleTree>, mode: ScanMode) -> Result<ScanOutcome, ScanError>;

    /// Check each file against its leaf, rehashing the ones whose stat moved. One answer per request, in order.
    async fn refresh(&self, files: Vec<RefreshRequest>) -> Vec<Refreshed>;

    /// Read each file. One answer per request, in order. A denied path is answered `Denied` and not opened.
    async fn read(&self, files: Vec<ReadRequest>) -> Vec<Result<FileRead, ReadError>>;
}

/// The real source: the cap-std root, walked and read on a worker pool.
pub struct FsSource {
    root: NfsRoot,
    pool: Pool,
    cfg: Arc<WalkConfig>,
}

impl fmt::Debug for FsSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FsSource")
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

impl FsSource {
    pub fn new(root: NfsRoot, pool: Pool, cfg: WalkConfig) -> Self {
        Self {
            root,
            pool,
            cfg: Arc::new(cfg),
        }
    }
}

#[async_trait]
impl TreeSource for FsSource {
    fn deny(&self) -> DenyList {
        self.cfg.deny_list().clone()
    }

    async fn scan(&self, previous: Option<MerkleTree>, mode: ScanMode) -> Result<ScanOutcome, ScanError> {
        let dir = self.root.shared();
        let cfg = Arc::clone(&self.cfg);
        let pool = self.pool.clone();
        self.pool
            .run(move || walk(&pool, &dir, previous.as_ref(), mode, &cfg))
            .await
            .map_err(|_| ScanError::Interrupted)?
    }

    async fn refresh(&self, files: Vec<RefreshRequest>) -> Vec<Refreshed> {
        let dir = self.root.shared();
        let limit = self.cfg.hash_limit();
        let count = files.len();
        let done = self
            .pool
            .run(move || {
                files
                    .par_iter()
                    .map(|f| refresh_one(&dir, f, limit))
                    .collect::<Vec<_>>()
            })
            .await;
        // A worker that panicked answers for none of them: say so for each, and the caller keeps what it has.
        done.unwrap_or_else(|_| vec![Refreshed::Failed; count])
    }

    async fn read(&self, files: Vec<ReadRequest>) -> Vec<Result<FileRead, ReadError>> {
        let dir = self.root.shared();
        let count = files.len();
        // One look at the list for the whole call, taken now: a glob the hub adds a moment later applies to the next call.
        let deny = self.cfg.deny_list().snapshot();
        let done = self
            .pool
            .run(move || {
                files
                    .par_iter()
                    .map(|f| read_one(&dir, &deny, f))
                    .collect::<Vec<_>>()
            })
            .await;
        done.unwrap_or_else(|_| vec![Err(ReadError::Io); count])
    }
}

fn refresh_one(dir: &Dir, request: &RefreshRequest, hash_limit: u64) -> Refreshed {
    let path = request.path.as_str();
    let mut file = match dir.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Refreshed::Gone,
        Err(_) => return Refreshed::Failed,
    };
    let Ok(md) = file.metadata() else {
        return Refreshed::Failed;
    };
    if !md.is_file() {
        return Refreshed::Failed;
    }
    let stat = stat_of(&md);
    if stat == request.leaf.stat {
        return Refreshed::Same;
    }
    match hash_stream(&mut file, hash_limit) {
        Ok(Some((hash, _))) => Refreshed::Changed(FileLeaf::new(hash, stat).with_denied(request.leaf.denied)),
        Ok(None) | Err(_) => Refreshed::Failed,
    }
}

fn read_one(dir: &Dir, deny: &DenySnapshot, request: &ReadRequest) -> Result<FileRead, ReadError> {
    use std::io::Read;

    // Before anything is opened: a denied file's bytes are not read for a delta at all (D79).
    if deny.is_denied(request.path.as_str()) {
        return Err(ReadError::Denied);
    }
    let mut file = dir.open(request.path.as_str()).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ReadError::NotFound
        } else {
            ReadError::Io
        }
    })?;
    let md = file.metadata().map_err(|_| ReadError::Io)?;
    if !md.is_file() {
        return Err(ReadError::NotRegular);
    }
    if md.len() > request.max_bytes {
        return Err(ReadError::TooLarge);
    }
    let stat = stat_of(&md);
    // One byte more than the limit, so a file that grew since the stat is noticed instead of truncated.
    let mut bytes = Vec::with_capacity(usize::try_from(md.len()).unwrap_or(0));
    (&mut file)
        .take(request.max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ReadError::Io)?;
    if bytes.len() as u64 > request.max_bytes {
        return Err(ReadError::TooLarge);
    }
    let hash = super::hash::hash_bytes(&bytes);
    Ok(FileRead {
        bytes: Bytes::from(bytes),
        hash,
        stat,
    })
}

/// A leaf for tests and tools that build trees from bytes.
pub fn leaf_for(bytes: &[u8], stat: Stat) -> Entry {
    Entry::File(FileLeaf::new(super::hash::hash_bytes(bytes), stat))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::tree::hash::hash_bytes;
    use crate::tree::testfs::{create_dir, remove_file, write};

    fn source(dir: &TempDir) -> FsSource {
        FsSource::new(
            NfsRoot::open(dir.path()).unwrap(),
            Pool::new(2).unwrap(),
            WalkConfig::default(),
        )
    }

    fn req(path: &str, max: u64) -> ReadRequest {
        ReadRequest {
            path: NfsPath::parse(path).unwrap(),
            max_bytes: max,
        }
    }

    #[tokio::test]
    async fn read_returns_the_exact_bytes_and_their_hash() {
        let dir = TempDir::new().unwrap();
        let content = b"\xef\xbb\xbfline\r\nline\r\n\x00\xff";
        write(dir.path(), "a/f.bin", content);
        let out = source(&dir).read(vec![req("a/f.bin", 100)]).await;
        let got = out[0].as_ref().unwrap();
        assert_eq!(&got.bytes[..], &content[..]);
        assert_eq!(got.hash, hash_bytes(content));
        assert_eq!(got.stat.size, content.len() as u64);
    }

    #[tokio::test]
    async fn read_refuses_what_it_should_not_read() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "big", &[1_u8; 101]);
        create_dir(dir.path(), "d");
        std::os::unix::fs::symlink("big", dir.path().join("link")).unwrap();
        let outside = TempDir::new().unwrap();
        write(outside.path(), "secret", b"outside");
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("escape")).unwrap();
        let out = source(&dir)
            .read(vec![
                req("big", 100),
                req("d", 100),
                req("missing", 100),
                req("escape", 100),
                req("link", 1000),
            ])
            .await;
        assert_eq!(out[0].as_ref().unwrap_err(), &ReadError::TooLarge);
        assert_eq!(out[1].as_ref().unwrap_err(), &ReadError::NotRegular);
        assert_eq!(out[2].as_ref().unwrap_err(), &ReadError::NotFound);
        // A link out of the root is refused by cap-std itself, never followed.
        assert!(out[3].is_err(), "{:?}", out[3]);
    }

    #[tokio::test]
    async fn refresh_tells_same_changed_and_gone_apart() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "same", b"s");
        write(dir.path(), "changes", b"before");
        write(dir.path(), "gone", b"g");
        let src = source(&dir);
        let tree = src.scan(None, ScanMode::Full).await.unwrap().tree;
        let leaf = |name: &str| match tree.get(name) {
            Some(Entry::File(l)) => *l,
            other => panic!("{other:?}"),
        };
        write(dir.path(), "changes", b"after, longer");
        remove_file(dir.path(), "gone");
        let answers = src
            .refresh(
                ["same", "changes", "gone"]
                    .into_iter()
                    .map(|n| RefreshRequest {
                        path: NfsPath::parse(n).unwrap(),
                        leaf: leaf(n),
                    })
                    .collect(),
            )
            .await;
        assert_eq!(answers[0], Refreshed::Same);
        assert!(matches!(&answers[1], Refreshed::Changed(l) if l.hash == hash_bytes(b"after, longer")));
        assert_eq!(answers[2], Refreshed::Gone);
    }
}
