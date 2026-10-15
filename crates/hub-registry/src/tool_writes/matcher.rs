//! The late commit (T1, A10): the leased job `registry-tool-write-matcher`.
//!
//! A registration becomes visible when the caller's transaction commits, and prompt 06 commits after the file is written, so the agent can
//! report the new file before the registration is visible: the observation is applied, nothing explains it, and it is an out-of-band change
//! with an unknown author. A lock would close that window (the observation could not be applied while a registration is in flight), but
//! the order of such a lock against the audit chain's lock cannot be controlled from here (prompt 06 may record its audit event first), and a
//! deadlock with an ingestion apply is worse than a short delay. So the window is closed afterwards, by this job:
//!
//! 1. **Expire.** Registrations stamped at or before `now - TTL` are deleted, in batches of `tool_write_sweep_batch`, at most ten batches
//!    in a run. The oldest first (the age index serves it).
//! 2. **Look.** The swimlanes that have a live registration, at most `tool_write_match_lanes`. For each: read its live registrations and the
//!    observations of their paths applied within the TTL, and decide which observation each registration explains
//!    ([`super::matching::late_matches_of_lane`]). With nothing to explain the swimlane's lock is never taken: this is the usual state, and
//!    the lock is the one every apply of the swimlane's chunks holds.
//! 3. **Attribute**, in one transaction under the swimlane's lock (the same lock ingestion's chunks take, always first; the audit chain's lock
//!    is taken last, by the audit write): the look is made again under the lock, each observation is upgraded to `tool_write` / `certain`
//!    with the registration's user as the actor and its source becomes `tool_write`, the registration is consumed (a `DELETE`, in the same
//!    transaction: the app holds no row locks), and one `AttributionUpgraded` audit event and one `AttributionUpdated` outbox event are
//!    written for each upgrade, as batches, exactly as a sync's claim does. An observation that stopped being eligible in between is not
//!    touched and its registration is kept.
//!
//! Attribution only goes up: an observation that is a tool write already, or that nothing could be above, is left alone. A run is bounded
//! (swimlanes, rows read, batches) and asks [`JobContext::ensure_held`] before each unit of work that writes.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    Actor, Attribution, AttributionSource, Confidence, DomainEvent, Evidence, NfsPath, ShortText, SwimlaneId,
    Timestamp,
};
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};
use sqlx::PgConnection;
use tracing::debug;

use super::matching::{Observed, late_matches_of_lane};
use super::{DROP_SQL, Inner, MAX_READ, Pending, PendingRow, TTL_MS, pending_of};
use crate::config::RegistryConfig;
use crate::error::RegistryError;
use crate::ingest::apply::LOCK_SQL;
use crate::metrics::ToolWriteResult;

/// The job's name, which is also its lease's name.
pub const JOB_NAME: &str = "registry-tool-write-matcher";

/// The audit actor of an upgrade the matcher makes: the registry's own component.
const COMPONENT: &str = "registry-tool-write-matcher";

/// Statements of the expiry sweep in one run.
const MAX_SWEEP_ROUNDS: usize = 10;

/// The swimlanes that have a live registration (`$1` the cutoff in Unix ms, `$2` the limit).
pub const LANES_SQL: &str = "SELECT DISTINCT swimlane_id FROM tool_write_registrations \
     WHERE registered_at > (timestamptz 'epoch' + ($1::bigint::text || ' milliseconds')::interval) \
     ORDER BY swimlane_id LIMIT $2";

/// The live registrations of a swimlane, oldest first (`$1` the swimlane, `$2` the cutoff, `$3` the limit). The columns of
/// [`PendingRow`]; the last, the state before a deletion, is not read here (it is the observation's).
pub const LANE_PENDING_SQL: &str = "SELECT r.id, r.path, r.hash::bytea, r.is_delete, r.user_tid, r.user_oid, r.request_id, \
            (EXTRACT(EPOCH FROM r.registered_at) * 1000)::bigint, NULL::bytea \
     FROM tool_write_registrations r \
     WHERE r.swimlane_id = $1 \
       AND r.registered_at > (timestamptz 'epoch' + ($2::bigint::text || ' milliseconds')::interval) \
     ORDER BY r.id LIMIT $3";

