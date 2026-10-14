//! The settings index (T2, D65, S13): one set of rows for every unique structured blob, written once, whoever sees the blob first.
//!
//! Settings search, the grid and feature status are built on these rows (T6), so no request ever parses YAML.
//!
//! # Where the work happens
//!
//! - **Off the acknowledgement path.** Ingestion offers the hash of every structured blob that becomes a file's current version
//!   ([`SettingsIndexer::offer`]): a lock on a small set and a `try_send` on a bounded queue, nothing else. A [`SettingsWorker`] takes
//!   them in batches. A hash offered to a full queue is dropped and counted; the sweeper ([`super::SettingsSweeper`]) finds the
//!   current blobs that have no marker, so the bound costs latency and never an index row.
//! - **CPU work on rayon, inside `spawn_blocking`** (rule 4): `engine::index_settings` and `engine::secret_scan` for each blob of a
//!   batch, on a pool of `pool_threads`. The pool is the registry's own, so a burst of indexing cannot starve the readers.
//! - **One short transaction per batch** writes the rows of every blob and then the markers. The bytes were read and parsed before it
//!   began, so no transaction is held across a blob read or a parse.
//!
//! # Exactly once
//!
//! The key of a row is `(blob_hash, setting_path)` and the key of a marker is the blob hash. Rows and markers are `INSERT ... ON
//! CONFLICT DO NOTHING`, so two replicas (or the queue and the sweeper) that index the same blob at once write one set of rows: the
//! engine is deterministic, so the sets are equal, and the second writer's statements change nothing. A blob that already has a marker
//! is skipped before it is read. A blob that does not parse (or is over a limit) gets a marker with `parse_ok = false` and no rows, so
//! it is parsed once and not on every sweep. A blob that could not be indexed *now* (the database failed) gets no marker, so the next
//! sweep tries again.
//!
//! The index is keyed by content, so a blob is indexed as the syntax of the first file that brought it. The same bytes under a `.yml`
//! and a `.properties` name would read differently; the case does not occur in practice and the first reading stays (the rows are
//! the same for the JSON that is also YAML).
//!
//! # Secrets never enter the table (S13)
//!
//! The scan (`engine::secret_scan`) runs over the whole text. A setting whose lines hold a hit is stored as
//! `[REDACTED: N chars, rule]` (N is the length of the whole value, the rule the first hit's) with the rule in `secret_rule`. This
//! errs towards hiding: a hit on a line that several settings share (a flow mapping, a minified JSON file) redacts all of them. If
//! the scan cannot finish (more hits than it will list) every value of the file is stored redacted with the rule `unscanned`: a
//! value that was not checked is not shown. Revealing a value is an audited action that reads the blob, never this table (D79).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use bytes::Bytes;
use domain::{ContentHash, ValueType};
use engine::{Limits, ScanConfig, SecretMatch, SettingRow, Syntax, decode_text, index_settings, secret_scan};
use ports::BlobStore;
use rayon::ThreadPool;
use rayon::prelude::*;
use sqlx::PgPool;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, warn};

use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::{Db, Txn, ts};
use crate::error::RegistryError;
use crate::metrics::{IndexResult, RegistryMetrics};

/// The rule written where the scan could not finish and the value was hidden for safety.
const UNSCANNED: &str = "unscanned";

/// One blob to index: its hash, and how to read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexJob {
    pub hash: ContentHash,
    pub syntax: Syntax,
}

/// What [`SettingsIndexer::offer`] did with a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    Queued,
    /// Already waiting, or being indexed, on this replica.
    Duplicate,
    /// The queue is full; the sweeper will find the blob.
    Dropped,
}

/// One row of `settings_index`, as it is written.
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    /// The value as written, or its redaction marker.
    pub value: String,
    pub ty: &'static str,
    pub start_line: i32,
    pub end_line: i32,
    /// The rule that flagged the value, when it is stored redacted.
    pub secret_rule: Option<&'static str>,
}

impl std::fmt::Debug for Row {
    // A value may be a secret (S13, S21): the row shows its size, never its text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("path", &self.path)
            .field("value_bytes", &self.value.len())
            .field("ty", &self.ty)
            .field("lines", &(self.start_line, self.end_line))
            .field("secret_rule", &self.secret_rule)
            .finish()
    }
}

/// What one blob became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Indexed {
    /// The blob parsed: its rows, flagged values already redacted. A file with no settings is `Rows(vec![])`.
    Rows(Vec<Row>),
    /// The blob does not parse, or is over a limit.
    Unparseable,
}

