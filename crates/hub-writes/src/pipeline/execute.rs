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
    OpResult, RejectReason, RequestId, ServiceRef, SwimlaneId, UserId, ValidationIssue, WriteOutcome,
};
use ports::{AuditEvent, GatewayError, Tx, content_hash};

use crate::error::{Internal, Res};
use crate::idempotency::{ClaimKey, Follow, Intent, IntentOp, Phase};
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
    // The agent was told to expect a file that is not there.
    call(hub, swimlane, cmd, WriteOutcome::Conflict { current: None }).await
}

/// Send `DeleteFile` and read the answer. The agent compares `expected` with the file and refuses on a mismatch.
pub(crate) async fn send_delete(
    hub: &Inner,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    expected: ContentHash,
    request_id: &RequestId,
) -> Sent {
    let cmd = HubCommand::DeleteFile {
        request_id: request_id.clone(),
        path: path.clone(),
        expected,
    };
    // A file that is not there is not the file the caller meant to delete.
    call(hub, swimlane, cmd, WriteOutcome::Conflict { current: None }).await
}

/// Send `RestartDeployment` and read the answer.
pub(crate) async fn send_restart(
    hub: &Inner,
    swimlane: &SwimlaneId,
    service: &ServiceRef,
    request_id: &RequestId,
) -> Sent {
    let cmd = HubCommand::RestartDeployment {
        request_id: request_id.clone(),
        service: service.clone(),
    };
    // The cluster has no such Deployment.
    call(hub, swimlane, cmd, rejected(RejectReason::InvalidContent)).await
}

/// What the gateway said, or that it said nothing in time.
type Asked = Result<Result<AgentReply, GatewayError>, tokio::time::error::Elapsed>;

/// Send `NotifyConfigServer` and read the answer (D86). The agent answers with the config-server's HTTP status: a `2xx` is a
/// notification that happened, any other status is the config-server's refusal. Neither is the caller's fault, and a retry may
/// succeed, so a refusal is never stored with the key. An agent that cannot call the config-server at all (it has no address for
/// it) says so; that is a setting somebody can change, so it too is only "unavailable".
pub(crate) async fn send_notify(
    hub: &Inner,
    swimlane: &SwimlaneId,
    paths: Vec<NfsPath>,
    request_id: &RequestId,
) -> Sent {
    let cmd = HubCommand::NotifyConfigServer {
        request_id: request_id.clone(),
        paths,
    };
    let unavailable = Sent::Refused {
        outcome: rejected(RejectReason::AgentUnavailable),
        transient: true,
    };
    match ask(hub, swimlane, cmd).await {
        Ok(Ok(AgentReply::Notify { status, .. })) => {
            if (200..300).contains(&status) {
                Sent::Landed(None)
            } else {
                unavailable
            }
        }
        Ok(Ok(AgentReply::Op(OpResult {
            ok: false,
            error: Some(OpError::Unsupported),
            ..
        }))) => unavailable,
        other => interpret(&other, rejected(RejectReason::InvalidContent)),
    }
}

/// Send a command and turn the answer into what the request does next. `not_found` is the outcome when the agent says the
/// thing the command names is not there.
async fn call(hub: &Inner, swimlane: &SwimlaneId, cmd: HubCommand, not_found: WriteOutcome) -> Sent {
    interpret(&ask(hub, swimlane, cmd).await, not_found)
}

async fn ask(hub: &Inner, swimlane: &SwimlaneId, cmd: HubCommand) -> Asked {
    // The gateway has its own timeout; this one is the backstop, a little longer, so the gateway's answer wins when both fire.
    let outer = hub.cfg.agent_timeout + std::time::Duration::from_secs(1);
    tokio::time::timeout(outer, hub.gateway.request(swimlane, cmd, hub.cfg.agent_timeout)).await
}

