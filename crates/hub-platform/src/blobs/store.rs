//! `PgBlobStore`: the `BlobStore` port over the `blobs` table (T8, D47, P7, P12).
//!
//! - **`put`** hashes the bytes here (SHA-256 of exactly what it was given: line endings are not touched), refuses more than
//!   4 MiB, and inserts with `ON CONFLICT DO NOTHING`. The table's own `CHECK (hash = sha256(content))` keeps it
//!   content-addressed even if this code were wrong. A written blob is not put in the cache: writes are often never read again.
//! - **`get`** looks in the cache, then reads the row and caches it. A hash that is not stored is `None` and is not cached.
//!   Concurrent reads of the same cold blob share one database read (single flight), so 100 users opening the same big file cost
//!   one read of it, not 100.
//! - **`get_many`** serves what the cache has and reads all the rest with one `= ANY($1)` query, never one query per blob.
//!
//! **Bounds (rule 5).** Every database call has a timeout. At most `read_concurrency` reads of content run at once (the rest wait,
//! also with a timeout), so a flood of cold reads cannot take every connection of the pool from the rest of the hub. The
//! single-flight table holds at most [`MAX_INFLIGHT`] entries; beyond that a read simply does not coalesce. The cache is bounded by
//! bytes ([`BlobCache`]).
//!
//! **Errors (S21).** A database failure is [`BlobError::Unavailable`] and nothing more: the cause (a driver message can name the
//! host, the user or the SQL) is reduced to a fixed word in the log. Blob content and hashes are never logged.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::ContentHash;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use ports::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::warn;

use super::cache::{BlobCache, DEFAULT_CACHE_BYTES};
use super::metrics::BlobMetrics;

/// One blob by address. `hash = $1::bytea` compares with the column's base type, so the primary key's index is used.
pub const GET_ONE_SQL: &str = "SELECT content::bytea FROM blobs WHERE hash = $1::bytea";
/// Many blobs by address, in one statement.
pub const GET_MANY_SQL: &str = "SELECT hash::bytea, content::bytea FROM blobs WHERE hash = ANY($1::bytea[])";
const PUT_SQL: &str =
    "INSERT INTO blobs (hash, content, size) VALUES ($1::bytea, $2::bytea, $3) ON CONFLICT (hash) DO NOTHING";

/// Blobs up to this size are hashed on the async thread (about 30 microseconds); bigger ones go to a blocking thread (rule 4).
const INLINE_HASH_LIMIT: usize = 64 * 1024;
/// Distinct cold blobs that may be loading at once with their readers coalesced.
pub const MAX_INFLIGHT: usize = 1024;

/// Bounds and timings of the store.
#[derive(Debug, Clone, Copy)]
pub struct BlobSettings {
    /// What the cache may hold, in weighed bytes (`LK_BLOB_CACHE_MIB`, default 512 MiB).
    pub cache_bytes: u64,
    /// Reads of content that may run at once.
    pub read_concurrency: usize,
    /// One database call, and one wait for a read slot.
    pub query_timeout: Duration,
}

impl Default for BlobSettings {
    fn default() -> Self {
        Self {
            cache_bytes: DEFAULT_CACHE_BYTES,
            read_concurrency: 16,
            query_timeout: Duration::from_secs(5),
        }
    }
}

type Load = Shared<BoxFuture<'static, Result<Option<Bytes>, BlobError>>>;

#[derive(Default)]
struct Inflight {
    loads: Mutex<HashMap<ContentHash, Load>>,
}

impl Inflight {
    fn lock(&self) -> MutexGuard<'_, HashMap<ContentHash, Load>> {
        self.loads.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Takes a load out of the table when the task that ran it ends, however it ends (a panic included).
struct Finished {
    inflight: Arc<Inflight>,
    hash: ContentHash,
}

impl Drop for Finished {
    fn drop(&mut self) {
        self.inflight.lock().remove(&self.hash);
    }
}

/// What a database read needs, cheap to clone into the task that runs it.
#[derive(Clone)]
struct Reader {
    pool: PgPool,
    cache: BlobCache,
    metrics: BlobMetrics,
    slots: Arc<Semaphore>,
    timeout: Duration,
}

/// A fixed word for why a call failed. A driver's own message is never logged.
fn kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "io",
        sqlx::Error::Database(_) => "database",
        _ => "other",
    }
}

impl Reader {
    fn failed(&self, op: &'static str, why: &'static str) -> BlobError {
        self.metrics.store_error();
        warn!(op, why, "blob store call failed");
        BlobError::Unavailable
    }

    /// Wait for a read slot, at most `timeout`.
    async fn slot(&self, op: &'static str) -> Result<tokio::sync::OwnedSemaphorePermit, BlobError> {
        match tokio::time::timeout(self.timeout, Arc::clone(&self.slots).acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(self.failed(op, "closed")),
            Err(_) => Err(self.failed(op, "queue_timed_out")),
        }
    }

    /// One blob from the database, into the cache.
    async fn read_one(&self, hash: ContentHash) -> Result<Option<Bytes>, BlobError> {
        // A load that finished while this one was being set up has already cached it.
        if let Some(found) = self.cache.peek(&hash) {
            return Ok(Some(found));
        }
        let _slot = self.slot("get").await?;
        self.metrics.database_read();
        let call = sqlx::query_scalar::<_, Vec<u8>>(GET_ONE_SQL)
            .bind(hash.as_bytes().as_slice())
            .fetch_optional(&self.pool);
        match tokio::time::timeout(self.timeout, call).await {
            Ok(Ok(Some(content))) => {
                let content = Bytes::from(content);
                self.cache.insert(hash, content.clone());
                Ok(Some(content))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(error)) => Err(self.failed("get", kind(&error))),
            Err(_) => Err(self.failed("get", "timed_out")),
        }
    }

