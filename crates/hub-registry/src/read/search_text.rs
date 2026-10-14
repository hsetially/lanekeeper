//! Text search across swimlanes (T6, P7: 400 ms, S11, A27, rule 4, rule 5).
//!
//! The engine's `search_text` (ripgrep's matcher, linear-time regex under size limits) over a **corpus**: the current stored version
//! of every readable structured or text file of the swimlanes, each distinct blob once however many swimlanes hold it. It never
//! touches NFS and never parses; what it needs from the registry is the list of files and the bytes.
//!
//! # The corpus is built once for a set of swimlane versions
//!
//! `swimlane_registry.version` moves on every applied change to a swimlane (ingestion and the pipeline both bump it), so a set of
//! (swimlane, version) pairs names exactly the files there are. The file list of a swimlane at a version is cached
//! ([`FileList`]), and so is the corpus of a set of them ([`CorpusEntry`], weighed by the bytes of the blobs it holds, so one too big
//! for `corpus_cache_bytes` is built for the request and not kept). Concurrent searches over the same set share one build
//! (singleflight): a cold first search reads the blobs in batches of 1,000, four batches at once, and the rest wait for it. The blobs
//! come through the `BlobStore` port, whose own cache is why a corpus built a second time is cheap. A blob the store does not have
//! is left out.
//!
//! # Bounds
//!
//! - The pattern is at most 512 bytes and not empty; the regex is the engine's linear-time one under its size, DFA and nesting limits.
//! - The caps are the server's ceilings, 1,000 hits in all and 50 for a file (A27), whatever the request asks; REST keeps its own
//!   lower limits on top. `truncated` says that a cap cut the result.
//! - The search runs on the read pool inside `spawn_blocking`, with a cancel flag the deadline sets: after `search_timeout` (5 s) the
//!   request is answered 503 and the engine stops at its next blob, so a pattern cannot hold the pool.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::Bytes;
use domain::{ContentHash, FileClass, NfsPath, SwimlaneId, TextQuery, TextResults};
use engine::{CancelFlag, SearchBlob, SearchCorpus, SearchError, SearchLocation, search_text};
use futures::stream::{self, StreamExt};
use ports::ReadError;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tracing::warn;

use super::cache::{CorpusEntry, FileList};
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
    async fn search_scope(&self, asked: &[SwimlaneId]) -> Result<Vec<(String, i64)>, ReadError> {
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

    /// Build the corpus of these swimlanes: the blobs of their current files, each once, with every place that holds it.
    async fn build_corpus(&self, scope: &[(String, i64)]) -> Result<Arc<CorpusEntry>, ReadError> {
        // The file lists, a few swimlanes at a time.
        let lists: Vec<(String, Arc<FileList>)> = stream::iter(scope.iter().cloned())
            .map(|(lane, version)| async move { self.file_list(&lane, version).await.map(|l| (lane, l)) })
            .buffered(8)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;

        // Where each blob is: 80,000 places at the target scale, each a swimlane and a path, built off the async threads (rule 4) and in
        // a hash map (the order of the blobs is made below, by hash, whatever order this has).
        let places: HashMap<ContentHash, Vec<SearchLocation>> = tokio::task::spawn_blocking(move || {
            let mut places: HashMap<ContentHash, Vec<SearchLocation>> =
                HashMap::with_capacity(lists.iter().map(|(_, l)| l.files.len()).sum::<usize>() / 2);
            for (lane, list) in &lists {
                let swimlane = SwimlaneId::parse(lane).map_err(|_| ReadError::Unavailable)?;
                for (path, _, hash) in &list.files {
                    places.entry(*hash).or_default().push(SearchLocation {
                        swimlane: swimlane.clone(),
                        path: path.clone(),
                    });
                }
            }
            Ok::<_, ReadError>(places)
        })
        .await
        .map_err(|_| {
            warn!("a corpus task on the blocking pool did not finish");
            ReadError::Unavailable
        })??;

        // The bytes, in batches of the port's limit, several batches at once.
        let hashes: Vec<ContentHash> = places.keys().copied().collect();
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

        // The corpus: the blobs that came back, each with its places, in hash order, sorted by place for the engine (all of it CPU).
        let (corpus, total_bytes, place_count) = tokio::task::spawn_blocking(move || {
            let mut places = places;
            let mut total_bytes: u64 = 0;
            let mut blobs: Vec<SearchBlob> = Vec::with_capacity(places.len());
            let mut place_count = 0_usize;
            for (hash, content) in fetched.into_iter().flatten() {
                if let Some(locations) = places.remove(&hash) {
                    total_bytes += content.len() as u64;
                    place_count += locations.len();
                    blobs.push(SearchBlob {
                        hash,
                        content,
                        locations,
                    });
                }
            }
            // Deterministic whatever order the batches came back in.
            blobs.sort_by_key(|b| b.hash);
            SearchCorpus::new(blobs).map(|corpus| (corpus, total_bytes, place_count))
        })
        .await
        .map_err(|_| {
            warn!("a corpus task on the blocking pool did not finish");
            ReadError::Unavailable
        })?
        .map_err(map_error)?;
        let weight = total_bytes
            .saturating_add(place_count as u64 * 100)
            .saturating_add(4096);
        Ok(Arc::new(CorpusEntry {
            corpus,
            bytes: total_bytes,
            weight: u32::try_from(weight).unwrap_or(u32::MAX),
        }))
    }

    /// The corpus of a scope, built once for concurrent callers.
    async fn corpus_of(&self, scope: &[(String, i64)]) -> Result<Arc<CorpusEntry>, ReadError> {
        let mut h = Sha256::new();
        h.update(b"lanekeeper/corpus-key/v1\n");
        for (lane, version) in scope {
            h.update((lane.len() as u64).to_be_bytes());
            h.update(lane.as_bytes());
            h.update(version.to_be_bytes());
        }
        let key = ContentHash::from_bytes(h.finalize().into());
        if let Some(entry) = self.caches.corpus.get(&key).await {
            self.caches.count(CacheName::Corpus, true);
            return Ok(entry);
        }
        self.caches.count(CacheName::Corpus, false);
        self.caches
            .corpus
            .try_get_with(key, self.build_corpus(scope))
            .await
            .map_err(|e| *e)
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
        let entry = self.corpus_of(&scope).await?;

        // Every place of the corpus is in the scope already, so the query's own swimlane list adds nothing.
        let query = TextQuery {
            swimlanes: Vec::new(),
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
            search_text(&entry.corpus, &query, &deny, &limits, &pool, &flag)
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
        let outcome = joined
            .map_err(|_| {
                warn!("a search task on the blocking pool did not finish");
                ReadError::Unavailable
            })?
            .map_err(map_error)?;
        if outcome.results.truncated {
            self.metrics.read_search_truncated();
        }
        Ok(outcome.results)
    }
}
