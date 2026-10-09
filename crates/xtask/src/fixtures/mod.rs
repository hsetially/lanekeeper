//! `cargo xtask gen-fixtures --scale small|target`: deterministic synthetic data (prompt 01, T8; plan Q20, Q21).
//!
//! Output, all below the output directory and git-ignored (AGENTS.md rule 12: never real tenant data):
//!
//! ```text
//! manifest.json                         what was generated, and the planted items (an oracle for engine tests)
//! repos/configuration-base-saas.git     bare repo: branch master, three commits, tags v1.0.0..v3.0.0, files under config/
//! repos/csp-tenant-data.git             bare repo: sit1..sitN, templates/core, templates/common-docs; files under data/config/
//! nfs/<swimlane>/...                    one NFS snapshot per swimlane, tenant files named <name>-<tenant>.<ext>
//! ```
//!
//! Everything derives from the seed through [`rng::Rng`], so two runs with the same seed give the same manifest, the
//! same files and the same Git object ids. The repos are built with `git fast-import` ([`gitrepo`]).

pub mod catalog;
pub mod content;
pub mod gitrepo;
pub mod manifest;
pub mod nfs;
pub mod rng;
pub mod tenant;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use domain::PathMappingRule;

use self::catalog::CatalogParams;
use self::content::{Doc, HEAD_VERSION};
use self::manifest::{
    Counts, DuplicateKeys, DuplicateName, Manifest, PathRoots, Planted, Repo, Repos, StaleFork, Swimlane,
    TenantPath,
};
use self::rng::Rng;
use self::tenant::{ForkKind, TenantPlan};

/// The seed used when none is given.
pub const DEFAULT_SEED: u64 = 20_260_101;

/// Why generation failed.
#[derive(Debug)]
pub enum FixtureError {
    Io(std::io::Error),
    Git(String),
    Domain(String),
    /// The output directory exists and is not a previous output.
    OutputNotEmpty(PathBuf),
    Manifest(String),
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Git(e) => write!(f, "git: {e}"),
            Self::Domain(e) => write!(f, "path mapping: {e}"),
            Self::OutputNotEmpty(p) => write!(
                f,
                "{} exists and does not look like fixture output (no manifest.json); refusing to delete it",
                p.display()
            ),
            Self::Manifest(e) => write!(f, "manifest: {e}"),
        }
    }
}

impl std::error::Error for FixtureError {}

impl From<std::io::Error> for FixtureError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// How much to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// A few swimlanes with a few hundred files: for unit tests and the determinism check.
    Small,
    /// The design scale of `docs/performance.md`: 40 swimlanes of about 2,000 files and 60 tenant branches.
    Target,
}

