//! Text search across swimlanes (T6, P7: 400 ms, S11, A27, rule 4, rule 5).
//!
//! The engine's `search_text` (ripgrep's matcher, linear-time regex under size limits) over a **corpus**: the current stored version
//! of every readable structured or text file of the swimlanes, each distinct blob once however many swimlanes hold it. It never
//! touches NFS and never parses; what it needs from the registry is the list of files and the bytes.
//!
//! # The corpora survive a change to one swimlane
//!
//! The corpus is one [`Snapshot`] of all swimlanes, in shards split by blob hash, each with all the places of its blobs (see
//! [`super::corpora`] for why that shape, and for the proof that the answer is the single corpus's). A swimlane's file list at a
//! version is cached ([`FileList`]); when a search finds that a swimlane it covers has moved on, the snapshot is made again from the
//! one before: the moved swimlane's list is read, and only the shards whose places changed are made again, from the bytes the shard
//! already held and the few blobs it has not (through the `BlobStore` port, whose own cache is why a blob read twice is cheap). The
//! other swimlanes' lists and shards are shared with the old snapshot. Concurrent searches (and the warm-up task, [`super::warmup`])
//! share one build: it is made under a lock, and the ones that waited look again at what it made before making anything. A blob the
//! store does not have is left out.
//!
//! A search over some swimlanes reads the same snapshot and gives the engine the swimlanes' list, which keeps the places of the others
//! out of the answer. The snapshot is kept only while it is inside `corpus_cache_bytes`; one over it is made for the request.
//!
//! # Bounds
//!
//! - The pattern is at most 512 bytes and not empty; the regex is the engine's linear-time one under its size, DFA and nesting limits.
//! - The caps are the server's ceilings, 1,000 hits in all and 50 for a file (A27), whatever the request asks; REST keeps its own
//!   lower limits on top. `truncated` says that a cap cut the result.
//! - The search runs on the read pool inside `spawn_blocking`, with a cancel flag the deadline sets: after `search_timeout` (5 s) the
//!   request is answered 503 and the engine stops at its next blob, so a pattern cannot hold the pool.
//! - A build is one at a time, and its work is bounded: the lists of the swimlanes asked for, the shards that changed, the blobs they
//!   lack, in batches, each call with a timeout. A viewer cannot make more builds than there are changes to the data.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use bytes::Bytes;
use domain::{ContentHash, FileClass, NfsPath, SwimlaneId, TextQuery, TextResults};
use engine::{
    CancelFlag, DenyMatcher, Limits, SearchBlob, SearchCorpus, SearchError, SearchLocation, search_text,
};
use futures::stream::{self, StreamExt};
use ports::ReadError;
use sqlx::Row;
use tracing::{debug, warn};

use super::cache::FileList;
use super::corpora::{LaneEntry, Shard, Snapshot, effective_total, merge, shard_key};
use super::rows;
use super::service::Inner;
use crate::metrics::CacheName;

/// The readable files of a swimlane: the current version of each structured or text file that is not denied.
pub const FILES_SQL: &str = "\
SELECT path, class, nfs_hash::bytea AS hash FROM file_state \
WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL AND NOT denied AND class IN ('structured', 'text') LIMIT 200001";

pub const VERSIONS_SQL: &str =
    "SELECT swimlane_id, version FROM swimlane_registry WHERE swimlane_id = ANY($1::text[])";

fn map_error(e: SearchError) -> ReadError {
    match e {
        SearchError::EmptyPattern
        | SearchError::PatternTooLong
        | SearchError::InvalidPattern
        | SearchError::InvalidGlob => ReadError::BadRequest,
        SearchError::CorpusTooLarge | SearchError::Cancelled => ReadError::Unavailable,
    }
}

