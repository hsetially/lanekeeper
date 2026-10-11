//! T8 (P7, P11, rule 5): the blob cache is bounded by the bytes it holds, hands out the cached buffer without copying it, and the
//! two big caches of the platform, blobs and tree indexes, stay inside their caps however much is pushed through them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::sync::Arc;

use bytes::Bytes;
use domain::{CommitId, ContentHash, RepoKind, RepoPath, TreeEntry, TreeIndex};
use hub_platform::blobs::cache::BlobCache;
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::tree_index::TreeIndexCache;
use ports::content_hash;

const KIB: usize = 1024;
const MIB: u64 = 1024 * 1024;

fn blob(n: usize, seed: u8) -> (ContentHash, Bytes) {
    let bytes = Bytes::from(vec![seed; n]);
    (content_hash(&bytes), bytes)
}

#[test]
fn blob_cache_is_bounded_by_weight() {
    let cache = BlobCache::new(MIB, BlobMetrics::default());
    // 4 MiB of distinct 64 KiB blobs through a 1 MiB cache.
    for seed in 0..64u8 {
        let (hash, bytes) = blob(64 * KIB, seed);
        cache.insert(hash, bytes);
    }
    assert!(
        cache.weighted_size() <= MIB,
        "holds {} bytes, the cap is {MIB}",
        cache.weighted_size()
    );
    assert!(
        cache.len() <= 16,
        "{} blobs fit in 1 MiB at 64 KiB each",
        cache.len()
    );
    assert!(!cache.is_empty(), "a bounded cache is not an empty one");
}

#[test]
fn a_blob_bigger_than_the_whole_cache_is_not_kept() {
    let cache = BlobCache::new(100 * KIB as u64, BlobMetrics::default());
    let (hash, bytes) = blob(200 * KIB, 1);
    cache.insert(hash, bytes);
    assert!(cache.get(&hash).is_none());
    assert!(cache.weighted_size() <= 100 * KIB as u64);
}

#[test]
fn empty_blobs_have_a_weight_too() {
    let cache = BlobCache::new(1_000, BlobMetrics::default());
    // The empty blob is one value with one address, so a flood of distinct empty buffers cannot happen through the store; but the
    // cache itself must still count each entry, or a bug upstream could fill memory with free entries.
    for n in 0..10_000u32 {
        let mut hash = [0u8; 32];
        hash[..4].copy_from_slice(&n.to_be_bytes());
        cache.insert(ContentHash::from_bytes(hash), Bytes::new());
    }
    assert!(cache.weighted_size() <= 1_000, "{}", cache.weighted_size());
    // Each entry weighs its bytes plus the fixed cost of holding it (the key, the buffer's header, the cache's own record).
    assert!(cache.len() <= 7, "{} empty blobs in 1,000 bytes", cache.len());
}

#[test]
fn the_cached_buffer_is_shared_not_copied() {
    let cache = BlobCache::new(MIB, BlobMetrics::default());
    let (hash, bytes) = blob(256 * KIB, 7);
    let original = bytes.as_ptr();
    cache.insert(hash, bytes);
    let first = cache.get(&hash).unwrap();
    let second = cache.get(&hash).unwrap();
    assert_eq!(first.as_ptr(), original, "a hit hands out the stored buffer");
    assert_eq!(second.as_ptr(), original);
}

#[test]
fn hits_and_misses_are_counted_and_a_miss_stores_nothing() {
    let metrics = BlobMetrics::default();
    let cache = BlobCache::new(MIB, metrics.clone());
    let (hash, bytes) = blob(10, 1);
    assert!(cache.get(&hash).is_none());
    assert!(cache.get(&hash).is_none());
    assert_eq!((metrics.cache_hits(), metrics.cache_misses()), (0, 2));
    assert_eq!(cache.len(), 0, "negative results are not cached");
    cache.insert(hash, bytes);
    assert!(cache.get(&hash).is_some());
    assert_eq!((metrics.cache_hits(), metrics.cache_misses()), (1, 2));
}

fn index(n: usize, seed: u8) -> Arc<TreeIndex> {
    let commit = CommitId::parse(&format!("{seed:02x}").repeat(20)).unwrap();
    let entries = (0..n)
        .map(|i| TreeEntry {
            path: RepoPath::parse(&format!("config/f{i}.yml")).unwrap(),
            git_oid: CommitId::parse(&format!("{:040x}", i + 1)).unwrap(),
            sha256: ContentHash::from_bytes([seed; 32]),
            size: 1,
        })
        .collect();
    Arc::new(TreeIndex::new(RepoKind::Base, commit, entries))
}

#[test]
fn blob_cache_and_tree_cache_respect_their_caps() {
    let blobs = BlobCache::new(4 * MIB, BlobMetrics::default());
    let trees = TreeIndexCache::new(1_000, GitMetrics::default());
    // Ten times what each can hold.
    for seed in 0..=255u8 {
        let (hash, bytes) = blob(160 * KIB, seed);
        blobs.insert(hash, bytes);
    }
    for seed in 1..=60u8 {
        let commit = CommitId::parse(&format!("{seed:02x}").repeat(20)).unwrap();
        let built = trees
            .get_or_build::<()>(RepoKind::Base, &commit, || Ok(index(600, seed)))
            .unwrap();
        assert_eq!(built.entries().len(), 600);
    }
    assert!(
        blobs.weighted_size() <= 4 * MIB,
        "{} bytes",
        blobs.weighted_size()
    );
    assert!(
        trees.weighted_size() <= 1_000,
        "{} entries",
        trees.weighted_size()
    );
}
