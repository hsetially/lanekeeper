//! NFS snapshots: one directory per swimlane, laid out the way the agent sees the mount.
//!
//! A base file `a/b.yml` sits at `a/b.yml`. A tenant file sits beside it with the tenant id before the extension,
//! `a/b-sit1.yml` (D13, `PathMappingRule`). On top of the files Git has, a snapshot carries planted differences that
//! the drift and check code must find: hand edits, a stale base version, untracked files and orphan suffixes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use domain::{PathMappingRule, RepoPath, TenantId};
use sha2::{Digest, Sha256};

use super::FixtureError;
use super::content::{Doc, Format, HEAD_VERSION};
use super::manifest::{GitAhead, Orphan, SwimlanePath};
use super::rng::Rng;
use super::tenant::{TenantPlan, render};

/// What one swimlane needs to be written.
#[derive(Debug)]
pub struct SwimlaneSpec<'a> {
    pub id: &'a str,
    pub tenants: &'a [String],
    /// A tenant that belongs to another swimlane, used for the orphan file.
    pub foreign_tenant: &'a str,
    /// Whether the snapshot lags behind Git for some base files.
    pub lagging: bool,
    pub docs: &'a [Doc],
    pub plans: &'a BTreeMap<String, TenantPlan>,
}

/// Numbers and planted items found while writing one swimlane.
#[derive(Debug, Default)]
pub struct SwimlaneOutcome {
    pub files: usize,
    pub tree_sha256: String,
    pub max_file_bytes: usize,
    pub max_file_lines: usize,
    pub largest_yaml_lines: usize,
    pub image_files: usize,
    pub nfs_ahead: Vec<SwimlanePath>,
    pub git_ahead: Vec<GitAhead>,
    pub untracked: Vec<SwimlanePath>,
    pub orphans: Vec<Orphan>,
}

struct Pending {
    path: String,
    bytes: Vec<u8>,
    /// Index of the catalog doc and whether it is a tenant file, for hand edits.
    editable: Option<Format>,
    eol: &'static str,
}

/// The NFS path of a tenant file.
///
/// # Errors
/// When the mapping rejects the path (a generator bug).
pub fn tenant_nfs_path(rule: &PathMappingRule, doc_path: &str, tenant: &str) -> Result<String, FixtureError> {
    let repo_path = RepoPath::parse(&format!("data/config/{doc_path}"))
        .map_err(|e| FixtureError::Domain(e.to_string()))?;
    let tenant = TenantId::parse(tenant).map_err(|e| FixtureError::Domain(e.to_string()))?;
    let nfs = rule
        .tenant_to_nfs(&repo_path, &tenant)
        .map_err(|e| FixtureError::Domain(e.to_string()))?;
    Ok(nfs.as_str().to_owned())
}

fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn lines_in(bytes: &[u8]) -> usize {
    bytes.iter().fold(0, |n, &b| n + usize::from(b == b'\n'))
}

fn is_yaml(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

fn is_image(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| {
        ["png", "bmp", "gif", "jpg"]
            .iter()
            .any(|i| e.eq_ignore_ascii_case(i))
    })
}

/// Base files at Git's head, except a few that lag behind (`git_ahead`) in a lagging swimlane.
fn base_files(spec: &SwimlaneSpec<'_>, rng: &mut Rng, out: &mut SwimlaneOutcome) -> Vec<Pending> {
    let mut lag_budget = if spec.lagging {
        (spec.docs.len() / 60).max(1)
    } else {
        0
    };
    let mut files = Vec::with_capacity(spec.docs.len() + 700);
    for doc in spec.docs {
        let mut bytes = doc.render(HEAD_VERSION, None);
        let mut lagging = false;
        if lag_budget > 0 && doc.born == 1 && doc.format != Format::Binary && rng.chance(25) {
            if let Some(v) = older_version(doc, &bytes) {
                bytes = doc.render(v, None);
                lag_budget -= 1;
                lagging = true;
                out.git_ahead.push(GitAhead {
                    swimlane: spec.id.to_owned(),
                    path: doc.path.clone(),
                    nfs_version: v,
                });
            }
        }
        files.push(Pending {
            path: doc.path.clone(),
            bytes,
            editable: (doc.format.takes_trailing_comment() && !lagging).then_some(doc.format),
            eol: doc.eol.as_str(),
        });
    }
    // A small catalog may not have used the lag budget; a lagging swimlane must still have one such file.
    if spec.lagging && out.git_ahead.is_empty() {
        for (i, doc) in spec.docs.iter().enumerate() {
            if doc.born != 1 || doc.format == Format::Binary {
                continue;
            }
            if let Some(v) = older_version(doc, &files[i].bytes) {
                files[i].bytes = doc.render(v, None);
                files[i].editable = None;
                out.git_ahead.push(GitAhead {
                    swimlane: spec.id.to_owned(),
                    path: doc.path.clone(),
                    nfs_version: v,
                });
                break;
            }
        }
    }
    files
}

/// The newest older version whose content differs from `head`.
fn older_version(doc: &Doc, head: &[u8]) -> Option<u8> {
    (1..HEAD_VERSION).rev().find(|&v| doc.render(v, None) != head)
}

