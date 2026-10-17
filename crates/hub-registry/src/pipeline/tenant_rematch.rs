//! The tenant-suffix review (T4, D21, D84, A46): the tenants a swimlane deploys are a set that an admin confirms, and the files on NFS can
//! stop agreeing with it.
//!
//! A tenant file's name carries the tenant's branch as a suffix (`core-sit2.yml`). The engine reads a suffix against *every* deployable
//! branch, so a file of a branch the swimlane does not deploy is still a tenant file, and C4 (`orphan_file`) says so: "the name carries
//! the suffix of sit2, which this swimlane does not deploy". That is the sign that a swimlane has been deployed from another branch than
//! the one an admin confirmed (a sync from `sit2` after `sit1`, a swimlane that was reused). The hub does not change the set by itself;
//! it asks:
//!
//! 1. After each batch, for each swimlane that has been adopted, the suffixes of those C4 findings are looked at (`foreign_tenants`).
//!    If they are not the ones the last look saw, `Inner::rematch` runs. The last look is remembered in memory (so that a batch costs
//!    nothing when nothing changed) and in the newest review row of the swimlane (so that another replica, or a restart, does not raise
//!    the same review twice).
//! 2. The deployable branches are ranked against what is on NFS with the engine's `adoption_rank` (D22), on the registry's rayon pool,
//!    over the mirror's tree indexes (`crate::adoption::rank`, the ranking the adoption pass uses too). A template, the docs branch and
//!    any branch the registry's rule does not make deployable are never ranked ([`crate::branches`]). Both sides are compared by the raw
//!    SHA-256 the tree index holds: a swimlane whose files have other line endings than Git's scores lower on every branch alike, and
//!    the order, which is what an admin is shown, is what matters.
//! 3. An `adoptions` row with the reason `tenant_suffix_changed` and the status `review` holds the ranking and the suffixes. At most one
//!    such review is open for a swimlane (the table's unique index): a new suffix refreshes it while it waits, and an admin who has
//!    started on it (`in_progress`) is not disturbed. A cancelled or confirmed review is closed: the same suffixes do not raise it again,
//!    different ones do. The row is audited (one `SettingsChanged` event, rule 8) and carries ids only.
//!
//! Nothing here touches `swimlane_tenants`, baselines or any file: confirming is the adoption pass's (T5, `crate::adoption`), which works
//! these rows with the same steps as the first pass.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{MutexGuard, PoisonError};

use domain::{FindingKind, ShortText, SwimlaneId, TenantId, Timestamp};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};
use serde_json::{Value, json};
use tracing::warn;

use super::batch::BatchReport;
use super::findings_sync::{FindingKey, FindingRow};
use super::{Exec, Inner};
use crate::adoption::rank::{Ranked, Ranker};
use crate::db::ts;
use crate::error::RegistryError;
use crate::metrics::RematchResult;

/// The component named as the actor of the review's audit event.
const COMPONENT: &str = "registry-pipeline";
/// The most swimlanes whose last look is remembered in memory. Past it a look is made every batch, which costs a query and nothing else.
const MAX_REMEMBERED: usize = 10_000;
/// What to review for one swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub swimlane: SwimlaneId,
    /// The suffixes on NFS that the swimlane does not deploy.
    pub foreign: BTreeSet<TenantId>,
    /// The tenants the swimlane deploys, for the review's hints.
    pub tenants: Vec<String>,
}

/// The suffixes of the C4 findings that say "the name carries the suffix of a branch this swimlane does not deploy".
pub(crate) fn foreign_tenants(findings: &BTreeMap<FindingKey, FindingRow>) -> BTreeSet<TenantId> {
    findings
        .values()
        .filter(|row| {
            row.kind == FindingKind::OrphanFile
                && row.details.get("reason").and_then(Value::as_str) == Some("tenant_not_deployed")
        })
        .filter_map(|row| {
            row.details
                .get("tenant")
                .and_then(Value::as_str)
                .and_then(|t| TenantId::parse(t).ok())
        })
        .collect()
}

