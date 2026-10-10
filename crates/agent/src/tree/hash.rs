//! Hashing and the worker pool (T4, P4, code rule 4).
//!
//! All CPU-heavy and blocking work of the scanner runs on a [`Pool`]: a rayon pool of the configured size, never the
//! global one, entered from `spawn_blocking`. Nothing here is async.

use std::cell::RefCell;
use std::fmt;
use std::io::{self, Read};
use std::sync::Arc;

use cap_std::fs::{Metadata, MetadataExt};
use domain::ContentHash;
use sha2::{Digest, Sha256};

use super::node::{Stat, StatTime};

/// The most bytes of one file the agent hashes. A bigger file is kind 2 in the tree and is reported as too large.
pub const HASH_LIMIT: u64 = 64 * 1024 * 1024;

/// Read buffer for streaming hashes. Each worker thread keeps one and reuses it for every file it hashes.
const CHUNK: usize = 64 * 1024;

thread_local! {
    static CHUNK_BUF: RefCell<Vec<u8>> = RefCell::new(vec![0; CHUNK]);
}

/// The pool did not start.
#[derive(Debug, thiserror::Error)]
#[error("the worker pool could not be started")]
pub struct PoolError;

/// A fixed-size pool of worker threads for walking and hashing.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<rayon::ThreadPool>,
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("threads", &self.inner.current_num_threads())
            .finish()
    }
}

impl Pool {
    /// A pool of `threads` workers (at least one). Each has a 4 MiB stack: the walk recurses once per directory level.
    pub fn new(threads: usize) -> Result<Self, PoolError> {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .stack_size(4 * 1024 * 1024)
            .thread_name(|i| format!("lk-scan-{i}"))
            .build()
            .map(|pool| Self {
                inner: Arc::new(pool),
            })
            .map_err(|_| PoolError)
    }

    /// Run `f` on the pool and wait for it. Blocks: call it from `spawn_blocking`, or use [`Pool::run`].
    pub fn install<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        self.inner.install(f)
    }

    /// Run `f` on the pool from async code, without blocking the runtime.
    pub async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce() -> R + Send + 'static,
    ) -> Result<R, tokio::task::JoinError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || pool.install(f)).await
    }

    pub fn threads(&self) -> usize {
        self.inner.current_num_threads()
    }
}

/// The attributes a stat walk compares, from an open file's or an entry's metadata.
pub fn stat_of(md: &Metadata) -> Stat {
    let nanos = |n: i64| u32::try_from(n).unwrap_or(0);
    Stat::new(
        md.len(),
        StatTime::new(md.mtime(), nanos(md.mtime_nsec())),
        StatTime::new(md.ctime(), nanos(md.ctime_nsec())),
        md.ino(),
    )
}

/// Stream `reader` through SHA-256. `None` when it holds more than `limit` bytes (nothing past `limit` is read). On
/// success: the hash and the number of bytes read.
pub fn hash_stream<R: Read>(reader: &mut R, limit: u64) -> io::Result<Option<(ContentHash, u64)>> {
    CHUNK_BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            total += n as u64;
            if total > limit {
                return Ok(None);
            }
            hasher.update(&buf[..n]);
        }
        Ok(Some((ContentHash::from_bytes(hasher.finalize().into()), total)))
    })
}

/// SHA-256 of `bytes`.
pub fn hash_bytes(bytes: &[u8]) -> ContentHash {
    ContentHash::from_bytes(Sha256::digest(bytes).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_hash_equals_one_shot_hash() {
        let data: Vec<u8> = (0..(3 * CHUNK + 5))
            .map(|i| u8::try_from(i % 253).unwrap())
            .collect();
        let (hash, n) = hash_stream(&mut data.as_slice(), HASH_LIMIT).unwrap().unwrap();
        assert_eq!(hash, hash_bytes(&data));
        assert_eq!(n, data.len() as u64);
    }

    #[test]
    fn empty_input_hashes_to_the_empty_digest() {
        let (hash, n) = hash_stream(&mut io::empty(), 10).unwrap().unwrap();
        assert_eq!((hash, n), (hash_bytes(b""), 0));
    }

    #[test]
    fn a_stream_over_the_limit_is_refused_without_reading_it_all() {
        struct Endless(u64);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0 += buf.len() as u64;
                buf.fill(7);
                Ok(buf.len())
            }
        }
        let mut endless = Endless(0);
        assert!(hash_stream(&mut endless, 1000).unwrap().is_none());
        assert!(endless.0 <= (1000 + CHUNK as u64), "read {} bytes", endless.0);
        // Exactly at the limit is fine.
        let data = vec![1_u8; 1000];
        assert!(hash_stream(&mut data.as_slice(), 1000).unwrap().is_some());
        assert!(hash_stream(&mut data.as_slice(), 999).unwrap().is_none());
    }

    #[test]
    fn a_failing_read_is_an_error_and_an_interrupted_one_is_retried() {
        struct Flaky(u8);
        impl Read for Flaky {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                    2 => {
                        buf[..3].copy_from_slice(b"abc");
                        Ok(3)
                    }
                    3 => Err(io::Error::from(io::ErrorKind::TimedOut)),
                    _ => Ok(0),
                }
            }
        }
        let err = hash_stream(&mut Flaky(0), 100).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn the_pool_has_the_size_it_was_given_and_never_zero() {
        assert_eq!(Pool::new(3).unwrap().threads(), 3);
        assert_eq!(Pool::new(0).unwrap().threads(), 1);
    }
}