    /// Several blobs with one statement, each into the cache.
    async fn read_many(&self, hashes: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError> {
        let _slot = self.slot("get_many").await?;
        self.metrics.database_read();
        let addresses: Vec<Vec<u8>> = hashes.iter().map(|h| h.as_bytes().to_vec()).collect();
        let call = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(GET_MANY_SQL)
            .bind(addresses)
            .fetch_all(&self.pool);
        let rows = match tokio::time::timeout(self.timeout, call).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(error)) => return Err(self.failed("get_many", kind(&error))),
            Err(_) => return Err(self.failed("get_many", "timed_out")),
        };
        let mut found = Vec::with_capacity(rows.len());
        for (address, content) in rows {
            // The column is `sha256_hash`, 32 bytes by its own CHECK; a row that is not is skipped, never guessed at.
            let Ok(address) = <[u8; 32]>::try_from(address) else {
                continue;
            };
            let (hash, content) = (ContentHash::from_bytes(address), Bytes::from(content));
            self.cache.insert(hash, content.clone());
            found.push((hash, content));
        }
        Ok(found)
    }
}

/// The blob store on Postgres, with its cache.
pub struct PgBlobStore {
    reader: Reader,
    inflight: Arc<Inflight>,
}

impl std::fmt::Debug for PgBlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgBlobStore")
            .field("cache", &self.reader.cache)
            .finish_non_exhaustive()
    }
}

impl PgBlobStore {
    /// Must be called inside a Tokio runtime only when reads happen; building the store needs none.
    #[must_use]
    pub fn new(pool: PgPool, settings: BlobSettings, metrics: BlobMetrics) -> Self {
        let cache = BlobCache::new(settings.cache_bytes, metrics.clone());
        Self {
            reader: Reader {
                pool,
                cache,
                metrics,
                slots: Arc::new(Semaphore::new(settings.read_concurrency.max(1))),
                timeout: settings.query_timeout,
            },
            inflight: Arc::new(Inflight::default()),
        }
    }

    /// The cache in front of the table.
    #[must_use]
    pub fn cache(&self) -> &BlobCache {
        &self.reader.cache
    }

    /// The read of `hash` that is running, or a new one. `None` when the table is full, so this read does not coalesce.
    fn running(&self, hash: ContentHash) -> Option<Load> {
        let mut loads = self.inflight.lock();
        if let Some(running) = loads.get(&hash) {
            return Some(running.clone());
        }
        if loads.len() >= MAX_INFLIGHT {
            return None;
        }
        // The read runs as a task of its own, so that a caller that goes away (a client that disconnects) does not abandon it for
        // the others, and so that its table entry is always removed when it ends. The lock is held while the task is spawned and
        // entered, so its removal cannot come before its insertion.
        let (reader, inflight) = (self.reader.clone(), Arc::clone(&self.inflight));
        let task = tokio::spawn(async move {
            let _finished = Finished { inflight, hash };
            reader.read_one(hash).await
        });
        let load: Load = async move { task.await.unwrap_or(Err(BlobError::Unavailable)) }
            .boxed()
            .shared();
        loads.insert(hash, load.clone());
        Some(load)
    }

    /// Read one cold blob, sharing the read with every other caller that asks for it meanwhile.
    async fn load(&self, hash: ContentHash) -> Result<Option<Bytes>, BlobError> {
        match self.running(hash) {
            Some(load) => load.await,
            None => self.reader.read_one(hash).await,
        }
    }
}

#[async_trait]
impl BlobStore for PgBlobStore {
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        let hash = if bytes.len() <= INLINE_HASH_LIMIT {
            content_hash(&bytes)
        } else {
            let to_hash = bytes.clone();
            tokio::task::spawn_blocking(move || content_hash(&to_hash))
                .await
                .map_err(|_| self.reader.failed("put", "hash_task"))?
        };
        let size = i64::try_from(bytes.len()).map_err(|_| BlobError::TooLarge)?;
        let call = sqlx::query(PUT_SQL)
            .bind(hash.as_bytes().as_slice())
            .bind(bytes.as_ref())
            .bind(size)
            .execute(&self.reader.pool);
        match tokio::time::timeout(self.reader.timeout, call).await {
            Ok(Ok(_)) => Ok(hash),
            Ok(Err(error)) => Err(self.reader.failed("put", kind(&error))),
            Err(_) => Err(self.reader.failed("put", "timed_out")),
        }
    }

    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError> {
        if let Some(found) = self.reader.cache.get(h) {
            return Ok(Some(found));
        }
        self.load(*h).await
    }

    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError> {
        if hs.len() > MAX_GET_MANY {
            return Err(BlobError::TooMany);
        }
        // Each hash once, in the order it was first asked for.
        let mut seen = HashSet::with_capacity(hs.len());
        let order: Vec<ContentHash> = hs.iter().copied().filter(|h| seen.insert(*h)).collect();
        let mut found: HashMap<ContentHash, Bytes> = HashMap::with_capacity(order.len());
        let mut cold = Vec::new();
        for hash in &order {
            match self.reader.cache.get(hash) {
                Some(bytes) => {
                    found.insert(*hash, bytes);
                }
                None => cold.push(*hash),
            }
        }
        if !cold.is_empty() {
            found.extend(self.reader.read_many(&cold).await?);
        }
        Ok(order
            .into_iter()
            .filter_map(|hash| found.remove(&hash).map(|bytes| (hash, bytes)))
            .collect())
    }
}
