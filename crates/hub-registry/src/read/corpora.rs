//! The text-search corpora: content-addressed shards that survive a change to one swimlane (T6, P7: 400 ms).
//!
//! The engine searches a [`SearchCorpus`]: the distinct blobs, each with every place (swimlane and path) that holds it. One corpus for
//! all swimlanes is the cheapest to search (a blob shared by thirty swimlanes is read once), but it is also made of all of them, so
//! that any applied change anywhere meant making it again (about 100 ms of sorting 80,000 places, and the same again for the place
//! lists). A corpus per swimlane survives a change to another swimlane but searches a shared blob once per swimlane (up to about 3.4
//! times the regex work at the target scale), which is what the budget cannot afford.
//!
//! # The shards
//!
//! The blobs are split by their hash into [`RegistryConfig::search_shards`](crate::RegistryConfig) shards, each a corpus of its blobs
//! **with all of their places in every swimlane**. A blob is in exactly one shard, so it is read once per search, as before. A shard is
//! made again only when the places of its blobs change, which a change to one swimlane does for the few shards that hold the blobs of the
//! files that changed (a swimlane's contribution to a shard is a digest of the paths and blobs in it, so a new version of a swimlane
//! that changes nothing for a shard leaves it as it was).
//!
//! # The answer is the single corpus's answer
//!
//! A search runs the engine on every shard with the same query and puts the answers together with [`merge`]. The engine's answer over
//! one corpus is: the places that have a hit, in swimlane and path order, each with up to `per_file` hits; at most `total` hits;
//! `truncated` when the total cut something or when a place that was reached had more than `per_file` hits. A place is in one shard
//! and each shard keeps that order, so merging the shards' hits by (swimlane, path) and cutting at `total` is the first `total` hits of
//! the whole; and `truncated` is whether the shards held more than `total` hits between them or any of them said it was truncated (a
//! shard says so for a cut of its own, which has `total` hits in it, or for an overflow, which the whole would have reached or
//! been cut before). `tests/corpora_equivalence.rs` holds the engine on one corpus to this over random corpora and queries and over the
//! twenty benchmark queries on the target-scale fixtures.
//!
//! # The snapshot
//!
//! What a search uses is a [`Snapshot`]: the file list of each swimlane (at a version) and the shards made from them, immutable, behind an
//! `Arc`. A reader takes the current one without a lock held across work; a build makes the next one from it, sharing every list and
//! shard that did not change, and swaps it in. A snapshot answers a search when each swimlane the search covers is at its current
//! version or later. A swimlane's version moves on every applied change, and the file list is read after the version, so a list is
//! never older than its version says.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, PoisonError, RwLock};

use bytes::Bytes;
use domain::{ContentHash, TextHit, TextResults};
use engine::SearchCorpus;
use sha2::{Digest, Sha256};

use super::cache::FileList;

/// The shard of a blob: its hash modulo the number of shards (a SHA-256 is uniform, so the shards are alike in size).
pub fn shard_of(hash: &ContentHash, shards: usize) -> usize {
    usize::from(hash.as_bytes()[0]) % shards.max(1)
}

/// The cap on the hits that the engine applies for a request: the request's own, held to the server's ceiling (A27).
pub fn effective_total(asked: u32, ceiling: usize) -> usize {
    usize::try_from(asked).map_or(ceiling, |a| a.min(ceiling))
}

