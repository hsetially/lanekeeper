//! `search_docs`, `read_doc`, `grep_docs`, `get_docs_for_file`, `get_feature_status`.

use domain::{ChannelName, DocPath, FeatureQuery, LogicalFile, SettingPath, ShortText, SwimlaneId};
use serde::Serialize;

use super::to_json;
use crate::output::cap::cut_chars;
use crate::output::envelope::{DATA_LABEL, Parts, fit_lines};
use crate::output::{RedactError, RedactedText, Redactor, line_count};
use crate::tools::args::{GetDocsForFile, GetFeatureStatus, GrepDocs, ReadDoc, SearchDocs};
use crate::tools::dto::{CELL_VALUE_CHARS, LinesDto, SettingValueDto, setting_value};
use crate::tools::error::{Code, ToolError};
use crate::tools::paging::capped_result;
use crate::tools::{CallCx, ToolResult};

const SNIPPET_CHARS: usize = 1024;
/// The contract's `ShortText`: 256 characters.
const TITLE_CHARS: usize = 256;
const LINE_CHARS: usize = 512;
const MAX_HEADINGS: usize = 8;
const MAX_RELATED: usize = 20;
/// The most lines `read_doc` returns: the contract's 200.
const DEFAULT_LINES: u32 = 200;

#[derive(Serialize)]
struct DocHitDto {
    path: DocPath,
    /// The title and the headings are written by the author of the doc, so they are doc text and are scanned like the snippet.
    title: RedactedText,
    heading_path: Vec<RedactedText>,
    snippet: RedactedText,
    score: f64,
    related_files: Vec<LogicalFile>,
}

fn doc_hits(redactor: &Redactor, hits: &[domain::DocHit]) -> Result<(Vec<DocHitDto>, u32), RedactError> {
    let mut count = 0;
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        let redacted = redactor.text(&hit.snippet)?;
        count += redacted.redacted;
        let title = redactor.text(hit.title.as_str())?;
        count += title.redacted;
        let mut heading_path = Vec::with_capacity(hit.heading_path.len().min(MAX_HEADINGS));
        for heading in hit.heading_path.iter().take(MAX_HEADINGS) {
            let heading = redactor.text(heading.as_str())?;
            count += heading.redacted;
            heading_path.push(heading.text.truncated(TITLE_CHARS));
        }
        out.push(DocHitDto {
            path: hit.path.clone(),
            title: title.text.truncated(TITLE_CHARS),
            heading_path,
            snippet: redacted.text.truncated(SNIPPET_CHARS),
            // A score is a number between 0 and 1000 in the contract; anything else (NaN, a negative) is no score.
            score: if hit.score.is_finite() {
                f64::from(hit.score).clamp(0.0, 1000.0)
            } else {
                0.0
            },
            related_files: hit.related_files.iter().take(MAX_RELATED).cloned().collect(),
        });
    }
    Ok((out, count))
}

pub async fn search_docs(cx: &CallCx<'_>, args: SearchDocs) -> ToolResult {
    let q = args.q.0.into_string();
    let limit = args.limit.map_or(10, |l| l.0 as usize);
    let hits = cx.host.docs.search(cx.user, &q, limit).await?;
    let (items, redacted_values) = doc_hits(&cx.host.redactor, &hits)?;
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        hits: &'a [DocHitDto],
        untrusted: bool,
        label: &'static str,
    }
    Ok(capped_result(
        &cx.shape(true),
        &items,
        false,
        |shown| {
            to_json(&Data {
                hits: shown,
                untrusted: true,
                label: DATA_LABEL,
            })
        },
        |s| {
            format!(
                "{} doc hits shown{}.",
                s.shown,
                if s.more { ", more exist" } else { "" }
            )
        },
        cx.host.links.docs_search(&q),
    ))
}

