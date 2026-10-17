//! Effective config (T6, D82, D83, A28, A30): what a service gets for a file, a tenant and a channel, from the engine, over setting
//! rows that are read once per blob and cached by hash.
//!
//! # Which files are the layers
//!
//! The same three the engine's lane computation uses (`engine::batch`, `spring_hash`), so a read and the pipeline agree:
//!
//! - **a property source** (`spring_merge`): the tenant's copy (`<name>-<tenant>.<ext>`), the base file, and below them the root
//!   `application.*` files, merged setting by setting;
//! - **a resource** (`whole_file`): the whole file, the tenant's copy or else the base copy, the channel folder's first when a channel
//!   is asked for. Its `hash` is the hash of the *rendered* file (`${KEY}` replaced where the application's property view has the key,
//!   `render_file`), and it has no settings (the domain type: "empty for resource files").
//!
//! A layer that does not parse falls back to the file as it is (the engine's own fallback: the hash of the file that is served).
//!
//! # A30: one tenant stands for a swimlane
//!
//! The port has no tenant, so the first tenant in sorted order stands for the swimlane in `effective`, `grid` and `compare`, and the
//! response names it. REST's `effective` takes the tenant. C3 and C4 use the whole set.
//!
//! # What is cached, and by what
//!
//! An [`EffectiveEntry`] is keyed by a hash of the layers it was made from (their paths, the hashes of their normalised content, the
//! mode), never by a swimlane, so 40 swimlanes that read the same files share one entry, and a changed file is a new key.
//!
//! # Resource rendering is an approximation of the config-server's, held to it every night
//!
//! The application whose property view renders a resource is the folder it is in, when that is one of the swimlane's applications
//! (D88 `SPRING_APPLICATION_NAME`), and `application` (the root files alone) otherwise. The search locations are every folder of the
//! swimlane except the channel folders `channels.yml` lists (D83; [`Inner::listed_channels`](super::service::Inner), read for every
//! request) and the requested channel's. The config-server keeps the folders and the channel list it had when it started, which this
//! does not (C11 says when a restart is due). [`crate::served`] compares the engine's rendering with what the config-server really
//! serves, every night, and raises a high-severity finding where they differ.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use domain::{
    AppName, ChannelName, ContentHash, EffectiveConfig, EffectiveSetting, EffectiveSource, FileClass,
    FileKind, FileRole, LogicalFile, MergeMode, NfsPath, SettingLocation, SettingPath, SwimlaneId, TenantId,
};
use engine::{
    DenyMatcher, Effective, FlatSetting, Layer, Layers, Syntax, effective, logical_file_role, merge_mode,
    resolve_resource,
};
use ports::ReadError;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tracing::warn;

use super::api::Tagged;
use super::cache::{BlobRows, EffectiveEntry};
use super::etag;
use super::rows;
use super::service::Inner;
use crate::error::RegistryError;
use crate::metrics::CacheName;

/// The root files of a swimlane: the lowest layer of every property source (the engine's `ROOT_FILES`).
pub const ROOT_FILES: [&str; 3] = ["application.properties", "application.yml", "application.yaml"];

/// A file row, as far as the layers need it.
#[derive(Debug, Clone)]
pub struct LayerRow {
    pub path: NfsPath,
    pub kind: FileKind,
    pub class: FileClass,
    /// The raw content, the key of the blob and of its setting rows.
    pub blob: Option<ContentHash>,
    /// The content with line endings normalised: the hash a layer is named by.
    pub norm: Option<ContentHash>,
    pub size: u64,
    pub denied: bool,
}

/// What one swimlane has for one logical file.
#[derive(Debug, Clone)]
pub struct Group {
    pub swimlane: SwimlaneId,
    /// Sorted.
    pub tenants: Vec<TenantId>,
    /// The applications its Deployments run (D88) that are named like the logical file (`app_candidates`), sorted, without repeats:
    /// what the file's role needs of them, and no more.
    pub apps: Vec<AppName>,
    /// The files of the logical file's group: the base file and the tenants' copies.
    pub files: Vec<LayerRow>,
    /// The root `application.*` files that are on NFS, strongest first.
    pub roots: Vec<LayerRow>,
}

