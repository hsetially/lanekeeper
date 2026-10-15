//! `list_files`, `read_file`, `get_drift`, `get_history`.

use bytes::Bytes;
use domain::{ContentHash, ContentSource, DriftQuery, FileClass, NfsPath, SettingLocation};
use engine::{Limits, Syntax, decode_text, normalise_eol};
use serde::Serialize;

use super::to_json;
use crate::output::envelope::{Parts, Rendered, fit_lines, render};
use crate::output::{Redacted, RedactedText, line_count};
use crate::tools::args::{GetDrift, GetHistory, ListFiles, ReadFile, SourceArg};
use crate::tools::cache::{FileCache, MAX_OUTLINE};
use crate::tools::dto::{FileEntryDto, LinesDto, VersionDto, safe};
use crate::tools::error::{Code, ToolError};
use crate::tools::paging::{Resume, Start, paged_result};
use crate::tools::{CallCx, ToolResult};

/// The most lines a default read returns: the contract's 200.
pub const DEFAULT_LINES: u32 = 200;
/// A file over this many bytes (as text) is large: it gets an outline and its first lines (D58).
pub const LARGE_BYTES: usize = 16 * 1024;

pub async fn list_files(cx: &CallCx<'_>, args: ListFiles) -> ToolResult {
    let swimlane = args.swimlane.0;
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let page = start.port_page()?;
    let prefix = args.prefix.map_or_else(NfsPath::root, |p| p.0);
    let (paged, link) = if let Some(state) = args.drift {
        let query = DriftQuery {
            state: Some(state),
            prefix: (!prefix.is_root()).then(|| prefix.clone()),
        };
        (
            cx.host.registry.drift(cx.user, &swimlane, &query, page).await?,
            cx.host.links.drift(&swimlane),
        )
    } else {
        (
            cx.host.registry.tree(cx.user, &swimlane, &prefix, page).await?,
            cx.host.links.file(&swimlane, &prefix),
        )
    };
    let port_next = paged.next.map(|c| c.as_str().to_owned());
    let items: Vec<FileEntryDto> = paged
        .items
        .iter()
        .skip(start.skip as usize)
        .map(FileEntryDto::from)
        .collect();
    Ok(files_page(
        cx, &start, port_next, &items, link, &swimlane, "entries",
    ))
}

pub async fn get_drift(cx: &CallCx<'_>, args: GetDrift) -> ToolResult {
    let swimlane = args.swimlane.0;
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let page = start.port_page()?;
    let query = DriftQuery {
        state: args.state,
        prefix: args.prefix.map(|p| p.0).filter(|p| !p.is_root()),
    };
    let paged = cx.host.registry.drift(cx.user, &swimlane, &query, page).await?;
    let port_next = paged.next.map(|c| c.as_str().to_owned());
    let items: Vec<FileEntryDto> = paged
        .items
        .iter()
        .skip(start.skip as usize)
        .map(FileEntryDto::from)
        .collect();
    Ok(files_page(
        cx,
        &start,
        port_next,
        &items,
        cx.host.links.drift(&swimlane),
        &swimlane,
        "files",
    ))
}

fn files_page(
    cx: &CallCx<'_>,
    start: &Start,
    port_next: Option<String>,
    items: &[FileEntryDto],
    link: Option<String>,
    swimlane: &domain::SwimlaneId,
    noun: &'static str,
) -> Rendered {
    #[derive(Serialize)]
    struct Data<'a> {
        files: &'a [FileEntryDto],
    }
    paged_result(
        &cx.shape(false),
        cx.spec.name(),
        start,
        &Resume::Port { port_next },
        items,
        |shown| to_json(&Data { files: shown }),
        |s| {
            format!(
                "Swimlane {swimlane}: {} {noun} shown{}.",
                s.shown,
                if s.more { ", more follow" } else { "" }
            )
        },
        link,
    )
}

pub async fn get_history(cx: &CallCx<'_>, args: GetHistory) -> ToolResult {
    let swimlane = args.swimlane.0;
    let path = args.path.0;
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let page = start.port_page()?;
    let paged = cx.host.registry.history(cx.user, &swimlane, &path, page).await?;
    let port_next = paged.next.map(|c| c.as_str().to_owned());
    let items: Vec<VersionDto> = paged
        .items
        .iter()
        .skip(start.skip as usize)
        .map(VersionDto::from)
        .collect();

    #[derive(Serialize)]
    struct Data<'a> {
        versions: &'a [VersionDto],
    }
    Ok(paged_result(
        &cx.shape(false),
        cx.spec.name(),
        &start,
        &Resume::Port { port_next },
        &items,
        |shown| to_json(&Data { versions: shown }),
        |s| {
            format!(
                "Swimlane {swimlane}: {} versions shown, newest first{}.",
                s.shown,
                if s.more { ", more follow" } else { "" }
            )
        },
        cx.host.links.history(&swimlane, &path),
    ))
}

// ---------------------------------------------------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct FileData {
    path: NfsPath,
    hash: ContentHash,
    class: FileClass,
    size: u64,
    total_lines: Option<u32>,
    content_withheld: bool,
    redacted_values: u32,
    returned_lines: Option<LinesDto>,
    outline: Vec<SettingLocation>,
    text: RedactedText,
    untrusted: bool,
    label: &'static str,
}

