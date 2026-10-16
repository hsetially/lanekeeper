//! The answers the ports give in the benchmarks of P9, at target scale (40 swimlanes of 2,000 files), and the ports that give them.
//!
//! The benchmark measures this crate's share of a tool call: decode, authenticate, check the role, admit, parse the arguments, ask
//! the port, shape the answer, redact, cap and serialise. The port's own work (the registry's queries, the engine's computation) is
//! not this crate's and is measured in 05 and 04; these ports hand back answers that were computed once, before the clock starts,
//! the size they have at target scale: a page of 200 entries out of 2,000 files, the 6,000-line CRLF file of swimlane 1 with a
//! planted fake key near line 5,000, a comparison and its setting changes from the engine, a grid of 40 swimlanes, 500 text hits,
//! 200 findings and 100 doc hits of 1 KB.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::format_collect,
    clippy::unnecessary_fallible_conversions,
    dead_code
)]

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AgentStatus, Attribution, AttributionSource, ChannelName, CompareQuery, CompareRef, Comparison,
    Confidence, ContentSource, Cursor, DocGrepHit, DocHit, DocId, DocPath, DocText, DriftQuery, DriftState,
    EffectiveConfig, EffectiveQuery, EffectiveSetting, EffectiveSource, EolStyle, FeatureQuery,
    FeatureStatus, FileClass, FileContent, FileEntry, FileFormat, FileKind, FileRole, Finding, FindingId,
    FindingKind, FindingsQuery, FlagStatus, FlagValue, Grid, GridCell, GridQuery, GridRow, LineRange,
    LogicalFile, NfsPath, ObservationSource, Page, Paged, PickupState, Role, ServeRequest, ServedFile,
    ServiceRef, ServiceState, SettingHit, SettingLocation, SettingPath, SettingsQuery, Severity, ShortText,
    SwimlaneId, SwimlaneSummary, TenantId, TextEncoding, TextHit, TextQuery, TextResults, Timestamp,
    TreeCompareEntry, TreeComparison, User, Version,
};
use engine::{Limits, Syntax, flatten, parse, semantic_diff, text_diff};
use ports::{DocError, DocSearch, ReadError, RegistryRead};
use xtask::fixtures::content::big_yaml_doc;
use xtask::fixtures::rng::Rng;

pub const SWIMLANES: usize = 40;
pub const FILES: usize = 2_000;

/// A key shaped like a real token, built at run time from parts so that no literal in the repository is one (A18).
pub fn fake_token() -> String {
    format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8")
}

