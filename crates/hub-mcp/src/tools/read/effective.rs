//! `get_effective_config`: the engine's view of a logical file. What the config-server really serves is `get_served_config`
//! ([`super::served`]).

use domain::{
    ChannelName, ContentHash, EffectiveQuery, FileRole, LogicalFile, NfsPath, SettingLocation, SettingPath,
    ShortText, SwimlaneId, TenantId, ValueType,
};
use serde::Serialize;

use super::to_json;
use crate::output::RedactedText;
use crate::output::cap::take_fitting;
use crate::output::envelope::DATA_LABEL;
use crate::tools::args::GetEffectiveConfig;
use crate::tools::dto::{LIST_VALUE_CHARS, SettingValueDto, SourceDto, setting_value};
use crate::tools::error::ToolError;
use crate::tools::paging::capped_result;
use crate::tools::{CallCx, ToolResult};

/// What the summary adds when the settings shown are no answer to "what is in this file": a resource file (a body, not settings),
/// or a file with no settings. This view returns no `text` (D92b), so "0 of 0 settings shown" would read as an empty file. Fixed text
/// of this crate; it holds nothing of the request or the file.
const BODY_NOTE: &str = " This is not an empty file: its body is not part of this view, and `get_served_config` returns what the \
                         config-server serves for it (a resource file is not settings).";

/// The most settings listed: the contract's `maxItems`.
const MAX_SETTINGS: usize = 200;
/// The most sources and placeholders listed.
const MAX_SOURCES: usize = 50;
const MAX_PLACEHOLDERS: usize = 100;

#[derive(Serialize)]
struct EffectiveSettingDto {
    path: SettingPath,
    value: SettingValueDto,
    source: NfsPath,
    location: Option<SettingLocation>,
}

#[allow(clippy::too_many_lines)] // one pipeline: ask the port, shape each part, share the cap between them
pub async fn get_effective_config(cx: &CallCx<'_>, args: GetEffectiveConfig) -> ToolResult {
    let swimlane = args.swimlane.0;
    let query = EffectiveQuery {
        file: args.file.0,
        tenant: args.tenant.0,
        channel: args.channel.map(|c| c.0),
        setting_prefix: args.setting_prefix.map(|p| p.0),
    };
    // The values of a denied file are its content (D79): not asked for, and not shown if they come back.
    if cx.host.denied.file(&query.file) {
        return Err(ToolError::denied());
    }
    let effective = cx.host.registry.effective_for(cx.user, &swimlane, &query).await?;
    // Only what is shown is looked at (the first settings and the first sources), which bounds the cost of the check.
    if cx.host.denied.file(&effective.file)
        || effective
            .settings
            .iter()
            .take(MAX_SETTINGS)
            .any(|s| cx.host.denied.path(&s.source))
        || effective
            .sources
            .iter()
            .take(MAX_SOURCES)
            .any(|s| cx.host.denied.path(&s.path))
    {
        return Err(ToolError::denied());
    }
    let settings_total = effective.settings.len();
    let more = settings_total > MAX_SETTINGS;
    let mut redacted_values = 0_u32;
    let mut items = Vec::with_capacity(settings_total.min(MAX_SETTINGS));
    for setting in effective.settings.iter().take(MAX_SETTINGS) {
        // The port gives the text of a value and not its type or its flag, so they are not claimed (see the blockers file).
        let value = domain::SettingValue {
            text: setting.value.clone(),
            ty: ValueType::Other,
            flag: None,
        };
        let (dto, n) = setting_value(&cx.host.redactor, &value, LIST_VALUE_CHARS)?;
        redacted_values += n;
        items.push(EffectiveSettingDto {
            path: setting.path.clone(),
            value: dto,
            source: setting.source.clone(),
            location: setting.location.clone(),
        });
    }
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        swimlane: &'a SwimlaneId,
        file: &'a LogicalFile,
        tenant: &'a TenantId,
        channel: &'a Option<ChannelName>,
        role: FileRole,
        hash: ContentHash,
        sources: Vec<SourceDto>,
        settings: &'a [EffectiveSettingDto],
        settings_total: u64,
        text: Option<RedactedText>,
        unresolved_placeholders: Vec<&'a ShortText>,
        untrusted: bool,
        label: &'static str,
    }
    // The sources and the placeholders are not what this result pages, so each gets a share of the cap (a quarter), and the
    // settings get what is left.
    let shape = cx.shape(true);
    let (sources, sources_cut) = take_fitting(
        effective
            .sources
            .iter()
            .take(MAX_SOURCES)
            .map(SourceDto::from)
            .collect::<Vec<_>>(),
        shape.cap / 4,
    );
    let (placeholders, placeholders_cut) = take_fitting(
        effective
            .unresolved_placeholders
            .iter()
            .take(MAX_PLACEHOLDERS)
            .collect::<Vec<&ShortText>>(),
        shape.cap / 4,
    );
    let more = more
        || sources_cut
        || placeholders_cut
        || effective.sources.len() > MAX_SOURCES
        || effective.unresolved_placeholders.len() > MAX_PLACEHOLDERS;
    let link = cx
        .host
        .links
        .effective(&swimlane, &effective.file, effective.tenant.as_str());
    let body_note = if effective.role == FileRole::Resource || settings_total == 0 {
        BODY_NOTE
    } else {
        ""
    };
    Ok(capped_result(
        &shape,
        &items,
        more,
        |shown| {
            to_json(&Data {
                swimlane: &effective.swimlane,
                file: &effective.file,
                tenant: &effective.tenant,
                channel: &effective.channel,
                role: effective.role,
                hash: effective.hash,
                sources: sources.clone(),
                settings: shown,
                settings_total: settings_total as u64,
                text: None,
                unresolved_placeholders: placeholders.clone(),
                untrusted: true,
                label: DATA_LABEL,
            })
        },
        |s| {
            format!(
                "Swimlane {swimlane}: {} of {settings_total} settings shown{}.{body_note}",
                s.shown,
                if redacted_values > 0 {
                    format!(", {redacted_values} secret values redacted")
                } else {
                    String::new()
                }
            )
        },
        link,
    ))
}