/// The name of a value type as `settings_index.value_type` holds it: the wire name of `domain::ValueType`.
pub const fn type_name(ty: ValueType) -> &'static str {
    match ty {
        ValueType::String => "string",
        ValueType::Int => "int",
        ValueType::Float => "float",
        ValueType::Bool => "bool",
        ValueType::Null => "null",
        ValueType::Other => "other",
    }
}

/// Postgres text cannot hold a NUL; a value that has one (a file with a raw NUL inside a quoted scalar) must not make the whole
/// blob unwritable, so each is replaced by U+FFFD.
fn clean(text: &str) -> String {
    if text.contains('\0') {
        text.replace('\0', "\u{FFFD}")
    } else {
        text.to_owned()
    }
}

fn marker(value_chars: usize, rule: &str) -> String {
    format!("[REDACTED: {value_chars} chars, {rule}]")
}

/// The rule of the first hit on lines `start..=end`, if there is one. `hits` is in line order.
fn hit_in(hits: &[SecretMatch], start: u32, end: u32) -> Option<&'static str> {
    let first = hits.partition_point(|h| h.line < start);
    hits.get(first).filter(|h| h.line <= end).map(|h| h.rule.name())
}

/// A blob as rows (pure: no I/O, no clock). See the module documentation for the redaction rules.
pub fn index_blob(data: &[u8], syntax: Syntax, limits: &Limits, scan: &ScanConfig) -> Indexed {
    let Ok(index) = index_settings(data, syntax, limits) else {
        return Indexed::Unparseable;
    };
    // What the scan judges is the text the parser read. A text that cannot be decoded here cannot be checked: hide every value.
    let hits: Option<Vec<SecretMatch>> = decode_text(data, limits)
        .ok()
        .and_then(|d| secret_scan(&d.text, scan, limits).ok());
    let rows = index
        .rows
        .iter()
        .filter_map(|row| row_of(row, hits.as_deref()))
        .collect();
    Indexed::Rows(rows)
}

fn row_of(row: &SettingRow, hits: Option<&[SecretMatch]>) -> Option<Row> {
    let start_line = i32::try_from(row.lines.start()).ok()?;
    let end_line = i32::try_from(row.lines.end()).ok()?;
    let rule = match hits {
        None => Some(UNSCANNED),
        Some(hits) => hit_in(hits, row.lines.start(), row.lines.end()),
    };
    let (value, secret_rule) = match rule {
        Some(rule) => (marker(row.value.text.chars().count(), rule), Some(rule)),
        None => (clean(&row.value.text), None),
    };
    Some(Row {
        path: row.path.as_str().to_owned(),
        value,
        ty: type_name(row.value.ty),
        start_line,
        end_line,
        secret_rule,
    })
}

// ------------------------------------------------------------------------------------------------ the indexer

/// What the indexer needs from the rest of the hub.
pub struct IndexDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for IndexDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexDeps").finish_non_exhaustive()
    }
}

/// What one batch did, in counts. Nothing in it is content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub indexed: u64,
    pub unparseable: u64,
    pub already_indexed: u64,
    pub missing: u64,
    pub failed: u64,
    pub rows: u64,
    pub redacted: u64,
}

impl IndexReport {
    fn add(&mut self, other: Self) {
        self.indexed += other.indexed;
        self.unparseable += other.unparseable;
        self.already_indexed += other.already_indexed;
        self.missing += other.missing;
        self.failed += other.failed;
        self.rows += other.rows;
        self.redacted += other.redacted;
    }
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct Inner {
    pub(crate) db: Db,
    blobs: Arc<dyn BlobStore>,
    clock: Arc<dyn Clock>,
    config: RegistryConfig,
    metrics: RegistryMetrics,
    pool: Arc<ThreadPool>,
    limits: Limits,
    scan: ScanConfig,
    /// Hashes waiting in the queue or being indexed by the worker. At most `index_queue` plus one batch.
    inflight: Mutex<HashSet<ContentHash>>,
    idle: Notify,
}

/// The handle ingestion and the sweeper use. Cloning shares the queue.
#[derive(Clone)]
pub struct SettingsIndexer {
    inner: Arc<Inner>,
    queue: mpsc::Sender<IndexJob>,
}

impl std::fmt::Debug for SettingsIndexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsIndexer").finish_non_exhaustive()
    }
}

/// The task that drains the queue. Spawn [`SettingsWorker::run`] once per replica; it ends when every [`SettingsIndexer`] is dropped.
pub struct SettingsWorker {
    inner: Arc<Inner>,
    queue: mpsc::Receiver<IndexJob>,
}

