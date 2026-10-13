//! The opt-in run against the real repositories (plan decision A4, acceptance 4). It needs two working-tree checkouts that
//! this repository never holds (AGENTS.md rule 12):
//!
//! - `LK_REAL_BASE_DIR`: `configuration-base-saas`, default branch (the `config/` folder, or the checkout itself);
//! - `LK_REAL_TENANT_DIR`: `csp-tenant-data`, branch `sit1` (the `data/config/` folder, or the checkout itself).
//!
//! Run it with `just engine-real-snapshot`. Without the two variables the recipe says `real-snapshot: NOT RUN` and the gate
//! does not count it as passed; the test itself refuses to run, so it can never pass vacuously.
//!
//! What it prints is counts, file names and setting paths. Never a value and never a line of a file: the files may hold
//! secrets, and a log is not a place for them (S10, S13).
//!
//! T3 adds the first expectation: on every structured file of both checkouts, tree-sitter and the parser agree on where every
//! setting is. T6 adds the checks of `sit1`: C2 for three files, C1 for the missing `atm` entry (this one reads the history of
//! both checkouts with `git`, which therefore have to be clones and not exports), and C8 for the three keys of
//! `security-roles.yml`. Later tasks add the rest (the 13 ambiguous names of C9 in T10).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use domain::{
    CommitId, ContentHash, FileClass, FindingKind, MergeMode, NfsPath, PathMappingRule, RepoPath, TenantId,
    TenantSet, Timestamp,
};
use engine::{
    BaseHistory, BaseVersion, CheckDetail, CheckFile, CheckInput, CheckResult, DenyMatcher, FileFacts, Flat,
    HostRules, Limits, LocationSource, ParseWarning, RepoSource, Structured, Syntax, c1_missed_base_changes,
    c8_duplicate_keys, checks_c1_to_c8, classify, classify_path, flatten, index_settings, map_path,
    normalised_hash, parse,
};

/// The checkouts named by the environment. Panics when they are not set: a run that cannot happen is not a run that passed.
fn real_dirs() -> (PathBuf, PathBuf) {
    let read = |name: &str| -> PathBuf {
        let value = std::env::var_os(name).unwrap_or_else(|| {
            panic!("real-snapshot: NOT RUN ({name} is not set; see tests/real_snapshot.rs)")
        });
        let dir = PathBuf::from(value);
        assert!(dir.is_dir(), "real-snapshot: {name} is not a directory");
        dir
    };
    (read("LK_REAL_BASE_DIR"), read("LK_REAL_TENANT_DIR"))
}

fn syntax_of(name: &str) -> Option<Syntax> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "yml" | "yaml" => Some(Syntax::Yaml),
        "json" => Some(Syntax::Json),
        "properties" => Some(Syntax::Properties),
        _ => None,
    }
}

/// Every regular file below `dir` (not following links, not entering `.git`), sorted, as (path relative to `dir` with `/`,
/// path on disk).
fn files_below(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("cannot read a directory below {}: {}", base.display(), e.kind()))
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let Ok(kind) = entry.file_type() else { continue };
            if entry.file_name() == ".git" {
                continue;
            }
            if kind.is_dir() {
                walk(base, &entry.path(), out);
            } else if kind.is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, entry.path()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// What a run over one checkout found.
#[derive(Debug, Default)]
struct Report {
    files: usize,
    settings: usize,
    yaml_files: usize,
    /// Structured files the engine refused (too big, not valid): not a disagreement, but listed.
    refused: Vec<String>,
    /// Places where tree-sitter and the parser disagree, or tree-sitter could not read a file. Names only.
    disagreements: Vec<String>,
}

