//! Step 6: execute. The agent is sent the change with the hash it must find; then one transaction records it.
//!
//! The order is the point:
//!
//! 1. The intent (what was sent, and the audit event it will record) is kept with the claim, before the command goes out. If
//!    this process dies, or the answer is lost, the intent is what lets the write be completed or cleared later.
//! 2. The command goes to the agent with **no database connection held** (the performance rules forbid a transaction across an
//!    agent round trip). The agent compares the expected hash with the file and refuses on a mismatch: a conflict writes nothing.
//! 3. If the agent says it wrote, the new version is stored (it is a blob, kept by hash, D47) and one short transaction records
//!    the write: it locks the claim, registers the tool write, publishes the event through the outbox, records the audit event and
//!    stores the response. They commit together or not at all (rule 8, D80). The audit event is recorded after the cheap
//!    statements, so the chain lock is held for as short a time as possible.

use domain::{
    AgentReply, ContentHash, DomainEvent, Expected, HubCommand, NfsPath, ObservationSource, OpError,
    OpResult, RejectReason, RequestId, SwimlaneId, WriteOutcome,
};
use ports::{GatewayError, Tx, content_hash};

use crate::error::{Internal, Res};
use crate::idempotency::{ClaimKey, Intent, Phase};
use crate::service::Inner;

/// What the agent said to a write.
pub(crate) enum Sent {
    /// It wrote the file. The hash is what it found on NFS afterwards.
    Landed(Option<ContentHash>),
    /// It refused, and nothing changed. The outcome is final for this request when `transient` is false.
    Refused { outcome: WriteOutcome, transient: bool },
    /// The command was never delivered (no stream, the queue is full).
    Undelivered,
    /// The command may or may not have run.
    Ambiguous,
}

fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

/// Send `WriteFile` and read the answer.
pub(crate) async fn send_write(
    hub: &Inner,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    expected: Expected,
    bytes: bytes::Bytes,
    request_id: &RequestId,
) -> Sent {
    let cmd = HubCommand::WriteFile {
        request_id: request_id.clone(),
        path: path.clone(),
        expected,
        bytes,
    };
    // The gateway has its own timeout; this one is the backstop, a little longer, so the gateway's answer wins when both fire.
    let outer = hub.cfg.agent_timeout + std::time::Duration::from_secs(1);
    let reply = tokio::time::timeout(outer, hub.gateway.request(swimlane, cmd, hub.cfg.agent_timeout)).await;
    match reply {
        Ok(Err(GatewayError::NotConnected | GatewayError::Busy | GatewayError::Unavailable)) => {
            Sent::Undelivered
        }
        Ok(Ok(AgentReply::Op(OpResult {
            ok: true,
            current_hash,
            ..
        }))) => Sent::Landed(current_hash),
        Ok(Ok(AgentReply::Op(OpResult {
            ok: false,
            error,
            current_hash,
            ..
        }))) => match error {
            Some(OpError::Conflict) => Sent::Refused {
                outcome: WriteOutcome::Conflict {
                    current: current_hash,
                },
                transient: false,
            },
            // The agent was told to expect a file that is not there.
            Some(OpError::NotFound) => Sent::Refused {
                outcome: WriteOutcome::Conflict { current: None },
                transient: false,
            },
            Some(OpError::Denied) => Sent::Refused {
                outcome: rejected(RejectReason::Denied),
                transient: false,
            },
            Some(OpError::Unsupported) => Sent::Refused {
                outcome: rejected(RejectReason::InvalidContent),
                transient: false,
            },
            // The agent could not write (its I/O failed) and says nothing changed: try again later.
            Some(OpError::Io) | None => Sent::Refused {
                outcome: rejected(RejectReason::AgentUnavailable),
                transient: true,
            },
        },
        // No answer in time, a stream that closed, or an answer of another kind (a protocol error): the write may have run.
        Err(_) | Ok(Err(GatewayError::Timeout | GatewayError::Closed) | Ok(_)) => Sent::Ambiguous,
    }
}

