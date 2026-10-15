//! The write tools. Each one that acts on the world ends in [`crate::confirm::gate`], the only place that calls a mutating method of
//! the write service; `propose_change` only records a draft.

use bytes::Bytes;
use domain::{
    ContentHash, DraftId, EditRequest, NfsPath, PrRequest, RestartRequest, Severity, SwimlaneId, Timestamp,
};
use serde::Serialize;

use crate::confirm::gate::{bounded, write_ctx};
use crate::confirm::{Action, Plan, draft_mode, gate, idempotency};
use crate::output::envelope::DATA_LABEL;
use crate::tools::args::{
    ApplyChange, DeleteFile, NotifyServices, ProposeChange, RaisePr, RestartService, UploadFile,
};
use crate::tools::dto::{SettingChangeDto, setting_change};
use crate::tools::error::{Code, ToolError};
use crate::tools::paging::capped_result;
use crate::tools::read::to_json;
use crate::tools::{CallCx, ToolResult};

/// The most setting changes a draft's summary lists: the contract's `maxItems`.
const MAX_CHANGES: usize = 20;
/// The most services whose effect is listed first; fewer when the 4 KB cap leaves no room (the rest are behind the link).
const EFFECTS_TRIED: [usize; 4] = [10, 4, 1, 0];
/// The longest setting value in a draft's summary, in characters: the cap is 4 KB.
const VALUE_CHARS: usize = 160;

#[derive(Serialize)]
struct EffectDto<'a> {
    service: &'a domain::ServiceRef,
    pickup: domain::PickupState,
}

#[derive(Serialize)]
struct DraftData<'a> {
    draft_id: &'a DraftId,
    swimlane: &'a SwimlaneId,
    path: &'a NfsPath,
    expected: &'a domain::Expected,
    new_hash: &'a ContentHash,
    setting_change_count: u64,
    setting_changes: &'a [SettingChangeDto],
    hunk_count: u64,
    severity: Option<Severity>,
    effects: Vec<EffectDto<'a>>,
    expires_at: Timestamp,
    untrusted: bool,
    label: &'static str,
}

/// A write of a denied file is refused before anything is asked or recorded (D79): not drafted, not put to the user, and not given a
/// link to a web page that would only say no. The write service refuses it too; this is the early, certain no.
fn refuse_denied<'a>(cx: &CallCx<'_>, paths: impl IntoIterator<Item = &'a NfsPath>) -> Result<(), ToolError> {
    if paths.into_iter().any(|p| cx.host.denied.path(p)) {
        return Err(ToolError::denied());
    }
    Ok(())
}

/// `propose_change` records a draft of a whole-file edit and returns what it would change. It has no side effect: the file is not
/// written, and nobody is asked, because the change is confirmed when `apply_change` runs (D52).
pub async fn propose_change(cx: &CallCx<'_>, args: ProposeChange) -> ToolResult {
    refuse_denied(cx, [&args.path.0])?;
    let key = cx
        .session_id
        .zip(cx.rpc_id)
        .and_then(|(session, rpc)| idempotency::tool_key(&cx.user.id, session, rpc, cx.spec.name()))
        .ok_or_else(|| ToolError::new(Code::Internal))?;
    let ctx = write_ctx(cx, key)?;
    let host = cx.host;
    let draft = bounded(
        host.limits.timeout(crate::limits::Class::Write),
        host.writes.propose_draft(
            &ctx,
            EditRequest {
                swimlane: args.swimlane.0,
                path: args.path.0,
                expected: args.expected.to_domain(),
                content: Bytes::from(args.content.into_string()),
                change_eol: args.change_eol,
            },
        ),
    )
    .await?;

    // The values of a draft are text from a file: scanned before they are shown (S14b).
    let mut changes: Vec<SettingChangeDto> = Vec::new();
    let mut redacted_values = 0_u32;
    for change in draft.setting_changes.iter().take(MAX_CHANGES) {
        let (dto, n) = setting_change(&host.redactor, change, VALUE_CHARS)?;
        redacted_values += n;
        changes.push(dto);
    }
    host.metrics.redacted(redacted_values);

    let setting_count = draft.setting_changes.len() as u64;
    let hunk_count = draft.hunks.len() as u64;
    let link = host.links.draft(&draft.id);
    let shape = cx.shape(true);
    let severity = draft
        .severity
        .map_or_else(String::new, |s| format!(" Severity: {s}."));
    let mut last = None;
    for tried in EFFECTS_TRIED {
        let effects: Vec<EffectDto<'_>> = draft
            .effects
            .iter()
            .take(tried)
            .map(|(service, pickup)| EffectDto {
                service,
                pickup: *pickup,
            })
            .collect();
        let more = draft.setting_changes.len() > changes.len() || draft.effects.len() > effects.len();
        let rendered = capped_result(
            &shape,
            &changes,
            more,
            |shown| {
                to_json(&DraftData {
                    draft_id: &draft.id,
                    swimlane: &draft.swimlane,
                    path: &draft.path,
                    expected: &draft.expected,
                    new_hash: &draft.new_hash,
                    setting_change_count: setting_count,
                    setting_changes: shown,
                    hunk_count,
                    severity: draft.severity,
                    effects: effects
                        .iter()
                        .map(|e| EffectDto {
                            service: e.service,
                            pickup: e.pickup,
                        })
                        .collect(),
                    expires_at: draft.expires_at,
                    untrusted: true,
                    label: DATA_LABEL,
                })
            },
            |shown| {
                format!(
                    "Draft saved: {} of {setting_count} setting changes shown, {hunk_count} hunks.{severity} Nothing was written to NFS. \
                     Apply it with apply_change; the user is asked to confirm.",
                    shown.shown
                )
            },
            link.clone(),
        );
        let fits = rendered.bytes <= shape.cap;
        last = Some(rendered);
        if fits {
            break;
        }
    }
    last.ok_or_else(|| ToolError::new(Code::Internal))
}

