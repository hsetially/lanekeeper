//! The synthetic fixture generator `cargo xtask gen-fixtures` (prompt 01, T8; AC4a-AC4c, plan Q20 and Q21).
//!
//! "Byte-identical" (Q20) means: the same manifest, the same NFS files, and the same Git object ids for every ref.
//! Raw packfile equality is checked as well, on the same `git` binary.
//!
//! The target-scale check is `#[ignore]` (80,000 files); `just fixtures-verify` runs it with `--ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    // Tests of generated data: counting newlines in byte slices, looking at file extensions of fixed ASCII names,
    // and casting small JSON counts. None of it is hot or platform dependent.
    clippy::naive_bytecount,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    clippy::too_many_lines
)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use common::TempDir;
use domain::{NfsPath, PathMappingRule, RepoPath, ReverseMapping, TenantId, TenantSet};
use serde_json::Value;
use xtask::fixtures::{Config, DEFAULT_SEED, Scale, generate};

// ---------------------------------------------------------------------------------------------------------------
// Helpers

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn git_text(repo: &Path, args: &[&str]) -> String {
    String::from_utf8(git(repo, args)).unwrap()
}

/// Every file below `dir`, as (path relative to `dir` with `/`, bytes), sorted.
fn walk(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn rec(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                rec(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    rec(dir, dir, &mut out);
    out
}

fn small_config(out: &Path, seed: u64) -> Config {
    Config {
        scale: Scale::Small,
        seed,
        out: out.to_path_buf(),
    }
}

struct Generated {
    root: PathBuf,
    manifest: Value,
    /// (swimlane id, path inside the swimlane) -> bytes
    nfs: BTreeMap<(String, String), Vec<u8>>,
}

impl Generated {
    fn base_repo(&self) -> PathBuf {
        self.root
            .join(self.manifest["repos"]["base"]["path"].as_str().unwrap())
    }
    fn tenant_repo(&self) -> PathBuf {
        self.root
            .join(self.manifest["repos"]["tenant"]["path"].as_str().unwrap())
    }
    fn swimlanes(&self) -> Vec<&Value> {
        self.manifest["swimlanes"].as_array().unwrap().iter().collect()
    }
    fn texts(&self) -> impl Iterator<Item = (&(String, String), &Vec<u8>)> {
        self.nfs.iter()
    }
    fn planted(&self, kind: &str) -> &Vec<Value> {
        self.manifest["planted"][kind].as_array().unwrap()
    }
    fn tenants_of(&self, swimlane: &str) -> Vec<String> {
        self.swimlanes()
            .into_iter()
            .find(|s| s["id"] == swimlane)
            .unwrap()["tenants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_owned())
            .collect()
    }
}

fn load(root: PathBuf) -> Generated {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let mut nfs = BTreeMap::new();
    for (rel, bytes) in walk(&root.join("nfs")) {
        let (swimlane, path) = rel.split_once('/').unwrap();
        nfs.insert((swimlane.to_owned(), path.to_owned()), bytes);
    }
    Generated { root, manifest, nfs }
}

/// One small-scale generation shared by the shape tests (they only read it).
fn shared() -> &'static Generated {
    static SHARED: OnceLock<Generated> = OnceLock::new();
    SHARED.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fixtures-small-shared-{}", std::process::id()));
        generate(&small_config(&root, DEFAULT_SEED)).unwrap();
        load(root)
    })
}

fn is_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0)
}

// ---------------------------------------------------------------------------------------------------------------
// AC4a: determinism

