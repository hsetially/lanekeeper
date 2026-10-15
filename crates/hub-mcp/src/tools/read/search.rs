//! `search_settings` and `search_text`.

use domain::{NfsPath, SettingLocation, SettingPath, SettingsQuery, SwimlaneId, TextQuery};
use serde::Serialize;

use super::to_json;
use crate::output::RedactedText;
use crate::output::cap::cut_chars;
use crate::output::envelope::DATA_LABEL;
use crate::tools::args::{SearchSettings, SearchText};
use crate::tools::dto::{LIST_VALUE_CHARS, SettingValueDto, setting_value};
use crate::tools::paging::{Resume, Start, capped_result, paged_result};
use crate::tools::{CallCx, ToolResult};

#[derive(Serialize)]
struct SettingHitDto {
    swimlane: SwimlaneId,
    file: NfsPath,
    path: SettingPath,
    value: SettingValueDto,
    location: SettingLocation,
}

pub async fn search_settings(cx: &CallCx<'_>, args: SearchSettings) -> ToolResult {
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let q = args.q.0;
    let query = SettingsQuery {
        path_contains: q.clone(),
        swimlanes: args
            .swimlanes
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.0)
            .collect(),
        file: args.file.map(|f| f.0),
        page: start.port_page()?,
    };
    let paged = cx.host.registry.search_settings(cx.user, query).await?;
    let port_next = paged.next.map(|c| c.as_str().to_owned());
    let mut redacted_values = 0_u32;
    let mut items = Vec::new();
    // A hit in a denied file is left out (D79). The hits that are left are what the cursor counts, so the page still ends where the
    // next one starts: the filter is the same on every call for the same port page.
    for hit in paged
        .items
        .iter()
        .filter(|hit| !cx.host.denied.path(&hit.file))
        .skip(start.skip as usize)
    {
        let (value, n) = setting_value(&cx.host.redactor, &hit.value, LIST_VALUE_CHARS)?;
        redacted_values += n;
        items.push(SettingHitDto {
            swimlane: hit.swimlane.clone(),
            file: hit.file.clone(),
            path: hit.path.clone(),
            value,
            location: hit.location.clone(),
        });
    }
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        hits: &'a [SettingHitDto],
    }
    Ok(paged_result(
        &cx.shape(true),
        cx.spec.name(),
        &start,
        &Resume::Port { port_next },
        &items,
        |shown| to_json(&Data { hits: shown }),
        |s| {
            format!(
                "{} setting hits shown{}{}.",
                s.shown,
                if s.more { ", more follow" } else { "" },
                if redacted_values > 0 {
                    format!(", {redacted_values} secret values redacted")
                } else {
                    String::new()
                }
            )
        },
        cx.host.links.search_settings(q.as_str()),
    ))
}

/// The longest matching line in a hit: the contract's `maxLength`.
const HIT_CHARS: usize = 512;
/// The most ranges of one hit.
const MAX_RANGES: usize = 20;

#[derive(Serialize)]
struct TextHitDto {
    swimlane: SwimlaneId,
    path: NfsPath,
    line: u32,
    text: RedactedText,
    ranges: Vec<[u32; 2]>,
}

pub async fn search_text(cx: &CallCx<'_>, args: SearchText) -> ToolResult {
    let pattern = args.pattern.0.into_string();
    let query = TextQuery {
        pattern: pattern.clone(),
        regex: args.regex.unwrap_or(false),
        case_sensitive: args.case_sensitive.unwrap_or(false),
        swimlanes: args
            .swimlanes
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.0)
            .collect(),
        path_glob: args.path_glob.map(|g| g.0),
        max_total: args.max_total.map_or(50, |n| n.0),
        max_per_file: args.max_per_file.map_or(5, |n| n.0),
    };
    let results = cx.host.registry.search_text(cx.user, query).await?;
    let mut redacted_values = 0_u32;
    let mut items = Vec::with_capacity(results.hits.len());
    for hit in results.hits.iter().filter(|hit| !cx.host.denied.path(&hit.path)) {
        let redacted = cx.host.redactor.text(&hit.text)?;
        redacted_values += redacted.redacted;
        // The byte ranges are positions in the line as the engine found it. They stay true only while the text is untouched: a
        // replaced secret moves everything after it, so a hit that was redacted has no ranges rather than wrong ones.
        let (head, was_cut) = cut_chars(redacted.text.as_str(), HIT_CHARS);
        let kept = head.len();
        let ranges: Vec<[u32; 2]> = if redacted.redacted > 0 {
            Vec::new()
        } else {
            hit.ranges
                .iter()
                .filter(|(start, end)| *start <= *end && (*end as usize) <= kept)
                .take(MAX_RANGES)
                .map(|(start, end)| [(*start).min(100_000), (*end).min(100_000)])
                .collect()
        };
        let text = if was_cut {
            redacted.text.truncated(HIT_CHARS)
        } else {
            redacted.text
        };
        items.push(TextHitDto {
            swimlane: hit.swimlane.clone(),
            path: hit.path.clone(),
            line: hit.line,
            text,
            ranges,
        });
    }
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        hits: &'a [TextHitDto],
        untrusted: bool,
        label: &'static str,
    }
    Ok(capped_result(
        &cx.shape(true),
        &items,
        results.truncated,
        |shown| {
            to_json(&Data {
                hits: shown,
                untrusted: true,
                label: DATA_LABEL,
            })
        },
        |s| {
            format!(
                "{} matching lines shown{}{}.",
                s.shown,
                if s.more {
                    ", more exist; narrow the search or open the link"
                } else {
                    ""
                },
                if redacted_values > 0 {
                    format!(", {redacted_values} secret values redacted")
                } else {
                    String::new()
                }
            )
        },
        cx.host.links.search_text(&pattern),
    ))
}
