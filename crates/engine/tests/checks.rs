//! T6 (domain-model "Consistency checks", D19, D20, D84, S13, S21): checks C1 to C8, one finding-to-be per fact.
//!
//! The checks are pure functions of what they are given, so the tests build small swimlanes by hand and read the results as
//! text. The goldens (`golden_*`) pin the wording and the details of each check; the other tests pin the rules one at a
//! time (when a check is silent as much as when it fires).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;

use bytes::Bytes;
use domain::{
    AttributionSource, CommitId, ContentHash, DriftState, FileClass, FileKind, FindingKind, LogicalFile,
    MergeMode, NfsPath, RepoPath, ShortText, TenantId, TenantSet, Timestamp,
};
use engine::{
    BaseHistory, BaseVersion, CheckDetail, CheckFile, CheckInput, CheckResult, Drift, FileFacts, Flat,
    HostMatcher, HostRule, HostRules, HostRulesError, Limits, Overwrite, ParseWarning, Structured, Syntax,
    checks_c1_to_c8, flatten, normalised_hash, parse,
};
use proptest::prelude::*;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn tenant(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

fn tenants(names: &[&str]) -> TenantSet {
    TenantSet::new(names.iter().map(|n| tenant(n))).unwrap()
}

fn short(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

fn h(n: u8) -> ContentHash {
    ContentHash::from_bytes([n; 32])
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_unix_millis(seconds * 1000)
}

fn commit(n: u8) -> CommitId {
    CommitId::parse(&format!("{n:02x}").repeat(20)).unwrap()
}

/// A file of the swimlane with everything the view borrows.
struct Owned {
    path: NfsPath,
    facts: FileFacts,
    class: FileClass,
    hash: ContentHash,
    mode: MergeMode,
    flat: Option<Flat>,
    parse_warnings: Vec<ParseWarning>,
    parse_dropped: usize,
    root: Option<Flat>,
    drift: Option<Drift>,
    overwrite: Option<Overwrite>,
    since: Vec<BaseVersion>,
    at_tenant_commit: Option<BaseVersion>,
    tenant_commit_time: Option<Timestamp>,
}

fn facts(kind: FileKind, logical: &str, tenant: Option<&str>) -> FileFacts {
    FileFacts {
        kind,
        logical: LogicalFile::parse(logical).unwrap(),
        tenant: tenant.map(self::tenant),
        repo_path: RepoPath::parse(&format!("config/{logical}")).unwrap(),
    }
}

impl Owned {
    fn new(path: &str, facts: FileFacts, class: FileClass, data: &[u8]) -> Self {
        let bytes = Bytes::copy_from_slice(data);
        let (flat, parse_warnings, parse_dropped) = if class == FileClass::Structured {
            let syntax = Syntax::of_path(&nfs(path)).unwrap();
            let text = std::str::from_utf8(data).unwrap();
            let parsed = parse(text, syntax, &Limits::default()).unwrap();
            let flat = flatten(&parsed, &Limits::default()).unwrap();
            (Some(flat), parsed.warnings, parsed.warnings_dropped)
        } else {
            (None, Vec::new(), 0)
        };
        Self {
            path: nfs(path),
            facts,
            class,
            hash: normalised_hash(&bytes, class),
            mode: MergeMode::WholeFile,
            flat,
            parse_warnings,
            parse_dropped,
            root: None,
            drift: None,
            overwrite: None,
            since: Vec::new(),
            at_tenant_commit: None,
            tenant_commit_time: None,
        }
    }

    /// A structured base file at `path` (its own logical file).
    fn base(path: &str, text: &str) -> Self {
        Self::new(
            path,
            facts(FileKind::Base, path, None),
            FileClass::Structured,
            text.as_bytes(),
        )
    }

    /// A structured tenant file `path` of the logical file `logical`.
    fn tenant(path: &str, logical: &str, tenant: &str, text: &str) -> Self {
        Self::new(
            path,
            facts(FileKind::Tenant, logical, Some(tenant)),
            FileClass::Structured,
            text.as_bytes(),
        )
    }

    fn raw(path: &str, logical: &str, tenant: Option<&str>, class: FileClass, data: &[u8]) -> Self {
        let kind = if tenant.is_some() {
            FileKind::Tenant
        } else {
            FileKind::Base
        };
        Self::new(path, facts(kind, logical, tenant), class, data)
    }

    fn mode(mut self, mode: MergeMode) -> Self {
        self.mode = mode;
        self
    }

    fn kind(mut self, kind: FileKind) -> Self {
        self.facts.kind = kind;
        self
    }

    fn drift(mut self, d: Drift) -> Self {
        self.drift = Some(d);
        self
    }

    fn overwrite(mut self, o: Overwrite) -> Self {
        self.overwrite = Some(o);
        self
    }

    fn root(mut self, text: &str) -> Self {
        let parsed = parse(text, Syntax::Yaml, &Limits::default()).unwrap();
        self.root = Some(flatten(&parsed, &Limits::default()).unwrap());
        self
    }

    fn history(
        mut self,
        tenant_commit: i64,
        before: Option<(u8, i64, &str)>,
        since: &[(u8, i64, Option<&str>)],
    ) -> Self {
        self.tenant_commit_time = Some(at(tenant_commit));
        self.at_tenant_commit = before.map(|(n, t, text)| BaseVersion {
            commit: commit(n),
            time: at(t),
            content: Some(Bytes::copy_from_slice(text.as_bytes())),
        });
        self.since = since
            .iter()
            .map(|(n, t, text)| BaseVersion {
                commit: commit(*n),
                time: at(*t),
                content: text.map(|s| Bytes::copy_from_slice(s.as_bytes())),
            })
            .collect();
        self
    }

    fn view(&self) -> CheckFile<'_> {
        CheckFile {
            path: &self.path,
            facts: &self.facts,
            class: self.class,
            hash: self.hash,
            merge_mode: self.mode,
            structured: self.flat.as_ref().map(|flat| Structured {
                settings: &flat.settings,
                parse_warnings: &self.parse_warnings,
                parse_warnings_dropped: self.parse_dropped,
            }),
            root_settings: self.root.as_ref().map(|f| f.settings.as_slice()),
            drift: self.drift,
            overwrite: self.overwrite,
            base_history: self.tenant_commit_time.map(|t| BaseHistory {
                tenant_commit_time: t,
                at_tenant_commit: self.at_tenant_commit.as_ref(),
                since: &self.since,
            }),
        }
    }
}

fn drift(state: DriftState) -> Drift {
    Drift {
        state,
        underlying: state,
        set_baseline_to_nfs: false,
        label: None,
        mark_stale: false,
    }
}

/// The synthetic host tiers of the plan (A14): `-dev` and `-uat` in the name, and the two Okta domains.
fn rules() -> HostRules {
    HostRules::new(vec![
        HostRule {
            tier: short("dev"),
            matcher: HostMatcher::Contains("-dev".to_owned()),
        },
        HostRule {
            tier: short("uat"),
            matcher: HostMatcher::Contains("-uat".to_owned()),
        },
        HostRule {
            tier: short("dev"),
            matcher: HostMatcher::Suffix(".oktapreview.com".to_owned()),
        },
        HostRule {
            tier: short("prod"),
            matcher: HostMatcher::Suffix(".okta.com".to_owned()),
        },
    ])
    .unwrap()
}

fn run_with(
    files: &[Owned],
    deployed: &TenantSet,
    tier: Option<&ShortText>,
    host_rules: &HostRules,
) -> Vec<CheckResult> {
    let views: Vec<CheckFile<'_>> = files.iter().map(Owned::view).collect();
    checks_c1_to_c8(&CheckInput {
        tenants: deployed,
        tier,
        host_rules,
        files: &views,
        limits: &Limits::default(),
    })
}

fn run(files: &[Owned], deployed: &[&str]) -> Vec<CheckResult> {
    run_with(files, &tenants(deployed), None, &HostRules::default())
}

fn of(results: &[CheckResult], kind: FindingKind) -> Vec<&CheckResult> {
    results.iter().filter(|r| r.kind == kind).collect()
}

/// The text form the goldens use.
fn dump(results: &[CheckResult]) -> String {
    let mut out = String::new();
    for r in results {
        writeln!(
            out,
            "{} {} {}",
            r.kind.code(),
            r.kind,
            r.path
                .as_ref()
                .map_or("(no path)".to_owned(), ToString::to_string)
        )
        .unwrap();
        writeln!(out, "  message: {}", r.message).unwrap();
        for l in &r.locations {
            writeln!(out, "  at {} L{}-{}", l.path, l.lines.start(), l.lines.end()).unwrap();
        }
        writeln!(out, "  detail: {:?}", r.detail).unwrap();
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// C1, whole_file: the base file has commits after the tenant file's last commit.
// ---------------------------------------------------------------------------------------------------------------------

const GROUPS_V1: &str = "\
entryGroups:
  branch:
    limit: 10
  web:
    limit: 20
";

const GROUPS_V3: &str = "\
entryGroups:
  branch:
    limit: 15
  web:
    limit: 20
  atm:
    limit: 5
";

/// The tenant copy was made at v1 (time 100); the base has had two commits since.
fn stale_copy() -> Owned {
    Owned::tenant(
        "tx-infinity-api/entryGroupConfig-sit1.yml",
        "tx-infinity-api/entryGroupConfig.yml",
        "sit1",
        GROUPS_V1,
    )
    .history(
        100,
        Some((1, 90, GROUPS_V1)),
        &[
            (
                2,
                200,
                Some("entryGroups:\n  branch:\n    limit: 15\n  web:\n    limit: 20\n"),
            ),
            (3, 300, Some(GROUPS_V3)),
        ],
    )
}

#[test]
fn golden_c1_whole_file() {
    let results = run(&[stale_copy()], &["sit1"]);
    insta::assert_snapshot!("checks_c1_whole_file", dump(&results));
}

#[test]
fn c1_whole_file_reports_the_commits_and_the_setting_changes_since_the_copy() {
    let results = run(&[stale_copy()], &["sit1"]);
    let c1 = of(&results, FindingKind::MissedBaseChanges);
    assert_eq!(c1.len(), 1, "{}", dump(&results));
    let CheckDetail::MissedBaseChanges {
        commits,
        commits_total,
        changes,
        changes_total,
        compared,
        ..
    } = &c1[0].detail
    else {
        panic!("wrong detail: {:?}", c1[0].detail);
    };
    assert_eq!(*commits_total, 2);
    assert_eq!(
        commits.iter().map(|c| c.commit.clone()).collect::<Vec<_>>(),
        vec![commit(2), commit(3)]
    );
    assert!(*compared);
    assert_eq!(*changes_total, 2);
    let listed: Vec<(String, String)> = changes
        .iter()
        .map(|c| (c.path.to_string(), format!("{:?}", c.kind)))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("entryGroups.atm.limit".to_owned(), "Added".to_owned()),
            ("entryGroups.branch.limit".to_owned(), "Changed".to_owned()),
        ]
    );
    assert!(
        c1[0].message.as_str().contains("commit time"),
        "the matching must be said: {}",
        c1[0].message
    );
}

#[test]
fn c1_whole_file_points_at_the_lines_of_the_stale_settings_in_the_tenant_file() {
    let results = run(&[stale_copy()], &["sit1"]);
    let c1 = of(&results, FindingKind::MissedBaseChanges);
    // `atm` is not in the copy, so there is nothing to point at; `branch.limit` is on line 3 of the copy.
    let at: Vec<(String, u32)> = c1[0]
        .locations
        .iter()
        .map(|l| (l.path.to_string(), l.lines.start()))
        .collect();
    assert_eq!(at, vec![("entryGroups.branch.limit".to_owned(), 3)]);
}

#[test]
fn c1_whole_file_is_silent_when_the_base_has_no_commit_after_the_copy() {
    let copy =
        Owned::tenant("a/x-sit1.yml", "a/x.yml", "sit1", "k: 1\n").history(100, Some((1, 90, "k: 1\n")), &[]);
    assert!(of(&run(&[copy], &["sit1"]), FindingKind::MissedBaseChanges).is_empty());
}

#[test]
fn c1_whole_file_ignores_versions_not_after_the_tenant_commit_and_sorts_the_rest() {
    // The caller's list is out of order and holds a version at the very time of the tenant commit: that one is not "after".
    let copy = Owned::tenant("a/x-sit1.yml", "a/x.yml", "sit1", "k: 1\n").history(
        100,
        Some((1, 90, "k: 1\n")),
        &[
            (4, 400, Some("k: 4\n")),
            (2, 100, Some("k: 2\n")),
            (3, 300, Some("k: 3\n")),
        ],
    );
    let results = run(&[copy], &["sit1"]);
    let c1 = of(&results, FindingKind::MissedBaseChanges);
    let CheckDetail::MissedBaseChanges { commits, changes, .. } = &c1[0].detail else {
        panic!()
    };
    assert_eq!(
        commits.iter().map(|c| c.commit.clone()).collect::<Vec<_>>(),
        vec![commit(3), commit(4)]
    );
    // The change is read between the copy's version and the newest one.
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path.to_string(), "k");
}

