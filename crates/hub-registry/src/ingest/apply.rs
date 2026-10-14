//! Applying a whole scan delta: the transactions that turn what an agent saw into observations, current file state, audit
//! events and outbox events (T1, D47, D80, P1, P15).
//!
//! # Shape
//!
//! A logical delta (all its parts) is applied in **chunks of at most `chunk_paths` paths, one transaction each**, then a last
//! small transaction moves the root. Every step is idempotent, so a crash between two transactions is repaired by the agent
//! sending the same delta again (it is not acknowledged until the last transaction commits):
//!
//! - an observation is keyed by `(swimlane, path, observed_at, hash)`, so inserting it twice is one row (the history of a
//!   replay is the history that was);
//! - what a version *does* is decided against the stored state, never against whether its observation was already there: a
//!   version equal to the stored one is skipped before anything is written, a version older than the stored one is history only,
//!   and a version that is newer moves the state, audits and publishes. A chunk that committed before a crash therefore finds its
//!   paths equal to the delta and does nothing, and a replay of old deltas over newer state changes nothing;
//! - the root, the acknowledgement and the staging rows move last.
//!
//! # One chunk
//!
//! Lock the swimlane, read the stored state, the tool writes and the sync windows of the chunk's paths, decide what becomes of
//! each path ([`super::plan`]), insert the observations, upsert `file_state`, note new folders, write the audit events, write the
//! outbox events, commit.
//! The audit events come last among the writes because `AuditLog::record` holds the audit chain's lock until the transaction ends
//! (A12): everything slow is done by then, so the lock is held for the audit inserts, the events and the commit.
//!
//! # What this does not do
//!
//! It does not compute drift. `file_state` gets the NFS columns (hash, drift hash, size, format, observation); the drift columns
//! (`state`, `git_hash`, `baseline_hash`, `severity`) belong to the pipeline (T3), which reacts to `FileObserved`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::time::Instant;

use domain::{
    Actor, AttributionSource, ContentHash, DomainEvent, FileClass, JobRef, NfsPath, ObservationSource,
    SwimlaneId, Timestamp,
};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};

use super::Inner;
use super::plan::{Candidate, PathPlan, PlanCtx, Stored, ToolWrite, Version, Window, plan_path};
use super::rules::{LaneRules, Names};
use super::validate::{StagedGap, StagedPart};
use crate::db::{Txn, ms, ts};
use crate::error::RegistryError;
use crate::metrics::VersionResult;

/// Folder rows and event batches stay small whatever a delta holds.
const MAX_WINDOWS: i64 = 100;
/// The most paths of a swimlane read when a full scan looks for files that are gone.
const MAX_LANE_PATHS: i64 = 1_000_000;

/// The advisory lock that serialises the writes of one swimlane's applies, across replicas. A separate key space from the audit
/// chain's (03a): the swimlane's lock is always taken first, the chain's last.
const LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.apply.' || $1, 0))";

/// What an apply did, in counts. Nothing in it is content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ApplyReport {
    /// New current versions (files).
    pub recorded: u64,
    /// Removals that moved the state.
    pub removed: u64,
    /// Versions kept only in the history.
    pub history_only: u64,
    /// Versions equal to what the hub had.
    pub unchanged: u64,
    /// Versions an earlier delivery had already recorded.
    pub replayed: u64,
    /// Changes nobody explains (each has an audit event).
    pub out_of_band: u64,
    /// Paths whose state moved.
    pub paths: u64,
    pub chunks: u64,
}

impl ApplyReport {
    fn add(&mut self, other: Self) {
        self.recorded += other.recorded;
        self.removed += other.removed;
        self.history_only += other.history_only;
        self.unchanged += other.unchanged;
        self.replayed += other.replayed;
        self.out_of_band += other.out_of_band;
        self.paths += other.paths;
        self.chunks += other.chunks;
    }
}

/// A logical delta, gathered from its parts.
pub(crate) struct Logical {
    pub swimlane: SwimlaneId,
    pub base_root: Option<ContentHash>,
    pub new_root: ContentHash,
    /// The sequence numbers of every part, the last one being the message that completed the delta.
    pub seqs: Vec<u64>,
    pub parts: Vec<StagedPart>,
}

fn hash_of(bytes: Vec<u8>) -> Result<ContentHash, RegistryError> {
    <[u8; 32]>::try_from(bytes)
        .map(ContentHash::from_bytes)
        .map_err(|_| RegistryError::Corrupt)
}

fn opt_hash(bytes: Option<Vec<u8>>) -> Result<Option<ContentHash>, RegistryError> {
    bytes.map(hash_of).transpose()
}

fn job_text(job: &JobRef) -> String {
    format!("{}/{}", job.name(), job.uid())
}

fn json_text<T: serde::Serialize>(v: &T) -> Result<String, RegistryError> {
    serde_json::to_string(v).map_err(|_| RegistryError::Invalid)
}