/// The layers a tenant reads.
#[derive(Debug, Clone)]
pub struct Layering {
    pub role: FileRole,
    pub mode: MergeMode,
    pub tenant: Option<LayerRow>,
    pub base: Option<LayerRow>,
    /// Only for a property source, and not when the base file is itself a root file.
    pub roots: Vec<LayerRow>,
}

/// `$1` swimlanes, `$2` the logical file, `$3` the names of the root files: the rows of the group and the root files.
pub const GROUP_ROWS_SQL: &str = "\
SELECT swimlane_id, path, kind, class, nfs_hash::bytea AS blob, nfs_drift_hash::bytea AS norm, COALESCE(size, 0) AS size, denied, (logical_file = $2) AS in_group \
FROM file_state \
WHERE swimlane_id = ANY($1::text[]) AND (logical_file = $2 OR path = ANY($3::text[])) AND nfs_hash IS NOT NULL";

pub const TENANTS_SQL: &str = "\
SELECT swimlane_id, array_agg(tenant_id ORDER BY tenant_id) AS tenants FROM swimlane_tenants \
WHERE swimlane_id = ANY($1::text[]) GROUP BY swimlane_id";

/// `$1` swimlanes, `$2` the names to look for: the applications of those swimlanes that have one of these names (D88). A swimlane runs
/// hundreds of services, each with a name of its own, and what a logical file needs of them is only whether one is named like the file
/// (see [`app_candidates`]), so the rows that come back are those few and not every service.
pub const APPS_SQL: &str = "\
SELECT swimlane_id, env_values ->> 'SPRING_APPLICATION_NAME' AS app FROM services \
WHERE swimlane_id = ANY($1::text[]) AND removed_at IS NULL AND env_values ->> 'SPRING_APPLICATION_NAME' = ANY($2::text[]) LIMIT 100000";

/// The application names that can change what is read of a logical file, which is the only thing a group's applications are used for
/// (D88): the engine asks whether a prefix of the file's *name*, cut before a `.` or a `-` (the stem, or an application with a tenant
/// suffix), or the whole name, is an application (`is_property_source`), and the view that renders a resource is the application named
/// like a *folder* of the path (`resource::application_of`). Every such name is a candidate, more than will be asked about: a name that
/// is not asked about changes nothing. Tests hold the engine's rule and `application_of` to this list.
pub fn app_candidates(file: &LogicalFile) -> Vec<String> {
    let path = file.as_str();
    let name = file.as_path().file_name();
    let folders = path.rsplit_once('/').map_or("", |(folders, _)| folders);
    let mut out: Vec<String> = name
        .match_indices(['.', '-'])
        .map(|(i, _)| name[..i].to_owned())
        .chain(folders.split('/').map(str::to_owned))
        .filter(|n| !n.is_empty())
        .collect();
    out.push(name.to_owned());
    out.sort();
    out.dedup();
    out
}

pub const EXISTING_SQL: &str = "SELECT id FROM swimlanes WHERE id = ANY($1::text[])";

/// A file row as a layer. `denied` is [`crate::denied::is_denied`]: the flag, the class and the hub's own globs, decided here once for
/// everything that reads a file through a layer (the comparisons, the effective config, the resources, the grid), and the class says
/// `denied` when it is.
pub(crate) fn layer_row(row: &sqlx::postgres::PgRow, deny: &DenyMatcher) -> Result<LayerRow, RegistryError> {
    let path: String = row.get("path");
    let kind: String = row.get("kind");
    let class: String = row.get("class");
    let blob: Option<Vec<u8>> = row.get("blob");
    let norm: Option<Vec<u8>> = row.get("norm");
    let path = NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?;
    let class = rows::class(&class)?;
    let denied = crate::denied::is_denied(deny, &path, row.get("denied"), class);
    Ok(LayerRow {
        kind: rows::kind(&kind)?,
        class: if denied { FileClass::Denied } else { class },
        blob: rows::opt_hash(blob.as_deref())?,
        norm: rows::opt_hash(norm.as_deref())?,
        size: u64::try_from(row.get::<i64, _>("size")).unwrap_or(0),
        denied,
        path,
    })
}

