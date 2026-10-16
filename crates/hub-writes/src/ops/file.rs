//! What every action that puts content into a file has in common: the screening before anything is read, the form the content is
//! put into, and the last two steps of the pipeline (approval, execute). `edit`, `upload` and `revert` differ in where the content
//! comes from and in what they check first; from the moment there are bytes to write, they are one and the same path, so a fix
//! to it is a fix to all three.

use bytes::Bytes;
use domain::{
    ChangeEol, ContentHash, Expected, FileClass, NfsPath, RejectReason, SwimlaneId, User, WriteCtx,
    WriteOutcome,
};
use engine::classify;
use ports::{AuditAction, AuditActor, AuditEvent};
use serde_json::json;

use crate::fidelity::{Incoming, folder_newline, prepare_checked};
use crate::idempotency::{Follow, Intent, IntentOp, Phase};
use crate::pipeline::approval::{Proposal, base_hash, expected_hash, propose, via_of};
use crate::pipeline::claim::Claim;
use crate::pipeline::current::{Found, old_content};
use crate::pipeline::execute::{
    Committed, Sent, commit_applied, hash_of, mark_ambiguous, send_delete, send_write, store_blob,
};
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::validate::{Checked, Old};
use crate::pipeline::{Abort, Done, Step};
use crate::service::Inner;

pub(crate) fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

/// Which action is putting content into the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Edit,
    Upload,
    Revert,
    /// An edit that a draft prepared; the audit event says so (`DraftApplied`).
    Draft,
}

impl Kind {
    fn audit(self) -> AuditAction {
        match self {
            Self::Edit => AuditAction::FileEdited,
            Self::Upload => AuditAction::FileUploaded,
            Self::Revert => AuditAction::FileReverted,
            Self::Draft => AuditAction::DraftApplied,
        }
    }

    /// The `proposals.action` of the action.
    fn proposal(self) -> &'static str {
        match self {
            Self::Edit | Self::Draft => "edit",
            Self::Upload => "upload",
            Self::Revert => "revert",
        }
    }
}

/// Step 3 and the cheap checks of step 4: the editing lock (D2), the size of the content, and the deny globs (D79). `len` is the
/// size of the content when it is known before it is fetched. `Some` ends the request.
pub(crate) async fn screen(
    hub: &Inner,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    len: usize,
) -> Step<Option<Done>> {
    match check_lock(hub, swimlane).await? {
        LockState::Open => {}
        LockState::Locked => return Ok(Some(Done::Release(WriteOutcome::Locked { until: None }))),
        LockState::UnknownSwimlane => {
            return Ok(Some(Done::Release(rejected(RejectReason::UnknownSwimlane))));
        }
    }
    if len > hub.cfg.max_content_bytes {
        return Ok(Some(Done::Store(rejected(RejectReason::TooLarge))));
    }
    if hub.deny.is_denied(path) {
        return Ok(Some(Done::Store(rejected(RejectReason::Denied))));
    }
    Ok(None)
}

/// What the file holds now, for the diff and for the form of the content that replaces it. A file that the registry withholds
/// ends the request.
pub(crate) async fn load_old(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    expected: &Expected,
) -> Step<Result<Old, Done>> {
    match old_content(hub, user, swimlane, path, expected).await {
        Found::Old(old) => Ok(Ok(old)),
        Found::Withheld => Ok(Err(Done::Store(rejected(RejectReason::Denied)))),
    }
}

/// The content, put into the form of the file it replaces and validated, on the blocking pool (T2). `Err` ends the request.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    path: &NfsPath,
    content: &Bytes,
    change_eol: Option<ChangeEol>,
    old: Old,
) -> Step<Result<(Bytes, Checked), Done>> {
    // A new file follows its folder, unless the request names a style.
    let folder = if matches!(old, Old::Absent) && change_eol.is_none() {
        folder_newline(hub, user, swimlane, path).await
    } else {
        None
    };
    let prepared = hub
        .blocking("validate", {
            let (path, content, deny) = (path.clone(), content.clone(), hub.deny.clone());
            let (limits, max_bytes) = (hub.limits.clone(), hub.cfg.max_content_bytes);
            move || {
                let class: FileClass = classify(&path, Some(&content), &deny);
                prepare_checked(
                    &Incoming {
                        path: &path,
                        class,
                        content: &content,
                        folder,
                        change_eol,
                        max_bytes,
                    },
                    &old,
                    &limits,
                )
            }
        })
        .await?;
    Ok(match prepared {
        Ok(ok) => Ok(ok),
        Err(reason) => Err(Done::Store(rejected(reason))),
    })
}

