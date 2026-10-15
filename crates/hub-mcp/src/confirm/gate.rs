//! The gate in front of every write tool that acts on the world (`apply_change`, `upload_file`, `delete_file`, `raise_pr`,
//! `restart_service` and `notify_services`).
//!
//! **This is the only place in the crate that calls a mutating method of the write service** (`tests/mcp_auth/confirm.rs` scans
//! the sources for it). That includes `notify`, which ends at the config-server's unauthenticated `/update-resources` (Q37): the
//! write service audits it and sends it through the swimlane's agent, and this crate never makes that call itself.
//!
//! A call goes through these steps, and leaves at the first one that says no:
//!
//! 1. **Mode.** Draft mode (configured), or a client that did not declare that it can show a form: [`draft_mode`] answers with a
//!    link and no port is touched.
//! 2. **Key and replay.** The idempotency key is derived ([`idempotency`]). A finished write of this user under this key, for this
//!    request, is returned as it was and nobody is asked again (D68).
//! 3. **Prepare**, with calls that change nothing: `apply_change` reads the draft (its author only), `upload_file` and
//!    `delete_file` ask for the preview whose token the write must present (D46).
//! 4. **Ask** ([`elicit`]). The message is made from server data ([`message`]). Only an exact accept with `confirm: true` goes on
//!    ([`decision`]); a decline is `confirmation_declined`; a cancel, an answer that is no confirmation, a timeout, a withdrawn
//!    question and a vanished client are `confirmation_cancelled`.
//! 5. **Look at the user again.** The question can stay open for two minutes, so `Users::require_active` runs once more with the
//!    tool's role, and the write carries that fresh record (S4): a role that was lowered or a user that was disabled meanwhile
//!    writes nothing, and neither does an account whose approval rule changed, because the question said what would happen.
//! 6. **Write**, as a task of its own with a timeout (30 s by default). A user's yes is not undone by the client cancelling the
//!    call afterwards, and no cancel, disconnect or deadline of the call drops a write service call half way (rule 8: a state
//!    change and its audit event stay together); only the write's own timeout ends it, and a retry with the same key finds out what
//!    became of it (D66). The result is stored for the replay by that task, so it is there even when nobody is waiting for it.
//!
//! Writes go out with `Via::Mcp`; the write service audits them (rule 8). A user with `requires_approval` confirms like anybody
//! else and gets `proposal_created` from the write service (D37).

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{
    ContentHash, DeleteRequest, DraftId, Expected, IdempotencyKey, NfsPath, NotifyRequest, PrRequest,
    PreviewChange, PreviewId, PreviewRequest, RejectReason, RequestId, RestartRequest, SwimlaneId,
    UploadRequest, UserId, Via, WriteCtx, WriteOutcome,
};
use ports::{WriteError, WriteService, content_hash};
use serde_json::{Value, json};

use super::decision::Decision;
use super::elicit::Answer;
use super::replay::Replay;
use super::{draft_mode, idempotency, message};
use crate::config::ConfirmMode;
use crate::limits::Class;
use crate::output::envelope::{Parts, Rendered, Shape, render};
use crate::tools::dto::safe;
use crate::tools::error::{Code, ToolError};
use crate::tools::{CallCx, ToolResult};

/// What a write tool wants to do, owned.
#[derive(Clone)]
pub enum Action {
    /// Apply a draft the caller wrote.
    Apply {
        draft: DraftId,
    },
    Upload {
        swimlane: SwimlaneId,
        path: NfsPath,
        expected: Expected,
        content: Bytes,
    },
    Delete {
        swimlane: SwimlaneId,
        path: NfsPath,
        expected: ContentHash,
    },
    RaisePr(PrRequest),
    Restart(RestartRequest),
    /// Tell the config-server that files changed (D85). Off by default, and then it never gets here.
    Notify(NotifyRequest),
}

// The content of an upload and the body of a PR can hold a secret: a request never prints them (S10).
impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Action({})", self.tool())
    }
}

