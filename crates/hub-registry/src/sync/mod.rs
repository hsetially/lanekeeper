//! Syncs (T4, D19, D21, D75): what the hub does when it is told that a sync Job has put Git's content on NFS.
//!
//! [`SyncService::record_sync`] is the one entry for a report that a sync of a tenant branch finished, whatever reported it (D21: the
//! reporting option is still open, so the report says where it came from in [`SyncSource`] and nothing else depends on it):
//!
//! | Step | What | Where |
//! |---|---|---|
//! | 0 | The swimlane, the branch (a deployable one, [`crate::branches`]), the tenant (one the swimlane deploys) and both commits (in the mirror) are checked. A swimlane whose baseline nobody has confirmed yet only gets its deployment recorded: the first baseline is the adoption pass's (D22). | `mod` |
//! | 1 | The drift states that C7 needs are written down **first**: the files that are ahead on NFS or in conflict. | `overwrite` |
//! | 2 | A full scan is requested through the `AgentGateway`, and the swimlane is flagged so that the next heartbeat asks again if the request is lost. The hub waits (bounded) for the scan to be applied. | `mod` |
//! | 3 | **C7 first**: a file from step 1 that now equals Git at the reported commits was overwritten by the sync. Its observation gets the context and, when nobody better attributed it, the sync job as author (one `AttributionUpgraded` event each). One notification names the count. | `overwrite` |
//! | 4 | Baselines: a file whose NFS content equals Git **at the reported commits** (not at today's heads) gets that as its baseline, with a `blob_refs` row for the raw version behind it. One `BaselineAdopted` event per baseline that changes. | `record` |
//! | 5 | The deployment record of the tenant is replaced, with one `SettingsChanged` event. | `deployment` |
//! | 6 | The pipeline is told which paths to recompute. | `mod` |
//!
//! # Sync windows (T8)
//!
//! A sync Job's window (D75) is the other way a sync reaches the hub: the agent reports the Job as running and as finished, and the
//! leased job `registry-sync-watch` ([`window`]) does the same processing for every tenant of the swimlane when the window closes, with
//! **one** scan request, at the commits that are the heads of the mirrored branches (A33). While a window is open the drift is recorded and
//! the alerts are held ([`hold`]); when it closes they are evaluated once ([`alerts`]): one notification for each severity, C7 first.
//!
//! Everything that changes the database does it in chunks of at most `chunk_paths` paths, one transaction each, under the swimlane's
//! lock that ingestion's applies take too, with its audit events (one batch, last, so the chain's lock is held briefly) and its outbox
//! events in the same transaction (D80). Every step is idempotent: a report that is made again after a failure finds the work that
//! was done and does the rest.
//!
//! # Limits
//!
//! - **C7 from a report needs the files to be ahead when the report arrives.** The states of step 1 are read before the scan and are
//!   gone once the pipeline has recomputed them. A sync whose changes the agent had already sent, and the pipeline had already
//!   recomputed, before the report arrived leaves nothing for step 3 to find. A sync that the agent tagged with its Job, or that falls in
//!   a sync window, does not depend on this: ingestion writes the context when the version arrives (`overwrite`). The same loss
//!   follows a failure after the scan was applied and before step 3 committed: the report is made again, and finds the states gone.
//! - **The commits are not checked to be on the branch.** The mirror's port can say a commit exists, not that a branch contains it; the
//!   report is made by something the caller has authorised (S4), and a wrong commit gives baselines that the pipeline's convergence
//!   rule and the next report correct.
//! - **The wait is a poll.** The scan clears `needs_full_scan` in the transaction of its last chunk, and the report looks every
//!   `sync_poll_interval` until `sync_scan_timeout`; a caller that cannot hold a request that long makes the call from a task.

pub mod alerts;
pub(crate) mod deployment;
pub(crate) mod hold;
pub(crate) mod overwrite;
pub(crate) mod record;
pub mod window;

pub use alerts::{Alerts, AlertsDeps};
pub use window::{WATCH_JOB, WatchReport};

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use domain::{
    Actor, CommitId, FindingKind, HubCommand, NfsPath, RepoKind, Severity, ShortText, SwimlaneId, TenantId,
};
use engine::{ChangeContext, SeverityRules, default_rules, severity};
use ports::{
    AgentGateway, AuditActor, AuditLog, EventBus, GatewayError, GitError, GitReader, Notification,
    NotificationKind, Notifier,
};
use sqlx::PgPool;
use tracing::{info, warn};