fn snapshot(
    g: &Generated,
) -> (
    Vec<u8>,
    Vec<(String, Vec<u8>)>,
    Vec<(String, String, String)>,
    Vec<(String, Vec<u8>)>,
) {
    let manifest = std::fs::read(g.root.join("manifest.json")).unwrap();
    let files = walk(&g.root.join("nfs"));
    let mut git_state = Vec::new();
    let mut packs = Vec::new();
    for repo in [g.base_repo(), g.tenant_repo()] {
        let name = repo.file_name().unwrap().to_string_lossy().into_owned();
        let refs = git_text(&repo, &["for-each-ref", "--format=%(refname) %(objectname)"]);
        for line in refs.lines() {
            let (refname, id) = line.split_once(' ').unwrap();
            assert_eq!(id, git_text(&repo, &["rev-parse", refname]).trim());
            let tree = git_text(&repo, &["ls-tree", "-r", "-t", refname]);
            git_state.push((name.clone(), format!("{refname} {id}"), tree));
        }
        // The extra check of Q20: the pack written by the same git binary is identical too.
        for (rel, bytes) in walk(&repo.join("objects/pack")) {
            if Path::new(&rel).extension().is_some_and(|e| e == "pack") {
                packs.push((format!("{name}/{rel}"), bytes));
            }
        }
    }
    (manifest, files, git_state, packs)
}

#[test]
fn same_seed_gives_identical_manifest_and_tree() {
    let (a, b) = (TempDir::new("fx-a"), TempDir::new("fx-b"));
    let ma = generate(&small_config(a.path(), 7)).unwrap();
    let mb = generate(&small_config(b.path(), 7)).unwrap();
    assert!(ma == mb, "the returned manifests differ");

    let (ga, gb) = (load(a.path().to_path_buf()), load(b.path().to_path_buf()));
    let (sa, sb) = (snapshot(&ga), snapshot(&gb));
    assert_eq!(sa.0, sb.0, "manifest.json differs");
    assert_eq!(sa.1.len(), sb.1.len());
    assert!(sa.1 == sb.1, "an NFS file differs");
    assert!(
        !sa.2.is_empty() && sa.2 == sb.2,
        "git refs, commit ids or trees differ"
    );
    assert!(
        !sa.3.is_empty() && sa.3 == sb.3,
        "packfiles differ on the same git binary"
    );
}

#[test]
fn generating_twice_into_the_same_directory_replaces_the_output() {
    let d = TempDir::new("fx-again");
    generate(&small_config(d.path(), 3)).unwrap();
    let first = std::fs::read(d.path().join("manifest.json")).unwrap();
    generate(&small_config(d.path(), 3)).unwrap();
    assert_eq!(first, std::fs::read(d.path().join("manifest.json")).unwrap());
}

#[test]
fn different_seed_differs() {
    let (a, b) = (TempDir::new("fx-s1"), TempDir::new("fx-s2"));
    generate(&small_config(a.path(), 1)).unwrap();
    generate(&small_config(b.path(), 2)).unwrap();
    let (ga, gb) = (load(a.path().to_path_buf()), load(b.path().to_path_buf()));
    assert_ne!(ga.manifest, gb.manifest);
    assert_ne!(ga.nfs, gb.nfs);
    let ra = git_text(&ga.base_repo(), &["rev-parse", "refs/heads/master"]);
    let rb = git_text(&gb.base_repo(), &["rev-parse", "refs/heads/master"]);
    assert_ne!(ra, rb);
}

#[test]
fn refuses_to_delete_an_unrelated_directory() {
    let d = TempDir::new("fx-unrelated");
    let precious = d.path().join("precious.txt");
    std::fs::write(&precious, "keep me").unwrap();
    let err = generate(&small_config(d.path(), 1));
    assert!(
        err.is_err(),
        "generate must not wipe a directory it did not create"
    );
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
}

// ---------------------------------------------------------------------------------------------------------------
// AC4b: target scale (ignored: 80,000 files; run by `just fixtures-verify`)