impl Action {
    /// The tool that makes this action.
    #[must_use]
    pub fn tool(&self) -> &'static str {
        match self {
            Self::Apply { .. } => "apply_change",
            Self::Upload { .. } => "upload_file",
            Self::Delete { .. } => "delete_file",
            Self::RaisePr(_) => "raise_pr",
            Self::Restart(_) => "restart_service",
            Self::Notify(_) => "notify_services",
        }
    }

    /// `raise_pr` has a second field in its result, `job`.
    fn with_job(&self) -> bool {
        matches!(self, Self::RaisePr(_))
    }

    /// A fingerprint of the request, so that a stored result answers that request and no other under the same key.
    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        let canonical = match self {
            Self::Apply { draft } => format!("apply|{draft}"),
            Self::Upload {
                swimlane,
                path,
                expected,
                content,
            } => format!("upload|{swimlane}|{path}|{expected:?}|{}", content_hash(content)),
            Self::Delete {
                swimlane,
                path,
                expected,
            } => format!("delete|{swimlane}|{path}|{expected}"),
            Self::RaisePr(r) => format!(
                "raise_pr|{}|{}|{}|{:?}|{}",
                r.swimlane,
                r.paths.iter().map(NfsPath::as_str).collect::<Vec<_>>().join("\n"),
                r.title,
                r.target,
                content_hash(r.body.as_deref().unwrap_or_default().as_bytes()),
            ),
            Self::Restart(r) => format!(
                "restart|{}|{}|{}",
                r.swimlane,
                r.service.namespace(),
                r.service.name()
            ),
            Self::Notify(r) => format!(
                "notify|{}|{}",
                r.swimlane,
                r.paths.iter().map(NfsPath::as_str).collect::<Vec<_>>().join("\n"),
            ),
        };
        content_hash(canonical.as_bytes())
    }

    /// The idempotency key (D68): from the draft id for `apply_change`, otherwise from the user, the session, the request and the
    /// tool. `None` when the session or the request id is not known, which is a defect and never a reason to write without a key.
    fn key(&self, cx: &CallCx<'_>) -> Option<IdempotencyKey> {
        match self {
            Self::Apply { draft } => idempotency::apply_key(draft),
            _ => idempotency::tool_key(&cx.user.id, cx.session_id?, cx.rpc_id?, self.tool()),
        }
    }

    /// What the summary says when the write was done.
    fn done_text(&self) -> &'static str {
        match self {
            Self::Apply { .. } => "The draft was applied.",
            Self::Upload { .. } => "The file was uploaded.",
            Self::Delete { .. } => "The file was deleted.",
            Self::RaisePr(_) => "The pull request was raised.",
            Self::Restart(_) => "The service is being restarted.",
            Self::Notify(_) => "The config-server was notified through the swimlane's agent.",
        }
    }
}

/// A write tool's request to the gate.
#[derive(Debug, Clone)]
pub struct Plan {
    pub action: Action,
    /// The page of the web UI where the change can be read in full, or confirmed when there is no question.
    pub link: Option<String>,
}

/// `outcome` of the write service as the contract's `result`, or the error it stands for (A13).
///
/// # Errors
///
/// The [`ToolError`] of a write that was refused or did not happen: `conflict` with the hash that is there now, `locked`,
/// `unprocessable` for content that was refused (with the reason and the places), `payload_too_large`, `not_found`,
/// `unavailable` and `forbidden`.
pub fn map_outcome(outcome: &WriteOutcome) -> Result<Value, ToolError> {
    match outcome {
        WriteOutcome::Applied {
            hash,
            audit_id,
            warnings,
        } => {
            let mut result = json!({
                "outcome": "applied",
                "hash": hash.map(|h| h.to_string()),
                "audit_id": u64::try_from(audit_id.get()).map_or(Value::Null, |n| json!(safe(n))),
                "proposal_id": null,
            });
            if !warnings.is_empty() {
                let shown: Vec<_> = warnings
                    .iter()
                    .take(domain::ValidationIssue::MAX_PER_OUTCOME)
                    .collect();
                result["warnings"] = serde_json::to_value(shown).unwrap_or(Value::Null);
            }
            Ok(result)
        }
        WriteOutcome::ProposalCreated { id } => Ok(json!({
            "outcome": "proposal_created",
            "hash": null,
            "audit_id": null,
            "proposal_id": u64::try_from(id.get()).map_or(Value::Null, |n| json!(safe(n))),
        })),
        WriteOutcome::Conflict { current } => Err(ToolError {
            current_hash: *current,
            ..ToolError::new(Code::Conflict)
        }),
        WriteOutcome::Locked { .. } => Err(ToolError::new(Code::Locked)),
        WriteOutcome::Rejected { reason, issues } => Err(match reason {
            RejectReason::Denied
            | RejectReason::InvalidContent
            | RejectReason::SecretDetected
            | RejectReason::ParseError => ToolError {
                reason: Some(*reason),
                issues: issues.clone(),
                ..ToolError::new(Code::Unprocessable)
            },
            RejectReason::TooLarge => ToolError::new(Code::PayloadTooLarge),
            RejectReason::UnknownSwimlane => ToolError::new(Code::NotFound),
            RejectReason::AgentUnavailable => ToolError::new(Code::Unavailable),
            RejectReason::NotAllowed => ToolError::new(Code::Forbidden),
        }),
    }
}