impl Scale {
    /// # Errors
    /// When `s` is not `small` or `target`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "small" => Ok(Self::Small),
            "target" => Ok(Self::Target),
            other => Err(format!("unknown scale {other:?}, expected small or target")),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Target => "target",
        }
    }

    fn params(self) -> Params {
        match self {
            Self::Small => Params {
                swimlanes: 4,
                two_tenant_swimlanes: 2,
                base_files: 80,
                tenant_files_single: 40,
                tenant_files_primary_split: 25,
                tenant_files_secondary_split: 15,
                adapters: 4,
                dup_names: 3,
                big_yaml: 1,
                huge_xsl_bytes: 60_000,
                docs_pages: 3,
            },
            Self::Target => Params {
                swimlanes: 40,
                two_tenant_swimlanes: 20,
                base_files: 1_400,
                tenant_files_single: 600,
                tenant_files_primary_split: 400,
                tenant_files_secondary_split: 200,
                adapters: 12,
                dup_names: 13,
                big_yaml: 4,
                huge_xsl_bytes: 1_800_000,
                docs_pages: 3,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Params {
    swimlanes: usize,
    /// Swimlanes that get a second tenant (Q34: a swimlane has a set of tenants).
    two_tenant_swimlanes: usize,
    base_files: usize,
    tenant_files_single: usize,
    tenant_files_primary_split: usize,
    tenant_files_secondary_split: usize,
    adapters: usize,
    dup_names: usize,
    big_yaml: usize,
    huge_xsl_bytes: usize,
    docs_pages: usize,
}

/// What to generate and where.
#[derive(Debug, Clone)]
pub struct Config {
    pub scale: Scale,
    pub seed: u64,
    pub out: PathBuf,
}

const BASE_REPO: &str = "repos/configuration-base-saas.git";
const TENANT_REPO: &str = "repos/csp-tenant-data.git";
/// 2025-01-01T00:00:00Z. Commit times are fixed so ids do not depend on the clock.
const EPOCH: i64 = 1_735_689_600;
const DAY: i64 = 86_400;

/// Removes a previous output, but only if `out` is one (it holds a `manifest.json`) or is empty.
fn prepare_out(out: &Path) -> Result<(), FixtureError> {
    if out.exists() {
        let empty = std::fs::read_dir(out)?.next().is_none();
        if !empty && !out.join("manifest.json").is_file() {
            return Err(FixtureError::OutputNotEmpty(out.to_path_buf()));
        }
        std::fs::remove_dir_all(out)?;
    }
    std::fs::create_dir_all(out)?;
    Ok(())
}

/// Tenants and their file plans. Swimlane i (1-based) has tenant `sit<i>`; every other swimlane (up to
/// `two_tenant_swimlanes`) also has `sit<swimlanes + j>` (Q34: a swimlane has a set of tenants).
struct Tenants {
    /// The tenant set of each swimlane, in swimlane order.
    per_swimlane: Vec<Vec<String>>,
    plans: BTreeMap<String, TenantPlan>,
    /// Every tenant id, ordered by number.
    ids: Vec<String>,
}

fn plan_tenants(root: &Rng, p: &Params, docs: &[Doc]) -> Tenants {
    let mut per_swimlane: Vec<Vec<String>> = Vec::new();
    let mut plans: BTreeMap<String, TenantPlan> = BTreeMap::new();
    let mut second = 0;
    for i in 1..=p.swimlanes {
        let primary = format!("sit{i}");
        let has_second = i % 2 == 1 && second < p.two_tenant_swimlanes;
        let count = if has_second {
            p.tenant_files_primary_split
        } else {
            p.tenant_files_single
        };
        plans.insert(primary.clone(), tenant::plan(root, &primary, count, docs));
        let mut tenants = vec![primary];
        if has_second {
            second += 1;
            let secondary = format!("sit{}", p.swimlanes + second);
            plans.insert(
                secondary.clone(),
                tenant::plan(root, &secondary, p.tenant_files_secondary_split, docs),
            );
            tenants.push(secondary);
        }
        per_swimlane.push(tenants);
    }
    let mut ids: Vec<String> = plans.keys().cloned().collect();
    ids.sort_by_key(|t| t.trim_start_matches("sit").parse::<usize>().unwrap_or(usize::MAX));
    Tenants {
        per_swimlane,
        plans,
        ids,
    }
}

/// Planted items that follow from the catalog and the tenant plans (the NFS-side ones come from the snapshots).
fn plant_from_plans(planted: &mut Planted, catalog: &catalog::Catalog, tenants: &Tenants) {
    planted.duplicate_names = catalog
        .duplicate_names
        .iter()
        .map(|(name, folders)| DuplicateName {
            name: name.clone(),
            folders: folders.clone(),
        })
        .collect();
    planted.duplicate_keys = catalog
        .docs
        .iter()
        .filter(|d| !d.dup_keys.is_empty())
        .map(|d| DuplicateKeys {
            path: d.path.clone(),
            keys: d.dup_keys.clone(),
        })
        .collect();
    for t in &tenants.ids {
        for f in &tenants.plans[t].files {
            let doc = &catalog.docs[f.doc];
            match f.kind {
                ForkKind::Redundant => planted.redundant_copies.push(TenantPath {
                    tenant: t.clone(),
                    path: doc.path.clone(),
                }),
                ForkKind::Stale(v) => planted.stale_forks.push(StaleFork {
                    tenant: t.clone(),
                    path: doc.path.clone(),
                    fork_version: v,
                    missing: doc.missing_keys(v),
                }),
                ForkKind::Modified => {}
            }
        }
    }
}

/// Writes every swimlane snapshot and folds their numbers and planted items into `counts` and `planted`.
fn write_swimlanes(
    cfg: &Config,
    root: &Rng,
    docs: &[Doc],
    tenants: &Tenants,
    counts: &mut Counts,
    planted: &mut Planted,
) -> Result<Vec<Swimlane>, FixtureError> {
    let rule = PathMappingRule::default();
    let n = tenants.per_swimlane.len();
    let mut swimlanes = Vec::new();
    for (i, set) in tenants.per_swimlane.iter().enumerate() {
        let id = format!("sl-{:02}", i + 1);
        // The orphan file carries the suffix of a tenant that belongs to another swimlane.
        let foreign = &tenants.per_swimlane[(i + 7) % n][0];
        let foreign = if set.contains(foreign) {
            &tenants.per_swimlane[(i + 1) % n][0]
        } else {
            foreign
        };
        let spec = nfs::SwimlaneSpec {
            id: &id,
            tenants: set,
            foreign_tenant: foreign,
            lagging: if cfg.scale == Scale::Small {
                i % 2 == 1
            } else {
                i % 4 == 3
            },
            docs,
            plans: &tenants.plans,
        };
        let nfs_root = format!("nfs/{id}");
        let outcome = nfs::write_swimlane(&cfg.out.join(&nfs_root), &spec, root, &rule)?;
        counts.nfs_files += outcome.files;
        counts.max_file_bytes = counts.max_file_bytes.max(outcome.max_file_bytes);
        counts.max_file_lines = counts.max_file_lines.max(outcome.max_file_lines);
        counts.largest_yaml_lines = counts.largest_yaml_lines.max(outcome.largest_yaml_lines);
        counts.image_files += outcome.image_files;
        planted.nfs_ahead.extend(outcome.nfs_ahead);
        planted.git_ahead.extend(outcome.git_ahead);
        planted.untracked.extend(outcome.untracked);
        planted.orphans.extend(outcome.orphans);
        swimlanes.push(Swimlane {
            id,
            tenants: set.clone(),
            nfs_root,
            files: outcome.files,
            tree_sha256: outcome.tree_sha256,
        });
    }
    Ok(swimlanes)
}

/// Generates the fixtures and returns (and writes) the manifest.
///
/// # Errors
/// When the output directory is not a previous output, `git` is missing or fails, or a file cannot be written.
pub fn generate(cfg: &Config) -> Result<Manifest, FixtureError> {
    let p = cfg.scale.params();
    prepare_out(&cfg.out)?;
    let root = Rng::new(cfg.seed);

    let catalog = catalog::build(
        &mut root.fork("catalog"),
        &CatalogParams {
            base_files: p.base_files,
            adapters: p.adapters,
            dup_names: p.dup_names,
            big_yaml: p.big_yaml,
            huge_xsl_bytes: p.huge_xsl_bytes,
        },
    );
    let tenants = plan_tenants(&root, &p, &catalog.docs);

    let base_refs = write_base_repo(&cfg.out.join(BASE_REPO), &catalog.docs)?;
    let tenant_refs = write_tenant_repo(&cfg.out.join(TENANT_REPO), &catalog.docs, &tenants, &root, &p)?;

    let mut planted = Planted::default();
    let mut counts = Counts {
        swimlanes: p.swimlanes,
        tenant_branches: tenants.ids.len(),
        base_files: catalog.docs.len(),
        ..Counts::default()
    };
    let swimlanes = write_swimlanes(cfg, &root, &catalog.docs, &tenants, &mut counts, &mut planted)?;
    plant_from_plans(&mut planted, &catalog, &tenants);
    // The lists must not depend on the order files were written in.
    planted
        .nfs_ahead
        .sort_by(|a, b| (&a.swimlane, &a.path).cmp(&(&b.swimlane, &b.path)));
    planted
        .git_ahead
        .sort_by(|a, b| (&a.swimlane, &a.path).cmp(&(&b.swimlane, &b.path)));
    planted
        .untracked
        .sort_by(|a, b| (&a.swimlane, &a.path).cmp(&(&b.swimlane, &b.path)));
    planted
        .orphans
        .sort_by(|a, b| (&a.swimlane, &a.path).cmp(&(&b.swimlane, &b.path)));

    let manifest = Manifest {
        schema: 1,
        scale: cfg.scale.as_str().to_owned(),
        seed: cfg.seed,
        repos: Repos {
            base: Repo {
                path: BASE_REPO.to_owned(),
                default_branch: "master".to_owned(),
                refs: base_refs,
            },
            tenant: Repo {
                path: TENANT_REPO.to_owned(),
                default_branch: "sit1".to_owned(),
                refs: tenant_refs,
            },
        },
        paths: PathRoots {
            base_root: "config".to_owned(),
            tenant_root: "data/config".to_owned(),
        },
        channels: catalog.channels.clone(),
        channel_folders: catalog.channel_folders.clone(),
        tenants: tenants.ids,
        counts,
        swimlanes,
        planted,
    };
    let mut json =
        serde_json::to_string_pretty(&manifest).map_err(|e| FixtureError::Manifest(e.to_string()))?;
    json.push('\n');
    std::fs::write(cfg.out.join("manifest.json"), json)?;
    Ok(manifest)
}

/// Three commits on `master` (versions 1 to 3) with a tag on each.
fn write_base_repo(repo: &Path, docs: &[Doc]) -> Result<BTreeMap<String, String>, FixtureError> {
    let mut import = gitrepo::Import::init(repo, "master")?;
    for version in 1..=HEAD_VERSION {
        let mark = u32::from(version);
        import.begin_commit(
            "refs/heads/master",
            mark,
            EPOCH + i64::from(version - 1) * 30 * DAY,
            &format!("Base configuration v{version}.0.0\n"),
        )?;
        for d in docs {
            if d.born > version {
                continue;
            }
            // Only files that are new or changed since the previous version are written; the rest carry over.
            let now = d.render(version, None);
            if version > 1 && d.born < version && d.render(version - 1, None) == now {
                continue;
            }
            import.file(&format!("config/{}", d.path), &now)?;
        }
        import.end_commit()?;
        import.reset(&format!("refs/tags/v{version}.0.0"), mark)?;
    }
    import.finish()?;
    gitrepo::refs(repo)
}

const TEMPLATE_FILES: usize = 12;

fn write_tenant_repo(
    repo: &Path,
    docs: &[Doc],
    tenants: &Tenants,
    root: &Rng,
    p: &Params,
) -> Result<BTreeMap<String, String>, FixtureError> {
    let mut import = gitrepo::Import::init(repo, "sit1")?;
    let when = EPOCH + 90 * DAY;
    let mut mark = 1_u32;
    for t in &tenants.ids {
        import.begin_commit(&format!("refs/heads/{t}"), mark, when, &format!("Tenant {t}\n"))?;
        mark += 1;
        for f in &tenants.plans[t].files {
            let doc = &docs[f.doc];
            import.file(
                &format!("data/config/{}", doc.path),
                &tenant::render(doc, f.kind, t),
            )?;
        }
        import.end_commit()?;
    }

    // Core templates are never deployed; they carry the tenant layout for new tenants.
    let mut rng = root.fork("templates");
    let mut idx: Vec<usize> = (0..docs.len())
        .filter(|&i| docs[i].path != "channels.yml")
        .collect();
    rng.shuffle(&mut idx);
    idx.truncate(TEMPLATE_FILES.min(docs.len()));
    idx.sort_unstable();
    import.begin_commit("refs/heads/templates/core", mark, when, "Core template\n")?;
    mark += 1;
    for i in idx {
        import.file(
            &format!("data/config/{}", docs[i].path),
            &docs[i].render(HEAD_VERSION, Some("template")),
        )?;
    }
    import.end_commit()?;

    // Documentation lives on its own branch.
    import.begin_commit("refs/heads/templates/common-docs", mark, when, "Common docs\n")?;
    for (name, text) in doc_pages(p.docs_pages) {
        import.file(&format!("docs/{name}"), text.as_bytes())?;
    }
    import.end_commit()?;
    import.finish()?;
    gitrepo::refs(repo)
}

/// Markdown pages for the docs feature (prompt 12 indexes them): a runbook, an overview and a feature-flag table.
fn doc_pages(n: usize) -> Vec<(String, String)> {
    let pages = vec![
        (
            "overview.md".to_owned(),
            "# Overview\n\nSynthetic documentation for the fixture tenants. It describes no real system.\n".to_owned(),
        ),
        (
            "feature-flags.md".to_owned(),
            "# Feature Flags\n\n| Flag | File | Channel | Template default | Description |\n|---|---|---|---|---|\n\
             | enableAccountSorting | tx-infinity-api/tx-infinity-core.yml | remote-itm-teller | true | Sorts accounts in the teller UI. |\n"
                .to_owned(),
        ),
        (
            "runbook.md".to_owned(),
            "# Runbook\n\nRestart the config-server after adding a folder or changing channels.yml.\n".to_owned(),
        ),
    ];
    pages.into_iter().take(n.max(1)).collect()
}
