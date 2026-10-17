//! What a sync replaced on NFS (C7, D21, D73).
//!
//! C7 (`overwritten_nfs_change`) says "a sync replaced a change that was only on NFS, and the version it replaced can be restored". The
//! engine decides it from [`engine::Overwrite`]: the drift state the file was in before the change, the hash for comparing (N) it had,
//! and what attributed the change. Drift is recomputed on every change, so once the sync's version is the file's state the earlier state
//! is gone: it has to be written down *when the sync's version arrives*, on the observation of that version (`previous_state` and
//! `previous_hash` of `file_observations`, which the pipeline's loader reads for the file's last observation).
//!
//! Two places write it:
//!
//! - **Ingestion**, for a version the agent tagged with a Job or that falls in a sync window ([`previous_of`]): the attribution is
//!   known when the version is first seen, and so is the state it replaces (the one the pipeline last computed).
//! - **`record_sync`** ([`super::SyncService`]), for a sync that nobody tagged: a report says a sync of these commits has finished,
//!   and a file that was ahead on NFS before the sync and equals Git at those commits afterwards was overwritten by it. The report
//!   upgrades the observation's attribution to the sync job and writes the context ([`Inner::claim`]). The states it needs are read
//!   before the scan is asked for ([`snapshot`]), because the pipeline recomputes them as soon as the scan is applied.

use std::collections::BTreeMap;
use std::str::FromStr;

use domain::{
    Actor, Attribution, AttributionSource, Confidence, ContentHash, DomainEvent, DriftState, Evidence,
    NfsPath, SwimlaneId, Timestamp,
};
use ports::{AuditAction, AuditEvent, AuditVia, Tx};

use super::record::{NfsRow, Resolved};
use super::{Inner, Report};
use crate::db::{Db, Txn};
use crate::error::RegistryError;
use crate::ingest::apply::LOCK_SQL;
use crate::ingest::plan::Stored;
use crate::ingest::rules::LaneRules;

/// The most rows of one swimlane read: the engine refuses more files than this for a swimlane anyway.
const MAX_ROWS: i64 = 200_001;
/// Paths in one `= ANY` of a read that is not in a transaction of a chunk.
const READ_BATCH: usize = 5_000;

/// What a sync job's version of a file replaced: the drift state the file was in, and the hash for comparing (N) it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Previous {
    pub state: DriftState,
    pub hash: Option<ContentHash>,
}

/// What the version attributed by `attribution`, which follows `before` on the path, replaced, if it is a sync job's.
///
/// Only the version that replaces what the hub stored carries it (`before` is the stored hash): a later version of the same delta
/// replaced a version the pipeline never computed a state for. A path the hub has no content for (new, or removed) had nothing to
/// replace, and neither has one the pipeline has not computed yet (its row is the placeholder ingestion wrote).
pub(crate) fn previous_of(
    stored: Option<&Stored>,
    before: Option<ContentHash>,
    attribution: &Attribution,
) -> Option<Previous> {
    if attribution.source != AttributionSource::SyncJob {
        return None;
    }
    let stored = stored.filter(|s| s.observed)?;
    let hash = stored.hash?;
    if before != Some(hash) {
        return None;
    }
    Some(Previous {
        state: stored.state?,
        hash: stored.drift_hash,
    })
}

/// A path that was ahead on NFS or in conflict when the sync was reported, as it was then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ahead {
    pub path: NfsPath,
    pub state: DriftState,
    /// N then: the hash for comparing of the version a sync would replace.
    pub hash: Option<ContentHash>,
}

fn hash_of(bytes: Vec<u8>) -> Result<ContentHash, RegistryError> {
    <[u8; 32]>::try_from(bytes)
        .map(ContentHash::from_bytes)
        .map_err(|_| RegistryError::Corrupt)
}

