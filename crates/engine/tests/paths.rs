//! T4 (S11, D13, D82-D84): the path mapping between the two repositories and the NFS tree, and the classification of an NFS
//! path as a base file, a tenant file or an untracked file.
//!
//! The mapping itself is `domain::PathMappingRule`; these tests pin what the engine builds on it: `map_path`,
//! `reverse_map`, `classify_path`, and the interim `MergeRules`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::match_wildcard_for_single_variants
)]

use domain::{
    FileKind, LogicalFile, MapError, MergeMode, NfsPath, PathMappingRule, RepoPath, ReverseMapping, TenantId,
    TenantSet,
};
use engine::{FileFacts, MergeRules, MergeRulesError, RepoSource, classify_path, map_path, reverse_map};
use proptest::prelude::*;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn repo(s: &str) -> RepoPath {
    RepoPath::parse(s).unwrap()
}

fn tenant(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

fn tenants(ids: &[&str]) -> TenantSet {
    TenantSet::new(ids.iter().map(|t| tenant(t))).unwrap()
}

fn tracked(_: &ReverseMapping) -> bool {
    true
}

fn untracked(_: &ReverseMapping) -> bool {
    false
}

// --- map_path and reverse_map ---

#[test]
fn map_path_drops_the_root_and_inserts_the_suffix_before_the_extension() {
    let rule = PathMappingRule::default();
    assert_eq!(
        map_path(
            &rule,
            &RepoSource::Base,
            &repo("config/tx-infinity-api/tx-infinity-core.yml")
        )
        .unwrap(),
        nfs("tx-infinity-api/tx-infinity-core.yml")
    );
    assert_eq!(
        map_path(
            &rule,
            &RepoSource::Tenant(tenant("sit1")),
            &repo("data/config/tx-infinity-api/tx-infinity-core.yml")
        )
        .unwrap(),
        nfs("tx-infinity-api/tx-infinity-core-sit1.yml")
    );
}

#[test]
fn extensionless_suffix_at_end() {
    let rule = PathMappingRule::default();
    let mapped = map_path(
        &rule,
        &RepoSource::Tenant(tenant("sit1")),
        &repo("data/config/scripts/startup"),
    )
    .unwrap();
    assert_eq!(mapped, nfs("scripts/startup-sit1"));
    // A multi-dot name keeps everything before the last dot as its stem.
    let mapped = map_path(
        &rule,
        &RepoSource::Tenant(tenant("sit1")),
        &repo("data/config/a/b.tar.gz"),
    )
    .unwrap();
    assert_eq!(mapped, nfs("a/b.tar-sit1.gz"));
    // And a name that only starts with a dot has no extension.
    let mapped = map_path(
        &rule,
        &RepoSource::Tenant(tenant("sit1")),
        &repo("data/config/a/.env"),
    )
    .unwrap();
    assert_eq!(mapped, nfs("a/.env-sit1"));

    // Back again.
    let set = tenants(&["sit1"]);
    for (path, logical) in [
        ("scripts/startup-sit1", "scripts/startup"),
        ("a/b.tar-sit1.gz", "a/b.tar.gz"),
        ("a/.env-sit1", "a/.env"),
    ] {
        match reverse_map(&rule, &nfs(path), &set).unwrap() {
            ReverseMapping::Tenant {
                tenant: t,
                logical: l,
                ..
            } => {
                assert_eq!(t, tenant("sit1"), "{path}");
                assert_eq!(l, LogicalFile::parse(logical).unwrap(), "{path}");
            }
            other => panic!("{path}: {other:?}"),
        }
    }
}

#[test]
fn hyphenated_names_not_split() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1", "sit2"]);
    // The base file's own hyphens are not a tenant suffix.
    match reverse_map(&rule, &nfs("tx-infinity-api/tx-infinity-core.yml"), &set).unwrap() {
        ReverseMapping::Base { logical, repo_path } => {
            assert_eq!(logical.as_str(), "tx-infinity-api/tx-infinity-core.yml");
            assert_eq!(repo_path.as_str(), "config/tx-infinity-api/tx-infinity-core.yml");
        }
        other => panic!("{other:?}"),
    }
    // Only the exact suffix of a known tenant, immediately before the extension, is stripped.
    match reverse_map(&rule, &nfs("tx-infinity-api/tx-infinity-core-sit1.yml"), &set).unwrap() {
        ReverseMapping::Tenant {
            tenant: t,
            logical,
            repo_path,
        } => {
            assert_eq!(t, tenant("sit1"));
            assert_eq!(logical.as_str(), "tx-infinity-api/tx-infinity-core.yml");
            assert_eq!(
                repo_path.as_str(),
                "data/config/tx-infinity-api/tx-infinity-core.yml"
            );
        }
        other => panic!("{other:?}"),
    }
    // A suffix in the middle of the name, or one that is only part of a longer word, is not a tenant suffix.
    for name in [
        "a/core-sit1-extra.yml",
        "a/coresit1.yml",
        "a/core-xsit1.yml",
        "a/core_sit1.yml",
    ] {
        assert!(
            matches!(
                reverse_map(&rule, &nfs(name), &set).unwrap(),
                ReverseMapping::Base { .. }
            ),
            "{name}"
        );
    }
}