impl std::fmt::Debug for SettingsWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsWorker").finish_non_exhaustive()
    }
}

impl SettingsIndexer {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range, or the thread pool cannot be built.
    pub fn new(deps: IndexDeps) -> Result<(Self, SettingsWorker), ConfigError> {
        deps.config.validate()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(deps.config.pool_threads)
            .thread_name(|i| format!("registry-index-{i}"))
            .build()
            .map_err(|_| ConfigError::Invalid("pool_threads"))?;
        let limits = Limits {
            input_bytes: usize::try_from(deps.config.max_file_bytes).unwrap_or(usize::MAX),
            ..Limits::default()
        };
        let (queue_tx, queue_rx) = mpsc::channel(deps.config.index_queue);
        let inner = Arc::new(Inner {
            db: Db::new(deps.pool, deps.config.query_timeout),
            blobs: deps.blobs,
            clock: deps.clock,
            config: deps.config,
            metrics: deps.metrics,
            pool: Arc::new(pool),
            limits,
            scan: ScanConfig::default(),
            inflight: Mutex::new(HashSet::new()),
            idle: Notify::new(),
        });
        Ok((
            Self {
                inner: Arc::clone(&inner),
                queue: queue_tx,
            },
            SettingsWorker {
                inner,
                queue: queue_rx,
            },
        ))
    }

    /// Offer a blob to the queue, without waiting. Never blocks and never fails: a hash that cannot be queued is counted, and the
    /// sweeper finds the blob.
    pub fn offer(&self, job: IndexJob) -> Offer {
        let offer = {
            let mut inflight = locked(&self.inner.inflight);
            if !inflight.insert(job.hash) {
                Offer::Duplicate
            } else if self.queue.try_send(job).is_ok() {
                Offer::Queued
            } else {
                inflight.remove(&job.hash);
                Offer::Dropped
            }
        };
        let result = match offer {
            Offer::Queued => None,
            Offer::Duplicate => Some(IndexResult::Duplicate),
            Offer::Dropped => Some(IndexResult::Dropped),
        };
        if let Some(result) = result {
            self.inner.metrics.index_blob(result, 1);
        }
        self.inner
            .metrics
            .index_queue(self.queue.max_capacity() - self.queue.capacity());
        offer
    }

    /// Index these blobs now, in this task, in batches. For the sweeper and for tests; the queue is for the hot path.
    pub async fn index_now(&self, jobs: Vec<IndexJob>) -> Result<IndexReport, RegistryError> {
        let mut total = IndexReport::default();
        let mut seen = HashSet::new();
        let jobs: Vec<IndexJob> = jobs.into_iter().filter(|j| seen.insert(j.hash)).collect();
        for batch in jobs.chunks(self.inner.config.index_batch) {
            total.add(self.inner.index_batch(batch).await?);
        }
        Ok(total)
    }

    /// Returns when nothing is waiting in the queue or being indexed by the worker. For tests and for a graceful stop.
    pub async fn idle(&self) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            // Registered before the check, so that a notification between the check and the await is not lost.
            notified.as_mut().enable();
            if locked(&self.inner.inflight).is_empty() {
                return;
            }
            notified.await;
        }
    }

    /// The hashes that are waiting or being indexed.
    pub fn pending(&self) -> usize {
        locked(&self.inner.inflight).len()
    }

    pub(crate) fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl SettingsWorker {
    /// Drain the queue until every handle is dropped. A failed batch is counted and logged, and the loop goes on: the blobs of it
    /// have no marker, so the sweeper takes them.
    pub async fn run(mut self) {
        while let Some(first) = self.queue.recv().await {
            let mut jobs = vec![first];
            while jobs.len() < self.inner.config.index_batch {
                match self.queue.try_recv() {
                    Ok(job) => jobs.push(job),
                    Err(_) => break,
                }
            }
            if let Err(error) = self.inner.index_batch(&jobs).await {
                self.inner
                    .metrics
                    .index_blob(IndexResult::Failed, jobs.len() as u64);
                warn!(blobs = jobs.len(), %error, "settings index batch failed");
            }
            {
                let mut inflight = locked(&self.inner.inflight);
                for job in &jobs {
                    inflight.remove(&job.hash);
                }
            }
            self.inner.metrics.index_queue(self.queue.len());
            self.inner.idle.notify_waiters();
        }
    }
}