/// Indexes every structured file below `root` and records where tree-sitter and the parser do not agree.
fn check_locations(root: &Path) -> Report {
    let limits = Limits::default();
    let mut report = Report::default();
    for (rel, path) in files_below(root) {
        let Some(syntax) = syntax_of(&rel) else { continue };
        let Ok(bytes) = fs::read(&path) else {
            report.refused.push(format!("{rel}: unreadable"));
            continue;
        };
        let index = match index_settings(&bytes, syntax, &limits) {
            Ok(index) => index,
            Err(error) => {
                // The error says what was wrong and where, never the text.
                report.refused.push(format!("{rel}: {error}"));
                continue;
            }
        };
        report.files += 1;
        report.settings += index.rows.len();
        if syntax == Syntax::Yaml {
            report.yaml_files += 1;
            if index.located_by != LocationSource::TreeSitter {
                report
                    .disagreements
                    .push(format!("{rel}: tree-sitter could not read the file"));
            }
            for warning in &index.warnings {
                if let ParseWarning::LocationMismatch { path: Some(setting) } = warning {
                    report.disagreements.push(format!("{rel}: {setting}"));
                }
            }
        }
    }
    report
}

fn summary(label: &str, report: &Report) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "real-snapshot: {label}: {} files indexed ({} YAML), {} settings, {} refused, {} disagreements",
        report.files,
        report.yaml_files,
        report.settings,
        report.refused.len(),
        report.disagreements.len()
    )
    .unwrap();
    for line in &report.refused {
        writeln!(out, "real-snapshot: {label}: refused {line}").unwrap();
    }
    for line in &report.disagreements {
        writeln!(out, "real-snapshot: {label}: DISAGREES {line}").unwrap();
    }
    out
}

/// Acceptance 4, first part (T3): on every real file, the tree-sitter location agrees with the parser's.
#[test]
#[ignore = "needs LK_REAL_BASE_DIR and LK_REAL_TENANT_DIR: just engine-real-snapshot"]
fn locations_agree_on_every_real_file() {
    let (base, tenant) = real_dirs();
    let mut failed = false;
    for (label, dir) in [("base", base), ("tenant", tenant)] {
        let report = check_locations(&dir);
        print!("{}", summary(label, &report));
        assert!(
            report.files > 0,
            "real-snapshot: {label}: no structured file found below the directory"
        );
        failed |= !report.disagreements.is_empty();
    }
    assert!(
        !failed,
        "real-snapshot: tree-sitter and the parser disagree (see above)"
    );
    println!("real-snapshot: locations: PASS");
}

// --- T6: the checks of sit1 -----------------------------------------------------------------------------------------------

/// What one expectation found. `ok` is only true when the thing was found; a missing file or a missing history is a failure
/// that says so, never a pass.
struct Expectation {
    name: &'static str,
    ok: bool,
    detail: String,
}

impl Expectation {
    fn print(&self) {
        println!(
            "real-snapshot: {}: {} ({})",
            self.name,
            if self.ok { "PASS" } else { "FAIL" },
            self.detail
        );
    }
}

/// The folder that holds the files: `<dir>/<sub>` if it exists, otherwise the checkout itself is taken to be that folder.
fn content_root(dir: &Path, sub: &str) -> PathBuf {
    let inner = dir.join(sub);
    if inner.is_dir() { inner } else { dir.to_path_buf() }
}

/// The folder of the base files: `config/` of the base checkout, or the checkout itself.
fn base_root(base_dir: &Path) -> PathBuf {
    content_root(base_dir, "config")
}

/// The folder of the tenant files: `data/config/` of the tenant checkout, or the checkout itself.
fn tenant_root(tenant_dir: &Path) -> PathBuf {
    let inner = tenant_dir.join("data/config");
    if inner.is_dir() {
        inner
    } else {
        tenant_dir.to_path_buf()
    }
}

/// A file of the sit1 view, with everything the checks borrow.
struct ViewFile {
    path: NfsPath,
    facts: FileFacts,
    class: FileClass,
    hash: ContentHash,
    flat: Option<Flat>,
    warnings: Vec<ParseWarning>,
    dropped: usize,
}

impl ViewFile {
    fn check_file(&self) -> CheckFile<'_> {
        CheckFile {
            path: &self.path,
            facts: &self.facts,
            class: self.class,
            hash: self.hash,
            merge_mode: MergeMode::WholeFile,
            structured: self.flat.as_ref().map(|flat| Structured {
                settings: &flat.settings,
                parse_warnings: &self.warnings,
                parse_warnings_dropped: self.dropped,
            }),
            root_settings: None,
            drift: None,
            overwrite: None,
            base_history: None,
        }
    }
}

