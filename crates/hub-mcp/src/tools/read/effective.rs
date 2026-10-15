//! `get_effective_config` and `get_served_config`.

use domain::{
    ChannelName, ContentHash, EffectiveQuery, FileRole, LogicalFile, NfsPath, ServeRequest, SettingLocation,
    SettingPath, ShortText, SwimlaneId, TenantId, ValueType,
};
use serde::Serialize;

use super::to_json;
use crate::output::cap::take_fitting;
use crate::output::envelope::{DATA_LABEL, Parts, fit_lines, render};
use crate::output::{RedactedText, line_count};
use crate::tools::args::{GetEffectiveConfig, GetServedConfig};
use crate::tools::dto::{LIST_VALUE_CHARS, SettingValueDto, SourceDto, setting_value};
use crate::tools::error::{Code, ToolError};
use crate::tools::paging::capped_result;
use crate::tools::{CallCx, ToolResult};

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
    let effective = cx.host.registry.effective_for(cx.user, &swimlane, &query).await?;
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
                "Swimlane {swimlane}: {} of {settings_total} settings shown{}.",
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
    let withheld = served.content_withheld;

    // The text: scanned as a whole, off the async threads, and only if the file is shown at all.
    let (text, redacted_values, lines) = match (&served.text, withheld) {
        (Some(text), false) => {
            let _permit = cx
                .host
                .limits
                .reserve_bytes((text.len() as u64).saturating_mul(3), cx.byte_wait())
                .await?;
            let redactor = cx.host.redactor.clone();
            let text = text.clone();
            let redacted = tokio::task::spawn_blocking(move || redactor.text(&text))
                .await
                .map_err(|_| ToolError::new(Code::Internal))??;
            let lines = line_count(redacted.text.as_str());
            (Some(redacted.text), redacted.redacted, lines)
        }
        _ => (None, 0, 0),
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
    let shape = cx.shape(true);
    // The sources and the placeholders get a share of the cap each, and the text gets what is left.
    let (sources, sources_cut) = take_fitting(
        served
            .sources
            .iter()
            .take(16)
            .map(SourceDto::from)
            .collect::<Vec<_>>(),
        shape.cap / 6,
    );
    let (placeholders, placeholders_cut) = take_fitting(
        served
            .unresolved_placeholders
            .iter()
            .take(MAX_PLACEHOLDERS)
            .cloned()
            .collect::<Vec<ShortText>>(),
        shape.cap / 4,
    );
    let lists_cut = sources_cut || placeholders_cut;
    let build = |k: u32| {
        let shown = text.as_ref().map(|t| t.first_lines(k as usize));
        let truncated = (text.is_some() && k < lines) || lists_cut;
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
            } else if text.is_some() {
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
    if text.is_none() {
        return Ok(render(&shape, build(0)));
    }
    Ok(fit_lines(&shape, lines, build))
}
