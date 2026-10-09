//! Comparison DTOs: file diffs, tree comparisons and the grid.

use serde::{Deserialize, Serialize};

use crate::{
    ChannelName, CompareRef, ContentHash, FileClass, LogicalFile, Page, Paged, SettingLocation, SettingPath,
    SwimlaneId,
};

/// The type tag a scalar keeps from its source (`yes` is not `true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    String,
    Int,
    Float,
    Bool,
    Null,
    Other,
}

/// Flags on values whose meaning differs between YAML 1.1 and 1.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueFlag {
    /// `yes`, `no`, `on`, `off`: not equal to booleans in YAML 1.2.
    YamlBooleanLike,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingValue {
    /// The source text of the scalar, exactly as written.
    pub text: String,
    pub ty: ValueType,
    pub flag: Option<ValueFlag>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
    TypeChanged,
}

/// A setting-level difference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChange {
    pub path: SettingPath,
    pub kind: ChangeKind,
    pub left: Option<SettingValue>,
    pub right: Option<SettingValue>,
    pub left_location: Option<SettingLocation>,
    pub right_location: Option<SettingLocation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// A byte range within a diff line that changed (inline change highlighting, D76).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Without the line terminator.
    pub text: String,
    pub inline_changes: Vec<InlineRange>,
}

/// A unified-diff hunk, computed on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// Hash comparison for binary files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryCompare {
    pub left: Option<ContentHash>,
    pub right: Option<ContentHash>,
    pub left_size: Option<u64>,
    pub right_size: Option<u64>,
}

/// The result of comparing one logical file between two references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub left: CompareRef,
    pub right: CompareRef,
    pub file: Option<LogicalFile>,
    pub class: FileClass,
    pub identical: bool,
    /// The only differences are line endings (flagged, but not counted as a change by default).
    pub eol_only: bool,
    pub hunks: Vec<DiffHunk>,
    pub setting_changes: Vec<SettingChange>,
    pub binary: Option<BinaryCompare>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareState {
    Same,
    Different,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeCompareEntry {
    pub file: LogicalFile,
    pub state: CompareState,
}

/// A whole-swimlane comparison. Subtrees with equal effective tree hashes are skipped and counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeComparison {
    pub entries: Paged<TreeCompareEntry>,
    pub skipped_identical_subtrees: u32,
}

/// Rows are setting paths, columns are swimlanes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridQuery {
    pub file: LogicalFile,
    /// At most 64 swimlanes.
    pub swimlanes: Vec<SwimlaneId>,
    /// Matches setting paths that contain this channel name.
    pub channel: Option<ChannelName>,
    /// Show only rows whose values differ (the default in the UI).
    pub only_differing: bool,
    pub page: Page,
}

impl GridQuery {
    pub const MAX_SWIMLANES: usize = 64;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridCell {
    /// `None` renders as "absent".
    pub value: Option<SettingValue>,
    pub location: Option<SettingLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridRow {
    pub path: SettingPath,
    /// One cell per swimlane, in the order of [`Grid::swimlanes`].
    pub cells: Vec<GridCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub swimlanes: Vec<SwimlaneId>,
    pub rows: Paged<GridRow>,
}