#[test]
fn c1_whole_file_without_the_old_base_version_reports_commits_but_no_setting_changes() {
    let copy = Owned::tenant("a/x-sit1.yml", "a/x.yml", "sit1", "k: 1\n").history(
        100,
        None,
        &[(2, 200, Some("k: 2\n"))],
    );
    let results = run(&[copy], &["sit1"]);
    let CheckDetail::MissedBaseChanges {
        commits_total,
        changes,
        compared,
        ..
    } = &of(&results, FindingKind::MissedBaseChanges)[0].detail
    else {
        panic!()
    };
    assert_eq!((*commits_total, changes.len(), *compared), (1, 0, false));
}

#[test]
fn c1_whole_file_with_an_unparsable_base_version_reports_commits_and_says_it_did_not_compare() {
    let copy = Owned::tenant("a/x-sit1.yml", "a/x.yml", "sit1", "k: 1\n").history(
        100,
        Some((1, 90, "k: 1\n")),
        &[(2, 200, Some("k: [unclosed\n"))],
    );
    let results = run(&[copy], &["sit1"]);
    let c1 = of(&results, FindingKind::MissedBaseChanges);
    assert_eq!(c1.len(), 1);
    let CheckDetail::MissedBaseChanges { compared, .. } = &c1[0].detail else {
        panic!()
    };
    assert!(!*compared);
}

