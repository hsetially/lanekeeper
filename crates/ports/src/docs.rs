//! The docs search port (implemented by 14, used by REST and MCP).

use async_trait::async_trait;
use domain::{
    DocGrepHit, DocHit, DocPath, DocText, FeatureQuery, FeatureStatus, LineRange, LogicalFile, SettingPath,
    User,
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

/// [`DocSearch::related_to`] and [`DocSearch::grep_in`] are additive (`contract/07-mcp`, G6): they take the parameters of the
/// REST `docsForFile` and `grepDocs` (a setting, a path prefix, a limit) that [`DocSearch::related`] and [`DocSearch::grep`]
/// lack, so a caller never has to post-filter a capped result. Each has a default that answers [`DocError::Unavailable`], and
/// the conformance suite ([`crate::conformance::doc_search`]) fails an implementation that keeps it. A wrapper of `DocSearch`
/// must forward them.
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
