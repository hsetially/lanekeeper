//! `PgBlobStore`: the `BlobStore` port over the `blobs` table (T8, D47, P7, P12).
//!
//! - **`put`** hashes the bytes here (SHA-256 of exactly what it was given: line endings are not touched), refuses more than
//!   4 MiB, and runs [`PUT_SQL`]: one statement that inserts a new blob and refreshes the age of one that exists (below). The
//!   table's own `CHECK (hash = sha256(content))` keeps it content-addressed even if this code were wrong. A written blob is not
//!   put in the cache: writes are often never read again.
//! - **`get`** looks in the cache, then reads the row and caches it. A hash that is not stored is `None` and is not cached.
//!   Concurrent reads of the same cold blob share one database read (single flight), so 100 users opening the same big file cost
//!   one read of it, not 100.
//! - **`get_many`** serves what the cache has and reads all the rest with one `= ANY($1)` query, never one query per blob.
//!
//! **The age of a blob (migration 0008, D78).** Retention deletes blobs that nothing references and whose `touched_at` is older than
//! a grace period, and a writer stores a blob before the transaction that references it commits, so a blob that is stored again
//! must not keep its old age: an old, unreferenced blob whose content came back would be collected in between. `put` of a blob that
//! exists therefore refreshes `touched_at`, at most once per [`TOUCH_INTERVAL`] (a hot blob stored a thousand times an hour is
//! written once). The relation that makes this safe is a constant, [`MIN_RETENTION_GRACE`]: the grace must be at least the interval
//! plus the longest wait for the reference. The age trails the last `put` by at most one interval plus the wait for a row lock (see
//! [`TOUCH_INTERVAL`]). The rules that a collector and the code that references blobs follow are in `docs/interfaces.md`, under
//! `BlobStore`, and in the rustdoc of [`ports::BlobStore::put`].
//!
//! - *One statement, and nothing written for a recent blob.* A blob touched within the interval is found by a read-only probe
//!   first, and then the statement inserts nothing and takes no lock: no new tuple, no WAL, no transaction id (so no commit
//!   flush). A plain `ON CONFLICT DO UPDATE ... WHERE` would take the row lock before it tests the age, which is one WAL record and
//!   a commit flush for every re-put of a hot blob (measured in `tests/blob_put_cost.rs`). Only a new blob (an insert) or one older
//!   than the interval (an update of the one column) writes. The update half is `ON CONFLICT DO UPDATE`, so a concurrent delete of
//!   the same row is waited for and the blob is stored again afterwards.
//! - *The cache cannot make the refresh unreachable, because `put` never reads it.* A cached blob that is stored again still runs
//!   the statement, and the statement decides by the table's age. Do not add a shortcut that skips `put` for a blob the cache holds
//!   (`a_cached_blob_that_is_put_again_still_refreshes` fails): it would let a hot, cached blob go stale in the table for ever.
//! - *Outside any caller transaction.* `put` runs [`PUT_SQL`] on the pool, in autocommit, and takes no `Tx`: `now()` is the
//!   transaction's start time, so inside a long transaction the statement would write an older `touched_at`.
//! - Reads (`get`, `get_many`) never touch the age: reading is not a reference.
//! - There is no batch `put`. A caller that stores many blobs calls `put` for each, each in its own statement, so no transaction
//!   ever holds two blob rows and there is no lock order to get wrong.
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
/// Store a blob, or refresh the age of one that exists, in one statement. Binds: `$1` the hash, `$2` the content, `$3` the size, `$4`
/// the touch interval in seconds ([`TOUCH_INTERVAL`]).
///
/// - The `WHERE NOT EXISTS` probe reads the row by its primary key: a blob touched within the interval inserts nothing, so the
///   statement is read-only (no tuple, no row lock, no WAL, no transaction id).
/// - Otherwise the row is inserted, and `ON CONFLICT (hash) DO UPDATE` handles the rest atomically: a concurrent insert or delete of
///   the same row is waited for, then the age is refreshed only if it is still older than the interval (a put that lost the race to
///   another put writes nothing), and a row that a collector deleted meanwhile is inserted again.
/// - Only `touched_at` is ever updated (the app holds UPDATE on that one column and no other, migration 0008).
///
/// Run it outside any caller transaction: `now()` is the transaction start time, so inside a long transaction it would write an
/// older `touched_at`. (`PgBlobStore::put` runs it on the pool, in autocommit.)
pub const PUT_SQL: &str = "\
INSERT INTO blobs (hash, content, size) \
SELECT $1::bytea, $2::bytea, $3::bigint \
WHERE NOT EXISTS (SELECT 1 FROM blobs WHERE hash = $1::bytea AND touched_at >= now() - ($4::bigint * interval '1 second')) \
ON CONFLICT (hash) DO UPDATE SET touched_at = now() \
WHERE blobs.touched_at < now() - ($4::bigint * interval '1 second')";

