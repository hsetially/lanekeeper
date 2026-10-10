//! A spool on a temporary directory, with the helpers the spool tests share (T9).
//!
//! Everything runs with [`IoMode::Inline`]: the spool's file operations happen on the polling thread, so a test under
//! `tokio::time::pause` never sees virtual time jump while a `spawn_blocking` task is still working.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent::clock::Clock;
use agent::spool::{IoMode, Recovery, Spool, SpoolLimits, SpoolOptions, SpoolVolume};
use agent::transport::outbox::{self, OutboxLimits};
use bytes::Bytes;
use domain::{ContentHash, NfsPath, ScanDelta, ScanEntry, Timestamp};
use proto::convert::FromAgent;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::clock::TestClock;
use super::test_ca::T0_MS;

pub fn hash_of(content: &[u8]) -> ContentHash {
    ContentHash::from_bytes(Sha256::digest(content).into())
}

/// A root that depends only on `n`, so a chain of deltas has distinct, checkable roots.
pub fn root(n: u64) -> ContentHash {
    hash_of(format!("root {n}").as_bytes())
}

pub fn file(path: &str, content: &[u8], observed_ms: i64) -> ScanEntry {
    ScanEntry {
        path: NfsPath::parse(path).unwrap(),
        hash: hash_of(content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(observed_ms),
        observed_at: Timestamp::from_unix_millis(observed_ms),
        denied: false,
        bytes: Some(Bytes::copy_from_slice(content)),
    }
}

/// A denied file: name, size and hash, never bytes (D79).
pub fn denied_file(path: &str, content_len: u64, observed_ms: i64) -> ScanEntry {
    ScanEntry {
        path: NfsPath::parse(path).unwrap(),
        hash: hash_of(path.as_bytes()),
        size: content_len,
        mtime: Timestamp::from_unix_millis(observed_ms),
        observed_at: Timestamp::from_unix_millis(observed_ms),
        denied: true,
        bytes: None,
    }
}

/// One whole logical delta (a single message).
pub fn delta(seq: u64, entries: Vec<ScanEntry>) -> ScanDelta {
    part(seq, 0, false, entries)
}

/// One message of a logical delta.
pub fn part(seq: u64, part: u32, more: bool, entries: Vec<ScanEntry>) -> ScanDelta {
    ScanDelta {
        seq,
        base_root: Some(root(seq.saturating_sub(1))),
        new_root: root(seq),
        entries,
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more,
        part,
        gap: None,
    }
}

pub fn removal(seq: u64, path: &str) -> ScanDelta {
    let mut d = delta(seq, Vec::new());
    d.removed.push(NfsPath::parse(path).unwrap());
    d
}

/// A spool on its own temporary directory.
pub struct Rig {
    pub dir: TempDir,
    pub clock: Arc<TestClock>,
    pub spool: Spool,
    pub recovery: Recovery,
    limits: SpoolLimits,
}

impl Rig {
    /// Generous limits (the real defaults), small segments so that a few records span several files.
    pub fn new() -> Self {
        Self::with_limits(SpoolLimits::new(512 * 1024 * 1024, 100_000).with_segment_bytes(64 * 1024))
    }

    pub fn with_limits(limits: SpoolLimits) -> Self {
        let dir = TempDir::new().unwrap();
        Self::open_in(dir, limits)
    }

    pub fn open_in(dir: TempDir, limits: SpoolLimits) -> Self {
        Self::open_with(dir, limits, |options| options)
    }

    pub fn open_with(
        dir: TempDir,
        limits: SpoolLimits,
        configure: impl FnOnce(SpoolOptions) -> SpoolOptions,
    ) -> Self {
        let clock = Arc::new(TestClock::starting_at(T0_MS));
        let (spool, recovery) = open(dir.path(), limits, clock.clone(), configure);
        Self {
            dir,
            clock,
            spool,
            recovery,
            limits,
        }
    }

    /// Close the spool the way a killed process does (nothing is flushed that was not already) and open it again.
    pub fn reopen(self) -> Self {
        let Self {
            dir,
            clock,
            spool,
            limits,
            ..
        } = self;
        drop(spool);
        let (spool, recovery) = open(dir.path(), limits, clock.clone(), |options| options);
        Self {
            dir,
            clock,
            spool,
            recovery,
            limits,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The segment files, oldest first.
    pub fn segments(&self) -> Vec<PathBuf> {
        segment_files(self.dir.path())
    }

    pub async fn append(&self, delta: ScanDelta) {
        self.spool.append(delta).await.unwrap();
    }

    /// The next `count` messages a connection would get, as a fresh connection would get them. Does not acknowledge.
    pub async fn replay(&self, count: usize) -> Vec<ScanDelta> {
        self.replay_with(count, |_| {}).await
    }

    /// As [`Rig::replay`], calling `on_message` with each message as it arrives (to acknowledge it, say).
    pub async fn replay_with(&self, count: usize, mut on_message: impl FnMut(&ScanDelta)) -> Vec<ScanDelta> {
        let (outbox, mut rx) = outbox::channel(OutboxLimits::default());
        let pump = self.spool.attach(outbox);
        let task = tokio::spawn(pump.run());
        let mut got = Vec::new();
        while got.len() < count {
            let queued = tokio::time::timeout(Duration::from_secs(60), rx.recv())
                .await
                .expect("the spool replays the message within a virtual minute")
                .expect("the connection stays open");
            let (message, _permit) = queued.into_parts();
            let Some(FromAgent::Delta(d)) = FromAgent::from_proto(message).unwrap() else {
                panic!("the pump sends deltas only");
            };
            on_message(&d);
            got.push(d);
        }
        task.abort();
        got
    }

    /// True when a connection gets nothing more within a virtual minute.
    pub async fn nothing_more_to_replay(&self, already_sent: usize) -> bool {
        let (outbox, mut rx) = outbox::channel(OutboxLimits::default());
        let pump = self.spool.attach(outbox);
        let task = tokio::spawn(pump.run());
        let mut seen = 0;
        let quiet = loop {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
                Ok(Some(_)) => seen += 1,
                Ok(None) | Err(_) => break true,
            }
            if seen > already_sent {
                break false;
            }
        };
        task.abort();
        quiet && seen == already_sent
    }
}

pub fn open(
    path: &Path,
    limits: SpoolLimits,
    clock: Arc<dyn Clock>,
    configure: impl FnOnce(SpoolOptions) -> SpoolOptions,
) -> (Spool, Recovery) {
    let volume = SpoolVolume::open(path).unwrap();
    let options = configure(SpoolOptions::new(limits).with_io(IoMode::Inline));
    Spool::open(volume, options, clock).unwrap()
}

pub fn segment_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("seg-") && Path::new(n).extension().is_some_and(|e| e == "lks")
            })
        })
        .collect();
    files.sort();
    files
}

pub fn paths_of(deltas: &[ScanDelta]) -> Vec<(u64, Vec<String>)> {
    deltas
        .iter()
        .map(|d| {
            (
                d.seq,
                d.entries.iter().map(|e| e.path.as_str().to_owned()).collect(),
            )
        })
        .collect()
}