/// The tenants and the application names that the two statements found, on the groups they belong to.
fn tenants_and_apps(
    out: &mut HashMap<SwimlaneId, Group>,
    tenant_rows: &[sqlx::postgres::PgRow],
    app_rows: &[sqlx::postgres::PgRow],
) {
    for row in tenant_rows {
        let id: String = row.get("swimlane_id");
        let tenants: Vec<String> = row.get("tenants");
        if let Some(g) = SwimlaneId::parse(&id).ok().and_then(|s| out.get_mut(&s)) {
            g.tenants = tenants.iter().filter_map(|t| TenantId::parse(t).ok()).collect();
        }
    }
    for row in app_rows {
        let id: String = row.get("swimlane_id");
        let app: Option<String> = row.get("app");
        if let (Some(g), Some(app)) = (SwimlaneId::parse(&id).ok().and_then(|s| out.get_mut(&s)), app) {
            if let Ok(app) = AppName::parse(&app) {
                g.apps.push(app);
            }
        }
    }
    for g in out.values_mut() {
        g.apps.sort();
        g.apps.dedup();
    }
}

/// Whether `path` is the copy of `file` that `tenant` reads (`<name>-<tenant>.<ext>`), by the engine's own rule for choosing one.
pub(crate) fn is_tenant_copy(file: &LogicalFile, tenant: &TenantId, path: &NfsPath) -> bool {
    let only: BTreeSet<NfsPath> = std::iter::once(path.clone()).collect();
    resolve_resource(file, Some(tenant), None, &only).is_some_and(|c| c.tenant_copy && c.path == *path)
}

/// The layers `tenant` reads out of what the swimlane has.
pub fn layering(group: &Group, file: &LogicalFile, tenant: &TenantId) -> Layering {
    let role = logical_file_role(file, &group.apps);
    let mode = merge_mode(role);
    // The tenant's copy is found by its name, whatever the row's `kind` says: the pipeline sets `kind` from what Git has, so a tenant file
    // that Git does not have is `untracked`, and it is still the file that tenant reads.
    let tenant_layer = group
        .files
        .iter()
        .find(|r| r.path != *file.as_path() && is_tenant_copy(file, tenant, &r.path))
        .cloned();
    let base = group.files.iter().find(|r| r.path == *file.as_path()).cloned();
    // The root files are below the base file of a property source, and not below a root file itself (it is a file of its own).
    let base_is_root = base
        .as_ref()
        .is_some_and(|b| group.roots.iter().any(|r| r.path == b.path));
    let roots = if mode == MergeMode::SpringMerge && !base_is_root {
        group.roots.clone()
    } else {
        Vec::new()
    };
    Layering {
        role,
        mode,
        tenant: tenant_layer,
        base,
        roots,
    }
}

/// A hash of the layers an effective config is made of: the mode, and each layer's path and hash. The key of the cache and the
/// material of the entity tag.
pub fn key_of(l: &Layering) -> ContentHash {
    let mut h = Sha256::new();
    h.update(b"lanekeeper/effective-key/v1\n");
    h.update([u8::from(l.mode == MergeMode::SpringMerge)]);
    let mut layer = |tag: &[u8], row: Option<&LayerRow>| {
        h.update(tag);
        if let Some(row) = row {
            h.update((row.path.as_str().len() as u64).to_be_bytes());
            h.update(row.path.as_str().as_bytes());
            h.update(row.norm.map_or([0_u8; 32], |n| *n.as_bytes()));
            h.update(row.blob.map_or([0_u8; 32], |n| *n.as_bytes()));
        }
    };
    layer(b"t", l.tenant.as_ref());
    layer(b"b", l.base.as_ref());
    for root in &l.roots {
        layer(b"r", Some(root));
    }
    ContentHash::from_bytes(h.finalize().into())
}

/// The blobs whose rows a layering reads, each with how to read it.
fn wanted(l: &Layering) -> Vec<(ContentHash, Syntax)> {
    l.tenant
        .iter()
        .chain(l.base.iter())
        .chain(l.roots.iter())
        .filter(|r| r.class == FileClass::Structured && !r.denied)
        .filter_map(|r| Some((r.blob?, Syntax::of_path(&r.path)?)))
        .collect()
}

