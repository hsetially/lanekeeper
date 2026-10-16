//! Draft mode (D52): a write tool that does nothing and says where the user confirms.
//!
//! It applies when the deployment asks for it (`LK_MCP_CONFIRM=draft`) and when the client did not declare that it can show a form.
//! **It touches no port**: no write, no preview, no draft lookup. The answer is a link, and the person confirms in the web UI,
//! where the change is shown and audited as any other.

use serde_json::{Value, json};

use crate::output::envelope::{Parts, render};
use crate::tools::{CallCx, ToolResult};

/// The `result` of a write that was not done and is left for the web UI.
#[must_use]
pub fn draft_link_outcome() -> Value {
    json!({"outcome": "draft_link", "hash": null, "audit_id": null, "proposal_id": null})
}

/// The answer of a write tool in draft mode. `with_job`: the tool has a second field, `job` (`raise_pr`), which is `null` because
/// nothing was started.
pub fn answer(cx: &CallCx<'_>, link: Option<String>, with_job: bool) -> ToolResult {
    let mut data = json!({"result": draft_link_outcome()});
    if with_job {
        data["job"] = Value::Null;
    }
    Ok(render(
        &cx.shape(false),
        Parts {
            summary: "Nothing was done. Confirm this change in the web UI.".to_owned(),
            data,
            truncated: false,
            next: None,
            web_link: link,
        },
    ))
}