pub fn sid(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

pub fn np(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

pub fn lf(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

pub fn sp(s: &str) -> SettingPath {
    SettingPath::parse(s).unwrap()
}

pub fn st(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

pub fn lane(n: usize) -> String {
    format!("sit{}", n + 1)
}

/// The shape of the largest real file: a CRLF YAML of about 6,000 lines, with the fake key planted near line 5,000.
fn big_file() -> (String, String) {
    let mut rng = Rng::new(xtask::fixtures::DEFAULT_SEED).fork("mcp-bench");
    let doc = big_yaml_doc(&mut rng, "tx-infinity-api/big.yml", 5_950);
    let text = String::from_utf8(doc.render(3, None)).unwrap();
    let mut out = String::with_capacity(text.len() + 128);
    for (index, line) in text.split_inclusive('\n').enumerate() {
        out.push_str(line);
        if index == 4_999 {
            write!(out, "    apiToken: {}\r\n", fake_token()).unwrap();
        }
    }
    let lines = out.lines().count();
    assert!((5_900..=6_700).contains(&lines), "{lines} lines");
    assert!(out.contains("\r\n") && !out.contains("\n\n"));
    // The other version changes the weight of every third item: 330 changes, 18 lines apart, so 330 hunks and not one.
    let mut second = String::with_capacity(out.len() + 1024);
    let mut item = 0usize;
    for line in out.split_inclusive('\n') {
        if line.starts_with("  item-") {
            item += 1;
        }
        match line.strip_prefix("    weight: ") {
            Some(rest) if item % 3 == 0 => write!(second, "    weight: 1{rest}").unwrap(),
            _ => second.push_str(line),
        }
    }
    (out, second)
}

/// Everything the ports answer with.
pub struct Fixtures {
    pub big: Bytes,
    pub big_path: NfsPath,
    pub swimlanes: Vec<SwimlaneSummary>,
    pub tree: Vec<FileEntry>,
    pub drift: Vec<FileEntry>,
    pub comparison: Comparison,
    pub tree_comparison: Vec<TreeCompareEntry>,
    pub grid_swimlanes: Vec<SwimlaneId>,
    pub grid_rows: Vec<GridRow>,
    pub setting_hits: Vec<SettingHit>,
    pub text: TextResults,
    pub findings: Vec<Finding>,
    pub history: Vec<Version>,
    pub services: Vec<ServiceState>,
    pub effective: EffectiveConfig,
    pub served: ServedFile,
    pub doc_hits: Vec<DocHit>,
    pub doc_text: String,
    pub grep_hits: Vec<DocGrepHit>,
    pub features: FeatureStatus,
}

fn attribution(n: usize) -> Attribution {
    Attribution {
        source: AttributionSource::SentinelLogin,
        confidence: Confidence::High,
        actor: Some(domain::Actor::OsLogin {
            login: st(&format!("operator{}", n % 12)),
        }),
        evidence: domain::Evidence {
            executable: Some(st("vim")),
            login_identity: None,
            observed_at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        },
    }
}

fn entry(n: usize, drift: DriftState) -> FileEntry {
    let folder = format!("tx-service-{:02}/adapters/adapter-{:03}", n % 40, n % 120);
    FileEntry {
        path: np(&format!("{folder}/application-configuration-{n:04}.yml")),
        is_dir: false,
        kind: if n % 3 == 0 {
            FileKind::Tenant
        } else {
            FileKind::Base
        },
        class: FileClass::Structured,
        role: FileRole::PropertySource,
        size: 4_000 + (n as u64 * 7) % 90_000,
        hash: Some(ports::content_hash(format!("file-{n}").as_bytes())),
        drift,
        format: Some(FileFormat {
            eol: EolStyle::Crlf,
            encoding: TextEncoding::Utf8,
            bom: false,
        }),
        content_withheld: false,
        attribution: (drift != DriftState::InSync).then(|| attribution(n)),
        severity: (drift != DriftState::InSync).then_some(Severity::Medium),
        pickup_state: (drift != DriftState::InSync).then_some(PickupState::NeedsNotifyOrRestart),
        logical_file: None,
        tenant: None,
    }
}

impl Fixtures {
    pub fn build() -> Self {
        let (text, changed) = big_file();
        let limits = Limits::default();
        // The comparison: the line diff and the setting changes of two versions, computed once by the engine.
        let diff = text_diff(&text, &changed, &limits);
        let left = flatten(&parse(&text, Syntax::Yaml, &limits).unwrap(), &limits).unwrap();
        let right = flatten(&parse(&changed, Syntax::Yaml, &limits).unwrap(), &limits).unwrap();
        let setting_changes = semantic_diff(&left.settings, &right.settings);
        assert!(diff.hunks.len() > 100 && setting_changes.len() > 100);
        let comparison = Comparison {
            left: "nfs:sit1".parse().unwrap(),
            right: "nfs:sit2".parse().unwrap(),
            file: Some(lf("tx-infinity-api/big.yml")),
            class: FileClass::Structured,
            identical: false,
            eol_only: false,
            hunks: diff.hunks,
            setting_changes,
            binary: None,
        };
        let effective_settings: Vec<EffectiveSetting> = left
            .settings
            .iter()
            .take(800)
            .enumerate()
            .map(|(n, s)| EffectiveSetting {
                path: s.path.clone(),
                value: s.value.text.clone(),
                source: np(if n % 2 == 0 {
                    "tx-infinity-api/big-sit1.yml"
                } else {
                    "tx-infinity-api/big.yml"
                }),
                location: Some(SettingLocation {
                    path: s.path.clone(),
                    lines: LineRange::new(n as u32 + 1, n as u32 + 1).unwrap(),
                }),
            })
            .collect();
        let grid_swimlanes: Vec<SwimlaneId> = (0..SWIMLANES).map(|n| sid(&lane(n))).collect();
        let grid_rows = left
            .settings
            .iter()
            .take(400)
            .map(|s| GridRow {
                path: s.path.clone(),
                cells: (0..SWIMLANES)
                    .map(|n| {
                        if n % 17 == 5 {
                            GridCell {
                                value: None,
                                location: None,
                            }
                        } else {
                            GridCell {
                                value: Some(s.value.clone()),
                                location: Some(SettingLocation {
                                    path: s.path.clone(),
                                    lines: LineRange::new(10, 10).unwrap(),
                                }),
                            }
                        }
                    })
                    .collect(),
            })
            .collect();
        let setting_hits = left
            .settings
            .iter()
            .take(1_000)
            .map(|s| SettingHit {
                swimlane: sid(&lane(s.path.as_str().len() % SWIMLANES)),
                file: np("tx-infinity-api/big.yml"),
                path: s.path.clone(),
                value: s.value.clone(),
                location: SettingLocation {
                    path: s.path.clone(),
                    lines: LineRange::new(3, 3).unwrap(),
                },
                logical_file: None,
                tenant: None,
            })
            .collect();
        let text_hits = TextResults {
            hits: text
                .lines()
                .filter(|l| l.contains("weight"))
                .take(500)
                .enumerate()
                .map(|(n, l)| TextHit {
                    swimlane: sid(&lane(n % SWIMLANES)),
                    path: np(&format!("tx-service-{:02}/application-{n:03}.yml", n % 40)),
                    line: u32::try_from(n * 6 + 5).unwrap(),
                    text: l.to_owned(),
                    ranges: vec![(4, 10)],
                    logical_file: None,
                    tenant: None,
                })
                .collect(),
            truncated: true,
        };
        let findings = (0..400)
            .map(|n| Finding {
                id: FindingId::from(i64::try_from(n).unwrap() + 1),
                kind: [FindingKind::Drift, FindingKind::MissedBaseChanges, FindingKind::RedundantTenantFile][n % 3],
                swimlane: sid(&lane(n % SWIMLANES)),
                path: Some(np(&format!("tx-service-{:02}/application-{n:03}-sit1.yml", n % 40))),
                severity: [Severity::Low, Severity::Medium, Severity::High][n % 3],
                rule_ids: vec![st("R-drift-1"), st("R-prod-2")],
                message: st("The tenant copy is missing 3 settings that the base file gained since the copy was made."),
                locations: (0..4).map(|k| SettingLocation { path: sp(&format!("feature.flags.f{k}.enabled")), lines: LineRange::new(10 + k, 12 + k).unwrap() }).collect(),
                detected_at: Timestamp::from_unix_millis(1_700_000_000_000),
                logical_file: None,
                tenant: None,
            })
            .collect();
        let history = (0..200)
            .map(|n| Version {
                hash: ports::content_hash(format!("version-{n}").as_bytes()),
                observed_at: Timestamp::from_unix_millis(1_700_000_000_000 - n as i64 * 3_600_000),
                source: ObservationSource::Scan,
                size: 90_000 + n as u64,
                attribution: attribution(n as usize),
                severity: Some(Severity::Medium),
                commit: None,
            })
            .collect();
        let services = (0..100)
            .map(|n| ServiceState {
                service: ServiceRef::new("apps", &format!("tx-service-{n:03}")).unwrap(),
                pickup: [
                    PickupState::Live,
                    PickupState::LiveWithinTtl,
                    PickupState::NeedsNotifyOrRestart,
                ][n % 3],
                pending_since: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
                live_by: Some(Timestamp::from_unix_millis(1_700_001_200_000)),
                notifications_enabled: n % 2 == 0,
                cache_ttl_secs: 1_200,
                pending_files: (0..6)
                    .map(|k| np(&format!("tx-service-{n:03}/application-{k}.yml")))
                    .collect(),
            })
            .collect();
        let doc_hits = (0..100)
            .map(|n| DocHit {
                doc: DocId::from(i64::try_from(n).unwrap() + 1),
                title: st(&format!("Feature flags of service {n}")),
                path: DocPath::parse(&format!("/git/main/services/service-{n:03}/features.md")).unwrap(),
                heading_path: vec![st("Features"), st("Flags")],
                snippet: format!("| flag{n} | `tx-infinity-api/big.yml` | feature.flags.f{n}.enabled | Turns the feature on. {} |", "text ".repeat(150)),
                score: 0.9,
                related_files: vec![lf("tx-infinity-api/big.yml")],
            })
            .collect();
        let doc_text: String = (0..3_000)
            .map(|n| format!("Line {n} of the guide: timeout and retries are set in `big.yml`.\n"))
            .collect();
        let grep_hits = (0..200)
            .map(|n| DocGrepHit {
                path: DocPath::parse(&format!("/git/main/services/service-{n:03}/features.md")).unwrap(),
                line: 10 + n,
                text: "The timeout is set in the property source of the service.".to_owned(),
                before: vec!["Before line one".to_owned(); 5],
                after: vec!["After line one".to_owned(); 5],
            })
            .collect();
        let features = FeatureStatus {
            flags: (0..50)
                .map(|n| FlagStatus {
                    flag: st(&format!("feature-{n}")),
                    file: lf("tx-infinity-api/big.yml"),
                    channel: Some(ChannelName::parse("remote-itm").unwrap()),
                    setting: sp(&format!("feature.flags.f{n}.enabled")),
                    template_default: Some(left.settings[n].value.clone()),
                    values: (0..SWIMLANES)
                        .map(|k| FlagValue {
                            swimlane: sid(&lane(k)),
                            value: (k % 9 != 4).then(|| left.settings[n].value.clone()),
                        })
                        .collect(),
                })
                .collect(),
        };
        let big = Bytes::from(text.clone().into_bytes());
        Self {
            big_path: np("tx-infinity-api/big.yml"),
            swimlanes: (0..SWIMLANES)
                .map(|n| SwimlaneSummary {
                    id: sid(&lane(n)),
                    display_name: st(&format!("Swimlane {}", n + 1)),
                    tenants: vec![
                        TenantId::parse(&format!("sit{}", n + 1)).unwrap(),
                        TenantId::parse(&format!("sit{}", n + 41)).unwrap(),
                    ],
                    agent: AgentStatus::Connected,
                    last_scan_at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
                    base_version: None,
                    drift_counts: vec![
                        (DriftState::NfsAhead, 12),
                        (DriftState::GitAhead, 3),
                        (DriftState::Conflict, 1),
                    ],
                    finding_count: 40,
                })
                .collect(),
            tree: (0..FILES).map(|n| entry(n, DriftState::InSync)).collect(),
            drift: (0..FILES).map(|n| entry(n, DriftState::NfsAhead)).collect(),
            comparison,
            tree_comparison: (0..FILES)
                .map(|n| TreeCompareEntry {
                    file: lf(&format!(
                        "tx-service-{:02}/adapters/adapter-{:03}/application-configuration-{n:04}.yml",
                        n % 40,
                        n % 120
                    )),
                    state: domain::CompareState::Different,
                })
                .collect(),
            grid_swimlanes,
            grid_rows,
            setting_hits,
            text: text_hits,
            findings,
            history,
            services,
            effective: EffectiveConfig {
                swimlane: sid("sit1"),
                file: lf("tx-infinity-api/big.yml"),
                tenant: TenantId::parse("sit1").unwrap(),
                channel: None,
                role: FileRole::PropertySource,
                hash: ports::content_hash(&big),
                sources: vec![
                    EffectiveSource {
                        path: np("tx-infinity-api/big-sit1.yml"),
                        rank: 1,
                    },
                    EffectiveSource {
                        path: np("tx-infinity-api/big.yml"),
                        rank: 3,
                    },
                ],
                settings: effective_settings,
                unresolved_placeholders: vec![st("${HOST}"), st("${DB_URL}")],
            },
            served: ServedFile {
                swimlane: sid("sit1"),
                application: domain::AppName::parse("tx-infinity-api").unwrap(),
                tenant: TenantId::parse("sit1").unwrap(),
                channel: None,
                file: np("big.yml"),
                role: FileRole::PropertySource,
                hash: ports::content_hash(&big),
                content_withheld: false,
                text: Some(text.clone()),
                sources: vec![EffectiveSource {
                    path: np("tx-infinity-api/big.yml"),
                    rank: 1,
                }],
                unresolved_placeholders: vec![st("${HOST}")],
            },
            big,
            doc_hits,
            doc_text,
            grep_hits,
            features,
        }
    }
}

fn page_of<T: Clone>(all: &[T], page: &Page) -> Result<Paged<T>, ReadError> {
    let start = match page.cursor() {
        None => 0,
        Some(c) => c
            .as_str()
            .strip_prefix('o')
            .and_then(|n| n.parse().ok())
            .ok_or(ReadError::BadRequest)?,
    };
    let limit = page.limit() as usize;
    let items: Vec<T> = all.iter().skip(start).take(limit).cloned().collect();
    let end = start + items.len();
    let next = (end < all.len()).then(|| Cursor::parse(&format!("o{end}")).unwrap());
    Ok(Paged { items, next })
}

fn allowed(u: &User) -> Result<(), ReadError> {
    if u.allows(Role::Viewer) {
        Ok(())
    } else {
        Err(ReadError::Forbidden)
    }
}

/// The registry as the benchmark has it: the answers of [`Fixtures`], paged as a port pages.
#[derive(Clone)]
pub struct FixtureRegistry(pub Arc<Fixtures>);

impl std::fmt::Debug for FixtureRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FixtureRegistry")
    }
}

#[async_trait]
impl RegistryRead for FixtureRegistry {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        allowed(u)?;
        Ok(self.0.swimlanes.clone())
    }

    async fn tree(
        &self,
        u: &User,
        _s: &SwimlaneId,
        _prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        allowed(u)?;
        page_of(&self.0.tree, &page)
    }

    async fn content(
        &self,
        u: &User,
        _s: &SwimlaneId,
        p: &NfsPath,
        _src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        allowed(u)?;
        if *p != self.0.big_path {
            return Err(ReadError::NotFound);
        }
        Ok(FileContent {
            path: p.clone(),
            hash: ports::content_hash(&self.0.big),
            class: FileClass::Structured,
            format: FileFormat {
                eol: EolStyle::Crlf,
                encoding: TextEncoding::Utf8,
                bom: false,
            },
            size: self.0.big.len() as u64,
            bytes: Some(self.0.big.clone()),
            withheld: false,
        })
    }

    async fn history(
        &self,
        u: &User,
        _s: &SwimlaneId,
        _p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        allowed(u)?;
        page_of(&self.0.history, &page)
    }

    async fn compare(
        &self,
        u: &User,
        _l: CompareRef,
        _r: CompareRef,
        _p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        allowed(u)?;
        Ok(self.0.comparison.clone())
    }

    async fn compare_tree(
        &self,
        u: &User,
        _l: CompareRef,
        _r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        allowed(u)?;
        Ok(TreeComparison {
            entries: page_of(&self.0.tree_comparison, &page)?,
            skipped_identical_subtrees: 1_300,
        })
    }

    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        allowed(u)?;
        Ok(Grid {
            swimlanes: self.0.grid_swimlanes.clone(),
            rows: page_of(&self.0.grid_rows, &q.page)?,
        })
    }

    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        allowed(u)?;
        page_of(&self.0.setting_hits, &q.page)
    }

    async fn search_text(&self, u: &User, _q: TextQuery) -> Result<TextResults, ReadError> {
        allowed(u)?;
        Ok(self.0.text.clone())
    }

    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        allowed(u)?;
        page_of(&self.0.findings, &q.page)
    }

    async fn effective(
        &self,
        u: &User,
        _s: &SwimlaneId,
        _f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        allowed(u)?;
        Ok(self.0.effective.clone())
    }

    async fn pending_restarts(&self, u: &User, _s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        allowed(u)?;
        Ok(self.0.services.clone())
    }

    async fn effective_for(
        &self,
        u: &User,
        _s: &SwimlaneId,
        _q: &EffectiveQuery,
    ) -> Result<EffectiveConfig, ReadError> {
        allowed(u)?;
        Ok(self.0.effective.clone())
    }

    async fn compare_in(&self, u: &User, _q: &CompareQuery) -> Result<Comparison, ReadError> {
        allowed(u)?;
        Ok(self.0.comparison.clone())
    }

    async fn compare_tree_under(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        _prefix: &NfsPath,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.compare_tree(u, l, r, page).await
    }

    async fn drift(
        &self,
        u: &User,
        _s: &SwimlaneId,
        _q: &DriftQuery,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        allowed(u)?;
        page_of(&self.0.drift, &page)
    }

    async fn auto_notify(&self, u: &User, _s: &SwimlaneId) -> Result<bool, ReadError> {
        allowed(u)?;
        Ok(true)
    }

    async fn served(&self, u: &User, _s: &SwimlaneId, r: &ServeRequest) -> Result<ServedFile, ReadError> {
        allowed(u)?;
        let mut served = self.0.served.clone();
        served.application = r.application.clone();
        served.tenant = r.tenant.clone();
        served.channel.clone_from(&r.channel);
        served.file = r.file.clone();
        Ok(served)
    }
}

