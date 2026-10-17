//! The caches of the read side (T6, mechanism 7, rule 5).
//!
//! Every key is made of content hashes (a blob, the layers of an effective config, the two sides of a diff, the version of a
//! swimlane's file list), so an entry is never stale and nothing ever invalidates it: a changed file is a new key. Every cache is
//! weighed in bytes and bounded by `RegistryConfig`, so the memory they take is a number in the settings (P11), not a result of
//! the traffic. `moka` evicts by recency and frequency; an entry heavier than the whole cache is not admitted. The text-search corpora
//! are not a `moka` cache: one snapshot of all swimlanes, in shards, replaced as a whole and kept only while it is inside its bound
//! (see [`super::corpora`]).
//!
//! A hit and a miss are counted by cache name only, never by key (S10): a key names a file.

use std::sync::Arc;

use domain::ContentHash;
use engine::{Effective, FlatSetting};
use moka::future::Cache as AsyncCache;
use moka::sync::Cache;

use crate::config::RegistryConfig;
use crate::metrics::{CacheName, CacheResult, RegistryMetrics};

/// The setting rows of one blob, as the engine reads them, from the settings index.
pub struct BlobRows {
    pub settings: Vec<FlatSetting>,
    /// For each setting, whether the secret scan flagged it (its value is stored as a redaction marker).
    pub secret: Vec<bool>,
    /// The blob does not parse, so it has no settings at all (the engine would not have indexed it either).
    pub unparseable: bool,
    weight: u32,
}

impl BlobRows {
    pub fn new(settings: Vec<FlatSetting>, secret: Vec<bool>, unparseable: bool) -> Self {
        let bytes: usize = settings
            .iter()
            .map(|s| 160 + s.path.as_str().len() + s.value.text.len())
            .sum::<usize>()
            + secret.len()
            + 64;
        Self {
            settings,
            secret,
            unparseable,
            weight: u32::try_from(bytes).unwrap_or(u32::MAX),
        }
    }

    pub fn any_secret(&self) -> bool {
        self.secret.iter().any(|s| *s)
    }
}

impl std::fmt::Debug for BlobRows {
    // The rows are config values: only their number is printed (S21).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobRows")
            .field("settings", &self.settings.len())
            .field("unparseable", &self.unparseable)
            .finish_non_exhaustive()
    }
}

/// An effective config and its weight in the cache. Its values are redaction markers where the scan flagged them.
#[derive(Debug)]
pub struct EffectiveEntry {
    pub effective: Effective,
    pub weight: u32,
}

/// A comparison of two contents, as computed.
#[derive(Debug)]
pub struct DiffEntry {
    pub body: super::compare::DiffBody,
    pub weight: u32,
}

/// The readable files of one swimlane at one version: what a text search needs to find the blobs to read.
#[derive(Debug)]
pub struct FileList {
    /// (path, class, content hash), the current version of every non-denied file that has content.
    pub files: Vec<(domain::NfsPath, domain::FileClass, ContentHash)>,
    pub weight: u32,
}

pub struct Caches {
    pub rows: Cache<ContentHash, Arc<BlobRows>>,
    pub effective: Cache<ContentHash, Arc<EffectiveEntry>>,
    pub diffs: Cache<ContentHash, Arc<DiffEntry>>,
    pub files: AsyncCache<(String, i64), Arc<FileList>>,
    pub corpora: super::corpora::Corpora,
    metrics: RegistryMetrics,
}

impl std::fmt::Debug for Caches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Caches").finish_non_exhaustive()
    }
}

impl Caches {
    pub fn new(config: &RegistryConfig, metrics: RegistryMetrics) -> Self {
        Self {
            rows: Cache::builder()
                .max_capacity(config.row_cache_bytes)
                .weigher(|_, v: &Arc<BlobRows>| v.weight)
                .build(),
            effective: Cache::builder()
                .max_capacity(config.effective_cache_bytes)
                .weigher(|_, v: &Arc<EffectiveEntry>| v.weight)
                .build(),
            diffs: Cache::builder()
                .max_capacity(config.diff_cache_bytes)
                .weigher(|_, v: &Arc<DiffEntry>| v.weight)
                .build(),
            files: AsyncCache::builder()
                .max_capacity(config.file_list_cache_bytes)
                .weigher(|_, v: &Arc<FileList>| v.weight)
                .build(),
            corpora: super::corpora::Corpora::default(),
            metrics,
        }
    }

    /// Count a lookup.
    pub fn count(&self, cache: CacheName, hit: bool) {
        self.metrics
            .read_cache(cache, if hit { CacheResult::Hit } else { CacheResult::Miss });
    }

    /// Drop everything. For tests and benchmarks that measure a cold request; never called by the service itself.
    pub fn clear(&self) {
        self.rows.invalidate_all();
        self.effective.invalidate_all();
        self.diffs.invalidate_all();
        self.files.invalidate_all();
        self.corpora.set(None);
    }

    /// Run the cache's deferred work so that counts and weights are exact (tests).
    pub fn sync(&self) {
        self.rows.run_pending_tasks();
        self.effective.run_pending_tasks();
        self.diffs.run_pending_tasks();
    }
}
