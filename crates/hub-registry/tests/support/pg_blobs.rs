//! A `BlobStore` over the real `blobs` table, for tests: content-addressed, idempotent, counted, and able to fail.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use domain::ContentHash;
use ports::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgBlobs {
    pool: PgPool,
    puts: AtomicU64,
    fail: AtomicBool,
    fail_reads: AtomicBool,
    hang: AtomicBool,
    /// The hashes `get_many` was asked for, in order, for tests that say which content was read.
    requested: std::sync::Mutex<Vec<ContentHash>>,
    /// Calls to `get_many`.
    many_calls: AtomicU64,
    /// Calls to `get`.
    gets: AtomicU64,
}

impl PgBlobs {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            puts: AtomicU64::new(0),
            fail: AtomicBool::new(false),
            fail_reads: AtomicBool::new(false),
            hang: AtomicBool::new(false),
            requested: std::sync::Mutex::new(Vec::new()),
            many_calls: AtomicU64::new(0),
            gets: AtomicU64::new(0),
        }
    }

    /// The hashes asked for through `get_many` since the last `forget_requests`.
    pub fn requested(&self) -> Vec<ContentHash> {
        self.requested.lock().unwrap().clone()
    }

    pub fn forget_requests(&self) {
        self.requested.lock().unwrap().clear();
        self.many_calls.store(0, Ordering::SeqCst);
        self.gets.store(0, Ordering::SeqCst);
    }

    /// Calls to `get` since the last `forget_requests`.
    pub fn gets(&self) -> u64 {
        self.gets.load(Ordering::SeqCst)
    }

    /// Calls to `get_many` since the last `forget_requests`.
    pub fn many_calls(&self) -> u64 {
        self.many_calls.load(Ordering::SeqCst)
    }

    /// Calls to `put`, whether or not the blob was new.
    pub fn puts(&self) -> u64 {
        self.puts.load(Ordering::SeqCst)
    }

    pub fn set_failing(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }

    /// Make `get` and `get_many` fail, as a store that has lost its connection does for readers.
    pub fn set_failing_reads(&self, fail: bool) {
        self.fail_reads.store(fail, Ordering::SeqCst);
    }

    /// Make `put` wait for ever, as a store that has lost its connection does.
    pub fn set_hanging(&self, hang: bool) {
        self.hang.store(hang, Ordering::SeqCst);
    }

    /// Blobs stored.
    pub async fn stored(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM blobs")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn has(&self, hash: &ContentHash) -> bool {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM blobs WHERE hash = $1::bytea)")
            .bind(hash.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[async_trait]
impl BlobStore for PgBlobs {
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(BlobError::Unavailable);
        }
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        let hash = content_hash(&bytes);
        sqlx::query("INSERT INTO blobs (hash, content, size) VALUES ($1::bytea, $2::bytea, $3) ON CONFLICT (hash) DO NOTHING")
            .bind(hash.as_bytes().as_slice())
            .bind(bytes.as_ref())
            .bind(i64::try_from(bytes.len()).unwrap())
            .execute(&self.pool)
            .await
            .map_err(|_| BlobError::Unavailable)?;
        Ok(hash)
    }

    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(BlobError::Unavailable);
        }
        let row: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT content::bytea FROM blobs WHERE hash = $1::bytea")
                .bind(h.as_bytes().as_slice())
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| BlobError::Unavailable)?;
        Ok(row.map(Bytes::from))
    }

    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError> {
        if hs.len() > MAX_GET_MANY {
            return Err(BlobError::TooMany);
        }
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(BlobError::Unavailable);
        }
        self.many_calls.fetch_add(1, Ordering::SeqCst);
        self.requested.lock().unwrap().extend_from_slice(hs);
        // One statement for the whole batch, like the real store, in the order first requested, each at most once.
        let wanted: Vec<Vec<u8>> = hs.iter().map(|h| h.as_bytes().to_vec()).collect();
        let rows: Vec<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT hash::bytea, content::bytea FROM blobs WHERE hash = ANY($1::bytea[])")
                .bind(&wanted)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| BlobError::Unavailable)?;
        let mut found: std::collections::HashMap<Vec<u8>, Bytes> =
            rows.into_iter().map(|(h, c)| (h, Bytes::from(c))).collect();
        let mut out = Vec::new();
        for h in hs {
            if let Some(b) = found.remove(h.as_bytes().as_slice()) {
                out.push((*h, b));
            }
        }
        Ok(out)
    }
}