/// A row of `file_state`, as `stored` reads it.
type StoredRow = (String, Option<Vec<u8>>, String, bool, i64);
/// A row of `file_observations` of source `tool_write`, as `tool_writes` reads it.
type ToolWriteRow = (String, Option<Vec<u8>>, i64, Option<serde_json::Value>);
/// The key of an observation: its path, its time and its hash (`None` for a removal).
type ObservationKey = (String, i64, Option<ContentHash>);

fn key_of(path: &NfsPath, c: &Candidate) -> ObservationKey {
    (path.as_str().to_owned(), c.observed_at_ms, c.hash)
}

/// What a chunk read and decided, before anything is written.
struct Decided<'p> {
    stored: HashMap<NfsPath, Stored>,
    plans: Vec<(&'p NfsPath, PathPlan)>,
}

/// A path whose state moves in a chunk: its first and its last version there, and the observation of the last.
struct Move<'a> {
    first: &'a Candidate,
    last: &'a Candidate,
    observation_id: i64,
    /// Someone has to audit the change: not the first inventory, and not a write the tool made (it audits its own).
    audited: bool,
}

/// The paths whose state moves, with the counts of the report.
fn moves_of<'a>(
    plans: &'a [(&NfsPath, PathPlan)],
    ids: &HashMap<ObservationKey, i64>,
    report: &mut ApplyReport,
) -> Result<Vec<Move<'a>>, RegistryError> {
    let mut moves = Vec::new();
    for (path, plan) in plans {
        let mut mv: Option<Move<'a>> = None;
        for c in &plan.candidates {
            if !c.moves_state {
                report.history_only += 1;
                continue;
            }
            let id = *ids.get(&key_of(path, c)).ok_or(RegistryError::Corrupt)?;
            if c.hash.is_some() {
                report.recorded += 1;
            } else {
                report.removed += 1;
            }
            if c.out_of_band {
                report.out_of_band += 1;
            }
            let audited = !c.initial() && c.attribution.source != AttributionSource::ToolWrite;
            mv = Some(match mv {
                None => Move {
                    first: c,
                    last: c,
                    observation_id: id,
                    audited,
                },
                Some(m) => Move {
                    first: m.first,
                    last: c,
                    observation_id: id,
                    audited: m.audited || audited,
                },
            });
        }
        moves.extend(mv);
    }
    report.paths = moves.len() as u64;
    Ok(moves)
}

