//! The correlator (T7, D73, A19): which observation a sentinel record explains, which one was an NFS client's, and the one writer that
//! upgrades an attribution.
//!
//! # The order, and the one gate
//!
//! The domain model's order (`docs/domain-model.md` → Attribution) is: a tool write (certain), a sentinel record on the same path within
//! ±60 s (high), a sync window or a Job (high), an NFS client (medium), a file-owner hint (low), unknown. Three of them are decided
//! before the sentinel is heard: a tool write by ingestion and the late matcher ([`crate::tool_writes`], final: nothing here replaces
//! it), a Job by ingestion and `record_sync` ([`crate::sync`]); this file does the two that depend on the sentinel. **The only gate is
//! confidence**: an upgrade replaces an attribution only when its confidence is strictly higher (`Attribution::may_upgrade_to`), and an
//! equal one never does. It is held twice: by the statements that choose (they select only observations below the new confidence) and by
//! the `UPDATE`, which re-tests it on the row it writes, so that two writers racing for one observation cannot downgrade each other.
//! Consequences, all tested: a tool write is never touched; a sync Job's change stays the Job's even with a sentinel record next to it;
//! a sentinel's first answer stands (a second record by another user does not replace it); an NFS client's can still become a
//! sentinel's when a late record arrives.
//!
//! # The rules of a match
//!
//! A record explains an observation when all of these hold, and the nearest in time wins among several:
//!
//! - the record is the **swimlane's** (it was stored under it: [`super::map`]), on **the same path**, and **succeeded** (a failed write
//!   changed nothing, so it names nobody);
//! - the record's time is within **±60 s** of the observation's (`observed_at`, the agent's time of seeing the change; inclusive);
//! - the operation fits: a `create` or `write` explains a version of the file, a `delete` explains a deletion, a `rename` either (the
//!   record carries one path, and a rename moves both a name and a file);
//! - the record names someone (a login user, or failing that an effective user);
//! - the observation is **not part of the first inventory** (that is not a change), and is weaker than `sentinel_login`.
//!
//! A record is not used up: two changes of one path within a minute of one local write are both attributed to its user. That is the
//! rule as D73 states it, and it is a residual risk the threat model names.
//!
//! # `nfs_client`, and what "healthy" means (A19)
//!
//! An observation that nothing explains is `unknown` when it is first seen. After `nfs_client_grace` (30 s from when the hub applied it)
//! it becomes `nfs_client` (medium) only when **every** sentinel of its swimlane has been **seen since the change** (`sentinels.last_seen_at`
//! at or after the observation's time), no record matches it, and the change is not older than `nfs_client_window` (a spool replayed after
//! an outage says nothing about whether the sentinel was up when the change was made). **The sentinel is seen by `hello` and by every
//! batch; its heartbeat does not reach the registry** (the platform's stream handler drops it), so an idle VM is not seen and its NFS
//! writes stay `unknown`: the safe direction. If the platform also wrote `last_seen_at` on a heartbeat, this rule would cover idle VMs
//! with no change here.
//!
//! # Two ways in, one writer
//!
//! - **A batch arrives** ([`Inner::attribute_batch`], in the sink's transaction): the records just stored look for the observations
//!   they explain. The swimlane's lock is taken only when there is something to write.
//! - **A job runs** ([`Inner::attribute_lane`], `registry-attribution-grace`): the observations the hub applied after their record
//!   had arrived look for it, and the ones that stayed unexplained past the grace become `nfs_client`.
//!
//! Both end in [`Inner::write`]: the guarded `UPDATE`, then, in the **same transaction**, one `AttributionUpdated` outbox event and one
//! `AttributionUpgraded` audit event for each observation upgraded (batches, as the sync's claim and the tool-write matcher write theirs;
//! the outbox first and the audit last, because the audit chain's lock is the last one taken). Nothing is fanned out before the commit.
//! Ingestion is untouched: its apply loop gains no statement for any of this.

use std::collections::{BTreeSet, HashMap};

use domain::{
    Actor, Attribution, AttributionSource, Confidence, ContentHash, DomainEvent, Evidence, NfsPath,
    ShortText, SwimlaneId, Timestamp, UserId,
};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};

use super::{Inner, os_login};
use crate::db::Txn;
use crate::error::RegistryError;
use crate::ingest::apply::LOCK_SQL;
use crate::metrics::AttributionResult;