impl Inner {
    /// The blobs of a batch that have a marker already.
    async fn marked(&self, hashes: &[ContentHash]) -> Result<HashSet<ContentHash>, RegistryError> {
        let raw: Vec<Vec<u8>> = hashes.iter().map(|h| h.as_bytes().to_vec()).collect();
        let found: Vec<Vec<u8>> = self
            .db
            .run(
                "index_markers",
                sqlx::query_scalar(MARKED_SQL)
                    .bind(&raw)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(found.into_iter().filter_map(|b| hash_of(&b)).collect())
    }

    /// One batch: skip what is indexed, read the bytes, parse on the pool, write in one transaction.
    pub(crate) async fn index_batch(&self, jobs: &[IndexJob]) -> Result<IndexReport, RegistryError> {
        let started = Instant::now();
        let mut report = IndexReport::default();
        let hashes: Vec<ContentHash> = jobs.iter().map(|j| j.hash).collect();
        let marked = self.marked(&hashes).await?;
        let todo: Vec<IndexJob> = jobs
            .iter()
            .copied()
            .filter(|j| !marked.contains(&j.hash))
            .collect();
        report.already_indexed = (jobs.len() - todo.len()) as u64;
        if !todo.is_empty() {
            let wanted: Vec<ContentHash> = todo.iter().map(|j| j.hash).collect();
            let stored = tokio::time::timeout(self.config.blob_timeout, self.blobs.get_many(&wanted))
                .await
                .map_err(|_| RegistryError::Timeout)??;
            let mut bytes: std::collections::HashMap<ContentHash, Bytes> = stored.into_iter().collect();
            let mut work = Vec::with_capacity(todo.len());
            for job in &todo {
                match bytes.remove(&job.hash) {
                    Some(b) => work.push((*job, b)),
                    None => report.missing += 1,
                }
            }
            let parsed = self.parse(work).await?;
            report.add(self.write(parsed).await);
        }
        self.metrics
            .index_blob(IndexResult::AlreadyIndexed, report.already_indexed);
        self.metrics.index_batch_seconds(started.elapsed().as_secs_f64());
        debug!(
            indexed = report.indexed,
            unparseable = report.unparseable,
            already_indexed = report.already_indexed,
            missing = report.missing,
            failed = report.failed,
            rows = report.rows,
            "settings index batch"
        );
        Ok(report)
    }

    /// Parse the blobs of a batch on the registry's pool, inside `spawn_blocking`.
    async fn parse(
        &self,
        work: Vec<(IndexJob, Bytes)>,
    ) -> Result<Vec<(ContentHash, Indexed)>, RegistryError> {
        if work.is_empty() {
            return Ok(Vec::new());
        }
        let (pool, limits, scan) = (Arc::clone(&self.pool), self.limits.clone(), self.scan.clone());
        tokio::task::spawn_blocking(move || {
            pool.install(|| {
                work.par_iter()
                    .map(|(job, bytes)| (job.hash, index_blob(bytes, job.syntax, &limits, &scan)))
                    .collect()
            })
        })
        .await
        .map_err(|_| {
            warn!("the settings index pool failed");
            RegistryError::Corrupt
        })
    }

    /// Write a parsed batch in one transaction; if that fails, each blob in its own, so one blob that cannot be written does not
    /// take the others with it. A blob that still fails has no marker and is counted as failed.
    async fn write(&self, mut parsed: Vec<(ContentHash, Indexed)>) -> IndexReport {
        // Every writer inserts in the same order (by hash), so that two transactions that share blobs wait for each other and never
        // deadlock.
        parsed.sort_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
        match self.write_all(&parsed).await {
            Ok(report) => report,
            Err(_) if parsed.len() > 1 => {
                let mut total = IndexReport::default();
                for one in parsed {
                    if let Ok(report) = self.write_all(std::slice::from_ref(&one)).await {
                        total.add(report);
                    } else {
                        total.failed += 1;
                        self.metrics.index_blob(IndexResult::Failed, 1);
                    }
                }
                total
            }
            Err(_) => {
                self.metrics.index_blob(IndexResult::Failed, parsed.len() as u64);
                IndexReport {
                    failed: parsed.len() as u64,
                    ..IndexReport::default()
                }
            }
        }
    }

    async fn write_all(&self, parsed: &[(ContentHash, Indexed)]) -> Result<IndexReport, RegistryError> {
        let mut tx = self.db.begin().await?;
        // Rows first, then the markers: a marker never exists without its rows.
        let mut batch = RowBatch::default();
        let mut rows_written = 0_u64;
        for (hash, indexed) in parsed {
            if let Indexed::Rows(rows) = indexed {
                for row in rows {
                    batch.push(hash, row);
                    if batch.len() >= self.config.rows_per_statement {
                        let full = std::mem::take(&mut batch);
                        rows_written += self.insert_rows(&mut tx, &full).await?;
                    }
                }
            }
        }
        if !batch.is_empty() {
            rows_written += self.insert_rows(&mut tx, &batch).await?;
        }
        let won = self.insert_markers(&mut tx, parsed).await?;
        self.db.commit(tx).await?;

        let mut report = IndexReport {
            rows: rows_written,
            ..IndexReport::default()
        };
        for (hash, indexed) in parsed {
            if !won.contains(hash) {
                // Another writer (a replica, or the sweeper) marked it between our check and our write: same rows, nothing changed.
                report.already_indexed += 1;
                continue;
            }
            match indexed {
                Indexed::Rows(rows) => {
                    report.indexed += 1;
                    report.redacted += rows.iter().filter(|r| r.secret_rule.is_some()).count() as u64;
                }
                Indexed::Unparseable => report.unparseable += 1,
            }
        }
        self.metrics.index_blob(IndexResult::Indexed, report.indexed);
        self.metrics
            .index_blob(IndexResult::Unparseable, report.unparseable);
        self.metrics.index_rows(report.rows);
        self.metrics.index_redacted(report.redacted);
        Ok(report)
    }

    async fn insert_rows(&self, tx: &mut Txn, b: &RowBatch) -> Result<u64, RegistryError> {
        let done = self
            .db
            .run(
                "index_rows",
                sqlx::query(
                    "INSERT INTO settings_index (blob_hash, setting_path, value_text, value_type, start_line, end_line, secret_rule) \
                     SELECT x.h, x.p, x.v, x.t, x.s, x.e, x.r \
                     FROM unnest($1::bytea[], $2::text[], $3::text[], $4::text[], $5::int[], $6::int[], $7::text[]) \
                          AS x(h, p, v, t, s, e, r) \
                     ON CONFLICT (blob_hash, setting_path) DO NOTHING",
                )
                .bind(&b.hashes)
                .bind(&b.paths)
                .bind(&b.values)
                .bind(&b.types)
                .bind(&b.starts)
                .bind(&b.ends)
                .bind(&b.rules)
                .execute(&mut **tx),
            )
            .await?;
        Ok(done.rows_affected())
    }

    /// The markers of a batch. Returns the blobs whose marker this call wrote.
    async fn insert_markers(
        &self,
        tx: &mut Txn,
        parsed: &[(ContentHash, Indexed)],
    ) -> Result<HashSet<ContentHash>, RegistryError> {
        let hashes: Vec<Vec<u8>> = parsed.iter().map(|(h, _)| h.as_bytes().to_vec()).collect();
        let counts: Vec<i32> = parsed
            .iter()
            .map(|(_, i)| match i {
                Indexed::Rows(rows) => i32::try_from(rows.len()).unwrap_or(i32::MAX),
                Indexed::Unparseable => 0,
            })
            .collect();
        let ok: Vec<bool> = parsed
            .iter()
            .map(|(_, i)| matches!(i, Indexed::Rows(_)))
            .collect();
        let sql = format!(
            "INSERT INTO settings_index_blobs (blob_hash, indexed_at, setting_count, parse_ok) \
             SELECT x.h, {at}, x.n, x.ok FROM unnest($1::bytea[], $2::int[], $3::boolean[]) AS x(h, n, ok) \
             ON CONFLICT (blob_hash) DO NOTHING RETURNING blob_hash::bytea",
            at = ts("$4"),
        );
        let won: Vec<Vec<u8>> = self
            .db
            .run(
                "index_markers_insert",
                sqlx::query_scalar(&sql)
                    .bind(&hashes)
                    .bind(&counts)
                    .bind(&ok)
                    .bind(self.clock.now().unix_millis())
                    .fetch_all(&mut **tx),
            )
            .await?;
        Ok(won.into_iter().filter_map(|b| hash_of(&b)).collect())
    }

    /// The current structured blobs of a swimlane that have no marker, in hash order, after `after`: each with the smallest path that
    /// holds it (which says how to read it). Served by `file_state_current_hash_idx` (swimlane, hash).
    pub(crate) async fn unindexed(
        &self,
        swimlane: &str,
        after: &[u8],
        limit: i64,
    ) -> Result<Vec<(ContentHash, String)>, RegistryError> {
        let rows: Vec<(Vec<u8>, String)> = self
            .db
            .run(
                "index_unindexed",
                sqlx::query_as(UNINDEXED_SQL)
                    .bind(swimlane)
                    .bind(after)
                    .bind(limit)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(h, p)| hash_of(&h).map(|h| (h, p)))
            .collect())
    }

