//! `compare`, `compare_tree`, `compare_grid`.

use domain::{
    BinaryCompare, CompareQuery, CompareRef, FileClass, GridQuery, LogicalFile, SettingLocation, SwimlaneId,
};
use serde::Serialize;

use super::to_json;
use crate::output::RedactError;
use crate::output::envelope::{DATA_LABEL, Parts, fit_lines};
use crate::tools::args::{Compare, CompareGrid, CompareTree, DetailArg};
use crate::tools::dto::{
    CELL_VALUE_CHARS, DiffHunkDto, LIST_VALUE_CHARS, SettingChangeDto, SettingValueDto, TreeEntryDto,
    diff_hunk, safe, setting_change, setting_value,
};
use crate::tools::paging::{Resume, Start, capped_result, paged_result};
use crate::tools::{CallCx, ToolResult};

/// The most setting changes and hunks listed: the contract's `maxItems`.
const MAX_CHANGES: usize = 100;
const MAX_HUNKS: usize = 20;

#[derive(Serialize, Clone, Copy)]
struct BinaryDto {
    left: Option<domain::ContentHash>,
    right: Option<domain::ContentHash>,
    left_size: Option<u64>,
    right_size: Option<u64>,
}

impl From<&BinaryCompare> for BinaryDto {
    fn from(b: &BinaryCompare) -> Self {
        Self {
            left: b.left,
            right: b.right,
            left_size: b.left_size.map(safe),
            right_size: b.right_size.map(safe),
        }
    }
}

#[allow(clippy::too_many_lines)] // one pipeline: ask the port, shape each part, share the cap between them
pub async fn compare(cx: &CallCx<'_>, args: Compare) -> ToolResult {
    let query = CompareQuery {
        left: args.left.0.clone(),
        right: args.right.0.clone(),
        file: Some(args.file.0.clone()),
        channel: args.channel.map(|c| c.0),
    };
    let comparison = cx.host.registry.compare_in(cx.user, &query).await?;
    let settings_view = match args.detail.unwrap_or(DetailArg::Auto) {
        DetailArg::Settings => true,
        DetailArg::Lines => false,
        DetailArg::Auto => comparison.class == FileClass::Structured,
    };
    let redactor = &cx.host.redactor;
    let mut redacted_values = 0_u32;
    let mut changes: Vec<SettingChangeDto> = Vec::new();
    let mut hunks: Vec<DiffHunkDto> = Vec::new();
    if settings_view {
        for change in comparison.setting_changes.iter().take(MAX_CHANGES) {
            let (dto, n) = setting_change(redactor, change, LIST_VALUE_CHARS)?;
            redacted_values += n;
            changes.push(dto);
        }
    } else {
        for hunk in comparison.hunks.iter().take(MAX_HUNKS) {
            let (dto, n) = diff_hunk(redactor, hunk)?;
            redacted_values += n;
            hunks.push(dto);
        }
    }
    cx.host.metrics.redacted(redacted_values);
    let link = comparison.file.as_ref().and_then(|f| {
        cx.host
            .links
            .compare(&comparison.left.to_string(), &comparison.right.to_string(), f)
    });

    #[derive(Serialize)]
    struct Data<'a> {
        left: &'a CompareRef,
        right: &'a CompareRef,
        file: &'a Option<LogicalFile>,
        class: FileClass,
        identical: bool,
        eol_only: bool,
        setting_change_count: u64,
        setting_changes: &'a [SettingChangeDto],
        hunk_count: u64,
        hunks: &'a [DiffHunkDto],
        binary: Option<BinaryDto>,
        untrusted: bool,
        label: &'static str,
    }
    let setting_change_count = comparison.setting_changes.len() as u64;
    let hunk_count = comparison.hunks.len() as u64;
    let binary = comparison.binary.as_ref().map(BinaryDto::from);
    let summary = |shown: usize, more: bool| {
        let what = if settings_view {
            format!("{shown} of {setting_change_count} setting changes")
        } else {
            format!("{shown} of {hunk_count} hunks")
        };
        let state = if comparison.identical {
            "The files are identical"
        } else if comparison.eol_only {
            "The files differ only in line endings"
        } else {
            "The files differ"
        };
        format!(
            "{state}; {what} shown{}{}.",
            if more { ", more exist" } else { "" },
            if redacted_values > 0 {
                format!(", {redacted_values} secret values redacted")
            } else {
                String::new()
            }
        )
    };
    let shape = cx.shape(true);
    if settings_view {
        let more = comparison.setting_changes.len() > MAX_CHANGES;
        Ok(capped_result(
            &shape,
            &changes,
            more,
            |shown| {
                to_json(&Data {
                    left: &comparison.left,
                    right: &comparison.right,
                    file: &comparison.file,
                    class: comparison.class,
                    identical: comparison.identical,
                    eol_only: comparison.eol_only,
                    setting_change_count,
                    setting_changes: shown,
                    hunk_count,
                    hunks: &[],
                    binary,
                    untrusted: true,
                    label: DATA_LABEL,
                })
            },
            |s| summary(s.shown, s.more),
            link,
        ))
    } else {
        // Hunks are cut by their lines, not as whole hunks, since one hunk can be longer than the whole result.
        let total_lines: usize = hunks.iter().map(|h| h.lines.len()).sum();
        let more_hunks = comparison.hunks.len() > MAX_HUNKS;
        let build = |k: u32| {
            let mut left = k as usize;
            let mut shown: Vec<DiffHunkDto> = Vec::new();
            for hunk in &hunks {
                if left == 0 {
                    break;
                }
                let take = hunk.lines.len().min(left);
                left -= take;
                shown.push(DiffHunkDto {
                    old_start: hunk.old_start,
                    old_lines: hunk.old_lines,
                    new_start: hunk.new_start,
                    new_lines: hunk.new_lines,
                    lines: hunk.lines.iter().take(take).cloned().collect(),
                });
            }
            let truncated = (k as usize) < total_lines || more_hunks;
            Parts {
                summary: summary(shown.len(), truncated),
                data: to_json(&Data {
                    left: &comparison.left,
                    right: &comparison.right,
                    file: &comparison.file,
                    class: comparison.class,
                    identical: comparison.identical,
                    eol_only: comparison.eol_only,
                    setting_change_count,
                    setting_changes: &[],
                    hunk_count,
                    hunks: &shown,
                    binary,
                    untrusted: true,
                    label: DATA_LABEL,
                }),
                truncated,
                next: None,
                web_link: if truncated { link.clone() } else { None },
            }
        };
        Ok(fit_lines(
            &shape,
            u32::try_from(total_lines).unwrap_or(u32::MAX),
            build,
        ))
    }
}