/// Run `call` for at most `limit`; a call that takes longer is `unavailable`.
pub(crate) async fn bounded<T>(
    limit: Duration,
    call: impl Future<Output = Result<T, WriteError>>,
) -> Result<T, ToolError> {
    match tokio::time::timeout(limit, call).await {
        Ok(result) => result.map_err(ToolError::from),
        Err(_) => Err(took_too_long()),
    }
}

fn took_too_long() -> ToolError {
    ToolError::with_detail(
        Code::Unavailable,
        "The write service took too long. Check the web UI before you try again.",
    )
}

/// The write context of a call.
pub(crate) fn write_ctx(cx: &CallCx<'_>, key: IdempotencyKey) -> Result<WriteCtx, ToolError> {
    let request_id = RequestId::parse(cx.request_id).map_err(|_| ToolError::new(Code::Internal))?;
    Ok(WriteCtx {
        user: cx.user.clone(),
        via: Via::Mcp,
        idempotency_key: key,
        request_id,
    })
}

/// What the read-only calls of step 3 found: the question, and the preview token the write must present.
struct Prepared {
    message: String,
    preview: Option<PreviewId>,
}

async fn prepare(
    cx: &CallCx<'_>,
    ctx: &WriteCtx,
    action: &Action,
    link: Option<&str>,
) -> Result<Prepared, ToolError> {
    let host = cx.host;
    let limit = host.limits.timeout(Class::Write);
    let approval = ctx.user.requires_approval;
    match action {
        Action::Apply { draft } => {
            let found = bounded(limit, host.writes.draft(ctx, draft.clone())).await?;
            // The port gives a draft to its author only. Checked again here: a draft of another user is as good as none.
            if found.author != ctx.user.id || &found.id != draft {
                return Err(ToolError::new(Code::NotFound));
            }
            Ok(Prepared {
                message: message::apply_change(&found, approval, link),
                preview: None,
            })
        }
        Action::Upload {
            swimlane,
            path,
            expected,
            content,
        } => {
            let preview = bounded(
                limit,
                host.writes.preview_effect(
                    ctx,
                    PreviewRequest {
                        swimlane: swimlane.clone(),
                        path: path.clone(),
                        change: PreviewChange::Upload {
                            expected: *expected,
                            content: content.clone(),
                        },
                    },
                ),
            )
            .await?;
            Ok(Prepared {
                message: message::upload_file(
                    swimlane,
                    path,
                    expected,
                    content.len(),
                    &preview,
                    approval,
                    link,
                ),
                preview: Some(preview.preview_id),
            })
        }
        Action::Delete {
            swimlane,
            path,
            expected,
        } => {
            let preview = bounded(
                limit,
                host.writes.preview_effect(
                    ctx,
                    PreviewRequest {
                        swimlane: swimlane.clone(),
                        path: path.clone(),
                        change: PreviewChange::Delete { expected: *expected },
                    },
                ),
            )
            .await?;
            Ok(Prepared {
                message: message::delete_file(swimlane, path, expected, &preview, approval, link),
                preview: Some(preview.preview_id),
            })
        }
        Action::RaisePr(request) => Ok(Prepared {
            message: message::raise_pr(request, approval, link),
            preview: None,
        }),
        Action::Restart(request) => Ok(Prepared {
            message: message::restart_service(request, approval, link),
            preview: None,
        }),
        Action::Notify(request) => Ok(Prepared {
            message: message::notify_services(request, approval, link),
            preview: None,
        }),
    }
}

/// The write itself: the one place where a mutating method of the write service is called.
async fn write(
    writes: &dyn WriteService,
    ctx: &WriteCtx,
    action: Action,
    preview: Option<PreviewId>,
) -> Result<WriteOutcome, WriteError> {
    match action {
        Action::Apply { draft } => writes.apply_draft(ctx, draft).await,
        Action::Upload {
            swimlane,
            path,
            expected,
            content,
        } => {
            writes
                .upload(
                    ctx,
                    UploadRequest {
                        swimlane,
                        path,
                        expected,
                        content,
                        preview,
                    },
                )
                .await
        }
        Action::Delete {
            swimlane,
            path,
            expected,
        } => {
            writes
                .delete(
                    ctx,
                    DeleteRequest {
                        swimlane,
                        path,
                        expected,
                        preview,
                    },
                )
                .await
        }
        Action::RaisePr(request) => writes.raise_pr(ctx, request).await,
        Action::Restart(request) => writes.restart(ctx, request).await,
        Action::Notify(request) => writes.notify(ctx, request).await,
    }
}

/// What the task that writes needs once the write is done.
struct Finish {
    shape: Shape,
    link: Option<String>,
    with_job: bool,
    done_text: &'static str,
    replay: Replay,
    user: UserId,
    key: IdempotencyKey,
    fingerprint: ContentHash,
}