/// The engine's answer over all the shards' blobs in one corpus, from its answers over each shard, for the same query (see the module
/// documentation for why this is exact). `total` is [`effective_total`].
pub fn merge(parts: Vec<TextResults>, total: usize) -> TextResults {
    let any_truncated = parts.iter().any(|p| p.truncated);
    let mut hits: Vec<TextHit> = parts.into_iter().flat_map(|p| p.hits).collect();
    let more_than_total = hits.len() > total;
    // Stable, and a place is in one shard, so the hits of a place stay in line order.
    hits.sort_by(|a, b| a.swimlane.cmp(&b.swimlane).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(total);
    TextResults {
        hits,
        truncated: more_than_total || any_truncated,
    }
}

/// One swimlane's file list at a version, with its files split by shard.
#[derive(Debug)]
pub struct LaneEntry {
    /// The swimlane's version when the list was read (read before the list: the list is at least this new).
    pub version: i64,
    pub list: Arc<FileList>,
    /// For each shard, the indexes into `list.files` of the files whose blob is in it, in path order.
    pub buckets: Vec<Vec<u32>>,
    /// For each shard, a digest of the paths and blobs in its bucket (`None` for an empty one), the same whatever order the list came in.
    pub digests: Vec<Option<[u8; 32]>>,
}

impl LaneEntry {
    pub fn new(version: i64, list: Arc<FileList>, shards: usize) -> Self {
        let shards = shards.max(1);
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); shards];
        for (i, (_, _, hash)) in list.files.iter().enumerate() {
            if let Ok(i) = u32::try_from(i) {
                buckets[shard_of(hash, shards)].push(i);
            }
        }
        let digests = buckets
            .iter_mut()
            .map(|bucket| {
                if bucket.is_empty() {
                    return None;
                }
                bucket.sort_by(|&a, &b| list.files[a as usize].0.cmp(&list.files[b as usize].0));
                let mut h = Sha256::new();
                for &i in bucket.iter() {
                    let (path, _, hash) = &list.files[i as usize];
                    h.update((path.as_str().len() as u64).to_be_bytes());
                    h.update(path.as_str().as_bytes());
                    h.update(hash.as_bytes());
                }
                Some(h.finalize().into())
            })
            .collect();
        Self {
            version,
            list,
            buckets,
            digests,
        }
    }

    /// What this entry holds in memory, besides the list (which the file-list cache weighs too).
    pub fn weight(&self) -> u64 {
        let files: usize = self.buckets.iter().map(Vec::len).sum();
        u64::from(self.list.weight) + (files as u64) * 4 + (self.buckets.len() as u64) * 64
    }
}

/// The key of a shard: what decides which places its blobs have, so that equal keys mean equal corpora. It names the shard and, for each
/// swimlane that has files in it, the swimlane and its digest of them.
pub fn shard_key(index: usize, shards: usize, lanes: &BTreeMap<String, Arc<LaneEntry>>) -> ContentHash {
    let mut h = Sha256::new();
    h.update(b"lanekeeper/corpus-shard/v1\n");
    h.update((shards as u64).to_be_bytes());
    h.update((index as u64).to_be_bytes());
    for (lane, entry) in lanes {
        if let Some(Some(digest)) = entry.digests.get(index) {
            h.update((lane.len() as u64).to_be_bytes());
            h.update(lane.as_bytes());
            h.update(digest);
        }
    }
    ContentHash::from_bytes(h.finalize().into())
}

/// One shard: a corpus of the blobs whose hash falls in it, with all their places.
pub struct Shard {
    pub key: ContentHash,
    /// The bytes of its blobs, which the corpus shares (`Bytes` is counted once): what the next version of the shard reads from instead
    /// of the blob store.
    pub contents: HashMap<ContentHash, Bytes>,
    pub corpus: SearchCorpus,
    /// The bytes of the blobs and an allowance for each place.
    pub weight: u64,
}

impl std::fmt::Debug for Shard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shard")
            .field("blobs", &self.corpus.blob_count())
            .field("places", &self.corpus.location_count())
            .field("weight", &self.weight)
            .finish_non_exhaustive()
    }
}

/// What a search runs on: the swimlanes' file lists and the shards made from them.
#[derive(Debug)]
pub struct Snapshot {
    pub lanes: BTreeMap<String, Arc<LaneEntry>>,
    pub shards: Vec<Arc<Shard>>,
    pub weight: u64,
}

impl Snapshot {
    /// An empty snapshot of `shards` shards: nothing covered.
    pub fn empty() -> Self {
        Self {
            lanes: BTreeMap::new(),
            shards: Vec::new(),
            weight: 0,
        }
    }

    /// Whether every one of these (swimlane, version) is in the snapshot at that version or a later one.
    pub fn covers(&self, required: &[(String, i64)]) -> bool {
        !self.shards.is_empty()
            && required
                .iter()
                .all(|(lane, version)| self.lanes.get(lane).is_some_and(|e| e.version >= *version))
    }