    pub(crate) fn batch_size(&self) -> usize {
        self.config.index_batch
    }

    pub(crate) fn sweep_max_blobs(&self) -> usize {
        self.config.index_sweep_max_blobs
    }
}

/// The markers among a batch of blobs: a lookup by primary key. A constant so that the EXPLAIN snapshot test takes exactly what runs.
pub const MARKED_SQL: &str =
    "SELECT blob_hash::bytea FROM settings_index_blobs WHERE blob_hash = ANY($1::bytea[])";

/// The sweeper's query. A constant so that the EXPLAIN snapshot test takes exactly what runs.
pub const UNINDEXED_SQL: &str = "SELECT f.nfs_hash::bytea, min(f.path) \
     FROM file_state f \
     WHERE f.swimlane_id = $1 AND f.nfs_hash IS NOT NULL AND NOT f.denied AND f.class = 'structured' \
       AND f.nfs_hash > $2::bytea \
       AND NOT EXISTS (SELECT 1 FROM settings_index_blobs b WHERE b.blob_hash = f.nfs_hash) \
     GROUP BY f.nfs_hash ORDER BY f.nfs_hash LIMIT $3";

fn hash_of(bytes: &[u8]) -> Option<ContentHash> {
    <[u8; 32]>::try_from(bytes).ok().map(ContentHash::from_bytes)
}

