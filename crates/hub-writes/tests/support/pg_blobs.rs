//! A test double of the `BlobStore` port over the real `blobs` table, so that the foreign keys of `proposals` and `blob_refs`
//! are the production ones (the fake store has no table behind it).

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use domain::ContentHash;
use ports::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgBlobs {
    pool: PgPool,
    puts: AtomicUsize,
}

impl PgBlobs {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            puts: AtomicUsize::new(0),
        }
    }

    pub fn puts(&self) -> usize {
        self.puts.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BlobStore for PgBlobs {
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        let hash = content_hash(&bytes);
        self.puts.fetch_add(1, Ordering::SeqCst);
        sqlx::query(
            "INSERT INTO blobs (hash, content, size) VALUES ($1, $2, $3) ON CONFLICT (hash) DO NOTHING",
        )
        .bind(hash.as_bytes().as_slice())
        .bind(bytes.as_ref())
        .bind(i64::try_from(bytes.len()).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await
        .map_err(|_| BlobError::Unavailable)?;
        Ok(hash)
    }

    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError> {
        let row: Option<Vec<u8>> = sqlx::query_scalar("SELECT content FROM blobs WHERE hash = $1")
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
