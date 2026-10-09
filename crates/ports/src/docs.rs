//! The docs search port (implemented by 14, used by REST and MCP).

use async_trait::async_trait;
use domain::{
    DocGrepHit, DocHit, DocPath, DocText, FeatureQuery, FeatureStatus, LineRange, LogicalFile, User,
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
}