/// The observations of some paths that the registry applied after the cutoff (`$1` the swimlane, `$2` the paths, `$3` the cutoff in Unix ms,
/// `$4` the limit), in the order of the path's observations. For a deletion, the state of the path immediately before it, read by the
/// same text that ingestion embeds.
pub const OBSERVATIONS_SQL: &str = concat!(
    "SELECT o.id, o.path, o.hash::bytea, o.attribution_source, o.attribution_confidence, \
            (EXTRACT(EPOCH FROM o.recorded_at) * 1000)::bigint, (EXTRACT(EPOCH FROM o.observed_at) * 1000)::bigint, \
            CASE WHEN o.hash IS NULL THEN ",
    state_before!(of_observation),
    " END \
     FROM file_observations o \
     WHERE o.swimlane_id = $1 AND o.path = ANY($2::text[]) \
       AND o.recorded_at > (timestamptz 'epoch' + ($3::bigint::text || ' milliseconds')::interval) \
     ORDER BY o.path, o.id LIMIT $4"
);

/// The upgrade of the observations a registration explains (`$1` the swimlane, `$2` the observation ids, `$3` the actor of each, as JSON).
/// An observation that became a tool write, or reached `certain`, since it was read is not touched: attribution only goes up. Returns the
/// ids it upgraded.
pub const ATTRIBUTE_SQL: &str = "UPDATE file_observations o SET source = 'tool_write', attribution_source = 'tool_write', \
            attribution_confidence = 'certain', attribution_actor = x.actor::jsonb \
     FROM unnest($2::bigint[], $3::text[]) AS x(id, actor) \
     WHERE o.swimlane_id = $1 AND o.id = x.id AND o.attribution_source <> 'tool_write' \
       AND o.attribution_confidence <> 'certain' \
     RETURNING o.id";

/// The expiry sweep: the oldest registrations stamped at or before the cutoff (`$1` in Unix ms), at most `$2`. The age index serves it.
pub const SWEEP_SQL: &str = "DELETE FROM tool_write_registrations WHERE id = ANY(ARRAY( \
       SELECT id FROM tool_write_registrations \
       WHERE registered_at <= (timestamptz 'epoch' + ($1::bigint::text || ' milliseconds')::interval) \
       ORDER BY registered_at, id LIMIT $2))";

/// An observation with its path, as [`OBSERVATIONS_SQL`] returns it.
type ObservationRow = (
    i64,
    String,
    Option<Vec<u8>>,
    String,
    String,
    i64,
    i64,
    Option<Vec<u8>>,
);

/// What one run did, in counts. Nothing in it is content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatchReport {
    /// Registrations deleted because they expired.
    pub expired: u64,
    /// Swimlanes looked at.
    pub lanes: usize,
    /// Observations upgraded to a tool write (and registrations consumed).
    pub matched: usize,
    /// Swimlanes that could not be handled; the next run looks at them again.
    pub failed: usize,
    /// More swimlanes had live registrations than `tool_write_match_lanes`; the next run goes on.
    pub truncated: bool,
}

/// The matcher. Cloning shares everything.
#[derive(Clone)]
pub struct ToolWriteMatcher {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ToolWriteMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolWriteMatcher").finish_non_exhaustive()
    }
}

impl ToolWriteMatcher {
    pub(super) fn new(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    /// How the job is scheduled: every `tool_write_match_interval`, under a 90 second lease that the runner renews every 30, for at most
    /// ten minutes.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes (the interval is range-checked by `RegistryConfig::validate`).
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(
            JOB_NAME,
            config.tool_write_match_interval,
            Duration::from_secs(90),
        )?
        .with_max_run(Duration::from_secs(10 * 60))
    }

    /// The job (`registry-tool-write-matcher`), for the hub's job runner.
    pub fn job(&self) -> Arc<dyn Job> {
        Arc::new(Run {
            matcher: self.clone(),
        })
    }