fn sit1() -> TenantId {
    TenantId::parse("sit1").unwrap()
}

fn read_file(path: &NfsPath, class: FileClass, data: &[u8]) -> (Option<Flat>, Vec<ParseWarning>, usize) {
    let syntax = Syntax::of_path(path);
    let (FileClass::Structured, Some(syntax)) = (class, syntax) else {
        return (None, Vec::new(), 0);
    };
    let Ok(text) = std::str::from_utf8(data) else {
        return (None, Vec::new(), 0);
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let limits = Limits::default();
    match parse(text, syntax, &limits).and_then(|p| flatten(&p, &limits).map(|f| (f, p))) {
        Ok((flat, parsed)) => (Some(flat), parsed.warnings, parsed.warnings_dropped),
        Err(_) => (None, Vec::new(), 0),
    }
}

/// The NFS view of swimlane `sit1`, built the way the hub would: every file of the base checkout at its forward-mapped path,
/// every file of the tenant checkout at its forward-mapped path (the suffix `-sit1` before the extension).
fn build_sit1_view(base_dir: &Path, tenant_dir: &Path) -> Vec<ViewFile> {
    let rule = PathMappingRule::default();
    let tenants = TenantSet::new([sit1()]).unwrap();
    let deny = DenyMatcher::with_globs([]).unwrap();
    let mut out = Vec::new();
    let sources = [
        (base_root(base_dir), "config", RepoSource::Base),
        (tenant_root(tenant_dir), "data/config", RepoSource::Tenant(sit1())),
    ];
    for (root, repo_root, source) in sources {
        for (rel, disk) in files_below(&root) {
            let Ok(repo_path) = RepoPath::parse(&format!("{repo_root}/{rel}")) else {
                continue;
            };
            let Ok(path) = map_path(&rule, &source, &repo_path) else {
                continue;
            };
            let Ok(data) = fs::read(&disk) else { continue };
            let Ok(facts) = classify_path(&rule, &path, &tenants, |_| true) else {
                continue;
            };
            let class = classify(&path, Some(&data), &deny);
            let hash = normalised_hash(&Bytes::from(data.clone()), class);
            let (flat, warnings, dropped) = read_file(&path, class, &data);
            out.push(ViewFile {
                path,
                facts,
                class,
                hash,
                flat,
                warnings,
                dropped,
            });
        }
    }
    out
}

fn run_checks(view: &[ViewFile]) -> Vec<CheckResult> {
    let files: Vec<CheckFile<'_>> = view.iter().map(ViewFile::check_file).collect();
    let tenants = TenantSet::new([sit1()]).unwrap();
    checks_c1_to_c8(&CheckInput {
        tenants: &tenants,
        tier: None,
        host_rules: &HostRules::default(),
        files: &files,
        limits: &Limits::default(),
    })
}

/// Expectation (T6): C2 for `account-sorting-config.yml`, `miniStatementConfig.yml` and `receipt-ci_1.bmp` of sit1.
fn sit1_c2(base_dir: &Path, tenant_dir: &Path) -> Expectation {
    const WANTED: [&str; 3] = [
        "tx-infinity-api/account-sorting-config.yml",
        "tx-infinity-api/miniStatementConfig.yml",
        "document-service/resources/receipt-ci_1.bmp",
    ];
    let view = build_sit1_view(base_dir, tenant_dir);
    let results = run_checks(&view);
    let flagged: Vec<String> = results
        .iter()
        .filter(|r| r.kind == FindingKind::RedundantTenantFile)
        .filter_map(|r| {
            let path = r.path.as_ref()?;
            view.iter()
                .find(|f| &f.path == path)
                .map(|f| f.facts.logical.to_string())
        })
        .collect();
    let missing: Vec<&str> = WANTED
        .iter()
        .copied()
        .filter(|w| !flagged.iter().any(|f| f == w))
        .collect();
    Expectation {
        name: "sit1_c2_account_sorting_mini_statement_receipt",
        ok: missing.is_empty(),
        detail: format!(
            "{} files in the view, {} redundant copies found; missing: {missing:?}",
            view.len(),
            flagged.len()
        ),
    }
}

fn git_out(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git ({})", e.kind()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {} failed (is {} a clone with history?)",
            args.first().unwrap_or(&""),
            dir.display()
        ))
    }
}