impl Inner {
    /// The swimlanes a search covers and their versions: those asked for (each must exist), or all of them.
    pub(crate) async fn search_scope(&self, asked: &[SwimlaneId]) -> Result<Vec<(String, i64)>, ReadError> {
        let lanes: Vec<String> = if asked.is_empty() {
            self.db
                .run(
                    "read_all_swimlanes",
                    sqlx::query_scalar("SELECT id FROM swimlanes ORDER BY id LIMIT 1000")
                        .fetch_all(self.db.pool()),
                )
                .await?
        } else {
            let mut ids: Vec<&str> = asked.iter().map(SwimlaneId::as_str).collect();
            ids.sort_unstable();
            ids.dedup();
            let existing = self.existing(&ids).await?;
            if existing.len() != ids.len() {
                return Err(ReadError::NotFound);
            }
            ids.into_iter().map(str::to_owned).collect()
        };
        let fetched = self
            .db
            .run(
                "read_versions",
                sqlx::query(VERSIONS_SQL).bind(&lanes).fetch_all(self.db.pool()),
            )
            .await?;
        let versions: BTreeMap<String, i64> = fetched
            .iter()
            .map(|r| (r.get::<String, _>("swimlane_id"), r.get::<i64, _>("version")))
            .collect();
        Ok(lanes
            .into_iter()
            .map(|l| {
                let v = versions.get(&l).copied().unwrap_or(0);
                (l, v)
            })
            .collect())
    }

    /// The file list of one swimlane at one version.
    async fn file_list(&self, lane: &str, version: i64) -> Result<Arc<FileList>, ReadError> {
        let key = (lane.to_owned(), version);
        if let Some(list) = self.caches.files.get(&key).await {
            self.caches.count(CacheName::Files, true);
            return Ok(list);
        }
        self.caches.count(CacheName::Files, false);
        let fetched = self
            .db
            .run(
                "read_file_list",
                sqlx::query(FILES_SQL).bind(lane).fetch_all(self.db.pool()),
            )
            .await?;
        let mut files: Vec<(NfsPath, FileClass, ContentHash)> = Vec::with_capacity(fetched.len());
        let mut bytes = 256_usize;
        for row in &fetched {
            let path: String = row.get("path");
            let class: String = row.get("class");
            let hash: Vec<u8> = row.get("hash");
            files.push((
                NfsPath::parse(&path).map_err(|_| ReadError::Unavailable)?,
                rows::class(&class)?,
                rows::hash(&hash)?,
            ));
            bytes += 96 + path.len();
        }
        let list = Arc::new(FileList {
            files,
            weight: u32::try_from(bytes).unwrap_or(u32::MAX),
        });
        self.caches.files.insert(key, Arc::clone(&list)).await;
        Ok(list)
    }

