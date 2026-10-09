use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AgentStatus, Attribution, CompareRef, Comparison, ContentSource, Cursor, DriftState, EffectiveConfig,
    EolStyle, FileClass, FileContent, FileEntry, FileFormat, FileKind, FileRole, Finding, FindingsQuery,
    Grid, GridQuery, LogicalFile, NfsPath, ObservationSource, Page, Paged, Role, ServiceState, SettingHit,
    SettingsQuery, ShortText, SwimlaneId, SwimlaneSummary, TextEncoding, TextQuery, TextResults, Timestamp,
    TreeComparison, User, Version,
};

use super::util::lock;
use crate::conformance::RegistryScenario;
use crate::{ReadError, RegistryRead, content_hash};

/// Most swimlanes, files per swimlane and versions per file the fake holds.
const MAX_SWIMLANES: usize = 256;
const MAX_FILES: usize = 20_000;
const MAX_VERSIONS: usize = 100;

struct FileData {
    versions: Vec<(Bytes, Timestamp)>,
}

#[derive(Default)]
struct SwimlaneData {
    files: BTreeMap<NfsPath, FileData>,
    pending_restarts: Vec<ServiceState>,
}

#[derive(Default)]
struct Canned {
    comparison: Option<Comparison>,
    tree_comparison: Option<TreeComparison>,
    grid: Option<Grid>,
    effective: Option<EffectiveConfig>,
    settings: Vec<SettingHit>,
    text: Option<TextResults>,
}

#[derive(Default)]
struct State {
    swimlanes: BTreeMap<SwimlaneId, SwimlaneData>,
    findings: Vec<Finding>,
    canned: Canned,
    clock: i64,
}

/// A registry over seeded files. It lists, pages and reads what was seeded and enforces access, but it does
/// not compute facts (diffs, drift, effective config, search): script those with the `set_*` methods or use
/// `crates/engine` for the real answers.
#[derive(Clone, Default)]
pub struct FakeRegistry {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for FakeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeRegistry").finish_non_exhaustive()
    }
}

fn check(u: &User) -> Result<(), ReadError> {
    if u.allows(Role::Viewer) {
        Ok(())
    } else {
        Err(ReadError::Forbidden)
    }
}

/// The fake's cursor is a position in the result list (`o<n>`); a real implementation uses a key.
fn offset(page: &Page) -> Result<usize, ReadError> {
    match page.cursor() {
        None => Ok(0),
        Some(c) => c
            .as_str()
            .strip_prefix('o')
            .and_then(|n| n.parse().ok())
            .ok_or(ReadError::BadRequest),
    }
}

fn paginate<T: Clone>(all: &[T], page: &Page) -> Result<Paged<T>, ReadError> {
    let start = offset(page)?;
    let limit = page.limit() as usize;
    let items: Vec<T> = all.iter().skip(start).take(limit).cloned().collect();
    let end = start + items.len();
    let next = if end < all.len() {
        Some(Cursor::parse(&format!("o{end}")).map_err(|_| ReadError::Unavailable)?)
    } else {
        None
    };
    Ok(Paged { items, next })
}

fn class_and_role(path: &NfsPath) -> (FileClass, FileRole) {
    let name = path.file_name();
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if ["yml", "yaml", "properties"]
        .iter()
        .any(|k| ext.eq_ignore_ascii_case(k))
    {
        (FileClass::Structured, FileRole::PropertySource)
    } else {
        (FileClass::Text, FileRole::Resource)
    }
}

fn format() -> FileFormat {
    FileFormat {
        eol: EolStyle::Lf,
        encoding: TextEncoding::Utf8,
        bom: false,
    }
}