/// The history C1 needs for `rel`: the time of the tenant file's last commit and the versions of the base file, oldest first.
fn history_of(
    base_root: &Path,
    tenant_root: &Path,
    rel: &str,
) -> Result<(Timestamp, Vec<BaseVersion>), String> {
    let spec = format!("./{rel}");
    let tenant_log = git_out(tenant_root, &["log", "-1", "--format=%ct", "--", &spec])?;
    let tenant_time: i64 = String::from_utf8_lossy(&tenant_log)
        .trim()
        .parse()
        .map_err(|_| "the tenant file has no commit in the tenant checkout".to_owned())?;
    let log = git_out(base_root, &["log", "--reverse", "--format=%H %ct", "--", &spec])?;
    let mut versions = Vec::new();
    for line in String::from_utf8_lossy(&log).lines() {
        let (sha, time) = line.split_once(' ').ok_or("unreadable git log")?;
        let time: i64 = time.parse().map_err(|_| "unreadable commit time")?;
        let commit = CommitId::parse(sha).map_err(|_| "unreadable commit id")?;
        // A commit that deleted the file has no content.
        let content = git_out(base_root, &["show", &format!("{sha}:./{rel}")])
            .ok()
            .map(Bytes::from);
        versions.push(BaseVersion {
            commit,
            time: Timestamp::from_unix_millis(time * 1000),
            content,
        });
    }
    Ok((Timestamp::from_unix_millis(tenant_time * 1000), versions))
}

/// Expectation (T6): C1 for the `atm` entry that sit1's `entryGroupConfig.yml` lacks.
fn sit1_c1_entry_group_config(base_dir: &Path, tenant_dir: &Path) -> Expectation {
    const NAME: &str = "sit1_c1_entry_group_config_missing_atm";
    const REL: &str = "tx-infinity-api/entryGroupConfig.yml";
    let fail = |detail: String| Expectation {
        name: NAME,
        ok: false,
        detail,
    };

    let view = build_sit1_view(base_dir, tenant_dir);
    let Some(copy) = view
        .iter()
        .find(|f| f.facts.logical.as_str() == REL && f.facts.tenant.is_some())
    else {
        return fail(format!("the sit1 view has no tenant copy of {REL}"));
    };
    let (tenant_time, versions) = match history_of(&base_root(base_dir), &tenant_root(tenant_dir), REL) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    let at = versions.iter().rposition(|v| v.time <= tenant_time);
    let since: Vec<BaseVersion> = versions
        .iter()
        .filter(|v| v.time > tenant_time)
        .cloned()
        .collect();
    let history = BaseHistory {
        tenant_commit_time: tenant_time,
        at_tenant_commit: at.map(|i| &versions[i]),
        since: &since,
    };
    let mut file = copy.check_file();
    file.base_history = Some(history);
    let tenants = TenantSet::new([sit1()]).unwrap();
    let Some(result) = c1_missed_base_changes(&file, None, &tenants, &Limits::default()) else {
        return fail(format!(
            "C1 is silent: {} base commits after the copy's last commit (matched by commit time)",
            since.len()
        ));
    };
    let CheckDetail::MissedBaseChanges {
        commits_total,
        changes,
        compared,
        ..
    } = &result.detail
    else {
        return fail("C1 gave a result of another shape".to_owned());
    };
    let atm: Vec<String> = changes
        .iter()
        .filter(|c| c.path.as_str().split(['.', '[']).any(|part| part == "atm"))
        .map(|c| c.path.to_string())
        .collect();
    Expectation {
        name: NAME,
        ok: *compared && !atm.is_empty(),
        detail: format!(
            "{commits_total} base commits since the copy, {} settings changed, compared: {compared}; atm settings: {atm:?}",
            changes.len()
        ),
    }
}

