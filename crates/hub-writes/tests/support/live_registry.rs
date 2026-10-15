//! A `RegistryRead` that answers `content` from the fake agent's NFS, so it is never behind: the common case in service, where
//! the registry has seen the file by the time somebody edits it. Every other read is unavailable; the write path asks for
//! content only.

use async_trait::async_trait;
use domain::{
    CompareRef, Comparison, ContentSource, EffectiveConfig, EolStyle, FileClass, FileContent, FileEntry,
    FileFormat, Finding, FindingsQuery, Grid, GridQuery, LogicalFile, NfsPath, Page, Paged, ServiceState,
    SettingHit, SettingsQuery, SwimlaneId, SwimlaneSummary, TextEncoding, TextQuery, TextResults,
    TreeComparison, User, Version,
};
use ports::{ReadError, RegistryRead, content_hash};

use super::fake_agent::FakeAgent;

#[derive(Debug, Clone)]
pub struct LiveRegistry(pub FakeAgent);

#[async_trait]
impl RegistryRead for LiveRegistry {
    async fn content(
        &self,
        _u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        _src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        let bytes = self.0.content(s, p).ok_or(ReadError::NotFound)?;
        Ok(FileContent {
            path: p.clone(),
            hash: content_hash(&bytes),
            class: FileClass::Structured,
            format: FileFormat {
                eol: EolStyle::Lf,
                encoding: TextEncoding::Utf8,
                bom: false,
            },
            size: bytes.len() as u64,
            bytes: Some(bytes),
            withheld: false,
        })
    }

    async fn swimlanes(&self, _u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn tree(
        &self,
        _u: &User,
        _s: &SwimlaneId,
        _p: &NfsPath,
        _page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn history(
        &self,
        _u: &User,
        _s: &SwimlaneId,
        _p: &NfsPath,
        _page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn compare(
        &self,
        _u: &User,
        _l: CompareRef,
        _r: CompareRef,
        _p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn compare_tree(
        &self,
        _u: &User,
        _l: CompareRef,
        _r: CompareRef,
        _page: Page,
    ) -> Result<TreeComparison, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn grid(&self, _u: &User, _q: GridQuery) -> Result<Grid, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn search_settings(&self, _u: &User, _q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn search_text(&self, _u: &User, _q: TextQuery) -> Result<TextResults, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn findings(&self, _u: &User, _q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn effective(
        &self,
        _u: &User,
        _s: &SwimlaneId,
        _f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        Err(ReadError::Unavailable)
    }

    async fn pending_restarts(&self, _u: &User, _s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        Err(ReadError::Unavailable)
    }
}
