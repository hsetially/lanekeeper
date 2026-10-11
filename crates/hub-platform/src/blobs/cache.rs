//! The blob cache: moka, weighted by bytes, holding `Bytes` (T8, P7, P11, rule 5).
//!
//! A blob is addressed by the SHA-256 of its content, so a cached blob is never stale and nothing ever invalidates it. The cache
//! is bounded by what it holds, not by how many blobs it holds: each entry weighs its bytes plus a fixed cost for the key, the
//! buffer's header and the cache's own record. A hit hands out a clone of the `Bytes`, which shares the buffer; the content is
//! never copied between the cache, the store and the response.
//!
//! Only found blobs are cached. A hash that is not in the store is asked again next time, so a blob stored a moment later is
//! found at once and the cache can never hold a "no".

use bytes::Bytes;
use domain::ContentHash;
use moka::sync::Cache;

use super::metrics::{BlobMetrics, CacheResult};

/// What holding one entry costs besides its bytes: the 32-byte key, the buffer header, and the cache's bookkeeping.
pub const ENTRY_OVERHEAD: usize = 128;

/// The default capacity: 512 MiB (`LK_BLOB_CACHE_MIB`).
pub const DEFAULT_CACHE_BYTES: u64 = 512 * 1024 * 1024;

fn weight(bytes: &Bytes) -> u32 {
    // A blob is at most 4 MiB, so this never saturates; a buffer that somehow did is heavy enough to be evicted at once.
    u32::try_from(bytes.len().saturating_add(ENTRY_OVERHEAD)).unwrap_or(u32::MAX)
}

/// A byte-weighted cache of blob content, keyed by address.
#[derive(Clone)]
pub struct BlobCache {
    cache: Cache<ContentHash, Bytes>,
    metrics: BlobMetrics,
    max_bytes: u64,
}

impl std::fmt::Debug for BlobCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobCache")
            .field("entries", &self.cache.entry_count())
            .finish_non_exhaustive()
    }
}

impl BlobCache {
    /// Holds at most `max_bytes` of weighed content in all.
    #[must_use]
    pub fn new(max_bytes: u64, metrics: BlobMetrics) -> Self {
        let max_bytes = max_bytes.max(1);
        let cache = Cache::builder()
            .max_capacity(max_bytes)
            .weigher(|_hash: &ContentHash, bytes: &Bytes| weight(bytes))
            .build();
        Self {
            cache,
            metrics,
            max_bytes,
        }
    }

    /// The cached content, counting a hit or a miss.
    #[must_use]
    pub fn get(&self, hash: &ContentHash) -> Option<Bytes> {
        let found = self.cache.get(hash);
        self.metrics.lookup(if found.is_some() {
            CacheResult::Hit
        } else {
            CacheResult::Miss
        });
        found
    }

    /// Like [`BlobCache::get`], without counting: for the check a load makes after it has already counted its miss.
    #[must_use]
    pub(crate) fn peek(&self, hash: &ContentHash) -> Option<Bytes> {
        self.cache.get(hash)
    }

    /// Remember `bytes` under `hash`. The caller has already checked that the bytes hash to it. A blob that would not fit in the
    /// whole cache is not kept (the cache would admit it and evict it at its next housekeeping, holding it in between).
    pub fn insert(&self, hash: ContentHash, bytes: Bytes) {
        if u64::from(weight(&bytes)) > self.max_bytes {
            return;
        }
        self.cache.insert(hash, bytes);
        self.metrics
            .set_cache(self.cache.weighted_size(), self.cache.entry_count());
    }

    /// Weighed bytes held right now. Settles the cache's pending bookkeeping first, so it is exact (and not for a hot path).
    #[must_use]
    pub fn weighted_size(&self) -> u64 {
        self.cache.run_pending_tasks();
        self.cache.weighted_size()
    }

    /// Blobs held right now, settled like [`BlobCache::weighted_size`].
    #[must_use]
    pub fn len(&self) -> u64 {
        self.cache.run_pending_tasks();
        self.cache.entry_count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