/// Expectation (T6): C8 for the three keys of the base file `security/security-roles.yml`.
fn base_c8_security_roles(base_dir: &Path) -> Expectation {
    const NAME: &str = "base_c8_security_roles_three_keys";
    const REL: &str = "security/security-roles.yml";
    let disk = base_root(base_dir).join(REL);
    let Ok(data) = fs::read(&disk) else {
        return Expectation {
            name: NAME,
            ok: false,
            detail: format!("no {REL} in the base checkout"),
        };
    };
    let path = NfsPath::parse(REL).unwrap();
    let class = FileClass::Structured;
    let (flat, warnings, dropped) = read_file(&path, class, &data);
    let facts = classify_path(
        &PathMappingRule::default(),
        &path,
        &TenantSet::new([sit1()]).unwrap(),
        |_| true,
    )
    .unwrap();
    let file = ViewFile {
        hash: normalised_hash(&Bytes::from(data), class),
        path,
        facts,
        class,
        flat,
        warnings,
        dropped,
    };
    let keys: Vec<String> = c8_duplicate_keys(&file.check_file(), &Limits::default())
        .and_then(|r| match r.detail {
            CheckDetail::DuplicateKeys { keys, .. } => Some(keys),
            _ => None,
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.path.as_ref().map(ToString::to_string))
        .collect();
    let wanted = [
        "txRemoteJuniorTeller",
        "txRemoteSeniorTeller",
        "txRemoteSupervisor",
    ];
    let missing: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|w| !keys.iter().any(|k| k.rsplit('.').next() == Some(*w)))
        .collect();
    Expectation {
        name: NAME,
        ok: missing.is_empty() && keys.len() == 3,
        detail: format!("{} duplicated keys found; missing: {missing:?}", keys.len()),
    }
}

/// Acceptance 4, second part (T6): C2 for the three files of sit1.
#[test]
#[ignore = "needs LK_REAL_BASE_DIR and LK_REAL_TENANT_DIR: just engine-real-snapshot"]
fn sit1_c2_account_sorting_mini_statement_receipt() {
    let (base, tenant) = real_dirs();
    let e = sit1_c2(&base, &tenant);
    e.print();
    assert!(e.ok, "real-snapshot: {}: {}", e.name, e.detail);
}

/// Acceptance 4, second part (T6): C1 for the `atm` entry that sit1's `entryGroupConfig.yml` lacks.
#[test]
#[ignore = "needs LK_REAL_BASE_DIR and LK_REAL_TENANT_DIR: just engine-real-snapshot"]
fn sit1_c1_entry_group_config_missing_atm() {
    let (base, tenant) = real_dirs();
    let e = sit1_c1_entry_group_config(&base, &tenant);
    e.print();
    assert!(e.ok, "real-snapshot: {}: {}", e.name, e.detail);
}

/// Acceptance 4, second part (T6): C8 for the three keys of `security-roles.yml`.
#[test]
#[ignore = "needs LK_REAL_BASE_DIR and LK_REAL_TENANT_DIR: just engine-real-snapshot"]
fn base_c8_security_roles_three_keys() {
    let (base, _) = real_dirs();
    let e = base_c8_security_roles(&base);
    e.print();
    assert!(e.ok, "real-snapshot: {}: {}", e.name, e.detail);
}

// --- the run itself is tested on made-up files, so that it cannot pass by doing nothing -------------------------------

mod harness {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    // --- T6: the sit1 expectations, on repositories made up here ------------------------------------------------------