/// The merged settings of the root files, as one layer: a single file's own settings, or several merged with the stronger one on top
/// (`.properties` over `.yml` over `.yaml`), the way the engine's lane computation does.
fn merged_roots<S: std::hash::BuildHasher>(
    roots: &[LayerRow],
    rows: &HashMap<ContentHash, Arc<BlobRows>, S>,
) -> Option<(NfsPath, ContentHash, Vec<FlatSetting>)> {
    let first = roots.first()?;
    let settings_of = |r: &LayerRow| -> Option<&[FlatSetting]> {
        let rows = rows.get(&r.blob?)?;
        (!rows.unparseable).then_some(rows.settings.as_slice())
    };
    let hash_of = |r: &LayerRow| r.norm;
    let settings = if let [only] = roots {
        settings_of(only)?.to_vec()
    } else {
        let layers: Option<Vec<Layer<'_>>> = roots
            .iter()
            .map(|r| {
                Some(Layer {
                    path: &r.path,
                    hash: hash_of(r)?,
                    settings: settings_of(r)?,
                })
            })
            .collect();
        let mut layers = layers?.into_iter();
        let merged = effective(
            MergeMode::SpringMerge,
            &Layers {
                tenant: layers.next(),
                base: layers.next(),
                root: layers.next(),
            },
        )?;
        merged
            .settings
            .into_iter()
            .map(|v| FlatSetting {
                path: v.path,
                value: v.value,
                lines: v.origin.lines,
                via_alias: false,
            })
            .collect()
    };
    Some((first.path.clone(), hash_of(first)?, settings))
}

/// A layer with its settings in hand.
struct Held<'a> {
    row: &'a LayerRow,
    settings: Vec<FlatSetting>,
    /// False for a structured file whose rows say it does not parse (or that has none): it is compared by hash only.
    parsed: bool,
}

impl Held<'_> {
    fn layer(&self) -> Option<Layer<'_>> {
        Some(Layer {
            path: &self.row.path,
            hash: self.row.norm?,
            settings: &self.settings,
        })
    }
}

fn held<'a, S: std::hash::BuildHasher>(
    row: &'a LayerRow,
    rows: &HashMap<ContentHash, Arc<BlobRows>, S>,
) -> Held<'a> {
    if row.class != FileClass::Structured {
        return Held {
            row,
            settings: Vec::new(),
            parsed: true,
        };
    }
    match row.blob.and_then(|b| rows.get(&b)) {
        Some(r) if !r.unparseable => Held {
            row,
            settings: r.settings.clone(),
            parsed: true,
        },
        _ => Held {
            row,
            settings: Vec::new(),
            parsed: false,
        },
    }
}

/// The effective config of a layering. `None` when there is no layer at all. Pure: the rows are given.
pub fn compute<S: std::hash::BuildHasher>(
    l: &Layering,
    rows: &HashMap<ContentHash, Arc<BlobRows>, S>,
) -> Option<Effective> {
    let tenant = l.tenant.as_ref().map(|r| held(r, rows));
    let base = l.base.as_ref().map(|r| held(r, rows));
    let parsed = tenant.as_ref().is_none_or(|t| t.parsed) && base.as_ref().is_none_or(|b| b.parsed);
    if l.mode == MergeMode::SpringMerge && parsed {
        let root = merged_roots(&l.roots, rows);
        let root_layer = root.as_ref().map(|(path, hash, settings)| Layer {
            path,
            hash: *hash,
            settings,
        });
        // A root file that is on NFS but does not parse: the engine falls back to the file that is served, and so does this.
        if l.roots.is_empty() || root_layer.is_some() {
            let layers = Layers {
                tenant: tenant.as_ref().and_then(Held::layer),
                base: base.as_ref().and_then(Held::layer),
                root: root_layer,
            };
            if let Some(e) = effective(MergeMode::SpringMerge, &layers) {
                return Some(e);
            }
        }
    }
    effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: tenant.as_ref().and_then(Held::layer),
            base: base.as_ref().and_then(Held::layer),
            root: None,
        },
    )
}