pub async fn get_docs_for_file(cx: &CallCx<'_>, args: GetDocsForFile) -> ToolResult {
    let file = args.file.0;
    let setting: Option<SettingPath> = args.setting.map(|s| s.0);
    let limit = args.limit.map_or(10, |l| l.0 as usize);
    let hits = cx
        .host
        .docs
        .related_to(cx.user, &file, setting.as_ref(), limit)
        .await?;
    let (items, redacted_values) = doc_hits(&cx.host.redactor, &hits)?;
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        hits: &'a [DocHitDto],
    }
    Ok(capped_result(
        &cx.shape(true),
        &items,
        false,
        |shown| to_json(&Data { hits: shown }),
        |s| {
            format!(
                "{} doc hits shown{}.",
                s.shown,
                if s.more { ", more exist" } else { "" }
            )
        },
        cx.host.links.docs_search(file.as_str()),
    ))
}

pub async fn read_doc(cx: &CallCx<'_>, args: ReadDoc) -> ToolResult {
    let path = args.path.0;
    let range = args.lines.map(|l| l.0);
    let doc = cx.host.docs.read(cx.user, &path, range).await?;
    let _permit = cx
        .host
        .limits
        .reserve_bytes((doc.text.len() as u64).saturating_mul(3), cx.byte_wait())
        .await?;
    let redactor = cx.host.redactor.clone();
    let text = doc.text.clone();
    let redacted = tokio::task::spawn_blocking(move || redactor.text(&text))
        .await
        .map_err(|_| ToolError::new(Code::Internal))??;
    cx.host.metrics.redacted(redacted.redacted);
    let first = doc.lines.map_or(1, |r| r.start());
    let available = line_count(redacted.text.as_str()).min(DEFAULT_LINES);
    let total_in_text = line_count(redacted.text.as_str());
    let redacted_values = redacted.redacted;

    #[derive(Serialize)]
    struct Data {
        path: DocPath,
        returned_lines: Option<LinesDto>,
        total_lines: u32,
        text: RedactedText,
        untrusted: bool,
        label: &'static str,
    }
    let link = cx.host.links.doc(path.as_str());
    let shape = cx.shape(true);
    let build = |k: u32| {
        let truncated = k < total_in_text || doc.truncated;
        Parts {
            summary: if k > 0 {
                format!(
                    "Lines {first} to {} of {} returned{}.",
                    first + k - 1,
                    doc.total_lines,
                    if redacted_values > 0 {
                        format!(", {redacted_values} secret values redacted")
                    } else {
                        String::new()
                    }
                )
            } else {
                format!("{} lines in the doc; none returned.", doc.total_lines)
            },
            data: to_json(&Data {
                path: path.clone(),
                returned_lines: (k > 0).then(|| LinesDto {
                    start: first,
                    end: first + k - 1,
                }),
                total_lines: doc.total_lines,
                text: redacted.text.first_lines(k as usize),
                untrusted: true,
                label: DATA_LABEL,
            }),
            truncated,
            next: None,
            web_link: if truncated { link.clone() } else { None },
        }
    };
    Ok(fit_lines(&shape, available, build))
}

#[derive(Serialize)]
struct GrepHitDto {
    path: DocPath,
    line: u32,
    text: RedactedText,
    before: Vec<RedactedText>,
    after: Vec<RedactedText>,
}

fn grep_line(redactor: &Redactor, text: &str, count: &mut u32) -> Result<RedactedText, RedactError> {
    let redacted = redactor.text(text)?;
    *count += redacted.redacted;
    let (_, cut) = cut_chars(redacted.text.as_str(), LINE_CHARS);
    Ok(if cut {
        redacted.text.truncated(LINE_CHARS)
    } else {
        redacted.text
    })
}