/// The files of the swimlane whose last computed state is `nfs_ahead` or `conflict`, with the hash they have now (before the scan).
pub(crate) async fn snapshot(db: &Db, swimlane: &SwimlaneId) -> Result<Vec<Ahead>, RegistryError> {
    let rows: Vec<(String, String, Option<Vec<u8>>)> = db
        .run(
            "sync_snapshot",
            sqlx::query_as(
                "SELECT path, state, nfs_drift_hash::bytea FROM file_state \
                 WHERE swimlane_id = $1 AND state IN ('nfs_ahead', 'conflict') AND nfs_hash IS NOT NULL \
                 ORDER BY path LIMIT $2",
            )
            .bind(swimlane.as_str())
            .bind(MAX_ROWS)
            .fetch_all(db.pool()),
        )
        .await?;
    if rows.len() >= usize::try_from(MAX_ROWS).unwrap_or(usize::MAX) {
        return Err(RegistryError::Capacity);
    }
    rows.into_iter()
        .map(|(path, state, hash)| {
            Ok(Ahead {
                path: NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?,
                state: DriftState::from_str(&state).map_err(|_| RegistryError::Corrupt)?,
                hash: hash.map(hash_of).transpose()?,
            })
        })
        .collect()
}

/// An observation, as `claim` reads it.
struct Observation {
    id: i64,
    path: NfsPath,
    hash: Option<ContentHash>,
    /// The attribution as the decision reads it.
    attribution: Attribution,
    /// The attribution columns as stored, so that a write that does not change the attribution writes back exactly what was there (an
    /// actor of a shape this code does not know stays what it is).
    source: String,
    confidence: String,
    actor: Option<serde_json::Value>,
    has_context: bool,
    observed_at_ms: i64,
}

type ObservationRow = (
    i64,
    String,
    Option<Vec<u8>>,
    String,
    String,
    Option<serde_json::Value>,
    bool,
    i64,
);

fn observation_of(row: ObservationRow) -> Result<Observation, RegistryError> {
    let (id, path, hash, source, confidence, actor, has_context, observed_at_ms) = row;
    Ok(Observation {
        id,
        path: NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?,
        hash: hash.map(hash_of).transpose()?,
        attribution: Attribution {
            source: AttributionSource::from_str(&source).map_err(|_| RegistryError::Corrupt)?,
            confidence: Confidence::from_str(&confidence).map_err(|_| RegistryError::Corrupt)?,
            actor: actor
                .clone()
                .and_then(|v| serde_json::from_value::<Actor>(v).ok()),
            evidence: Evidence::default(),
        },
        source,
        confidence,
        actor,
        has_context,
        observed_at_ms,
    })
}

/// What `claim` decides for one observation.
#[derive(Debug, Clone, PartialEq)]
enum Decision {
    /// Someone better placed than a sync report attributed the change, or the observation has its context already.
    Leave,
    /// The observation is the sync job's already (the agent tagged it, but the pipeline had not computed a state to record): context only.
    ContextOnly,
    /// The observation was attributed below the sync job: the sync job becomes its author, and it gets its context.
    Upgrade(Attribution),
}

/// Whether the observation of a version that equals Git at the reported commits is the sync's, and what to do about it.
fn decide(obs: &Observation, job: Option<&Actor>) -> Decision {
    if obs.has_context {
        return Decision::Leave;
    }
    if obs.attribution.source == AttributionSource::SyncJob {
        return Decision::ContextOnly;
    }
    let sync = Attribution {
        source: AttributionSource::SyncJob,
        confidence: Confidence::High,
        // A user who reported the sync did not write the files: only a Job is the author of a sync's change.
        actor: job.cloned(),
        evidence: Evidence {
            observed_at: Some(Timestamp::from_unix_millis(obs.observed_at_ms)),
            ..Evidence::default()
        },
    };
    // The same gate the sentinel's correlator uses (D73): a strictly higher confidence replaces, an equal one never does.
    if obs.attribution.may_upgrade_to(&sync) {
        Decision::Upgrade(sync)
    } else {
        Decision::Leave
    }
}

