//! The read service port (implemented by 05, used by REST and MCP).
//!
//! Every method takes the calling [`User`] and must refuse pending, disabled and role-less users with
//! [`ReadError::Forbidden`] (S4). Facts are computed by `crates/engine`; this port only names the questions.

use async_trait::async_trait;
use domain::{
    CompareQuery, CompareRef, Comparison, ContentSource, DriftQuery, EffectiveConfig, EffectiveQuery,
    FileContent, FileEntry, Finding, FindingsQuery, Grid, GridQuery, LogicalFile, NfsPath, Page, Paged,
    ServeRequest, ServedFile, ServiceState, SettingHit, SettingsQuery, SwimlaneId, SwimlaneSummary,
    TextQuery, TextResults, TreeComparison, User, Version,
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

/// The methods after [`RegistryRead::pending_restarts`] are additive (`contract/07-mcp`, G1 to G5): each has a default that
/// answers [`ReadError::Unavailable`], so an implementation written before them still compiles, and the conformance suite
/// ([`crate::conformance::registry_read`]) fails it until it overrides them. They answer the questions of the REST operations
/// that have parameters the older methods lack (a tenant, a channel, a prefix, a drift filter) and the two the MCP tools need
/// that have no method at all (`auto_notify`, the served file). A wrapper of `RegistryRead` must forward them: the defaults
/// would otherwise hide the wrapped implementation's answers.
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

    /// The effective config of `q.file` for `q.tenant` and `q.channel` (D82, D83), computed by the engine, with only the
    /// settings at or below `q.setting_prefix` when one is given (`settings` is filtered here, so a caller never has to cut
    /// a capped page). The answer is for the question asked: its `file`, `tenant` and `channel` are `q`'s. A swimlane or a
    /// tenant the swimlane does not deploy is [`ReadError::NotFound`]. [`RegistryRead::effective`] is the question without
    /// a tenant or a channel. Default: [`ReadError::Unavailable`].
    async fn effective_for(
        &self,
        _u: &User,
        _s: &SwimlaneId,
        _q: &EffectiveQuery,
    ) -> Result<EffectiveConfig, ReadError> {
        Err(ReadError::Unavailable)
    }

    /// One file between two typed references, as served to `q.channel` when there is one. Same answer as
    /// [`RegistryRead::compare`] when `q.channel` is `None`. Default: [`ReadError::Unavailable`].
    async fn compare_in(&self, _u: &User, _q: &CompareQuery) -> Result<Comparison, ReadError> {
        Err(ReadError::Unavailable)
    }

    /// [`RegistryRead::compare_tree`] for the files below `prefix` only (a folder, as on NFS), one page at a time. The
    /// entries are the same ones, in the same order, that the whole comparison lists below `prefix`; the filtering happens
    /// here, so a caller never post-filters a page. Default: [`ReadError::Unavailable`].
    async fn compare_tree_under(
        &self,
        _u: &User,
        _l: CompareRef,
        _r: CompareRef,
        _prefix: &NfsPath,
        _page: Page,
    ) -> Result<TreeComparison, ReadError> {
        Err(ReadError::Unavailable)
    }

    /// The files of `s` that match `q` (the REST `listDrift`): by default the ones that are not in sync, of one state when
    /// `q.state` is set, below `q.prefix` at any depth. Files, not folders, ordered by path, one page at a time; every entry
    /// carries its drift state, attribution and severity (D73, D76). It also serves the drift filter of a folder listing.
    /// Default: [`ReadError::Unavailable`].
    async fn drift(
        &self,
        _u: &User,
        _s: &SwimlaneId,
        _q: &DriftQuery,
        _page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        Err(ReadError::Unavailable)
    }

    /// Whether `s` tells the config-server by itself when a write changes a file (D85). Off by default, so a swimlane
    /// nobody switched on answers `false`. Default: [`ReadError::Unavailable`], and a caller shows no value rather than
    /// guessing `false`.
    async fn auto_notify(&self, _u: &User, _s: &SwimlaneId) -> Result<bool, ReadError> {
        Err(ReadError::Unavailable)
    }

    /// What the config-server serves for `r` in `s` (D82, D86), fetched read-only through the swimlane's agent. The answer
    /// is for the question asked: its application, tenant, channel and file are `r`'s. [`ReadError::Unavailable`] when the
    /// agent or the config-server cannot be reached, and by default.
    async fn served(&self, _u: &User, _s: &SwimlaneId, _r: &ServeRequest) -> Result<ServedFile, ReadError> {
        Err(ReadError::Unavailable)
    }
}
