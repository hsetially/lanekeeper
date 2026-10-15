//! The crash window of a write (plan A1), and the sweep of expired idempotency rows.
//!
//! An NFS write cannot be part of a database transaction, and holding a connection across the agent's answer is forbidden by the
//! performance rules. So a write can land on NFS and the hub fail before it records it: the process dies, the database is gone for
//! a moment, the reply is lost. The claim row then stays unfinished and carries the *intent*: what was sent, and the audit event it
//! will record. Whoever finds it asks the agent for the file:
//!
//! - the file has the hash that was sent: the write landed, and it is recorded now, as it would have been;
//! - the file is gone or has another hash: it did not land (or something else changed it since); the claim is cleared and the
//!   key is free.
//!
//! "Whoever finds it" is the request that retries with the same key (at once, if the first request has said it gave up, and after
//! `stale_claim_after` otherwise), and the [`ReconcileWrites`] job, which looks at claims older than that. Both end in the same
//! transaction, which locks the claim first, so a write is recorded once however many of them race.
//!
//! What remains: a write that landed and was overwritten by somebody else before anybody looked is not recorded as ours (the
//! registry still observed that version, D47). It is listed as a residual risk in the threat model.

use std::sync::Arc;

use domain::{AgentReply, OpError, RequestId, WriteOutcome};

use crate::idempotency::{ClaimKey, Intent, IntentOp};
use crate::pipeline::execute::{Committed, commit_applied, store_blob};
use crate::service::{Inner, millis};

/// What looking at one unfinished claim came to.
#[derive(Debug)]
pub(crate) enum Resolved {
    /// The write had landed; it is recorded. This is its outcome.
    Completed(WriteOutcome),
    /// The write did not land (or nothing was sent): the claim is gone.
    Cleared,
    /// The agent could not be asked, or the store could not be written: nothing changed, ask again later.
    Unreachable,
    /// Somebody else finished the claim while this was looking.
    Raced,
}

/// Look at an unfinished claim and settle it. `body` is what the row holds: the intent, or nothing.
pub(crate) async fn resolve(hub: &Inner, key: &ClaimKey, body: Option<&[u8]>) -> Resolved {
    let Some(intent) = body.and_then(|b| serde_json::from_slice::<Intent>(b).ok()) else {
        // No intent: the request died before it sent anything.
        return clear(hub, key).await;
    };
    let Ok(request_id) = RequestId::parse(&format!("recon-{:016x}", fastrand::u64(..))) else {
        return Resolved::Unreachable;
    };
    let cmd = domain::HubCommand::ReadFile {
        request_id,
        path: intent.path.clone(),
    };
    let asked = tokio::time::timeout(
        hub.cfg.agent_timeout + std::time::Duration::from_secs(1),
        hub.gateway.request(&intent.swimlane, cmd, hub.cfg.agent_timeout),
    )
    .await;
    match asked {
        // A delete landed when the file is gone; any file at the path means it did not (a file that has the hash that was to be
        // deleted was not deleted, and another file is somebody else's).
        Ok(Ok(AgentReply::Op(op)))
            if intent.op == IntentOp::Delete && op.error == Some(OpError::NotFound) =>
        {
            match commit_applied(hub, key, &intent, intent.new_hash).await {
                Ok(Committed::Done(outcome)) => Resolved::Completed(outcome),
                Ok(Committed::Lost(_)) => Resolved::Raced,
                Err(_) => Resolved::Unreachable,
            }
        }
        Ok(Ok(AgentReply::File { .. })) if intent.op == IntentOp::Delete => clear(hub, key).await,
        Ok(Ok(AgentReply::File { hash, bytes, .. }))
            if hash == intent.new_hash && ports::content_hash(&bytes) == hash =>
        {
            // It landed. Keep the version and record the write.
            if store_blob(hub, bytes).await.is_err() {
                return Resolved::Unreachable;
            }
            match commit_applied(hub, key, &intent, hash).await {
                Ok(Committed::Done(outcome)) => Resolved::Completed(outcome),
                Ok(Committed::Lost(_)) => Resolved::Raced,
                Err(_) => Resolved::Unreachable,
            }
        }
        // Another version is there.
        Ok(Ok(AgentReply::File { .. })) => clear(hub, key).await,
        // The file is not there.
        Ok(Ok(AgentReply::Op(op))) if op.error == Some(OpError::NotFound) => clear(hub, key).await,
        _ => Resolved::Unreachable,
    }
}

async fn clear(hub: &Inner, key: &ClaimKey) -> Resolved {
    match hub.within("clear", hub.idem.release(key)).await {
        Ok(Ok(())) => Resolved::Cleared,
        _ => Resolved::Unreachable,
    }
}

/// What one run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Writes that had landed and are recorded now.
    pub completed: usize,
    /// Claims removed: the write did not land, or nothing had been sent.
    pub cleared: usize,
    /// Claims left for the next run: the agent could not be asked.
    pub unreachable: usize,
    /// Finished idempotency rows that expired and were deleted.
    pub purged: u64,
}

/// The leased job (D63). Every replica builds one; only the holder of the lease runs it, and it stops when the lease is lost.
#[derive(Clone)]
pub struct ReconcileWrites {
    hub: Arc<Inner>,
}

impl std::fmt::Debug for ReconcileWrites {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReconcileWrites").finish_non_exhaustive()
    }
}

/// The most pages of expired rows one run deletes.
const MAX_PURGE_PAGES: usize = 20;

impl ReconcileWrites {
    pub(crate) fn new(hub: Arc<Inner>) -> Self {
        Self { hub }
    }

    /// One run: finish or clear the unfinished claims that are older than `stale_claim_after`, then delete expired rows.
    /// `still_held` is asked before each unit of work; when it says no, the run stops (the lease is lost).
    pub async fn run_once(&self, still_held: &dyn Fn() -> bool) -> ReconcileReport {
        let hub = &*self.hub;
        let mut report = ReconcileReport::default();
        let batch = i64::try_from(hub.cfg.reconcile_batch).unwrap_or(100);

        let stale = hub
            .within(
                "reconcile",
                hub.idem.stale(
                    hub.now_ms(),
                    millis(hub.cfg.stale_claim_after),
                    millis(hub.cfg.idempotency_ttl),
                    batch,
                ),
            )
            .await;
        let Ok(Ok(stale)) = stale else {
            return report;
        };
        if !stale.is_empty() {
            tracing::warn!(
                count = stale.len(),
                "unfinished write claims older than the grace period"
            );
        }
        for claim in stale {
            if !still_held() {
                return report;
            }
            match resolve(hub, &claim.key, claim.body.as_deref()).await {
                Resolved::Completed(_) => report.completed += 1,
                Resolved::Cleared => report.cleared += 1,
                Resolved::Unreachable => report.unreachable += 1,
                Resolved::Raced => {}
            }
        }

        for _ in 0..MAX_PURGE_PAGES {
            if !still_held() {
                break;
            }
            let purged = hub.within("purge", hub.idem.purge(hub.now_ms(), batch)).await;
            let Ok(Ok(n)) = purged else {
                break;
            };
            report.purged += n;
            if n < u64::try_from(batch).unwrap_or(100) {
                break;
            }
        }
        report
    }
}
