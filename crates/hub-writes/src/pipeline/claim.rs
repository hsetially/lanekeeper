//! Step 2: the idempotency claim (D68), and what becomes of it when the request ends.

use std::time::Duration;

use domain::{PrJobId, WriteCtx, WriteOutcome};
use ports::WriteError;
use tokio::time::Instant;

use crate::idempotency::{Claim as Stored, ClaimKey, Intent, Phase, RequestHash};
use crate::pipeline::{Abort, Done};
use crate::reconcile::{Resolved, resolve};
use crate::service::{Inner, millis};

/// A claim this request owns.
#[derive(Debug)]
pub(crate) struct Claim {
    pub key: ClaimKey,
}

pub(crate) enum Claimed {
    Mine(Claim),
    /// An earlier request with this key and this request finished: its response.
    Replay(WriteOutcome),
}

/// [`Claimed`] for a request whose answer can be a job: a pull request (T9).
pub(crate) enum ClaimedAny {
    Mine(Claim),
    Replay(WriteOutcome),
    /// An earlier request with this key and this request was accepted as this job.
    Job(PrJobId),
}

/// How often a request that waits for another one looks at the row again, with jitter so replicas do not look in step.
const POLL_BASE_MS: u64 = 25;
const POLL_JITTER_MS: u64 = 25;

/// Claim `key` for the request with hash `hash`.
///
/// - Nobody used the key: the claim is made and the request runs.
/// - The key was used for this request and it finished: its response.
/// - The key was used for another request: [`WriteError::IdempotencyConflict`].
/// - The same request is running: wait up to `idempotency_wait` for it, then [`WriteError::Unavailable`]. A request whose sender
///   gave up without knowing whether the write landed (or one that began more than `stale_claim_after` ago) is not waited for:
///   the file is looked at and the write is completed or the claim cleared.
pub(crate) async fn claim(hub: &Inner, ctx: &WriteCtx, hash: RequestHash) -> Result<Claimed, WriteError> {
    match claim_any(hub, ctx, hash).await? {
        ClaimedAny::Mine(c) => Ok(Claimed::Mine(c)),
        ClaimedAny::Replay(o) => Ok(Claimed::Replay(o)),
        // A key is bound to one request hash and a write's hash carries its own action: a job cannot be what it was given.
        ClaimedAny::Job(_) => Err(WriteError::IdempotencyConflict),
    }
}

/// [`claim`] for a request that can also be answered with a job.
pub(crate) async fn claim_any(
    hub: &Inner,
    ctx: &WriteCtx,
    hash: RequestHash,
) -> Result<ClaimedAny, WriteError> {
    let key = ClaimKey::of(ctx);
    let ttl = millis(hub.cfg.idempotency_ttl);
    let stale_after = millis(hub.cfg.stale_claim_after);
    let deadline = Instant::now() + hub.cfg.idempotency_wait;

    let mut state = hub
        .within("claim", hub.idem.claim(&key, &hash, hub.now_ms(), ttl))
        .await??;
    loop {
        match state {
            Stored::Fresh { born_ms } => {
                return Ok(ClaimedAny::Mine(Claim {
                    key: key.born(born_ms),
                }));
            }
            Stored::Replay(outcome) => return Ok(ClaimedAny::Replay(outcome)),
            Stored::ReplayJob(id) => return Ok(ClaimedAny::Job(id)),
            Stored::Conflict => return Err(WriteError::IdempotencyConflict),
            Stored::InFlight { created_ms, body } => {
                let abandoned = body
                    .as_deref()
                    .and_then(|b| serde_json::from_slice::<Intent>(b).ok())
                    .is_some_and(|i| i.phase == Phase::Ambiguous);
                let aged = hub.now_ms().saturating_sub(created_ms) >= stale_after;
                if abandoned || aged {
                    match resolve(hub, &key.born(created_ms), body.as_deref()).await {
                        Resolved::Completed(outcome) => return Ok(ClaimedAny::Replay(outcome)),
                        Resolved::Cleared => {
                            state = hub
                                .within("claim", hub.idem.claim(&key, &hash, hub.now_ms(), ttl))
                                .await??;
                            continue;
                        }
                        Resolved::Unreachable => return Err(WriteError::Unavailable),
                        // Somebody else is finishing it: wait for their answer like any other request in flight.
                        Resolved::Raced => {}
                    }
                }
                if Instant::now() >= deadline {
                    return Err(WriteError::Unavailable);
                }
                tokio::time::sleep(Duration::from_millis(
                    POLL_BASE_MS + fastrand::u64(0..=POLL_JITTER_MS),
                ))
                .await;
                state = match hub.within("claim", hub.idem.peek(&key, &hash)).await?? {
                    Some(s) => s,
                    // The request we waited for released the key: it is ours to take.
                    None => {
                        hub.within("claim", hub.idem.claim(&key, &hash, hub.now_ms(), ttl))
                            .await??
                    }
                };
            }
        }
    }
}

/// End the request: store the response, free the key, or leave the claim as it is, as `result` says. The response goes back
/// to the caller whatever happens to the claim.
pub(crate) async fn settle(
    hub: &Inner,
    claim: &Claim,
    result: Result<Done, Abort>,
) -> Result<WriteOutcome, WriteError> {
    match result {
        Ok(Done::Stored(outcome)) => Ok(outcome),
        Ok(Done::Store(outcome)) => {
            // A failure to store is logged by the store; the caller still gets the answer, and the claim, which has no intent,
            // is cleared by the reconcile job.
            let _ = hub.within("finish", hub.idem.finish(&claim.key, &outcome)).await;
            Ok(outcome)
        }
        Ok(Done::Release(outcome)) => {
            release(hub, claim).await;
            Ok(outcome)
        }
        Err(abort) => {
            if !abort.keep {
                release(hub, claim).await;
            }
            Err(abort.error)
        }
    }
}

async fn release(hub: &Inner, claim: &Claim) {
    if !matches!(
        hub.within("release", hub.idem.release(&claim.key)).await,
        Ok(Ok(()))
    ) {
        // The row stays unfinished and without an intent, which the reconcile job clears once it is old.
        tracing::warn!("could not release an idempotency claim");
    }
}
