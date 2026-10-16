//! `notify_services` (D85, D86, Q37): ask the config-server to refresh files.
//!
//! The config-server's `/update-resources` endpoint has no authentication, so this is the most carefully fenced tool of the server:
//!
//! - **Off by default.** `tools.notify_services.enabled` is `false` until Q37 is answered. While it is off the tool is absent from
//!   `tools/list` and a call is `forbidden`; the switch is read first in the call pipeline (`server::handler`), before the user, the
//!   budget or the arguments.
//! - **Operator.** The floor is the contract's, checked on every call against the directory as it is now.
//! - **Asks first.** The tool does nothing but hand the request to [`crate::confirm::gate`], which asks the person through
//!   elicitation and proceeds only on an exact accept. This file names no write method: the gate is the only caller of a mutating
//!   method of the write service, and a source scan holds it to that.
//! - **Through the write service.** The gate calls `WriteService::notify` with `Via::Mcp`. The write service audits the call (rule 8)
//!   and sends it through the swimlane's agent; the hub never calls the endpoint, and neither does this crate (it has no HTTP client).
//!
//! This is the only tool that *requests* a notification. Auto-notify (D85) is a different thing: an Admin's per-swimlane setting in
//! the web UI, which makes the write service notify after a committed write, whichever client made it. No tool changes it.
//!
//! In draft mode, and for a client that did not declare elicitation, the gate answers with a link and calls nothing.

use domain::NotifyRequest;

use crate::confirm::{Action, Plan, gate};
use crate::tools::args::NotifyServices;
use crate::tools::{CallCx, ToolResult};

pub async fn notify_services(cx: &CallCx<'_>, args: NotifyServices) -> ToolResult {
    let link = cx.host.links.swimlane(&args.swimlane.0);
    gate(
        cx,
        Plan {
            action: Action::Notify(NotifyRequest {
                swimlane: args.swimlane.0,
                paths: args.paths.into_iter().map(|p| p.0).collect(),
            }),
            link,
        },
    )
    .await
}