#[test]
fn c1_whole_file_on_a_text_file_reports_commits_only() {
    let copy = Owned::raw(
        "doc/t-sit1.xsl",
        "doc/t.xsl",
        Some("sit1"),
        FileClass::Text,
        b"<a/>",
    )
    .history(100, Some((1, 90, "<a/>")), &[(2, 200, Some("<b/>"))]);
    let results = run(&[copy], &["sit1"]);
    let c1 = of(&results, FindingKind::MissedBaseChanges);
    assert_eq!(c1.len(), 1);
    let CheckDetail::MissedBaseChanges {
        commits_total,
        changes,
        compared,
        ..
    } = &c1[0].detail
    else {
        panic!()
    };
    assert_eq!((*commits_total, changes.len(), *compared), (1, 0, false));
}

#[test]
fn c1_whole_file_when_the_newest_base_version_deleted_the_file_does_not_compare() {
    let copy = Owned::tenant("a/x-sit1.yml", "a/x.yml", "sit1", "k: 1\n").history(
        100,
        Some((1, 90, "k: 1\n")),
        &[(2, 200, None)],
    );
    let results = run(&[copy], &["sit1"]);
    let CheckDetail::MissedBaseChanges {
        compared,
        commits_total,
        ..
    } = &of(&results, FindingKind::MissedBaseChanges)[0].detail
    else {
        panic!()
    };
    assert_eq!((*commits_total, *compared), (1, false));
}

#[test]
fn c1_skips_a_tenant_file_of_a_tenant_the_swimlane_does_not_deploy() {
    // It is never read (C4), so it cannot be behind.
    assert!(of(&run(&[stale_copy()], &["sit2"]), FindingKind::MissedBaseChanges).is_empty());
}

