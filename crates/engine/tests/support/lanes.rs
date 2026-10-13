//! The synthetic fixtures read the way the hub reads a swimlane (T8): every file on NFS classified against all the deployable
//! branches, with the content hashes of the Git side (G) and a baseline, ready for the batch functions. Used by the tests and,
//! through `#[path]`, by the benchmarks of T8.
//!
//! The reading is the test's own code and not the engine's: the engine does no I/O. Git is read with the `git` command line
//! (`ls-tree`, `cat-file --batch`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bytes::Bytes;
use domain::{ContentHash, NfsPath, PathMappingRule, ReverseMapping, SwimlaneId, TenantId, TenantSet};
use engine::{Baseline, DenyMatcher, LaneFile, LaneInput, NfsContent, classify, classify_path, drift_hash};
use sha2::{Digest, Sha256};

/// A swimlane read from the fixtures.
pub struct LaneData {
    pub id: SwimlaneId,
    pub tenants: TenantSet,
    pub files: Vec<LaneFile>,
}

impl LaneData {
    pub fn input(&self) -> LaneInput<'_> {
        LaneInput {
            swimlane: &self.id,
            tenants: &self.tenants,
            tier: None,
            files: &self.files,
        }
    }
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

/// The contents of every blob in `oids`, by object id, from one `cat-file --batch` run.
fn read_blobs(repo: &Path, oids: &BTreeSet<String>) -> BTreeMap<String, Bytes> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let wanted: Vec<String> = oids.iter().cloned().collect();
    let writer = std::thread::spawn(move || {
        for oid in wanted {
            writeln!(stdin, "{oid}").unwrap();
        }
    });
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    writer.join().unwrap();
    assert!(child.wait().unwrap().success());

    let mut blobs = BTreeMap::new();
    let mut at = 0;
    while at < out.len() {
        let eol = at + out[at..].iter().position(|&b| b == b'\n').unwrap();
        let header = std::str::from_utf8(&out[at..eol]).unwrap();
        let mut parts = header.split(' ');
        let oid = parts.next().unwrap().to_owned();
        assert_eq!(parts.next(), Some("blob"), "{header}");
        let size: usize = parts.next().unwrap().parse().unwrap();
        let body = eol + 1;
        blobs.insert(oid, Bytes::copy_from_slice(&out[body..body + size]));
        at = body + size + 1;
    }
    blobs
}

/// The files of one revision: repository path to content.
fn tree_files(repo: &Path, rev: &str) -> BTreeMap<String, Bytes> {
    let listing = git(repo, &["ls-tree", "-r", "-z", rev]);
    let mut entries: Vec<(String, String)> = Vec::new();
    for entry in listing.split(|&b| b == 0).filter(|e| !e.is_empty()) {
        let text = std::str::from_utf8(entry).unwrap();
        let (meta, path) = text.split_once('\t').unwrap();
        let oid = meta.split(' ').nth(2).unwrap();
        entries.push((path.to_owned(), oid.to_owned()));
    }
    let oids: BTreeSet<String> = entries.iter().map(|(_, oid)| oid.clone()).collect();
    let blobs = read_blobs(repo, &oids);
    entries
        .into_iter()
        .map(|(path, oid)| (path, blobs[&oid].clone()))
        .collect()
}

fn sha256(data: &[u8]) -> ContentHash {
    ContentHash::from_bytes(Sha256::digest(data).into())
}

