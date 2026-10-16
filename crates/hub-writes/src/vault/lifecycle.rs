//! The vault's nightly job (T5): credentials of users who have not been seen for 30 days are deleted, and the ones that expire
//! within a week are counted and logged so that someone can warn their owners.
//!
//! Every replica builds the job; the holder of its lease runs it (D63). A credential is deleted in its own transaction with its
//! audit event (rule 8), and the delete re-checks the age, so a user who signed in since the scan keeps their token. The scan reads
//! the primary key in order, at most `expire_batch` rows a page and 100 pages a run.

use std::sync::Arc;

use domain::UserId;
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};

use super::{DAY_MS, EXPIRY_WARNING_DAYS, UNSEEN_DAYS, short, store};
use crate::error::{Internal, Res};
use crate::service::Inner;

/// The most pages one run reads.
const MAX_PAGES: usize = 100;

/// What one run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// Credentials deleted because their user was not seen for [`UNSEEN_DAYS`] days.
    pub deleted: usize,
    /// Credentials the run could not delete; the next run tries again.
    pub failed: usize,
    /// Stored tokens that expire within [`EXPIRY_WARNING_DAYS`] days (or have expired).
    pub expiring_soon: usize,
}

/// The leased job.
#[derive(Clone)]
pub struct CredentialCleanup {
    hub: Arc<Inner>,
}

impl std::fmt::Debug for CredentialCleanup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialCleanup").finish_non_exhaustive()
    }
}

impl CredentialCleanup {
    pub(crate) fn new(hub: Arc<Inner>) -> Self {
        Self { hub }
    }

    /// One run. `still_held` is asked before each unit of work; when it says no, the run stops (the lease is lost).
    pub async fn run_once(&self, still_held: &dyn Fn() -> bool) -> CleanupReport {
        let hub = &*self.hub;
        let mut report = CleanupReport::default();
        let batch = i64::try_from(hub.cfg.expire_batch).unwrap_or(100);
        let cutoff = hub.now_ms().saturating_sub(UNSEEN_DAYS * DAY_MS);

        let mut after = 0_i64;
        for _ in 0..MAX_PAGES {
            let page = async {
                let mut conn = hub
                    .pool
                    .acquire()
                    .await
                    .map_err(|e| Internal::db("cleanup", &e))?;
                store::stale(&mut conn, after, cutoff, batch).await
            };
            let Ok(Ok(page)) = hub.within("cleanup", page).await else {
                break;
            };
            let full = page.len() >= hub.cfg.expire_batch;
            let Some(last) = page.last().map(|s| s.id) else {
                break;
            };
            for stale in page {
                if !still_held() {
                    return report;
                }
                match hub
                    .within("cleanup", delete_one(hub, stale.id, &stale.user, cutoff))
                    .await
                {
                    Ok(Ok(true)) => report.deleted += 1,
                    // Seen since the scan, or deleted by its owner meanwhile: nothing to do.
                    Ok(Ok(false)) => {}
                    _ => report.failed += 1,
                }
            }
            after = last;
            if !full {
                break;
            }
        }

        if still_held() {
            report.expiring_soon = self.count_expiring(batch).await;
            if report.expiring_soon > 0 {
                tracing::warn!(
                    count = report.expiring_soon,
                    days = EXPIRY_WARNING_DAYS,
                    "stored github tokens expire soon"
                );
            }
        }
        report
    }

    async fn count_expiring(&self, batch: i64) -> usize {
        let hub = &*self.hub;
        let before = hub.now_ms().saturating_add(EXPIRY_WARNING_DAYS * DAY_MS);
        let mut after = 0_i64;
        let mut count = 0_usize;
        for _ in 0..MAX_PAGES {
            let page = async {
                let mut conn = hub
                    .pool
                    .acquire()
                    .await
                    .map_err(|e| Internal::db("cleanup", &e))?;
                store::expiring(&mut conn, after, before, batch).await
            };
            let Ok(Ok(ids)) = hub.within("cleanup", page).await else {
                break;
            };
            count += ids.len();
            match ids.last() {
                Some(last) if ids.len() >= hub.cfg.expire_batch => after = *last,
                _ => break,
            }
        }
        count
    }
}

/// Delete one credential if its user is still unseen, with its audit event. Returns whether it was deleted.
async fn delete_one(hub: &Inner, id: i64, user: &UserId, cutoff_ms: i64) -> Res<bool> {
    let mut tx = hub.pool.begin().await.map_err(|e| Internal::db("cleanup", &e))?;
    let Some(login) = store::delete_if_stale(&mut tx, id, cutoff_ms).await? else {
        let _ = tx.rollback().await;
        return Ok(false);
    };
    let component = short("credential-cleanup").ok_or_else(|| Internal::new("cleanup", "actor"))?;
    let mut event = AuditEvent::new(
        hub.clock.now(),
        AuditActor::System { component },
        AuditAction::GithubTokenDeleted,
        AuditVia::System,
    );
    event.github_login = short(&login);
    event.diff = Some(format!(
        "deleted the github token of user {user}, not seen for {UNSEEN_DAYS} days"
    ));
    hub.audit
        .record(&mut Tx::from_pg(&mut tx), event)
        .await
        .map_err(|_| Internal::new("cleanup", "audit log"))?;
    tx.commit().await.map_err(|e| Internal::db("cleanup", &e))?;
    Ok(true)
}