#[test]
fn c1_never_fires_for_a_base_file_or_a_file_without_history() {
    let base = Owned::base("a/x.yml", "k: 1\n");
    let no_history = Owned::tenant("a/y-sit1.yml", "a/y.yml", "sit1", "k: 1\n");
    assert!(
        of(
            &run(&[base, no_history], &["sit1"]),
            FindingKind::MissedBaseChanges
        )
        .is_empty()
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// C1, spring_merge: dead overrides.
// ---------------------------------------------------------------------------------------------------------------------

const SPRING_BASE: &str = "\
server:
  port: 8080
tasks:
  - a
  - b
users:
  - name: x
    role: y
";

const SPRING_TENANT: &str = "\
server:
  port: 9090
  old: 1
tasks:
  - c
users:
  - name: z
    shiny: true
removed:
  gone: 2
from_root: 3
";

fn spring_pair(root: Option<&str>) -> Vec<Owned> {
    let base = Owned::base("svc/app.yml", SPRING_BASE).mode(MergeMode::SpringMerge);
    let mut tenant =
        Owned::tenant("svc/app-sit1.yml", "svc/app.yml", "sit1", SPRING_TENANT).mode(MergeMode::SpringMerge);
    if let Some(root) = root {
        tenant = tenant.root(root);
    }
    vec![base, tenant]
}

#[test]
fn golden_c1_spring_merge_dead_overrides() {
    let results = run(&spring_pair(Some("from_root: 1\n")), &["sit1"]);
    insta::assert_snapshot!("checks_c1_spring_merge_dead_overrides", dump(&results));
}

#[test]
fn c1_spring_merge_flags_exactly_the_tenant_settings_whose_base_setting_is_gone() {
    let results = run(&spring_pair(Some("from_root: 1\n")), &["sit1"]);
    let c1 = of(&results, FindingKind::MissedBaseChanges);
    assert_eq!(c1.len(), 1, "{}", dump(&results));
    let CheckDetail::DeadOverrides { settings, total } = &c1[0].detail else {
        panic!("{:?}", c1[0].detail)
    };
    // `server.old` and `removed.gone` have no base setting. `server.port` exists, `tasks[0]` replaces a list the base has,
    // `users[0].name` and `.shiny` are items of a list the base has, and `from_root` comes from the root file.
    assert_eq!(
        settings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec!["removed.gone", "server.old"]
    );
    assert_eq!(*total, 2);
    let lines: Vec<(String, u32)> = c1[0]
        .locations
        .iter()
        .map(|l| (l.path.to_string(), l.lines.start()))
        .collect();
    assert_eq!(
        lines,
        vec![("removed.gone".to_owned(), 10), ("server.old".to_owned(), 3)]
    );
}

#[test]
fn c1_spring_merge_without_the_root_file_flags_a_setting_only_the_root_defines() {
    let results = run(&spring_pair(None), &["sit1"]);
    let CheckDetail::DeadOverrides { settings, .. } = &of(&results, FindingKind::MissedBaseChanges)[0].detail
    else {
        panic!()
    };
    assert!(settings.iter().any(|s| s.as_str() == "from_root"));
}

#[test]
fn c1_spring_merge_without_a_base_file_has_nothing_to_be_dead_against() {
    let tenant =
        Owned::tenant("svc/app-sit1.yml", "svc/app.yml", "sit1", SPRING_TENANT).mode(MergeMode::SpringMerge);
    assert!(of(&run(&[tenant], &["sit1"]), FindingKind::MissedBaseChanges).is_empty());
}

#[test]
fn c1_spring_merge_a_scalar_over_a_map_and_a_map_over_a_scalar_are_live() {
    let base = Owned::base("svc/app.yml", "a:\n  b: 1\nc: 2\n").mode(MergeMode::SpringMerge);
    let tenant = Owned::tenant("svc/app-sit1.yml", "svc/app.yml", "sit1", "a: 5\nc:\n  d: 1\n")
        .mode(MergeMode::SpringMerge);
    assert!(of(&run(&[base, tenant], &["sit1"]), FindingKind::MissedBaseChanges).is_empty());
}

#[test]
fn c1_spring_merge_everything_live_is_silent() {
    let base = Owned::base("svc/app.yml", "a: 1\nb: 2\n").mode(MergeMode::SpringMerge);
    let tenant =
        Owned::tenant("svc/app-sit1.yml", "svc/app.yml", "sit1", "a: 9\n").mode(MergeMode::SpringMerge);
    assert!(run(&[base, tenant], &["sit1"]).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------
// C2: a tenant file equal to base.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn golden_c2_semantic_and_byte_equal() {
    let files = vec![
        // Same settings, other order, comments and CRLF: semantically equal.
        Owned::base("tx/account-sorting-config.yml", "a: 1\nb:\n  c: 2\n"),
        Owned::tenant(
            "tx/account-sorting-config-sit1.yml",
            "tx/account-sorting-config.yml",
            "sit1",
            "# copied\r\nb:\r\n  c: 2\r\na: 1\r\n",
        ),
        // Text, equal apart from line endings: equal.
        Owned::raw("tx/t.xsl", "tx/t.xsl", None, FileClass::Text, b"<a>\n</a>\n"),
        Owned::raw(
            "tx/t-sit1.xsl",
            "tx/t.xsl",
            Some("sit1"),
            FileClass::Text,
            b"<a>\r\n</a>\r\n",
        ),
        // Binary, byte-equal.
        Owned::raw(
            "doc/receipt.bmp",
            "doc/receipt.bmp",
            None,
            FileClass::Binary,
            b"BM\x00\x01",
        ),
        Owned::raw(
            "doc/receipt-sit1.bmp",
            "doc/receipt.bmp",
            Some("sit1"),
            FileClass::Binary,
            b"BM\x00\x01",
        ),
        // Not equal: one value differs.
        Owned::base("tx/other.yml", "a: 1\n"),
        Owned::tenant("tx/other-sit1.yml", "tx/other.yml", "sit1", "a: 2\n"),
    ];
    let results = run(&files, &["sit1"]);
    insta::assert_snapshot!("checks_c2_redundant", dump(&results));
}

#[test]
fn c2_flags_semantic_equality_for_structured_files_and_byte_equality_for_the_rest() {
    let files = vec![
        Owned::base("tx/a.yml", "x: 1\ny: 2\n"),
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "y: 2\nx: 1\n"),
        Owned::raw("doc/b.bmp", "doc/b.bmp", None, FileClass::Binary, b"BM1"),
        Owned::raw(
            "doc/b-sit1.bmp",
            "doc/b.bmp",
            Some("sit1"),
            FileClass::Binary,
            b"BM2",
        ),
    ];
    let results = run(&files, &["sit1"]);
    let c2 = of(&results, FindingKind::RedundantTenantFile);
    assert_eq!(c2.len(), 1, "{}", dump(&results));
    assert_eq!(c2[0].path.as_ref().unwrap().as_str(), "tx/a-sit1.yml");
    let CheckDetail::RedundantTenantFile { tenant: t, equal_by } = &c2[0].detail else {
        panic!()
    };
    assert_eq!(t.as_str(), "sit1");
    assert_eq!(format!("{equal_by:?}"), "Settings");
}

#[test]
fn c2_yes_and_true_are_not_equal() {
    let files = vec![
        Owned::base("tx/a.yml", "x: true\n"),
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: yes\n"),
    ];
    assert!(of(&run(&files, &["sit1"]), FindingKind::RedundantTenantFile).is_empty());
}

#[test]
fn c2_needs_a_base_file_and_a_deployed_tenant() {
    let only_tenant = Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 1\n");
    assert!(of(&run(&[only_tenant], &["sit1"]), FindingKind::RedundantTenantFile).is_empty());

    let files = vec![
        Owned::base("tx/a.yml", "x: 1\n"),
        Owned::tenant("tx/a-sit2.yml", "tx/a.yml", "sit2", "x: 1\n"),
    ];
    assert!(of(&run(&files, &["sit1"]), FindingKind::RedundantTenantFile).is_empty());
}

#[test]
fn c2_one_finding_per_tenant_file() {
    let files = vec![
        Owned::base("tx/a.yml", "x: 1\n"),
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 1\n"),
        Owned::tenant("tx/a-sit2.yml", "tx/a.yml", "sit2", "x: 1\n"),
    ];
    let paths: Vec<String> = of(&run(&files, &["sit1", "sit2"]), FindingKind::RedundantTenantFile)
        .iter()
        .map(|r| r.path.as_ref().unwrap().to_string())
        .collect();
    assert_eq!(paths, vec!["tx/a-sit1.yml", "tx/a-sit2.yml"]);
}

// ---------------------------------------------------------------------------------------------------------------------
// C3: tenant files for some of the swimlane's tenants and not for others.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn golden_c3() {
    let files = vec![
        Owned::base("tx/a.yml", "x: 1\n"),
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 2\n"),
        Owned::tenant("tx/a-sit2.yml", "tx/a.yml", "sit2", "x: 3\n"),
        // All three tenants have this one.
        Owned::tenant("tx/b-sit1.yml", "tx/b.yml", "sit1", "x: 1\n"),
        Owned::tenant("tx/b-sit2.yml", "tx/b.yml", "sit2", "x: 2\n"),
        Owned::tenant("tx/b-sit3.yml", "tx/b.yml", "sit3", "x: 3\n"),
        // Only one tenant has this one, and there is no base file.
        Owned::tenant("tx/c-sit3.yml", "tx/c.yml", "sit3", "x: 1\n"),
    ];
    let results = run(&files, &["sit1", "sit2", "sit3"]);
    insta::assert_snapshot!("checks_c3_uneven", dump(&results));
}

#[test]
fn c3_names_the_tenants_with_and_without_the_file() {
    let files = vec![
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 2\n"),
        Owned::tenant("tx/a-sit3.yml", "tx/a.yml", "sit3", "x: 3\n"),
    ];
    let results = run(&files, &["sit1", "sit2", "sit3"]);
    let c3 = of(&results, FindingKind::UnevenTenantFiles);
    assert_eq!(c3.len(), 1);
    assert_eq!(
        c3[0].path.as_ref().unwrap().as_str(),
        "tx/a.yml",
        "the finding is about the logical file"
    );
    let CheckDetail::UnevenTenantFiles {
        with_file,
        without_file,
    } = &c3[0].detail
    else {
        panic!()
    };
    assert_eq!(
        with_file.iter().map(TenantId::as_str).collect::<Vec<_>>(),
        vec!["sit1", "sit3"]
    );
    assert_eq!(
        without_file.iter().map(TenantId::as_str).collect::<Vec<_>>(),
        vec!["sit2"]
    );
}

#[test]
fn c3_a_swimlane_with_one_tenant_is_never_uneven() {
    let files = vec![Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 2\n")];
    assert!(of(&run(&files, &["sit1"]), FindingKind::UnevenTenantFiles).is_empty());
}

#[test]
fn c3_ignores_tenant_files_of_tenants_the_swimlane_does_not_deploy() {
    // sit9 is not deployed; the file for it is an orphan (C4), not evidence that sit1 and sit2 are uneven.
    let files = vec![
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 1\n"),
        Owned::tenant("tx/a-sit2.yml", "tx/a.yml", "sit2", "x: 1\n"),
        Owned::tenant("tx/a-sit9.yml", "tx/a.yml", "sit9", "x: 1\n"),
    ];
    assert!(of(&run(&files, &["sit1", "sit2"]), FindingKind::UnevenTenantFiles).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------
// C4: orphans.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn golden_c4_other_branch_suffix_and_untracked() {
    let files = vec![
        Owned::base("tx/fine.yml", "x: 1\n"),
        Owned::tenant("tx/fine-sit1.yml", "tx/fine.yml", "sit1", "x: 1\n"),
        // The suffix of a branch the swimlane does not deploy: no service of the swimlane reads it.
        Owned::tenant("tx/stray-sit2.yml", "tx/stray.yml", "sit2", "x: 1\n"),
        // No Git source at all.
        Owned::base("tx/scratch.yml", "x: 1\n").kind(FileKind::Untracked),
        // Untracked and tenant-looking, for a deployed tenant: still an orphan, so that it can be adopted.
        Owned::tenant("tx/new-sit1.yml", "tx/new.yml", "sit1", "x: 1\n").kind(FileKind::Untracked),
        // Untracked and for a branch that is not deployed: the suffix is the first thing to say.
        Owned::tenant("tx/odd-sit2.yml", "tx/odd.yml", "sit2", "x: 1\n").kind(FileKind::Untracked),
    ];
    let results = run(&files, &["sit1"]);
    insta::assert_snapshot!("checks_c4_orphans", dump(&results));
}

#[test]
fn c4_reasons() {
    let files = vec![
        Owned::tenant("tx/stray-sit2.yml", "tx/stray.yml", "sit2", "x: 1\n"),
        Owned::base("tx/scratch.yml", "x: 1\n").kind(FileKind::Untracked),
    ];
    let results = run(&files, &["sit1"]);
    let reasons: Vec<String> = of(&results, FindingKind::OrphanFile)
        .iter()
        .map(|r| match &r.detail {
            CheckDetail::OrphanFile { reason } => format!("{} {reason:?}", r.path.as_ref().unwrap()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        reasons,
        vec![
            "tx/scratch.yml Untracked".to_owned(),
            "tx/stray-sit2.yml TenantNotDeployed(TenantId(\"sit2\"))".to_owned(),
        ]
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// C5: drift.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn golden_c5() {
    let labelled = |state, label| Drift {
        label: Some(label),
        ..drift(state)
    };
    let files = vec![
        Owned::base("d/in_sync.yml", "a: 1\n").drift(drift(DriftState::InSync)),
        Owned::base("d/intentional.yml", "a: 1\n").drift(drift(DriftState::IntentionalDivergence)),
        Owned::base("d/git_ahead.yml", "a: 1\n").drift(drift(DriftState::GitAhead)),
        Owned::base("d/nfs_ahead.yml", "a: 1\n").drift(drift(DriftState::NfsAhead)),
        Owned::base("d/conflict.yml", "a: 1\n").drift(drift(DriftState::Conflict)),
        Owned::base("d/unknown.yml", "a: 1\n").drift(drift(DriftState::Unknown)),
        Owned::base("d/untracked.yml", "a: 1\n").drift(drift(DriftState::Untracked)),
        Owned::base("d/deleted_on_nfs.yml", "a: 1\n")
            .drift(labelled(DriftState::NfsAhead, engine::DriftLabel::DeletedOnNfs)),
        Owned::base("d/new_in_git.yml", "a: 1\n")
            .drift(labelled(DriftState::GitAhead, engine::DriftLabel::NewInGit)),
        Owned::base("d/deleted_in_git.yml", "a: 1\n")
            .drift(labelled(DriftState::GitAhead, engine::DriftLabel::DeletedInGit)),
        Owned::base("d/no_drift_input.yml", "a: 1\n"),
    ];
    let results = run(&files, &[]);
    insta::assert_snapshot!("checks_c5_drift", dump(&results));
}

#[test]
fn c5_reports_every_state_but_in_sync_and_intentional_divergence() {
    let states = [
        (DriftState::InSync, false),
        (DriftState::IntentionalDivergence, false),
        (DriftState::GitAhead, true),
        (DriftState::NfsAhead, true),
        (DriftState::Conflict, true),
        (DriftState::Unknown, true),
        (DriftState::Untracked, true),
    ];
    for (state, flagged) in states {
        let results = run(&[Owned::base("d/x.yml", "a: 1\n").drift(drift(state))], &[]);
        assert_eq!(
            of(&results, FindingKind::Drift).len(),
            usize::from(flagged),
            "{state:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// C6: environment host mismatch.
// ---------------------------------------------------------------------------------------------------------------------

const HOSTS: &str = "\
video:
  url: https://video-dev.example.com:8443/stream
auth:
  issuer: https://login.oktapreview.com/oauth2/default
  fine: https://login.okta.com/oauth2/default
db: user:secret@host
local:
  cluster: http://core-adapter-symxchange:8080/api
  fqdn_free: core-adapter-symxchange
  address: http://10.1.2.3:8080
plain: video-dev.example.com
";

fn host_file(text: &str) -> Owned {
    Owned::tenant("svc/app-sit1.yml", "svc/app.yml", "sit1", text)
}

fn c6(text: &str, tier: &str) -> Vec<CheckResult> {
    run_with(
        &[host_file(text)],
        &tenants(&["sit1"]),
        Some(&short(tier)),
        &rules(),
    )
}

#[test]
fn golden_c6_cluster_local_excluded() {
    let results = c6(HOSTS, "prod");
    insta::assert_snapshot!("checks_c6_hosts", dump(&results));
}

fn mismatched(results: &[CheckResult]) -> Vec<(String, String, String)> {
    of(results, FindingKind::EnvironmentHostMismatch)
        .iter()
        .flat_map(|r| match &r.detail {
            CheckDetail::EnvironmentHostMismatch { hosts, .. } => hosts
                .iter()
                .map(|h| (h.path.to_string(), h.host.clone(), h.host_tier.to_string()))
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn c6_flags_hosts_of_another_tier_and_nothing_else() {
    let got = mismatched(&c6(HOSTS, "prod"));
    assert_eq!(
        got,
        vec![
            (
                "auth.issuer".to_owned(),
                "login.oktapreview.com".to_owned(),
                "dev".to_owned()
            ),
            (
                "plain".to_owned(),
                "video-dev.example.com".to_owned(),
                "dev".to_owned()
            ),
            (
                "video.url".to_owned(),
                "video-dev.example.com".to_owned(),
                "dev".to_owned()
            ),
        ]
    );
}

#[test]
fn c6_a_host_of_the_swimlanes_own_tier_is_fine() {
    assert!(
        mismatched(&c6(HOSTS, "dev"))
            .iter()
            .all(|(_, _, tier)| tier != "dev")
    );
    // For a dev swimlane the prod Okta host is the mismatch.
    assert_eq!(
        mismatched(&c6(HOSTS, "dev")),
        vec![(
            "auth.fine".to_owned(),
            "login.okta.com".to_owned(),
            "prod".to_owned()
        )]
    );
}

#[test]
fn c6_cluster_local_names_and_addresses_are_excluded() {
    let text = "a: http://core-adapter-dev:8080/x\nb: core-adapter-dev\nc: http://10.0.0.1/x\nd: http://[::1]:80/x\n";
    // `core-adapter-dev` matches the `-dev` rule but has no dot, so it is a service name, not an external host.
    assert!(mismatched(&c6(text, "prod")).is_empty());
}

#[test]
fn c6_ignores_user_info_ports_paths_and_case() {
    let text = "a: HTTPS://admin:pw@Video-DEV.Example.com:8443/x?y=1#z\n";
    assert_eq!(
        mismatched(&c6(text, "prod")),
        vec![(
            "a".to_owned(),
            "video-dev.example.com".to_owned(),
            "dev".to_owned()
        )]
    );
}

#[test]
fn c6_finds_several_hosts_in_one_value() {
    let text = "a: \"https://one-dev.example.com/x,https://two-uat.example.com/y\"\n";
    let got = mismatched(&c6(text, "prod"));
    assert_eq!(got.len(), 2, "{got:?}");
}

#[test]
fn c6_the_first_matching_rule_names_the_tier() {
    let rules = HostRules::new(vec![
        HostRule {
            tier: short("uat"),
            matcher: HostMatcher::Contains("-uat".to_owned()),
        },
        HostRule {
            tier: short("dev"),
            matcher: HostMatcher::Contains("-dev".to_owned()),
        },
    ])
    .unwrap();
    let file = host_file("a: https://x-dev-uat.example.com/\n");
    let results = run_with(&[file], &tenants(&["sit1"]), Some(&short("prod")), &rules);
    assert_eq!(mismatched(&results)[0].2, "uat");
}

#[test]
fn c6_is_silent_without_a_tier_without_rules_and_for_hosts_no_rule_names() {
    let file = host_file("a: https://video-dev.example.com/\n");
    assert!(
        run_with(
            &[host_file("a: https://video-dev.example.com/\n")],
            &tenants(&["sit1"]),
            None,
            &rules()
        )
        .is_empty()
    );
    assert!(
        run_with(
            &[file],
            &tenants(&["sit1"]),
            Some(&short("prod")),
            &HostRules::default()
        )
        .is_empty()
    );
    assert!(mismatched(&c6("a: https://unclassified.example.org/\n", "prod")).is_empty());
}

#[test]
fn c6_does_not_look_at_the_keys_or_at_non_string_values() {
    let text = "https://video-dev.example.com: 1\nn: 5\nb: true\n";
    assert!(mismatched(&c6(text, "prod")).is_empty());
}

#[test]
fn c6_lists_and_locations_are_capped_but_counted() {
    let mut text = String::new();
    for i in 0..80 {
        writeln!(text, "k{i}: https://h{i}-dev.example.com/").unwrap();
    }
    let results = c6(&text, "prod");
    let r = &of(&results, FindingKind::EnvironmentHostMismatch)[0];
    let CheckDetail::EnvironmentHostMismatch { hosts, total, .. } = &r.detail else {
        panic!()
    };
    assert_eq!((hosts.len(), *total), (50, 80));
    assert_eq!(r.locations.len(), 50);
}

#[test]
fn host_rules_are_validated_and_never_printed() {
    let bad = |needle: &str| {
        HostRules::new(vec![HostRule {
            tier: short("dev"),
            matcher: HostMatcher::Contains(needle.to_owned()),
        }])
    };
    assert_eq!(bad("").unwrap_err(), HostRulesError::EmptyNeedle);
    assert_eq!(bad(&"a".repeat(254)).unwrap_err(), HostRulesError::NeedleTooLong);
    assert_eq!(bad("-dév").unwrap_err(), HostRulesError::InvalidNeedle);
    let many: Vec<HostRule> = (0..=HostRules::MAX_RULES)
        .map(|i| HostRule {
            tier: short("dev"),
            matcher: HostMatcher::Contains(format!("-d{i}")),
        })
        .collect();
    assert_eq!(HostRules::new(many).unwrap_err(), HostRulesError::TooManyRules);
    // Needles are matched in lower case.
    let ok = HostRules::new(vec![HostRule {
        tier: short("dev"),
        matcher: HostMatcher::Suffix(".OktaPreview.com".to_owned()),
    }])
    .unwrap();
    assert_eq!(ok.tier_of("a.oktapreview.com").unwrap().as_str(), "dev");
    let shown = format!("{ok:?}");
    assert!(!shown.contains("oktapreview"), "{shown}");
}

// ---------------------------------------------------------------------------------------------------------------------
// C7: a sync replaced a change that was only on NFS.
// ---------------------------------------------------------------------------------------------------------------------

fn overwritten(
    previous_state: DriftState,
    previous_nfs: Option<ContentHash>,
    source: AttributionSource,
) -> Owned {
    Owned::base("tx/synced.yml", "a: 1\n").overwrite(Overwrite {
        previous_state,
        previous_nfs,
        source,
    })
}

#[test]
fn golden_c7_links_lost_version() {
    let files = vec![
        overwritten(DriftState::NfsAhead, Some(h(7)), AttributionSource::SyncJob),
        Owned::base("tx/deleted.yml", "a: 1\n").overwrite(Overwrite {
            previous_state: DriftState::NfsAhead,
            previous_nfs: None,
            source: AttributionSource::SyncJob,
        }),
        Owned::base("tx/conflicted.yml", "a: 1\n").overwrite(Overwrite {
            previous_state: DriftState::Conflict,
            previous_nfs: Some(h(8)),
            source: AttributionSource::SyncJob,
        }),
    ];
    let results = run(&files, &[]);
    insta::assert_snapshot!("checks_c7_overwritten", dump(&results));
}

#[test]
fn c7_fires_for_a_sync_over_nfs_ahead_or_conflict_and_links_the_lost_version() {
    let results = run(
        &[overwritten(
            DriftState::NfsAhead,
            Some(h(7)),
            AttributionSource::SyncJob,
        )],
        &[],
    );
    let c7 = of(&results, FindingKind::OverwrittenNfsChange);
    assert_eq!(c7.len(), 1);
    let CheckDetail::OverwrittenNfsChange {
        previous_state,
        lost_version,
        source,
    } = &c7[0].detail
    else {
        panic!()
    };
    assert_eq!(
        (*previous_state, *lost_version, *source),
        (DriftState::NfsAhead, Some(h(7)), AttributionSource::SyncJob)
    );
    assert!(
        run(
            &[overwritten(
                DriftState::Conflict,
                Some(h(7)),
                AttributionSource::SyncJob
            )],
            &[]
        )
        .iter()
        .any(|r| r.kind == FindingKind::OverwrittenNfsChange)
    );
}

#[test]
fn c7_is_silent_for_other_states_other_sources_and_an_unchanged_file() {
    for state in [
        DriftState::InSync,
        DriftState::GitAhead,
        DriftState::Unknown,
        DriftState::Untracked,
        DriftState::IntentionalDivergence,
    ] {
        let results = run(&[overwritten(state, Some(h(7)), AttributionSource::SyncJob)], &[]);
        assert!(
            of(&results, FindingKind::OverwrittenNfsChange).is_empty(),
            "{state:?}"
        );
    }
    for source in [
        AttributionSource::ToolWrite,
        AttributionSource::SentinelLogin,
        AttributionSource::NfsClient,
        AttributionSource::FsOwnerHint,
        AttributionSource::Unknown,
    ] {
        let results = run(&[overwritten(DriftState::NfsAhead, Some(h(7)), source)], &[]);
        assert!(
            of(&results, FindingKind::OverwrittenNfsChange).is_empty(),
            "{source:?}"
        );
    }
    // The file on NFS now is the one that was there before: nothing was replaced.
    let same = Owned::base("tx/same.yml", "a: 1\n");
    let same_hash = same.hash;
    let same = same.overwrite(Overwrite {
        previous_state: DriftState::NfsAhead,
        previous_nfs: Some(same_hash),
        source: AttributionSource::SyncJob,
    });
    assert!(of(&run(&[same], &[]), FindingKind::OverwrittenNfsChange).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------
// C8: duplicate keys.
// ---------------------------------------------------------------------------------------------------------------------

const ROLES: &str = "\
roles:
  txRemoteJuniorTeller:
    level: 1
  txRemoteSeniorTeller:
    level: 2
  txRemoteSupervisor:
    level: 3
  txRemoteJuniorTeller:
    level: 4
  txRemoteSeniorTeller:
    level: 5
  txRemoteSupervisor:
    level: 6
  txRemoteSupervisor:
    level: 7
other: 1
";

#[test]
fn golden_c8_three_keys() {
    let results = run(&[Owned::base("security/security-roles.yml", ROLES)], &[]);
    insta::assert_snapshot!("checks_c8_duplicate_keys", dump(&results));
}

#[test]
fn c8_reports_each_duplicated_key_once_with_every_line() {
    let results = run(&[Owned::base("security/security-roles.yml", ROLES)], &[]);
    let c8 = of(&results, FindingKind::DuplicateKeys);
    assert_eq!(c8.len(), 1, "one finding per file: {}", dump(&results));
    let CheckDetail::DuplicateKeys {
        keys,
        total,
        more_not_listed,
    } = &c8[0].detail
    else {
        panic!()
    };
    assert_eq!((*total, *more_not_listed), (3, false));
    let got: Vec<(String, Vec<u32>)> = keys
        .iter()
        .map(|k| (k.path.as_ref().unwrap().to_string(), k.lines.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("roles.txRemoteJuniorTeller".to_owned(), vec![2, 8]),
            ("roles.txRemoteSeniorTeller".to_owned(), vec![4, 10]),
            ("roles.txRemoteSupervisor".to_owned(), vec![6, 12, 14]),
        ]
    );
}

#[test]
fn c8_is_silent_for_a_clean_file_and_a_file_that_is_not_structured() {
    let files = vec![
        Owned::base("a/clean.yml", "a: 1\nb: 2\n"),
        Owned::raw("a/t.xsl", "a/t.xsl", None, FileClass::Text, b"<a/>"),
    ];
    assert!(of(&run(&files, &[]), FindingKind::DuplicateKeys).is_empty());
}

#[test]
fn c8_says_when_the_parser_dropped_warnings() {
    let mut file = Owned::base("a/dup.yml", "a: 1\na: 2\n");
    file.parse_dropped = 5;
    let results = run(&[file], &[]);
    let CheckDetail::DuplicateKeys { more_not_listed, .. } =
        &of(&results, FindingKind::DuplicateKeys)[0].detail
    else {
        panic!()
    };
    assert!(*more_not_listed);
}

#[test]
fn c8_caps_the_list_and_the_locations_but_counts_everything() {
    let mut text = String::new();
    for i in 0..60 {
        writeln!(text, "k{i}: 1\nk{i}: 2").unwrap();
    }
    let results = run(&[Owned::base("a/many.yml", &text)], &[]);
    let r = &of(&results, FindingKind::DuplicateKeys)[0];
    let CheckDetail::DuplicateKeys { keys, total, .. } = &r.detail else {
        panic!()
    };
    assert_eq!((keys.len(), *total, r.locations.len()), (50, 60, 50));
}

// ---------------------------------------------------------------------------------------------------------------------
// The whole: order, determinism, bounded messages.
// ---------------------------------------------------------------------------------------------------------------------

fn swimlane() -> Vec<Owned> {
    vec![
        Owned::base("security/security-roles.yml", ROLES),
        Owned::base("tx/a.yml", "x: 1\n").drift(drift(DriftState::NfsAhead)),
        Owned::tenant("tx/a-sit1.yml", "tx/a.yml", "sit1", "x: 1\n"),
        Owned::tenant("tx/a-sit2.yml", "tx/a.yml", "sit2", "x: 1\n"),
        stale_copy(),
        Owned::base("tx/scratch.yml", "x: 1\n").kind(FileKind::Untracked),
    ]
}

#[test]
fn results_are_sorted_by_check_then_path() {
    let results = run(&swimlane(), &["sit1", "sit2"]);
    let keys: Vec<(FindingKind, String)> = results
        .iter()
        .map(|r| {
            (
                r.kind,
                r.path.as_ref().map(ToString::to_string).unwrap_or_default(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert!(results.len() >= 5, "{}", dump(&results));
}

#[test]
fn the_order_of_the_files_does_not_change_the_results() {
    let forward = dump(&run(&swimlane(), &["sit1", "sit2"]));
    let mut reversed = swimlane();
    reversed.reverse();
    assert_eq!(forward, dump(&run(&reversed, &["sit1", "sit2"])));
}

#[test]
fn a_check_that_finds_nothing_returns_an_empty_list() {
    assert!(run(&[], &["sit1"]).is_empty());
    assert!(run(&[Owned::base("a/x.yml", "k: 1\n")], &["sit1"]).is_empty());
}

#[test]
fn no_value_reaches_a_message_a_detail_or_a_debug_print() {
    let secret = "S3CR3T-PASSWORD-VALUE";
    let files = vec![
        Owned::base(
            "a/x.yml",
            &format!("db:\n  pw: {secret}\n  host: https://x-dev.example.com/\n  pw: other\n"),
        ),
        Owned::tenant(
            "a/x-sit1.yml",
            "a/x.yml",
            "sit1",
            &format!("db:\n  pw: {secret}\n"),
        ),
    ];
    let results = run_with(&files, &tenants(&["sit1"]), Some(&short("prod")), &rules());
    assert!(results.len() >= 2, "{}", dump(&results));
    let shown = format!("{results:?}{}", dump(&results));
    assert!(!shown.contains(secret), "a value reached the output: {shown}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the keys, tenants and paths are, every message is a valid `ShortText` (at most 256 bytes, no control
    /// characters) and the checks never panic.
    #[test]
    fn finding_message_fits_short_text_prop(
        key in "[a-zA-Z0-9_]{1,300}",
        extra in "[a-z]{1,40}",
        host in "[a-z0-9-]{1,60}",
        dirs in prop::collection::vec("[a-z]{1,30}", 0..6),
    ) {
        let dir = if dirs.is_empty() { String::new() } else { format!("{}/", dirs.join("/")) };
        let base_path = format!("{dir}{extra}.yml");
        let tenant_path = format!("{dir}{extra}-sit1.yml");
        let dup = format!("{key}: 1\n{key}: 2\nu: https://{host}-dev.example.com/\n");
        let files = vec![
            Owned::base(&base_path, &dup),
            Owned::tenant(&tenant_path, &base_path, "sit1", &dup).drift(drift(DriftState::Conflict)),
            Owned::tenant(&format!("{dir}{extra}-sit2.yml"), &base_path, "sit2", "k: 1\n"),
        ];
        let results = run_with(&files, &tenants(&["sit1", "sit3"]), Some(&short("prod")), &rules());
        prop_assert!(!results.is_empty());
        for r in &results {
            prop_assert!(r.message.as_str().len() <= 256);
            prop_assert!(!r.message.as_str().chars().any(char::is_control));
            prop_assert!(!r.message.as_str().is_empty());
        }
    }
}
