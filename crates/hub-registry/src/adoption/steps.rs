//! The early steps of the pass (T5): opening it (or resuming the open one), choosing the branches, choosing the base version, and closing
//! a pass without confirming it. The files, the secret scan and the confirmation are in their own modules.
//!
//! Every write is one transaction with the audit event of its state change where it has one (rule 8, D93): a pass is audited when it is
//! opened by an admin and when it is cancelled; the choices between are the wizard's working data and carry who and when.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use domain::{CommitId, RepoKind, ReverseMapping, SwimlaneId, TenantId, TenantSet, Timestamp, User};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};
use serde_json::{Value, json};

use super::base_commit::{Bounds, best_base_commit};
use super::hints::merge_into;
use super::rank::Ranker;
use super::resolutions::settled_step;
use super::store::{AdoptionReason, AdoptionStatus, AdoptionStep, Row, ranking_json};
use super::{AdoptionError, AdoptionView, Inner};
use crate::branches::BranchClassifier;
use crate::db::ts;
use crate::error::RegistryError;
use crate::pipeline::blocking;

/// The most branches one pass can choose (the column's bound).
const MAX_TENANTS: usize = 64;

impl Inner {
    pub(crate) fn ranker(&self) -> Ranker {
        Ranker {
            db: self.db.clone(),
            git: Arc::clone(&self.git),
            pool: Arc::clone(&self.pool),
            timeout: self.config.git_timeout,
        }
    }

    /// The tenant ids of every deployable branch of the mirror.
    async fn deployable_tenants(&self) -> Result<BTreeSet<TenantId>, RegistryError> {
        let classifier = BranchClassifier::load(&self.db).await?;
        let git = Arc::clone(&self.git);
        blocking(self.config.git_timeout, move || {
            Ok(git
                .branches(RepoKind::Tenant)?
                .into_iter()
                .filter_map(|b| classifier.tenant_of(b.name.as_str()))
                .collect())
        })
        .await
    }

    pub(super) fn actor(u: &User) -> AuditActor {
        AuditActor::User { user: u.id.clone() }
    }

    /// An event of the pass's own lifecycle (`SettingsChanged`: the contract has no better action, and a later `AuditAction` is a ports
    /// change).
    pub(super) fn lifecycle_event(&self, u: &User, swimlane: &SwimlaneId, text: String) -> AuditEvent {
        let mut event = AuditEvent::new(
            Timestamp::from_unix_millis(self.clock.now().unix_millis()),
            Self::actor(u),
            AuditAction::SettingsChanged,
            AuditVia::Ui,
        );
        event.swimlane = Some(swimlane.clone());
        event.diff = Some(text);
        event
    }

    /// Open the pass of `swimlane` for `reason`, or resume the open one.
    pub(crate) async fn start(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        reason: AdoptionReason,
    ) -> Result<AdoptionView, AdoptionError> {
        let lane = self.lane(swimlane).await?;
        match reason {
            AdoptionReason::Initial if lane.adopted => return Err(AdoptionError::AlreadyAdopted),
            AdoptionReason::TenantSuffixChanged if !lane.adopted => return Err(AdoptionError::NotAdopted),
            _ => {}
        }
        if let Some(row) = self.open_of(swimlane, reason).await? {
            return self.resume(u, row).await;
        }
        if reason == AdoptionReason::TenantSuffixChanged {
            // The pipeline raises the review; there is nothing to start.
            return Err(AdoptionError::NotOpen);
        }
        if !lane.inventory {
            return Err(AdoptionError::NoInventory);
        }
        self.open_initial(u, swimlane).await
    }

    /// A new first pass: rank, collect the hints, write the row.
    async fn open_initial(&self, u: &User, swimlane: &SwimlaneId) -> Result<AdoptionView, AdoptionError> {
        let started = Instant::now();
        let ranked = self.ranker().rank(swimlane).await?;
        self.metrics.adoption_ranked(started.elapsed().as_secs_f64());
        let candidates: BTreeSet<TenantId> = ranked.iter().map(|r| r.tenant.clone()).collect();
        let hints = merge_into(&json!({}), &self.hints(swimlane, &candidates).await?);
        let kept = ranking_json(&ranked[..ranked.len().min(self.config.rematch_ranking)]);

        let now = self.clock.now().unix_millis();
        let (tid, oid) = Self::user_ids(u);
        let sql = format!(
            "INSERT INTO adoptions (swimlane_id, reason, status, step, ranking, hints, started_by_tid, started_by_oid, started_at, updated_at) \
             VALUES ($1, 'initial', 'in_progress', 'branch', $2::jsonb, $3::jsonb, $4, $5, {now}, {now}) \
             ON CONFLICT (swimlane_id, reason) WHERE status IN ('in_progress', 'review') DO NOTHING \
             RETURNING id",
            now = ts("$6")
        );
        let mut tx = self.db.begin().await?;
        let id: Option<i64> = self
            .db
            .run(
                "adoption_open_initial",
                sqlx::query_scalar(&sql)
                    .bind(swimlane.as_str())
                    .bind(kept.to_string())
                    .bind(hints.to_string())
                    .bind(&tid)
                    .bind(&oid)
                    .bind(now)
                    .fetch_optional(&mut *tx),
            )
            .await?;
        let Some(id) = id else {
            // Another admin opened it between the look and the write: theirs is the pass, and this one resumes it.
            drop(tx);
            let row = self
                .open_of(swimlane, AdoptionReason::Initial)
                .await?
                .ok_or(AdoptionError::NotOpen)?;
            return self.resume(u, row).await;
        };
        let event = self.lifecycle_event(u, swimlane, format!("adoption pass {id} opened (initial)"));
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::record_all(&*self.audit, &mut handle, vec![event]).await?;
        self.db.commit(tx).await?;
        self.view(id).await
    }

