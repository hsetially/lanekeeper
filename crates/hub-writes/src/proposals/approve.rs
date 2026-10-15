//! Approving and rejecting a proposal (D37).
//!
//! **The rules, in the order they are checked:**
//!
//! 1. Only an admin decides, read from the directory and not from the request (S4).
//! 2. Nobody decides their own proposal. This is checked here first; the database refuses it again (the trigger of
//!    `approvals`, and from migration 0004 the application role cannot change an approval or a proposal's author).
//! 3. The proposal has to be pending and not past its time.
//! 4. One decider at a time: a lease on the proposal is held while the command is on its way, so two admins on two replicas
//!    cannot both apply it, and a rejection cannot slip in between the re-check and the write.
//!
//! **Approving** looks at the file again. The proposal holds the hash its author expected there; if the file has another hash
//! the world has moved on, nothing is written and the proposal becomes `stale` (a 409 with the hash found). If it matches, the
//! change is written like any other write, through the same executor: the intent is kept with the idempotency claim, the command
//! goes to the agent with the expected hash and no connection held, and one transaction records the audit event, the decision,
//! the proposal's new status and the events together (`pipeline::follow`). The audit event is the one of the action (`file_edited`
//! and so on) with the approver as its actor, the proposal as its `approval` and the proposer as its `author` (D38).
//!
//! **Rejecting** writes the decision, the status, the event and one audit event in one transaction. Nothing else changes.

use domain::{
    ContentHash, DomainEvent, Expected, FileClass, NfsPath, ProposalId, ProposalStatus, RejectReason, Role,
    ServiceRef, Severity, ShortText, User, WriteCtx, WriteOutcome,
};
use engine::classify;
use ports::{AuditAction, AuditActor, AuditEvent, Notification, NotificationKind, Tx, WriteError};
use sqlx::{Postgres, Transaction};

use super::RejectResult;
use super::list::one as view_of_one;
use super::store::{self, Base, NewDecision, Stored};
use crate::drafts;
use crate::error::{Internal, Res};
use crate::idempotency::{ClaimKey, Follow, Intent, IntentOp, Phase, RequestHash, finish_in_tx, lock_claim};
use crate::ops::file::{Command, carry_out, rejected};
use crate::ops::restart::{self, ByApproval};
use crate::pipeline::approval::via_of;
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::current::{AgentFile, agent_file};
use crate::pipeline::execute::{hash_of, store_blob};
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::validate::{Old, check, delete_diff};
use crate::pipeline::{Abort, Done, Step};
use crate::service::Inner;
use std::time::Duration;

/// How long the lease on a proposal is held: longer than the agent's timeout, so it outlives the command it guards.
const LEASE_TTL: Duration = Duration::from_secs(60);

fn lease_name(id: ProposalId) -> String {
    format!("proposal:{}", id.get())
}

// -------------------------------------------------------------------------------------------------------------- approve

pub(crate) async fn approve(
    hub: &Inner,
    ctx: &WriteCtx,
    id: ProposalId,
    comment: Option<ShortText>,
) -> Result<WriteOutcome, WriteError> {
    let admin = authorize(hub, ctx, Role::Admin).await?;
    let hash = RequestHash::of_approve(id, comment.as_ref().map(ShortText::as_str));
    let claim = match claim(hub, ctx, hash).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = decide(hub, ctx, &admin, &claim, id, Act::Approve(comment)).await;
    settle(hub, &claim, result).await
}

/// What the admin decided.
enum Act {
    Approve(Option<ShortText>),
    Reject(ShortText),
}

