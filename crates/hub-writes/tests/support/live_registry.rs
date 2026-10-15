//! A `RegistryRead` that answers `content` from the fake agent's NFS, so it is never behind: the common case in service, where
//! the registry has seen the file by the time somebody edits it. Every other read is unavailable; the write path asks for
//! content only.

use async_trait::async_trait;
use domain::{
    CompareRef, Comparison, ContentSource, DriftState, EffectiveConfig, EolStyle, FileClass, FileContent,
    FileEntry, FileFormat, FileKind, FileRole, Finding, FindingsQuery, Grid, GridQuery, LogicalFile, NfsPath,
    Page, Paged, ServiceState, SettingHit, SettingsQuery, SwimlaneId, SwimlaneSummary, TextEncoding,
    TextQuery, TextResults, TreeComparison, User, Version,
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

    /// The files directly below `prefix`, with the format their bytes have (so the line endings of a folder can be told).
    async fn tree(
        &self,
        _u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        _page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        let deny = engine::DenyMatcher::with_globs([]).map_err(|_| ReadError::Unavailable)?;
        let mut items: Vec<FileEntry> = self
            .0
            .files_of(s)
            .into_iter()
            .filter(|(p, _)| p.parent() == *prefix)
            .map(|(path, bytes)| FileEntry {
                class: engine::classify(&path, Some(&bytes), &deny),
                role: FileRole::Resource,
                size: bytes.len() as u64,
                hash: Some(content_hash(&bytes)),
                format: Some(engine::detect_format(&bytes)),
                path,
                is_dir: false,
                kind: FileKind::Base,
                drift: DriftState::InSync,
                content_withheld: false,
                attribution: None,
                severity: None,
                pickup_state: None,
            })
            .collect();
        items.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Paged { items, next: None })
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