#[test]
fn orphan_suffix_is_untracked_base() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1", "sit2"]);
    // `sit9` is not a tenant of the swimlane: the file is not a tenant file, the name stays whole, and with no Git source
    // it is untracked.
    let facts = classify_path(&rule, &nfs("tx-api/core-sit9.yml"), &set, untracked).unwrap();
    assert_eq!(facts.kind, FileKind::Untracked);
    assert_eq!(facts.logical.as_str(), "tx-api/core-sit9.yml");
    assert_eq!(facts.tenant, None);
    // The same name with a Git source is just a base file whose name ends in a hyphen and a word.
    let facts = classify_path(&rule, &nfs("tx-api/core-sit9.yml"), &set, tracked).unwrap();
    assert_eq!(facts.kind, FileKind::Base);
    assert_eq!(facts.tenant, None);
}

#[test]
fn classify_path_names_the_kind_the_logical_file_and_the_tenant() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1", "sit2"]);

    let base = classify_path(&rule, &nfs("a/b.yml"), &set, tracked).unwrap();
    assert_eq!(
        base,
        FileFacts {
            kind: FileKind::Base,
            logical: LogicalFile::parse("a/b.yml").unwrap(),
            tenant: None,
            repo_path: repo("config/a/b.yml"),
        }
    );

    let tenant_file = classify_path(&rule, &nfs("a/b-sit2.yml"), &set, tracked).unwrap();
    assert_eq!(
        tenant_file,
        FileFacts {
            kind: FileKind::Tenant,
            logical: LogicalFile::parse("a/b.yml").unwrap(),
            tenant: Some(tenant("sit2")),
            repo_path: repo("data/config/a/b.yml"),
        }
    );

    // A tenant file that Git does not have keeps what the name says (so that the orphan can be adopted), and is untracked.
    let loose = classify_path(&rule, &nfs("a/b-sit2.yml"), &set, untracked).unwrap();
    assert_eq!(loose.kind, FileKind::Untracked);
    assert_eq!(loose.logical.as_str(), "a/b.yml");
    assert_eq!(loose.tenant, Some(tenant("sit2")));

    // The closure sees what the name was read as, so a caller asks Git about the right repository and path.
    let seen = std::cell::RefCell::new(Vec::new());
    classify_path(&rule, &nfs("a/b-sit1.yml"), &set, |m| {
        seen.borrow_mut().push(m.clone());
        true
    })
    .unwrap();
    assert!(matches!(&seen.borrow()[..], [ReverseMapping::Tenant { tenant: t, .. }] if *t == tenant("sit1")));
}

#[test]
fn a_known_tenant_suffix_on_the_root_name_alone_is_not_stripped() {
    // `-sit1.yml` has nothing before the hyphen: stripping would leave `.yml`.
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1"]);
    let facts = classify_path(&rule, &nfs("-sit1.yml"), &set, tracked).unwrap();
    assert_eq!(facts.kind, FileKind::Base);
    assert_eq!(facts.logical.as_str(), "-sit1.yml");
}

#[test]
fn the_longest_known_tenant_wins() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1", "a-sit1"]);
    let facts = classify_path(&rule, &nfs("x/core-a-sit1.yml"), &set, tracked).unwrap();
    assert_eq!(facts.tenant, Some(tenant("a-sit1")));
    assert_eq!(facts.logical.as_str(), "x/core.yml");
}