    /// One run, as a test calls it: the lease is taken to be held.
    ///
    /// # Errors
    ///
    /// `Failed` when the expiry sweep or the list of swimlanes cannot be read.
    pub async fn run_once(&self) -> Result<MatchReport, JobError> {
        self.sweep(|| Ok(())).await
    }

    /// One run. `held` is asked before each unit of work that writes and ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`; `Failed` when the sweep or the list of swimlanes cannot be read.
    pub async fn sweep(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send,
    ) -> Result<MatchReport, JobError> {
        let inner = &*self.inner;
        held()?;
        let now_ms = inner.clock.now().unix_millis();
        let mut report = MatchReport {
            expired: inner.expire(now_ms, &held).await?,
            ..MatchReport::default()
        };
        inner.metrics.tool_write(ToolWriteResult::Expired, report.expired);

        let limit = i64::try_from(inner.config.tool_write_match_lanes).unwrap_or(i64::MAX);
        let lanes: Vec<String> = inner
            .db
            .run(
                "tool_write_lanes",
                sqlx::query_scalar(LANES_SQL)
                    .bind(now_ms.saturating_sub(TTL_MS))
                    .bind(limit.saturating_add(1))
                    .fetch_all(inner.db.pool()),
            )
            .await
            .map_err(|_| JobError::Failed("registry_tool_write_lanes"))?;
        report.truncated = i64::try_from(lanes.len()).is_ok_and(|n| n > limit);
        for lane in lanes.iter().take(inner.config.tool_write_match_lanes) {
            held()?;
            let Ok(lane) = SwimlaneId::parse(lane) else {
                continue;
            };
            match inner.attribute_lane(&lane, now_ms).await {
                Ok(n) => {
                    report.lanes += 1;
                    report.matched += n;
                }
                Err(_) => report.failed += 1,
            }
        }
        // Counts only (S10).
        debug!(
            expired = report.expired,
            lanes = report.lanes,
            matched = report.matched,
            failed = report.failed,
            "tool write matcher"
        );
        Ok(report)
    }
}

/// What the matcher found for one registration.
struct Found {
    path: NfsPath,
    registration: Pending,
    observation: Observed,
}

impl Inner {
    /// Delete the registrations that expired, a batch at a time.
    async fn expire(
        &self,
        now_ms: i64,
        held: &(impl Fn() -> Result<(), JobError> + Send),
    ) -> Result<u64, JobError> {
        let batch = i64::try_from(self.config.tool_write_sweep_batch).unwrap_or(i64::MAX);
        let mut total = 0_u64;
        for _ in 0..MAX_SWEEP_ROUNDS {
            held()?;
            let done = self
                .db
                .run(
                    "tool_write_sweep",
                    sqlx::query(SWEEP_SQL)
                        .bind(now_ms.saturating_sub(TTL_MS))
                        .bind(batch)
                        .execute(self.db.pool()),
                )
                .await
                .map_err(|_| JobError::Failed("registry_tool_write_sweep"))?
                .rows_affected();
            total += done;
            if i64::try_from(done).is_ok_and(|d| d < batch) {
                break;
            }
        }
        Ok(total)
    }