impl Finish {
    fn complete(
        self,
        outcome: Result<Result<WriteOutcome, WriteError>, tokio::time::error::Elapsed>,
    ) -> ToolResult {
        let outcome = match outcome {
            Err(_) => return Err(took_too_long()),
            Ok(Err(error)) => return Err(ToolError::from(error)),
            Ok(Ok(outcome)) => outcome,
        };
        let result = map_outcome(&outcome)?;
        let mut data = json!({ "result": result });
        if self.with_job {
            data["job"] = Value::Null;
        }
        let summary = match outcome {
            WriteOutcome::ProposalCreated { .. } => {
                "A proposal was created for an admin to approve. Nothing was done yet.".to_owned()
            }
            _ => self.done_text.to_owned(),
        };
        let rendered: Rendered = render(
            &self.shape,
            Parts {
                summary,
                data,
                truncated: false,
                next: None,
                web_link: self.link,
            },
        );
        // Only a write that went through is remembered; a refusal is asked again if the person tries again.
        self.replay
            .put(&self.user, &self.key, &self.fingerprint, &rendered);
        Ok(rendered)
    }
}

/// Gate a write: ask the user, and write only on an exact yes.
///
/// # Errors
///
/// The [`ToolError`] of the first step that said no (see the module).
pub async fn gate(cx: &CallCx<'_>, plan: Plan) -> ToolResult {
    let host = cx.host;
    let Plan { action, link } = plan;

    // 1. Mode.
    let asker = match cx.asker {
        Some(asker) if host.confirm_mode == ConfirmMode::Elicitation && asker.can_ask() => asker,
        _ => {
            host.metrics.confirmation("draft_link");
            return draft_mode::answer(cx, link, action.with_job());
        }
    };

    // 2. Key and replay.
    let Some(key) = action.key(cx) else {
        tracing::error!(target: "hub_mcp::call", tool = action.tool(), "no idempotency key can be made for this call");
        return Err(ToolError::new(Code::Internal));
    };
    let fingerprint = action.fingerprint();
    if let Some(done) = host.replay.get(&cx.user.id, &key, &fingerprint) {
        host.metrics.confirmation("replayed");
        return Ok(done);
    }
    let ctx = write_ctx(cx, key.clone())?;

    // 3. Prepare.
    let prepared = prepare(cx, &ctx, &action, link.as_deref()).await?;

    // 4. Ask.
    host.metrics.confirmation("asked");
    match asker.ask(&prepared.message).await {
        Answer::Replied(Decision::Proceed) => host.metrics.confirmation("accepted"),
        Answer::Replied(Decision::Declined) => {
            host.metrics.confirmation("declined");
            return Err(ToolError::new(Code::ConfirmationDeclined));
        }
        Answer::Replied(Decision::Cancelled) => {
            host.metrics.confirmation("cancelled");
            return Err(ToolError::new(Code::ConfirmationCancelled));
        }
        Answer::TimedOut => {
            host.metrics.confirmation("timed_out");
            return Err(ToolError::with_detail(
                Code::ConfirmationCancelled,
                "No answer arrived in time. Nothing was done.",
            ));
        }
        Answer::Aborted => {
            host.metrics.confirmation("aborted");
            return Err(ToolError::new(Code::ConfirmationCancelled));
        }
    }

    // 5. Look at the user again: the question may have been open for minutes.
    let user = host
        .require_active(&cx.user.id, cx.spec.role())
        .await
        .map_err(ToolError::from)?;
    // The question said whether this becomes a proposal. If that changed while it was open, what was confirmed is not what would
    // happen, and nothing is done.
    if user.requires_approval != ctx.user.requires_approval {
        host.metrics.confirmation("terms_changed");
        return Err(ToolError::with_detail(
            Code::ConfirmationCancelled,
            "Your account changed while the question was open. Nothing was done. Ask again.",
        ));
    }
    let ctx = WriteCtx { user, ..ctx };

    // 6. Write, as a task of its own.
    let writes = Arc::clone(&host.writes);
    let write_limit = host.limits.timeout(Class::Write);
    let finish = Finish {
        shape: cx.shape(false),
        link,
        with_job: action.with_job(),
        done_text: action.done_text(),
        replay: host.replay.clone(),
        user: cx.user.id.clone(),
        key,
        fingerprint,
    };
    let preview = prepared.preview;
    let task = tokio::spawn(async move {
        let outcome = tokio::time::timeout(write_limit, write(&*writes, &ctx, action, preview)).await;
        finish.complete(outcome)
    });
    match task.await {
        Ok(result) => result,
        Err(_) => Err(ToolError::new(Code::Internal)),
    }
}
