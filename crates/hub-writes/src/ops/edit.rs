//! `edit`: replace or create a file (T1). The reference executor of the pipeline.

use domain::{EditRequest, Expected, FileClass, RejectReason, Role, WriteCtx, WriteOutcome};
use engine::classify;
use ports::WriteError;

use crate::idempotency::{Intent, Phase, RequestHash};
use crate::pipeline::approval::{EditProposal, expected_hash, propose_edit, via_of};
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::current::{Found, old_content};
use crate::pipeline::execute::{Committed, Sent, commit_applied, hash_of, mark_ambiguous, send_write};
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::validate::{Checked, check as check_content};
use crate::pipeline::{Abort, Done, Step};
use crate::service::Inner;
use ports::{AuditAction, AuditActor, AuditEvent};

fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

pub(crate) async fn edit(hub: &Inner, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim(hub, ctx, RequestHash::of_edit(&r)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r).await;
    settle(hub, &claim, result).await
}

async fn run(hub: &Inner, ctx: &WriteCtx, user: &domain::User, claim: &Claim, r: EditRequest) -> Step<Done> {
    // 3. The editing lock.
    match check_lock(hub, &r.swimlane).await? {
        LockState::Open => {}
        LockState::Locked => return Ok(Done::Release(WriteOutcome::Locked { until: None })),
        LockState::UnknownSwimlane => return Ok(Done::Release(rejected(RejectReason::UnknownSwimlane))),
    }

    // 4. Validation: the cheap checks first, then the engine on the blocking pool.
    if r.content.len() > hub.cfg.max_content_bytes {
        return Ok(Done::Store(rejected(RejectReason::TooLarge)));
    }
    if hub.deny.is_denied(&r.path) {
        return Ok(Done::Store(rejected(RejectReason::Denied)));
    }
    // T2 replaces this with the file's own line endings, encoding and BOM.
    let prepared = r.content.clone();
    let class: FileClass = classify(&r.path, Some(&prepared), &hub.deny);
    let old = match old_content(hub, user, &r.swimlane, &r.path, &r.expected).await {
        Found::Old(old) => old,
        Found::Withheld => return Ok(Done::Store(rejected(RejectReason::Denied))),
    };
    let checked = hub
        .blocking("validate", {
            let (path, prepared, limits) = (r.path.clone(), prepared.clone(), hub.limits.clone());
            move || check_content(&path, class, &prepared, &old, &limits)
        })
        .await?;
    if let Some((reason, issues)) = checked.reject {
        return Ok(Done::Store(WriteOutcome::Rejected { reason, issues }));
    }

    // 5. A user who needs approval proposes; nothing is written.
    if user.requires_approval {
        return propose_edit(
            hub,
            EditProposal {
                ctx,
                user,
                claim,
                swimlane: &r.swimlane,
                path: &r.path,
                expected: &r.expected,
                content: prepared,
                diff: checked.diff,
            },
        )
        .await;
    }

    execute(hub, ctx, user, claim, r, prepared, checked).await
}

/// Step 6: send the change to the agent and record it.
async fn execute(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &domain::User,
    claim: &Claim,
    r: EditRequest,
    prepared: bytes::Bytes,
    checked: Checked,
) -> Step<Done> {
    let new_hash = hash_of(&prepared);
    let mut event = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: user.id.clone(),
        },
        AuditAction::FileEdited,
        via_of(ctx.via),
    );
    event.swimlane = Some(r.swimlane.clone());
    event.path = Some(r.path.clone());
    event.hash_before = expected_hash(&r.expected);
    event.hash_after = Some(new_hash);
    event.diff = Some(checked.diff);
    event.request_id = Some(ctx.request_id.clone());
    let intent = Intent {
        v: Intent::VERSION,
        phase: Phase::Sent,
        swimlane: r.swimlane.clone(),
        path: r.path.clone(),
        new_hash,
        user: user.id.clone(),
        request_id: ctx.request_id.clone(),
        event,
        warnings: checked.warnings,
    };
    hub.within("intent", hub.idem.set_intent(&claim.key, &intent))
        .await??;

    let expected: Expected = r.expected;
    match send_write(
        hub,
        &r.swimlane,
        &r.path,
        expected,
        prepared.clone(),
        &ctx.request_id,
    )
    .await
    {
        Sent::Undelivered => Ok(Done::Release(rejected(RejectReason::AgentUnavailable))),
        Sent::Refused { outcome, transient } => Ok(if transient {
            Done::Release(outcome)
        } else {
            Done::Store(outcome)
        }),
        Sent::Ambiguous => {
            mark_ambiguous(hub, &claim.key, &intent).await;
            Err(Abort::keep(WriteError::Unavailable))
        }
        Sent::Landed(found) => {
            let observed = found.unwrap_or(new_hash);
            // From here the file is on NFS: whatever fails, the claim stays with its intent so that the write is recorded later.
            let recorded = async {
                crate::pipeline::execute::store_blob(hub, prepared).await?;
                commit_applied(hub, &claim.key, &intent, observed).await
            }
            .await;
            match recorded {
                Ok(Committed::Done(outcome) | Committed::Lost(outcome)) => Ok(Done::Stored(outcome)),
                Err(e) => {
                    mark_ambiguous(hub, &claim.key, &intent).await;
                    Err(Abort::keep(e))
                }
            }
        }
    }
}