    /// An open pass is returned as it is stored; a review that the registry raised is taken by the admin who starts on it, which is what
    /// keeps the registry from refreshing it under them.
    async fn resume(&self, u: &User, row: Row) -> Result<AdoptionView, AdoptionError> {
        if row.status != AdoptionStatus::Review {
            return self.view(row.id).await;
        }
        let candidates = self.deployable_tenants().await?;
        let hints = merge_into(&row.hints, &self.hints(&row.swimlane, &candidates).await?);
        let now = self.clock.now().unix_millis();
        let (tid, oid) = Self::user_ids(u);
        let sql = format!(
            "UPDATE adoptions SET status = 'in_progress', hints = $2::jsonb, \
             started_by_tid = COALESCE(started_by_tid, $3), started_by_oid = COALESCE(started_by_oid, $4), updated_at = {} \
             WHERE id = $1 AND status = 'review'",
            ts("$5")
        );
        self.db
            .run(
                "adoption_take_review",
                sqlx::query(&sql)
                    .bind(row.id)
                    .bind(hints.to_string())
                    .bind(&tid)
                    .bind(&oid)
                    .bind(now)
                    .execute(self.db.pool()),
            )
            .await?;
        // Taken by this call or by another admin's a moment before: either way it is in progress now, or closed.
        self.working_row(row.id).await?;
        self.view(row.id).await
    }

    /// Choose the branches the swimlane was deployed from: each must be a deployable branch of the mirror, its head is fixed as the commit
    /// the baselines are taken from, and the best-matching base commit is proposed.
    pub(crate) async fn choose_branches(
        &self,
        id: i64,
        tenants: &[TenantId],
    ) -> Result<AdoptionView, AdoptionError> {
        let row = self.working_row(id).await?;
        let chosen: BTreeSet<&TenantId> = tenants.iter().collect();
        if chosen.is_empty() || chosen.len() > MAX_TENANTS {
            return Err(AdoptionError::Read(ports::ReadError::BadRequest));
        }
        let classifier = BranchClassifier::load(&self.db).await?;
        let rule = self.mapping_rule().await?;
        let files = self.nfs_files(&row.swimlane).await?;
        let set = TenantSet::new(chosen.iter().map(|t| (*t).clone())).map_err(|_| RegistryError::Capacity)?;
        let wanted: Vec<TenantId> = chosen.into_iter().cloned().collect();
        let bounds = Bounds {
            candidates: self.config.adoption_base_candidates,
            history_paths: self.config.adoption_history_paths,
            history_depth: self.config.adoption_history_depth,
        };
        let git = Arc::clone(&self.git);
        let (heads, found) = blocking(
            self.config.git_timeout.max(std::time::Duration::from_secs(60)),
            move || {
                // Each chosen tenant is a deployable branch of the mirror; its head is the commit the baselines are taken from.
                let branches = git.branches(RepoKind::Tenant)?;
                let mut heads: Vec<(TenantId, CommitId)> = Vec::with_capacity(wanted.len());
                for tenant in &wanted {
                    let head = branches
                        .iter()
                        .find(|b| {
                            b.name.as_str() == tenant.as_str()
                                && classifier.tenant_of(b.name.as_str()).as_ref() == Some(tenant)
                        })
                        .map(|b| b.head.clone());
                    match head {
                        Some(head) => heads.push((tenant.clone(), head)),
                        None => return Err(RegistryError::NotFound),
                    }
                }
                // The swimlane's base files (no tenant suffix of the chosen set) as repository paths and raw hashes.
                let ours: Vec<(domain::RepoPath, domain::ContentHash)> = files
                    .iter()
                    .filter_map(|f| match rule.reverse(&f.path, &set) {
                        Ok(ReverseMapping::Base { repo_path, .. }) => Some((repo_path, f.raw)),
                        _ => None,
                    })
                    .collect();
                let found = best_base_commit(&*git, &rule, &ours, bounds)?;
                Ok((heads, found))
            },
        )
        .await
        .map_err(|e| match e {
            // A branch that is not in the mirror, or not deployable: the caller named it.
            RegistryError::NotFound => AdoptionError::NotDeployable,
            other => AdoptionError::from(other),
        })?;

        let commits: serde_json::Map<String, Value> = heads
            .iter()
            .map(|(t, c)| (t.as_str().to_owned(), Value::String(c.as_str().to_owned())))
            .collect();
        let names: Vec<String> = heads.iter().map(|(t, _)| t.as_str().to_owned()).collect();
        let label = serde_json::to_value(&found.label).map_err(|_| RegistryError::Corrupt)?;
        let now = self.clock.now().unix_millis();
        let sql = format!(
            "UPDATE adoptions SET tenants = $2::text[], tenant_commits = $3::jsonb, base_commit = $4, base_label = $5::jsonb, \
             secret_scan = '[]'::jsonb, step = 'base_version', updated_at = {} WHERE id = $1 AND status = 'in_progress'",
            ts("$6")
        );
        let mut tx = self.db.begin().await?;
        let done = self
            .db
            .run(
                "adoption_choose_branches",
                sqlx::query(&sql)
                    .bind(row.id)
                    .bind(&names)
                    .bind(Value::Object(commits).to_string())
                    .bind(found.commit.as_str())
                    .bind(label.to_string())
                    .bind(now)
                    .execute(&mut *tx),
            )
            .await?;
        if done.rows_affected() == 0 {
            return Err(AdoptionError::NotOpen);
        }
        // What the admin looked at was made against other commits.
        self.clear_resolutions(&mut tx, row.id).await?;
        self.db.commit(tx).await?;
        self.view(row.id).await
    }