fn files_under(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// Reads the first `limit` swimlanes of the fixtures at `root` (all of them with `usize::MAX`).
///
/// The baseline of a tracked file is its Git content, except for the files the generator planted as `git_ahead`, where NFS
/// still holds the baseline and Git has moved on. So the fixtures show `in_sync`, `nfs_ahead`, `git_ahead` and `untracked`,
/// and never the rest (a baseline has to be recorded by somebody).
pub fn load_lanes(root: &Path, limit: usize) -> Vec<LaneData> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let text = |v: &serde_json::Value| v.as_str().unwrap().to_owned();
    let base_repo = root.join(text(&manifest["repos"]["base"]["path"]));
    let tenant_repo = root.join(text(&manifest["repos"]["tenant"]["path"]));
    let base_default = text(&manifest["repos"]["base"]["default_branch"]);
    let all_tenants: Vec<String> = manifest["tenants"].as_array().unwrap().iter().map(text).collect();

    let base_tree = tree_files(&base_repo, &base_default);
    let tenant_trees: BTreeMap<String, BTreeMap<String, Bytes>> = all_tenants
        .iter()
        .map(|t| (t.clone(), tree_files(&tenant_repo, &format!("refs/heads/{t}"))))
        .collect();
    let all = TenantSet::new(all_tenants.iter().map(|t| TenantId::parse(t).unwrap())).unwrap();

    let git_ahead: BTreeSet<(String, String)> = manifest["planted"]["git_ahead"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (text(&g["swimlane"]), text(&g["path"])))
        .collect();

    let rule = PathMappingRule::default();
    let deny = DenyMatcher::default();
    let mut lanes = Vec::new();
    for lane in manifest["swimlanes"].as_array().unwrap().iter().take(limit) {
        let id = text(&lane["id"]);
        let tenants = TenantSet::new(
            lane["tenants"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| TenantId::parse(t.as_str().unwrap()).unwrap()),
        )
        .unwrap();
        let mut files = Vec::new();
        for (rel, disk) in files_under(&root.join(text(&lane["nfs_root"]))) {
            let path = NfsPath::parse(&rel).unwrap();
            let data = Bytes::from(std::fs::read(&disk).unwrap());
            let class = classify(&path, Some(&data), &deny);
            let git_content = std::cell::Cell::new(None);
            let facts = classify_path(&rule, &path, &all, |mapping| {
                let found = match mapping {
                    ReverseMapping::Base { repo_path, .. } => base_tree.get(repo_path.as_str()),
                    ReverseMapping::Tenant {
                        tenant, repo_path, ..
                    } => tenant_trees
                        .get(tenant.as_str())
                        .and_then(|tree| tree.get(repo_path.as_str())),
                };
                git_content.set(found);
                found.is_some()
            })
            .unwrap();
            let git_hash = git_content
                .get()
                .map(|b: &Bytes| drift_hash(Some(b), class).unwrap());
            let nfs_hash = drift_hash(Some(&data), class);
            let baseline = match git_hash {
                None => Baseline::NotRecorded,
                Some(_) if git_ahead.contains(&(id.clone(), rel.clone())) => Baseline::Recorded(nfs_hash),
                Some(g) => Baseline::Recorded(Some(g)),
            };
            files.push(LaneFile {
                path,
                facts,
                class,
                nfs: Some(NfsContent {
                    blob: sha256(&data),
                    bytes: data,
                }),
                git: git_hash,
                baseline,
                intentional: None,
                overwrite: None,
                history: None,
            });
        }
        lanes.push(LaneData {
            id: SwimlaneId::parse(&id).unwrap(),
            tenants,
            files,
        });
    }
    lanes
}

/// A branch of the tenant repository with its files hashed like the swimlane's (`drift_hash`).
pub struct BranchData {
    pub name: domain::GitRef,
    pub tenant: TenantId,
    pub files: Vec<(domain::RepoPath, ContentHash)>,
}

/// Every tenant branch of the fixtures at `root`.
pub fn load_branches(root: &Path) -> Vec<BranchData> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let tenant_repo = root.join(manifest["repos"]["tenant"]["path"].as_str().unwrap());
    let deny = DenyMatcher::default();
    manifest["tenants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let name = t.as_str().unwrap();
            let files = tree_files(&tenant_repo, &format!("refs/heads/{name}"))
                .into_iter()
                .map(|(path, content)| {
                    let class = classify(&NfsPath::parse(&path).unwrap(), Some(&content), &deny);
                    (
                        domain::RepoPath::parse(&path).unwrap(),
                        drift_hash(Some(&content), class).unwrap(),
                    )
                })
                .collect();
            BranchData {
                name: domain::GitRef::parse(name).unwrap(),
                tenant: TenantId::parse(name).unwrap(),
                files,
            }
        })
        .collect()
}