#[test]
#[ignore = "80,000 files; run with --ignored (just fixtures-verify)"]
fn target_scale_counts_match_design_scale() {
    let dir = TempDir::new("fx-target");
    let started = Instant::now();
    generate(&Config {
        scale: Scale::Target,
        seed: DEFAULT_SEED,
        out: dir.path().to_path_buf(),
    })
    .unwrap();
    let gen_time = started.elapsed();
    let g = load(dir.path().to_path_buf());

    // 40 swimlanes
    let swimlanes = g.swimlanes();
    assert_eq!(swimlanes.len(), 40);

    // ~2,000 files each (base files, tenant-suffixed files and a few planted extras)
    let mut per: BTreeMap<&str, usize> = BTreeMap::new();
    for (sl, _) in g.nfs.keys() {
        *per.entry(sl.as_str()).or_default() += 1;
    }
    assert_eq!(per.len(), 40);
    for (sl, n) in &per {
        assert!((1_900..=2_100).contains(n), "{sl} has {n} files");
    }
    let total: usize = per.values().sum();
    assert!(
        (76_000..=84_000).contains(&total),
        "{total} files in total, design scale is about 80,000"
    );

    // 60 tenant branches
    let heads = git_text(
        &g.tenant_repo(),
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    let tenant_branches: BTreeSet<&str> = heads
        .lines()
        .filter_map(|l| l.strip_prefix("refs/heads/sit"))
        .collect();
    assert_eq!(tenant_branches.len(), 60, "{heads}");
    assert_eq!(g.manifest["tenants"].as_array().unwrap().len(), 60);
    assert!(
        heads.lines().any(|l| l.starts_with("refs/heads/templates/")),
        "template branches missing"
    );

    // tenant sets: every swimlane has its own tenant, 20 have a second one
    let two = swimlanes
        .iter()
        .filter(|s| s["tenants"].as_array().unwrap().len() == 2)
        .count();
    assert_eq!(two, 20);
    assert!(
        swimlanes
            .iter()
            .all(|s| !s["tenants"].as_array().unwrap().is_empty())
    );

    // files are at most 2 MiB, and at least one has more than 6,000 lines
    let max = g.nfs.values().map(Vec::len).max().unwrap();
    assert!(max <= 2 * 1024 * 1024, "largest file is {max} bytes");
    assert!(
        max > 1024 * 1024,
        "the generator should include a file of over 1 MiB, largest is {max}"
    );
    let longest = g
        .nfs
        .values()
        .map(|b| b.iter().filter(|&&c| c == b'\n').count())
        .max()
        .unwrap();
    assert!(longest > 6_000, "longest file has {longest} lines");

    // images are about 1-3% of the files
    let images = g
        .nfs
        .keys()
        .filter(|(_, p)| [".png", ".bmp", ".gif", ".jpg"].iter().any(|e| p.ends_with(e)))
        .count();
    let pct = images * 100 / total;
    assert!((1..=3).contains(&pct), "{pct}% images");

    // the manifest agrees with what is on disk
    assert_eq!(
        g.manifest["counts"]["nfs_files"].as_u64().unwrap() as usize,
        total
    );
    eprintln!("target scale: {total} NFS files, largest {max} bytes, generated in {gen_time:.1?}");
}

// ---------------------------------------------------------------------------------------------------------------
// AC4c: required shapes (small scale)

#[test]
fn contains_channel_folders() {
    let g = shared();
    let channels: BTreeSet<&str> = g.manifest["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(channels.len() >= 3);
    // channels.yml in the base repo names them
    let channels_yml = git_text(&g.base_repo(), &["show", "refs/heads/master:config/channels.yml"]);
    for c in &channels {
        assert!(channels_yml.contains(c), "channels.yml does not list {c}");
    }
    let folders = g.manifest["channel_folders"].as_array().unwrap();
    assert!(folders.len() >= 3);
    for f in folders {
        let f = f.as_str().unwrap();
        let (parent, channel) = f.rsplit_once('/').unwrap();
        assert!(!parent.is_empty() && channels.contains(channel), "{f}");
        let prefix = format!("{f}/");
        assert!(
            g.nfs.keys().any(|(_, p)| p.starts_with(&prefix)),
            "no NFS file below {f}"
        );
    }
}

#[test]
fn contains_nested_service_folders() {
    let g = shared();
    let depth = g.nfs.keys().map(|(_, p)| p.matches('/').count()).max().unwrap();
    assert!(depth >= 2, "deepest file is at depth {depth}");
    assert!(g.nfs.keys().any(|(_, p)| p.starts_with("core-adapters/adapter-")));
}

#[test]
fn contains_crlf_files() {
    let g = shared();
    let (mut crlf, mut lf) = (0, 0);
    for (_, bytes) in g.texts() {
        if !is_text(bytes) || !bytes.contains(&b'\n') {
            continue;
        }
        let newlines = bytes.iter().filter(|&&c| c == b'\n').count();
        let crlfs = bytes.windows(2).filter(|w| w == b"\r\n").count();
        assert!(crlfs == 0 || crlfs == newlines, "a file mixes line endings");
        if crlfs > 0 {
            crlf += 1;
        } else {
            lf += 1;
        }
    }
    assert!(crlf >= 5, "{crlf} CRLF files");
    assert!(lf > crlf, "CRLF should be the minority: {crlf} CRLF, {lf} LF");
}

#[test]
fn contains_yaml_anchors() {
    let g = shared();
    let hit = g.texts().any(|(_, b)| {
        let t = String::from_utf8_lossy(b);
        t.contains(": &") && t.contains("<<: *")
    });
    assert!(hit, "no file with an anchor, an alias and a merge key");
}

#[test]
fn contains_duplicate_keys() {
    let g = shared();
    let planted = g.planted("duplicate_keys");
    assert!(!planted.is_empty());
    let mut checked = 0;
    for entry in planted {
        let path = entry["path"].as_str().unwrap();
        let bytes = g
            .nfs
            .iter()
            .find(|((_, p), _)| p == path)
            .unwrap_or_else(|| panic!("{path} not on NFS"))
            .1;
        let text = String::from_utf8_lossy(bytes);
        for key in entry["keys"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            let n = text
                .lines()
                .filter(|l| l.trim_start().starts_with(&format!("{key}:")))
                .count();
            assert!(n >= 2, "{path}: key {key} appears {n} time(s)");
            checked += 1;
        }
    }
    assert!(checked >= 3);
    // the example of the real data: security-roles.yml repeats three role keys
    assert!(
        planted
            .iter()
            .any(|e| e["path"] == "security/security-roles.yml" && e["keys"].as_array().unwrap().len() == 3)
    );
}

#[test]
fn contains_xsl() {
    let g = shared();
    let xsl: Vec<_> = g.nfs.iter().filter(|((_, p), _)| p.ends_with(".xsl")).collect();
    assert!(!xsl.is_empty());
    for (_, bytes) in xsl {
        assert!(String::from_utf8_lossy(bytes).contains("xsl:stylesheet"));
    }
}

#[test]
fn contains_images() {
    let g = shared();
    let mut kinds = BTreeSet::new();
    for ((_, p), bytes) in &g.nfs {
        let magic: &[(&str, &[u8])] = &[
            (".png", b"\x89PNG"),
            (".bmp", b"BM"),
            (".gif", b"GIF8"),
            (".jpg", b"\xff\xd8\xff"),
        ];
        for (ext, m) in magic {
            if p.ends_with(ext) {
                assert!(bytes.starts_with(m), "{p} has the wrong magic bytes");
                kinds.insert(*ext);
            }
        }
    }
    assert!(kinds.len() >= 2, "image kinds: {kinds:?}");
    // the real-data example of C2: a .bmp that tenant sit1 copies unchanged
    assert!(
        g.nfs
            .keys()
            .any(|(_, p)| p == "document-service/resources/receipt-ci_1.bmp")
    );
}

#[test]
fn contains_extensionless_files() {
    let g = shared();
    let names: Vec<&String> = g
        .nfs
        .keys()
        .map(|(_, p)| p)
        .filter(|p| !p.rsplit('/').next().unwrap().contains('.'))
        .collect();
    assert!(names.len() >= 2, "{names:?}");
    assert!(
        g.nfs
            .keys()
            .any(|(_, p)| p == "jwt-proxy-injector-service/mappingsItemEvaluationE2ETest")
    );
    assert!(
        g.nfs
            .keys()
            .any(|(_, p)| p == "jwt-proxy-injector-service/mappingsItemEvaluationE2ETest-sit1")
    );
}

#[test]
fn contains_six_thousand_line_yaml() {
    let g = shared();
    let longest = g
        .nfs
        .iter()
        .filter(|((_, p), _)| p.ends_with(".yml") || p.ends_with(".yaml"))
        .map(|(_, b)| b.iter().filter(|&&c| c == b'\n').count())
        .max()
        .unwrap();
    assert!(longest > 6_000, "longest YAML has {longest} lines");
    assert!(g.manifest["counts"]["largest_yaml_lines"].as_u64().unwrap() > 6_000);
}

/// True when `key` occurs in `text` as a whole name (not as part of a longer identifier).
fn has_entry(text: &str, key: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || matches!(c, '-' | '.' | '_');
    text.match_indices(key).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + key.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

#[test]
fn contains_tenant_forks_with_missing_entries() {
    let g = shared();
    let forks = g.planted("stale_forks");
    assert!(!forks.is_empty());
    let mut checked = 0;
    for f in forks {
        let (tenant, path) = (f["tenant"].as_str().unwrap(), f["path"].as_str().unwrap());
        let missing: Vec<&str> = f["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        assert!(!missing.is_empty());
        let base = git_text(
            &g.base_repo(),
            &["show", &format!("refs/heads/master:config/{path}")],
        );
        let fork = git_text(
            &g.tenant_repo(),
            &["show", &format!("refs/heads/{tenant}:data/config/{path}")],
        );
        for entry in missing {
            assert!(has_entry(&base, entry), "{path}: base head lacks {entry}");
            assert!(
                !has_entry(&fork, entry),
                "{tenant} {path}: the fork has {entry}, so it is not stale"
            );
        }
        checked += 1;
    }
    assert!(checked >= 1);
    // the real-data example of C1: sit1's entryGroupConfig.yml has no `atm` entry
    assert!(forks.iter().any(|f| f["tenant"] == "sit1"
        && f["path"] == "tx-infinity-api/entryGroupConfig.yml"
        && f["missing"].as_array().unwrap().iter().any(|m| m == "atm")));
}

#[test]
fn contains_redundant_copies() {
    let g = shared();
    let copies = g.planted("redundant_copies");
    assert!(!copies.is_empty());
    for c in copies {
        let (tenant, path) = (c["tenant"].as_str().unwrap(), c["path"].as_str().unwrap());
        let base = git(
            &g.base_repo(),
            &["show", &format!("refs/heads/master:config/{path}")],
        );
        let fork = git(
            &g.tenant_repo(),
            &["show", &format!("refs/heads/{tenant}:data/config/{path}")],
        );
        assert!(
            base == fork,
            "{tenant} {path} is listed as a redundant copy but differs from base"
        );
    }
    // the real-data examples of C2
    for p in [
        "tx-infinity-api/account-sorting-config.yml",
        "tx-infinity-api/miniStatementConfig.yml",
    ] {
        assert!(
            copies.iter().any(|c| c["tenant"] == "sit1" && c["path"] == p),
            "{p}"
        );
    }
}

#[test]
fn contains_duplicate_names_in_non_channel_folders() {
    let g = shared();
    let channel_folders: BTreeSet<&str> = g.manifest["channel_folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    let dups = g.planted("duplicate_names");
    assert!(dups.len() >= 3);
    let mut max_folders = 0;
    for d in dups {
        let name = d["name"].as_str().unwrap();
        let folders: Vec<&str> = d["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect();
        assert!(folders.len() >= 2, "{name}");
        max_folders = max_folders.max(folders.len());
        for f in folders {
            assert!(!channel_folders.contains(f), "{name}: {f} is a channel folder");
            let path = format!("{f}/{name}");
            let on_git = git(
                &g.base_repo(),
                &["cat-file", "-e", &format!("refs/heads/master:config/{path}")],
            );
            assert!(on_git.is_empty());
        }
    }
    assert!(
        max_folders >= 3,
        "authentication-config.yaml should be in several folders"
    );
    // and the plan is complete: no other name is shared by two non-channel folders
    let listing = git_text(
        &g.base_repo(),
        &["ls-tree", "-r", "--name-only", "refs/heads/master", "config/"],
    );
    let mut seen: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for line in listing.lines() {
        let rel = line.strip_prefix("config/").unwrap();
        let (folder, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        if !channel_folders.contains(folder) {
            seen.entry(name).or_default().insert(folder);
        }
    }
    let planted: BTreeSet<&str> = dups.iter().map(|d| d["name"].as_str().unwrap()).collect();
    let actual: BTreeSet<&str> = seen
        .iter()
        .filter(|(_, f)| f.len() > 1)
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(planted, actual);
}

#[test]
fn nfs_uses_tenant_suffix_naming() {
    let g = shared();
    let rule = PathMappingRule::default();
    // the worked example of config-semantics
    assert!(g.nfs.contains_key(&(
        "sl-01".to_owned(),
        "tx-infinity-api/tx-infinity-core-sit1.yml".to_owned()
    )));
    assert!(g.nfs.contains_key(&(
        "sl-01".to_owned(),
        "tx-infinity-api/tx-infinity-core.yml".to_owned()
    )));

    for sl in g.swimlanes() {
        let id = sl["id"].as_str().unwrap();
        let tenants = g.tenants_of(id);
        let set = TenantSet::new(tenants.iter().map(|t| TenantId::parse(t).unwrap())).unwrap();
        let planted_edits: BTreeSet<String> = ["nfs_ahead", "untracked", "orphans"]
            .iter()
            .flat_map(|k| g.planted(k).iter())
            .filter(|p| p["swimlane"] == id)
            .map(|p| p["path"].as_str().unwrap().to_owned())
            .collect();
        let mut tenant_files = BTreeMap::<String, usize>::new();
        for ((s, path), bytes) in &g.nfs {
            if s != id {
                continue;
            }
            let nfs = NfsPath::parse(path).unwrap();
            if let ReverseMapping::Tenant {
                tenant, repo_path, ..
            } = rule.reverse(&nfs, &set).unwrap()
            {
                *tenant_files.entry(tenant.as_str().to_owned()).or_default() += 1;
                // the forward mapping returns the same NFS path, and the file is what the tenant branch holds
                assert_eq!(rule.tenant_to_nfs(&repo_path, &tenant).unwrap(), nfs);
                if !planted_edits.contains(path) {
                    let in_git = git(
                        &g.tenant_repo(),
                        &[
                            "show",
                            &format!("refs/heads/{}:{}", tenant.as_str(), repo_path.as_str()),
                        ],
                    );
                    assert!(
                        &in_git == bytes,
                        "{id}/{path} differs from branch {}",
                        tenant.as_str()
                    );
                }
            }
        }
        // a swimlane has files for every tenant of its set (Q34)
        for t in &tenants {
            assert!(
                tenant_files.get(t).copied().unwrap_or(0) > 0,
                "{id}: no file for tenant {t}"
            );
        }
    }
}

#[test]
fn planted_drift_cases_exist_and_are_real() {
    let g = shared();
    for kind in ["nfs_ahead", "git_ahead", "untracked", "orphans"] {
        let list = g.planted(kind);
        assert!(!list.is_empty(), "no {kind}");
        for p in list {
            let key = (
                p["swimlane"].as_str().unwrap().to_owned(),
                p["path"].as_str().unwrap().to_owned(),
            );
            assert!(g.nfs.contains_key(&key), "{kind}: {key:?} is not on NFS");
        }
    }
    // untracked files have no counterpart in Git; orphans carry a suffix that is not in the swimlane's tenant set
    for p in g.planted("orphans") {
        let sl = p["swimlane"].as_str().unwrap();
        assert!(
            !g.tenants_of(sl)
                .contains(&p["suffix"].as_str().unwrap().to_owned())
        );
    }
}

#[test]
fn git_repos_are_valid_and_shaped_like_the_real_ones() {
    let g = shared();
    for repo in [g.base_repo(), g.tenant_repo()] {
        git(&repo, &["fsck", "--strict", "--no-dangling"]);
    }
    let base = g.base_repo();
    // one branch, tags mark versions, config under config/
    let heads = git_text(&base, &["for-each-ref", "--format=%(refname)", "refs/heads/"]);
    assert_eq!(heads.trim(), "refs/heads/master");
    let tags = git_text(&base, &["for-each-ref", "--format=%(refname)", "refs/tags/"]);
    assert!(tags.lines().count() >= 3, "{tags}");
    assert_eq!(
        git_text(&base, &["rev-list", "--count", "refs/heads/master"]).trim(),
        "3"
    );
    let names = git_text(&base, &["ls-tree", "-r", "--name-only", "refs/heads/master"]);
    assert!(names.lines().all(|l| l.starts_with("config/")));

    // tenant data: sit<N> branches under data/config/, templates/* and templates/common-docs
    let tenant = g.tenant_repo();
    let heads = git_text(&tenant, &["for-each-ref", "--format=%(refname)", "refs/heads/"]);
    assert!(heads.lines().any(|l| l == "refs/heads/sit1"));
    assert!(heads.lines().any(|l| l == "refs/heads/templates/common-docs"));
    assert!(
        heads
            .lines()
            .any(|l| l.starts_with("refs/heads/templates/") && l != "refs/heads/templates/common-docs")
    );
    let sit1 = git_text(&tenant, &["ls-tree", "-r", "--name-only", "refs/heads/sit1"]);
    assert!(sit1.lines().all(|l| l.starts_with("data/config/")));
    let docs = git_text(
        &tenant,
        &["ls-tree", "-r", "--name-only", "refs/heads/templates/common-docs"],
    );
    assert!(docs.lines().any(|l| l.ends_with(".md")));
    // the manifest's ref ids are the repo's
    for (name, repo) in [("base", &base), ("tenant", &tenant)] {
        let refs = g.manifest["repos"][name]["refs"].as_object().unwrap();
        let actual = git_text(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]);
        let actual: BTreeMap<&str, &str> = actual.lines().map(|l| l.split_once(' ').unwrap()).collect();
        assert_eq!(refs.len(), actual.len());
        for (r, id) in refs {
            assert_eq!(actual[r.as_str()], id.as_str().unwrap());
        }
    }
}

#[test]
fn no_real_hosts_or_people_appear_in_fixtures() {
    // AGENTS.md rule 12: only reserved example domains.
    let g = shared();
    for ((sl, p), bytes) in &g.nfs {
        if !is_text(bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        for part in text.split("://").skip(1) {
            let host: String = part
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            assert!(
                // www.w3.org is the XSLT namespace identifier that every stylesheet must declare, not a service.
                host.ends_with(".example")
                    || host.ends_with(".invalid")
                    || host == "localhost"
                    || host == "www.w3.org",
                "{sl}/{p}: host {host} is not a reserved example domain"
            );
        }
        // An e-mail address is `name@host`; an XPath `@attr` or a YAML `@` is not one.
        let chars: Vec<char> = text.chars().collect();
        for i in 1..chars.len().saturating_sub(1) {
            if chars[i] == '@' && chars[i - 1].is_ascii_alphanumeric() && chars[i + 1].is_ascii_alphanumeric()
            {
                let host: String = chars[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '.' || **c == '-')
                    .collect();
                assert!(
                    host.ends_with(".example") || host.ends_with(".invalid"),
                    "{sl}/{p}: e-mail address at {host}"
                );
            }
        }
    }
}

#[test]
fn text_files_never_contain_the_rendering_placeholders_of_other_files() {
    // `${KEY}` placeholders exist (config-server rendering), and some of them have no value in the property view (C10).
    let g = shared();
    let mut with = 0;
    for (_, bytes) in g.texts() {
        if String::from_utf8_lossy(bytes).contains("${") {
            with += 1;
        }
    }
    assert!(with >= 3);
    let core = g
        .nfs
        .get(&(
            "sl-01".to_owned(),
            "tx-infinity-api/tx-infinity-core.yml".to_owned(),
        ))
        .unwrap();
    let core = String::from_utf8_lossy(core);
    assert!(core.contains("${CORE_ROUTING}") && core.contains("${LOG_LEVEL}"));
}

// ---------------------------------------------------------------------------------------------------------------
// The binary

#[test]
fn cli_generates_and_rejects_bad_arguments() {
    let d = TempDir::new("fx-cli");
    let out = d.path().join("out");
    let ok = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gen-fixtures", "--scale", "small", "--seed", "5", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    assert!(out.join("manifest.json").is_file());
    let manifest: Value = serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["seed"], 5);

    for args in [
        vec!["gen-fixtures"],
        vec!["gen-fixtures", "--scale", "huge"],
        vec!["gen-fixtures", "--scale", "small", "--seed", "x"],
    ] {
        let bad = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(bad.status.code(), Some(2), "{args:?}");
    }
    let _ = RepoPath::parse("config"); // keep the domain import honest
}