#[test]
fn hostile_paths_never_escape_mapping() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1"]);
    // A path that is not a plain relative path is refused before the mapping sees it.
    for hostile in [
        "../etc/passwd",
        "a/../../b.yml",
        "/etc/passwd",
        "a//b.yml",
        "a/./b.yml",
        "a\\..\\b.yml",
        "C:/windows/x.yml",
        "a/b.yml\0",
        "a/\u{202e}b.yml",
        "",
        "a/b/..",
        "~/x.yml",
        "a/.git/config",
    ] {
        if let Ok(path) = NfsPath::parse(hostile) {
            // Whatever the parser lets through must stay under the roots after the mapping.
            // A name that cannot be a repository path (`.git` components) is refused, which is as safe as it gets.
            let Ok(mapping) = reverse_map(&rule, &path, &set) else {
                continue;
            };
            let (repo_path, root) = match mapping {
                ReverseMapping::Base { repo_path, .. } => (repo_path, "config/"),
                ReverseMapping::Tenant { repo_path, .. } => (repo_path, "data/config/"),
            };
            assert!(repo_path.as_str().starts_with(root), "{hostile}");
            assert!(
                !repo_path
                    .as_str()
                    .split('/')
                    .any(|c| c == ".." || c == "." || c == ".git"),
                "{hostile}"
            );
        }
    }
    // A repository path outside the configured root is refused by the forward mapping.
    for outside in [
        "other/a.yml",
        "configx/a.yml",
        "config",
        "data/other/a.yml",
        "data/config",
    ] {
        if let Ok(p) = RepoPath::parse(outside) {
            assert_eq!(
                map_path(&rule, &RepoSource::Base, &p),
                Err(MapError::OutsideRoot),
                "base {outside}"
            );
            assert!(
                map_path(&rule, &RepoSource::Tenant(tenant("sit1")), &p).is_err(),
                "{outside}"
            );
        }
    }
    // A tenant repository path is not a base path, and the other way around.
    assert!(map_path(&rule, &RepoSource::Base, &repo("data/config/a.yml")).is_err());
    assert!(map_path(&rule, &RepoSource::Tenant(tenant("sit1")), &repo("config/a.yml")).is_err());
}

fn stem() -> impl Strategy<Value = String> {
    // Hyphenated words in the style of the real names (`tx-infinity-core`): letters only, so that no part of a stem is
    // ever a tenant id (which has a digit).
    proptest::collection::vec("[a-z]{1,6}", 1..5).prop_map(|parts| parts.join("-"))
}

fn dir() -> impl Strategy<Value = String> {
    proptest::collection::vec(stem(), 0..3).prop_map(|parts| parts.join("/"))
}

fn ext() -> impl Strategy<Value = Option<&'static str>> {
    prop_oneof![
        Just(None),
        Just(Some("yml")),
        Just(Some("yaml")),
        Just(Some("properties")),
        Just(Some("bmp"))
    ]
}

fn tenant_id() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("sit1"), Just("sit2"), Just("uat3"), Just("sit10")]
}

fn join(dir: &str, stem: &str, ext: Option<&str>) -> String {
    let name = ext.map_or_else(|| stem.to_owned(), |e| format!("{stem}.{e}"));
    if dir.is_empty() {
        name
    } else {
        format!("{dir}/{name}")
    }
}

proptest! {
    /// Mapping a tenant file forward and back gives the same tenant and the same repository path, and back again gives
    /// the same NFS path.
    #[test]
    fn reverse_map_roundtrip_prop(dir in dir(), stem in stem(), ext in ext(), t in tenant_id()) {
        let rule = PathMappingRule::default();
        let set = tenants(&["sit1", "sit2", "uat3", "sit10"]);
        let tenant = tenant(t);
        let repo_path = repo(&format!("data/config/{}", join(&dir, &stem, ext)));
        let mapped = map_path(&rule, &RepoSource::Tenant(tenant.clone()), &repo_path).unwrap();
        match reverse_map(&rule, &mapped, &set).unwrap() {
            ReverseMapping::Tenant { tenant: back, repo_path: back_repo, logical } => {
                prop_assert_eq!(back, tenant);
                prop_assert_eq!(&back_repo, &repo_path);
                prop_assert_eq!(logical.as_str(), join(&dir, &stem, ext));
            }
            other => prop_assert!(false, "{:?}", other),
        }
        // A base file is the identity.
        let base_repo = repo(&format!("config/{}", join(&dir, &stem, ext)));
        let base_nfs = map_path(&rule, &RepoSource::Base, &base_repo).unwrap();
        prop_assert_eq!(base_nfs.as_str(), join(&dir, &stem, ext));
        match reverse_map(&rule, &base_nfs, &set).unwrap() {
            ReverseMapping::Base { repo_path: back, .. } => prop_assert_eq!(back, base_repo),
            other => prop_assert!(false, "{:?}", other),
        }
    }

    /// Whatever the name, reading it back to Git and forward again gives the same NFS path.
    #[test]
    fn forward_after_reverse_is_the_identity_prop(path in "[a-z0-9._-]{1,8}(/[a-z0-9._-]{1,12}){0,3}") {
        let Ok(path) = NfsPath::parse(&path) else { return Ok(()) };
        let rule = PathMappingRule::default();
        let set = tenants(&["sit1", "sit2", "a-sit1"]);
        let back = match reverse_map(&rule, &path, &set).unwrap() {
            ReverseMapping::Base { repo_path, .. } => map_path(&rule, &RepoSource::Base, &repo_path),
            ReverseMapping::Tenant { tenant, repo_path, .. } => map_path(&rule, &RepoSource::Tenant(tenant), &repo_path),
        };
        prop_assert_eq!(back.unwrap(), path);
    }
}

