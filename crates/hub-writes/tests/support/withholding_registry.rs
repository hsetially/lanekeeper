//! A `RegistryRead` that answers like another one, except that it withholds the content of the paths it is told to (D79): the
//! answer a real registry gives for a file the agent's own deny globs matched, which the hub's globs may not know about.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{
    CompareRef, Comparison, ContentSource, EffectiveConfig, FileContent, FileEntry, Finding, FindingsQuery,
    Grid, GridQuery, LogicalFile, NfsPath, Page, Paged, ServiceState, SettingHit, SettingsQuery, SwimlaneId,
    SwimlaneSummary, TextQuery, TextResults, TreeComparison, User, Version,
};
use ports::{ReadError, RegistryRead};

#[derive(Clone)]
pub struct WithholdingRegistry {
    inner: Arc<dyn RegistryRead>,
    withheld: Vec<NfsPath>,
}

impl std::fmt::Debug for WithholdingRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WithholdingRegistry").finish_non_exhaustive()
    }
}

impl WithholdingRegistry {
    pub fn new(inner: Arc<dyn RegistryRead>, withheld: Vec<NfsPath>) -> Self {
        Self { inner, withheld }
    }
}

#[async_trait]
impl RegistryRead for WithholdingRegistry {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        self.inner.swimlanes(u).await
    }

    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.inner.tree(u, s, prefix, page).await
    }

    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        let mut found = self.inner.content(u, s, p, src).await?;
        if self.withheld.contains(p) {
            // Withheld and still carrying the bytes: the flag alone must be enough for the hub to refuse.
            found.withheld = true;
        }
        Ok(found)
    }

    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        self.inner.history(u, s, p, page).await
    }

    async fn compare(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        self.inner.compare(u, l, r, p).await
    }

    async fn compare_tree(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.inner.compare_tree(u, l, r, page).await
    }

    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        self.inner.grid(u, q).await
    }

    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        self.inner.search_settings(u, q).await
    }

    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
        self.inner.search_text(u, q).await
    }

    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        self.inner.findings(u, q).await
    }

    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        self.inner.effective(u, s, f).await
    }

    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        self.inner.pending_restarts(u, s).await
    }
}