impl Inner {
    /// C7 first: the files of `ahead` that the sync replaced get the context, and the sync job as author when nobody better attributed
    /// them. Returns their paths (what was written, in the order of the paths).
    pub(crate) async fn claim(
        &self,
        r: &Report,
        rules: &LaneRules,
        ahead: Vec<Ahead>,
    ) -> Result<Vec<NfsPath>, RegistryError> {
        if ahead.is_empty() {
            return Ok(Vec::new());
        }
        let then: BTreeMap<NfsPath, Ahead> = ahead.into_iter().map(|a| (a.path.clone(), a)).collect();
        // The files as they are after the scan, and which of them the scan changed.
        let names: Vec<String> = then.keys().map(|p| p.as_str().to_owned()).collect();
        let mut changed: Vec<NfsRow> = Vec::new();
        for batch in names.chunks(READ_BATCH) {
            for row in self.rows_of(&r.swimlane, batch).await? {
                let was = then.get(&row.path).and_then(|a| a.hash);
                if was != Some(row.drift) {
                    changed.push(row);
                }
            }
        }
        // A file the sync wrote is one that now equals Git at the reported commits.
        let replaced: Vec<Resolved> = self
            .resolve(r, rules, changed)
            .await?
            .into_iter()
            .filter(|x| x.git == x.row.drift)
            .collect();

        let job = match &r.actor {
            Actor::Job { .. } => Some(r.actor.clone()),
            Actor::User { .. } | Actor::OsLogin { .. } => None,
        };
        let mut claimed = Vec::new();
        for chunk in replaced.chunks(self.config.chunk_paths) {
            claimed.extend(self.claim_chunk(r, &then, chunk, job.as_ref()).await?);
        }
        Ok(claimed)
    }

    /// One transaction: the observations of up to `chunk_paths` files, their attribution events and the context.
    async fn claim_chunk(
        &self,
        r: &Report,
        then: &BTreeMap<NfsPath, Ahead>,
        chunk: &[Resolved],
        job: Option<&Actor>,
    ) -> Result<Vec<NfsPath>, RegistryError> {
        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "sync_claim_lock",
                sqlx::query(LOCK_SQL).bind(r.swimlane.as_str()).execute(&mut *tx),
            )
            .await?;
        // The files as they are now, under the lock that ingestion's applies take: a newer version since the read above is not the sync's.
        let paths: Vec<String> = chunk.iter().map(|x| x.row.path.as_str().to_owned()).collect();
        let current = self.rows_in(&mut tx, &r.swimlane, &paths).await?;
        let mut wanted: BTreeMap<i64, (&NfsRow, &Ahead)> = BTreeMap::new();
        for x in chunk {
            let Some(now) = current.iter().find(|c| c.path == x.row.path) else {
                continue;
            };
            let (Some(id), Some(ahead)) = (now.observation, then.get(&now.path)) else {
                continue;
            };
            if now.drift == x.row.drift && now.nfs == x.row.nfs {
                wanted.insert(id, (now, ahead));
            }
        }
        let ids: Vec<i64> = wanted.keys().copied().collect();
        let observations: Vec<ObservationRow> = self
            .db
            .run(
                "sync_claim_observations",
                sqlx::query_as(
                    "SELECT id, path, hash::bytea, attribution_source, attribution_confidence, attribution_actor, \
                            previous_state IS NOT NULL, (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint \
                     FROM file_observations WHERE swimlane_id = $1 AND id = ANY($2::bigint[])",
                )
                .bind(r.swimlane.as_str())
                .bind(&ids)
                .fetch_all(&mut *tx),
            )
            .await?;

        let mut write = Writes::default();
        for row in observations {
            let obs = observation_of(row)?;
            let Some((now, ahead)) = wanted.get(&obs.id) else {
                continue;
            };
            // The observation must be the one of the version the file has now.
            if obs.hash != Some(now.nfs) {
                continue;
            }
            match decide(&obs, job) {
                Decision::Leave => {}
                Decision::ContextOnly => write.push(&obs, ahead, None)?,
                Decision::Upgrade(sync) => write.push(&obs, ahead, Some((sync, now.nfs)))?,
            }
        }
        let updated = self.write_claims(&mut tx, &r.swimlane, &write).await?;