// --- MergeRules (interim, deleted by T10) ---

fn logical(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

#[test]
fn merge_rules_default_to_whole_file_and_the_first_matching_rule_wins() {
    let rules = MergeRules::default();
    assert_eq!(rules.mode_for(&logical("a/b.yml")), MergeMode::WholeFile);

    let rules = MergeRules::new(
        MergeMode::WholeFile,
        &[
            ("**/special/*.yml", MergeMode::WholeFile),
            ("**/*.yml", MergeMode::SpringMerge),
            ("application*.properties", MergeMode::SpringMerge),
        ],
    )
    .unwrap();
    assert_eq!(rules.mode_for(&logical("svc/x.yml")), MergeMode::SpringMerge);
    // The earlier rule wins over the later one that also matches.
    assert_eq!(
        rules.mode_for(&logical("svc/special/x.yml")),
        MergeMode::WholeFile
    );
    // No rule: the default.
    assert_eq!(rules.mode_for(&logical("svc/x.xsl")), MergeMode::WholeFile);
    assert_eq!(
        rules.mode_for(&logical("application.properties")),
        MergeMode::SpringMerge
    );
    // Case matters in file names.
    assert_eq!(rules.mode_for(&logical("svc/X.YML")), MergeMode::WholeFile);
    // A different default.
    let rules = MergeRules::new(MergeMode::SpringMerge, &[("*.xsl", MergeMode::WholeFile)]).unwrap();
    assert_eq!(rules.mode_for(&logical("a.xsl")), MergeMode::WholeFile);
    assert_eq!(rules.mode_for(&logical("a.yml")), MergeMode::SpringMerge);
}

#[test]
fn merge_rules_refuse_too_many_too_long_and_invalid_globs() {
    let many: Vec<(&str, MergeMode)> = (0..=MergeRules::MAX_RULES)
        .map(|_| ("*.yml", MergeMode::WholeFile))
        .collect();
    assert_eq!(
        MergeRules::new(MergeMode::WholeFile, &many).unwrap_err(),
        MergeRulesError::TooManyRules
    );
    let exactly: Vec<(&str, MergeMode)> = (0..MergeRules::MAX_RULES)
        .map(|_| ("*.yml", MergeMode::WholeFile))
        .collect();
    assert!(MergeRules::new(MergeMode::WholeFile, &exactly).is_ok());

    let long = "a".repeat(MergeRules::MAX_GLOB_BYTES + 1);
    assert_eq!(
        MergeRules::new(MergeMode::WholeFile, &[(long.as_str(), MergeMode::WholeFile)]).unwrap_err(),
        MergeRulesError::GlobTooLong
    );
    let at_limit = "a".repeat(MergeRules::MAX_GLOB_BYTES);
    assert!(MergeRules::new(MergeMode::WholeFile, &[(at_limit.as_str(), MergeMode::WholeFile)]).is_ok());

    for bad in ["[", "a{b", "**a/**b/[x"] {
        let err = MergeRules::new(MergeMode::WholeFile, &[(bad, MergeMode::WholeFile)]).unwrap_err();
        assert_eq!(err, MergeRulesError::InvalidGlob, "{bad}");
        // The error never repeats the glob.
        assert!(!format!("{err} {err:?}").contains(bad));
    }
}