/// How far a record's time may be from the observation's: D73's ±60 seconds, a rule of the product and not a setting. Inclusive.
pub const WINDOW_MS: i64 = 60_000;

/// `(timestamptz 'epoch' + ($n::bigint::text || ' milliseconds')::interval)`: the exact conversion `crate::db::ts` makes, written as text
/// because a const cannot call it. A test holds each use to the helper's output.
macro_rules! at {
    ($p:literal) => {
        concat!(
            "(timestamptz 'epoch' + (",
            $p,
            "::bigint::text || ' milliseconds')::interval)"
        )
    };
}

/// `(EXTRACT(EPOCH FROM col) * 1000)::bigint`: what `crate::db::ms` makes.
macro_rules! ms {
    ($c:literal) => {
        concat!("(EXTRACT(EPOCH FROM ", $c, ") * 1000)::bigint")
    };
}

/// The hash an observation names in its events: its own, or for a deletion the hash of the version that went away (the closest earlier
/// observation of the path that has one), as `FileObserved` and the tool-write matcher name it.
macro_rules! event_hash {
    () => {
        "COALESCE(o.hash::bytea, (SELECT p.hash::bytea FROM file_observations p WHERE p.swimlane_id = o.swimlane_id \
         AND p.path = o.path AND p.id < o.id AND p.hash IS NOT NULL ORDER BY p.id DESC LIMIT 1))"
    };
}

/// A record that can be about an observation: the write succeeded (a refused one changed nothing), and its operation fits (a `delete`
/// explains a deletion, a `rename` either, anything else a version of the file).
macro_rules! fits {
    () => {
        "r.success AND (r.operation = 'rename' OR (r.operation = 'delete') = (o.hash IS NULL))"
    };
}

/// And it names someone, so that an attribution can be made from it. An NFS client's change is judged without this: a local write that
/// names nobody is still a local write, so the change is not an NFS client's.
macro_rules! names_someone {
    () => {
        "COALESCE(r.login_user, r.effective_user) IS NOT NULL"
    };
}

/// The record's time is within the window of the observation's (`$6`, in ms, in every statement that uses it), the record on the left:
/// for the statements that start from an observation and probe the records by `(sentinel_id, path, observed_at)`.
macro_rules! record_near {
    () => {
        "r.observed_at >= o.observed_at - ($6::bigint::text || ' milliseconds')::interval \
         AND r.observed_at <= o.observed_at + ($6::bigint::text || ' milliseconds')::interval"
    };
}

/// The columns of a candidate: the observation, the hash its events name, whether it is a deletion, and the record that explains it.
macro_rules! candidate_columns {
    () => {
        concat!(
            "o.id, o.path, ",
            event_hash!(),
            ", o.hash IS NULL, ",
            ms!("r.observed_at"),
            ", r.login_user, r.effective_user, r.exe, r.comm"
        )
    };
}

/// The observations that the records of one stored batch explain (`$1` the sentinel, `$2` the swimlane, `$3` the batch's sequence number,
/// `$4` the confidences an observation may have (those below `high`), `$5` the most rows, `$6` the window in ms). One row for each
/// observation, with the nearest record (then the earlier one, then the lower index). The records are found by the primary key's prefix
/// and the observations by `(swimlane_id, path, observed_at)`.
pub const FROM_BATCH_SQL: &str = concat!(
    "SELECT DISTINCT ON (o.id) ",
    candidate_columns!(),
    " FROM sentinel_records r \
     JOIN file_observations o ON o.swimlane_id = $2 AND o.path = r.path \
       AND o.observed_at >= r.observed_at - ($6::bigint::text || ' milliseconds')::interval \
       AND o.observed_at <= r.observed_at + ($6::bigint::text || ' milliseconds')::interval \
     WHERE r.sentinel_id = $1 AND r.batch_seq = $3 \
       AND NOT o.initial AND o.attribution_confidence = ANY($4::text[]) AND ",
    fits!(),
    " AND ",
    names_someone!(),
    " ORDER BY o.id, abs(EXTRACT(EPOCH FROM (o.observed_at - r.observed_at))), r.observed_at, r.record_index \
     LIMIT $5"
);