/// Steps 1 to 4 for a decision: the lease on the proposal is taken, and then, with it held, the proposal is read, found not to be
/// the decider's own, found pending and not past its time, and the decision is carried out. Reading it under the lease is what
/// makes "pending" true for the whole of the decision: nobody else decides it between the read and the write.
async fn decide(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    id: ProposalId,
    act: Act,
) -> Step<Done> {
    let lease = match hub
        .within(
            "proposal_lease",
            hub.leases.try_acquire(&lease_name(id), LEASE_TTL),
        )
        .await?
    {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(Done::Release(WriteOutcome::Locked { until: None })),
        Err(_) => return Err(Abort::release(WriteError::Unavailable)),
    };
    let result = decide_locked(hub, ctx, admin, claim, id, act).await;
    if lease.release().await.is_err() {
        tracing::warn!("a proposal lease could not be released; it runs out by itself");
    }
    result
}

async fn decide_locked(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    id: ProposalId,
    act: Act,
) -> Step<Done> {
    let mut conn = hub
        .within("proposal_get", hub.pool.acquire())
        .await?
        .map_err(|e| Internal::db("proposal_get", &e))?;
    let found = hub.within("proposal_get", store::get(&mut conn, id)).await??;
    drop(conn);
    let Some(p) = found else {
        return Err(Abort::release(WriteError::NotFound));
    };
    // Nobody decides their own proposal (S4, D37).
    if p.author == admin.id {
        return Err(Abort::release(WriteError::Forbidden));
    }
    if p.status != ProposalStatus::Pending || p.expires_ms <= hub.now_ms() {
        return Ok(Done::Store(rejected(RejectReason::NotAllowed)));
    }
    match act {
        Act::Approve(comment) => apply(hub, ctx, admin, claim, p, comment).await,
        Act::Reject(reason) => reject_tx(hub, ctx, admin, claim, p, reason).await,
    }
}

/// The action of a proposal, carried out for the admin. Actions that other tasks wire (a PR, a notify) are not carried out yet:
/// the admin is told the service is unavailable and nothing changes.
async fn apply(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: Stored,
    comment: Option<ShortText>,
) -> Step<Done> {
    let comment = comment.map(|c| c.as_str().to_owned());
    let done = match p.action.as_str() {
        "edit" | "upload" | "revert" => apply_content(hub, ctx, admin, claim, &p, comment).await,
        "delete" => apply_delete(hub, ctx, admin, claim, &p, comment).await,
        "restart" => apply_restart(hub, ctx, admin, claim, &p, comment).await,
        _ => Err(Abort::release(WriteError::Unavailable)),
    };
    if matches!(done, Ok(Done::Stored(WriteOutcome::Applied { .. }))) {
        announce(hub, &p, "was approved and applied").await;
    }
    done
}

/// The audit event of the action a proposal carries out: the approver is the actor, the proposer the author, the proposal the
/// approval.
fn event(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    p: &Stored,
    action: AuditAction,
    path: Option<&NfsPath>,
) -> AuditEvent {
    let mut e = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: admin.id.clone(),
        },
        action,
        via_of(ctx.via),
    );
    e.swimlane = Some(p.swimlane.clone());
    e.path = path.cloned();
    e.author = Some(p.author.clone());
    e.approval = Some(p.id);
    e.request_id = Some(ctx.request_id.clone());
    e
}

fn follow(admin: &User, p: &Stored, comment: Option<String>, rechecked: Option<ContentHash>) -> Follow {
    Follow::Decided {
        proposal: p.id,
        approver: admin.id.clone(),
        comment,
        rechecked,
        draft: p.draft(),
    }
}

/// The path, the hash the author expected there (`None` for "not there") and the version it proposed, from the row. A row that
/// does not say is refused: the proposal was not made by this code.
struct Parts {
    path: NfsPath,
    expected: Option<ContentHash>,
}

fn parts(p: &Stored) -> Option<Parts> {
    let text = p.paths.first()?;
    Some(Parts {
        path: NfsPath::parse(text).ok()?,
        expected: match p.expected_at(text)? {
            Base::Absent => None,
            Base::At(hash) => Some(hash),
        },
    })
}

