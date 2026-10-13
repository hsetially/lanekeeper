//! T6 (acceptance 4, always-on mirror of the real-snapshot run): checks C1 to C8 on the small synthetic fixtures, against the
//! generator's manifest as the oracle.
//!
//! The fixtures reproduce the shapes of the real examples (a stale copy that lacks `atm`, three redundant copies, three
//! duplicated keys), so what the opt-in run expects of the real repositories, this expects of these. The swimlanes are read
//! as the hub would: every file on NFS is classified against all the deployable branches, with Git (the fixtures' bare
//! repositories) saying which files are tracked.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use bytes::Bytes;
use domain::{
    ChangeKind, CommitId, ContentHash, FileClass, FileKind, FindingKind, MergeMode, NfsPath, PathMappingRule,
    ReverseMapping, TenantId, TenantSet, Timestamp,
};
use engine::{
    BaseHistory, BaseVersion, CheckDetail, CheckFile, CheckInput, CheckResult, DenyMatcher, FileFacts, Flat,
    HostRules, Limits, OrphanReason, ParseWarning, Structured, Syntax, checks_c1_to_c8, classify,
    classify_path, flatten, normalised_hash, parse,
};
use support::{Fixtures, small_fixtures};

/// A file of a swimlane, with everything the checks borrow.
struct LaneFile {
    path: NfsPath,
    facts: FileFacts,
    class: FileClass,
    hash: ContentHash,
    flat: Option<Flat>,
    warnings: Vec<ParseWarning>,
    dropped: usize,
}

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    out.stdout
}