/// How far `touched_at` may lag behind the last `put` of a blob: a `put` refreshes the age only when it is older than this, so a hot
/// blob costs one write an hour however often it is stored. The age is therefore at most this much older than the start of the
/// last `put`, and that start can be earlier than the commit by the wait for a row lock when the blob is being refreshed or collected
/// at that moment (the statement waits, bounded by the put's own timeout, and writes the time it started); it is never newer. So
/// the lag behind the commit of the last `put` is at most this interval plus that wait (5 s by default), which is negligible against
/// the margin between [`MIN_RETENTION_GRACE`] and the grace period. Far below the grace period, and tied to it by
/// [`MIN_RETENTION_GRACE`] and a compile-time assertion.
pub const TOUCH_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The longest a writer may hold a blob it has stored before the transaction that references it commits: the registry's
/// `staging_ttl` (24 hours by default), the wait of a non-final part of a delta.
///
/// This is a constant, and the registry's `staging_ttl` is configurable, so [`MIN_RETENTION_GRACE`] is the floor for the default
/// staging lifetime only. The registry must range-check `staging_ttl <= MAX_PUT_TO_REFERENCE`, or compute its floor as
/// `TOUCH_INTERVAL + staging_ttl`; a longer staging lifetime needs a longer grace. And anything that stores a blob and references it
/// more than this long afterwards (for example a prompt-06 upload preview that puts a blob and creates its draft or proposal row
/// later) must create the row when it stores the blob, or `put` the blob again when it references it. A draft or a proposal is safe
/// once its row exists, because `drafts.new_hash` and `proposals.new_hash` are RESTRICT foreign keys.
pub const MAX_PUT_TO_REFERENCE: Duration = Duration::from_secs(24 * 60 * 60);

/// The shortest grace period retention may use: a blob stored at `t` has `touched_at >= t - TOUCH_INTERVAL`, so it is deletable no
/// earlier than `t - TOUCH_INTERVAL + grace`, which is after its reference commits (`t + MAX_PUT_TO_REFERENCE`) only if
/// `grace >= TOUCH_INTERVAL + MAX_PUT_TO_REFERENCE`. The registry range-checks its grace period against this, for the default
/// staging lifetime; for a longer one see [`MAX_PUT_TO_REFERENCE`].
pub const MIN_RETENTION_GRACE: Duration = TOUCH_INTERVAL.saturating_add(MAX_PUT_TO_REFERENCE);

// The grace period the human approved on 2026-10-15 is 48 hours (the registry owns the setting); it must leave room above the floor.
// And a refresh that is not rate-limited is not a refresh interval at all: a blob stored a thousand times would be written a
// thousand times.
const _: () = assert!(MIN_RETENTION_GRACE.as_secs() < 48 * 60 * 60);
const _: () = assert!(TOUCH_INTERVAL.as_secs() >= 60);

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
        // Never the cache: a blob that is cached and stored again must still reach the statement, which decides by the table's age.
        let call = sqlx::query(PUT_SQL)
            .bind(hash.as_bytes().as_slice())
            .bind(bytes.as_ref())
            .bind(size)
            .bind(i64::try_from(TOUCH_INTERVAL.as_secs()).unwrap_or(i64::MAX))
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
