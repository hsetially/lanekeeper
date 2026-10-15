//! `restart`: roll a Deployment through the agent (T3, D49). Operator only.
//!
//! - **At most one restart per service at a time,** across replicas: the request holds a lease on the service while the command
//!   runs. A second request for the same service meanwhile is `Locked`, and nothing is sent for it.
//! - The service has to be one the swimlane's cluster report lists; a name the agent never reported is refused here, so a forged
//!   or stale reference never becomes a command.
//! - A user who needs approval proposes a restart like any other change.
//! - **Restarting everything.** A change to a root file is read by every service (D82), so the screen offers "restart all services
//!   in this swimlane", with a confirmation that lists each one. [`restart_offer`] is that list. The restarts themselves are one
//!   request per service, each with its own idempotency key: there is no batch call, so one service that fails does not hide
//!   the others and each restart has its own audit event.

use std::time::Duration;

use domain::{
    DomainEvent, NfsPath, ProposalId, RejectReason, RestartRequest, Role, ServiceRef, SwimlaneId, User,
    UserId, WriteCtx, WriteOutcome,
};
use ports::{AuditAction, AuditActor, AuditEvent, WriteError};
use sha2::{Digest, Sha256};

use super::file::rejected;
use crate::error::{Internal, Res};
use crate::idempotency::{Follow, RequestHash};
use crate::pipeline::approval::{Proposal, propose, via_of};
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::execute::{Committed, Recording, Sent, record, send_restart};
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::{Abort, Done, Step};
use crate::service::Inner;

/// How long the lease on a service is held: longer than the agent's timeout, so the lease outlives the command it guards, and
/// short enough that a replica that dies in the middle frees the service quickly.
const LEASE_TTL: Duration = Duration::from_secs(60);

/// The most services one swimlane lists for a restart-all confirmation.
pub const MAX_RESTART_OFFER: usize = 2_000;

const SERVICE_KNOWN: &str = "SELECT 1 FROM services \
     WHERE swimlane_id = $1 AND namespace = $2 AND name = $3 AND removed_at IS NULL";

const SERVICES_OF: &str = "SELECT namespace, name FROM services WHERE swimlane_id = $1 AND removed_at IS NULL \
     ORDER BY namespace, name LIMIT $2";

const SERVICES_OF_APP: &str = "SELECT namespace, name FROM services \
     WHERE swimlane_id = $1 AND removed_at IS NULL AND name = $2 ORDER BY namespace, name LIMIT $3";

/// The name of the lease on one service: lease names are short and plain, so the service is reduced to a digest.
fn lease_name(swimlane: &SwimlaneId, service: &ServiceRef) -> String {
    let mut h = Sha256::new();
    for part in [swimlane.as_str(), service.namespace(), service.name()] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    let digest = h.finalize();
    format!("restart:{}", crate::hex::encode(&digest[..16]))
}

pub(crate) async fn restart(
    hub: &Inner,
    ctx: &WriteCtx,
    r: RestartRequest,
) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Operator).await?;
    let claim = match claim(hub, ctx, RequestHash::of_restart(&r)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r).await;
    settle(hub, &claim, result).await
}

pub(crate) async fn service_known(hub: &Inner, swimlane: &SwimlaneId, service: &ServiceRef) -> Res<bool> {
    let row: Option<i32> = hub
        .within(
            "restart_service",
            sqlx::query_scalar(SERVICE_KNOWN)
                .bind(swimlane.as_str())
                .bind(service.namespace())
                .bind(service.name())
                .fetch_optional(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("restart_service", &e))?;
    Ok(row.is_some())
}

fn note(service: &ServiceRef) -> String {
    format!("restart deployment {}/{}", service.namespace(), service.name())
}

async fn run(hub: &Inner, ctx: &WriteCtx, user: &User, claim: &Claim, r: RestartRequest) -> Step<Done> {
    // A restart is not a file action: the editing lock does not apply to it. The swimlane still has to exist.
    if matches!(check_lock(hub, &r.swimlane).await?, LockState::UnknownSwimlane) {
        return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane)));
    }
    if !service_known(hub, &r.swimlane, &r.service).await? {
        return Ok(Done::Store(rejected(RejectReason::NotAllowed)));
    }

    // A user who needs approval proposes; nothing is restarted.
    if user.requires_approval {
        return propose(
            hub,
            Proposal {
                ctx,
                user,
                claim,
                swimlane: &r.swimlane,
                action: "restart",
                path: None,
                paths: vec![format!("{}/{}", r.service.namespace(), r.service.name())],
                base_hashes: serde_json::json!({}),
                hash_before: None,
                content: None,
                params: serde_json::json!({
                    "namespace": r.service.namespace(),
                    "name": r.service.name(),
                }),
                diff: note(&r.service),
                draft: None,
            },
        )
        .await;
    }

    execute(hub, ctx, user, claim, &r.swimlane, &r.service, None).await
}

