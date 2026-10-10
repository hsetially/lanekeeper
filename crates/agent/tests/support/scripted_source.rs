//! A file system in memory, behind `TreeSource`: scans cost nothing, so tests of timing run in virtual time and tests
//! of deltas can make a file change, vanish or fail at exactly the moment they choose.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent::clock::Clock;
use agent::tree::{
    Entry, FileLeaf, FileRead, MerkleTree, ReadError, ReadRequest, RefreshRequest, Refreshed, ScanError,
    ScanMode, ScanOutcome, ScanStats, Stat, StatTime, TreeSource,
};
use async_trait::async_trait;
use bytes::Bytes;
use domain::ContentHash;
use sha2::{Digest, Sha256};

#[derive(Default)]
struct State {
    /// Content, and the times the file system reports for it.
    files: BTreeMap<String, (Vec<u8>, StatTime)>,
    /// A read returns these bytes instead of the file's, as if the file changed after the scan.
    stale_reads: BTreeMap<String, Vec<u8>>,
    read_errors: BTreeMap<String, ReadError>,
    refresh_gone: Vec<String>,
}

#[derive(Default)]
pub struct ScriptedSource {
    /// Where file times come from. `None`: a fixed instant in 2023, far before any test starts, so a change never looks
    /// recent. With a clock, a file is stamped with the clock's wall time, as a real file system stamps it.
    clock_for_times: Option<Arc<dyn Clock>>,
    state: Mutex<State>,
    clock: AtomicU64,
    scans: AtomicU64,
    reads: AtomicU64,
    largest_read_call: AtomicU64,
    fail_scans: Mutex<Option<ScanError>>,
    modes: Mutex<Vec<ScanMode>>,
}

impl std::fmt::Debug for ScriptedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScriptedSource")
    }
}

fn ino(path: &str) -> u64 {
    let digest = Sha256::digest(path.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().unwrap()) | 1
}

impl ScriptedSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// A source whose files carry the wall time of `clock` when they are written (mtime and ctime both).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock_for_times: Some(clock),
            ..Self::default()
        }
    }

    /// Create or change a file. Each write gets a later modification time, so the stat differs.
    pub fn write(&self, path: &str, bytes: &[u8]) {
        let version = i64::try_from(self.clock.fetch_add(1, Ordering::SeqCst)).unwrap() + 1;
        let time = match &self.clock_for_times {
            // The sub-millisecond part tells two writes in the same millisecond apart.
            Some(clock) => {
                let ms = clock.now().unix_millis();
                StatTime::new(
                    ms.div_euclid(1000),
                    u32::try_from(ms.rem_euclid(1000) * 1_000_000 + version % 1_000_000).unwrap(),
                )
            }
            None => StatTime::new(1_700_000_000 + version, 0),
        };
        self.state
            .lock()
            .unwrap()
            .files
            .insert(path.to_owned(), (bytes.to_vec(), time));
    }

    pub fn remove(&self, path: &str) {
        self.state.lock().unwrap().files.remove(path);
    }

    pub fn remove_all(&self) {
        self.state.lock().unwrap().files.clear();
    }

    /// From now on a read of `path` returns `bytes`, whatever the file holds.
    pub fn serve_stale(&self, path: &str, bytes: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .stale_reads
            .insert(path.to_owned(), bytes.to_vec());
    }

    pub fn fail_reads(&self, path: &str, error: ReadError) {
        self.state
            .lock()
            .unwrap()
            .read_errors
            .insert(path.to_owned(), error);
    }

    /// `refresh` answers `Gone` for this path although the file is still there.
    pub fn refresh_says_gone(&self, path: &str) {
        self.state.lock().unwrap().refresh_gone.push(path.to_owned());
    }

    pub fn fail_next_scans(&self, error: ScanError) {
        *self.fail_scans.lock().unwrap() = Some(error);
    }

    pub fn stop_failing_scans(&self) {
        *self.fail_scans.lock().unwrap() = None;
    }

    pub fn scans(&self) -> u64 {
        self.scans.load(Ordering::SeqCst)
    }

    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }

    /// The most files any single `read` call asked for: how much a delta reads before it sends.
    pub fn largest_read_call(&self) -> u64 {
        self.largest_read_call.load(Ordering::SeqCst)
    }

    /// The modes of every scan so far, in order.
    pub fn modes(&self) -> Vec<ScanMode> {
        self.modes.lock().unwrap().clone()
    }

    pub fn content(&self, path: &str) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(path).map(|(b, _)| b.clone())
    }

    pub fn contents(&self) -> BTreeMap<String, Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .files
            .iter()
            .map(|(p, (b, _))| (p.clone(), b.clone()))
            .collect()
    }

    fn leaf(path: &str, bytes: &[u8], time: StatTime) -> FileLeaf {
        let stat = Stat::new(bytes.len() as u64, time, time, ino(path));
        FileLeaf::new(ContentHash::from_bytes(Sha256::digest(bytes).into()), stat)
    }

    /// The tree of the files as they are now.
    pub fn tree(&self) -> MerkleTree {
        let state = self.state.lock().unwrap();
        MerkleTree::from_leaves(
            state
                .files
                .iter()
                .map(|(path, (bytes, time))| (path.as_str(), Entry::File(Self::leaf(path, bytes, *time)))),
        )
    }
}

#[async_trait]
impl TreeSource for ScriptedSource {
    async fn scan(&self, _previous: Option<MerkleTree>, mode: ScanMode) -> Result<ScanOutcome, ScanError> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        self.modes.lock().unwrap().push(mode);
        if let Some(error) = self.fail_scans.lock().unwrap().clone() {
            return Err(error);
        }
        let tree = self.tree();
        Ok(ScanOutcome {
            new_bytes: tree.retained_bytes(),
            stats: ScanStats {
                files: tree.file_count(),
                hashed: tree.file_count(),
                ..ScanStats::default()
            },
            tree,
        })
    }

    async fn refresh(&self, files: Vec<RefreshRequest>) -> Vec<Refreshed> {
        let state = self.state.lock().unwrap();
        files
            .iter()
            .map(|f| {
                let path = f.path.as_str();
                if state.refresh_gone.iter().any(|p| p == path) {
                    return Refreshed::Gone;
                }
                match state.files.get(path) {
                    None => Refreshed::Gone,
                    Some((bytes, time)) => {
                        let now = Self::leaf(path, bytes, *time);
                        if now.stat == f.leaf.stat {
                            Refreshed::Same
                        } else {
                            Refreshed::Changed(now.with_denied(f.leaf.denied))
                        }
                    }
                }
            })
            .collect()
    }

    async fn read(&self, files: Vec<ReadRequest>) -> Vec<Result<FileRead, ReadError>> {
        self.reads.fetch_add(files.len() as u64, Ordering::SeqCst);
        self.largest_read_call
            .fetch_max(files.len() as u64, Ordering::SeqCst);
        let state = self.state.lock().unwrap();
        files
            .iter()
            .map(|f| {
                let path = f.path.as_str();
                if let Some(error) = state.read_errors.get(path) {
                    return Err(*error);
                }
                let (bytes, time) = state.files.get(path).ok_or(ReadError::NotFound)?;
                let served = state.stale_reads.get(path).unwrap_or(bytes);
                if served.len() as u64 > f.max_bytes {
                    return Err(ReadError::TooLarge);
                }
                let leaf = Self::leaf(path, served, *time);
                Ok(FileRead {
                    bytes: Bytes::copy_from_slice(served),
                    hash: leaf.hash,
                    stat: leaf.stat,
                })
            })
            .collect()
    }
}