impl FakeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `s` exist with exactly these files, each with one version.
    pub fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) -> bool {
        let mut st = lock(&self.state);
        if (!st.swimlanes.contains_key(s) && st.swimlanes.len() >= MAX_SWIMLANES) || files.len() > MAX_FILES {
            return false;
        }
        st.clock += 1000;
        let at = Timestamp::from_unix_millis(1_700_000_000_000 + st.clock);
        let data = SwimlaneData {
            files: files
                .iter()
                .map(|(p, b)| {
                    (
                        p.clone(),
                        FileData {
                            versions: vec![(b.clone(), at)],
                        },
                    )
                })
                .collect(),
            pending_restarts: Vec::new(),
        };
        st.swimlanes.insert(s.clone(), data);
        true
    }

    /// Record a new version of a file (a scan saw a change). Returns false for an unknown swimlane or when
    /// the file already has 100 versions.
    pub fn observe(&self, s: &SwimlaneId, path: &NfsPath, bytes: Bytes) -> bool {
        let mut st = lock(&self.state);
        st.clock += 1000;
        let at = Timestamp::from_unix_millis(1_700_000_000_000 + st.clock);
        let Some(sw) = st.swimlanes.get_mut(s) else {
            return false;
        };
        let file = sw
            .files
            .entry(path.clone())
            .or_insert_with(|| FileData { versions: Vec::new() });
        if file.versions.len() >= MAX_VERSIONS {
            return false;
        }
        file.versions.push((bytes, at));
        true
    }

    pub fn add_finding(&self, f: Finding) {
        lock(&self.state).findings.push(f);
    }

    pub fn set_pending_restarts(&self, s: &SwimlaneId, services: Vec<ServiceState>) {
        if let Some(sw) = lock(&self.state).swimlanes.get_mut(s) {
            sw.pending_restarts = services;
        }
    }

    pub fn set_comparison(&self, c: Comparison) {
        lock(&self.state).canned.comparison = Some(c);
    }

    pub fn set_tree_comparison(&self, c: TreeComparison) {
        lock(&self.state).canned.tree_comparison = Some(c);
    }

    pub fn set_grid(&self, g: Grid) {
        lock(&self.state).canned.grid = Some(g);
    }

    pub fn set_effective(&self, e: EffectiveConfig) {
        lock(&self.state).canned.effective = Some(e);
    }

    pub fn set_setting_hits(&self, hits: Vec<SettingHit>) {
        lock(&self.state).canned.settings = hits;
    }

    pub fn set_text_results(&self, r: TextResults) {
        lock(&self.state).canned.text = Some(r);
    }
}

fn swimlane<'a>(st: &'a State, s: &SwimlaneId) -> Result<&'a SwimlaneData, ReadError> {
    st.swimlanes.get(s).ok_or(ReadError::NotFound)
}