use self::alerts::{Alerts as AlertSink, AlertsDeps as AlertSinkDeps};
use crate::branches::BranchClassifier;
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::error::RegistryError;
use crate::index::{GitIndex, GitIndexDeps};
use crate::ingest::rules::{LaneRules, load as load_rules};
use crate::metrics::{RegistryMetrics, SyncResult};
use crate::pipeline::{PipelineHandle, blocking};

/// The most paths a [`SyncOutcome`] lists. The count is exact; the list is for a person and a test.
pub const MAX_REPORTED_PATHS: usize = 1_000;

/// The component named as the actor of what a sync report decides, when the report was not made by a user.
const COMPONENT: &str = "registry-sync";

/// Where a sync report came from: `[a-z0-9_-]{1,64}` (for example `window`, `job-report`). Stored with the deployment record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSource(String);

impl SyncSource {
    pub const MAX_BYTES: usize = 64;

    /// # Errors
    ///
    /// `InvalidSource` for an empty or over-long name or one with a character other than `[a-z0-9_-]`.
    pub fn parse(s: &str) -> Result<Self, SyncError> {
        let ok = !s.is_empty()
            && s.len() <= Self::MAX_BYTES
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(SyncError::InvalidSource)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a sync report was not recorded. Carries no input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error("no such swimlane")]
    UnknownSwimlane,
    #[error("the source of the report is not valid")]
    InvalidSource,
    /// The branch is a template, the docs branch, or no rule makes it deployable.
    #[error("the branch is not deployable")]
    NotDeployable,
    /// The swimlane's tenant set does not hold the tenant. An admin confirms a changed set (the tenant-suffix review), the sync does not.
    #[error("the swimlane does not deploy this tenant")]
    TenantNotDeployed,
    #[error("the mirror has no such commit")]
    UnknownCommit,
    /// No stream is open for the swimlane's agent, so no scan could be asked for. Nothing was written. The swimlane stays flagged, so the
    /// agent's first heartbeat after it is back asks for the scan: report again then.
    #[error("the agent is not connected")]
    AgentNotConnected,
    /// The scan was asked for and not applied in time. The swimlane stays flagged, so the scan will come; nothing was written. Report again.
    #[error("the scan was not applied in time")]
    ScanTimeout,
    #[error("the registry failed")]
    Registry(RegistryError),
}

impl From<RegistryError> for SyncError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::NotFound => Self::UnknownSwimlane,
            RegistryError::Git(GitError::NotFound | GitError::InvalidRef) => Self::UnknownCommit,
            other => Self::Registry(other),
        }
    }
}

/// What a sync report did, in counts. Nothing in it is content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncOutcome {
    /// The swimlane's baseline was confirmed, so the sync was checked against it. `false`: only the deployment was recorded.
    pub adopted: bool,
    /// Baselines written because NFS equals Git at the reported commits and the baseline said otherwise.
    pub baselines_set: usize,
    /// Changes made only on NFS that the sync replaced (C7), the first [`MAX_REPORTED_PATHS`] of them.
    pub overwritten: Vec<NfsPath>,
    /// All of them.
    pub overwritten_total: usize,
    /// The notification about them was sent. `false` when there was nothing to say, and when the notifier failed (logged, never fatal).
    pub notified: bool,
}

/// What the registry needs for syncs.
pub struct SyncDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub git: Arc<dyn GitReader>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub gateway: Arc<dyn AgentGateway>,
    pub notifier: Arc<dyn Notifier>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// Told which paths changed. Without it the reconcile finds the change within `reconcile_interval`.
    pub pipeline: Option<PipelineHandle>,
}

impl std::fmt::Debug for SyncDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncDeps").finish_non_exhaustive()
    }
}

/// A report, as the steps read it.
#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub swimlane: SwimlaneId,
    pub tenant: TenantId,
    pub tenant_commit: CommitId,
    pub base_commit: CommitId,
    pub actor: Actor,
    pub source: SyncSource,
}

impl Report {
    /// Who the audit log names: the user who reported it, or the registry's sync component.
    pub(crate) fn audit_actor(&self) -> Result<AuditActor, RegistryError> {
        match &self.actor {
            Actor::User { user } => Ok(AuditActor::User { user: user.clone() }),
            Actor::OsLogin { .. } | Actor::Job { .. } => ShortText::parse(COMPONENT)
                .map(|component| AuditActor::System { component })
                .map_err(|_| RegistryError::Invalid),
        }
    }
}

