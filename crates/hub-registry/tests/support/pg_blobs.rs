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
}

impl PgBlobs {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            puts: AtomicU64::new(0),
            fail: AtomicBool::new(false),
            fail_reads: AtomicBool::new(false),
            hang: AtomicBool::new(false),
        }
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
        let mut out = Vec::new();
        for h in hs {
            if let Some(b) = self.get(h).await? {
                out.push((*h, b));
            }
        }
        Ok(out)
    }
}