/// The observations of a swimlane that a record already stored explains (`$1` the swimlane, `$2` the swimlane's sentinels, `$3` the oldest
/// `observed_at` to look at in ms, `$4` the confidences, `$5` the most rows, `$6` the window in ms). The same match as [`FROM_BATCH_SQL`]
/// from the other side: the observations of the swimlane by `(swimlane_id, observed_at)`, the records by `(sentinel_id, path, observed_at)`.
pub const FROM_OBSERVATIONS_SQL: &str = concat!(
    "SELECT DISTINCT ON (o.id) ",
    candidate_columns!(),
    " FROM file_observations o \
     JOIN sentinel_records r ON r.sentinel_id = ANY($2::bigint[]) AND r.path = o.path AND ",
    record_near!(),
    // Implied by the two conditions around it, and said so that the records read are those of the look-back and nothing older: the
    // partitions before it are pruned, and a hash join over a sentinel's records cannot cost more than its look-back holds.
    " AND r.observed_at >= ",
    at!("$3"),
    " - ($6::bigint::text || ' milliseconds')::interval \
     WHERE o.swimlane_id = $1 AND o.observed_at > ",
    at!("$3"),
    " AND NOT o.initial AND o.attribution_confidence = ANY($4::text[]) AND ",
    fits!(),
    " AND ",
    names_someone!(),
    " ORDER BY o.id, abs(EXTRACT(EPOCH FROM (o.observed_at - r.observed_at))), r.observed_at, r.sentinel_id, r.batch_seq, r.record_index \
     LIMIT $5"
);

/// The observations of a swimlane that are an NFS client's (A19): applied at least the grace ago and seen no later than the window ago
/// (`$2` the oldest `observed_at`, `$3` the latest `recorded_at`, both in ms), weaker than `nfs_client` (`$4`), from a swimlane that has
/// sentinels all of which have been seen since the change, and that no stored record explains (`$6` the window in ms). `$1` the swimlane,
/// `$5` the most rows.
pub const NFS_CLIENT_SQL: &str = concat!(
    "SELECT o.id, o.path, ",
    event_hash!(),
    ", o.hash IS NULL, ",
    ms!("o.observed_at"),
    " FROM file_observations o \
     WHERE o.swimlane_id = $1 AND o.observed_at > ",
    at!("$2"),
    " AND o.recorded_at <= ",
    at!("$3"),
    " AND NOT o.initial AND o.attribution_confidence = ANY($4::text[]) \
       AND EXISTS (SELECT 1 FROM sentinels s WHERE s.swimlane_id = o.swimlane_id LIMIT 1) \
       AND NOT EXISTS (SELECT 1 FROM sentinels s WHERE s.swimlane_id = o.swimlane_id \
                       AND (s.last_seen_at IS NULL OR s.last_seen_at < o.observed_at) LIMIT 1) \
       AND NOT EXISTS (SELECT 1 FROM sentinel_records r JOIN sentinels s ON s.id = r.sentinel_id \
                       WHERE s.swimlane_id = o.swimlane_id AND r.path = o.path AND ",
    record_near!(),
    " AND ",
    fits!(),
    " LIMIT 1) ORDER BY o.id LIMIT $5"
);

/// The upgrade (`$1` the swimlane, `$2` the observations, `$3` to `$6` the source, the confidence, the actor as JSON and the evidence as
/// JSON of each, `$7` the confidences that may be replaced). An observation that is not below the new confidence **when the row is
/// written** is not touched: attribution only goes up, whoever got there first. Returns the ids it upgraded.
pub const UPGRADE_SQL: &str = "UPDATE file_observations o SET attribution_source = x.src, attribution_confidence = x.conf, \
            attribution_actor = x.actor::jsonb, attribution_evidence = x.evidence::jsonb \
     FROM unnest($2::bigint[], $3::text[], $4::text[], $5::text[], $6::text[]) AS x(id, src, conf, actor, evidence) \
     WHERE o.swimlane_id = $1 AND o.id = x.id AND o.attribution_confidence = ANY($7::text[]) \
     RETURNING o.id";

/// The confidences strictly below `confidence`, as the words the table stores: what an upgrade to it may replace. Built from the
/// enum's order, so that the gate is `Attribution::may_upgrade_to` and not a second list.
pub(crate) fn below(confidence: Confidence) -> Vec<String> {
    Confidence::ALL
        .into_iter()
        .filter(|c| *c < confidence)
        .map(|c| c.as_str().to_owned())
        .collect()
}

/// What a record says, as the statements return it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFacts {
    /// The record's own time (the VM's clock), for the evidence.
    pub at_ms: i64,
    pub login: Option<String>,
    pub effective: Option<String>,
    pub exe: Option<String>,
    pub comm: Option<String>,
}