/// What the report's swimlane is like.
pub(crate) struct Lane {
    /// An admin has confirmed the baseline (D2, D22).
    pub adopted: bool,
    pub rules: LaneRules,
}

pub(crate) struct Inner {
    pub db: Db,
    pub git: Arc<dyn GitReader>,
    pub git_index: GitIndex,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub gateway: Arc<dyn AgentGateway>,
    pub notifier: Arc<dyn Notifier>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    pub pipeline: Option<PipelineHandle>,
    pub severity: SeverityRules,
    /// The evaluation of a closed window's alerts, and the warning about a stale one (T8).
    pub alerts: AlertSink,
}

/// Syncs. Cloning shares it.
#[derive(Clone)]
pub struct SyncService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SyncService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncService").finish_non_exhaustive()
    }
}

impl SyncService {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: SyncDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let git_index = GitIndex::new(GitIndexDeps {
            pool: deps.pool.clone(),
            git: Arc::clone(&deps.git),
            config: deps.config.clone(),
            metrics: deps.metrics.clone(),
        })?;
        let severity =
            SeverityRules::new(default_rules()).map_err(|_| ConfigError::Invalid("severity_rules"))?;
        let alerts = AlertSink::new(AlertSinkDeps {
            pool: deps.pool.clone(),
            notifier: Arc::clone(&deps.notifier),
            clock: Arc::clone(&deps.clock),
            config: deps.config.clone(),
            metrics: deps.metrics.clone(),
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                git: deps.git,
                git_index,
                bus: deps.bus,
                audit: deps.audit,
                gateway: deps.gateway,
                notifier: deps.notifier,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
                pipeline: deps.pipeline,
                severity,
                alerts,
            }),
        })
    }

    /// A sync of `tenant_branch` finished: the tenant branch was at `tenant_commit` and the base repository's default branch at
    /// `base_commit` when the Job copied them to NFS. See the module's table for what the hub does with that.
    ///
    /// This is not a route and checks no role: whoever calls it (the job that handles a closed sync window, the endpoint that prompt 10
    /// designs) has authorised the report (S4). What it checks is what it can: the swimlane, the branch, the tenant and the commits.
    /// `actor` is who made the report and `source` where it came from; both are stored with the deployment and named in the audit events.
    ///
    /// # Errors
    ///
    /// [`SyncError`]. Nothing is written for the errors of step 0, for `AgentNotConnected` and for `ScanTimeout`; after that every chunk
    /// that committed stays, and a second report finishes the rest.
    pub async fn record_sync(
        &self,
        swimlane: &SwimlaneId,
        tenant_branch: &TenantId,
        tenant_commit: &CommitId,
        base_commit: &CommitId,
        actor: &Actor,
        source: &SyncSource,
    ) -> Result<SyncOutcome, SyncError> {
        let started = Instant::now();
        let report = Report {
            swimlane: swimlane.clone(),
            tenant: tenant_branch.clone(),
            tenant_commit: tenant_commit.clone(),
            base_commit: base_commit.clone(),
            actor: actor.clone(),
            source: source.clone(),
        };
        let result = self.inner.record(&report).await;
        let label = match &result {
            Ok(outcome) if outcome.adopted => SyncResult::Recorded,
            Ok(_) => SyncResult::DeploymentOnly,
            Err(
                SyncError::UnknownSwimlane
                | SyncError::InvalidSource
                | SyncError::NotDeployable
                | SyncError::TenantNotDeployed
                | SyncError::UnknownCommit,
            ) => SyncResult::Rejected,
            Err(SyncError::AgentNotConnected) => SyncResult::AgentNotConnected,
            Err(SyncError::ScanTimeout) => SyncResult::ScanTimeout,
            Err(SyncError::Registry(_)) => SyncResult::Failed,
        };
        self.inner.metrics.sync(label, started.elapsed().as_secs_f64());
        result
    }
}