pub async fn apply_change(cx: &CallCx<'_>, args: ApplyChange) -> ToolResult {
    let link = cx.host.links.draft(&args.draft_id.0);
    gate(
        cx,
        Plan {
            action: Action::Apply {
                draft: args.draft_id.0,
            },
            link,
        },
    )
    .await
}

pub async fn upload_file(cx: &CallCx<'_>, args: UploadFile) -> ToolResult {
    refuse_denied(cx, [&args.path.0])?;
    let link = cx.host.links.file(&args.swimlane.0, &args.path.0);
    gate(
        cx,
        Plan {
            action: Action::Upload {
                swimlane: args.swimlane.0,
                path: args.path.0,
                expected: args.expected.to_domain(),
                content: Bytes::from(args.content.into_string()),
            },
            link,
        },
    )
    .await
}

pub async fn delete_file(cx: &CallCx<'_>, args: DeleteFile) -> ToolResult {
    refuse_denied(cx, [&args.path.0])?;
    let link = cx.host.links.file(&args.swimlane.0, &args.path.0);
    gate(
        cx,
        Plan {
            action: Action::Delete {
                swimlane: args.swimlane.0,
                path: args.path.0,
                expected: args.expected_hash.0,
            },
            link,
        },
    )
    .await
}

pub async fn raise_pr(cx: &CallCx<'_>, args: RaisePr) -> ToolResult {
    refuse_denied(cx, args.paths.iter().map(|p| &p.0))?;
    let link = cx.host.links.pull_requests(&args.swimlane.0);
    gate(
        cx,
        Plan {
            action: Action::RaisePr(PrRequest {
                swimlane: args.swimlane.0,
                paths: args.paths.into_iter().map(|p| p.0).collect(),
                title: args.title.0,
                body: args.body.map(crate::tools::args::Opaque::into_string),
                target: args.target.map(|t| t.0),
            }),
            link,
        },
    )
    .await
}

pub async fn restart_service(cx: &CallCx<'_>, args: RestartService) -> ToolResult {
    let link = cx.host.links.swimlane(&args.swimlane.0);
    gate(
        cx,
        Plan {
            action: Action::Restart(RestartRequest {
                swimlane: args.swimlane.0,
                service: args.service.0,
            }),
            link,
        },
    )
    .await
}

/// `notify_services` stays in the one safe form of T2 (a link, no write) until T7 puts it through the gate: it is off by default
/// (A2, Q37) and absent from `tools/list` while it is.
#[allow(clippy::unused_async)] // the signature of every tool; T7 puts it through the gate, which waits
pub async fn notify_services(cx: &CallCx<'_>, args: NotifyServices) -> ToolResult {
    draft_mode::answer(cx, cx.host.links.swimlane(&args.swimlane.0), false)
}
