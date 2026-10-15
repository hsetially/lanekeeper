//! `get_served_config` (D82, D86, D79, S14b, S15): what the config-server actually serves to a service.
//!
//! The registry port fetches it read-only through the swimlane's agent, so this is the answer a service gets and not the engine's
//! computation (that is `get_effective_config`). The tool computes nothing: it asks the port, withholds a denied file, redacts the
//! text with the engine's secret scan, caps what is left and labels it as data. `${KEY}` is substituted by the config-server, and
//! only where the property view has the key; the keys it did not find come back as `unresolved_placeholders` and are shown as they
//! are. Nothing here reads an environment value or fills a placeholder in.
//!
//! A denied file (D79) returns `content_withheld` with `text` null and no sources or placeholders, so it reveals at most its name,
//! size and hash. The hash is the port's: where the agent gets the hash of a file it will not read is open (D92), and this tool
//! invents none.

use domain::{ChannelName, ContentHash, FileRole, NfsPath, ServeRequest, ShortText, SwimlaneId, TenantId};
use serde::Serialize;

use super::files::DEFAULT_LINES;
use super::to_json;
use crate::output::cap::take_fitting;
use crate::output::envelope::{DATA_LABEL, Parts, fit_lines, render};
use crate::output::{RedactError, RedactedText, line_count};
use crate::tools::args::GetServedConfig;
use crate::tools::dto::SourceDto;
use crate::tools::error::{Code, ToolError};
use crate::tools::{CallCx, ToolResult};

/// The most placeholders listed: the contract's `maxItems`.
const MAX_PLACEHOLDERS: usize = 100;

#[allow(clippy::too_many_lines)] // one pipeline: ask the port, shape each part, share the cap between them
pub async fn get_served_config(cx: &CallCx<'_>, args: GetServedConfig) -> ToolResult {
    let swimlane = args.swimlane.0;
    let request = ServeRequest {
        application: args.application.0,
        tenant: args.tenant.0,
        channel: args.channel.map(|c| c.0),
        file: args.file.0,
    };
    let served = cx.host.registry.served(cx.user, &swimlane, &request).await?;
    let link = cx.host.links.served(
        &swimlane,
        served.application.as_str(),
        served.tenant.as_str(),
        &served.file,
    );
    // The flag of the port, and the name of the file as the floor (D79).
    let withheld = served.content_withheld || cx.host.denied.path(&served.file);

    let shape = cx.shape(true);

    // The text: scanned as a whole, off the async threads, and only if the file is shown at all. The scan, the count of lines and
    // the cut to what a result can show all happen there, so that what comes back is at most 200 lines and at most the cap in
    // bytes (`docs/mcp-tools.md`: "200 lines or 16 KB"), however large the file is. Only that part is fitted to the cap below: the
    // fitting renders the text once per step of a binary search, and a 2 MiB file would make each step a copy of the lot.
    let (window, available, redacted_values, lines) = match (&served.text, withheld) {
        (Some(text), false) => {
            let _permit = cx
                .host
                .limits
                .reserve_bytes((text.len() as u64).saturating_mul(3), cx.byte_wait())
                .await?;
            let redactor = cx.host.redactor.clone();
            let text = text.clone();
            let cap = shape.cap;
            let (window, available, redacted, lines) = tokio::task::spawn_blocking(move || {
                let redacted = redactor.text(&text)?;
                let lines = line_count(redacted.text.as_str());
                let (window, available) = redacted.text.window(DEFAULT_LINES as usize, cap);
                Ok::<_, RedactError>((window, available, redacted.redacted, lines))
            })
            .await
            .map_err(|_| ToolError::new(Code::Internal))??;
            (Some(window), available, redacted, lines)
        }
        _ => (None, 0, 0, 0),
    };
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data {
        swimlane: SwimlaneId,
        application: domain::AppName,
        tenant: TenantId,
        channel: Option<ChannelName>,
        file: NfsPath,
        role: FileRole,
        hash: ContentHash,
        content_withheld: bool,
        text: Option<RedactedText>,
        sources: Vec<SourceDto>,
        unresolved_placeholders: Vec<ShortText>,
        redacted_values: u32,
        untrusted: bool,
        label: &'static str,
    }
    // The sources and the placeholders get a share of the cap each, and the text gets what is left.
    // A withheld file reveals its name, size and hash and nothing else: no sources and no placeholders (the contract, D79).
    let (sources, sources_cut) = take_fitting(
        served
            .sources
            .iter()
            .take(if withheld { 0 } else { 16 })
            .map(SourceDto::from)
            .collect::<Vec<_>>(),
        shape.cap / 6,
    );
    let (placeholders, placeholders_cut) = take_fitting(
        served
            .unresolved_placeholders
            .iter()
            .take(if withheld { 0 } else { MAX_PLACEHOLDERS })
            .cloned()
            .collect::<Vec<ShortText>>(),
        shape.cap / 4,
    );
    let lists_cut = sources_cut || placeholders_cut;
    let build = |k: u32| {
        let shown = window.as_ref().map(|t| t.first_lines(k as usize));
        let truncated = (window.is_some() && k < lines) || lists_cut;
        let data = Data {
            swimlane: served.swimlane.clone(),
            application: served.application.clone(),
            tenant: served.tenant.clone(),
            channel: served.channel.clone(),
            file: served.file.clone(),
            role: served.role,
            hash: served.hash,
            content_withheld: withheld,
            text: shown,
            sources: sources.clone(),
            unresolved_placeholders: placeholders.clone(),
            redacted_values,
            untrusted: true,
            label: DATA_LABEL,
        };
        Parts {
            summary: if withheld {
                format!("Swimlane {swimlane}: the served content of this file is withheld.")
            } else if window.is_some() {
                format!(
                    "Swimlane {swimlane}: {k} of {lines} lines of the served file{}.",
                    if redacted_values > 0 {
                        format!(", {redacted_values} secret values redacted")
                    } else {
                        String::new()
                    }
                )
            } else {
                format!("Swimlane {swimlane}: the served file is not text.")
            },
            data: to_json(&data),
            truncated,
            next: None,
            web_link: if truncated { link.clone() } else { None },
        }
    };
    if window.is_none() {
        return Ok(render(&shape, build(0)));
    }
    Ok(fit_lines(&shape, available, build))
}