impl Inner {
    /// Apply a logical delta. The caller holds the swimlane's in-process lock.
    pub(crate) async fn apply_logical(
        &self,
        l: &Logical,
        progress: &mut super::delta::Progress,
    ) -> Result<ApplyReport, RegistryError> {
        let started = Instant::now();
        let swimlane = &l.swimlane;
        let mut report = ApplyReport::default();

        // The first inventory of a swimlane is not a change (A11).
        let first_inventory: Option<Option<i64>> = self
            .db
            .run(
                "registry_row",
                sqlx::query_scalar(&format!(
                    "SELECT {} FROM swimlane_registry WHERE swimlane_id = $1",
                    ms("first_inventory_at")
                ))
                .bind(swimlane.as_str())
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let first_inventory = first_inventory.ok_or(RegistryError::NotFound)?;
        let initial_inventory = first_inventory.is_none();

        // Gather the versions by path.
        let mut work: BTreeMap<NfsPath, Vec<Version>> = BTreeMap::new();
        let mut seen: HashSet<NfsPath> = HashSet::new();
        let mut delta_time = 0_i64;
        let mut gap: Option<StagedGap> = None;
        for part in &l.parts {
            for e in &part.entries {
                delta_time = delta_time.max(e.observed_at_ms);
                seen.insert(e.path.clone());
                work.entry(e.path.clone()).or_default().push(Version::File {
                    entry: Box::new(e.clone()),
                    during_job: part.during_job.clone(),
                });
            }
            for p in &part.removed {
                seen.insert(p.clone());
                work.entry(p.clone()).or_default().push(Version::Removal);
            }
            seen.extend(part.skipped.iter().cloned());
            gap = gap.or(part.gap);
        }

        let rules = {
            let mut tx = self.db.begin().await?;
            let rules = super::rules::load(&self.db, &mut tx, swimlane).await?;
            drop(tx);
            rules
        };

        let paths: Vec<NfsPath> = work.keys().cloned().collect();
        for batch in paths.chunks(self.config.chunk_paths) {
            let items: Vec<(&NfsPath, &[Version])> = batch
                .iter()
                .filter_map(|p| work.get(p).map(|v| (p, v.as_slice())))
                .collect();
            let r = self
                .apply_chunk(swimlane, &rules, &items, initial_inventory, delta_time)
                .await?;
            progress.chunks += r.chunks;
            report.add(r);
        }

        // A full scan names every file there is: a file the hub has that the scan does not mention is gone (D63).
        if l.base_root.is_none() && !initial_inventory {
            let gone = self.vanished(swimlane, &seen).await?;
            for batch in gone.chunks(self.config.chunk_paths) {
                let removal = [Version::Removal];
                let items: Vec<(&NfsPath, &[Version])> =
                    batch.iter().map(|p| (p, removal.as_slice())).collect();
                let r = self
                    .apply_chunk(swimlane, &rules, &items, initial_inventory, delta_time)
                    .await?;
                progress.chunks += r.chunks;
                report.add(r);
            }
        }

        self.finish(l, gap.is_some()).await?;

        self.metrics.version(VersionResult::Recorded, report.recorded);
        self.metrics.version(VersionResult::Removed, report.removed);
        self.metrics
            .version(VersionResult::HistoryOnly, report.history_only);
        self.metrics.version(VersionResult::Unchanged, report.unchanged);
        self.metrics.version(VersionResult::Replayed, report.replayed);
        self.metrics.out_of_band(report.out_of_band);
        self.metrics.applied(started.elapsed().as_secs_f64());
        Ok(report)
    }

    /// The paths the hub holds a file for that a full scan did not mention.
    async fn vanished(
        &self,
        swimlane: &SwimlaneId,
        seen: &HashSet<NfsPath>,
    ) -> Result<Vec<NfsPath>, RegistryError> {
        let held: Vec<String> = self
            .db
            .run(
                "vanished",
                sqlx::query_scalar(
                    "SELECT path FROM file_state WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL ORDER BY path LIMIT $2",
                )
                .bind(swimlane.as_str())
                .bind(MAX_LANE_PATHS)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let mut gone = Vec::new();
        for path in held {
            let path = NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?;
            if !seen.contains(&path) {
                gone.push(path);
            }
        }
        Ok(gone)
    }

    async fn windows(&self, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<Vec<Window>, RegistryError> {
        let sql = format!(
            "SELECT job_name, job_uid, {}, {} FROM sync_windows WHERE swimlane_id = $1 \
             ORDER BY opened_at DESC LIMIT $2",
            ms("opened_at"),
            ms("closed_at")
        );
        let rows: Vec<(String, String, i64, Option<i64>)> = self
            .db
            .run(
                "windows",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(MAX_WINDOWS)
                    .fetch_all(&mut **tx),
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(name, uid, opened, closed)| {
                JobRef::new(&name, &uid).ok().map(|job| Window {
                    job,
                    opened_ms: opened,
                    closed_ms: closed,
                })
            })
            .collect())
    }

    async fn apply_chunk(
        &self,
        swimlane: &SwimlaneId,
        rules: &LaneRules,
        items: &[(&NfsPath, &[Version])],
        initial_inventory: bool,
        delta_time_ms: i64,
    ) -> Result<ApplyReport, RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "apply_lock",
                sqlx::query(LOCK_SQL).bind(swimlane.as_str()).execute(&mut *tx),
            )
            .await?;

        let Decided { stored, plans } = self
            .decide(&mut tx, swimlane, items, initial_inventory, delta_time_ms)
            .await?;
        let mut report = ApplyReport {
            chunks: 1,
            unchanged: plans.iter().map(|(_, plan)| plan.unchanged as u64).sum(),
            ..ApplyReport::default()
        };
        let ids = self
            .record(&mut tx, swimlane, &plans, &stored, &mut report)
            .await?;
        let moves = moves_of(&plans, &ids, &mut report)?;
        if !moves.is_empty() {
            self.move_state(&mut tx, swimlane, rules, &moves, &stored, now_ms)
                .await?;
        }
        // The audit chain's lock is taken here, last, and held until the commit.
        self.audit_moves(&mut tx, swimlane, &moves, now_ms).await?;
        self.publish_moves(&mut tx, swimlane, &moves).await?;
        self.db.commit(tx).await?;
        self.offer_to_index(&moves);
        Ok(report)
    }