fn locked<T>(m: &std::sync::Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    /// The swimlanes of a batch whose foreign suffixes are not what the last look saw. Adopted swimlanes only: before the adoption pass
    /// has confirmed a tenant set there is none to change, and the pass is what asks.
    pub(crate) fn tenant_checks(&self, exec: &Exec, report: &BatchReport) -> Vec<Check> {
        let seen = locked(&self.rematched);
        report
            .lanes
            .iter()
            .filter_map(|lane| {
                let entry = exec.store.get(&lane.swimlane)?;
                if !entry.adopted {
                    return None;
                }
                let foreign = foreign_tenants(&entry.findings);
                if seen.get(&lane.swimlane).unwrap_or(&BTreeSet::new()) == &foreign {
                    return None;
                }
                Some(Check {
                    swimlane: lane.swimlane.clone(),
                    foreign,
                    tenants: entry.tenants.iter().map(|t| t.as_str().to_owned()).collect(),
                })
            })
            .collect()
    }

    /// Review each swimlane. A failure is logged and counted, and the swimlane is looked at again in the next batch: it is never the
    /// batch's failure.
    pub(crate) async fn review_tenants(&self, checks: Vec<Check>) {
        for check in checks {
            match self.rematch(&check).await {
                Ok(result) => {
                    self.metrics.rematch(result);
                    // A review left alone because an admin is working on it is looked at again, not remembered.
                    if result != RematchResult::Deferred {
                        self.remember_review(&check);
                    }
                }
                Err(error) => {
                    self.metrics.rematch(RematchResult::Failed);
                    warn!(swimlane = %check.swimlane, ?error, "the tenant-suffix review could not be made");
                }
            }
        }
    }

    fn remember_review(&self, check: &Check) {
        let mut seen = locked(&self.rematched);
        if check.foreign.is_empty() {
            seen.remove(&check.swimlane);
        } else if seen.len() < MAX_REMEMBERED || seen.contains_key(&check.swimlane) {
            seen.insert(check.swimlane.clone(), check.foreign.clone());
        }
    }

    /// Raise or refresh the review of a swimlane whose foreign suffixes are `check.foreign`.
    async fn rematch(&self, check: &Check) -> Result<RematchResult, RegistryError> {
        if check.foreign.is_empty() {
            // The suffixes are gone. A review that is open stays for the admin to close: it is a question, and nobody answered it.
            return Ok(RematchResult::Unchanged);
        }
        let wanted: Vec<String> = check.foreign.iter().map(|t| t.as_str().to_owned()).collect();

        // What the newest review was about, whatever became of it.
        let latest: Option<Option<Value>> = self
            .db
            .run(
                "rematch_latest",
                sqlx::query_scalar(
                    "SELECT hints -> 'foreign_suffixes' FROM adoptions \
                     WHERE swimlane_id = $1 AND reason = 'tenant_suffix_changed' ORDER BY id DESC LIMIT 1",
                )
                .bind(check.swimlane.as_str())
                .fetch_optional(self.db.pool()),
            )
            .await?;
        if let Some(Some(seen)) = latest {
            if seen == json!(wanted) {
                return Ok(RematchResult::Unchanged);
            }
        }

        let ranking = self.rank(&check.swimlane).await?;
        let leader = ranking
            .first()
            .and_then(|b| b.get("branch"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let hints = json!({"foreign_suffixes": wanted, "swimlane_tenants": check.tenants});

        let now_ms = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;
        // A review that waits is refreshed, one an admin is working on is not touched (the WHERE), and there is never a second open one.
        let written: Option<bool> = self
            .db
            .run(
                "rematch_write",
                sqlx::query_scalar(&format!(
                    "INSERT INTO adoptions (swimlane_id, reason, status, step, ranking, hints, started_at, updated_at) \
                     VALUES ($1, 'tenant_suffix_changed', 'review', 'branch', $2::jsonb, $3::jsonb, {now}, {now}) \
                     ON CONFLICT (swimlane_id, reason) WHERE status IN ('in_progress', 'review') DO UPDATE SET \
                       ranking = EXCLUDED.ranking, hints = EXCLUDED.hints, updated_at = EXCLUDED.updated_at \
                       WHERE adoptions.status = 'review' \
                     RETURNING (xmax = 0)",
                    now = ts("$4")
                ))
                .bind(check.swimlane.as_str())
                .bind(Value::Array(ranking).to_string())
                .bind(hints.to_string())
                .bind(now_ms)
                .fetch_optional(&mut *tx),
            )
            .await?;
        let Some(inserted) = written else {
            return Ok(RematchResult::Deferred);
        };
        let mut event = AuditEvent::new(
            Timestamp::from_unix_millis(now_ms),
            AuditActor::System {
                component: ShortText::parse(COMPONENT).map_err(|_| RegistryError::Invalid)?,
            },
            AuditAction::SettingsChanged,
            AuditVia::System,
        );
        event.swimlane = Some(check.swimlane.clone());
        event.diff = Some(format!(
            "tenant suffix review {}: suffixes on NFS that the swimlane does not deploy: {}; best match: {}",
            if inserted { "raised" } else { "refreshed" },
            wanted.join(", "),
            leader.as_deref().unwrap_or("none"),
        ));
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::record_all(&*self.audit, &mut handle, vec![event]).await?;
        if inserted {
            // A review is an open adoption: the baseline in the swimlane summary is `in_progress` from now on (CG4a, CG1b). A refreshed one
            // was open already, so nothing changed for a client.
            crate::batch::publish_swimlane_changed(&*self.bus, &mut handle, &check.swimlane).await?;
        }
        self.db.commit(tx).await?;
        Ok(if inserted {
            RematchResult::Raised
        } else {
            RematchResult::Refreshed
        })
    }

    /// The deployable branches ranked against the tenant files on the swimlane's NFS, best first, as the JSON the review stores. The ranking
    /// is the adoption pass's (`crate::adoption`): one implementation, so a review and a pass cannot rank differently.
    async fn rank(&self, swimlane: &SwimlaneId) -> Result<Vec<Value>, RegistryError> {
        let ranker = Ranker {
            db: self.db.clone(),
            git: std::sync::Arc::clone(&self.git),
            pool: std::sync::Arc::clone(&self.pool),
            timeout: self.config.git_timeout,
        };
        Ok(ranker
            .rank(swimlane)
            .await?
            .iter()
            .take(self.config.rematch_ranking)
            .map(Ranked::to_json)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use domain::Severity;

    use super::*;

    fn orphan(details: Value) -> FindingRow {
        FindingRow {
            kind: FindingKind::OrphanFile,
            path: Some("app/a-sit2.yml".to_owned()),
            severity: Severity::Low,
            rule_ids: Vec::new(),
            message: "m".to_owned(),
            locations: Value::Null,
            details,
            fingerprint: "f".to_owned(),
        }
    }

    fn rows(rows: Vec<FindingRow>) -> BTreeMap<FindingKey, FindingRow> {
        rows.into_iter()
            .enumerate()
            .map(|(i, r)| ((r.kind, format!("{i}")), r))
            .collect()
    }

    #[test]
    fn the_suffixes_are_those_of_the_orphans_that_name_a_branch_not_deployed() {
        let found = foreign_tenants(&rows(vec![
            orphan(json!({"check": "orphan_file", "reason": "tenant_not_deployed", "tenant": "sit2"})),
            orphan(json!({"check": "orphan_file", "reason": "tenant_not_deployed", "tenant": "sit3"})),
            orphan(json!({"check": "orphan_file", "reason": "tenant_not_deployed", "tenant": "sit2"})),
            // A file only NFS has is an orphan for another reason: it says nothing about tenants.
            orphan(json!({"check": "orphan_file", "reason": "untracked"})),
            // A finding of another kind is not an orphan.
            FindingRow {
                kind: FindingKind::Drift,
                ..orphan(json!({"reason": "tenant_not_deployed", "tenant": "sit9"}))
            },
            // What is not a tenant id is not a suffix.
            orphan(json!({"reason": "tenant_not_deployed", "tenant": "Not A Tenant"})),
        ]));
        let names: Vec<&str> = found.iter().map(TenantId::as_str).collect();
        assert_eq!(names, vec!["sit2", "sit3"], "sorted, without repeats");
        assert!(foreign_tenants(&BTreeMap::new()).is_empty());
    }
}