    /// The observations that the live registrations of `swimlane` explain, by the rules of [`super::matching`]. Read-only.
    async fn find(
        &self,
        conn: &mut PgConnection,
        swimlane: &SwimlaneId,
        now_ms: i64,
    ) -> Result<Vec<Found>, RegistryError> {
        let cutoff = now_ms.saturating_sub(TTL_MS);
        let rows: Vec<PendingRow> = self
            .db
            .run(
                "tool_write_lane_pending",
                sqlx::query_as(LANE_PENDING_SQL)
                    .bind(swimlane.as_str())
                    .bind(cutoff)
                    .bind(MAX_READ)
                    .fetch_all(&mut *conn),
            )
            .await?;
        let mut pending: BTreeMap<NfsPath, Vec<Pending>> = BTreeMap::new();
        let mut unreadable = 0_u64;
        for row in rows {
            match pending_of(row) {
                Some((path, registration, _)) => pending.entry(path).or_default().push(registration),
                None => unreadable += 1,
            }
        }
        self.metrics.tool_write(ToolWriteResult::Unreadable, unreadable);
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        let paths: Vec<String> = pending.keys().map(|p| p.as_str().to_owned()).collect();
        let rows: Vec<ObservationRow> = self
            .db
            .run(
                "tool_write_lane_observations",
                sqlx::query_as(OBSERVATIONS_SQL)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(cutoff)
                    .bind(MAX_READ)
                    .fetch_all(&mut *conn),
            )
            .await?;
        let mut observed: BTreeMap<NfsPath, Vec<Observed>> = BTreeMap::new();
        for (id, path, hash, source, confidence, applied_ms, agent_seen_ms, before) in rows {
            let (Ok(path), Ok(source), Ok(confidence)) = (
                NfsPath::parse(&path),
                AttributionSource::from_str(&source),
                Confidence::from_str(&confidence),
            ) else {
                continue;
            };
            observed.entry(path).or_default().push(Observed {
                id,
                hash: hash.and_then(super::hash_of),
                source,
                confidence,
                applied_ms,
                agent_seen_ms,
                before: before.and_then(super::hash_of),
            });
        }
        Ok(late_matches_of_lane(&pending, &observed, now_ms)
            .into_iter()
            .map(|(path, registration, observation)| Found {
                path,
                registration,
                observation,
            })
            .collect())
    }

    /// Explain the observations of one swimlane that a registration committed after. Returns how many were upgraded.
    async fn attribute_lane(&self, swimlane: &SwimlaneId, now_ms: i64) -> Result<usize, RegistryError> {
        // Look first, without the lock: with nothing to explain (the usual state) the swimlane's lock is never taken.
        let found = {
            let mut conn = self.db.run("tool_write_lane", self.db.pool().acquire()).await?;
            self.find(&mut conn, swimlane, now_ms).await?
        };
        if found.is_empty() {
            return Ok(0);
        }

        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "tool_write_lock",
                sqlx::query(LOCK_SQL).bind(swimlane.as_str()).execute(&mut *tx),
            )
            .await?;
        // Again, under the lock: an apply that ran since the look has consumed or contradicted what it should have.
        let found = self.find(&mut tx, swimlane, now_ms).await?;
        if found.is_empty() {
            self.db.commit(tx).await?;
            return Ok(0);
        }

        let ids: Vec<i64> = found.iter().map(|f| f.observation.id).collect();
        let actors: Vec<String> = found
            .iter()
            .map(|f| {
                serde_json::to_string(&Actor::User {
                    user: f.registration.user.clone(),
                })
                .map_err(|_| RegistryError::Invalid)
            })
            .collect::<Result<_, _>>()?;
        let upgraded: Vec<i64> = self
            .db
            .run(
                "tool_write_attribute",
                sqlx::query_scalar(ATTRIBUTE_SQL)
                    .bind(swimlane.as_str())
                    .bind(&ids)
                    .bind(&actors)
                    .fetch_all(&mut *tx),
            )
            .await?;
        let done: Vec<&Found> = found
            .iter()
            .filter(|f| upgraded.contains(&f.observation.id))
            .collect();
        // The registrations of what was upgraded are spent. One that could not be used (the observation changed under us) stays.
        let spent: Vec<i64> = done.iter().map(|f| f.registration.id).collect();
        if !spent.is_empty() {
            self.db
                .run(
                    "tool_write_consume",
                    sqlx::query(DROP_SQL)
                        .bind(swimlane.as_str())
                        .bind(&spent)
                        .execute(&mut *tx),
                )
                .await?;
        }

