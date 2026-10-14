//! The two traits the read service implements (T6, plan A4, A5).
//!
//! [`ports::RegistryRead`] is the port: what MCP (07) and the REST handlers share, with the shape `docs/interfaces.md` gives it.
//! [`RegistryApi`] is hub-registry's own superset for what the REST spec has and the port lacks: the drift filter on the tree and the
//! drift list, the swimlane detail, `prefix` on the whole-tree compare, `channel` on the compare, the tenant of an effective config,
//! pages of services, the content reads' entity tags, and the file as the config-server serves it (A4: not on the port, so the port
//! and its fakes and conformance suite stay as they are).

use std::time::Instant;

use async_trait::async_trait;
use domain::{
    ChannelName, CompareRef, Comparison, ContentSource, DriftState, EffectiveConfig, FileContent, FileEntry,
    Finding, FindingsQuery, Grid, GridQuery, LogicalFile, NfsPath, Page, Paged, ServeRequest, ServiceState,
    SettingHit, SettingsQuery, SwimlaneId, SwimlaneSummary, TenantId, TextQuery, TextResults, TreeComparison,
    User, Version,
};
use engine::Limits;
use ports::{ReadError, RegistryRead};

use super::authz::require_viewer;
use super::etag;
use super::service::RegistryService;
use crate::http::dto::{ServedFile, SwimlaneDetail};
use crate::metrics::ReadOp;

/// A read that carries an entity tag: the content itself, or "not modified" when the caller already has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tagged<T> {
    /// The caller's `If-None-Match` matched: no body.
    NotModified {
        etag: String,
    },
    Fresh {
        etag: String,
        value: T,
    },
}

/// A value and the tenant that stood for each swimlane in it (A30): where a view needs one tenant of a swimlane that has several,
/// the first in sorted order, and the response says which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named<T> {
    pub value: T,
    pub tenants: Vec<(SwimlaneId, TenantId)>,
}

#[async_trait]
pub trait RegistryApi: RegistryRead {
    async fn swimlanes_page(&self, u: &User, page: Page) -> Result<Paged<SwimlaneSummary>, ReadError>;

    async fn swimlane(&self, u: &User, s: &SwimlaneId) -> Result<SwimlaneDetail, ReadError>;

    async fn tree_filtered(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        drift: Option<DriftState>,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError>;

    async fn drift(
        &self,
        u: &User,
        s: &SwimlaneId,
        state: Option<DriftState>,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError>;

    /// The content of a file; `NotModified` when `if_none_match` is its tag, before the blob is read.
    async fn content_tagged(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<FileContent>, ReadError>;

    async fn compare_tagged(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        file: &LogicalFile,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<Named<Comparison>>, ReadError>;

    async fn compare_tree_under(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Named<TreeComparison>, ReadError>;

    async fn grid_named(&self, u: &User, q: GridQuery) -> Result<Named<Grid>, ReadError>;

    /// The effective config for `tenant` (`None`: the swimlane's first tenant, A30).
    async fn effective_tagged(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
        tenant: Option<&TenantId>,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<EffectiveConfig>, ReadError>;

    async fn pending_restarts_page(
        &self,
        u: &User,
        s: &SwimlaneId,
        page: Page,
    ) -> Result<Paged<ServiceState>, ReadError>;

    /// The engine's limits the answers are made under: what a handler needs to turn a file's bytes into the text of its response.
    fn limits(&self) -> &Limits;

    /// A file as the config-server serves it (plan A4).
    async fn served(
        &self,
        u: &User,
        s: &SwimlaneId,
        request: ServeRequest,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<ServedFile>, ReadError>;
}

#[async_trait]
impl RegistryRead for RegistryService {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Swimlanes, self.inner.swimlanes_all())
            .await
    }

    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.tree_filtered(u, s, prefix, None, page).await
    }

    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        match self.content_tagged(u, s, p, src, None).await? {
            Tagged::Fresh { value, .. } => Ok(value),
            // No `If-None-Match` was given, so "not modified" cannot be the answer.
            Tagged::NotModified { .. } => Err(ReadError::Unavailable),
        }
    }

    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::History, self.inner.history_page(s, p, &page))
            .await
    }

    async fn compare(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        require_viewer(u)?;
        // A comparison of one file needs the file; a whole tree is `compare_tree`.
        let file = p.ok_or(ReadError::BadRequest)?;
        match self.compare_tagged(u, l, r, &file, None, None).await? {
            Tagged::Fresh { value, .. } => Ok(value.value),
            Tagged::NotModified { .. } => Err(ReadError::Unavailable),
        }
    }

    async fn compare_tree(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        Ok(self
            .compare_tree_under(u, l, r, &NfsPath::root(), page)
            .await?
            .value)
    }

    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        Ok(self.grid_named(u, q).await?.value)
    }

    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::SearchSettings, self.inner.search_settings(&q))
            .await
    }

    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::SearchText, self.inner.search_text(&q))
            .await
    }

    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Findings, self.inner.findings_page(&q))
            .await
    }

    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        match self.effective_tagged(u, s, f, None, None, None).await? {
            Tagged::Fresh { value, .. } => Ok(value),
            Tagged::NotModified { .. } => Err(ReadError::Unavailable),
        }
    }

    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::PendingRestarts, async {
                let page =
                    Page::new(super::restarts::MAX_SERVICES, None).map_err(|_| ReadError::BadRequest)?;
                Ok(self.inner.services_page(s, true, &page).await?.items)
            })
            .await
    }
}