/// Columns of the rows of one statement.
#[derive(Default)]
struct RowBatch {
    hashes: Vec<Vec<u8>>,
    paths: Vec<String>,
    values: Vec<String>,
    types: Vec<&'static str>,
    starts: Vec<i32>,
    ends: Vec<i32>,
    rules: Vec<Option<&'static str>>,
}

impl RowBatch {
    fn len(&self) -> usize {
        self.paths.len()
    }

    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    fn push(&mut self, hash: &ContentHash, row: &Row) {
        self.hashes.push(hash.as_bytes().to_vec());
        self.paths.push(row.path.clone());
        self.values.push(row.value.clone());
        self.types.push(row.ty);
        self.starts.push(row.start_line);
        self.ends.push(row.end_line);
        self.rules.push(row.secret_rule);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(text: &str, syntax: Syntax) -> Indexed {
        index_blob(
            text.as_bytes(),
            syntax,
            &Limits::default(),
            &ScanConfig::default(),
        )
    }

    fn rows(text: &str, syntax: Syntax) -> Vec<Row> {
        match index(text, syntax) {
            Indexed::Rows(rows) => rows,
            Indexed::Unparseable => panic!("the file does not parse"),
        }
    }

    fn value<'a>(rows: &'a [Row], path: &str) -> &'a Row {
        rows.iter()
            .find(|r| r.path == path)
            .expect("the setting is indexed")
    }

    // A GitHub token: `ghp_` and 36 letters and digits (40 characters).
    const TOKEN: &str = "ghp_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bD1fH3jL5";

    #[test]
    fn a_clean_file_gives_one_row_per_setting_with_its_lines() {
        let r = rows("server:\n  port: 8080\n  name: api\nflag: yes\n", Syntax::Yaml);
        assert_eq!(r.len(), 3);
        let port = value(&r, "server.port");
        assert_eq!((port.value.as_str(), port.ty), ("8080", "int"));
        assert_eq!((port.start_line, port.end_line), (2, 2));
        assert_eq!(port.secret_rule, None);
        assert_eq!(value(&r, "server.name").ty, "string");
        // `yes` stays the text that was written (YAML 1.1 and 1.2 disagree about it).
        assert_eq!(value(&r, "flag").value, "yes");
    }

    #[test]
    fn a_value_the_scan_flags_is_stored_redacted_with_its_rule() {
        let r = rows(
            &format!("api:\n  url: http://x\n  token: {TOKEN}\n"),
            Syntax::Yaml,
        );
        let token = value(&r, "api.token");
        assert_eq!(token.value, "[REDACTED: 40 chars, github_token]");
        assert_eq!(token.secret_rule, Some("github_token"));
        assert_eq!(
            value(&r, "api.url").value,
            "http://x",
            "a neighbour is not touched"
        );
        for row in &r {
            assert!(!row.value.contains(TOKEN), "the secret is nowhere in the rows");
        }
    }

    #[test]
    fn a_private_key_block_value_is_redacted_whole() {
        let text = "key: |\n  -----BEGIN RSA PRIVATE KEY-----\n  MIIEowIBAAKCAQEAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n  -----END RSA PRIVATE KEY-----\nother: 1\n";
        let r = rows(text, Syntax::Yaml);
        let key = value(&r, "key");
        assert_eq!(key.secret_rule, Some("private_key"));
        assert!(
            key.value.starts_with("[REDACTED: ") && key.value.ends_with(", private_key]"),
            "{}",
            key.value
        );
        assert!(!key.value.contains("MIIE"));
        assert_eq!(value(&r, "other").value, "1");
    }

    #[test]
    fn a_secret_on_a_line_that_settings_share_hides_all_of_them() {
        // Settings cannot be told apart below the line, so the line is hidden (the safe side, S13).
        let r = rows(&format!("env: {{A: 1, B: {TOKEN}}}\nlater: 2\n"), Syntax::Yaml);
        assert_eq!(value(&r, "env.A").secret_rule, Some("github_token"));
        assert_eq!(value(&r, "env.B").secret_rule, Some("github_token"));
        assert_eq!(value(&r, "later").secret_rule, None);
    }

    #[test]
    fn properties_and_json_files_are_scanned_too() {
        let p = rows(&format!("a=1\ntoken={TOKEN}\n"), Syntax::Properties);
        assert_eq!(value(&p, "token").secret_rule, Some("github_token"));
        assert_eq!(value(&p, "a").value, "1");
        let j = rows(
            &format!("{{\n  \"a\": 1,\n  \"t\": \"{TOKEN}\"\n}}\n"),
            Syntax::Json,
        );
        assert_eq!(value(&j, "t").secret_rule, Some("github_token"));
        assert_eq!(value(&j, "a").value, "1");
    }

    #[test]
    fn crlf_and_lone_cr_files_keep_the_scan_and_the_rows_on_the_same_lines() {
        let crlf = rows(&format!("a: 1\r\nt: {TOKEN}\r\nb: 2\r\n"), Syntax::Yaml);
        assert_eq!(value(&crlf, "t").secret_rule, Some("github_token"));
        assert_eq!(value(&crlf, "a").secret_rule, None);
        assert_eq!(value(&crlf, "b").secret_rule, None);
        // A lone CR is a line break to the scanner. Whether the parser reads it as one too, no row may hold the token.
        if let Indexed::Rows(cr) = index(&format!("a: 1\rt: {TOKEN}\rb: 2\r"), Syntax::Yaml) {
            for row in &cr {
                assert!(!row.value.contains(TOKEN), "{row:?}");
            }
        }
    }

    #[test]
    fn a_scan_that_cannot_finish_hides_every_value() {
        let scan = ScanConfig {
            max_matches: 1,
            ..ScanConfig::default()
        };
        let text = format!("a: {TOKEN}\nb: {TOKEN}\nc: plain\n");
        let Indexed::Rows(r) = index_blob(text.as_bytes(), Syntax::Yaml, &Limits::default(), &scan) else {
            panic!("the file parses");
        };
        assert_eq!(r.len(), 3);
        for row in &r {
            assert_eq!(row.secret_rule, Some("unscanned"), "{row:?}");
            assert!(
                row.value.starts_with("[REDACTED: "),
                "a value that was not checked is not shown"
            );
        }
        assert_eq!(value(&r, "c").value, "[REDACTED: 5 chars, unscanned]");
    }

    #[test]
    fn a_blob_that_does_not_parse_is_unparseable() {
        assert_eq!(index("a: [unclosed\n", Syntax::Yaml), Indexed::Unparseable);
        assert_eq!(index("{\"a\": ", Syntax::Json), Indexed::Unparseable);
        let huge = vec![b'x'; 3 * 1024 * 1024];
        assert_eq!(
            index_blob(
                &huge,
                Syntax::Properties,
                &Limits::default(),
                &ScanConfig::default()
            ),
            Indexed::Unparseable,
            "over the size limit"
        );
    }

    #[test]
    fn an_empty_file_has_no_rows_and_still_parses() {
        assert_eq!(index("", Syntax::Yaml), Indexed::Rows(Vec::new()));
        assert_eq!(
            index("# only a comment\n", Syntax::Yaml),
            Indexed::Rows(Vec::new())
        );
    }

    #[test]
    fn no_row_holds_a_nul() {
        // Whatever the engine makes of a raw NUL, nothing with one reaches Postgres text.
        let text = "a: \"x\0y\"\nb: 2\n";
        if let Indexed::Rows(r) = index(text, Syntax::Yaml) {
            for row in r {
                assert!(!row.value.contains('\0') && !row.path.contains('\0'), "{row:?}");
            }
        }
        assert_eq!(clean("x\0y"), "x\u{FFFD}y");
        assert_eq!(clean("plain"), "plain");
    }

    #[test]
    fn a_rows_debug_shows_no_value() {
        let r = rows(&format!("t: {TOKEN}\np: visible-value\n"), Syntax::Yaml);
        let text = format!("{r:?}");
        assert!(!text.contains(TOKEN) && !text.contains("visible-value"), "{text}");
        assert!(text.contains("value_bytes"));
    }

    #[test]
    fn type_names_are_the_wire_names_of_the_domain() {
        for ty in [
            ValueType::String,
            ValueType::Int,
            ValueType::Float,
            ValueType::Bool,
            ValueType::Null,
            ValueType::Other,
        ] {
            let wire = serde_json::to_value(ty).expect("a value type serialises");
            assert_eq!(
                wire,
                serde_json::Value::String(type_name(ty).to_owned()),
                "{ty:?}"
            );
        }
    }

    mod property {
        use std::fmt::Write as _;

        use proptest::prelude::*;

        use super::*;

        /// A value of a generated file: a plain word, a number, a flagged token (a GitHub token, an AWS key id, a JWT), or a line of
        /// a private key block as a literal scalar.
        fn value() -> impl Strategy<Value = (String, Option<String>)> {
            prop_oneof![
                "[a-z]{1,12}".prop_map(|w| (w, None)),
                (0u32..100_000).prop_map(|n| (n.to_string(), None)),
                "[A-Za-z0-9]{36}".prop_map(|t| (format!("ghp_{t}"), Some(format!("ghp_{t}")))),
                "[A-Z0-9]{16}".prop_map(|t| (format!("AKIA{t}"), Some(format!("AKIA{t}")))),
            ]
        }

        proptest! {
            /// Whatever a file holds, no stored value contains a token that the scan flags, and every row that held one is
            /// marked with its rule: the index never carries a secret (S13), however the settings are laid out.
            #[test]
            fn no_flagged_value_is_ever_stored(
                values in prop::collection::vec(value(), 1..20),
                flow in any::<bool>(),
            ) {
                let mut text = String::new();
                if flow {
                    let items: Vec<String> = values.iter().enumerate().map(|(i, (v, _))| format!("k{i}: {v}")).collect();
                    let _ = write!(text, "m: {{{}}}\nlater: 1\n", items.join(", "));
                } else {
                    // One group per value: a key that is written twice keeps only its last value, and the first is not indexed at all.
                    for (i, (v, _)) in values.iter().enumerate() {
                        let _ = write!(text, "g{i}:\n  k{i}: {v}\n");
                    }
                }
                let Indexed::Rows(rows) = index(&text, Syntax::Yaml) else {
                    return Err(TestCaseError::fail("a generated file must parse"));
                };
                for (_, planted) in &values {
                    if let Some(token) = planted {
                        for row in &rows {
                            prop_assert!(!row.value.contains(token.as_str()), "{} holds {}", row.path, token);
                            prop_assert!(!row.path.contains(token.as_str()));
                        }
                    }
                }
                let flagged = values.iter().filter(|(_, p)| p.is_some()).count();
                let marked = rows.iter().filter(|r| r.secret_rule.is_some()).count();
                prop_assert!(marked >= flagged, "{marked} rows marked for {flagged} tokens");
                for row in rows.iter().filter(|r| r.secret_rule.is_some()) {
                    prop_assert!(row.value.starts_with("[REDACTED: "), "{}", row.value);
                }
            }
        }
    }

    #[test]
    fn hits_are_found_by_line_range() {
        use engine::SecretRule;
        let hit = |line| SecretMatch {
            line,
            column: 1,
            length: 4,
            rule: SecretRule::Jwt,
        };
        let hits = [hit(3), hit(7), hit(8)];
        assert_eq!(hit_in(&hits, 1, 2), None);
        assert_eq!(hit_in(&hits, 1, 3), Some("jwt"));
        assert_eq!(hit_in(&hits, 4, 6), None);
        assert_eq!(hit_in(&hits, 6, 9), Some("jwt"));
        assert_eq!(hit_in(&hits, 9, 12), None);
        assert_eq!(hit_in(&[], 1, 12), None);
    }
}