    /// The bytes of these blobs, in batches of the port's limit, several batches at once, each call with the blob timeout. A blob the
    /// store does not have is not in the answer.
    async fn fetch_blobs(&self, hashes: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, ReadError> {
        let batches: Vec<Vec<ContentHash>> = hashes
            .chunks(self.config.search_blob_batch)
            .map(<[ContentHash]>::to_vec)
            .collect();
        let fetched: Vec<Vec<(ContentHash, Bytes)>> = stream::iter(batches)
            .map(|batch| async move {
                tokio::time::timeout(self.config.blob_timeout, self.blobs.get_many(&batch))
                    .await
                    .map_err(|_| {
                        warn!(op = "read_corpus", why = "timed_out", "blob read failed");
                        ReadError::Unavailable
                    })?
                    .map_err(|e| {
                        warn!(op = "read_corpus", ?e, "blob read failed");
                        ReadError::Unavailable
                    })
            })
            .buffer_unordered(self.config.search_blob_parallel)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;
        Ok(fetched.into_iter().flatten().collect())
    }

    /// One shard, made from the swimlanes' file lists: where each of its blobs is, the bytes (from the shard it replaces where that has
    /// them, from the store for the rest), and the engine's corpus of them. The CPU work is on the blocking pool (rule 4).
    async fn build_shard(
        &self,
        index: usize,
        key: ContentHash,
        lanes: &BTreeMap<String, Arc<LaneEntry>>,
        old: Option<&Arc<Shard>>,
    ) -> Result<Arc<Shard>, ReadError> {
        let lanes: Vec<(String, Arc<LaneEntry>)> =
            lanes.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();
        let places = tokio::task::spawn_blocking(move || places_of_shard(&lanes, index))
            .await
            .map_err(|_| {
                warn!("a corpus task on the blocking pool did not finish");
                ReadError::Unavailable
            })??;
        let missing: Vec<ContentHash> = places
            .keys()
            .filter(|h| old.is_none_or(|o| !o.contents.contains_key(*h)))
            .copied()
            .collect();
        let fetched = if missing.is_empty() {
            Vec::new()
        } else {
            self.fetch_blobs(&missing).await?
        };
        let old = old.map(Arc::clone);
        tokio::task::spawn_blocking(move || assemble_shard(key, places, old.as_deref(), fetched))
            .await
            .map_err(|_| {
                warn!("a corpus task on the blocking pool did not finish");
                ReadError::Unavailable
            })?
            .map(Arc::new)
    }

    /// The snapshot after `old` that covers `required`: the lists of the swimlanes that have moved on are read, and the shards whose
    /// places changed are made again; everything else is the old snapshot's. With `keep`, only those swimlanes stay in it.
    async fn build_snapshot(
        &self,
        old: Option<&Arc<Snapshot>>,
        required: &[(String, i64)],
        keep: Option<&BTreeSet<String>>,
    ) -> Result<Arc<Snapshot>, ReadError> {
        let count = self.config.search_shards;
        let mut lanes: BTreeMap<String, Arc<LaneEntry>> = old.map(|s| s.lanes.clone()).unwrap_or_default();
        let stale: Vec<(String, i64)> = required
            .iter()
            .filter(|(lane, version)| lanes.get(lane).is_none_or(|e| e.version < *version))
            .cloned()
            .collect();
        // The file lists, a few swimlanes at a time; the version of each was read before its list.
        let lists: Vec<(String, i64, Arc<FileList>)> = stream::iter(stale)
            .map(|(lane, version)| async move {
                self.file_list(&lane, version)
                    .await
                    .map(|list| (lane, version, list))
            })
            .buffered(8)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;
        let reloaded = lists.len();
        let entries = tokio::task::spawn_blocking(move || {
            lists
                .into_iter()
                .map(|(lane, version, list)| (lane, Arc::new(LaneEntry::new(version, list, count))))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| {
            warn!("a corpus task on the blocking pool did not finish");
            ReadError::Unavailable
        })?;
        lanes.extend(entries);
        if let Some(keep) = keep {
            lanes.retain(|lane, _| keep.contains(lane));
        }

        let keys: Vec<ContentHash> = (0..count).map(|i| shard_key(i, count, &lanes)).collect();
        let reused: Vec<Option<Arc<Shard>>> = keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                old.and_then(|s| s.shards.get(i))
                    .filter(|shard| shard.key == *key && shard.complete)
                    .map(Arc::clone)
            })
            .collect();
        let changed: Vec<usize> = (0..count)
            .filter(|i| reused.get(*i).is_some_and(Option::is_none))
            .collect();
        for i in 0..count {
            self.caches.count(CacheName::CorpusShard, !changed.contains(&i));
        }
        // Two shards at a time: the work is bounded, and a build does not take every thread of the blocking pool.
        let made: Vec<(usize, Arc<Shard>)> = stream::iter(changed.iter().copied())
            .map(|i| {
                let (lanes, keys, previous) = (&lanes, &keys, old.and_then(|s| s.shards.get(i)));
                async move {
                    let key = *keys.get(i).ok_or(ReadError::Unavailable)?;
                    self.build_shard(i, key, lanes, previous)
                        .await
                        .map(|shard| (i, shard))
                }
            })
            .buffer_unordered(2)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;
        let mut made: BTreeMap<usize, Arc<Shard>> = made.into_iter().collect();
        let shards: Vec<Arc<Shard>> = reused
            .into_iter()
            .enumerate()
            .filter_map(|(i, kept)| kept.or_else(|| made.remove(&i)))
            .collect();
        if shards.len() != count {
            return Err(ReadError::Unavailable);
        }
        let weight = Snapshot::weigh(&lanes, &shards);
        debug!(
            op = "build_corpora",
            lists_read = reloaded,
            shards_made = changed.len(),
            shards = count,
            weight,
            "the text corpora were brought up to date"
        );
        Ok(Arc::new(Snapshot {
            lanes,
            shards,
            weight,
        }))
    }