    /// Tell the settings index about the structured blobs that just became the current version of a file (T2). A non-blocking offer
    /// after the commit, so the acknowledgement never waits for a parse; a blob the queue cannot take is found by the sweeper.
    fn offer_to_index(&self, moves: &[Move<'_>]) {
        let Some(indexer) = self.indexer.get() else {
            return;
        };
        for m in moves {
            let Some(entry) = &m.last.entry else { continue };
            if entry.class != FileClass::Structured || entry.denied || !entry.has_content {
                continue;
            }
            if let Some(syntax) = engine::Syntax::of_path(&entry.path) {
                indexer.offer(crate::index::IndexJob {
                    hash: entry.hash,
                    syntax,
                });
            }
        }
    }

    /// Read what the chunk's decisions depend on (the stored state of its paths, the tool writes, the sync windows) and decide
    /// what becomes of the versions of each path.
    async fn decide<'p>(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        items: &[(&'p NfsPath, &[Version])],
        initial_inventory: bool,
        delta_time_ms: i64,
    ) -> Result<Decided<'p>, RegistryError> {
        let path_texts: Vec<String> = items.iter().map(|(p, _)| p.as_str().to_owned()).collect();
        let stored = self.stored(tx, swimlane, &path_texts).await?;
        let tool_writes = self.tool_writes(tx, swimlane, &path_texts).await?;
        let windows = self.windows(tx, swimlane).await?;
        let no_writes = HashMap::new();
        let plans = items
            .iter()
            .map(|(path, versions)| {
                let ctx = PlanCtx {
                    initial_inventory,
                    delta_time_ms,
                    windows: &windows,
                    tool_writes: tool_writes.get(*path).unwrap_or(&no_writes),
                };
                (*path, plan_path(path, stored.get(*path), versions, &ctx))
            })
            .collect();
        Ok(Decided { stored, plans })
    }

    /// Write the observations of the plans (the history keeps every version, D47). Returns the id of each observation that a
    /// version moving the state names, whether this call wrote it or an earlier delivery did.
    async fn record(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        plans: &[(&NfsPath, PathPlan)],
        stored: &HashMap<NfsPath, Stored>,
        report: &mut ApplyReport,
    ) -> Result<HashMap<ObservationKey, i64>, RegistryError> {
        // De-duplicated by their key, so that one INSERT never names a row twice.
        let mut rows: Vec<&Candidate> = Vec::new();
        let mut keys: HashSet<ObservationKey> = HashSet::new();
        for (path, plan) in plans {
            for c in &plan.candidates {
                if keys.insert(key_of(path, c)) {
                    rows.push(c);
                }
            }
        }
        let mut ids = self.insert_observations(tx, swimlane, &rows, stored).await?;
        // An observation an earlier delivery recorded is already in the history. That does not make the version old news: the state
        // moves by what the plan says about the *stored* state (a removal may have taken the file away since), so a replay that has
        // nothing to change still changes nothing, and one that has something to restore restores it.
        report.replayed += (rows.len() - ids.len()) as u64;
        let moving: Vec<&Candidate> = rows
            .iter()
            .copied()
            .filter(|c| c.moves_state && !ids.contains_key(&key_of(&c.path, c)))
            .collect();
        if !moving.is_empty() {
            ids.extend(self.existing_observations(tx, swimlane, &moving).await?);
        }
        Ok(ids)
    }

    /// Move the NFS columns of `file_state`, note new folders, and count the change for the readers (`version`).
    async fn move_state(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        rules: &LaneRules,
        moves: &[Move<'_>],
        stored: &HashMap<NfsPath, Stored>,
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let finals: Vec<(&NfsPath, &Candidate, i64)> = moves
            .iter()
            .map(|m| (&m.last.path, m.last, m.observation_id))
            .collect();
        self.upsert_state(tx, swimlane, rules, &finals, stored, now_ms)
            .await?;
        let firsts: Vec<&Candidate> = moves
            .iter()
            .filter(|m| m.first.first_sight())
            .map(|m| m.first)
            .collect();
        self.note_folders(tx, swimlane, &firsts).await?;
        self.db
            .run(
                "registry_version",
                sqlx::query(&format!(
                    "UPDATE swimlane_registry SET version = version + 1, updated_at = {} WHERE swimlane_id = $1",
                    ts("$2")
                ))
                .bind(swimlane.as_str())
                .bind(now_ms)
                .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }

    /// One audit event for each path whose change nobody else audits (A12).
    async fn audit_moves(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        moves: &[Move<'_>],
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        for m in moves.iter().filter(|m| m.audited) {
            let last = m.last;
            let via = if last.attribution.source == AttributionSource::SyncJob {
                AuditVia::Sync
            } else {
                AuditVia::AgentDetected
            };
            let mut event = AuditEvent::new(
                Timestamp::from_unix_millis(now_ms),
                AuditActor::Agent {
                    swimlane: swimlane.clone(),
                },
                AuditAction::FileChangeDetected,
                via,
            );
            event.swimlane = Some(swimlane.clone());
            event.path = Some(last.path.clone());
            event.hash_before = m.first.before;
            event.hash_after = last.hash;
            event.attribution = Some(last.attribution.clone());
            let mut handle = Tx::from_pg(tx);
            self.audit.record(&mut handle, event).await?;
        }
        Ok(())
    }

    /// One `FileObserved` for each path whose state moved, in the transaction of the change (D80).
    async fn publish_moves(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        moves: &[Move<'_>],
    ) -> Result<(), RegistryError> {
        for m in moves {
            let last = m.last;
            let event = DomainEvent::FileObserved {
                swimlane: swimlane.clone(),
                path: last.path.clone(),
                // A removal has no hash: the event names the version that went away.
                hash: last.hash.or(m.first.before).ok_or(RegistryError::Corrupt)?,
                source: if last.source == ObservationSource::Sync {
                    ObservationSource::Sync
                } else {
                    ObservationSource::Scan
                },
            };
            let mut handle = Tx::from_pg(tx);
            self.bus.publish_in_tx(&mut handle, event).await?;
        }
        Ok(())
    }

    /// The stored state of these paths.
    async fn stored(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[String],
    ) -> Result<HashMap<NfsPath, Stored>, RegistryError> {
        let sql = format!(
            "SELECT path, nfs_hash::bytea, class, denied, COALESCE({}, 0) \
             FROM file_state WHERE swimlane_id = $1 AND path = ANY($2::text[])",
            ms("observed_at")
        );
        let rows: Vec<StoredRow> = self
            .db
            .run(
                "stored_state",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(paths)
                    .fetch_all(&mut **tx),
            )
            .await?;
        let mut out = HashMap::with_capacity(rows.len());
        for (path, hash, class, denied, observed_at_ms) in rows {
            out.insert(
                NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?,
                Stored {
                    hash: opt_hash(hash)?,
                    observed_at_ms,
                    class: FileClass::from_str(&class).map_err(|_| RegistryError::Corrupt)?,
                    denied,
                },
            );
        }
        Ok(out)
    }

    /// The tool writes of these paths (A10), the newest per hash.
    async fn tool_writes(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[String],
    ) -> Result<HashMap<NfsPath, HashMap<Option<ContentHash>, ToolWrite>>, RegistryError> {
        let sql = format!(
            "SELECT DISTINCT ON (path, hash) path, hash::bytea, {}, attribution_actor \
             FROM file_observations \
             WHERE swimlane_id = $1 AND source = 'tool_write' AND path = ANY($2::text[]) \
             ORDER BY path, hash, observed_at DESC",
            ms("observed_at")
        );
        let rows: Vec<ToolWriteRow> = self
            .db
            .run(
                "tool_writes",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(paths)
                    .fetch_all(&mut **tx),
            )
            .await?;
        let mut out: HashMap<NfsPath, HashMap<Option<ContentHash>, ToolWrite>> = HashMap::new();
        for (path, hash, observed_at_ms, actor) in rows {
            let path = NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?;
            let actor: Option<Actor> = actor.and_then(|v| serde_json::from_value(v).ok());
            out.entry(path).or_default().insert(
                opt_hash(hash)?,
                ToolWrite {
                    observed_at_ms,
                    actor,
                },
            );
        }
        Ok(out)
    }

    /// Insert the observations; returns the ids of those that were new, by their key.
    async fn insert_observations(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        rows: &[&Candidate],
        stored: &HashMap<NfsPath, Stored>,
    ) -> Result<HashMap<ObservationKey, i64>, RegistryError> {
        if rows.is_empty() {
            return Ok(HashMap::new());
        }
        let mut paths = Vec::new();
        let mut hashes: Vec<Option<Vec<u8>>> = Vec::new();
        let mut sizes: Vec<Option<i64>> = Vec::new();
        let mut sources = Vec::new();
        let mut classes: Vec<Option<String>> = Vec::new();
        let mut eols: Vec<Option<String>> = Vec::new();
        let mut encodings: Vec<Option<String>> = Vec::new();
        let mut boms: Vec<Option<bool>> = Vec::new();
        let mut mtimes: Vec<Option<i64>> = Vec::new();
        let mut observed = Vec::new();
        let mut jobs: Vec<Option<String>> = Vec::new();
        let mut a_src = Vec::new();
        let mut a_conf = Vec::new();
        let mut a_actor: Vec<Option<String>> = Vec::new();
        let mut a_evid = Vec::new();
        let mut initial = Vec::new();
        for c in rows {
            paths.push(c.path.as_str().to_owned());
            hashes.push(c.hash.map(|h| h.as_bytes().to_vec()));
            sizes.push(c.entry.as_ref().and_then(|e| i64::try_from(e.size).ok()));
            sources.push(c.source.as_str().to_owned());
            classes.push(match &c.entry {
                Some(e) => Some(e.class.as_str().to_owned()),
                None => stored.get(&c.path).map(|s| s.class.as_str().to_owned()),
            });
            eols.push(
                c.entry
                    .as_ref()
                    .and_then(|e| e.eol)
                    .map(|v| v.as_str().to_owned()),
            );
            encodings.push(
                c.entry
                    .as_ref()
                    .and_then(|e| e.encoding)
                    .map(|v| v.as_str().to_owned()),
            );
            boms.push(c.entry.as_ref().and_then(|e| e.bom));
            mtimes.push(c.entry.as_ref().map(|e| e.mtime_ms));
            observed.push(c.observed_at_ms);
            jobs.push(c.during_job.as_ref().map(job_text));
            a_src.push(c.attribution.source.as_str().to_owned());
            a_conf.push(c.attribution.confidence.as_str().to_owned());
            a_actor.push(c.attribution.actor.as_ref().map(json_text).transpose()?);
            a_evid.push(json_text(&c.attribution.evidence)?);
            initial.push(c.initial());
        }
        let sql = format!(
            "INSERT INTO file_observations \
               (swimlane_id, path, hash, size, source, class, eol_style, encoding, bom, mtime, observed_at, during_job, \
                attribution_source, attribution_confidence, attribution_actor, attribution_evidence, initial) \
             SELECT $1, x.path, x.hash, x.size, x.source, x.class, x.eol, x.enc, x.bom, \
                    CASE WHEN x.mtime IS NULL THEN NULL ELSE {mtime} END, {observed}, x.job, \
                    x.a_src, x.a_conf, x.a_actor::jsonb, x.a_evid::jsonb, x.initial \
             FROM unnest($2::text[], $3::bytea[], $4::bigint[], $5::text[], $6::text[], $7::text[], $8::text[], \
                         $9::boolean[], $10::bigint[], $11::bigint[], $12::text[], $13::text[], $14::text[], \
                         $15::text[], $16::text[], $17::boolean[]) \
                  AS x(path, hash, size, source, class, eol, enc, bom, mtime, observed, job, a_src, a_conf, a_actor, \
                       a_evid, initial) \
             ON CONFLICT DO NOTHING \
             RETURNING id, path, hash::bytea, {observed_back}",
            mtime = ts("x.mtime"),
            observed = ts("x.observed"),
            observed_back = ms("observed_at"),
        );
        let returned: Vec<(i64, String, Option<Vec<u8>>, i64)> = self
            .db
            .run(
                "insert_observations",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(&hashes)
                    .bind(&sizes)
                    .bind(&sources)
                    .bind(&classes)
                    .bind(&eols)
                    .bind(&encodings)
                    .bind(&boms)
                    .bind(&mtimes)
                    .bind(&observed)
                    .bind(&jobs)
                    .bind(&a_src)
                    .bind(&a_conf)
                    .bind(&a_actor)
                    .bind(&a_evid)
                    .bind(&initial)
                    .fetch_all(&mut **tx),
            )
            .await?;
        let mut out = HashMap::with_capacity(returned.len());
        for (id, path, hash, at) in returned {
            out.insert((path, at, opt_hash(hash)?), id);
        }
        Ok(out)
    }

    /// The ids of observations that are already recorded.
    async fn existing_observations(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        rows: &[&Candidate],
    ) -> Result<HashMap<ObservationKey, i64>, RegistryError> {
        let paths: Vec<String> = rows.iter().map(|c| c.path.as_str().to_owned()).collect();
        let times: Vec<i64> = rows.iter().map(|c| c.observed_at_ms).collect();
        let hashes: Vec<Option<Vec<u8>>> = rows
            .iter()
            .map(|c| c.hash.map(|h| h.as_bytes().to_vec()))
            .collect();
        let sql = format!(
            "SELECT o.id, o.path, o.hash::bytea, {back} \
             FROM file_observations o \
             JOIN unnest($2::text[], $3::bigint[], $4::bytea[]) AS x(path, at, hash) \
               ON o.path = x.path AND o.observed_at = {at} AND o.hash IS NOT DISTINCT FROM x.hash \
             WHERE o.swimlane_id = $1",
            back = ms("o.observed_at"),
            at = ts("x.at"),
        );
        let found: Vec<(i64, String, Option<Vec<u8>>, i64)> = self
            .db
            .run(
                "existing_observations",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(&times)
                    .bind(&hashes)
                    .fetch_all(&mut **tx),
            )
            .await?;
        let mut out = HashMap::with_capacity(found.len());
        for (id, path, hash, at) in found {
            out.insert((path, at, opt_hash(hash)?), id);
        }
        Ok(out)
    }

    /// Set the NFS columns of `file_state` for the paths whose state moved. The drift columns are not touched.
    async fn upsert_state(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        rules: &LaneRules,
        finals: &[(&NfsPath, &Candidate, i64)],
        stored: &HashMap<NfsPath, Stored>,
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let mut paths = Vec::new();
        let mut logical = Vec::new();
        let mut kinds = Vec::new();
        let mut classes = Vec::new();
        let mut role_names: Vec<Option<String>> = Vec::new();
        let mut hashes: Vec<Option<Vec<u8>>> = Vec::new();
        let mut dhashes: Vec<Option<Vec<u8>>> = Vec::new();
        let mut sizes: Vec<Option<i64>> = Vec::new();
        let mut eols: Vec<Option<String>> = Vec::new();
        let mut encodings: Vec<Option<String>> = Vec::new();
        let mut boms: Vec<Option<bool>> = Vec::new();
        let mut denied = Vec::new();
        let mut observed = Vec::new();
        let mut ids = Vec::new();
        for (path, c, id) in finals {
            let Names {
                logical: l,
                kind,
                role,
            } = rules.names(path)?;
            paths.push(path.as_str().to_owned());
            logical.push(l.as_str().to_owned());
            kinds.push(kind.as_str().to_owned());
            role_names.push(Some(role.as_str().to_owned()));
            if let Some(e) = &c.entry {
                classes.push(e.class.as_str().to_owned());
                hashes.push(Some(e.hash.as_bytes().to_vec()));
                dhashes.push(Some(e.drift_hash.as_bytes().to_vec()));
                sizes.push(i64::try_from(e.size).ok());
                eols.push(e.eol.map(|v| v.as_str().to_owned()));
                encodings.push(e.encoding.map(|v| v.as_str().to_owned()));
                boms.push(e.bom);
                denied.push(e.denied);
            } else {
                // A removal keeps the class and the denied flag of what it removes.
                let s = stored.get(*path);
                classes.push(s.map_or(FileClass::Binary, |s| s.class).as_str().to_owned());
                hashes.push(None);
                dhashes.push(None);
                sizes.push(None);
                eols.push(None);
                encodings.push(None);
                boms.push(None);
                denied.push(s.is_some_and(|s| s.denied));
            }
            observed.push(c.observed_at_ms);
            ids.push(*id);
        }
        let sql = format!(
            "INSERT INTO file_state \
               (swimlane_id, path, logical_file, kind, class, role, nfs_hash, nfs_drift_hash, state, size, eol_style, \
                encoding, bom, denied, observed_at, last_observation_id, updated_at) \
             SELECT $1, x.path, x.logical, x.kind, x.class, x.role, x.hash, x.dhash, 'unknown', x.size, x.eol, x.enc, \
                    x.bom, x.denied, {observed}, x.obs_id, {now} \
             FROM unnest($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::bytea[], $8::bytea[], \
                         $9::bigint[], $10::text[], $11::text[], $12::boolean[], $13::boolean[], $14::bigint[], \
                         $15::bigint[]) \
                  AS x(path, logical, kind, class, role, hash, dhash, size, eol, enc, bom, denied, observed, obs_id) \
             ON CONFLICT (swimlane_id, path) DO UPDATE SET \
               class = EXCLUDED.class, nfs_hash = EXCLUDED.nfs_hash, nfs_drift_hash = EXCLUDED.nfs_drift_hash, \
               size = EXCLUDED.size, eol_style = EXCLUDED.eol_style, encoding = EXCLUDED.encoding, bom = EXCLUDED.bom, \
               denied = EXCLUDED.denied, observed_at = EXCLUDED.observed_at, \
               last_observation_id = EXCLUDED.last_observation_id, updated_at = EXCLUDED.updated_at",
            observed = ts("x.observed"),
            now = ts("$16"),
        );
        self.db
            .run(
                "upsert_state",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(&logical)
                    .bind(&kinds)
                    .bind(&classes)
                    .bind(&role_names)
                    .bind(&hashes)
                    .bind(&dhashes)
                    .bind(&sizes)
                    .bind(&eols)
                    .bind(&encodings)
                    .bind(&boms)
                    .bind(&denied)
                    .bind(&observed)
                    .bind(&ids)
                    .bind(now_ms)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }

    /// Record the folders of paths seen for the first time (C11: a folder created after the config-server started is not a
    /// search location until it restarts). A folder of the first inventory is marked `initial`.
    async fn note_folders(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        firsts: &[&Candidate],
    ) -> Result<(), RegistryError> {
        let mut dirs: BTreeMap<String, (i64, bool)> = BTreeMap::new();
        for c in firsts {
            let mut dir = c.path.parent();
            while !dir.is_root() {
                let entry = dirs
                    .entry(dir.as_str().to_owned())
                    .or_insert((c.observed_at_ms, c.initial()));
                entry.0 = entry.0.min(c.observed_at_ms);
                entry.1 = entry.1 && c.initial();
                dir = dir.parent();
            }
        }
        if dirs.is_empty() {
            return Ok(());
        }
        let (names, times, initial): (Vec<_>, Vec<_>, Vec<_>) = dirs
            .into_iter()
            .map(|(d, (t, i))| (d, t, i))
            .fold((Vec::new(), Vec::new(), Vec::new()), |mut acc, (d, t, i)| {
                acc.0.push(d);
                acc.1.push(t);
                acc.2.push(i);
                acc
            });
        let sql = format!(
            "INSERT INTO swimlane_folders (swimlane_id, dir_path, first_seen_at, initial) \
             SELECT $1, x.dir, {first_seen}, x.initial \
             FROM unnest($2::text[], $3::bigint[], $4::boolean[]) AS x(dir, seen, initial) \
             ON CONFLICT DO NOTHING",
            first_seen = ts("x.seen"),
        );
        self.db
            .run(
                "note_folders",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(&names)
                    .bind(&times)
                    .bind(&initial)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }

    /// The last transaction of a logical delta: the root, the agent's record, the acknowledgement, the pipeline's flags, and the
    /// staged parts gone. Only after this does the delta count as applied.
    async fn finish(&self, l: &Logical, gap: bool) -> Result<(), RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let swimlane = &l.swimlane;
        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "finish_lock",
                sqlx::query(LOCK_SQL).bind(swimlane.as_str()).execute(&mut *tx),
            )
            .await?;
        let known: Option<Vec<u8>> = self
            .db
            .run(
                "known_root",
                sqlx::query_scalar(
                    "SELECT hash::bytea FROM swimlane_merkle WHERE swimlane_id = $1 AND dir_path = ''",
                )
                .bind(swimlane.as_str())
                .fetch_optional(&mut *tx),
            )
            .await?;
        let known = opt_hash(known)?;
        let full = l.base_root.is_none();
        // Only a full scan is the inventory: an increment on top of nothing (a spool replayed into a new database) leaves the
        // inventory to the full scan that follows, so that its files are not taken for changes (A11).
        // A delta that does not start where the hub's root is, or that follows a lost stretch of the spool, may have left changes
        // out: only a full scan can say (A13). A full scan is the repair.
        let broken = gap || (l.base_root.is_some() && l.base_root != known);

        self.db
            .run(
                "merkle_root",
                sqlx::query(&format!(
                    "INSERT INTO swimlane_merkle (swimlane_id, dir_path, hash, entry_count, updated_at) \
                     VALUES ($1, '', $2::bytea, \
                             (SELECT count(*)::int FROM file_state WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL), {now}) \
                     ON CONFLICT (swimlane_id, dir_path) DO UPDATE SET \
                       hash = EXCLUDED.hash, entry_count = EXCLUDED.entry_count, updated_at = EXCLUDED.updated_at",
                    now = ts("$3")
                ))
                .bind(swimlane.as_str())
                .bind(l.new_root.as_bytes().as_slice())
                .bind(now_ms)
                .execute(&mut *tx),
            )
            .await?;
        self.db
            .run(
                "agent_root",
                sqlx::query("UPDATE agents SET merkle_root = $2::bytea WHERE swimlane_id = $1")
                    .bind(swimlane.as_str())
                    .bind(l.new_root.as_bytes().as_slice())
                    .execute(&mut *tx),
            )
            .await?;
        let last_seq = l.seqs.iter().copied().max().unwrap_or(0);
        self.db
            .run(
                "agent_ack",
                sqlx::query(&format!(
                    "INSERT INTO agent_acks (swimlane_id, acked_seq, updated_at) VALUES ($1, $2, {now}) \
                     ON CONFLICT (swimlane_id) DO UPDATE SET \
                       acked_seq = GREATEST(agent_acks.acked_seq, EXCLUDED.acked_seq), updated_at = EXCLUDED.updated_at",
                    now = ts("$3")
                ))
                .bind(swimlane.as_str())
                .bind(i64::try_from(last_seq).map_err(|_| RegistryError::Invalid)?)
                .bind(now_ms)
                .execute(&mut *tx),
            )
            .await?;
        self.db
            .run(
                "registry_flags",
                sqlx::query(&format!(
                    "UPDATE swimlane_registry SET \
                       first_inventory_at = CASE WHEN $2 THEN COALESCE(first_inventory_at, {now}) ELSE first_inventory_at END, \
                       needs_full_scan = CASE WHEN $2 THEN false ELSE needs_full_scan OR $3 END, \
                       updated_at = {now} \
                     WHERE swimlane_id = $1",
                    now = ts("$4")
                ))
                .bind(swimlane.as_str())
                .bind(full)
                .bind(broken)
                .bind(now_ms)
                .execute(&mut *tx),
            )
            .await?;
        let seqs: Vec<i64> = l.seqs.iter().filter_map(|s| i64::try_from(*s).ok()).collect();
        self.db
            .run(
                "staging_done",
                sqlx::query("DELETE FROM delta_staging WHERE swimlane_id = $1 AND seq = ANY($2::bigint[])")
                    .bind(swimlane.as_str())
                    .bind(&seqs)
                    .execute(&mut *tx),
            )
            .await?;
        self.db.commit(tx).await
    }

    /// Mark the swimlane so that the next heartbeat asks for a full scan. Best effort: used when an apply failed after some of
    /// its chunks committed (A13), where the database may well be what failed.
    pub(crate) async fn flag_full_scan(&self, swimlane: &SwimlaneId) {
        let _ = self
            .db
            .run(
                "flag_full_scan",
                sqlx::query("UPDATE swimlane_registry SET needs_full_scan = true WHERE swimlane_id = $1")
                    .bind(swimlane.as_str())
                    .execute(self.db.pool()),
            )
            .await;
    }
}
