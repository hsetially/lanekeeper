//! Metrics of the blob store and its cache (S10): hits, misses, database reads, failures and the bytes held.
//!
//! No label carries a hash, a path or a user: the only label is the closed [`CacheResult`] set, so the series count is fixed.

use std::sync::Arc;

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

/// How a read was served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum CacheResult {
    Hit,
    Miss,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct CacheLabels {
    result: CacheResult,
}

#[derive(Debug, Default)]
struct Inner {
    lookups: Family<CacheLabels, Counter>,
    database_reads: Counter,
    store_errors: Counter,
    cache_bytes: Gauge,
    cache_entries: Gauge,
}

/// Cloning shares the series.
#[derive(Debug, Clone, Default)]
pub struct BlobMetrics {
    inner: Arc<Inner>,
}

impl BlobMetrics {
    pub fn register(&self, registry: &mut Registry) {
        let i = &self.inner;
        registry.register(
            "blob_cache_lookups",
            "Blob cache lookups, by result",
            i.lookups.clone(),
        );
        registry.register(
            "blob_database_reads",
            "Reads of blob content from Postgres (a batched read counts once)",
            i.database_reads.clone(),
        );
        registry.register(
            "blob_store_errors",
            "Blob store calls that failed: the database was unreachable, too slow or refused",
            i.store_errors.clone(),
        );
        registry.register(
            "blob_cache_bytes",
            "Bytes the blob cache holds, as weighed (content plus a fixed cost per entry)",
            i.cache_bytes.clone(),
        );
        registry.register(
            "blob_cache_entries",
            "Blobs the cache holds",
            i.cache_entries.clone(),
        );
    }

    pub(crate) fn lookup(&self, result: CacheResult) {
        self.inner.lookups.get_or_create(&CacheLabels { result }).inc();
    }

    pub(crate) fn database_read(&self) {
        self.inner.database_reads.inc();
    }

    pub(crate) fn store_error(&self) {
        self.inner.store_errors.inc();
    }

    pub(crate) fn set_cache(&self, bytes: u64, entries: u64) {
        self.inner
            .cache_bytes
            .set(i64::try_from(bytes).unwrap_or(i64::MAX));
        self.inner
            .cache_entries
            .set(i64::try_from(entries).unwrap_or(i64::MAX));
    }

    #[must_use]
    pub fn cache_hits(&self) -> u64 {
        self.inner
            .lookups
            .get_or_create(&CacheLabels {
                result: CacheResult::Hit,
            })
            .get()
    }

    #[must_use]
    pub fn cache_misses(&self) -> u64 {
        self.inner
            .lookups
            .get_or_create(&CacheLabels {
                result: CacheResult::Miss,
            })
            .get()
    }

    /// Reads of content from the database since the start. A cold `get_many` is one read however many blobs it asked for.
    #[must_use]
    pub fn database_reads(&self) -> u64 {
        self.inner.database_reads.get()
    }

    #[must_use]
    pub fn store_errors(&self) -> u64 {
        self.inner.store_errors.get()
    }
}
