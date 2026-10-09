//! Search and findings DTOs.

use serde::{Deserialize, Serialize};

use crate::{
    FindingId, FindingKind, LogicalFile, NfsPath, Page, SettingLocation, SettingPath, Severity, ShortText,
    SwimlaneId, Timestamp, compare::SettingValue,
};

/// A result of one of the consistency checks C1-C11.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: FindingId,
    pub kind: FindingKind,
    pub swimlane: SwimlaneId,
    pub path: Option<NfsPath>,
    pub severity: Severity,
    /// Ids of the severity rules that matched (D76).
    pub rule_ids: Vec<ShortText>,
    /// A plain-language summary written by the engine, never by an AI layer (D33).
    pub message: ShortText,
    pub locations: Vec<SettingLocation>,
    pub detected_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingsQuery {
    pub swimlane: Option<SwimlaneId>,
    /// Empty means all kinds.
    pub kinds: Vec<FindingKind>,
    pub min_severity: Option<Severity>,
    pub page: Page,
}

/// Settings search over `settings_index` (`pg_trgm` on setting path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsQuery {
    /// A substring of the setting path, at most 256 bytes.
    pub path_contains: ShortText,
    /// Empty means all swimlanes the user can see.
    pub swimlanes: Vec<SwimlaneId>,
    pub file: Option<LogicalFile>,
    pub page: Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingHit {
    pub swimlane: SwimlaneId,
    pub file: NfsPath,
    pub path: SettingPath,
    pub value: SettingValue,
    pub location: SettingLocation,
}

/// Text search over stored file versions (never NFS directly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextQuery {
    /// Literal by default; a linear-time regex when `regex` is set. At most 512 bytes.
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub swimlanes: Vec<SwimlaneId>,
    pub path_glob: Option<ShortText>,
    /// Caps on the result set; truncation is reported.
    pub max_total: u32,
    pub max_per_file: u32,
}

impl TextQuery {
    pub const MAX_PATTERN_BYTES: usize = 512;
    pub const DEFAULT_MAX_TOTAL: u32 = 500;
    pub const DEFAULT_MAX_PER_FILE: u32 = 20;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextHit {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    /// 1-based.
    pub line: u32,
    /// Truncated, without the `\r` of a CRLF ending.
    pub text: String,
    /// Byte ranges of the matches within `text`.
    pub ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextResults {
    pub hits: Vec<TextHit>,
    pub truncated: bool,
}
