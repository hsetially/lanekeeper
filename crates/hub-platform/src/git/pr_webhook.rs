//! The `pull_request` webhook (T11, D77, S9, rule 8): GitHub tells the hub what became of a pull request, and the hub moves the
//! state of its `pr_links` rows to match.
//!
//! The delivery has already passed the HMAC and the replay guard in [`super::webhook`]; this module is what happens after the strict
//! parse. For one `(repository, number)` there can be several links (a pull request that changes several files has one per path),
//! and each is moved on its own by [`next_state`]:
//!
//! ```text
//!   open --closed(merged)--> merged            merged is terminal: nothing leaves it
//!   open --closed----------> closed
//!   closed --reopened------> open              unless another open link has the same (swimlane, path, hash): then it stays closed
//!   closed --closed(merged)> merged            a merge whose `reopened` was missed is still a merge
//! ```
//!
//! **One transaction.** The link rows are locked, each change is an `UPDATE`, one audit event is written for each link that
//! changed, and one `PrStateChanged` is published through `publish_in_tx` for it, all on the same transaction. If the audit log or
//! the bus refuses, the whole delivery rolls back, so there is never a state change without its audit event, nor an event for a
//! state that was not committed. Nothing is fanned out before the commit (D80).
//!
//! **The one-open rule.** `pr_links_one_open` allows one open link for a swimlane, path and content hash. Reopening a pull request
//! whose twin was raised in the meantime would break it, so the link is kept closed and the refusal is logged as a code. It is a
//! savepoint per link, because a unique violation aborts a `PostgreSQL` transaction and the other links of the same delivery must
//! still be applied.

use std::sync::Arc;
use std::time::Duration;

use domain::{DomainEvent, NfsPath, PrLinkId, PrState, RepoKind, ShortText, SwimlaneId};
use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, EventBus, Tx};
use sqlx::{Acquire, PgPool};
use tracing::{debug, warn};

use super::payload::{PrAction, PullRequestEvent};
use crate::clock::Clock;

/// The most links of one pull request that a delivery moves. A pull request with more changed paths is cut off, and the rest
/// keep their state until the next event (rule 5).
const MAX_LINKS: i64 = 500;
/// A whole delivery, from the first statement to the commit. A transaction that runs past it is dropped, which rolls it back.
const APPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// The `audit_events.actor_ref` of every change made here.
const COMPONENT: &str = "github-webhook";

/// The state a link moves to when `action` arrives while it is in `current`, or `None` if it stays where it is.
///
/// `merged` is the flag GitHub puts on a pull request; it only means something on `closed` and is ignored on every other action.
/// `opened` never changes anything: a link is created open, and acting on a late `opened` could undo a `closed` that was
/// delivered before it.
#[must_use]
pub fn next_state(current: PrState, action: PrAction, merged: bool) -> Option<PrState> {
    let target = match (action, merged) {
        (PrAction::Closed, true) => PrState::Merged,
        (PrAction::Closed, false) => PrState::Closed,
        (PrAction::Reopened, _) => PrState::Open,
        (PrAction::Opened | PrAction::Other, _) => return None,
    };
    match current {
        // Terminal: a merged pull request cannot be reopened or closed again.
        PrState::Merged => None,
        c if c == target => None,
        _ => Some(target),
    }
}

/// What a delivery came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrOutcome {
    /// An action that does not move a state (`edited`, `synchronize`, `opened`...): the database was not asked.
    Ignored,
    /// No link belongs to this pull request: nobody raised it through the hub.
    UnknownPr,
    /// The links were found and none had to move.
    Unchanged,
    /// `changed` links moved (and were audited and published); `blocked` reopens were refused by the one-open rule.
    Applied { changed: usize, blocked: usize },
}

/// The delivery could not be applied now; nothing was written. GitHub may redeliver it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrError {
    #[error("the pull request links are unavailable")]
    Unavailable,
}

/// The pull request links of the hub, updated from webhooks.
pub struct PrLinks {
    pool: PgPool,
    audit: Arc<dyn AuditLog>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PrLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrLinks").finish_non_exhaustive()
    }
}

/// What became of one link.
enum LinkResult {
    Unchanged,
    Changed,
    Blocked,
}

/// A failed database step: logged with a code and the step, and answered as unavailable. The cause stays in the log's code, never
/// in an answer.
fn unavailable(step: &'static str) -> impl FnOnce(sqlx::Error) -> PrError {
    move |_| {
        warn!(
            code = "pr_webhook_db_failed",
            step, "a pull request update failed"
        );
        PrError::Unavailable
    }
}

struct LinkRow {
    id: i64,
    swimlane_id: String,
    path: String,
    state: String,
}