#[async_trait]
impl RegistryApi for RegistryService {
    async fn swimlanes_page(&self, u: &User, page: Page) -> Result<Paged<SwimlaneSummary>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Swimlanes, self.inner.swimlanes_page(&page))
            .await
    }

    async fn swimlane(&self, u: &User, s: &SwimlaneId) -> Result<SwimlaneDetail, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Swimlane, self.inner.swimlane_detail(s))
            .await
    }

    async fn tree_filtered(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        drift: Option<DriftState>,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Tree, self.inner.tree_page(s, prefix, drift, &page))
            .await
    }

    async fn drift(
        &self,
        u: &User,
        s: &SwimlaneId,
        state: Option<DriftState>,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Drift, self.inner.drift_page(s, state, prefix, &page))
            .await
    }

    async fn content_tagged(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<FileContent>, ReadError> {
        require_viewer(u)?;
        let started = Instant::now();
        let outcome = self
            .inner
            .timed(ReadOp::Content, async {
                let resolved = self.inner.resolve_content(s, p, &src).await?;
                let tag = format!("\"{}\"", resolved.hash);
                if etag::matches(if_none_match, &tag) {
                    return Ok(Tagged::NotModified { etag: tag });
                }
                let value = self.inner.load_content(resolved).await?;
                Ok(Tagged::Fresh { etag: tag, value })
            })
            .await;
        if matches!(outcome, Ok(Tagged::NotModified { .. })) {
            // `timed` counted it as an ordinary success; a not-modified answer is counted once more under its own outcome.
            self.inner.not_modified(ReadOp::Content, started);
        }
        outcome
    }

    async fn compare_tagged(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        file: &LogicalFile,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<Named<Comparison>>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(
                ReadOp::Compare,
                self.inner.compare_file(&l, &r, file, channel, if_none_match),
            )
            .await
    }

    async fn compare_tree_under(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Named<TreeComparison>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(
                ReadOp::CompareTree,
                self.inner.compare_tree(&l, &r, prefix, &page),
            )
            .await
    }

    async fn grid_named(&self, u: &User, q: GridQuery) -> Result<Named<Grid>, ReadError> {
        require_viewer(u)?;
        self.inner.timed(ReadOp::Grid, self.inner.grid_view(&q)).await
    }

    async fn effective_tagged(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
        tenant: Option<&TenantId>,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<EffectiveConfig>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(
                ReadOp::Effective,
                self.inner.effective_config(s, f, tenant, channel, if_none_match),
            )
            .await
    }

    async fn pending_restarts_page(
        &self,
        u: &User,
        s: &SwimlaneId,
        page: Page,
    ) -> Result<Paged<ServiceState>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::PendingRestarts, self.inner.services_page(s, true, &page))
            .await
    }

    fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    async fn served(
        &self,
        u: &User,
        s: &SwimlaneId,
        request: ServeRequest,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<ServedFile>, ReadError> {
        require_viewer(u)?;
        self.inner
            .timed(ReadOp::Served, self.inner.served_file(s, &request, if_none_match))
            .await
    }
}