async fn apply_content(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: &Stored,
    comment: Option<String>,
) -> Step<Done> {
    let (Some(Parts { path, expected }), Some(new_hash)) = (parts(p), p.new_hash) else {
        return Ok(Done::Store(rejected(RejectReason::InvalidContent)));
    };
    // The lock, the size and the deny globs are the ones of today, not of the day the proposal was made.
    match check_lock(hub, &p.swimlane).await? {
        LockState::Open => {}
        LockState::Locked => return Ok(Done::Release(WriteOutcome::Locked { until: None })),
        LockState::UnknownSwimlane => {
            return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane)));
        }
    }
    if hub.deny.is_denied(&path) {
        return Ok(Done::Store(rejected(RejectReason::Denied)));
    }
    let bytes = match hub.within("proposal_blob", hub.blobs.get(&new_hash)).await? {
        Ok(Some(b)) if hash_of(&b) == new_hash => b,
        _ => return Err(Abort::release(WriteError::Unavailable)),
    };
    if bytes.len() > hub.cfg.max_content_bytes {
        return Ok(Done::Store(rejected(RejectReason::TooLarge)));
    }

    // The re-check: what is on NFS now, from the agent (the registry can be a scan behind).
    let (current, old) = match agent_file(hub, &p.swimlane, &path).await {
        AgentFile::Present { hash, bytes } => (Some(hash), Old::Known(bytes)),
        AgentFile::Absent => (None, Old::Absent),
        AgentFile::Unreachable => return Ok(Done::Release(rejected(RejectReason::AgentUnavailable))),
    };
    if current != expected {
        return mark_stale(hub, ctx, claim, admin, p, &path, expected, current).await;
    }

    // And the content is validated again, with today's rules: a rule that learned a new secret shape blocks it.
    let checked = hub
        .blocking("approve_validate", {
            let (path, bytes, old) = (path.clone(), bytes.clone(), old.clone());
            let (deny, limits) = (hub.deny.clone(), hub.limits.clone());
            move || {
                let class: FileClass = classify(&path, Some(&bytes), &deny);
                check(&path, class, &bytes, &old, &limits)
            }
        })
        .await?;
    if let Some((reason, issues)) = checked.reject {
        return Ok(Done::Store(WriteOutcome::Rejected { reason, issues }));
    }

    let action = match p.action.as_str() {
        "upload" => AuditAction::FileUploaded,
        "revert" => AuditAction::FileReverted,
        _ => AuditAction::FileEdited,
    };
    let mut e = event(hub, ctx, admin, p, action, Some(&path));
    e.hash_before = expected;
    e.hash_after = Some(new_hash);
    e.diff = Some(checked.diff);
    let intent = Intent {
        v: Intent::VERSION,
        phase: Phase::Sent,
        op: IntentOp::Write,
        swimlane: p.swimlane.clone(),
        path,
        new_hash,
        user: admin.id.clone(),
        request_id: ctx.request_id.clone(),
        event: e,
        warnings: checked.warnings,
        follow: Some(follow(admin, p, comment, current)),
    };
    let command = Command::Write {
        expected: expected.map_or(Expected::Absent, |hash| Expected::Hash { hash }),
        bytes,
    };
    carry_out(hub, claim, intent, command).await
}

