//! The expiry job (D37, D63): a proposal that nobody decided in 72 hours is marked `expired`, and so is a draft that nobody applied
//! in a day.
//!
//! Every replica builds the job; the holder of its lease runs it. A proposal moves in its own transaction, which locks the row
//! first, so a decision made at the same moment either wins (the proposal is no longer pending when the job looks) or loses (it
//! finds `expired`), and the job's audit event, status change and event commit together (rule 8, D80). The scan reads the partial
//! index on `expires_at`, oldest first, at most `expire_batch` at a time.
//!
//! For each proposal the job also takes the lease that deciders take (`proposal:{id}`), and skips a proposal whose lease is held:
//! an approval is at the agent, and the approval is what moves the proposal. Without it the job could mark `expired` a proposal
//! whose file the approval was writing. The skipped proposal is looked at again by the next run, which finds it decided (or, when
//! the approval gave up, expires it).

use std::sync::Arc;

use domain::{DomainEvent, NfsPath, ProposalStatus, ShortText};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};

use super::approve::lease_name;
use super::store;
use crate::drafts;
use crate::error::{Internal, Res};
use crate::service::Inner;

/// The most batches one run works through: with the default batch, 10,000 proposals.
const MAX_BATCHES: usize = 100;

/// How long the job's lease on one proposal is held: the time one transaction takes, with room for a slow database.
const LEASE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// What one run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpireReport {
    /// Proposals marked expired.
    pub proposals: usize,
    /// Drafts marked expired.
    pub drafts: u64,
    /// Proposals the run could not mark (the store did not answer); the next run tries again.
    pub failed: usize,
    /// Proposals the run left alone because a decision on them was under way (their lease was held); the next run looks again.
    pub busy: usize,
}

/// The leased job.
#[derive(Clone)]
pub struct ExpireProposals {
    hub: Arc<Inner>,
}

impl std::fmt::Debug for ExpireProposals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExpireProposals").finish_non_exhaustive()
    }
}

impl ExpireProposals {
    pub(crate) fn new(hub: Arc<Inner>) -> Self {
        Self { hub }
    }

    /// One run. `still_held` is asked before each unit of work; when it says no, the run stops (the lease is lost).
    pub async fn run_once(&self, still_held: &dyn Fn() -> bool) -> ExpireReport {
        let hub = &*self.hub;
        let mut report = ExpireReport::default();
        let batch = i64::try_from(hub.cfg.expire_batch).unwrap_or(100);

        'batches: for _ in 0..MAX_BATCHES {
            let ids = async {
                let mut conn = hub.pool.acquire().await.map_err(|e| Internal::db("expire", &e))?;
                store::expired(&mut conn, hub.now_ms(), batch).await
            };
            let Ok(Ok(ids)) = hub.within("expire", ids).await else {
                break;
            };
            let full = ids.len() >= hub.cfg.expire_batch;
            let mut progressed = false;
            for id in ids {
                if !still_held() {
                    return report;
                }
                match hub.within("expire", expire_leased(hub, id)).await {
                    Ok(Ok(Expiry::Moved)) => {
                        report.proposals += 1;
                        progressed = true;
                    }
                    // Decided in the meantime: nothing to do, and it will not be listed again.
                    Ok(Ok(Expiry::Gone)) => progressed = true,
                    // Being decided: it is listed again until the decision is made, so it must not count as progress.
                    Ok(Ok(Expiry::Busy)) => report.busy += 1,
                    _ => report.failed += 1,
                }
            }
            // A batch that moved nothing would be read again for ever.
            if !full || !progressed {
                break 'batches;
            }
        }

        if still_held() {
            let swept = async {
                let mut conn = hub.pool.acquire().await.map_err(|e| Internal::db("expire", &e))?;
                drafts::expire(&mut conn, hub.now_ms(), batch).await
            };
            if let Ok(Ok(n)) = hub.within("expire_drafts", swept).await {
                report.drafts = n;
            }
        }
        report
    }
}

/// What the job did with one proposal.
enum Expiry {
    Moved,
    /// Not pending any more, or not yet due.
    Gone,
    /// A decision on it is under way.
    Busy,
}

/// [`expire_one`] with the lease on the proposal held, so that it does not run beside a decision.
async fn expire_leased(hub: &Inner, id: domain::ProposalId) -> Res<Expiry> {
    let lease = match hub.leases.try_acquire(&lease_name(id), LEASE_TTL).await {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(Expiry::Busy),
        Err(_) => return Err(Internal::new("expire", "lease")),
    };
    let moved = expire_one(hub, id).await;
    if lease.release().await.is_err() {
        tracing::warn!("a proposal lease could not be released; it runs out by itself");
    }
    Ok(if moved? { Expiry::Moved } else { Expiry::Gone })
}

/// Mark one proposal expired, if it is still pending and its time has run out. Returns whether it moved.
async fn expire_one(hub: &Inner, id: domain::ProposalId) -> Res<bool> {
    let mut tx = hub.pool.begin().await.map_err(|e| Internal::db("expire", &e))?;
    if store::lock_status(&mut tx, id).await? != Some(ProposalStatus::Pending) {
        let _ = tx.rollback().await;
        return Ok(false);
    }
    let Some(p) = store::get(&mut tx, id).await? else {
        let _ = tx.rollback().await;
        return Ok(false);
    };
    let now = hub.now_ms();
    if p.expires_ms > now {
        let _ = tx.rollback().await;
        return Ok(false);
    }
    store::set_status(&mut tx, id, ProposalStatus::Expired, now).await?;
    if let Some(draft) = p.draft() {
        drafts::mark(&mut tx, &draft, ProposalStatus::Expired).await?;
    }
    hub.bus
        .publish_in_tx(
            &mut Tx::from_pg(&mut tx),
            DomainEvent::ProposalChanged {
                id,
                status: ProposalStatus::Expired,
            },
        )
        .await
        .map_err(|_| Internal::new("expire", "event outbox"))?;

    let component = ShortText::parse("expire-proposals").map_err(|_| Internal::new("expire", "actor"))?;
    let mut e = AuditEvent::new(
        hub.clock.now(),
        AuditActor::System { component },
        AuditAction::ProposalExpired,
        AuditVia::System,
    );
    e.swimlane = Some(p.swimlane.clone());
    e.path = p.paths.first().and_then(|t| NfsPath::parse(t).ok());
    e.author = Some(p.author.clone());
    e.approval = Some(id);
    hub.audit
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .map_err(|_| Internal::new("expire", "audit log"))?;
    tx.commit().await.map_err(|e| Internal::db("expire", &e))?;
    Ok(true)
}