/// Store the bytes that were written as a blob.
pub(crate) async fn store_blob(hub: &Inner, bytes: bytes::Bytes) -> Res<ContentHash> {
    hub.within("blob_put", hub.blobs.put(bytes))
        .await?
        .map_err(|_| Internal::new("blob_put", "blob store"))
}

/// The outcome of a recorded write.
pub(crate) enum Committed {
    Done(WriteOutcome),
    /// Another request had already recorded this claim (the retry and the reconcile job raced): its outcome.
    Lost(WriteOutcome),
}

const BLOB_REFS: &str = "INSERT INTO blob_refs (hash, ref_kind, ref_id) \
     SELECT b.hash, 'audit_event', $3 FROM blobs b WHERE b.hash = $1 OR b.hash = $2 ON CONFLICT DO NOTHING";

/// Record a write that landed: the second transaction of the pipeline. `observed` is the hash the agent found afterwards.
pub(crate) async fn commit_applied(
    hub: &Inner,
    key: &ClaimKey,
    intent: &Intent,
    observed: ContentHash,
) -> Res<Committed> {
    hub.within("commit", commit_applied_inner(hub, key, intent, observed))
        .await?
}

async fn commit_applied_inner(
    hub: &Inner,
    key: &ClaimKey,
    intent: &Intent,
    observed: ContentHash,
) -> Res<Committed> {
    let mut tx = hub.pool.begin().await.map_err(|e| Internal::db("commit", &e))?;

    // The claim must still be unfinished. Locking it makes a second finisher (the retry and the reconcile job) wait here, and
    // then find it finished.
    if let Some(done) = crate::idempotency::lock_claim(&mut tx, key).await? {
        let _ = tx.rollback().await;
        return Ok(Committed::Lost(done));
    }

    let mut event = intent.event.clone();
    event.hash_after = Some(observed);

    hub.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &intent.swimlane,
            &intent.path,
            &observed,
            &intent.user,
            &intent.request_id,
        )
        .await
        .map_err(|_| Internal::new("commit", "tool write registration"))?;
    hub.bus
        .publish_in_tx(
            &mut Tx::from_pg(&mut tx),
            DomainEvent::FileObserved {
                swimlane: intent.swimlane.clone(),
                path: intent.path.clone(),
                hash: observed,
                source: ObservationSource::ToolWrite,
            },
        )
        .await
        .map_err(|_| Internal::new("commit", "event outbox"))?;
    let audit_id = hub
        .audit
        .record(&mut Tx::from_pg(&mut tx), event.clone())
        .await
        .map_err(|_| Internal::new("commit", "audit log"))?;

    // The versions the event names are kept for as long as the event is (D78).
    let before = event.hash_before.unwrap_or(observed);
    sqlx::query(BLOB_REFS)
        .bind(before.as_bytes().as_slice())
        .bind(observed.as_bytes().as_slice())
        .bind(audit_id.get().to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| Internal::db("commit", &e))?;

    let outcome = WriteOutcome::Applied {
        hash: Some(observed),
        audit_id,
        warnings: intent.warnings.clone(),
    };
    crate::idempotency::finish_in_tx(&mut tx, key, &outcome).await?;
    tx.commit().await.map_err(|e| Internal::db("commit", &e))?;
    tracing::info!(
        request_id = %intent.request_id,
        swimlane = %intent.swimlane,
        path = %intent.path,
        hash = %observed,
        audit_id = audit_id.get(),
        "write recorded"
    );
    Ok(Committed::Done(outcome))
}

/// Say that the request that sent this intent has given up without knowing whether the write landed, so that the next request
/// with the key looks at the file at once and does not wait for a request that is no longer there.
pub(crate) async fn mark_ambiguous(hub: &Inner, key: &ClaimKey, intent: &Intent) {
    let mut intent = intent.clone();
    intent.phase = Phase::Ambiguous;
    let _ = hub.within("intent", hub.idem.set_intent(key, &intent)).await;
}

/// The hash a write of `bytes` will leave.
pub(crate) fn hash_of(bytes: &[u8]) -> ContentHash {
    content_hash(bytes)
}