/// The proposal a restart carries out, when an admin's approval is what sends it.
pub(crate) struct ByApproval {
    pub proposal: ProposalId,
    pub author: UserId,
    /// Decided in the transaction that records the restart.
    pub follow: Follow,
}

/// Steps 6 of the pipeline for a restart: hold the lease on the service, send the command, record it.
pub(crate) async fn execute(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &User,
    claim: &Claim,
    swimlane: &SwimlaneId,
    service: &ServiceRef,
    by: Option<ByApproval>,
) -> Step<Done> {
    // At most one restart of the service at a time, on any replica.
    let name = lease_name(swimlane, service);
    let guard = match hub
        .within("restart_lease", hub.leases.try_acquire(&name, LEASE_TTL))
        .await?
    {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(Done::Release(WriteOutcome::Locked { until: None })),
        Err(_) => return Err(Abort::release(WriteError::Unavailable)),
    };
    let result = send_and_record(hub, ctx, user, claim, swimlane, service, by).await;
    // Free the service at once; a failure only means the lease runs out by itself.
    if guard.release().await.is_err() {
        tracing::warn!("a restart lease could not be released; it runs out by itself");
    }
    result
}

async fn send_and_record(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &User,
    claim: &Claim,
    swimlane: &SwimlaneId,
    service: &ServiceRef,
    by: Option<ByApproval>,
) -> Step<Done> {
    match send_restart(hub, swimlane, service, &ctx.request_id).await {
        Sent::Undelivered => Ok(Done::Release(rejected(RejectReason::AgentUnavailable))),
        Sent::Refused { outcome, transient } => Ok(if transient {
            Done::Release(outcome)
        } else {
            Done::Store(outcome)
        }),
        // The agent may or may not have started the rollout. A restart cannot be checked afterwards the way a file can, so the
        // request is not recorded and the key is freed: a retry restarts again, which is harmless (a Deployment rolls to the same
        // template), and the residual risk (an unaudited restart) is in the threat model.
        Sent::Ambiguous => {
            tracing::warn!(swimlane = %swimlane, "a restart command was sent and its answer was lost");
            Err(Abort::release(WriteError::Unavailable))
        }
        Sent::Landed(_) => {
            let mut event = AuditEvent::new(
                hub.clock.now(),
                AuditActor::User {
                    user: user.id.clone(),
                },
                AuditAction::ServiceRestarted,
                via_of(ctx.via),
            );
            event.swimlane = Some(swimlane.clone());
            event.diff = Some(note(service));
            event.request_id = Some(ctx.request_id.clone());
            let follow = by.map(|by| {
                event.author = Some(by.author);
                event.approval = Some(by.proposal);
                by.follow
            });
            let recorded = record(
                hub,
                Recording {
                    key: &claim.key,
                    swimlane,
                    event,
                    tool_write: None,
                    events: vec![DomainEvent::PickupChanged {
                        swimlane: swimlane.clone(),
                        services: vec![service.clone()],
                    }],
                    keep: Vec::new(),
                    applied_hash: None,
                    warnings: Vec::new(),
                    follow,
                },
            )
            .await;
            match recorded {
                Ok(Committed::Done(outcome) | Committed::Lost(outcome)) => Ok(Done::Stored(outcome)),
                // The Deployment is rolling and the record failed. The key is freed; the failure is logged by the store.
                Err(e) => Err(Abort::release(e)),
            }
        }
    }
}

/// The services a restart-all confirmation lists for a change to `path`, or none when the change does not call for one.
///
/// A file in the root folder is read by every service (D82), so every service of the swimlane is listed. A file in an
/// application's folder is read by that application only: the Deployments of that name are listed. Operator only, like the
/// restart itself.
pub(crate) async fn restart_offer(
    hub: &Inner,
    ctx: &WriteCtx,
    swimlane: &SwimlaneId,
    path: &NfsPath,
) -> Result<Vec<ServiceRef>, WriteError> {
    authorize(hub, ctx, Role::Operator).await?;
    let limit = i64::try_from(MAX_RESTART_OFFER).unwrap_or(2_000);
    let rows: Vec<(String, String)> = if path.parent().is_root() {
        hub.within(
            "restart_offer",
            sqlx::query_as(SERVICES_OF)
                .bind(swimlane.as_str())
                .bind(limit)
                .fetch_all(&hub.pool),
        )
        .await?
    } else {
        let app = path.components().next().unwrap_or_default().to_owned();
        hub.within(
            "restart_offer",
            sqlx::query_as(SERVICES_OF_APP)
                .bind(swimlane.as_str())
                .bind(app)
                .bind(limit)
                .fetch_all(&hub.pool),
        )
        .await?
    }
    .map_err(|e| Internal::db("restart_offer", &e))?;
    // A row that is not a valid reference (the table does not check Kubernetes naming) is left out, not guessed at.
    Ok(rows
        .into_iter()
        .filter_map(|(namespace, name)| ServiceRef::new(&namespace, &name).ok())
        .collect())
}
