//! The write tools. Each one that acts on the world ends in [`crate::confirm::gate`]; `propose_change` only records a draft.

use crate::confirm::{Plan, gate};
use crate::tools::args::{
    ApplyChange, DeleteFile, NotifyServices, ProposeChange, RaisePr, RestartService, UploadFile,
};
use crate::tools::error::{Code, ToolError};
use crate::tools::{CallCx, ToolResult};

/// `propose_change` records a draft and returns its summary (T3). Until then it answers that it is not available, and records
/// nothing.
#[allow(clippy::unused_async)]
pub async fn propose_change(_cx: &CallCx<'_>, _args: ProposeChange) -> ToolResult {
    Err(ToolError::with_detail(
        Code::Unavailable,
        "propose_change is not available yet. Make the edit in the web UI.",
    ))
}

pub async fn apply_change(cx: &CallCx<'_>, args: ApplyChange) -> ToolResult {
    gate(
        cx,
        Plan {
            link: cx.host.links.draft(&args.draft_id.0),
            with_job: false,
        },
    )
    .await
}

pub async fn upload_file(cx: &CallCx<'_>, args: UploadFile) -> ToolResult {
    gate(
        cx,
        Plan {
            link: cx.host.links.file(&args.swimlane.0, &args.path.0),
            with_job: false,
        },
    )
    .await
}

pub async fn delete_file(cx: &CallCx<'_>, args: DeleteFile) -> ToolResult {
    gate(
        cx,
        Plan {
            link: cx.host.links.file(&args.swimlane.0, &args.path.0),
            with_job: false,
        },
    )
    .await
}

pub async fn raise_pr(cx: &CallCx<'_>, args: RaisePr) -> ToolResult {
    gate(
        cx,
        Plan {
            link: cx.host.links.pull_requests(&args.swimlane.0),
            with_job: true,
        },
    )
    .await
}

pub async fn restart_service(cx: &CallCx<'_>, args: RestartService) -> ToolResult {
    gate(
        cx,
        Plan {
            link: cx.host.links.swimlane(&args.swimlane.0),
            with_job: false,
        },
    )
    .await
}

pub async fn notify_services(cx: &CallCx<'_>, args: NotifyServices) -> ToolResult {
    gate(
        cx,
        Plan {
            link: cx.host.links.swimlane(&args.swimlane.0),
            with_job: false,
        },
    )
    .await
}