fn tracked_in(repo: &Path, rev: &str) -> BTreeSet<String> {
    String::from_utf8(git(repo, &["ls-tree", "-r", "--name-only", rev]))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn tenant_set(names: &[String]) -> TenantSet {
    TenantSet::new(names.iter().map(|n| TenantId::parse(n).unwrap())).unwrap()
}

/// Every file of the swimlane `id`, classified.
fn load_lane(fx: &Fixtures, id: &str) -> Vec<LaneFile> {
    let rule = PathMappingRule::default();
    let all = tenant_set(&fx.manifest.tenants);
    let base_repo = fx.root.join(&fx.manifest.repos.base.path);
    let tenant_repo = fx.root.join(&fx.manifest.repos.tenant.path);
    let base_tracked = tracked_in(&base_repo, "master");
    let tenant_tracked: BTreeMap<String, BTreeSet<String>> = fx
        .manifest
        .tenants
        .iter()
        .map(|t| (t.clone(), tracked_in(&tenant_repo, &format!("refs/heads/{t}"))))
        .collect();
    let deny = DenyMatcher::with_globs([]).unwrap();

    let root = fx.root.join("nfs").join(id);
    let mut out = Vec::new();
    for (rel, disk) in support::files_below(&root) {
        let path = NfsPath::parse(&rel).unwrap();
        let data = std::fs::read(&disk).unwrap();
        let facts = classify_path(&rule, &path, &all, |m| match m {
            ReverseMapping::Base { repo_path, .. } => base_tracked.contains(repo_path.as_str()),
            ReverseMapping::Tenant {
                tenant, repo_path, ..
            } => tenant_tracked
                .get(tenant.as_str())
                .is_some_and(|set| set.contains(repo_path.as_str())),
        })
        .unwrap();
        let class = classify(&path, Some(&data), &deny);
        let hash = normalised_hash(&Bytes::from(data.clone()), class);
        let (flat, warnings, dropped) = match (class, Syntax::of_path(&path)) {
            (FileClass::Structured, Some(syntax)) => {
                let text = std::str::from_utf8(&data).unwrap();
                let parsed = parse(
                    text.strip_prefix('\u{feff}').unwrap_or(text),
                    syntax,
                    &Limits::default(),
                )
                .unwrap_or_else(|e| panic!("{rel}: {e}"));
                let flat = flatten(&parsed, &Limits::default()).unwrap();
                (Some(flat), parsed.warnings, parsed.warnings_dropped)
            }
            _ => (None, Vec::new(), 0),
        };
        out.push(LaneFile {
            path,
            facts,
            class,
            hash,
            flat,
            warnings,
            dropped,
        });
    }
    out
}

fn view<'a>(f: &'a LaneFile, history: Option<BaseHistory<'a>>) -> CheckFile<'a> {
    CheckFile {
        path: &f.path,
        facts: &f.facts,
        class: f.class,
        hash: f.hash,
        merge_mode: MergeMode::WholeFile,
        structured: f.flat.as_ref().map(|flat| Structured {
            settings: &flat.settings,
            parse_warnings: &f.warnings,
            parse_warnings_dropped: f.dropped,
        }),
        root_settings: None,
        drift: None,
        overwrite: None,
        base_history: history,
    }
}

fn run_lane(files: &[CheckFile<'_>], tenants: &TenantSet) -> Vec<CheckResult> {
    checks_c1_to_c8(&CheckInput {
        tenants,
        tier: None,
        host_rules: &HostRules::default(),
        files,
        limits: &Limits::default(),
    })
}

fn lanes(fx: &Fixtures) -> Vec<(String, TenantSet, Vec<LaneFile>)> {
    fx.manifest
        .swimlanes
        .iter()
        .map(|s| (s.id.clone(), tenant_set(&s.tenants), load_lane(fx, &s.id)))
        .collect()
}

#[test]
fn c2_matches_manifest_oracle() {
    let fx = small_fixtures();
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    for (_, tenants, files) in lanes(fx) {
        let views: Vec<CheckFile<'_>> = files.iter().map(|f| view(f, None)).collect();
        let logical: BTreeMap<&NfsPath, &FileFacts> = files.iter().map(|f| (&f.path, &f.facts)).collect();
        for r in run_lane(&views, &tenants) {
            if let CheckDetail::RedundantTenantFile { tenant, .. } = &r.detail {
                let facts = logical[r.path.as_ref().unwrap()];
                found.insert((tenant.to_string(), facts.logical.to_string()));
            }
        }
    }
    let planted: BTreeSet<(String, String)> = fx
        .manifest
        .planted
        .redundant_copies
        .iter()
        .map(|c| (c.tenant.clone(), c.path.clone()))
        .collect();
    assert!(
        planted.len() >= 3,
        "the fixtures plant redundant copies: {planted:?}"
    );
    // The three files of the real sit1 snapshot are among them.
    for real in [
        "tx-infinity-api/account-sorting-config.yml",
        "tx-infinity-api/miniStatementConfig.yml",
        "document-service/resources/receipt-ci_1.bmp",
    ] {
        assert!(
            found.contains(&("sit1".to_owned(), real.to_owned())),
            "{real} missing from {found:?}"
        );
    }
    assert_eq!(
        found, planted,
        "C2 on the fixtures must find exactly the planted redundant copies"
    );
}

#[test]
fn c8_matches_manifest_oracle() {
    let fx = small_fixtures();
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, tenants, files) in lanes(fx) {
        let views: Vec<CheckFile<'_>> = files.iter().map(|f| view(f, None)).collect();
        let by_path: BTreeMap<&NfsPath, &FileFacts> = files.iter().map(|f| (&f.path, &f.facts)).collect();
        for r in run_lane(&views, &tenants) {
            if let CheckDetail::DuplicateKeys { keys, .. } = &r.detail {
                let logical = by_path[r.path.as_ref().unwrap()].logical.to_string();
                let names = keys.iter().map(|k| {
                    k.path
                        .as_ref()
                        .unwrap()
                        .as_str()
                        .rsplit('.')
                        .next()
                        .unwrap()
                        .to_owned()
                });
                found.entry(logical).or_default().extend(names);
            }
        }
    }
    let planted: BTreeMap<String, BTreeSet<String>> = fx
        .manifest
        .planted
        .duplicate_keys
        .iter()
        .map(|d| (d.path.clone(), d.keys.iter().cloned().collect()))
        .collect();
    assert_eq!(
        planted["security/security-roles.yml"].len(),
        3,
        "the three keys of the real example"
    );
    assert_eq!(found, planted);
}

#[test]
fn c4_matches_manifest_oracle() {
    let fx = small_fixtures();
    let mut not_deployed: BTreeSet<(String, String)> = BTreeSet::new();
    let mut untracked: BTreeSet<(String, String)> = BTreeSet::new();
    for (id, tenants, files) in lanes(fx) {
        let views: Vec<CheckFile<'_>> = files.iter().map(|f| view(f, None)).collect();
        for r in run_lane(&views, &tenants) {
            let CheckDetail::OrphanFile { reason } = &r.detail else {
                continue;
            };
            let entry = (id.clone(), r.path.as_ref().unwrap().to_string());
            match reason {
                OrphanReason::TenantNotDeployed(_) => not_deployed.insert(entry),
                OrphanReason::Untracked => untracked.insert(entry),
            };
        }
    }
    let planted_orphans: BTreeSet<(String, String)> = fx
        .manifest
        .planted
        .orphans
        .iter()
        .map(|o| (o.swimlane.clone(), o.path.clone()))
        .collect();
    let planted_untracked: BTreeSet<(String, String)> = fx
        .manifest
        .planted
        .untracked
        .iter()
        .map(|o| (o.swimlane.clone(), o.path.clone()))
        .collect();
    assert!(!planted_orphans.is_empty() && !planted_untracked.is_empty());
    // An orphan the generator planted is a file for another branch; whether Git has it or not, the suffix is what C4 says.
    assert_eq!(not_deployed, planted_orphans);
    // The untracked files are the planted ones, except those that are also orphans (reported for the suffix).
    let expected_untracked: BTreeSet<_> = planted_untracked.difference(&planted_orphans).cloned().collect();
    assert_eq!(untracked, expected_untracked);
}

// --- C1 -------------------------------------------------------------------------------------------------------------

/// The versions of a base file on `master`, from Git: (commit, time, content), oldest first.
fn base_versions(fx: &Fixtures, path: &str) -> Vec<BaseVersion> {
    let repo = fx.root.join(&fx.manifest.repos.base.path);
    let spec = format!("config/{path}");
    let log = String::from_utf8(git(
        &repo,
        &["log", "--format=%H %ct", "--reverse", "master", "--", &spec],
    ))
    .unwrap();
    log.lines()
        .map(|line| {
            let (sha, time) = line.split_once(' ').unwrap();
            let content = git(&repo, &["show", &format!("{sha}:{spec}")]);
            BaseVersion {
                commit: CommitId::parse(sha).unwrap(),
                time: Timestamp::from_unix_millis(time.parse::<i64>().unwrap() * 1000),
                content: Some(Bytes::from(content)),
            }
        })
        .collect()
}

#[test]
fn c1_matches_manifest_oracle() {
    let fx = small_fixtures();
    let lanes = lanes(fx);
    let mut checked = 0;
    for stale in &fx.manifest.planted.stale_forks {
        // The fixtures date every tenant commit to the same day, so the copy's age is its version: the copy was made just
        // after base version `fork_version`. The base file only has a commit for the versions that changed it.
        let versions = base_versions(fx, &stale.path);
        let repo = fx.root.join(&fx.manifest.repos.base.path);
        let tag = format!("refs/tags/v{}.0.0", stale.fork_version);
        let made = String::from_utf8(git(&repo, &["log", "-1", "--format=%ct", &tag])).unwrap();
        let tenant_time = Timestamp::from_unix_millis(made.trim().parse::<i64>().unwrap() * 1000 + 1000);
        let at = versions.iter().rposition(|v| v.time <= tenant_time).unwrap();
        let before = &versions[at];
        let since = &versions[at + 1..];
        assert!(!since.is_empty(), "{stale:?} is not stale in the base history");

        let (_, tenants, files) = lanes
            .iter()
            .find(|(_, set, files)| {
                set.iter().any(|t| t.as_str() == stale.tenant)
                    && files.iter().any(|f| {
                        f.facts.kind == FileKind::Tenant
                            && f.facts.logical.as_str() == stale.path
                            && f.facts
                                .tenant
                                .as_ref()
                                .is_some_and(|t| t.as_str() == stale.tenant)
                    })
            })
            .unwrap_or_else(|| panic!("no swimlane has the tenant file of {stale:?}"));
        let history = BaseHistory {
            tenant_commit_time: tenant_time,
            at_tenant_commit: Some(before),
            since,
        };
        let views: Vec<CheckFile<'_>> = files
            .iter()
            .map(|f| {
                let is_it = f.facts.logical.as_str() == stale.path
                    && f.facts
                        .tenant
                        .as_ref()
                        .is_some_and(|t| t.as_str() == stale.tenant);
                view(f, is_it.then_some(history))
            })
            .collect();
        let results = run_lane(&views, tenants);
        let c1: Vec<&CheckResult> = results
            .iter()
            .filter(|r| r.kind == FindingKind::MissedBaseChanges)
            .collect();
        assert_eq!(c1.len(), 1, "{stale:?}: {c1:?}");
        let CheckDetail::MissedBaseChanges {
            commits_total,
            changes,
            compared,
            ..
        } = &c1[0].detail
        else {
            panic!()
        };
        assert!(*compared, "{stale:?}");
        assert_eq!(*commits_total, since.len());
        for missing in &stale.missing {
            // The entry may sit below other keys (`entryGroupConfigMap.atm.groups`) and have settings of its own.
            let needle = format!(".{missing}.");
            let hit = changes.iter().any(|c| {
                c.kind == ChangeKind::Added
                    && format!(".{}.", c.path.as_str().replace('[', ".[")).contains(&needle)
            });
            assert!(
                hit,
                "{stale:?}: the missing entry {missing} is not among the changes {changes:?}"
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 3,
        "the fixtures plant stale forks, including the atm entry of the real example"
    );
}