pub async fn grep_docs(cx: &CallCx<'_>, args: GrepDocs) -> ToolResult {
    let pattern = args.pattern.0.into_string();
    let context = args.context.map_or(0, |c| u8::try_from(c.0).unwrap_or(5));
    let limit = args.limit.map_or(20, |l| l.0 as usize);
    let prefix = args.path_prefix.map(|p| p.0);
    let hits = cx
        .host
        .docs
        .grep_in(
            cx.user,
            &pattern,
            args.regex.unwrap_or(false),
            context,
            prefix.as_ref(),
            limit,
        )
        .await?;
    let mut redacted_values = 0_u32;
    let mut items = Vec::with_capacity(hits.len());
    for hit in &hits {
        let redactor = &cx.host.redactor;
        items.push(GrepHitDto {
            path: hit.path.clone(),
            line: hit.line,
            text: grep_line(redactor, &hit.text, &mut redacted_values)?,
            before: hit
                .before
                .iter()
                .take(5)
                .map(|l| grep_line(redactor, l, &mut redacted_values))
                .collect::<Result<_, _>>()?,
            after: hit
                .after
                .iter()
                .take(5)
                .map(|l| grep_line(redactor, l, &mut redacted_values))
                .collect::<Result<_, _>>()?,
        });
    }
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        hits: &'a [GrepHitDto],
        untrusted: bool,
        label: &'static str,
    }
    Ok(capped_result(
        &cx.shape(true),
        &items,
        false,
        |shown| {
            to_json(&Data {
                hits: shown,
                untrusted: true,
                label: DATA_LABEL,
            })
        },
        |s| {
            format!(
                "{} matching doc lines shown{}{}.",
                s.shown,
                if s.more { ", more exist" } else { "" },
                if redacted_values > 0 {
                    format!(", {redacted_values} secret values redacted")
                } else {
                    String::new()
                }
            )
        },
        cx.host.links.docs_search(&pattern),
    ))
}

#[derive(Serialize)]
struct FlagValueDto {
    swimlane: SwimlaneId,
    value: Option<SettingValueDto>,
}

#[derive(Serialize)]
struct FlagDto {
    flag: ShortText,
    file: LogicalFile,
    channel: Option<ChannelName>,
    setting: SettingPath,
    template_default: Option<SettingValueDto>,
    values: Vec<FlagValueDto>,
}

pub async fn get_feature_status(cx: &CallCx<'_>, args: GetFeatureStatus) -> ToolResult {
    let query = FeatureQuery {
        flag: args.flag.map(|f| f.0),
        file: args.file.map(|f| f.0),
        channel: args.channel.map(|c| c.0),
        swimlanes: args
            .swimlanes
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.0)
            .collect(),
    };
    let status = cx.host.docs.feature_status(cx.user, query).await?;
    let more = status.flags.len() > 50;
    let mut redacted_values = 0_u32;
    let redactor = &cx.host.redactor;
    let mut items = Vec::new();
    for flag in status.flags.iter().take(50) {
        let mut value_of =
            |v: &Option<domain::SettingValue>| -> Result<Option<SettingValueDto>, RedactError> {
                v.as_ref()
                    .map(|v| {
                        let (dto, n) = setting_value(redactor, v, CELL_VALUE_CHARS)?;
                        redacted_values += n;
                        Ok(dto)
                    })
                    .transpose()
            };
        let template_default = value_of(&flag.template_default)?;
        let mut values = Vec::new();
        for v in flag.values.iter().take(64) {
            values.push(FlagValueDto {
                swimlane: v.swimlane.clone(),
                value: value_of(&v.value)?,
            });
        }
        items.push(FlagDto {
            flag: flag.flag.clone(),
            file: flag.file.clone(),
            channel: flag.channel.clone(),
            setting: flag.setting.clone(),
            template_default,
            values,
        });
    }
    cx.host.metrics.redacted(redacted_values);

    #[derive(Serialize)]
    struct Data<'a> {
        flags: &'a [FlagDto],
    }
    Ok(capped_result(
        &cx.shape(true),
        &items,
        more,
        |shown| to_json(&Data { flags: shown }),
        |s| {
            format!(
                "{} documented flags shown{}.",
                s.shown,
                if s.more { ", more exist" } else { "" }
            )
        },
        cx.host.links.docs_search("feature flags"),
    ))
}