    /// The snapshot that covers these (swimlane, version)s: the current one if it does, otherwise the next one, made once for everyone
    /// who needs it. With `keep`, a build leaves out the swimlanes that are not in it (the search was of all swimlanes, so it names those
    /// there are).
    pub(crate) async fn corpus_snapshot(
        &self,
        required: &[(String, i64)],
        keep: Option<&BTreeSet<String>>,
    ) -> Result<Arc<Snapshot>, ReadError> {
        let corpora = &self.caches.corpora;
        if let Some(snapshot) = corpora.current().filter(|s| s.covers(required)) {
            self.caches.count(CacheName::Corpus, true);
            return Ok(snapshot);
        }
        // One build at a time; the ones that waited for it look at what it made before making anything of their own.
        let _turn = corpora.build.lock().await;
        let old = corpora.current();
        if let Some(snapshot) = old.as_ref().filter(|s| s.covers(required)) {
            self.caches.count(CacheName::Corpus, true);
            return Ok(Arc::clone(snapshot));
        }
        self.caches.count(CacheName::Corpus, false);
        let snapshot = self.build_snapshot(old.as_ref(), required, keep).await?;
        if snapshot.weight <= self.config.corpus_cache_bytes {
            corpora.set(Some(Arc::clone(&snapshot)));
        } else {
            // Too big to keep: this request has it, and the next one makes its own (the old one is dropped with its memory).
            warn!(
                op = "build_corpora",
                weight = snapshot.weight,
                "the text corpora are over corpus_cache_bytes and are not kept"
            );
            corpora.set(None);
        }
        Ok(snapshot)
    }

    /// `searchText`.
    pub async fn search_text(&self, q: &TextQuery) -> Result<TextResults, ReadError> {
        if q.pattern.is_empty()
            || q.pattern.len() > TextQuery::MAX_PATTERN_BYTES
            || q.max_total == 0
            || q.max_per_file == 0
        {
            return Err(ReadError::BadRequest);
        }
        let scope = self.search_scope(&q.swimlanes).await?;
        if scope.is_empty() {
            return Ok(TextResults {
                hits: Vec::new(),
                truncated: false,
            });
        }
        // A search of every swimlane names the ones there are, so that a build leaves out any that went away.
        let keep: Option<BTreeSet<String>> = q
            .swimlanes
            .is_empty()
            .then(|| scope.iter().map(|(lane, _)| lane.clone()).collect());
        let snapshot = self.corpus_snapshot(&scope, keep.as_ref()).await?;

        // The snapshot has the places of every swimlane it was made from; the query's list is what keeps the others out.
        let swimlanes: Vec<SwimlaneId> = scope
            .iter()
            .map(|(lane, _)| SwimlaneId::parse(lane).map_err(|_| ReadError::Unavailable))
            .collect::<Result<_, _>>()?;
        let query = TextQuery {
            swimlanes,
            ..q.clone()
        };
        let cancel = Arc::new(CancelFlag::new());
        let (pool, deny, limits, flag) = (
            Arc::clone(&self.pool),
            Arc::clone(&self.deny),
            self.limits.clone(),
            Arc::clone(&cancel),
        );
        let job = tokio::task::spawn_blocking(move || {
            search_shards(&snapshot, &query, &deny, &limits, &pool, &flag)
        });
        let Ok(joined) = tokio::time::timeout(self.config.search_timeout, job).await else {
            // The deadline: stop the engine at its next blob and answer now.
            cancel.cancel();
            warn!(
                op = "search_text",
                why = "deadline",
                "a text search was cancelled"
            );
            return Err(ReadError::Unavailable);
        };
        let results = joined
            .map_err(|_| {
                warn!("a search task on the blocking pool did not finish");
                ReadError::Unavailable
            })?
            .map_err(map_error)?;
        if results.truncated {
            self.metrics.read_search_truncated();
        }
        // The engine has no tenant set and names neither (CG2): the registry does, from the classification of the files the hits are in.
        let mut results = results;
        let places: Vec<(&SwimlaneId, &NfsPath)> =
            results.hits.iter().map(|h| (&h.swimlane, &h.path)).collect();
        let logical = self.logical_of(&places).await?;
        for hit in &mut results.hits {
            (hit.logical_file, hit.tenant) = logical.of(&hit.swimlane, &hit.path);
        }
        Ok(results)
    }
}