#[async_trait]
impl RegistryRead for FakeRegistry {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        st.swimlanes
            .keys()
            .map(|id| {
                Ok(SwimlaneSummary {
                    id: id.clone(),
                    display_name: ShortText::parse(id.as_str()).map_err(|_| ReadError::Unavailable)?,
                    tenants: Vec::new(),
                    agent: AgentStatus::Connected,
                    last_scan_at: None,
                    base_version: None,
                    drift_counts: Vec::new(),
                    finding_count: u32::try_from(st.findings.iter().filter(|f| &f.swimlane == id).count())
                        .unwrap_or(u32::MAX),
                })
            })
            .collect()
    }

    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        let sw = swimlane(&st, s)?;
        let skip = prefix.components().count();
        // Direct children of the prefix: name -> (is_dir, size, hash)
        let mut children: BTreeMap<NfsPath, (bool, u64, Option<domain::ContentHash>)> = BTreeMap::new();
        for (path, data) in &sw.files {
            if !path.starts_with(prefix) || path == prefix {
                continue;
            }
            let comps: Vec<&str> = path.components().collect();
            let Some(child) = comps.get(..=skip).map(|c| c.join("/")) else {
                continue;
            };
            let child = NfsPath::parse(&child).map_err(|_| ReadError::Unavailable)?;
            if comps.len() > skip + 1 {
                children.entry(child).or_insert((true, 0, None));
            } else if let Some((bytes, _)) = data.versions.last() {
                children.insert(child, (false, bytes.len() as u64, Some(content_hash(bytes))));
            }
        }
        let entries: Vec<FileEntry> = children
            .into_iter()
            .map(|(path, (is_dir, size, hash))| {
                let (class, role) = class_and_role(&path);
                FileEntry {
                    is_dir,
                    kind: FileKind::Base,
                    class,
                    role,
                    size,
                    hash,
                    drift: DriftState::InSync,
                    format: if is_dir { None } else { Some(format()) },
                    content_withheld: false,
                    attribution: None,
                    severity: None,
                    pickup_state: None,
                    path,
                }
            })
            .collect();
        paginate(&entries, &page)
    }

    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        let sw = swimlane(&st, s)?;
        let data = sw.files.get(p).ok_or(ReadError::NotFound)?;
        let bytes = match src {
            ContentSource::Nfs => data.versions.last().map(|(b, _)| b.clone()),
            ContentSource::Blob { hash } => data
                .versions
                .iter()
                .find(|(b, _)| content_hash(b) == hash)
                .map(|(b, _)| b.clone()),
            ContentSource::Baseline | ContentSource::Git { .. } => return Err(ReadError::Unavailable),
        }
        .ok_or(ReadError::NotFound)?;
        let (class, _) = class_and_role(p);
        Ok(FileContent {
            path: p.clone(),
            hash: content_hash(&bytes),
            class,
            format: format(),
            size: bytes.len() as u64,
            bytes: Some(bytes),
            withheld: false,
        })
    }

    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        let sw = swimlane(&st, s)?;
        let data = sw.files.get(p).ok_or(ReadError::NotFound)?;
        let versions: Vec<Version> = data
            .versions
            .iter()
            .rev()
            .map(|(b, at)| Version {
                hash: content_hash(b),
                observed_at: *at,
                source: ObservationSource::Scan,
                size: b.len() as u64,
                attribution: Attribution::unknown(),
                severity: None,
                commit: None,
            })
            .collect();
        paginate(&versions, &page)
    }

    async fn compare(
        &self,
        u: &User,
        _l: CompareRef,
        _r: CompareRef,
        _p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        check(u)?;
        lock(&self.state)
            .canned
            .comparison
            .clone()
            .ok_or(ReadError::Unavailable)
    }

    async fn compare_tree(
        &self,
        u: &User,
        _l: CompareRef,
        _r: CompareRef,
        _page: Page,
    ) -> Result<TreeComparison, ReadError> {
        check(u)?;
        lock(&self.state)
            .canned
            .tree_comparison
            .clone()
            .ok_or(ReadError::Unavailable)
    }

    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        check(u)?;
        if q.swimlanes.len() > GridQuery::MAX_SWIMLANES {
            return Err(ReadError::BadRequest);
        }
        let st = lock(&self.state);
        for s in &q.swimlanes {
            swimlane(&st, s)?;
        }
        Ok(st.canned.grid.clone().unwrap_or(Grid {
            swimlanes: q.swimlanes,
            rows: Paged::last(Vec::new()),
        }))
    }

    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        check(u)?;
        let hits = lock(&self.state).canned.settings.clone();
        paginate(&hits, &q.page)
    }

    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
        check(u)?;
        if q.pattern.len() > TextQuery::MAX_PATTERN_BYTES {
            return Err(ReadError::BadRequest);
        }
        Ok(lock(&self.state).canned.text.clone().unwrap_or(TextResults {
            hits: Vec::new(),
            truncated: false,
        }))
    }

    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        let matching: Vec<Finding> = st
            .findings
            .iter()
            .filter(|f| q.swimlane.as_ref().is_none_or(|s| &f.swimlane == s))
            .filter(|f| q.kinds.is_empty() || q.kinds.contains(&f.kind))
            .filter(|f| q.min_severity.is_none_or(|m| f.severity >= m))
            .cloned()
            .collect();
        paginate(&matching, &q.page)
    }

    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        _f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        swimlane(&st, s)?;
        st.canned.effective.clone().ok_or(ReadError::Unavailable)
    }

    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        check(u)?;
        let st = lock(&self.state);
        Ok(swimlane(&st, s)?.pending_restarts.clone())
    }
}

#[async_trait]
impl RegistryScenario for FakeRegistry {
    async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
        Self::seed(self, s, files);
    }
}