impl RecordFacts {
    /// The OS Login name that the attribution is about: the **original login identity** (auditd keeps it through `sudo`, D72), and the
    /// effective user only when the record has no login identity. Empty is not a name.
    pub fn identity(&self) -> Option<&str> {
        self.login
            .as_deref()
            .filter(|l| !l.is_empty())
            .or_else(|| self.effective.as_deref().filter(|e| !e.is_empty()))
    }

    /// The attribution a record gives: `sentinel_login`, high, the user (or the OS Login name when no user is mapped), and the facts that
    /// made it (the executable, the login identity when it is not the effective user, the record's time).
    pub fn attribution(&self, user: Option<&UserId>) -> Option<Attribution> {
        let identity = self.identity()?;
        let actor = match user {
            Some(user) => Actor::User { user: user.clone() },
            None => Actor::OsLogin {
                login: ShortText::parse(identity).ok()?,
            },
        };
        let executable = self
            .exe
            .as_deref()
            .filter(|e| !e.is_empty())
            .or_else(|| self.comm.as_deref().filter(|c| !c.is_empty()))
            .and_then(|e| ShortText::parse(e).ok());
        let login_identity = self
            .login
            .as_deref()
            .filter(|l| !l.is_empty() && Some(*l) != self.effective.as_deref())
            .and_then(|l| ShortText::parse(l).ok());
        Some(Attribution {
            source: AttributionSource::SentinelLogin,
            confidence: Confidence::High,
            actor: Some(actor),
            evidence: Evidence {
                executable,
                login_identity,
                observed_at: Some(Timestamp::from_unix_millis(self.at_ms)),
            },
        })
    }
}

/// An observation that a record explains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub id: i64,
    pub path: NfsPath,
    /// The hash its events name (its own, or the version a deletion removed).
    pub hash: ContentHash,
    pub deletion: bool,
    pub record: RecordFacts,
}

/// An observation that is an NFS client's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NfsCandidate {
    pub id: i64,
    pub path: NfsPath,
    pub hash: ContentHash,
    pub deletion: bool,
    pub observed_at_ms: i64,
}