    /// The weight of a snapshot of these lanes and shards.
    pub fn weigh(lanes: &BTreeMap<String, Arc<LaneEntry>>, shards: &[Arc<Shard>]) -> u64 {
        lanes.values().map(|l| l.weight()).sum::<u64>() + shards.iter().map(|s| s.weight).sum::<u64>()
    }
}

/// The current snapshot, and the lock that lets one build run at a time.
#[derive(Default)]
pub struct Corpora {
    current: RwLock<Option<Arc<Snapshot>>>,
    /// Held for the length of a build: the others wait, then look again at the current snapshot (a build that covered them is theirs).
    pub build: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Corpora {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Corpora").finish_non_exhaustive()
    }
}

impl Corpora {
    pub fn current(&self) -> Option<Arc<Snapshot>> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set(&self, snapshot: Option<Arc<Snapshot>>) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = snapshot;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use domain::{FileClass, NfsPath};
    use proptest::prelude::*;

    fn list(files: &[(&str, &[u8])]) -> Arc<FileList> {
        Arc::new(FileList {
            files: files
                .iter()
                .map(|(p, c)| {
                    (
                        NfsPath::parse(p).unwrap(),
                        FileClass::Text,
                        ports::content_hash(c),
                    )
                })
                .collect(),
            weight: 100,
        })
    }

    fn lanes(entries: Vec<(&str, Arc<LaneEntry>)>) -> BTreeMap<String, Arc<LaneEntry>> {
        entries.into_iter().map(|(l, e)| (l.to_owned(), e)).collect()
    }

    fn keys(lanes: &BTreeMap<String, Arc<LaneEntry>>, shards: usize) -> Vec<ContentHash> {
        (0..shards).map(|i| shard_key(i, shards, lanes)).collect()
    }

    #[test]
    fn the_shard_of_a_blob_is_in_range_and_the_same_every_time() {
        for n in 1..=64 {
            for i in 0..50_u8 {
                let h = ports::content_hash(&[i]);
                let s = shard_of(&h, n);
                assert!(s < n);
                assert_eq!(s, shard_of(&h, n));
            }
        }
        assert_eq!(shard_of(&ports::content_hash(b"x"), 0), 0, "no shards is one");
    }

    #[test]
    fn the_effective_total_is_the_request_held_to_the_ceiling() {
        assert_eq!(effective_total(10, 1_000), 10);
        assert_eq!(effective_total(5_000, 1_000), 1_000);
        assert_eq!(effective_total(0, 1_000), 0);
    }

    #[test]
    fn a_swimlane_that_only_reorders_its_files_changes_no_shard() {
        let a = list(&[
            ("a.yml", b"1"),
            ("b.yml", b"2"),
            ("dir/c.yml", b"3"),
            ("d.yml", b"4"),
        ]);
        let b = list(&[
            ("dir/c.yml", b"3"),
            ("d.yml", b"4"),
            ("a.yml", b"1"),
            ("b.yml", b"2"),
        ]);
        let one = lanes(vec![("sl-01", Arc::new(LaneEntry::new(1, a, 4)))]);
        let other = lanes(vec![("sl-01", Arc::new(LaneEntry::new(2, b, 4)))]);
        assert_eq!(keys(&one, 4), keys(&other, 4));
    }