pub async fn compare_tree(cx: &CallCx<'_>, args: CompareTree) -> ToolResult {
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let page = start.port_page()?;
    let (left, right) = (args.left.0, args.right.0);
    let result = match args.prefix.map(|p| p.0).filter(|p| !p.is_root()) {
        Some(prefix) => {
            cx.host
                .registry
                .compare_tree_under(cx.user, left.clone(), right.clone(), &prefix, page)
                .await?
        }
        None => {
            cx.host
                .registry
                .compare_tree(cx.user, left.clone(), right.clone(), page)
                .await?
        }
    };
    let skipped = result.skipped_identical_subtrees;
    let port_next = result.entries.next.map(|c| c.as_str().to_owned());
    let items: Vec<TreeEntryDto> = result
        .entries
        .items
        .iter()
        .skip(start.skip as usize)
        .map(|e| TreeEntryDto {
            file: e.file.clone(),
            state: e.state,
        })
        .collect();

    #[derive(Serialize)]
    struct Data<'a> {
        entries: &'a [TreeEntryDto],
        skipped_identical_subtrees: u32,
    }
    Ok(paged_result(
        &cx.shape(false),
        cx.spec.name(),
        &start,
        &Resume::Port { port_next },
        &items,
        |shown| {
            to_json(&Data {
                entries: shown,
                skipped_identical_subtrees: skipped,
            })
        },
        |s| {
            format!(
                "{} differing files shown{}; {skipped} identical subtrees skipped.",
                s.shown,
                if s.more { ", more follow" } else { "" }
            )
        },
        cx.host.links.compare_tree(&left.to_string(), &right.to_string()),
    ))
}

#[derive(Serialize)]
struct CellDto {
    value: Option<SettingValueDto>,
    location: Option<SettingLocation>,
}

#[derive(Serialize)]
struct RowDto {
    path: domain::SettingPath,
    cells: Vec<CellDto>,
}

pub async fn compare_grid(cx: &CallCx<'_>, args: CompareGrid) -> ToolResult {
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let page = start.port_page()?;
    let file = args.file.0;
    let swimlanes: Vec<SwimlaneId> = args.swimlanes.into_iter().map(|s| s.0).collect();
    let query = GridQuery {
        file: file.clone(),
        swimlanes: swimlanes.clone(),
        channel: args.channel.map(|c| c.0),
        only_differing: args.only_differing.unwrap_or(true),
        page,
    };
    let grid = cx.host.registry.grid(cx.user, query).await?;
    let port_next = grid.rows.next.map(|c| c.as_str().to_owned());
    let redactor = &cx.host.redactor;
    let mut redacted_values = 0_u32;
    let mut rows: Vec<RowDto> = Vec::new();
    for row in grid.rows.items.iter().skip(start.skip as usize) {
        let mut cells = Vec::with_capacity(row.cells.len().min(domain::GridQuery::MAX_SWIMLANES));
        for cell in row.cells.iter().take(domain::GridQuery::MAX_SWIMLANES) {
            let value = cell
                .value
                .as_ref()
                .map(|v| setting_value(redactor, v, CELL_VALUE_CHARS))
                .transpose()
                .map_err(|_: RedactError| crate::tools::error::ToolError::from(RedactError))?;
            let value = value.map(|(dto, n)| {
                redacted_values += n;
                dto
            });
            cells.push(CellDto {
                value,
                location: cell.location.clone(),
            });
        }
        rows.push(RowDto {
            path: row.path.clone(),
            cells,
        });
    }
    cx.host.metrics.redacted(redacted_values);
    let names: Vec<&str> = grid.swimlanes.iter().map(SwimlaneId::as_str).collect();

    #[derive(Serialize)]
    struct Data<'a> {
        swimlanes: &'a [SwimlaneId],
        rows: &'a [RowDto],
    }
    Ok(paged_result(
        &cx.shape(true),
        cx.spec.name(),
        &start,
        &Resume::Port { port_next },
        &rows,
        |shown| {
            to_json(&Data {
                swimlanes: &grid.swimlanes,
                rows: shown,
            })
        },
        |s| {
            format!(
                "{} setting rows across {} swimlanes shown{}{}.",
                s.shown,
                grid.swimlanes.len(),
                if s.more { ", more follow" } else { "" },
                if redacted_values > 0 {
                    format!(", {redacted_values} secret values redacted")
                } else {
                    String::new()
                }
            )
        },
        cx.host.links.grid(&file, &names),
    ))
}
