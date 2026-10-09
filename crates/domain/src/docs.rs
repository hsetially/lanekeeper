//! Docs search DTOs (prompt 14 implements the port).

use serde::{Deserialize, Serialize};

use crate::{
    ChannelName, DocId, DocPath, LineRange, LogicalFile, SettingPath, ShortText, SwimlaneId,
    compare::SettingValue,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocHit {
    pub doc: DocId,
    pub title: ShortText,
    pub path: DocPath,
    pub heading_path: Vec<ShortText>,
    /// A snippet of the matching chunk, bounded by the producer.
    pub snippet: String,
    pub score: f32,
    /// Config files the chunk mentions (backticked paths).
    pub related_files: Vec<LogicalFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocText {
    pub path: DocPath,
    pub lines: Option<LineRange>,
    pub total_lines: u32,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGrepHit {
    pub path: DocPath,
    /// 1-based.
    pub line: u32,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// A feature-status question: which swimlanes have a documented flag on or off?
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureQuery {
    pub flag: Option<ShortText>,
    pub file: Option<LogicalFile>,
    pub channel: Option<ChannelName>,
    pub swimlanes: Vec<SwimlaneId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagValue {
    pub swimlane: SwimlaneId,
    /// `None` renders as "absent".
    pub value: Option<SettingValue>,
}

/// One documented flag, resolved to a setting path, with its value per swimlane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagStatus {
    pub flag: ShortText,
    pub file: LogicalFile,
    pub channel: Option<ChannelName>,
    pub setting: SettingPath,
    pub template_default: Option<SettingValue>,
    pub values: Vec<FlagValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureStatus {
    pub flags: Vec<FlagStatus>,
}