        // The events, in the order of the paths: the outbox first, the audit events last (the chain's lock is the last thing taken).
        let now_ms = self.clock.now().unix_millis();
        let mut events = Vec::new();
        let mut audit = Vec::new();
        for item in &write.items {
            if !updated.contains(&item.id) {
                continue;
            }
            let Some((attribution, raw)) = &item.upgrade else {
                continue;
            };
            let raw = *raw;
            events.push(DomainEvent::AttributionUpdated {
                swimlane: r.swimlane.clone(),
                path: item.path.clone(),
                hash: raw,
            });
            let mut event = AuditEvent::new(
                Timestamp::from_unix_millis(now_ms),
                r.audit_actor()?,
                AuditAction::AttributionUpgraded,
                AuditVia::Sync,
            );
            event.swimlane = Some(r.swimlane.clone());
            event.path = Some(item.path.clone());
            event.hash_after = Some(raw);
            event.attribution = Some(attribution.clone());
            audit.push(event);
        }
        {
            let mut handle = Tx::from_pg(&mut tx);
            crate::batch::publish_all(&*self.bus, &mut handle, events).await?;
        }
        {
            let mut handle = Tx::from_pg(&mut tx);
            crate::batch::record_all(&*self.audit, &mut handle, audit).await?;
        }
        self.db.commit(tx).await?;
        Ok(write
            .items
            .into_iter()
            .filter(|i| updated.contains(&i.id))
            .map(|i| i.path)
            .collect())
    }

    async fn write_claims(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        write: &Writes,
    ) -> Result<Vec<i64>, RegistryError> {
        if write.items.is_empty() {
            return Ok(Vec::new());
        }
        let updated: Vec<i64> = self
            .db
            .run(
                "sync_claim_write",
                sqlx::query_scalar(
                    "UPDATE file_observations o SET previous_state = x.state, previous_hash = x.hash, \
                            attribution_source = x.a_src, attribution_confidence = x.a_conf, attribution_actor = x.a_actor::jsonb \
                     FROM unnest($2::bigint[], $3::text[], $4::bytea[], $5::text[], $6::text[], $7::text[]) \
                          AS x(id, state, hash, a_src, a_conf, a_actor) \
                     WHERE o.swimlane_id = $1 AND o.id = x.id AND o.previous_state IS NULL \
                     RETURNING o.id",
                )
                .bind(swimlane.as_str())
                .bind(&write.ids)
                .bind(&write.states)
                .bind(&write.hashes)
                .bind(&write.sources)
                .bind(&write.confidences)
                .bind(&write.actors)
                .fetch_all(&mut **tx),
            )
            .await?;
        Ok(updated)
    }
}

/// One observation to write.
struct Item {
    id: i64,
    path: NfsPath,
    /// The new attribution and the raw hash of the version, when the attribution is upgraded (the events name them).
    upgrade: Option<(Attribution, ContentHash)>,
}

/// The arrays of one `UPDATE`, and the items they were made from.
#[derive(Default)]
struct Writes {
    items: Vec<Item>,
    ids: Vec<i64>,
    states: Vec<String>,
    hashes: Vec<Option<Vec<u8>>>,
    sources: Vec<String>,
    confidences: Vec<String>,
    actors: Vec<Option<String>>,
}

