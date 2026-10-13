//! The comparison grid (T5, D65, P7): one logical file across the swimlanes the user picks.
//!
//! Rows are setting paths, columns are swimlanes, and a cell is the effective value of the setting in that swimlane, or
//! nothing when the swimlane does not have it ("absent"). The inputs are the effective configs (`effective`), each already
//! flattened and sorted by setting path, so a grid costs one pass over them and no parsing. By default only the rows whose
//! cells differ are returned.
//!
//! # How it stays fast (P7: 40 swimlanes in 150 ms)
//!
//! - Swimlanes with the same effective hash are the same settings (the hash is the identity of an effective config, and
//!   many swimlanes are identical). They are grouped first, and the work is done for each distinct group. When only the
//!   differing rows are asked for and everything is one group, the answer is empty without reading a setting.
//! - The groups are merged like sorted lists, a cursor in each, so a row is found by comparing paths, not by looking each up.
//! - A row that is returned is built from one cursor per column, which only moves forward (rows come in path order), to get
//!   that column's own value and lines: the next setting of the column is almost always the one wanted. A row that is skipped
//!   allocates nothing.
//!
//! # Paging
//!
//! The result is the rows after `GridOptions::after` (a setting path, exclusive), at most `GridOptions::max_rows` of them, in
//! path order, and `truncated` says that more exist. The caller pages by passing the last path as the next `after` (keyset,
//! never offset), and caches by the hashes of the columns.

use domain::{ChannelName, ContentHash, LineRange, SettingPath, SettingValue, SwimlaneId};

use crate::effective::{Effective, EffectiveValue};
use crate::limits::Limits;

/// The most swimlanes in one grid (`domain::GridQuery::MAX_SWIMLANES`).
pub const MAX_GRID_COLUMNS: usize = domain::GridQuery::MAX_SWIMLANES;

/// One column of the grid.
#[derive(Debug, Clone, Copy)]
pub struct GridColumn<'a> {
    pub swimlane: &'a SwimlaneId,
    /// The effective config of the logical file in this swimlane, or `None` when the swimlane does not have the file (every
    /// cell is absent). Its settings must be sorted by path, each path once, as `effective` makes them.
    pub effective: Option<&'a Effective>,
}

/// What to put in the grid.
#[derive(Debug, Clone, Copy)]
pub struct GridOptions<'a> {
    /// Only the rows whose path has this channel as a whole name (`a.<channel>.b`, `a["<channel>"]`, `<channel>[0]`): a
    /// longer name that begins with it (`<channel>-2`) is another channel.
    pub channel: Option<&'a ChannelName>,
    /// Only the rows whose cells are not all the same (and not all present). The default of the UI.
    pub only_differing: bool,
    /// Rows after this path only (exclusive): the keyset of the previous page.
    pub after: Option<&'a SettingPath>,
    /// At most this many rows, and never more than `Limits::settings`.
    pub max_rows: usize,
}

/// A value in a cell, and the lines of the column's file it is written on. The value is the column's own, borrowed: a grid
/// of 40 swimlanes by 6,000 rows is 240,000 cells, and copying their text would cost more than finding them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GridValue<'a> {
    pub value: &'a SettingValue,
    pub lines: LineRange,
}

impl std::fmt::Debug for GridValue<'_> {
    // A value may be a secret: `Debug` shows its size, never its text (S13, S21).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GridValue")
            .field("value_bytes", &self.value.text.len())
            .field("ty", &self.value.ty)
            .field("lines", &self.lines)
            .finish()
    }
}

/// One row of the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridEntry<'a> {
    pub path: &'a SettingPath,
    /// One cell per column, in the order of `GridResult::swimlanes`. `None` is absent.
    pub cells: Vec<Option<GridValue<'a>>>,
}

/// The result of `grid`. It borrows from the columns it was made from: the caller keeps those (they are shared, flattened
/// once) and copies out only what it sends, one page of rows at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridResult<'a> {
    /// The columns, in the order they were given.
    pub swimlanes: Vec<&'a SwimlaneId>,
    /// Sorted by path.
    pub rows: Vec<GridEntry<'a>>,
    /// There are more rows after the last one returned.
    pub truncated: bool,
}

/// Why `grid` refused its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GridError {
    /// More than `MAX_GRID_COLUMNS` swimlanes.
    #[error("a grid has at most {MAX_GRID_COLUMNS} swimlanes")]
    TooManyColumns,
    /// A column's settings are not sorted by path, or hold a path twice. The grid is a merge of sorted lists and would give
    /// wrong rows, so it stops.
    #[error("the settings of a column are not sorted by path")]
    Unsorted,
}

/// A set of columns that are the same effective config.
struct Group<'a> {
    hash: Option<ContentHash>,
    settings: &'a [EffectiveValue],
    next: usize,
}