        // The outbox events first and the audit events last: the chain's lock is the last thing taken (A12).
        let events: Vec<DomainEvent> = done
            .iter()
            .map(|f| DomainEvent::AttributionUpdated {
                swimlane: swimlane.clone(),
                path: f.path.clone(),
                // A deletion has no hash: the event names the version that went away, as `FileObserved` does.
                hash: f.observation.hash.unwrap_or(f.registration.hash),
            })
            .collect();
        let audit = done
            .iter()
            .map(|f| upgrade_event(swimlane, f, now_ms))
            .collect::<Result<Vec<_>, _>>()?;
        {
            let mut handle = Tx::from_pg(&mut tx);
            crate::batch::publish_all(&*self.bus, &mut handle, events).await?;
        }
        {
            let mut handle = Tx::from_pg(&mut tx);
            crate::batch::record_all(&*self.audit, &mut handle, audit).await?;
        }
        self.db.commit(tx).await?;
        self.metrics.tool_write(
            ToolWriteResult::MatchedLate,
            u64::try_from(done.len()).unwrap_or(u64::MAX),
        );
        Ok(done.len())
    }
}

/// The audit event of one upgrade: the attribution the observation now has, and the request that made it.
fn upgrade_event(swimlane: &SwimlaneId, f: &Found, now_ms: i64) -> Result<AuditEvent, RegistryError> {
    let component = ShortText::parse(COMPONENT).map_err(|_| RegistryError::Invalid)?;
    let mut event = AuditEvent::new(
        Timestamp::from_unix_millis(now_ms),
        AuditActor::System { component },
        AuditAction::AttributionUpgraded,
        AuditVia::System,
    );
    event.swimlane = Some(swimlane.clone());
    event.path = Some(f.path.clone());
    match f.observation.hash {
        Some(hash) => event.hash_after = Some(hash),
        // A deletion: the version that went away, as a change that was detected has it.
        None => event.hash_before = Some(f.registration.hash),
    }
    event.request_id = Some(f.registration.request_id.clone());
    event.attribution = Some(Attribution {
        source: AttributionSource::ToolWrite,
        confidence: Confidence::Certain,
        actor: Some(Actor::User {
            user: f.registration.user.clone(),
        }),
        evidence: Evidence {
            observed_at: Some(Timestamp::from_unix_millis(f.observation.agent_seen_ms)),
            ..Evidence::default()
        },
    });
    Ok(event)
}

struct Run {
    matcher: ToolWriteMatcher,
}

#[async_trait]
impl Job for Run {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.matcher.sweep(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ts;

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec = ToolWriteMatcher::spec(&RegistryConfig::default())
            .expect("the default config gives a valid spec");
        assert_eq!(spec.name(), JOB_NAME);
        assert_eq!(JOB_NAME, "registry-tool-write-matcher");
        assert!(ports::is_valid_lease_name(JOB_NAME));
    }

    #[test]
    fn a_config_the_spec_refuses_is_an_error_and_not_a_panic() {
        let config = RegistryConfig {
            tool_write_match_interval: Duration::ZERO,
            ..RegistryConfig::default()
        };
        assert!(ToolWriteMatcher::spec(&config).is_err());
    }

    #[test]
    fn the_statements_use_the_exact_time_expressions_of_the_db_helpers() {
        assert!(LANES_SQL.contains(&ts("$1")));
        assert!(LANE_PENDING_SQL.contains(&ts("$2")));
        assert!(OBSERVATIONS_SQL.contains(&ts("$3")));
        assert!(SWEEP_SQL.contains(&ts("$1")));
    }

    #[test]
    fn the_matcher_never_upgrades_what_is_a_tool_write_already() {
        assert!(ATTRIBUTE_SQL.contains("o.attribution_source <> 'tool_write'"));
        assert!(ATTRIBUTE_SQL.contains("o.attribution_confidence <> 'certain'"));
    }

    #[test]
    fn the_matcher_reads_the_state_before_a_deletion_where_ingestion_does() {
        let shared = "p.id < o.id AND p.hash IS NOT NULL ORDER BY p.id DESC LIMIT 1";
        assert!(OBSERVATIONS_SQL.contains(shared));
        assert!(
            super::super::PENDING_OF_PATHS_SQL
                .contains("p.id < $5 AND p.hash IS NOT NULL ORDER BY p.id DESC LIMIT 1")
        );
    }
}