fn interpret(reply: &Asked, not_found: WriteOutcome) -> Sent {
    match reply {
        Ok(Err(GatewayError::NotConnected | GatewayError::Busy | GatewayError::Unavailable)) => {
            Sent::Undelivered
        }
        Ok(Ok(AgentReply::Op(OpResult {
            ok: true,
            current_hash,
            ..
        }))) => Sent::Landed(*current_hash),
        Ok(Ok(AgentReply::Op(OpResult {
            ok: false,
            error,
            current_hash,
            ..
        }))) => match error {
            Some(OpError::Conflict) => Sent::Refused {
                outcome: WriteOutcome::Conflict {
                    current: *current_hash,
                },
                transient: false,
            },
            Some(OpError::NotFound) => Sent::Refused {
                outcome: not_found,
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
            // The agent could not act (its I/O failed) and says nothing changed: try again later.
            Some(OpError::Io) | None => Sent::Refused {
                outcome: rejected(RejectReason::AgentUnavailable),
                transient: true,
            },
        },
        // No answer in time, a stream that closed, or an answer of another kind (a protocol error): the command may have run.
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
     SELECT b.hash, 'audit_event', $2 FROM blobs b WHERE b.hash = ANY($1::bytea[]) ON CONFLICT DO NOTHING";

/// The tool write to register in the recording transaction (D73).
pub(crate) struct ToolWrite<'a> {
    pub path: &'a NfsPath,
    pub hash: ContentHash,
    pub user: &'a UserId,
    pub request_id: &'a RequestId,
}

/// Everything the recording transaction of an action writes.
pub(crate) struct Recording<'a> {
    pub key: &'a ClaimKey,
    pub swimlane: &'a SwimlaneId,
    /// The audit event: exactly one for the action (rule 8).
    pub event: AuditEvent,
    pub tool_write: Option<ToolWrite<'a>>,
    /// Published through the outbox in the same transaction (D80).
    pub events: Vec<DomainEvent>,
    /// The versions the audit event names, kept for as long as the event is (D78).
    pub keep: Vec<ContentHash>,
    /// The hash the response carries.
    pub applied_hash: Option<ContentHash>,
    pub warnings: Vec<ValidationIssue>,
    /// The state change that commits with the action: the decision on the proposal it carries out, or the draft it applies.
    pub follow: Option<Follow>,
    /// A notification of the config-server (T11): the services' pickup states it changed, written and hinted with the audit event.
    pub pickups: Option<crate::ops::pickup::PickupUpdate>,
}

/// Record an action that happened: the second transaction of the pipeline. The claim is locked first, so that a second finisher
/// (the retry and the reconcile job) waits and then finds it finished.
pub(crate) async fn record(hub: &Inner, rec: Recording<'_>) -> Res<Committed> {
    // What the batcher would announce, read before the event is consumed.
    let announce = crate::autonotify::AutoItem::of(&rec.event);
    let committed = hub.within("commit", record_inner(hub, rec)).await??;
    // Only a write this request recorded is announced: one that another request already recorded (`Lost`) was announced there.
    if let (Committed::Done(_), Some(item)) = (&committed, announce) {
        hub.auto.offer(hub, item).await;
    }
    Ok(committed)
}

async fn record_inner(hub: &Inner, rec: Recording<'_>) -> Res<Committed> {
    let mut tx = hub.pool.begin().await.map_err(|e| Internal::db("commit", &e))?;

    if let Some(done) = crate::idempotency::lock_claim(&mut tx, rec.key).await? {
        let _ = tx.rollback().await;
        return Ok(Committed::Lost(done));
    }

    if let Some(follow) = &rec.follow {
        super::follow::apply(hub, &mut tx, follow).await?;
    }
    if let Some(update) = &rec.pickups {
        crate::ops::pickup::apply(hub, &mut tx, rec.swimlane, update).await?;
    }
    if let Some(w) = &rec.tool_write {
        hub.tool_writes
            .register(
                &mut Tx::from_pg(&mut tx),
                rec.swimlane,
                w.path,
                &w.hash,
                w.user,
                w.request_id,
            )
            .await
            .map_err(|_| Internal::new("commit", "tool write registration"))?;
    }
    for event in rec.events {
        hub.bus
            .publish_in_tx(&mut Tx::from_pg(&mut tx), event)
            .await
            .map_err(|_| Internal::new("commit", "event outbox"))?;
    }
    // The audit event goes last among the statements that can fail for reasons of ours, so the chain lock is held for as short a
    // time as possible.
    let audit_id = hub
        .audit
        .record(&mut Tx::from_pg(&mut tx), rec.event)
        .await
        .map_err(|_| Internal::new("commit", "audit log"))?;

    let keep: Vec<Vec<u8>> = rec.keep.iter().map(|h| h.as_bytes().to_vec()).collect();
    sqlx::query(BLOB_REFS)
        .bind(keep)
        .bind(audit_id.get().to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| Internal::db("commit", &e))?;

    let outcome = WriteOutcome::Applied {
        hash: rec.applied_hash,
        audit_id,
        warnings: rec.warnings,
    };
    crate::idempotency::finish_in_tx(&mut tx, rec.key, &outcome).await?;
    tx.commit().await.map_err(|e| Internal::db("commit", &e))?;
    tracing::info!(
        audit_id = audit_id.get(),
        swimlane = %rec.swimlane,
        "action recorded"
    );
    Ok(Committed::Done(outcome))
}

/// Record a write or a delete that landed, from the intent kept with its claim: the second transaction of the pipeline.
/// `observed` is the hash the agent found afterwards (ignored for a delete).
pub(crate) async fn commit_applied(
    hub: &Inner,
    key: &ClaimKey,
    intent: &Intent,
    observed: ContentHash,
) -> Res<Committed> {
    let mut event = intent.event.clone();
    let before = event.hash_before;
    let rec = match intent.op {
        IntentOp::Write => {
            event.hash_after = Some(observed);
            Recording {
                key,
                swimlane: &intent.swimlane,
                event,
                tool_write: Some(ToolWrite {
                    path: &intent.path,
                    hash: observed,
                    user: &intent.user,
                    request_id: &intent.request_id,
                }),
                events: vec![DomainEvent::FileObserved {
                    swimlane: intent.swimlane.clone(),
                    path: intent.path.clone(),
                    hash: observed,
                    source: ObservationSource::ToolWrite,
                }],
                keep: before.into_iter().chain([observed]).collect(),
                applied_hash: Some(observed),
                warnings: intent.warnings.clone(),
                follow: intent.follow.clone(),
                pickups: None,
            }
        }
        // A delete leaves no hash to register: the next scan sees the file gone. The drift hint tells the screens to look.
        IntentOp::Delete => {
            event.hash_after = None;
            Recording {
                key,
                swimlane: &intent.swimlane,
                event,
                tool_write: None,
                events: vec![DomainEvent::DriftChanged {
                    swimlane: intent.swimlane.clone(),
                    paths: vec![intent.path.clone()],
                }],
                keep: before.into_iter().collect(),
                applied_hash: None,
                warnings: Vec::new(),
                follow: intent.follow.clone(),
                pickups: None,
            }
        }
    };
    record(hub, rec).await
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
