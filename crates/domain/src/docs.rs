//! Docs search DTOs (prompt 14 implements the port).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    ChannelName, ContentHash, DocId, DocPath, LineRange, LogicalFile, SettingPath, ShortText, SwimlaneId,
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
    /// The id of the heading of the rendered page ([`RenderedDoc`] of the same `path`) that the chunk sits under, so that the
    /// reader opens the page at the hit. `None` when the chunk is above the first heading or the producer has no outline. When
    /// it is `Some`, it is the `id` of one of that page's [`RenderedDoc::headings`]; a client never builds one from a title.
    /// Additive (`contract/08-web`, CG11): absent in an old document, and never written when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ShortText>,
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
    /// The id of the heading, in the rendered page of the doc that documents this flag, under which the flag's row of the
    /// Feature Flags table sits. The doc is the one `docsForFile` returns for `file` and `setting`; its [`DocHit::anchor`] is
    /// the same value. `None` when the producer has no outline. Additive (`contract/08-web`, CG11): absent in an old document,
    /// and never written when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ShortText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureStatus {
    pub flags: Vec<FlagStatus>,
}

/// One heading of a rendered doc, in document order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocHeading {
    /// The `id` attribute of the heading element in [`RenderedDoc::html`]: `doc-` and a slug (lower case letters and digits
    /// separated by single hyphens), unique in the page. Chosen by the server, never copied from the Markdown, so a doc cannot
    /// name an element of the app (S12).
    pub id: ShortText,
    /// 1..=6, the level of the heading element.
    pub level: u8,
    /// The text of the heading with its markup removed. Plain text, not HTML: a client sets it as text content only.
    pub text: ShortText,
    /// 1-based line of the heading in the doc's text, the numbering of `read` and `grep`.
    pub line: u32,
}

/// The server-rendered, sanitised page of one doc (`DocSearch::render`).
///
/// `html` is the one HTML string a client may put into the page (S12), through its Trusted Types policy and nowhere else; the
/// allowlist it stays inside is stated in `api/openapi.yaml` (`RenderedDoc.html`) and checked by
/// `ports::conformance::html_violations`. [`fmt::Debug`] prints sizes only, never the page.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedDoc {
    pub path: DocPath,
    /// Sanitised HTML, a fragment. At most 8 MiB.
    pub html: String,
    /// The first 500 headings, in document order.
    pub headings: Vec<DocHeading>,
    /// SHA-256 of the doc's stored text: the same text has the same hash at any path.
    pub content_hash: ContentHash,
}

impl fmt::Debug for RenderedDoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderedDoc")
            .field("path", &self.path)
            .field("html_bytes", &self.html.len())
            .field("headings", &self.headings.len())
            .field("content_hash", &self.content_hash)
            .finish()
    }
}
