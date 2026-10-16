//! The docs search port (implemented by 14, used by REST and MCP).

use async_trait::async_trait;
use domain::{
    DocGrepHit, DocHit, DocPath, DocText, FeatureQuery, FeatureStatus, LineRange, LogicalFile, RenderedDoc,
    SettingPath, User,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DocError {
    #[error("not allowed")]
    Forbidden,
    #[error("not found")]
    NotFound,
    /// A parameter is out of range, or a pattern is too long.
    #[error("request is not valid")]
    BadRequest,
    #[error("docs index is unavailable")]
    Unavailable,
}

/// Most hits a search returns.
pub const MAX_DOC_HITS: usize = 100;
/// Longest query or grep pattern in bytes.
pub const MAX_DOC_PATTERN_BYTES: usize = 512;
/// Most context lines around a grep hit.
pub const MAX_GREP_CONTEXT: u8 = 10;
/// Most hits a grep returns.
pub const MAX_GREP_HITS: usize = 500;
/// Most bytes of doc text [`DocSearch::render`] accepts: the upload limit of a doc (prompt 14, T1). A longer doc, which only Git
/// can supply, is not rendered.
pub const MAX_RENDER_SOURCE_BYTES: usize = 2 * 1024 * 1024;
/// Most bytes of HTML [`DocSearch::render`] returns (`RenderedDoc.html` `maxLength` in `api/openapi.yaml`). A render that would be
/// longer is refused, never cut: a cut fragment is not sanitised output.
pub const MAX_RENDERED_HTML_BYTES: usize = 8 * 1024 * 1024;
/// Most headings a [`RenderedDoc`] lists. The page keeps every heading; the outline is cut at this many, in document order.
pub const MAX_DOC_HEADINGS: usize = 500;
/// Every heading id of a rendered doc starts with this, so that a doc cannot name an element of the app (S12).
pub const HEADING_ID_PREFIX: &str = "doc-";
/// Longest heading id in bytes, prefix included.
pub const MAX_HEADING_ID_BYTES: usize = 80;
/// The shape of a heading id (`DocHeading.id` in `api/openapi.yaml`): the prefix and a slug of lower case letters and digits
/// separated by single hyphens.
pub const HEADING_ID_PATTERN: &str = "^doc-[a-z0-9]+(-[a-z0-9]+)*$";

/// [`DocSearch::related_to`] and [`DocSearch::grep_in`] are additive (`contract/07-mcp`, G6): they take the parameters of the
/// REST `docsForFile` and `grepDocs` (a setting, a path prefix, a limit) that [`DocSearch::related`] and [`DocSearch::grep`]
/// lack, so a caller never has to post-filter a capped result. Each has a default that answers [`DocError::Unavailable`], and
/// the conformance suite ([`crate::conformance::doc_search`]) fails an implementation that keeps it. A wrapper of `DocSearch`
/// must forward them. [`DocSearch::render`] is additive in the same way (`contract/08-web`, CG11).
#[async_trait]
pub trait DocSearch: Send + Sync + 'static {
    /// Hybrid search. `limit` is 1..=[`MAX_DOC_HITS`], otherwise [`DocError::BadRequest`].
    async fn search(&self, u: &User, q: &str, limit: usize) -> Result<Vec<DocHit>, DocError>;
    /// Docs that mention a config file.
    async fn related(&self, u: &User, f: &LogicalFile) -> Result<Vec<DocHit>, DocError>;
    /// Exact text of a doc, optionally a 1-based inclusive line range.
    async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError>;
    /// Exact-match search. `regex` patterns run on a linear-time engine with size limits.
    async fn grep(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
    ) -> Result<Vec<DocGrepHit>, DocError>;
    async fn feature_status(&self, u: &User, q: FeatureQuery) -> Result<FeatureStatus, DocError>;

    /// Docs that mention `f`, and `setting` too when there is one, best first, at most `limit` of them. `limit` is
    /// 1..=[`MAX_DOC_HITS`], otherwise [`DocError::BadRequest`]. [`DocSearch::related`] is the same question with no setting
    /// and no limit. Default: [`DocError::Unavailable`].
    async fn related_to(
        &self,
        _u: &User,
        _f: &LogicalFile,
        _setting: Option<&SettingPath>,
        _limit: usize,
    ) -> Result<Vec<DocHit>, DocError> {
        Err(DocError::Unavailable)
    }

    /// The sanitised HTML page of the doc at `path`, for the reader (REST `renderDoc`, the one HTML sink the CSP lets through,
    /// S12). Errors as [`DocSearch::read`]: [`DocError::Forbidden`] for a user who may not read docs, [`DocError::NotFound`] for
    /// an unknown path. Default: [`DocError::Unavailable`].
    ///
    /// What an implementation guarantees (`ports::conformance::render_output_stays_inside_the_allowlist` checks the observable
    /// part):
    /// - **After the scan.** The text is read, its line endings are normalised, and the secret scan masks every flagged value
    ///   (S14b) before the Markdown is parsed. The marker `[REDACTED: N chars, rule]` is then ordinary text in the page. The
    ///   render never sees an unmasked value, and no reveal exists for docs.
    /// - **Inside the allowlist.** `RenderedDoc.html` leaves the allowlist stated in `api/openapi.yaml` for nothing: raw HTML in
    ///   the source is data, so it is escaped or removed, never passed through. `headings` agrees with the page: each entry is a
    ///   heading element of that level with that id, ids are unique, `line` is the 1-based line of the heading in the text.
    /// - **Bounded.** A doc of more than [`MAX_RENDER_SOURCE_BYTES`], or one whose page would exceed [`MAX_RENDERED_HTML_BYTES`],
    ///   is [`DocError::BadRequest`]: the path is typed, so a bad request here can only mean that the doc is outside the
    ///   bounds, and the REST edge answers 413. Never a truncated page. `headings` is cut at [`MAX_DOC_HEADINGS`].
    /// - **Cheap the second time.** Parsing and sanitising are CPU work: they run on the blocking pool under a timeout (a
    ///   render that times out is [`DocError::Unavailable`]), and the result is cached by `content_hash` together with the scan
    ///   configuration and the allowlist version, so a doc is rendered once and a changed configuration is never served from
    ///   an old cache entry. The cache has a weigher and a maximum size.
    ///
    /// `content_hash` is the SHA-256 of the stored text, so the same text has the same hash at any path.
    async fn render(&self, _u: &User, _path: &DocPath) -> Result<RenderedDoc, DocError> {
        Err(DocError::Unavailable)
    }

    /// [`DocSearch::grep`] over the docs under `prefix` only (the prefix itself, or a path that continues it after a `/`;
    /// `None`: every doc), at most `limit` hits, 1..=[`MAX_GREP_HITS`], otherwise [`DocError::BadRequest`]. The limits of
    /// `grep` apply (a pattern of at most [`MAX_DOC_PATTERN_BYTES`], context at most [`MAX_GREP_CONTEXT`]). Default:
    /// [`DocError::Unavailable`].
    async fn grep_in(
        &self,
        _u: &User,
        _pattern: &str,
        _regex: bool,
        _context: u8,
        _prefix: Option<&DocPath>,
        _limit: usize,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        Err(DocError::Unavailable)
    }
}