    #[test]
    fn a_changed_file_changes_the_shard_it_left_and_the_shard_it_came_to_and_no_other() {
        let shards = 8;
        let files: Vec<(String, Vec<u8>)> = (0..40)
            .map(|i| (format!("f{i}.yml"), format!("content {i}").into_bytes()))
            .collect();
        let as_list = |files: &[(String, Vec<u8>)]| {
            let refs: Vec<(&str, &[u8])> = files.iter().map(|(p, c)| (p.as_str(), c.as_slice())).collect();
            list(&refs)
        };
        let other = list(&[("x.yml", b"other swimlane")]);
        let before = lanes(vec![
            ("sl-01", Arc::new(LaneEntry::new(1, as_list(&files), shards))),
            ("sl-02", Arc::new(LaneEntry::new(1, Arc::clone(&other), shards))),
        ]);
        let mut changed = files.clone();
        changed[7].1 = b"a new content of f7".to_vec();
        let after = lanes(vec![
            ("sl-01", Arc::new(LaneEntry::new(2, as_list(&changed), shards))),
            ("sl-02", Arc::new(LaneEntry::new(1, other, shards))),
        ]);
        let (old, new) = (
            shard_of(&ports::content_hash(&files[7].1), shards),
            shard_of(&ports::content_hash(&changed[7].1), shards),
        );
        let moved: BTreeSet<usize> = [old, new].into_iter().collect();
        let (kb, ka) = (keys(&before, shards), keys(&after, shards));
        for i in 0..shards {
            assert_eq!(
                kb[i] != ka[i],
                moved.contains(&i),
                "shard {i}: left {old}, came to {new}"
            );
        }
    }

    #[test]
    fn a_new_version_of_a_swimlane_with_the_same_files_is_the_same_corpus() {
        let a = list(&[("a.yml", b"1"), ("b.yml", b"2")]);
        let v1 = lanes(vec![("sl-01", Arc::new(LaneEntry::new(1, Arc::clone(&a), 4)))]);
        let v2 = lanes(vec![("sl-01", Arc::new(LaneEntry::new(9, a, 4)))]);
        assert_eq!(
            keys(&v1, 4),
            keys(&v2, 4),
            "the version is not in a shard's key, the files are"
        );
    }

    #[test]
    fn a_snapshot_covers_what_is_at_the_version_asked_or_later() {
        let l = lanes(vec![(
            "sl-01",
            Arc::new(LaneEntry::new(5, list(&[("a.yml", b"1")]), 2)),
        )]);
        let mut s = Snapshot {
            lanes: l,
            shards: Vec::new(),
            weight: 0,
        };
        let need = |v: i64| vec![("sl-01".to_owned(), v)];
        assert!(!s.covers(&need(5)), "no shards, nothing is covered");
        let corpus = engine::SearchCorpus::new(Vec::new()).unwrap();
        let shard = Arc::new(Shard {
            key: ports::content_hash(b"k"),
            contents: HashMap::new(),
            corpus,
            weight: 0,
        });
        s.shards = vec![Arc::clone(&shard), shard];
        assert!(s.covers(&need(5)) && s.covers(&need(4)) && s.covers(&[]));
        assert!(!s.covers(&need(6)));
        assert!(
            !s.covers(&[("sl-02".to_owned(), 0)]),
            "a swimlane that is not in it is not covered"
        );
    }

    proptest! {
        #[test]
        fn every_file_is_in_exactly_one_bucket_in_path_order(
            names in prop::collection::btree_set("[a-d]{1,3}(/[a-d]{1,3})?\\.yml", 0..40),
            shards in 1_usize..9,
        ) {
            let files: Vec<(String, Vec<u8>)> = names.iter().enumerate().map(|(i, n)| (n.clone(), vec![u8::try_from(i % 256).unwrap()])).collect();
            let refs: Vec<(&str, &[u8])> = files.iter().map(|(p, c)| (p.as_str(), c.as_slice())).collect();
            let entry = LaneEntry::new(1, list(&refs), shards);
            let mut all: Vec<u32> = entry.buckets.iter().flatten().copied().collect();
            all.sort_unstable();
            prop_assert_eq!(all, (0..u32::try_from(files.len()).unwrap()).collect::<Vec<_>>());
            for (s, bucket) in entry.buckets.iter().enumerate() {
                prop_assert_eq!(entry.digests[s].is_some(), !bucket.is_empty());
                let paths: Vec<&str> = bucket.iter().map(|&i| entry.list.files[i as usize].0.as_str()).collect();
                prop_assert!(paths.windows(2).all(|w| w[0] < w[1]));
                for &i in bucket {
                    prop_assert_eq!(shard_of(&entry.list.files[i as usize].2, shards), s);
                }
            }
        }
    }
}