impl Inner {
    async fn record(&self, r: &Report) -> Result<SyncOutcome, SyncError> {
        // The deadline is fixed before anything is asked for, so that a clock that moves while the scan is on its way counts.
        let deadline = self
            .clock
            .now()
            .unix_millis()
            .saturating_add(i64::try_from(self.config.sync_scan_timeout.as_millis()).unwrap_or(i64::MAX));

        let lane = self.lane(&r.swimlane).await?;
        if !BranchClassifier::load(&self.db)
            .await?
            .is_deployable(r.tenant.as_str())
        {
            return Err(SyncError::NotDeployable);
        }
        self.commits_exist(r).await?;

        if !lane.adopted {
            // Before the adoption pass confirms a baseline there is nothing to check a sync against, and the tenant set is not final.
            self.record_deployment(r).await?;
            return Ok(SyncOutcome::default());
        }
        if !lane.rules.tenants.contains(&r.tenant) {
            return Err(SyncError::TenantNotDeployed);
        }

        // 1. The states C7 needs, before anything can change them.
        let ahead = overwrite::snapshot(&self.db, &r.swimlane).await?;
        // 2. The scan.
        self.request_scan(r).await?;
        self.await_scan(r, deadline).await?;
        // 3. C7 first, and the notification that does not wait for the rest.
        let overwritten = self.claim(r, &lane.rules, ahead).await?;
        let notified = self.notify_overwritten(r, overwritten.len()).await;
        self.metrics.sync_overwritten(overwritten.len() as u64);
        // 4. The baselines, 5. the deployment.
        let baselined = self.set_baselines(r, &lane.rules).await?;
        self.metrics.sync_baselines(baselined.len() as u64);
        self.record_deployment(r).await?;
        // 6. The pipeline.
        if let Some(handle) = &self.pipeline {
            let changed: BTreeSet<NfsPath> = overwritten.iter().chain(&baselined).cloned().collect();
            handle.paths_changed(r.swimlane.clone(), changed.into_iter().collect());
        }
        info!(
            swimlane = %r.swimlane,
            tenant = %r.tenant,
            baselines = baselined.len(),
            overwritten = overwritten.len(),
            "sync recorded"
        );
        let overwritten_total = overwritten.len();
        Ok(SyncOutcome {
            adopted: true,
            baselines_set: baselined.len(),
            overwritten: overwritten.into_iter().take(MAX_REPORTED_PATHS).collect(),
            overwritten_total,
            notified,
        })
    }