async fn apply_delete(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: &Stored,
    comment: Option<String>,
) -> Step<Done> {
    let Some(Parts {
        path,
        expected: Some(expected),
    }) = parts(p)
    else {
        return Ok(Done::Store(rejected(RejectReason::InvalidContent)));
    };
    match check_lock(hub, &p.swimlane).await? {
        LockState::Open => {}
        LockState::Locked => return Ok(Done::Release(WriteOutcome::Locked { until: None })),
        LockState::UnknownSwimlane => {
            return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane)));
        }
    }
    if hub.deny.is_denied(&path) {
        return Ok(Done::Store(rejected(RejectReason::Denied)));
    }
    let bytes = match agent_file(hub, &p.swimlane, &path).await {
        AgentFile::Present { hash, bytes } if hash == expected => bytes,
        AgentFile::Present { hash, .. } => {
            return mark_stale(hub, ctx, claim, admin, p, &path, Some(expected), Some(hash)).await;
        }
        AgentFile::Absent => {
            return mark_stale(hub, ctx, claim, admin, p, &path, Some(expected), None).await;
        }
        AgentFile::Unreachable => return Ok(Done::Release(rejected(RejectReason::AgentUnavailable))),
    };
    let diff = hub
        .blocking("approve_delete_diff", {
            let (path, deny, limits) = (path.clone(), hub.deny.clone(), hub.limits.clone());
            let old = Old::Known(bytes.clone());
            move || {
                let class: FileClass = match &old {
                    Old::Known(b) => classify(&path, Some(b), &deny),
                    _ => FileClass::Binary,
                };
                delete_diff(class, &old, &limits)
            }
        })
        .await?;
    // The version being deleted is kept, so a revert can bring it back.
    store_blob(hub, bytes).await?;

    let mut e = event(hub, ctx, admin, p, AuditAction::FileDeleted, Some(&path));
    e.hash_before = Some(expected);
    e.diff = Some(diff);
    let intent = Intent {
        v: Intent::VERSION,
        phase: Phase::Sent,
        op: IntentOp::Delete,
        swimlane: p.swimlane.clone(),
        path,
        new_hash: expected,
        user: admin.id.clone(),
        request_id: ctx.request_id.clone(),
        event: e,
        warnings: Vec::new(),
        follow: Some(follow(admin, p, comment, Some(expected))),
    };
    carry_out(hub, claim, intent, Command::Delete { expected }).await
}

async fn apply_restart(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: &Stored,
    comment: Option<String>,
) -> Step<Done> {
    let service = p
        .params
        .get("namespace")
        .and_then(serde_json::Value::as_str)
        .zip(p.params.get("name").and_then(serde_json::Value::as_str))
        .and_then(|(ns, name)| ServiceRef::new(ns, name).ok());
    let Some(service) = service else {
        return Ok(Done::Store(rejected(RejectReason::InvalidContent)));
    };
    if matches!(check_lock(hub, &p.swimlane).await?, LockState::UnknownSwimlane) {
        return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane)));
    }
    if !restart::service_known(hub, &p.swimlane, &service).await? {
        return Ok(Done::Store(rejected(RejectReason::NotAllowed)));
    }
    let by = ByApproval {
        proposal: p.id,
        author: p.author.clone(),
        follow: follow(admin, p, comment, None),
    };
    restart::execute(hub, ctx, admin, claim, &p.swimlane, &service, Some(by)).await
}

// ---------------------------------------------------------------------------------------------------------------- stale

/// What marking a proposal stale came to.
enum Marked {
    /// Done, and the response is stored with the claim.
    Stored(WriteOutcome),
    /// The proposal was decided by somebody else in the meantime.
    NotPending,
}