/// One row of [`FROM_BATCH_SQL`] and [`FROM_OBSERVATIONS_SQL`].
type CandidateRow = (
    i64,
    String,
    Option<Vec<u8>>,
    bool,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// One row of [`NFS_CLIENT_SQL`].
type NfsRow = (i64, String, Option<Vec<u8>>, bool, i64);

fn hash_of(bytes: Vec<u8>) -> Option<ContentHash> {
    <[u8; 32]>::try_from(bytes).ok().map(ContentHash::from_bytes)
}

fn candidate_of(row: CandidateRow) -> Option<Candidate> {
    let (id, path, hash, deletion, at_ms, login, effective, exe, comm) = row;
    Some(Candidate {
        id,
        path: NfsPath::parse(&path).ok()?,
        hash: hash_of(hash?)?,
        deletion,
        record: RecordFacts {
            at_ms,
            login,
            effective,
            exe,
            comm,
        },
    })
}

fn nfs_candidate_of(row: NfsRow) -> Option<NfsCandidate> {
    let (id, path, hash, deletion, observed_at_ms) = row;
    Some(NfsCandidate {
        id,
        path: NfsPath::parse(&path).ok()?,
        hash: hash_of(hash?)?,
        deletion,
        observed_at_ms,
    })
}

/// What an upgrade writes for one observation.
struct Upgrade {
    id: i64,
    path: NfsPath,
    hash: ContentHash,
    deletion: bool,
    attribution: Attribution,
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, RegistryError> {
    serde_json::to_string(value).map_err(|_| RegistryError::Invalid)
}

impl Inner {
    /// The observations that the records of batch `seq` of `sentinel` explain in `swimlane`, read on `conn`.
    async fn batch_candidates(
        &self,
        conn: &mut sqlx::PgConnection,
        sentinel: i64,
        swimlane: &SwimlaneId,
        seq: i64,
    ) -> Result<Vec<Candidate>, RegistryError> {
        let rows: Vec<CandidateRow> = self
            .db
            .run(
                "attribution_from_batch",
                sqlx::query_as(FROM_BATCH_SQL)
                    .bind(sentinel)
                    .bind(swimlane.as_str())
                    .bind(seq)
                    .bind(below(Confidence::High))
                    .bind(self.limit())
                    .bind(WINDOW_MS)
                    .fetch_all(&mut *conn),
            )
            .await?;
        Ok(rows.into_iter().filter_map(candidate_of).collect())
    }

    /// The observations of `swimlane` that records already stored explain, read on `conn`.
    pub(super) async fn observation_candidates(
        &self,
        conn: &mut sqlx::PgConnection,
        swimlane: &SwimlaneId,
        sentinels: &[i64],
        now_ms: i64,
    ) -> Result<Vec<Candidate>, RegistryError> {
        let oldest = now_ms.saturating_sub(duration_ms(self.config.sentinel_lookback));
        let rows: Vec<CandidateRow> = self
            .db
            .run(
                "attribution_from_observations",
                sqlx::query_as(FROM_OBSERVATIONS_SQL)
                    .bind(swimlane.as_str())
                    .bind(sentinels)
                    // Only the weakest: an NFS client's change is found by a record that arrives later, through the batch.
                    .bind(oldest)
                    .bind(below(Confidence::Medium))
                    .bind(self.limit())
                    .bind(WINDOW_MS)
                    .fetch_all(&mut *conn),
            )
            .await?;
        Ok(rows.into_iter().filter_map(candidate_of).collect())
    }

    /// The observations of `swimlane` that are an NFS client's, read on `conn`.
    pub(super) async fn nfs_candidates(
        &self,
        conn: &mut sqlx::PgConnection,
        swimlane: &SwimlaneId,
        now_ms: i64,
    ) -> Result<Vec<NfsCandidate>, RegistryError> {
        let rows: Vec<NfsRow> = self
            .db
            .run(
                "attribution_nfs_client",
                sqlx::query_as(NFS_CLIENT_SQL)
                    .bind(swimlane.as_str())
                    .bind(now_ms.saturating_sub(duration_ms(self.config.nfs_client_window)))
                    .bind(now_ms.saturating_sub(duration_ms(self.config.nfs_client_grace)))
                    .bind(below(Confidence::Medium))
                    .bind(self.limit())
                    .bind(WINDOW_MS)
                    .fetch_all(&mut *conn),
            )
            .await?;
        Ok(rows.into_iter().filter_map(nfs_candidate_of).collect())
    }

    fn limit(&self) -> i64 {
        i64::try_from(self.config.attribution_batch).unwrap_or(i64::MAX)
    }

    /// The records of a stored batch look for the observations they explain, in the sink's transaction. Returns how many observations
    /// became `sentinel_login`. Nothing is locked when nothing matches (the usual state: most of what a sentinel sees is not a change
    /// the agent has reported yet).
    pub(super) async fn attribute_batch(
        &self,
        tx: &mut Txn,
        sentinel: i64,
        name: &ShortText,
        swimlane: &SwimlaneId,
        seq: i64,
    ) -> Result<usize, RegistryError> {
        if self
            .batch_candidates(tx, sentinel, swimlane, seq)
            .await?
            .is_empty()
        {
            return Ok(0);
        }
        // The swimlane's lock, the one every apply of the swimlane holds (always taken before the audit chain's), and the look again under
        // it: what an apply that ran meanwhile attributed is not touched.
        self.db
            .run(
                "attribution_lock",
                sqlx::query(LOCK_SQL).bind(swimlane.as_str()).execute(&mut **tx),
            )
            .await?;
        let found = self.batch_candidates(tx, sentinel, swimlane, seq).await?;
        let actor = AuditActor::Sentinel { name: name.clone() };
        self.upgrade_from_records(tx, swimlane, found, actor, AttributionResult::SentinelOnRecord)
            .await
    }

    /// The upgrades that stored records give the observations they explain. `found` is what a look on `tx` returned, under the swimlane's
    /// lock. Returns how many observations were upgraded.
    pub(super) async fn upgrade_from_records(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        found: Vec<Candidate>,
        actor: AuditActor,
        result: AttributionResult,
    ) -> Result<usize, RegistryError> {
        if found.is_empty() {
            return Ok(0);
        }
        let names: BTreeSet<String> = found
            .iter()
            .filter_map(|c| c.record.identity().map(str::to_owned))
            .collect();
        let users: HashMap<String, UserId> = os_login::resolve(&self.db, tx, &names).await?;
        let upgrades: Vec<Upgrade> = found
            .into_iter()
            .filter_map(|c| {
                let user = c.record.identity().and_then(|n| users.get(n));
                let attribution = c.record.attribution(user)?;
                Some(Upgrade {
                    id: c.id,
                    path: c.path,
                    hash: c.hash,
                    deletion: c.deletion,
                    attribution,
                })
            })
            .collect();
        self.write(tx, swimlane, upgrades, Confidence::High, actor, result)
            .await
    }

    /// The upgrades of the observations that an NFS client made (A19). Returns how many were upgraded.
    pub(super) async fn upgrade_nfs_client(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        found: Vec<NfsCandidate>,
        actor: AuditActor,
    ) -> Result<usize, RegistryError> {
        let upgrades: Vec<Upgrade> = found
            .into_iter()
            .map(|c| Upgrade {
                id: c.id,
                path: c.path,
                hash: c.hash,
                deletion: c.deletion,
                attribution: Attribution {
                    source: AttributionSource::NfsClient,
                    confidence: Confidence::Medium,
                    actor: None,
                    evidence: Evidence {
                        observed_at: Some(Timestamp::from_unix_millis(c.observed_at_ms)),
                        ..Evidence::default()
                    },
                },
            })
            .collect();
        self.write(
            tx,
            swimlane,
            upgrades,
            Confidence::Medium,
            actor,
            AttributionResult::NfsClient,
        )
        .await
    }

    /// The one writer: the guarded `UPDATE`, then an `AttributionUpdated` and an `AttributionUpgraded` for each observation it upgraded, in
    /// the transaction. The caller holds the swimlane's lock.
    async fn write(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        upgrades: Vec<Upgrade>,
        to: Confidence,
        actor: AuditActor,
        result: AttributionResult,
    ) -> Result<usize, RegistryError> {
        if upgrades.is_empty() {
            return Ok(0);
        }
        let ids: Vec<i64> = upgrades.iter().map(|u| u.id).collect();
        let sources: Vec<&str> = upgrades.iter().map(|u| u.attribution.source.as_str()).collect();
        let confidences: Vec<&str> = upgrades
            .iter()
            .map(|u| u.attribution.confidence.as_str())
            .collect();
        let actors: Vec<Option<String>> = upgrades
            .iter()
            .map(|u| u.attribution.actor.as_ref().map(json).transpose())
            .collect::<Result<_, _>>()?;
        let evidence: Vec<String> = upgrades
            .iter()
            .map(|u| json(&u.attribution.evidence))
            .collect::<Result<_, _>>()?;
        let done: Vec<i64> = self
            .db
            .run(
                "attribution_upgrade",
                sqlx::query_scalar(UPGRADE_SQL)
                    .bind(swimlane.as_str())
                    .bind(&ids)
                    .bind(&sources)
                    .bind(&confidences)
                    .bind(&actors)
                    .bind(&evidence)
                    .bind(below(to))
                    .fetch_all(&mut **tx),
            )
            .await?;
        let raced = upgrades.len().saturating_sub(done.len());
        let applied: Vec<Upgrade> = upgrades.into_iter().filter(|u| done.contains(&u.id)).collect();
        self.metrics
            .attribution(AttributionResult::Raced, u64::try_from(raced).unwrap_or(u64::MAX));
        if applied.is_empty() {
            return Ok(0);
        }

        let now = Timestamp::from_unix_millis(self.clock.now().unix_millis());
        let events: Vec<DomainEvent> = applied
            .iter()
            .map(|u| DomainEvent::AttributionUpdated {
                swimlane: swimlane.clone(),
                path: u.path.clone(),
                hash: u.hash,
            })
            .collect();
        let audit: Vec<AuditEvent> = applied
            .iter()
            .map(|u| {
                let mut e = AuditEvent::new(
                    now,
                    actor.clone(),
                    AuditAction::AttributionUpgraded,
                    AuditVia::System,
                );
                e.swimlane = Some(swimlane.clone());
                e.path = Some(u.path.clone());
                // A deletion has no hash after it: the event names the version that went away, as a change that was detected has it.
                if u.deletion {
                    e.hash_before = Some(u.hash);
                } else {
                    e.hash_after = Some(u.hash);
                }
                e.attribution = Some(u.attribution.clone());
                e
            })
            .collect();
        // The outbox events first and the audit events last: the chain's lock is the last thing taken.
        {
            let mut handle = Tx::from_pg(tx);
            crate::batch::publish_all(&*self.bus, &mut handle, events).await?;
        }
        {
            let mut handle = Tx::from_pg(tx);
            crate::batch::record_all(&*self.audit, &mut handle, audit).await?;
        }
        self.metrics
            .attribution(result, u64::try_from(applied.len()).unwrap_or(u64::MAX));
        Ok(applied.len())
    }
}

/// A duration in whole milliseconds, saturating.
pub(crate) fn duration_ms(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ms, ts};

    fn facts(login: Option<&str>, effective: Option<&str>) -> RecordFacts {
        RecordFacts {
            at_ms: 1_800_000_000_000,
            login: login.map(str::to_owned),
            effective: effective.map(str::to_owned),
            exe: Some("/usr/bin/vim".to_owned()),
            comm: Some("vim".to_owned()),
        }
    }

    #[test]
    fn what_may_be_replaced_is_what_is_below_the_new_confidence() {
        assert_eq!(below(Confidence::Low), Vec::<String>::new());
        assert_eq!(below(Confidence::Medium), vec!["low"]);
        assert_eq!(below(Confidence::High), vec!["low", "medium"]);
        assert_eq!(below(Confidence::Certain), vec!["low", "medium", "high"]);
    }

    #[test]
    fn the_gate_is_the_domains_may_upgrade_to() {
        // `below` is built from the enum's order, and agrees with `Attribution::may_upgrade_to` for every pair.
        let of = |confidence| Attribution {
            confidence,
            ..Attribution::unknown()
        };
        for held in Confidence::ALL {
            for new in Confidence::ALL {
                assert_eq!(
                    of(held).may_upgrade_to(&of(new)),
                    below(new).iter().any(|w| w == held.as_str()),
                    "{held} -> {new}"
                );
            }
        }
    }

    #[test]
    fn the_window_is_sixty_seconds_and_the_statements_bind_it() {
        assert_eq!(WINDOW_MS, 60_000, "D73");
        for sql in [FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL, NFS_CLIENT_SQL] {
            assert!(sql.contains("$6::bigint::text || ' milliseconds'"), "{sql}");
            assert!(
                !sql.contains("60 seconds") && !sql.contains("'60 "),
                "no second copy of the number: {sql}"
            );
        }
    }

    #[test]
    fn the_statements_use_the_exact_time_expressions_of_the_db_helpers() {
        assert!(FROM_OBSERVATIONS_SQL.contains(&ts("$3")));
        assert!(NFS_CLIENT_SQL.contains(&ts("$2")));
        assert!(NFS_CLIENT_SQL.contains(&ts("$3")));
        assert!(FROM_BATCH_SQL.contains(&ms("r.observed_at")));
        assert!(NFS_CLIENT_SQL.contains(&ms("o.observed_at")));
    }

    #[test]
    fn a_record_explains_nothing_that_failed_and_names_nobody_without_a_user() {
        for sql in [FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL, NFS_CLIENT_SQL] {
            assert!(sql.contains("r.success"), "a failed write changed nothing: {sql}");
            assert!(
                sql.contains("(r.operation = 'delete') = (o.hash IS NULL)"),
                "{sql}"
            );
            assert!(sql.contains("r.operation = 'rename'"), "{sql}");
        }
        for sql in [FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL] {
            assert!(
                sql.contains("COALESCE(r.login_user, r.effective_user) IS NOT NULL"),
                "an attribution needs a name: {sql}"
            );
        }
        assert!(
            !NFS_CLIENT_SQL.contains("COALESCE(r.login_user, r.effective_user)"),
            "a local write that names nobody is still a local write: the change is not an NFS client's"
        );
        for sql in [FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL, NFS_CLIENT_SQL] {
            assert!(
                sql.contains("NOT o.initial"),
                "the first inventory is not a change: {sql}"
            );
        }
    }

    #[test]
    fn the_nearest_record_wins_then_the_earlier_one() {
        for sql in [FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL] {
            assert!(sql.contains("SELECT DISTINCT ON (o.id)"), "{sql}");
            assert!(
                sql.contains(
                    "ORDER BY o.id, abs(EXTRACT(EPOCH FROM (o.observed_at - r.observed_at))), r.observed_at"
                ),
                "{sql}"
            );
        }
    }

    #[test]
    fn the_update_writes_only_what_is_below_the_new_confidence_when_the_row_is_written() {
        assert!(UPGRADE_SQL.contains("o.attribution_confidence = ANY($7::text[])"));
        assert!(UPGRADE_SQL.contains("RETURNING o.id"));
    }

    #[test]
    fn a_nfs_client_needs_every_sentinel_seen_since_the_change_and_no_record() {
        assert!(
            NFS_CLIENT_SQL
                .contains("EXISTS (SELECT 1 FROM sentinels s WHERE s.swimlane_id = o.swimlane_id LIMIT 1)")
        );
        assert!(NFS_CLIENT_SQL.contains("s.last_seen_at IS NULL OR s.last_seen_at < o.observed_at"));
        assert!(NFS_CLIENT_SQL.contains("NOT EXISTS (SELECT 1 FROM sentinel_records r"));
        assert!(NFS_CLIENT_SQL.contains("o.recorded_at <="));
    }

    #[test]
    fn the_identity_is_the_login_user_and_the_effective_user_only_without_one() {
        assert_eq!(
            facts(Some("ada_example_com"), Some("root")).identity(),
            Some("ada_example_com")
        );
        assert_eq!(facts(None, Some("root")).identity(), Some("root"));
        assert_eq!(facts(Some(""), Some("root")).identity(), Some("root"));
        assert_eq!(facts(Some(""), Some("")).identity(), None);
        assert_eq!(facts(None, None).identity(), None);
        assert!(
            facts(None, None).attribution(None).is_none(),
            "nobody to name, nothing to attribute"
        );
    }

    #[test]
    fn an_unmapped_login_is_the_os_login_actor_and_a_mapped_one_is_the_user() {
        let record = facts(Some("ada_example_com"), Some("root"));
        let a = record.attribution(None).expect("an attribution");
        assert_eq!(a.source, AttributionSource::SentinelLogin);
        assert_eq!(a.confidence, Confidence::High);
        assert_eq!(
            a.actor,
            Some(Actor::OsLogin {
                login: ShortText::parse("ada_example_com").unwrap()
            })
        );
        let user = UserId::new(
            domain::Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
            domain::Guid::parse("00000000-0000-4000-8000-000000000001").unwrap(),
        );
        let a = record.attribution(Some(&user)).expect("an attribution");
        assert_eq!(a.actor, Some(Actor::User { user }));
    }

    #[test]
    fn the_evidence_is_the_executable_the_login_identity_when_it_differs_and_the_records_time() {
        let a = facts(Some("ada_example_com"), Some("root"))
            .attribution(None)
            .unwrap();
        assert_eq!(
            a.evidence.executable.as_ref().map(ShortText::as_str),
            Some("/usr/bin/vim")
        );
        assert_eq!(
            a.evidence.login_identity.as_ref().map(ShortText::as_str),
            Some("ada_example_com"),
            "the original login identity, through sudo"
        );
        assert_eq!(
            a.evidence.observed_at,
            Some(Timestamp::from_unix_millis(1_800_000_000_000))
        );
        // The same person at their own privileges: nothing differs, so nothing is added.
        let a = facts(Some("ada"), Some("ada")).attribution(None).unwrap();
        assert_eq!(a.evidence.login_identity, None);
        // No executable: the command name stands in.
        let mut record = facts(Some("ada"), Some("ada"));
        record.exe = None;
        assert_eq!(
            record
                .attribution(None)
                .unwrap()
                .evidence
                .executable
                .as_ref()
                .map(ShortText::as_str),
            Some("vim")
        );
        record.comm = None;
        assert_eq!(record.attribution(None).unwrap().evidence.executable, None);
    }

    #[test]
    fn a_row_that_cannot_be_read_back_is_skipped() {
        let good: CandidateRow = (
            1,
            "svc/a.yml".to_owned(),
            Some(vec![1; 32]),
            false,
            5,
            Some("ada".to_owned()),
            None,
            None,
            None,
        );
        assert!(candidate_of(good.clone()).is_some());
        let mut bad = good.clone();
        bad.2 = Some(vec![1; 31]);
        assert!(candidate_of(bad).is_none(), "a hash of the wrong length");
        let mut bad = good.clone();
        bad.2 = None;
        assert!(
            candidate_of(bad).is_none(),
            "a deletion with no version before it names nothing"
        );
        let mut bad = good;
        bad.1 = "../a.yml".to_owned();
        assert!(candidate_of(bad).is_none(), "a path that is not one");
    }
}