    /// Accept the proposed base commit, or name another.
    pub(crate) async fn choose_base_commit(
        &self,
        id: i64,
        commit: Option<&CommitId>,
    ) -> Result<AdoptionView, AdoptionError> {
        let row = self.working_row(id).await?;
        if row.step < AdoptionStep::BaseVersion {
            return Err(AdoptionError::WrongStep);
        }
        let stored = row.base()?;
        let chosen = match commit {
            Some(c) => c.clone(),
            None => stored.clone().ok_or(AdoptionError::WrongStep)?,
        };
        let git = Arc::clone(&self.git);
        let wanted = chosen.clone();
        let label = blocking(self.config.git_timeout, move || {
            git.tree_index(RepoKind::Base, &wanted)?;
            Ok(git.describe(RepoKind::Base, &wanted)?)
        })
        .await?;
        let label = serde_json::to_value(&label).map_err(|_| RegistryError::Corrupt)?;
        let now = self.clock.now().unix_millis();
        let sql = format!(
            "UPDATE adoptions SET base_commit = $2, base_label = $3::jsonb, secret_scan = '[]'::jsonb, step = 'files', updated_at = {} \
             WHERE id = $1 AND status = 'in_progress'",
            ts("$4")
        );
        let mut tx = self.db.begin().await?;
        let done = self
            .db
            .run(
                "adoption_choose_base",
                sqlx::query(&sql)
                    .bind(row.id)
                    .bind(chosen.as_str())
                    .bind(label.to_string())
                    .bind(now)
                    .execute(&mut *tx),
            )
            .await?;
        if done.rows_affected() == 0 {
            return Err(AdoptionError::NotOpen);
        }
        if stored.as_ref() != Some(&chosen) {
            self.clear_resolutions(&mut tx, row.id).await?;
        }
        self.db.commit(tx).await?;

        // With nothing to resolve the pass goes straight on to the secret scan.
        let row = self.working_row(id).await?;
        let plan = self.plan(&row).await?;
        let stored = self.stored_resolutions(id).await?;
        let step = settled_step(AdoptionStep::Files, Self::unresolved(&plan, &stored));
        if step != AdoptionStep::Files {
            self.set_step(id, step).await?;
        }
        self.view(id).await
    }

    /// Close an open pass.
    pub(crate) async fn cancel(&self, u: &User, id: i64) -> Result<(), AdoptionError> {
        let now = self.clock.now().unix_millis();
        let sql = format!(
            "UPDATE adoptions SET status = 'cancelled', updated_at = {} WHERE id = $1 AND status IN ('in_progress', 'review') \
             RETURNING swimlane_id, reason",
            ts("$2")
        );
        let mut tx = self.db.begin().await?;
        let closed: Option<(String, String)> = self
            .db
            .run(
                "adoption_cancel",
                sqlx::query_as(&sql).bind(id).bind(now).fetch_optional(&mut *tx),
            )
            .await?;
        let Some((swimlane, reason)) = closed else {
            return Err(AdoptionError::NotOpen);
        };
        let swimlane = SwimlaneId::parse(&swimlane).map_err(|_| RegistryError::Corrupt)?;
        let reason = AdoptionReason::parse(&reason)?;
        let event = self.lifecycle_event(
            u,
            &swimlane,
            format!("adoption pass {id} cancelled ({})", reason.as_str()),
        );
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::record_all(&*self.audit, &mut handle, vec![event]).await?;
        self.db.commit(tx).await?;
        Ok(())
    }
}