/// The file is not what the proposal's author expected: record that, in one transaction, and answer 409 with the hash found.
#[allow(clippy::too_many_arguments)]
async fn mark_stale(
    hub: &Inner,
    ctx: &WriteCtx,
    claim: &Claim,
    admin: &User,
    p: &Stored,
    path: &NfsPath,
    expected: Option<ContentHash>,
    current: Option<ContentHash>,
) -> Step<Done> {
    match hub
        .within(
            "proposal_stale",
            stale_tx(hub, ctx, &claim.key, admin, p, path, expected, current),
        )
        .await?
    {
        Ok(Marked::Stored(outcome)) => {
            announce(hub, p, "was not applied: the file changed").await;
            Ok(Done::Stored(outcome))
        }
        Ok(Marked::NotPending) => Ok(Done::Store(rejected(RejectReason::NotAllowed))),
        Err(e) => Err(Abort::release(e)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn stale_tx(
    hub: &Inner,
    ctx: &WriteCtx,
    key: &ClaimKey,
    admin: &User,
    p: &Stored,
    path: &NfsPath,
    expected: Option<ContentHash>,
    current: Option<ContentHash>,
) -> Res<Marked> {
    let mut tx = hub
        .pool
        .begin()
        .await
        .map_err(|e| Internal::db("proposal_stale", &e))?;
    if let Some(done) = lock_claim(&mut tx, key).await? {
        let _ = tx.rollback().await;
        return Ok(Marked::Stored(done));
    }
    if store::lock_status(&mut tx, p.id).await? != Some(ProposalStatus::Pending) {
        let _ = tx.rollback().await;
        return Ok(Marked::NotPending);
    }
    let now = hub.now_ms();
    store::insert_decision(
        &mut tx,
        &NewDecision {
            proposal: p.id,
            approved: true,
            approver: &admin.id,
            comment: None,
            rechecked: current,
            at_ms: now,
        },
    )
    .await?;
    store::set_status(&mut tx, p.id, ProposalStatus::Stale, now).await?;
    if let Some(draft) = p.draft() {
        drafts::mark(&mut tx, &draft, ProposalStatus::Stale).await?;
    }
    publish(hub, &mut tx, p.id, ProposalStatus::Stale).await?;

    let mut e = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: admin.id.clone(),
        },
        AuditAction::ProposalStale,
        via_of(ctx.via),
    );
    e.swimlane = Some(p.swimlane.clone());
    e.path = Some(path.clone());
    e.hash_before = expected;
    e.hash_after = current;
    e.author = Some(p.author.clone());
    e.approval = Some(p.id);
    e.request_id = Some(ctx.request_id.clone());
    hub.audit
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .map_err(|_| Internal::new("proposal_stale", "audit log"))?;
    let outcome = WriteOutcome::Conflict { current };
    finish_in_tx(&mut tx, key, &outcome).await?;
    tx.commit()
        .await
        .map_err(|e| Internal::db("proposal_stale", &e))?;
    Ok(Marked::Stored(outcome))
}

pub(crate) async fn publish(
    hub: &Inner,
    tx: &mut Transaction<'_, Postgres>,
    id: ProposalId,
    status: ProposalStatus,
) -> Res<()> {
    hub.bus
        .publish_in_tx(
            &mut Tx::from_pg(&mut *tx),
            DomainEvent::ProposalChanged { id, status },
        )
        .await
        .map_err(|_| Internal::new("proposal", "event outbox"))
}

// -------------------------------------------------------------------------------------------------------------- reject

pub(crate) async fn reject(
    hub: &Inner,
    ctx: &WriteCtx,
    id: ProposalId,
    reason: ShortText,
) -> Result<RejectResult, WriteError> {
    let admin = authorize(hub, ctx, Role::Admin).await?;
    let claim = match claim(hub, ctx, RequestHash::of_reject(id, reason.as_str())).await? {
        Claimed::Replay(outcome) => return finish_reject(hub, id, outcome).await,
        Claimed::Mine(claim) => claim,
    };
    let result = decide(hub, ctx, &admin, &claim, id, Act::Reject(reason)).await;
    let outcome = settle(hub, &claim, result).await?;
    finish_reject(hub, id, outcome).await
}

/// A rejection that went through answers with the proposal as it reads now; anything else is why nothing changed.
async fn finish_reject(
    hub: &Inner,
    id: ProposalId,
    outcome: WriteOutcome,
) -> Result<RejectResult, WriteError> {
    if !matches!(outcome, WriteOutcome::Applied { .. }) {
        return Ok(RejectResult::Refused(outcome));
    }
    let mut conn = hub
        .within("proposal_get", hub.pool.acquire())
        .await?
        .map_err(|e| Internal::db("proposal_get", &e))?;
    let stored = hub.within("proposal_get", store::get(&mut conn, id)).await??;
    drop(conn);
    let Some(stored) = stored else {
        return Err(WriteError::NotFound);
    };
    match view_of_one(hub, stored).await? {
        Some(view) => Ok(RejectResult::Rejected(Box::new(view))),
        None => Err(WriteError::Unavailable),
    }
}

async fn reject_tx(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: Stored,
    reason: ShortText,
) -> Step<Done> {
    match hub
        .within(
            "proposal_reject",
            reject_inner(hub, ctx, admin, claim, &p, &reason),
        )
        .await?
    {
        Ok(Some(outcome)) => {
            announce(hub, &p, "was rejected").await;
            Ok(Done::Stored(outcome))
        }
        Ok(None) => Ok(Done::Store(rejected(RejectReason::NotAllowed))),
        Err(e) => Err(Abort::release(e)),
    }
}

async fn reject_inner(
    hub: &Inner,
    ctx: &WriteCtx,
    admin: &User,
    claim: &Claim,
    p: &Stored,
    reason: &ShortText,
) -> Res<Option<WriteOutcome>> {
    let mut tx = hub
        .pool
        .begin()
        .await
        .map_err(|e| Internal::db("proposal_reject", &e))?;
    if let Some(done) = lock_claim(&mut tx, &claim.key).await? {
        let _ = tx.rollback().await;
        return Ok(Some(done));
    }
    if store::lock_status(&mut tx, p.id).await? != Some(ProposalStatus::Pending) {
        let _ = tx.rollback().await;
        return Ok(None);
    }
    let now = hub.now_ms();
    store::insert_decision(
        &mut tx,
        &NewDecision {
            proposal: p.id,
            approved: false,
            approver: &admin.id,
            comment: Some(reason.as_str()),
            rechecked: None,
            at_ms: now,
        },
    )
    .await?;
    store::set_status(&mut tx, p.id, ProposalStatus::Rejected, now).await?;
    if let Some(draft) = p.draft() {
        drafts::mark(&mut tx, &draft, ProposalStatus::Rejected).await?;
    }
    publish(hub, &mut tx, p.id, ProposalStatus::Rejected).await?;

    let path = p.paths.first().and_then(|t| NfsPath::parse(t).ok());
    let e = event(hub, ctx, admin, p, AuditAction::ProposalRejected, path.as_ref());
    let audit_id = hub
        .audit
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .map_err(|_| Internal::new("proposal_reject", "audit log"))?;
    let outcome = WriteOutcome::Applied {
        hash: None,
        audit_id,
        warnings: Vec::new(),
    };
    finish_in_tx(&mut tx, &claim.key, &outcome).await?;
    tx.commit()
        .await
        .map_err(|e| Internal::db("proposal_reject", &e))?;
    Ok(Some(outcome))
}

// ------------------------------------------------------------------------------------------------------------- notify

/// Announce that a proposal was decided, on the channel of the proposal-decided notification (D49). A failure is logged and never fails the decision.
async fn announce(hub: &Inner, p: &Stored, what: &str) {
    let (Ok(title), Ok(summary), Ok(link)) = (
        ShortText::parse("Proposal decided"),
        ShortText::parse(&format!("Proposal {} in {} {what}", p.id, p.swimlane)),
        ShortText::parse(&format!("/proposals/{}", p.id)),
    ) else {
        tracing::warn!("a decision notification could not be built");
        return;
    };
    let note = Notification {
        kind: NotificationKind::ProposalDecided,
        severity: Severity::Low,
        swimlane: Some(p.swimlane.clone()),
        title,
        summary,
        link: Some(link),
    };
    let sent = hub.within("notify_decided", hub.notifier.notify(note)).await;
    if !matches!(sent, Ok(Ok(()))) {
        tracing::warn!(
            proposal = p.id.get(),
            "a proposal decision could not be announced"
        );
    }
}
