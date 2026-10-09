//! The read service port (implemented by 05, used by REST and MCP).
//!
//! Every method takes the calling [`User`] and must refuse pending, disabled and role-less users with
//! [`ReadError::Forbidden`] (S4). Facts are computed by `crates/engine`; this port only names the questions.

use async_trait::async_trait;
use domain::{
    CompareRef, Comparison, ContentSource, EffectiveConfig, FileContent, FileEntry, Finding, FindingsQuery,
    Grid, GridQuery, LogicalFile, NfsPath, Page, Paged, ServiceState, SettingHit, SettingsQuery, SwimlaneId,
    SwimlaneSummary, TextQuery, TextResults, TreeComparison, User, Version,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The user may not read this (also for pending and disabled users).
    #[error("not allowed")]
    Forbidden,
    /// The swimlane, file or version does not exist, or the user may not know that it does.
    #[error("not found")]
    NotFound,
    /// A parameter is out of range (for example more swimlanes than [`GridQuery::MAX_SWIMLANES`]).
    #[error("request is not valid")]
    BadRequest,
    #[error("read service is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait RegistryRead: Send + Sync + 'static {
    /// The swimlanes the user can see, ordered by id.
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError>;

    /// Entries under `prefix`, ordered by path, one page at a time.
    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError>;

    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError>;

    /// Versions of a file, newest first.
    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError>;

    async fn compare(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError>;

    async fn compare_tree(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError>;

    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError>;

    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError>;

    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError>;

    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError>;

    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError>;

    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError>;
}