/// Everything the last two steps need to know about a write of content.
pub(crate) struct Write<'a> {
    pub kind: Kind,
    pub ctx: &'a WriteCtx,
    pub user: &'a User,
    pub claim: &'a Claim,
    pub swimlane: &'a SwimlaneId,
    pub path: &'a NfsPath,
    pub expected: &'a Expected,
    /// The version a revert restores; recorded with its proposal.
    pub restores: Option<ContentHash>,
    /// The draft this write applies (kind [`Kind::Draft`]): it is marked applied with the write, or waits with the proposal.
    pub draft: Option<&'a str>,
}

/// Steps 5 and 6 for content that is ready: refuse what failed validation, route a user who needs approval to a proposal, and
/// carry out the write for everybody else.
pub(crate) async fn finish(hub: &Inner, w: Write<'_>, prepared: Bytes, checked: Checked) -> Step<Done> {
    if let Some((reason, issues)) = checked.reject {
        return Ok(Done::Store(WriteOutcome::Rejected { reason, issues }));
    }

    // 5. A user who needs approval proposes; nothing is written.
    if w.user.requires_approval {
        let mut params = w
            .restores
            .map_or_else(|| json!({}), |to| json!({ "to": to.to_string() }));
        if let (Some(draft), Some(map)) = (w.draft, params.as_object_mut()) {
            map.insert("draft".to_owned(), json!(draft));
        }
        return propose(
            hub,
            Proposal {
                ctx: w.ctx,
                user: w.user,
                claim: w.claim,
                swimlane: w.swimlane,
                action: w.kind.proposal(),
                path: Some(w.path),
                paths: vec![w.path.as_str().to_owned()],
                base_hashes: base_hash(w.path, w.expected),
                hash_before: expected_hash(w.expected),
                content: Some(prepared),
                params,
                diff: checked.diff,
                draft: w.draft,
            },
        )
        .await;
    }

    // 6. Execute.
    let new_hash = hash_of(&prepared);
    let mut event = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: w.user.id.clone(),
        },
        w.kind.audit(),
        via_of(w.ctx.via),
    );
    event.swimlane = Some(w.swimlane.clone());
    event.path = Some(w.path.clone());
    event.hash_before = expected_hash(w.expected);
    event.hash_after = Some(new_hash);
    event.diff = Some(checked.diff);
    event.request_id = Some(w.ctx.request_id.clone());
    let intent = Intent {
        v: Intent::VERSION,
        phase: Phase::Sent,
        op: IntentOp::Write,
        swimlane: w.swimlane.clone(),
        path: w.path.clone(),
        new_hash,
        user: w.user.id.clone(),
        request_id: w.ctx.request_id.clone(),
        event,
        warnings: checked.warnings,
        follow: w.draft.map(|id| Follow::Draft { id: id.to_owned() }),
    };
    carry_out(
        hub,
        w.claim,
        intent,
        Command::Write {
            expected: *w.expected,
            bytes: prepared,
        },
    )
    .await
}

/// The command a write or a delete sends to the agent.
pub(crate) enum Command {
    Write { expected: Expected, bytes: Bytes },
    Delete { expected: ContentHash },
}

/// Keep the intent with the claim, send the command with no connection held, and record what happened. From the moment the file
/// has changed on NFS, whatever fails leaves the claim with its intent, so that the retry or the reconcile job records the write.
pub(crate) async fn carry_out(hub: &Inner, claim: &Claim, intent: Intent, command: Command) -> Step<Done> {
    hub.within("intent", hub.idem.set_intent(&claim.key, &intent))
        .await??;

    let sent = match &command {
        Command::Write { expected, bytes } => {
            send_write(
                hub,
                &intent.swimlane,
                &intent.path,
                *expected,
                bytes.clone(),
                &intent.request_id,
            )
            .await
        }
        Command::Delete { expected } => {
            send_delete(hub, &intent.swimlane, &intent.path, *expected, &intent.request_id).await
        }
    };
    match sent {
        Sent::Undelivered => Ok(Done::Release(rejected(RejectReason::AgentUnavailable))),
        Sent::Refused { outcome, transient } => Ok(if transient {
            Done::Release(outcome)
        } else {
            Done::Store(outcome)
        }),
        Sent::Ambiguous => {
            mark_ambiguous(hub, &claim.key, &intent).await;
            Err(Abort::keep(ports::WriteError::Unavailable))
        }
        Sent::Landed(found) => {
            let observed = found.unwrap_or(intent.new_hash);
            let recorded = async {
                // The new version is kept as a blob (D47). A deleted file's last version was kept before the command went out.
                if let Command::Write { bytes, .. } = command {
                    store_blob(hub, bytes).await?;
                }
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