/// True when `channel` is in `path` as a whole name: not preceded or followed by a character that is part of a name.
fn mentions_channel(path: &str, channel: &str) -> bool {
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut from = 0;
    while let Some(found) = path.get(from..).and_then(|rest| rest.find(channel)) {
        let start = from + found;
        let end = start + channel.len();
        let before = path.get(..start).and_then(|p| p.chars().next_back());
        let after = path.get(end..).and_then(|p| p.chars().next());
        if !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char) {
            return true;
        }
        // Step past this occurrence's first character (a name is ASCII or the search could split a character).
        from = start
            + path
                .get(start..)
                .and_then(|p| p.chars().next())
                .map_or(1, char::len_utf8);
    }
    false
}

/// The grid of `columns`.
///
/// # Errors
///
/// `TooManyColumns` for more than `MAX_GRID_COLUMNS` columns; `Unsorted` when a column that is read is not sorted by path.
pub fn grid<'a>(
    columns: &[GridColumn<'a>],
    options: &GridOptions<'_>,
    limits: &Limits,
) -> Result<GridResult<'a>, GridError> {
    if columns.len() > MAX_GRID_COLUMNS {
        return Err(GridError::TooManyColumns);
    }
    let swimlanes: Vec<&SwimlaneId> = columns.iter().map(|c| c.swimlane).collect();
    let mut result = GridResult {
        swimlanes,
        rows: Vec::new(),
        truncated: false,
    };
    let max_rows = options.max_rows.min(limits.settings);

    // The distinct effective configs, in the order they first appear. A column without the file is a group of its own.
    let mut groups: Vec<Group<'a>> = Vec::new();
    for column in columns {
        let hash = column.effective.map(|e| e.hash);
        if !groups.iter().any(|g| g.hash == hash) {
            groups.push(Group {
                hash,
                settings: column.effective.map_or(&[], |e| e.settings.as_slice()),
                next: 0,
            });
        }
    }
    if options.only_differing && groups.len() <= 1 {
        return Ok(result);
    }

    // Start after the keyset, in each group and in each column.
    let mut cursors: Vec<usize> = vec![0; columns.len()];
    if let Some(after) = options.after {
        for g in &mut groups {
            g.next = g.settings.partition_point(|s| s.path <= *after);
        }
        for (cursor, column) in cursors.iter_mut().zip(columns) {
            *cursor = column
                .effective
                .map_or(0, |e| e.settings.partition_point(|s| s.path <= *after));
        }
    }

    loop {
        // The smallest path any group has left.
        let Some(path) = groups
            .iter()
            .filter_map(|g| g.settings.get(g.next))
            .map(|s| &s.path)
            .min()
        else {
            break;
        };

        // Step every group that has it, and see whether the values agree.
        let mut present = 0;
        let mut first: Option<&SettingValue> = None;
        let mut agree = true;
        for g in &mut groups {
            let Some(s) = g.settings.get(g.next) else { continue };
            if s.path != *path {
                continue;
            }
            present += 1;
            match first {
                None => first = Some(&s.value),
                Some(v) if *v != s.value => agree = false,
                Some(_) => {}
            }
            g.next += 1;
            if g.settings
                .get(g.next)
                .is_some_and(|following| following.path <= s.path)
            {
                return Err(GridError::Unsorted);
            }
        }

        if options
            .channel
            .is_some_and(|c| !mentions_channel(path.as_str(), c.as_str()))
        {
            continue;
        }
        let differs = present < groups.len() || !agree;
        if !differs && options.only_differing {
            continue;
        }
        if result.rows.len() >= max_rows {
            result.truncated = true;
            break;
        }
        result.rows.push(GridEntry {
            path,
            cells: columns
                .iter()
                .zip(&mut cursors)
                .map(|(c, cursor)| cell(c.effective, cursor, path))
                .collect(),
        });
    }
    Ok(result)
}

/// The value of `path` in a column's effective config, with its own lines. `cursor` is where the column was last read: paths
/// come in ascending order, so the setting wanted is at the cursor or after it, and it is left just past what was found.
fn cell<'a>(
    effective: Option<&'a Effective>,
    cursor: &mut usize,
    path: &SettingPath,
) -> Option<GridValue<'a>> {
    let settings = &effective?.settings;
    let rest = settings.get(*cursor..)?;
    // Almost always the very next setting; otherwise skip ahead by search.
    let skip = match rest.first() {
        Some(s) if s.path >= *path => 0,
        _ => rest.partition_point(|s| s.path < *path),
    };
    *cursor += skip;
    let s = settings.get(*cursor).filter(|s| s.path == *path)?;
    *cursor += 1;
    Some(GridValue {
        value: &s.value,
        lines: s.origin.lines,
    })
}