    /// Runs git in `dir` with a fixed identity and no configuration of the machine.
    fn git(dir: &Path, args: &[&str], when: i64) {
        let date = format!("@{when} +0000");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// A repository with one commit per entry of `commits`: the files to write, and the commit time.
    type Commit<'a> = (i64, Vec<(&'a str, &'a [u8])>);

    fn repo(dir: &Path, commits: &[Commit<'_>]) {
        fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"], 0);
        for (when, files) in commits {
            for (name, data) in files {
                write_bytes(dir, name, data);
            }
            git(dir, &["add", "-A"], *when);
            git(dir, &["commit", "-q", "-m", "c"], *when);
        }
    }

    fn write_bytes(dir: &Path, name: &str, data: &[u8]) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    const GROUPS_OLD: &str = "entryGroupConfigMap:\r\n  branch:\r\n    groups: [a]\r\n";
    const GROUPS_NEW: &str =
        "entryGroupConfigMap:\r\n  branch:\r\n    groups: [a]\r\n  atm:\r\n    groups: [b]\r\n";
    const ROLES: &str = "roles:\n  txRemoteJuniorTeller: 1\n  txRemoteSeniorTeller: 2\n  txRemoteSupervisor: 3\n  txRemoteJuniorTeller: 4\n  txRemoteSeniorTeller: 5\n  txRemoteSupervisor: 6\n";

    /// Two clones that have what the real ones have: three redundant copies, a stale copy and a file with three repeated keys.
    fn made_up_clones(root: &Path, stale: bool, redundant: bool, duplicates: bool) -> (PathBuf, PathBuf) {
        let (base, tenant) = (root.join("base"), root.join("tenant"));
        let sorting: &[u8] = b"a: 1\nb: 2\n";
        let mini: &[u8] = b"x: 1\n";
        let bmp: &[u8] = b"BM\x00\x01\x02";
        let roles: &[u8] = if duplicates {
            ROLES.as_bytes()
        } else {
            b"roles:\n  a: 1\n"
        };
        repo(
            &base,
            &[
                (
                    1_000,
                    vec![
                        (
                            "config/tx-infinity-api/entryGroupConfig.yml",
                            GROUPS_OLD.as_bytes(),
                        ),
                        ("config/tx-infinity-api/account-sorting-config.yml", sorting),
                        ("config/tx-infinity-api/miniStatementConfig.yml", mini),
                        ("config/document-service/resources/receipt-ci_1.bmp", bmp),
                        ("config/security/security-roles.yml", roles),
                    ],
                ),
                (
                    3_000,
                    vec![(
                        "config/tx-infinity-api/entryGroupConfig.yml",
                        GROUPS_NEW.as_bytes(),
                    )],
                ),
            ],
        );
        let other_sorting: &[u8] = b"a: 9\n";
        let copy = if stale { GROUPS_OLD } else { GROUPS_NEW };
        repo(
            &tenant,
            &[(
                // A current copy was made after the base file's last commit; a stale one before it.
                if stale { 2_000 } else { 4_000 },
                vec![
                    (
                        "data/config/tx-infinity-api/entryGroupConfig.yml",
                        copy.as_bytes(),
                    ),
                    (
                        "data/config/tx-infinity-api/account-sorting-config.yml",
                        if redundant { b"b: 2\na: 1\n" } else { other_sorting },
                    ),
                    (
                        "data/config/tx-infinity-api/miniStatementConfig.yml",
                        if redundant { mini } else { b"x: 2\n" },
                    ),
                    (
                        "data/config/document-service/resources/receipt-ci_1.bmp",
                        if redundant { bmp } else { b"BM\x09" },
                    ),
                ],
            )],
        );
        (base, tenant)
    }

    #[test]
    fn the_sit1_expectations_pass_on_clones_that_have_the_facts() {
        let dir = tempfile::tempdir().unwrap();
        let (base, tenant) = made_up_clones(dir.path(), true, true, true);
        for e in [
            sit1_c2(&base, &tenant),
            sit1_c1_entry_group_config(&base, &tenant),
            base_c8_security_roles(&base),
        ] {
            e.print();
            assert!(e.ok, "{}: {}", e.name, e.detail);
        }
    }

    #[test]
    fn the_sit1_expectations_work_when_the_dirs_are_the_folders_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let (base, tenant) = made_up_clones(dir.path(), true, true, true);
        // `git` is run in the folder, so the history is still there.
        let (base, tenant) = (base.join("config"), tenant.join("data/config"));
        assert!(sit1_c2(&base, &tenant).ok);
        assert!(sit1_c1_entry_group_config(&base, &tenant).ok);
        assert!(base_c8_security_roles(&base).ok);
    }

    #[test]
    fn the_sit1_expectations_fail_when_the_facts_are_absent() {
        let dir = tempfile::tempdir().unwrap();
        // The copy is current, the copies differ from base, and no key is repeated.
        let (base, tenant) = made_up_clones(dir.path(), false, false, false);
        for e in [
            sit1_c2(&base, &tenant),
            sit1_c1_entry_group_config(&base, &tenant),
            base_c8_security_roles(&base),
        ] {
            assert!(
                !e.ok,
                "{} passed on a snapshot that does not have the fact: {}",
                e.name, e.detail
            );
        }
    }

    #[test]
    fn the_sit1_expectations_fail_without_history_and_without_the_files() {
        let dir = tempfile::tempdir().unwrap();
        // Plain folders, no repository: C1 cannot be shown, and says why.
        write_bytes(
            dir.path(),
            "base/config/tx-infinity-api/entryGroupConfig.yml",
            GROUPS_NEW.as_bytes(),
        );
        write_bytes(
            dir.path(),
            "tenant/data/config/tx-infinity-api/entryGroupConfig.yml",
            GROUPS_OLD.as_bytes(),
        );
        let e = sit1_c1_entry_group_config(&dir.path().join("base"), &dir.path().join("tenant"));
        assert!(!e.ok && e.detail.contains("history"), "{}", e.detail);
        let empty = tempfile::tempdir().unwrap();
        assert!(!sit1_c2(empty.path(), empty.path()).ok);
        assert!(!base_c8_security_roles(empty.path()).ok);
        assert!(!sit1_c1_entry_group_config(empty.path(), empty.path()).ok);
    }

    #[test]
    fn the_expectations_print_no_value() {
        let dir = tempfile::tempdir().unwrap();
        let (base, tenant) = made_up_clones(dir.path(), true, true, true);
        write_bytes(
            &base,
            "config/tx-infinity-api/secret-config.yml",
            b"password: S3CR3T-VALUE\npassword: S3CR3T-TWO\n",
        );
        let all = [
            sit1_c2(&base, &tenant),
            sit1_c1_entry_group_config(&base, &tenant),
            base_c8_security_roles(&base),
        ];
        for e in all {
            assert!(!e.detail.contains("S3CR3T"), "{}", e.detail);
        }
    }

    #[test]
    fn a_checkout_where_the_readers_agree_is_reported_clean() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "svc/app.yml", "a: 1\nb:\n  c: |\n    x\n  d: 2\n");
        write(dir.path(), "svc/app-sit1.yaml", "a: 2\r\n");
        write(dir.path(), "svc/app.properties", "k=v\n");
        write(dir.path(), "svc/data.json", "{\"a\": 1}");
        write(dir.path(), "svc/logo.bmp", "BM not text");
        write(dir.path(), ".git/config.yml", "ignored: [");
        let report = check_locations(dir.path());
        assert_eq!(
            (report.files, report.yaml_files, report.settings),
            (4, 2, 6),
            "{report:?}"
        );
        assert!(
            report.disagreements.is_empty() && report.refused.is_empty(),
            "{report:?}"
        );
    }

    #[test]
    fn a_disagreement_and_a_refusal_are_named_and_never_show_a_value() {
        let dir = tempfile::tempdir().unwrap();
        // An explicit block scalar key: tree-sitter does not decode it, so it does not find the setting.
        write(
            dir.path(),
            "svc/odd.yml",
            "? |\n  key\n: S3CR3T-VALUE-ONE\nok: 1\n",
        );
        write(dir.path(), "svc/broken.yml", "a: [S3CR3T-VALUE-TWO\n");
        let report = check_locations(dir.path());
        assert_eq!(report.disagreements.len(), 1, "{report:?}");
        assert!(report.disagreements[0].starts_with("svc/odd.yml: "));
        assert_eq!(report.refused.len(), 1, "{report:?}");
        assert!(report.refused[0].starts_with("svc/broken.yml: "));
        let text = summary("base", &report);
        assert!(!text.contains("S3CR3T"), "a value reached the report: {text}");
        assert!(text.contains("DISAGREES svc/odd.yml"), "{text}");
        assert!(text.contains("1 disagreements"), "{text}");
    }
}