/// Tenant files, named with the tenant id before the extension.
fn tenant_files(spec: &SwimlaneSpec<'_>, rule: &PathMappingRule) -> Result<Vec<Pending>, FixtureError> {
    let mut files = Vec::new();
    for tenant in spec.tenants {
        let plan = spec
            .plans
            .get(tenant)
            .ok_or_else(|| FixtureError::Domain(format!("no plan for tenant {tenant}")))?;
        for f in &plan.files {
            let doc = &spec.docs[f.doc];
            files.push(Pending {
                path: tenant_nfs_path(rule, &doc.path, tenant)?,
                bytes: render(doc, f.kind, tenant),
                editable: doc.format.takes_trailing_comment().then_some(doc.format),
                eol: doc.eol.as_str(),
            });
        }
    }
    Ok(files)
}

/// Hand edits on NFS: the file changed after the last sync (`nfs_ahead`).
fn hand_edits(id: &str, files: &mut [Pending], rng: &mut Rng, out: &mut SwimlaneOutcome) {
    let edits = (files.len() / 400).max(1);
    let mut candidates: Vec<usize> = (0..files.len())
        .filter(|&i| files[i].editable.is_some())
        .collect();
    rng.shuffle(&mut candidates);
    for &i in candidates.iter().take(edits) {
        let n = out.nfs_ahead.len() + 1;
        let eol = files[i].eol;
        files[i]
            .bytes
            .extend_from_slice(format!("# hotfix {n}{eol}").as_bytes());
        out.nfs_ahead.push(SwimlanePath {
            swimlane: id.to_owned(),
            path: files[i].path.clone(),
        });
    }
}

/// Two untracked files (present on NFS, absent from Git) and one orphan suffix.
fn leftovers(spec: &SwimlaneSpec<'_>, rng: &mut Rng, out: &mut SwimlaneOutcome) -> Vec<Pending> {
    let folders: BTreeSet<&str> = spec.docs.iter().map(|d| folder_of(&d.path)).collect();
    let folders: Vec<&str> = folders.into_iter().collect();
    let tenant = &spec.tenants[0];
    let extras = [
        (format!("scratch-{}-1-{tenant}.yml", spec.id), "untracked"),
        (format!("scratch-{}-2.txt", spec.id), "untracked"),
        (
            format!("orphan-{}-{}.yml", spec.id, spec.foreign_tenant),
            "orphan",
        ),
    ];
    let mut files = Vec::new();
    for (name, kind) in extras {
        let folder = *rng.pick(&folders);
        let path = join(folder, &name);
        let body = format!("# synthetic leftover on {}\nnote: {kind}\n", spec.id);
        files.push(Pending {
            path: path.clone(),
            bytes: body.into_bytes(),
            editable: None,
            eol: "\n",
        });
        if kind == "untracked" {
            out.untracked.push(SwimlanePath {
                swimlane: spec.id.to_owned(),
                path,
            });
        } else {
            out.orphans.push(Orphan {
                swimlane: spec.id.to_owned(),
                path,
                suffix: spec.foreign_tenant.to_owned(),
            });
        }
    }
    files
}

/// Writes the files, and records sizes and the tree hash.
fn write_all(root: &Path, files: &[Pending], out: &mut SwimlaneOutcome) -> Result<(), FixtureError> {
    let mut made: BTreeSet<PathBuf> = BTreeSet::new();
    let mut hashes: Vec<(&str, [u8; 32])> = Vec::with_capacity(files.len());
    for f in files {
        let full = root.join(&f.path);
        if let Some(dir) = full.parent() {
            if !made.contains(dir) {
                std::fs::create_dir_all(dir)?;
                made.insert(dir.to_path_buf());
            }
        }
        std::fs::write(&full, &f.bytes)?;
        hashes.push((f.path.as_str(), Sha256::digest(&f.bytes).into()));
        out.max_file_bytes = out.max_file_bytes.max(f.bytes.len());
        let lines = lines_in(&f.bytes);
        out.max_file_lines = out.max_file_lines.max(lines);
        if is_yaml(&f.path) {
            out.largest_yaml_lines = out.largest_yaml_lines.max(lines);
        }
        if is_image(&f.path) {
            out.image_files += 1;
        }
    }
    hashes.sort_by(|a, b| a.0.cmp(b.0));
    let mut tree = Sha256::new();
    for (path, h) in &hashes {
        tree.update(path.as_bytes());
        tree.update([0]);
        tree.update(h);
    }
    out.files = files.len();
    out.tree_sha256 = hex::encode(tree.finalize());
    Ok(())
}

/// Writes the snapshot of one swimlane below `root` (the swimlane's own directory).
///
/// # Errors
/// On a filesystem error or a mapping failure.
pub fn write_swimlane(
    root: &Path,
    spec: &SwimlaneSpec<'_>,
    rng_root: &Rng,
    rule: &PathMappingRule,
) -> Result<SwimlaneOutcome, FixtureError> {
    let mut rng = rng_root.fork(&format!("swimlane/{}", spec.id));
    let mut out = SwimlaneOutcome::default();
    let mut files = base_files(spec, &mut rng, &mut out);
    files.extend(tenant_files(spec, rule)?);
    hand_edits(spec.id, &mut files, &mut rng, &mut out);
    files.extend(leftovers(spec, &mut rng, &mut out));
    write_all(root, &files, &mut out)?;
    Ok(out)
}