/// What was worked out about a file in the blocking step.
struct Prepared {
    total_lines: u32,
    /// The lines the window holds, 1-based and inclusive, or `None` for an empty file.
    window: Option<(u32, u32)>,
    /// Something that was asked for, or that the file has, is not in the window (the cap of 200 lines, or the default view).
    cut: bool,
    outline: Vec<SettingLocation>,
    text: Redacted,
    cache_hit: bool,
}

struct Wanted {
    lines: Option<domain::LineRange>,
    setting_path: Option<domain::SettingPath>,
    syntax: Option<Syntax>,
}

fn detail(code: Code, text: &'static str) -> ToolError {
    ToolError::with_detail(code, text)
}

/// The blocking part of a read: decode, scan, locate, and cut the window. Runs from `spawn_blocking`.
fn prepare(
    bytes: &Bytes,
    key: &ContentHash,
    wanted: &Wanted,
    cache: &FileCache,
    redactor: &crate::output::Redactor,
) -> Result<Prepared, ToolError> {
    let normalised = normalise_eol(bytes);
    let decoded = decode_text(&normalised, &Limits::DEFAULT).map_err(|e| match e {
        engine::DecodeError::TooLarge => ToolError::new(Code::PayloadTooLarge),
        _ => detail(Code::Unprocessable, "The file is not valid text in its encoding."),
    })?;
    let text = decoded.text.as_ref();
    let total_lines = line_count(text);

    // The window.
    let mut outline = Vec::new();
    let mut cut = false;
    let mut cache_hit = true;
    let window: Option<(u32, u32)> = if total_lines == 0 {
        if wanted.lines.is_some() || wanted.setting_path.is_some() {
            return Err(detail(Code::InvalidArgument, "The file is empty."));
        }
        None
    } else if let Some(range) = wanted.lines {
        if range.start() > total_lines {
            return Err(detail(
                Code::InvalidArgument,
                "The file has fewer lines than the range asks for.",
            ));
        }
        let end = range
            .end()
            .min(total_lines)
            .min(range.start().saturating_add(DEFAULT_LINES - 1));
        cut = end < range.end().min(total_lines);
        Some((range.start(), end))
    } else if let Some(setting) = &wanted.setting_path {
        let syntax = wanted.syntax.ok_or_else(|| {
            detail(
                Code::InvalidArgument,
                "`setting_path` applies to structured files (YAML, JSON, properties).",
            )
        })?;
        let (index, hit) = cache.index(key, syntax, &normalised).map_err(|_| {
            detail(
                Code::Unprocessable,
                "The file does not parse, so its settings cannot be located.",
            )
        })?;
        cache_hit = hit;
        let range = index
            .locate(setting)
            .ok_or_else(|| ToolError::with_detail(Code::NotFound, "The file has no such setting."))?;
        let end = range
            .end()
            .min(total_lines)
            .min(range.start().saturating_add(DEFAULT_LINES - 1));
        cut = end < range.end().min(total_lines);
        Some((
            range.start().min(total_lines),
            end.max(range.start().min(total_lines)),
        ))
    } else if total_lines > DEFAULT_LINES || text.len() > LARGE_BYTES {
        // A large file: its outline, and its first lines.
        if let Some(syntax) = wanted.syntax
            && let Ok((index, hit)) = cache.index(key, syntax, &normalised)
        {
            outline = index.outline().iter().take(MAX_OUTLINE).cloned().collect();
            cache_hit = hit;
        }
        cut = true;
        Some((1, total_lines.min(DEFAULT_LINES)))
    } else {
        Some((1, total_lines))
    };

    let scan = cache.scan(key, text, redactor).map_err(ToolError::from)?;
    cache_hit &= scan.1;
    let redacted = match window {
        Some((first, last)) => redactor.window(text, &scan.0.matches, first, last),
        None => redactor.window(text, &[], 1, 0),
    };
    Ok(Prepared {
        total_lines,
        window,
        cut,
        outline,
        text: redacted,
        cache_hit,
    })
}