/// Where each blob of a shard is: its places in every swimlane, from the swimlanes' lists. CPU work for the blocking pool: at the target
/// scale a shard has about 10,000 places.
fn places_of_shard(
    lanes: &[(String, Arc<LaneEntry>)],
    index: usize,
) -> Result<HashMap<ContentHash, Vec<SearchLocation>>, ReadError> {
    let mut places: HashMap<ContentHash, Vec<SearchLocation>> = HashMap::new();
    for (lane, entry) in lanes {
        let Some(bucket) = entry.buckets.get(index) else {
            continue;
        };
        if bucket.is_empty() {
            continue;
        }
        let swimlane = SwimlaneId::parse(lane).map_err(|_| ReadError::Unavailable)?;
        for &i in bucket {
            let Some((path, _, hash)) = entry.list.files.get(i as usize) else {
                continue;
            };
            places.entry(*hash).or_default().push(SearchLocation {
                swimlane: swimlane.clone(),
                path: path.clone(),
            });
        }
    }
    Ok(places)
}

/// The shard from its places and its bytes. A blob that neither the shard it replaces nor the store had is left out, with its places.
fn assemble_shard(
    key: ContentHash,
    places: HashMap<ContentHash, Vec<SearchLocation>>,
    old: Option<&Shard>,
    fetched: Vec<(ContentHash, Bytes)>,
) -> Result<Shard, ReadError> {
    let mut fetched: HashMap<ContentHash, Bytes> = fetched.into_iter().collect();
    let mut contents: HashMap<ContentHash, Bytes> = HashMap::with_capacity(places.len());
    let mut blobs: Vec<SearchBlob> = Vec::with_capacity(places.len());
    let (mut bytes, mut place_count) = (0_u64, 0_usize);
    let mut complete = true;
    for (hash, locations) in places {
        let content = old
            .and_then(|o| o.contents.get(&hash).cloned())
            .or_else(|| fetched.remove(&hash));
        let Some(content) = content else {
            complete = false;
            continue;
        };
        bytes += content.len() as u64;
        place_count += locations.len();
        contents.insert(hash, content.clone());
        blobs.push(SearchBlob {
            hash,
            content,
            locations,
        });
    }
    // Deterministic whatever order the map gave them in.
    blobs.sort_by_key(|b| b.hash);
    let corpus = SearchCorpus::new(blobs).map_err(map_error)?;
    Ok(Shard {
        key,
        contents,
        corpus,
        weight: bytes
            .saturating_add(place_count as u64 * 100)
            .saturating_add(4096),
        complete,
    })
}

/// The engine on every shard (side by side on the read pool: they are made of different blobs) and the answers put together. Each
/// shard gets the query as it is; the answer is the one a single corpus of all of them would give (`corpora::merge`).
fn search_shards(
    snapshot: &Snapshot,
    query: &TextQuery,
    deny: &DenyMatcher,
    limits: &Limits,
    pool: &rayon::ThreadPool,
    cancel: &CancelFlag,
) -> Result<TextResults, SearchError> {
    use rayon::prelude::*;
    let parts: Vec<Result<engine::SearchOutcome, SearchError>> = pool.install(|| {
        snapshot
            .shards
            .par_iter()
            .map(|shard| search_text(&shard.corpus, query, deny, limits, pool, cancel))
            .collect()
    });
    let parts: Vec<TextResults> = parts
        .into_iter()
        .map(|p| p.map(|outcome| outcome.results))
        .collect::<Result<_, _>>()?;
    Ok(merge(parts, effective_total(query.max_total, limits.hits_total)))
}
