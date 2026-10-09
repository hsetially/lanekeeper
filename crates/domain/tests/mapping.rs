//! D13/D82-D84: Git path <-> NFS path mapping, including the tenant-suffix rename.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{NfsPath, PathMappingRule, RepoPath, ReverseMapping, TenantId, TenantSet};
use proptest::prelude::*;

fn tenant(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

fn tenants(names: &[&str]) -> TenantSet {
    TenantSet::new(names.iter().map(|n| tenant(n))).unwrap()
}

fn repo(s: &str) -> RepoPath {
    RepoPath::parse(s).unwrap()
}

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

#[test]
fn forward_matches_example() {
    let rule = PathMappingRule::default();
    let got = rule
        .tenant_to_nfs(
            &repo("data/config/tx-infinity-api/tx-infinity-core.yml"),
            &tenant("sit1"),
        )
        .unwrap();
    assert_eq!(got.as_str(), "tx-infinity-api/tx-infinity-core-sit1.yml");
}

#[test]
fn base_forward_has_no_rename() {
    let rule = PathMappingRule::default();
    let got = rule
        .base_to_nfs(&repo("config/tx-infinity-api/tx-infinity-core.yml"))
        .unwrap();
    assert_eq!(got.as_str(), "tx-infinity-api/tx-infinity-core.yml");
}

#[test]
fn forward_rejects_paths_outside_the_configured_root() {
    let rule = PathMappingRule::default();
    assert!(rule.base_to_nfs(&repo("data/config/a.yml")).is_err());
    assert!(rule.base_to_nfs(&repo("doc/readme.md")).is_err());
    assert!(
        rule.tenant_to_nfs(&repo("config/a.yml"), &tenant("sit1"))
            .is_err()
    );
    assert!(
        rule.tenant_to_nfs(&repo("data/other/a.yml"), &tenant("sit1"))
            .is_err()
    );
    // the root itself is not a file
    assert!(rule.base_to_nfs(&repo("config")).is_err());
}

#[test]
fn extensionless_files() {
    let rule = PathMappingRule::default();
    let got = rule
        .tenant_to_nfs(&repo("data/config/svc/Dockerfile"), &tenant("sit1"))
        .unwrap();
    assert_eq!(got.as_str(), "svc/Dockerfile-sit1");
    let set = tenants(&["sit1"]);
    match rule.reverse(&got, &set).unwrap() {
        ReverseMapping::Tenant {
            tenant: t,
            repo_path,
            logical,
        } => {
            assert_eq!(t, tenant("sit1"));
            assert_eq!(repo_path.as_str(), "data/config/svc/Dockerfile");
            assert_eq!(logical.as_str(), "svc/Dockerfile");
        }
        other @ ReverseMapping::Base { .. } => panic!("expected tenant, got {other:?}"),
    }
}

#[test]
fn dotfiles_and_multi_dot_names() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1"]);
    // leading-dot-only names are extension-less
    let dot = rule
        .tenant_to_nfs(&repo("data/config/.env"), &tenant("sit1"))
        .unwrap();
    assert_eq!(dot.as_str(), ".env-sit1");
    // the extension is the last dot segment
    let tar = rule
        .tenant_to_nfs(&repo("data/config/a.tar.gz"), &tenant("sit1"))
        .unwrap();
    assert_eq!(tar.as_str(), "a.tar-sit1.gz");
    for n in [&dot, &tar] {
        let ReverseMapping::Tenant { repo_path, .. } = rule.reverse(n, &set).unwrap() else {
            panic!("expected tenant mapping for {n:?}");
        };
        assert!(repo_path.as_str().starts_with("data/config/"));
    }
}

#[test]
fn hyphenated_names_keep_their_hyphens() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1"]);
    let n = nfs("tx-infinity-api/tx-infinity-core-sit1.yml");
    let ReverseMapping::Tenant {
        tenant: t,
        repo_path,
        logical,
    } = rule.reverse(&n, &set).unwrap()
    else {
        panic!("expected tenant mapping");
    };
    assert_eq!(t, tenant("sit1"));
    assert_eq!(
        repo_path.as_str(),
        "data/config/tx-infinity-api/tx-infinity-core.yml"
    );
    assert_eq!(logical.as_str(), "tx-infinity-api/tx-infinity-core.yml");
}

#[test]
fn only_exact_known_tenant_suffix_is_stripped() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1"]);
    for name in [
        "svc/tx-infinity-core.yml",       // no suffix at all
        "svc/tx-infinity-core-sit2.yml",  // unknown tenant
        "svc/tx-infinity-core-sit10.yml", // prefix of the tenant id is not an exact match
        "svc/tx-infinity-core-xsit1.yml", // tenant id must follow a hyphen
        "svc/tx-infinity-coresit1.yml",   // no hyphen
        "svc/-sit1.yml",                  // nothing left of the suffix
        "svc/sit1.yml",
        "sit1/core.yml", // a directory named like a tenant is not a suffix
    ] {
        let n = nfs(name);
        assert!(
            matches!(rule.reverse(&n, &set).unwrap(), ReverseMapping::Base { .. }),
            "{name} must be a base file"
        );
    }
}