/// The settings of an effective config in the contract's shape. Values the scan flagged are already redaction markers (A28).
fn settings_of(e: &Effective) -> Vec<EffectiveSetting> {
    e.settings
        .iter()
        .map(|v| EffectiveSetting {
            path: v.path.clone(),
            value: v.value.text.clone(),
            source: v.origin.file.clone(),
            location: Some(SettingLocation {
                path: v.path.clone(),
                lines: v.origin.lines,
            }),
        })
        .collect()
}

/// Whether a setting is at or below `prefix`: its path is the prefix, or continues it after a `.` or a `[` (the port's
/// `EffectiveQuery::setting_prefix`). So `db` selects `db.host` and `db[0]` and not `dbx.y`.
pub fn setting_is_below(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('['))
}

/// `config` with only the settings at or below `prefix`, when there is one. Nothing else of the answer changes: its hash, sources and
/// placeholders are those of the whole file (the cut is of the list the caller reads, never of what the file is).
pub fn restrict_to_prefix(mut config: EffectiveConfig, prefix: Option<&SettingPath>) -> EffectiveConfig {
    if let Some(prefix) = prefix {
        config
            .settings
            .retain(|s| setting_is_below(s.path.as_str(), prefix.as_str()));
    }
    config
}

fn weight_of(e: &Effective) -> u32 {
    let bytes: usize = e
        .settings
        .iter()
        .map(|s| 200 + s.path.as_str().len() + s.value.text.len() + s.origin.file.as_str().len())
        .sum::<usize>()
        + 256;
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

impl Inner {
    /// The swimlanes of `ids` that exist.
    pub async fn existing(&self, ids: &[&str]) -> Result<BTreeSet<String>, ReadError> {
        let found: Vec<String> = self
            .db
            .run(
                "read_existing",
                sqlx::query_scalar(EXISTING_SQL)
                    .bind(ids)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(found.into_iter().collect())
    }

    /// What each of these swimlanes has for `file`. A swimlane that does not exist is not in the answer.
    pub async fn groups_of(
        &self,
        swimlanes: &[SwimlaneId],
        file: &LogicalFile,
    ) -> Result<HashMap<SwimlaneId, Group>, ReadError> {
        let ids: Vec<&str> = swimlanes.iter().map(SwimlaneId::as_str).collect();
        let existing = self.existing(&ids).await?;
        if existing.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<&str> = existing.iter().map(String::as_str).collect();
        let roots: Vec<&str> = ROOT_FILES.to_vec();
        let candidates = app_candidates(file);
        let (file_rows, tenant_rows, app_rows) = tokio::try_join!(
            async {
                self.db
                    .run(
                        "read_group_rows",
                        sqlx::query(GROUP_ROWS_SQL)
                            .bind(&ids)
                            .bind(file.as_str())
                            .bind(&roots)
                            .fetch_all(self.db.pool()),
                    )
                    .await
                    .map_err(ReadError::from)
            },
            async {
                self.db
                    .run(
                        "read_group_tenants",
                        sqlx::query(TENANTS_SQL).bind(&ids).fetch_all(self.db.pool()),
                    )
                    .await
                    .map_err(ReadError::from)
            },
            async {
                self.db
                    .run(
                        "read_group_apps",
                        sqlx::query(APPS_SQL)
                            .bind(&ids)
                            .bind(&candidates)
                            .fetch_all(self.db.pool()),
                    )
                    .await
                    .map_err(ReadError::from)
            },
        )?;

        let mut out: HashMap<SwimlaneId, Group> = HashMap::with_capacity(ids.len());
        for id in &existing {
            let swimlane = SwimlaneId::parse(id).map_err(|_| ReadError::from(RegistryError::Corrupt))?;
            out.insert(
                swimlane.clone(),
                Group {
                    swimlane,
                    tenants: Vec::new(),
                    apps: Vec::new(),
                    files: Vec::new(),
                    roots: Vec::new(),
                },
            );
        }
        tenants_and_apps(&mut out, &tenant_rows, &app_rows);
        for row in &file_rows {
            let id: String = row.get("swimlane_id");
            let in_group: bool = row.get("in_group");
            let layer = layer_row(row, &self.deny)?;
            let Some(g) = SwimlaneId::parse(&id).ok().and_then(|s| out.get_mut(&s)) else {
                continue;
            };
            let is_root = layer.path.parent().is_root() && ROOT_FILES.contains(&layer.path.as_str());
            if is_root {
                g.roots.push(layer.clone());
            }
            if in_group {
                g.files.push(layer);
            }
        }
        for g in out.values_mut() {
            // Strongest first: `.properties` over `.yml` over `.yaml`.
            g.roots.sort_by_key(|r| {
                ROOT_FILES
                    .iter()
                    .position(|n| *n == r.path.as_str())
                    .unwrap_or(usize::MAX)
            });
        }
        Ok(out)
    }

    /// The effective configs of these (group, tenant) pairs, computed together: one statement for the setting rows of every blob they
    /// read and one pass of the read pool over the merges that are not cached. `None` for a pair that has no layer.
    pub async fn effectives_of(
        &self,
        jobs: &[(&Group, &LogicalFile, &TenantId)],
    ) -> Result<Vec<Option<(Layering, Arc<EffectiveEntry>)>>, ReadError> {
        let layerings: Vec<Layering> = jobs.iter().map(|(g, f, t)| layering(g, f, t)).collect();
        let keys: Vec<ContentHash> = layerings.iter().map(key_of).collect();

        let mut hits: Vec<Option<Arc<EffectiveEntry>>> = Vec::with_capacity(jobs.len());
        let mut want_rows: Vec<(ContentHash, Syntax)> = Vec::new();
        for (l, key) in layerings.iter().zip(&keys) {
            let hit = self.caches.effective.get(key);
            self.caches.count(CacheName::Effective, hit.is_some());
            if hit.is_none() && (l.tenant.is_some() || l.base.is_some()) {
                want_rows.extend(wanted(l));
            }
            hits.push(hit);
        }
        let rows = if want_rows.is_empty() {
            HashMap::new()
        } else {
            self.rows_of(&want_rows).await?
        };

        // The merges that are not cached, on the read pool (rule 4), in parallel; the answer does not depend on the pool.
        // Swimlanes that have the same layers have the same key and so the same effective config: it is made once and shared (in a fleet,
        // most swimlanes run the same files).
        let mut todo: Vec<usize> = Vec::new();
        let mut first_of: HashMap<ContentHash, usize> = HashMap::new();
        let mut same_as: Vec<(usize, usize)> = Vec::new();
        for i in 0..jobs.len() {
            if hits[i].is_some() || (layerings[i].tenant.is_none() && layerings[i].base.is_none()) {
                continue;
            }
            match first_of.entry(keys[i]) {
                std::collections::hash_map::Entry::Occupied(first) => same_as.push((i, *first.get())),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(i);
                    todo.push(i);
                }
            }
        }
        if !todo.is_empty() {
            let (pool, work) = (
                Arc::clone(&self.pool),
                todo.iter().map(|i| layerings[*i].clone()).collect::<Vec<_>>(),
            );
            let computed = tokio::task::spawn_blocking(move || {
                use rayon::prelude::*;
                pool.install(|| work.par_iter().map(|l| compute(l, &rows)).collect::<Vec<_>>())
            })
            .await
            .map_err(|_| {
                warn!("a merge task on the blocking pool did not finish");
                ReadError::Unavailable
            })?;
            for (i, e) in todo.into_iter().zip(computed) {
                if let Some(e) = e {
                    let entry = Arc::new(EffectiveEntry {
                        weight: weight_of(&e),
                        effective: e,
                    });
                    self.caches.effective.insert(keys[i], Arc::clone(&entry));
                    hits[i] = Some(entry);
                }
            }
            for (i, first) in same_as {
                let shared = hits[first].clone();
                hits[i] = shared;
            }
        }
        Ok(layerings
            .into_iter()
            .zip(hits)
            .map(|(l, hit)| hit.map(|h| (l, h)))
            .collect())
    }

    /// The first tenant, in sorted order, that stands for a swimlane (A30).
    pub fn standing_tenant(group: &Group) -> Result<TenantId, ReadError> {
        crate::tenants::standing(&group.tenants)
            .cloned()
            .ok_or(ReadError::NotFound)
    }

    /// `getEffectiveConfig`.
    pub async fn effective_config(
        &self,
        swimlane: &SwimlaneId,
        file: &LogicalFile,
        tenant: Option<&TenantId>,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<EffectiveConfig>, ReadError> {
        let groups = self.groups_of(std::slice::from_ref(swimlane), file).await?;
        let group = groups.get(swimlane).ok_or(ReadError::NotFound)?;
        let tenant = match tenant {
            Some(t) if group.tenants.contains(t) => t.clone(),
            Some(_) => return Err(ReadError::NotFound),
            None => Self::standing_tenant(group)?,
        };
        let layering = layering(group, file, &tenant);
        if layering.tenant.is_none() && layering.base.is_none() && layering.role == FileRole::PropertySource {
            return Err(ReadError::NotFound);
        }
        if [&layering.tenant, &layering.base]
            .into_iter()
            .flatten()
            .any(|r| r.denied || r.class == FileClass::Denied)
        {
            // A denied file has no effective config (D79).
            return Err(ReadError::NotFound);
        }
        if layering.role == FileRole::Resource {
            return self
                .effective_resource(group, file, &tenant, channel, if_none_match)
                .await;
        }
        let key = key_of(&layering);
        let tag = etag::tag(
            "effective",
            &[
                swimlane.as_str(),
                file.as_str(),
                tenant.as_str(),
                channel.map_or("", ChannelName::as_str),
                &key.to_string(),
            ],
        );
        if etag::matches(if_none_match, &tag) {
            return Ok(Tagged::NotModified { etag: tag });
        }
        let computed = self.effectives_of(&[(group, file, &tenant)]).await?;
        let Some((_, entry)) = computed.into_iter().next().flatten() else {
            return Err(ReadError::NotFound);
        };
        let e = &entry.effective;
        let value = EffectiveConfig {
            swimlane: swimlane.clone(),
            file: file.clone(),
            tenant,
            channel: channel.cloned(),
            role: layering.role,
            hash: e.hash,
            sources: e
                .sources
                .iter()
                .map(|s| EffectiveSource {
                    path: s.path.clone(),
                    rank: s.rank,
                })
                .collect(),
            settings: settings_of(e),
            unresolved_placeholders: Vec::new(),
        };
        Ok(Tagged::Fresh { etag: tag, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_below_a_prefix_when_it_is_the_prefix_or_continues_it_after_a_dot_or_a_bracket() {
        for (path, prefix, below) in [
            ("db", "db", true),
            ("db.host", "db", true),
            ("db.host.port", "db", true),
            ("db[0]", "db", true),
            ("db[0].name", "db[0]", true),
            ("db.host", "db.host", true),
            ("dbx.y", "db", false),
            ("db", "db.host", false),
            ("listing", "list", false),
            ("a", "", false),
            ("", "a", false),
            ("DB.host", "db", false),
        ] {
            assert_eq!(setting_is_below(path, prefix), below, "{path} below {prefix}");
        }
    }

    #[test]
    fn a_prefix_cuts_only_the_settings() {
        let setting = |p: &str| EffectiveSetting {
            path: SettingPath::parse(p).unwrap(),
            value: "v".to_owned(),
            source: NfsPath::parse("core.yml").unwrap(),
            location: None,
        };
        let config = EffectiveConfig {
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            file: LogicalFile::parse("core.yml").unwrap(),
            tenant: TenantId::parse("sit1").unwrap(),
            channel: None,
            role: FileRole::PropertySource,
            hash: ContentHash::from_bytes([7; 32]),
            sources: Vec::new(),
            settings: vec![setting("a.b"), setting("a.c"), setting("ab")],
            unresolved_placeholders: Vec::new(),
        };
        let all = restrict_to_prefix(config.clone(), None);
        assert_eq!(all, config, "no prefix, no cut");
        let cut = restrict_to_prefix(config.clone(), Some(&SettingPath::parse("a").unwrap()));
        assert_eq!(cut.settings, vec![setting("a.b"), setting("a.c")]);
        assert_eq!(
            EffectiveConfig {
                settings: config.settings.clone(),
                ..cut
            },
            config,
            "nothing but the settings differs"
        );
    }
}