fn doc_allowed(u: &User) -> Result<(), DocError> {
    if u.allows(Role::Viewer) {
        Ok(())
    } else {
        Err(DocError::Forbidden)
    }
}

/// The docs index as the benchmark has it.
#[derive(Clone)]
pub struct FixtureDocs(pub Arc<Fixtures>);

impl std::fmt::Debug for FixtureDocs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FixtureDocs")
    }
}

#[async_trait]
impl DocSearch for FixtureDocs {
    async fn search(&self, u: &User, _q: &str, limit: usize) -> Result<Vec<DocHit>, DocError> {
        doc_allowed(u)?;
        Ok(self.0.doc_hits.iter().take(limit).cloned().collect())
    }

    async fn related(&self, u: &User, _f: &LogicalFile) -> Result<Vec<DocHit>, DocError> {
        doc_allowed(u)?;
        Ok(self.0.doc_hits.clone())
    }

    async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError> {
        doc_allowed(u)?;
        Ok(DocText {
            path: path.clone(),
            lines,
            total_lines: 3_000,
            text: self.0.doc_text.clone(),
            truncated: false,
        })
    }

    async fn grep(
        &self,
        u: &User,
        _p: &str,
        _regex: bool,
        _context: u8,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        doc_allowed(u)?;
        Ok(self.0.grep_hits.clone())
    }

    async fn feature_status(&self, u: &User, _q: FeatureQuery) -> Result<FeatureStatus, DocError> {
        doc_allowed(u)?;
        Ok(self.0.features.clone())
    }

    async fn related_to(
        &self,
        u: &User,
        _f: &LogicalFile,
        _s: Option<&SettingPath>,
        limit: usize,
    ) -> Result<Vec<DocHit>, DocError> {
        doc_allowed(u)?;
        Ok(self.0.doc_hits.iter().take(limit).cloned().collect())
    }

    async fn grep_in(
        &self,
        u: &User,
        _p: &str,
        _regex: bool,
        _context: u8,
        _prefix: Option<&DocPath>,
        limit: usize,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        doc_allowed(u)?;
        Ok(self.0.grep_hits.iter().take(limit).cloned().collect())
    }
}
