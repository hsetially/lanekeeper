//! The gate in front of every write tool that acts on the world (`apply_change`, `upload_file`, `delete_file`, `raise_pr`,
//! `restart_service`, `notify_services`).
//!
//! In this form it calls no write method: it answers that nothing was done and where to confirm (D52 draft mode). A write tool
//! therefore cannot write by way of an MCP call until the elicitation of T3 stands in front of its write.

use serde_json::{Value, json};

use crate::output::envelope::{Parts, render};
use crate::tools::{CallCx, ToolResult};

/// What a write tool wants to do, in terms the gate can present.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The page of the web UI where the person confirms.
    pub link: Option<String>,
    /// Some tools have a second field in their result (`raise_pr` has `job`), which is `null` when nothing was started.
    pub with_job: bool,
}

/// The `result` of a write that was not done and is left for the web UI.
#[must_use]
pub fn draft_link_outcome() -> Value {
    json!({"outcome": "draft_link", "hash": null, "audit_id": null, "proposal_id": null})
}

/// Gate a write: in this form, always the draft link.
///
/// # Errors
///
/// None in this form; the signature is the one T3 needs (a confirmation can fail).
#[allow(clippy::unused_async)]
pub async fn gate(cx: &CallCx<'_>, plan: Plan) -> ToolResult {
    let mut data = json!({"result": draft_link_outcome()});
    if plan.with_job {
        data["job"] = Value::Null;
    }
    Ok(render(
        &cx.shape(false),
        Parts {
            summary: "Nothing was done. Confirm this change in the web UI.".to_owned(),
            data,
            truncated: false,
            next: None,
            web_link: plan.link,
        },
    ))
}