impl PrLinks {
    #[must_use]
    pub fn new(
        pool: PgPool,
        audit: Arc<dyn AuditLog>,
        bus: Arc<dyn EventBus>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            pool,
            audit,
            bus,
            clock,
        }
    }

    /// Apply `event`, a delivery for a pull request of `repo`, in one transaction.
    pub async fn apply(&self, repo: RepoKind, event: &PullRequestEvent) -> Result<PrOutcome, PrError> {
        if matches!(event.action, PrAction::Opened | PrAction::Other) {
            return Ok(PrOutcome::Ignored);
        }
        match tokio::time::timeout(APPLY_TIMEOUT, self.apply_inner(repo, event)).await {
            Ok(result) => result,
            Err(_elapsed) => {
                warn!(
                    code = "pr_webhook_timeout",
                    "applying a pull request event took too long"
                );
                Err(PrError::Unavailable)
            }
        }
    }

    async fn apply_inner(&self, repo: RepoKind, event: &PullRequestEvent) -> Result<PrOutcome, PrError> {
        let mut tx = self.pool.begin().await.map_err(unavailable("begin"))?;
        // `FOR UPDATE`, so two deliveries for one pull request (a close and a reopen in quick succession) are applied one after the
        // other and the second sees the first's state.
        let links: Vec<LinkRow> = sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT id, swimlane_id, path, state FROM pr_links WHERE repo = $1 AND pr_number = $2 \
             ORDER BY id LIMIT $3 FOR UPDATE",
        )
        .bind(repo.as_str())
        .bind(i32::try_from(event.number).map_err(|_| PrError::Unavailable)?)
        .bind(MAX_LINKS)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable("select"))?
        .into_iter()
        .map(|(id, swimlane_id, path, state)| LinkRow {
            id,
            swimlane_id,
            path,
            state,
        })
        .collect();
        if links.is_empty() {
            return Ok(PrOutcome::UnknownPr);
        }

        let (mut changed, mut blocked) = (0_usize, 0_usize);
        for link in &links {
            match self.apply_link(&mut tx, link, event).await? {
                LinkResult::Unchanged => {}
                LinkResult::Changed => changed += 1,
                LinkResult::Blocked => blocked += 1,
            }
        }
        tx.commit().await.map_err(unavailable("commit"))?;
        debug!(changed, blocked, "a pull request event was applied");
        Ok(if changed == 0 && blocked == 0 {
            PrOutcome::Unchanged
        } else {
            PrOutcome::Applied { changed, blocked }
        })
    }

    /// One link: the update (in a savepoint, so that a unique violation costs only this link), its audit event and its event.
    async fn apply_link(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        link: &LinkRow,
        event: &PullRequestEvent,
    ) -> Result<LinkResult, PrError> {
        let Some(current) = parse_state(&link.state) else {
            // The column has a CHECK, so this is a schema this build does not know. Leave the row alone.
            warn!(
                code = "pr_link_state_unknown",
                link = link.id,
                "a link has a state this build does not know"
            );
            return Ok(LinkResult::Unchanged);
        };
        let Some(next) = next_state(current, event.action, event.merged) else {
            return Ok(LinkResult::Unchanged);
        };
        let mut savepoint = tx.begin().await.map_err(unavailable("savepoint"))?;
        let update = sqlx::query("UPDATE pr_links SET state = $2, updated_at = now() WHERE id = $1")
            .bind(link.id)
            .bind(next.as_str())
            .execute(&mut *savepoint)
            .await;
        match update {
            Ok(_) => savepoint.commit().await.map_err(unavailable("release"))?,
            Err(error) if is_one_open_violation(&error) => {
                savepoint.rollback().await.map_err(unavailable("rollback"))?;
                warn!(
                    code = "pr_reopen_blocked_by_open_link",
                    link = link.id,
                    "a reopened pull request was kept closed: another open link has the same swimlane, path and content"
                );
                return Ok(LinkResult::Blocked);
            }
            Err(error) => return Err(unavailable("update")(error)),
        }
        self.audit
            .record(&mut Tx::from_pg(tx), self.audit_event(link, current, next)?)
            .await
            .map_err(|_| {
                warn!(
                    code = "pr_webhook_audit_failed",
                    link = link.id,
                    "the audit event could not be written"
                );
                PrError::Unavailable
            })?;
        self.bus
            .publish_in_tx(
                &mut Tx::from_pg(tx),
                DomainEvent::PrStateChanged {
                    link_id: PrLinkId::from(link.id),
                    state: next,
                },
            )
            .await
            .map_err(|_| {
                warn!(
                    code = "pr_webhook_publish_failed",
                    link = link.id,
                    "the event could not be written to the outbox"
                );
                PrError::Unavailable
            })?;
        Ok(LinkResult::Changed)
    }

    /// The audit event of one link's change. It names the swimlane and path, and says in `diff` what the state was and became;
    /// nothing from the payload is copied into it.
    fn audit_event(&self, link: &LinkRow, from: PrState, to: PrState) -> Result<AuditEvent, PrError> {
        // `COMPONENT` is a fixed text that `ShortText` accepts; the error is for the type's sake.
        let component = ShortText::parse(COMPONENT).map_err(|_| PrError::Unavailable)?;
        let mut event = AuditEvent::new(
            self.clock.now(),
            AuditActor::System { component },
            AuditAction::PrStateChanged,
            AuditVia::System,
        );
        event.swimlane = SwimlaneId::parse(&link.swimlane_id).ok();
        event.path = NfsPath::parse(&link.path).ok();
        event.diff = Some(format!("pr_links.state: {} -> {}\n", from.as_str(), to.as_str()));
        Ok(event)
    }
}

fn parse_state(text: &str) -> Option<PrState> {
    match text {
        "open" => Some(PrState::Open),
        "merged" => Some(PrState::Merged),
        "closed" => Some(PrState::Closed),
        _ => None,
    }
}

fn is_one_open_violation(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|db| {
        db.code().as_deref() == Some("23505") && db.constraint() == Some("pr_links_one_open")
    })
}
