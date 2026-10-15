//! `delete`: remove a file (T3, D46). Like an upload it needs the preview token, and it needs the hash of the file it removes: there
//! is no "delete whatever is there". The last version of the file stays in the blob store, stored before the command goes out, so
//! a delete can always be undone with a revert.

use domain::{DeleteRequest, Expected, FileClass, RejectReason, Role, WriteCtx, WriteOutcome};
use engine::classify;
use ports::{AuditAction, AuditActor, AuditEvent, WriteError};

use super::file::{Command, carry_out, load_old, rejected, screen};
use crate::idempotency::{Intent, IntentOp, Phase, RequestHash};
use crate::pipeline::approval::{Proposal, base_hash, propose, via_of};
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::current::{AgentFile, agent_file};
use crate::pipeline::execute::store_blob;
use crate::pipeline::validate::{Old, delete_diff};
use crate::pipeline::{Done, Step};
use crate::preview::token::{Binding, PreviewAction};
use crate::service::Inner;

pub(crate) async fn delete(
    hub: &Inner,
    ctx: &WriteCtx,
    r: DeleteRequest,
) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim(hub, ctx, RequestHash::of_delete(&r)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r).await;
    settle(hub, &claim, result).await
}

async fn run(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &domain::User,
    claim: &Claim,
    r: DeleteRequest,
) -> Step<Done> {
    if let Some(done) = screen(hub, &r.swimlane, &r.path, 0).await? {
        return Ok(done);
    }
    let expected = Expected::Hash { hash: r.expected };
    let binding = Binding {
        user: &user.id,
        swimlane: &r.swimlane,
        path: &r.path,
        action: PreviewAction::Delete,
        expected: &expected,
        content: None,
    };
    if !hub.preview_ok(&binding, r.preview.as_ref()).await? {
        return Ok(Done::Release(rejected(RejectReason::NotAllowed)));
    }

    // The version being deleted: it is read back, because it is kept (the audit event names it, and a revert restores it).
    let old = match load_old(hub, user, &r.swimlane, &r.path, &expected).await? {
        Ok(old) => old,
        Err(done) => return Ok(done),
    };
    let bytes = match old {
        Old::Known(bytes) => bytes,
        // Nobody could give the version the caller expects. Ask the agent what is there: a different file is a conflict, and the
        // right answer; the same file that could not be read earlier is read now. A delete never goes out for a file whose last
        // version is not in hand.
        Old::Absent | Old::Unknown => match agent_file(hub, &r.swimlane, &r.path).await {
            AgentFile::Present { hash, bytes } if hash == r.expected => bytes,
            AgentFile::Present { hash, .. } => {
                return Ok(Done::Store(WriteOutcome::Conflict { current: Some(hash) }));
            }
            AgentFile::Absent => return Ok(Done::Store(WriteOutcome::Conflict { current: None })),
            AgentFile::Unreachable => {
                return Ok(Done::Release(rejected(RejectReason::AgentUnavailable)));
            }
        },
    };

    let diff = hub
        .blocking("delete_diff", {
            let (path, deny, limits) = (r.path.clone(), hub.deny.clone(), hub.limits.clone());
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

    // 5. A user who needs approval proposes; nothing is deleted.
    if user.requires_approval {
        return propose(
            hub,
            Proposal {
                ctx,
                user,
                claim,
                swimlane: &r.swimlane,
                action: "delete",
                path: Some(&r.path),
                paths: vec![r.path.as_str().to_owned()],
                base_hashes: base_hash(&r.path, &expected),
                hash_before: Some(r.expected),
                content: None,
                params: serde_json::json!({}),
                diff,
                draft: None,
            },
        )
        .await;
    }

    // 6. Keep the version, then delete.
    store_blob(hub, bytes).await?;
    let mut event = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: user.id.clone(),
        },
        AuditAction::FileDeleted,
        via_of(ctx.via),
    );
    event.swimlane = Some(r.swimlane.clone());
    event.path = Some(r.path.clone());
    event.hash_before = Some(r.expected);
    event.diff = Some(diff);
    event.request_id = Some(ctx.request_id.clone());
    let intent = Intent {
        v: Intent::VERSION,
        phase: Phase::Sent,
        op: IntentOp::Delete,
        swimlane: r.swimlane.clone(),
        path: r.path.clone(),
        // For a delete, the hash that was deleted: the file is gone when it no longer has it.
        new_hash: r.expected,
        user: user.id.clone(),
        request_id: ctx.request_id.clone(),
        event,
        warnings: Vec::new(),
        follow: None,
    };
    carry_out(hub, claim, intent, Command::Delete { expected: r.expected }).await
}