    /// The swimlane's facts, or `UnknownSwimlane` when it has no registry row (an agent has never said hello for it).
    pub(crate) async fn lane(&self, swimlane: &SwimlaneId) -> Result<Lane, SyncError> {
        let adopted: Option<bool> = self
            .db
            .run(
                "sync_lane",
                sqlx::query_scalar(
                    "SELECT s.baseline_confirmed_at IS NOT NULL FROM swimlane_registry g \
                     JOIN swimlanes s ON s.id = g.swimlane_id WHERE g.swimlane_id = $1",
                )
                .bind(swimlane.as_str())
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let adopted = adopted.ok_or(SyncError::UnknownSwimlane)?;
        // The rules are read in a transaction that is not committed: it is only the way `rules::load` takes its reads.
        let mut tx = self.db.begin().await?;
        let rules = load_rules(&self.db, &mut tx, swimlane).await?;
        drop(tx);
        Ok(Lane { adopted, rules })
    }

    /// Both commits are in the mirror (`NotFound` is `UnknownCommit`).
    pub(crate) async fn commits_exist(&self, r: &Report) -> Result<(), SyncError> {
        let (git, tenant, base) = (
            Arc::clone(&self.git),
            r.tenant_commit.clone(),
            r.base_commit.clone(),
        );
        blocking(self.config.git_timeout, move || {
            git.tree_index(RepoKind::Tenant, &tenant)?;
            git.tree_index(RepoKind::Base, &base)?;
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Flag the swimlane (the next heartbeat asks for a scan if this request is lost) and ask the agent for one now.
    pub(crate) async fn request_scan(&self, r: &Report) -> Result<(), SyncError> {
        self.db
            .run(
                "sync_flag_scan",
                sqlx::query("UPDATE swimlane_registry SET needs_full_scan = true WHERE swimlane_id = $1")
                    .bind(r.swimlane.as_str())
                    .execute(self.db.pool()),
            )
            .await?;
        match self
            .gateway
            .request(
                &r.swimlane,
                HubCommand::RequestFullScan,
                self.config.sync_request_timeout,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(GatewayError::NotConnected) => Err(SyncError::AgentNotConnected),
            // The flag is set: the next heartbeat asks again, so the wait goes on.
            Err(e) => {
                warn!(swimlane = %r.swimlane, why = ?e, "the request for a scan did not reach the agent");
                Ok(())
            }
        }
    }

    /// Wait until the full scan has been applied (a full scan clears the flag in the transaction of its last chunk).
    pub(crate) async fn await_scan(&self, r: &Report, deadline_ms: i64) -> Result<(), SyncError> {
        loop {
            let pending: Option<bool> = self
                .db
                .run(
                    "sync_scan_pending",
                    sqlx::query_scalar(
                        "SELECT needs_full_scan FROM swimlane_registry WHERE swimlane_id = $1",
                    )
                    .bind(r.swimlane.as_str())
                    .fetch_optional(self.db.pool()),
                )
                .await?;
            match pending {
                None => return Err(SyncError::UnknownSwimlane),
                Some(false) => return Ok(()),
                Some(true) => {}
            }
            if self.clock.now().unix_millis() >= deadline_ms {
                return Err(SyncError::ScanTimeout);
            }
            tokio::time::sleep(self.config.sync_poll_interval).await;
        }
    }

    /// One notification for the changes the sync replaced. Never fails the report: a notifier that is down is logged.
    async fn notify_overwritten(&self, r: &Report, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        let Some(n) = self.overwritten_notification(r, count) else {
            return false;
        };
        // An external call: bounded (rule 5), and whatever it does the report goes on.
        match tokio::time::timeout(self.config.notify_timeout, self.notifier.notify(n)).await {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                warn!(swimlane = %r.swimlane, why = ?e, "the notification about overwritten changes was not sent");
                false
            }
            Err(_) => {
                warn!(swimlane = %r.swimlane, "the notification about overwritten changes timed out");
                false
            }
        }
    }

    /// Ids and counts only (S10): never a path, a hash or any content.
    fn overwritten_notification(&self, r: &Report, count: usize) -> Option<Notification> {
        let outcome = severity(
            &ChangeContext {
                finding_kind: Some(FindingKind::OverwrittenNfsChange),
                ..ChangeContext::default()
            },
            &self.severity,
        );
        let severity: Severity = outcome.severity;
        Some(Notification {
            kind: NotificationKind::CriticalFinding,
            severity,
            swimlane: Some(r.swimlane.clone()),
            title: ShortText::parse("A sync replaced changes that were only on NFS").ok()?,
            summary: ShortText::parse(&format!(
                "The sync of {} replaced {count} file(s) of {} that were ahead of Git. The replaced versions can be restored.",
                r.tenant, r.swimlane
            ))
            .ok()?,
            link: ShortText::parse(&format!("/swimlanes/{}", r.swimlane)).ok(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_a_short_lower_case_name() {
        for ok in ["window", "job-report", "a_b-9", &"x".repeat(64)] {
            assert_eq!(SyncSource::parse(ok).unwrap().as_str(), ok);
        }
        for bad in ["", "Window", "a b", "a/b", "a.b", &"x".repeat(65), "é", "a\n"] {
            assert_eq!(SyncSource::parse(bad), Err(SyncError::InvalidSource), "{bad:?}");
        }
    }

    #[test]
    fn a_registry_error_keeps_its_meaning_for_the_caller() {
        assert_eq!(
            SyncError::from(RegistryError::NotFound),
            SyncError::UnknownSwimlane
        );
        assert_eq!(
            SyncError::from(RegistryError::Git(GitError::NotFound)),
            SyncError::UnknownCommit
        );
        assert_eq!(
            SyncError::from(RegistryError::Git(GitError::InvalidRef)),
            SyncError::UnknownCommit
        );
        assert_eq!(
            SyncError::from(RegistryError::Git(GitError::Unavailable)),
            SyncError::Registry(RegistryError::Git(GitError::Unavailable))
        );
        assert_eq!(
            SyncError::from(RegistryError::Database),
            SyncError::Registry(RegistryError::Database)
        );
    }

    #[test]
    fn no_error_text_carries_input() {
        for e in [
            SyncError::UnknownSwimlane,
            SyncError::InvalidSource,
            SyncError::NotDeployable,
            SyncError::TenantNotDeployed,
            SyncError::UnknownCommit,
            SyncError::AgentNotConnected,
            SyncError::ScanTimeout,
            SyncError::Registry(RegistryError::Database),
        ] {
            assert!(e.to_string().len() < 80, "{e}");
        }
    }
}