#[allow(clippy::too_many_lines)] // one pipeline: ask the port, shape each part, share the cap between them
pub async fn read_file(cx: &CallCx<'_>, args: ReadFile) -> ToolResult {
    if args.lines.is_some() && args.setting_path.is_some() {
        return Err(detail(
            Code::InvalidArgument,
            "`lines` and `setting_path` cannot be used together.",
        ));
    }
    let swimlane = args.swimlane.0;
    let path = args.path.0;
    let source = match args.source.unwrap_or(SourceArg::Nfs) {
        SourceArg::Nfs | SourceArg::Baseline if args.repo.is_some() || args.git_ref.is_some() => {
            return Err(detail(
                Code::InvalidArgument,
                "`repo` and `ref` apply only with `source: git`.",
            ));
        }
        SourceArg::Nfs => ContentSource::Nfs,
        SourceArg::Baseline => ContentSource::Baseline,
        SourceArg::Git => match (args.repo, args.git_ref) {
            (Some(repo), Some(git_ref)) => ContentSource::Git {
                repo,
                git_ref: git_ref.0,
            },
            _ => {
                return Err(detail(
                    Code::InvalidArgument,
                    "`source: git` needs `repo` and `ref`.",
                ));
            }
        },
    };
    let content = cx
        .host
        .registry
        .content(cx.user, &swimlane, &path, source)
        .await?;
    let shape = cx.shape(true);
    let link = cx.host.links.file(&swimlane, &path);
    let summary_of = |text: String| text;

    // A denied file is never read: its class, or the port's flag, is the rule (D79).
    if content.withheld || content.class == FileClass::Denied {
        let data = FileData {
            path: content.path.clone(),
            hash: content.hash,
            class: content.class,
            size: safe(content.size),
            total_lines: None,
            content_withheld: true,
            redacted_values: 0,
            returned_lines: None,
            outline: Vec::new(),
            text: cx.host.redactor.fixed("content withheld"),
            untrusted: true,
            label: crate::output::envelope::DATA_LABEL,
        };
        return Ok(render(
            &shape,
            Parts {
                summary: summary_of(format!(
                    "Swimlane {swimlane}: the content of this file is withheld."
                )),
                data: to_json(&data),
                truncated: false,
                next: None,
                web_link: link,
            },
        ));
    }
    if content.class == FileClass::Binary {
        let data = FileData {
            path: content.path.clone(),
            hash: content.hash,
            class: content.class,
            size: safe(content.size),
            total_lines: None,
            content_withheld: false,
            redacted_values: 0,
            returned_lines: None,
            outline: Vec::new(),
            text: cx.host.redactor.fixed("binary file: not shown as text"),
            untrusted: true,
            label: crate::output::envelope::DATA_LABEL,
        };
        return Ok(render(
            &shape,
            Parts {
                summary: summary_of(format!(
                    "Swimlane {swimlane}: a binary file of {} bytes.",
                    content.size
                )),
                data: to_json(&data),
                truncated: false,
                next: None,
                web_link: link,
            },
        ));
    }
    let bytes = content
        .bytes
        .clone()
        .ok_or_else(|| ToolError::new(Code::Unavailable))?;

    // The budget of file bytes in flight: a file costs three times its size while it is decoded.
    let _bytes_permit = cx
        .host
        .limits
        .reserve_bytes((bytes.len() as u64).saturating_mul(3), cx.byte_wait())
        .await?;
    let wanted = Wanted {
        lines: args.lines.map(|l| l.0),
        setting_path: args.setting_path.map(|s| s.0),
        syntax: Syntax::of_path(&path),
    };
    let cache = cx.host.files.clone();
    let redactor = cx.host.redactor.clone();
    let prepared = {
        let bytes = bytes.clone();
        tokio::task::spawn_blocking(move || {
            let key = ports::content_hash(&bytes);
            prepare(&bytes, &key, &wanted, &cache, &redactor)
        })
        .await
        .map_err(|_| ToolError::new(Code::Internal))??
    };
    cx.host
        .metrics
        .cache(if prepared.cache_hit { "hit" } else { "miss" });
    cx.host.metrics.redacted(prepared.text.redacted);

    let Prepared {
        total_lines,
        window,
        cut,
        outline,
        text,
        ..
    } = prepared;
    // The outline shares the cap with the text: at most a third of it, so that the first lines always have room.
    let outline_fit = {
        let sizes = crate::output::cap::sizes_of(&outline);
        crate::output::cap::fit(&sizes, shape.cap / 3)
    };
    let outline_cut = outline_fit < outline.len();
    let outline: Vec<SettingLocation> = outline.into_iter().take(outline_fit).collect();
    let cut = cut || outline_cut;
    let available = window.map_or(0, |(a, b)| b - a + 1);
    let class = content.class;
    let size = safe(content.size);
    let hash = content.hash;
    let redacted_total = text.redacted;
    let build = |k: u32| {
        let kept = text.text.first_lines(k as usize);
        let returned = window.filter(|_| k > 0).map(|(a, _)| LinesDto {
            start: a,
            end: a + k - 1,
        });
        let truncated = cut || k < available;
        let data = FileData {
            path: path.clone(),
            hash,
            class,
            size,
            total_lines: Some(total_lines),
            content_withheld: false,
            redacted_values: redacted_total,
            returned_lines: returned,
            outline: outline.clone(),
            text: kept,
            untrusted: true,
            label: crate::output::envelope::DATA_LABEL,
        };
        Parts {
            summary: summary_of(match window {
                Some((a, _)) if k > 0 => format!(
                    "Swimlane {swimlane}: lines {a} to {} of {total_lines} returned{}.",
                    a + k - 1,
                    if redacted_total > 0 {
                        format!(", {redacted_total} secret values redacted")
                    } else {
                        String::new()
                    }
                ),
                _ => format!("Swimlane {swimlane}: {total_lines} lines in the file; none returned."),
            }),
            data: to_json(&data),
            truncated,
            next: None,
            web_link: if truncated { link.clone() } else { None },
        }
    };
    Ok(fit_lines(&shape, available, build))
}