impl Writes {
    /// Add an observation: with `upgrade`, its attribution becomes that; without, the attribution columns are written back as read.
    fn push(
        &mut self,
        obs: &Observation,
        ahead: &Ahead,
        upgrade: Option<(Attribution, ContentHash)>,
    ) -> Result<(), RegistryError> {
        self.ids.push(obs.id);
        self.states.push(ahead.state.as_str().to_owned());
        self.hashes.push(ahead.hash.map(|h| h.as_bytes().to_vec()));
        if let Some((a, _)) = &upgrade {
            self.sources.push(a.source.as_str().to_owned());
            self.confidences.push(a.confidence.as_str().to_owned());
            self.actors.push(
                a.actor
                    .as_ref()
                    .map(|actor| serde_json::to_string(actor).map_err(|_| RegistryError::Invalid))
                    .transpose()?,
            );
        } else {
            self.sources.push(obs.source.clone());
            self.confidences.push(obs.confidence.clone());
            self.actors.push(obs.actor.as_ref().map(ToString::to_string));
        }
        self.items.push(Item {
            id: obs.id,
            path: obs.path.clone(),
            upgrade,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use domain::JobRef;

    use super::*;

    fn job() -> Actor {
        Actor::Job {
            job: JobRef::new("sync-sit1", "uid-1").unwrap(),
        }
    }

    fn observation(source: AttributionSource, confidence: Confidence, has_context: bool) -> Observation {
        Observation {
            id: 7,
            path: NfsPath::parse("app/core.yml").unwrap(),
            hash: Some(ContentHash::from_bytes([1; 32])),
            attribution: Attribution {
                source,
                confidence,
                actor: None,
                evidence: Evidence::default(),
            },
            source: source.as_str().to_owned(),
            confidence: confidence.as_str().to_owned(),
            actor: Some(serde_json::json!({"kind": "something_a_later_version_adds"})),
            has_context,
            observed_at_ms: 42,
        }
    }

    fn ahead() -> Ahead {
        Ahead {
            path: NfsPath::parse("app/core.yml").unwrap(),
            state: DriftState::Conflict,
            hash: Some(ContentHash::from_bytes([2; 32])),
        }
    }

    #[test]
    fn an_observation_with_its_context_is_left_alone() {
        for source in [AttributionSource::Unknown, AttributionSource::SyncJob] {
            let o = observation(source, Confidence::Low, true);
            assert_eq!(decide(&o, Some(&job())), Decision::Leave, "{source:?}");
        }
    }

    #[test]
    fn a_change_nobody_attributed_becomes_the_sync_jobs() {
        for (source, confidence) in [
            (AttributionSource::Unknown, Confidence::Low),
            (AttributionSource::NfsClient, Confidence::Medium),
        ] {
            match decide(&observation(source, confidence, false), Some(&job())) {
                Decision::Upgrade(a) => {
                    assert_eq!(a.source, AttributionSource::SyncJob);
                    assert_eq!(a.confidence, Confidence::High);
                    assert_eq!(a.actor, Some(job()), "a Job is the author of a sync's change");
                    assert_eq!(a.evidence.observed_at, Some(Timestamp::from_unix_millis(42)));
                }
                other => panic!("{source:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_user_who_reported_the_sync_is_not_its_author() {
        match decide(
            &observation(AttributionSource::Unknown, Confidence::Low, false),
            None,
        ) {
            Decision::Upgrade(a) => assert_eq!(a.actor, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn evidence_that_is_as_good_or_better_is_never_replaced() {
        // D73: attribution only goes up. A sentinel's record (high) and a tool write (certain) stay; a sync job's own (high) only gets
        // its context.
        for (source, confidence) in [
            (AttributionSource::SentinelLogin, Confidence::High),
            (AttributionSource::ToolWrite, Confidence::Certain),
        ] {
            assert_eq!(
                decide(&observation(source, confidence, false), Some(&job())),
                Decision::Leave,
                "{source:?}"
            );
        }
        assert_eq!(
            decide(
                &observation(AttributionSource::SyncJob, Confidence::High, false),
                Some(&job())
            ),
            Decision::ContextOnly
        );
    }

    #[test]
    fn a_write_that_keeps_the_attribution_writes_back_exactly_what_was_stored() {
        // The actor JSON is of a shape this code does not know: it is not parsed into anything, so it is written back as it was.
        let o = observation(AttributionSource::SyncJob, Confidence::High, false);
        let mut w = Writes::default();
        w.push(&o, &ahead(), None).unwrap();
        assert_eq!(w.ids, vec![7]);
        assert_eq!(w.states, vec!["conflict".to_owned()]);
        assert_eq!(w.hashes, vec![Some(vec![2; 32])]);
        assert_eq!(w.sources, vec!["sync_job".to_owned()]);
        assert_eq!(w.confidences, vec!["high".to_owned()]);
        assert_eq!(
            w.actors,
            vec![Some("{\"kind\":\"something_a_later_version_adds\"}".to_owned())]
        );
        assert!(
            w.items[0].upgrade.is_none(),
            "no event for a context that changes no attribution"
        );
    }

    #[test]
    fn an_upgrade_writes_the_new_attribution_and_is_named_for_the_events() {
        let o = observation(AttributionSource::Unknown, Confidence::Low, false);
        let Decision::Upgrade(sync) = decide(&o, Some(&job())) else {
            panic!("an unattributed change is upgraded");
        };
        let raw = ContentHash::from_bytes([1; 32]);
        let mut w = Writes::default();
        w.push(&o, &ahead(), Some((sync, raw))).unwrap();
        assert_eq!(w.sources, vec!["sync_job".to_owned()]);
        assert_eq!(w.confidences, vec!["high".to_owned()]);
        let actor: serde_json::Value = serde_json::from_str(w.actors[0].as_deref().unwrap()).unwrap();
        assert_eq!(actor["kind"], "job");
        assert_eq!(w.items[0].upgrade.as_ref().map(|(_, h)| *h), Some(raw));
    }
}