#[test]
fn reverse_of_a_base_file_maps_into_the_base_root() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1"]);
    let ReverseMapping::Base { repo_path, logical } = rule.reverse(&nfs("svc/a.yml"), &set).unwrap() else {
        panic!("expected base mapping");
    };
    assert_eq!(repo_path.as_str(), "config/svc/a.yml");
    assert_eq!(logical.as_str(), "svc/a.yml");
}

#[test]
fn longest_known_tenant_wins() {
    let rule = PathMappingRule::default();
    let set = tenants(&["sit1", "t-sit1"]);
    let ReverseMapping::Tenant {
        tenant: t, logical, ..
    } = rule.reverse(&nfs("svc/core-t-sit1.yml"), &set).unwrap()
    else {
        panic!("expected tenant mapping");
    };
    assert_eq!(t, tenant("t-sit1"));
    assert_eq!(logical.as_str(), "svc/core.yml");
}

#[test]
fn tenant_set_is_bounded_and_deduplicated() {
    let one = TenantSet::new([tenant("sit1"), tenant("sit1")]).unwrap();
    assert_eq!(one.len(), 1);
    assert!(one.contains(&tenant("sit1")));
    assert!(!one.contains(&tenant("sit2")));
    let many = (0..=TenantSet::MAX_TENANTS).map(|i| tenant(&format!("sit{i}")));
    assert!(TenantSet::new(many).is_err());
}

proptest! {
    #[test]
    fn reverse_roundtrip_prop(
        dirs in proptest::collection::vec("[a-z][a-z0-9-]{0,8}", 0..4),
        stem in "[a-z][a-z0-9_-]{0,12}",
        ext in proptest::option::of("[a-z]{1,4}"),
        tenant_name in "sit[0-9]{1,2}",
    ) {
        let rule = PathMappingRule::default();
        let t = tenant(&tenant_name);
        let set = tenants(&[tenant_name.as_str()]);
        let mut rel = dirs.join("/");
        if !rel.is_empty() { rel.push('/'); }
        rel.push_str(&stem);
        if let Some(e) = &ext { rel.push('.'); rel.push_str(e); }

        let repo_path = repo(&format!("data/config/{rel}"));
        let n = rule.tenant_to_nfs(&repo_path, &t).unwrap();
        match rule.reverse(&n, &set).unwrap() {
            ReverseMapping::Tenant { tenant: back_t, repo_path: back, logical } => {
                prop_assert_eq!(back_t, t);
                prop_assert_eq!(back, repo_path);
                prop_assert_eq!(logical.as_str(), rel.as_str());
            }
            ReverseMapping::Base { .. } => prop_assert!(false, "tenant file reversed to base"),
        }
    }

    #[test]
    fn base_roundtrip_prop(
        dirs in proptest::collection::vec("[a-z][a-z0-9-]{0,8}", 0..4),
        stem in "[a-z][a-z0-9_]{0,12}",
        ext in proptest::option::of("[a-z]{1,4}"),
    ) {
        // the stem has no hyphen, so it can never look like a tenant file
        let rule = PathMappingRule::default();
        let set = tenants(&["sit1"]);
        let mut rel = dirs.join("/");
        if !rel.is_empty() { rel.push('/'); }
        rel.push_str(&stem);
        if let Some(e) = &ext { rel.push('.'); rel.push_str(e); }
        let repo_path = repo(&format!("config/{rel}"));
        let n = rule.base_to_nfs(&repo_path).unwrap();
        match rule.reverse(&n, &set).unwrap() {
            ReverseMapping::Base { repo_path: back, .. } => prop_assert_eq!(back, repo_path),
            ReverseMapping::Tenant { .. } => prop_assert!(false, "base file reversed to tenant"),
        }
    }

    #[test]
    fn reverse_never_panics_and_forward_inverts_it(raw in "[a-z0-9./_-]{1,60}") {
        let rule = PathMappingRule::default();
        let set = tenants(&["sit1", "t-sit1", "sit2"]);
        if let Ok(n) = NfsPath::parse(&raw) {
            match rule.reverse(&n, &set).unwrap() {
                ReverseMapping::Base { repo_path, .. } => {
                    prop_assert_eq!(rule.base_to_nfs(&repo_path).unwrap(), n);
                }
                ReverseMapping::Tenant { tenant: t, repo_path, .. } => {
                    prop_assert_eq!(rule.tenant_to_nfs(&repo_path, &t).unwrap(), n);
                }
            }
        }
    }
}
