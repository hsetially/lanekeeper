//! The work behind the batch functions: the table of parsed blobs, one swimlane's index, and the computation of a group.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use domain::{ContentHash, FileClass, LogicalFile, MergeMode, NfsPath, TenantId};
use rayon::ThreadPool;
use rayon::prelude::*;

use super::{BatchRules, LaneFile, LaneHistory, LaneInput, LaneState};
use crate::bytes::decode_text;
use crate::checks::{BaseHistory, CheckFile, CheckInput, CheckResult, Structured, c5_drift, checks_c1_to_c8};
use crate::drift::{Drift, DriftInput, drift_state};
use crate::effective::{Layer, Layers, effective};
use crate::error::BatchError;
use crate::limits::Limits;
use crate::parse::{FlatSetting, ParseWarning, Syntax, flatten, parse_decoded};
use crate::resolution::{logical_file_role, merge_mode};
use crate::tree_hash::{normalised_hash, swimlane_hash};

/// The root property files, in the order of precedence (`.properties` beats `.yml` beats `.yaml`, at equal rank). Together they
/// are the lowest layer of every property source's effective hash: all of those that exist are merged setting by setting, as
/// `property_view` merges them. (`application-<tenant>.*` is a layer of the application's `property_view`; the batch's
/// per-file hash uses the three layers tenant, base and root.)
const ROOT_FILES: [&str; 3] = ["application.properties", "application.yml", "application.yaml"];

/// What identifies work on a blob: the same bytes are parsed once per syntax, and hashed once per way of normalising.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct BlobKey {
    blob: ContentHash,
    class: FileClass,
    /// Only for a structured file whose name says how to read it.
    syntax: Option<Syntax>,
}

/// A structured file that parsed: what the checks and the effective config read.
pub(super) struct ParsedFile {
    settings: Vec<FlatSetting>,
    warnings: Vec<ParseWarning>,
    warnings_dropped: usize,
}

pub(super) struct BlobFacts {
    /// `normalised_hash` of the content.
    hash: ContentHash,
    /// `None` for a file that is not structured, or does not parse (it is then compared by hash only).
    parsed: Option<ParsedFile>,
}

fn key_of(file: &LaneFile, blob: ContentHash) -> BlobKey {
    BlobKey {
        blob,
        class: file.class,
        syntax: (file.class == FileClass::Structured)
            .then(|| Syntax::of_path(&file.path))
            .flatten(),
    }
}

/// Adds the blob of `file` (if it is on NFS) to the work.
pub(super) fn collect_key<'a>(file: &'a LaneFile, into: &mut BTreeMap<BlobKey, &'a Bytes>) {
    if let Some(content) = &file.nfs {
        into.entry(key_of(file, content.blob)).or_insert(&content.bytes);
    }
}

fn parse_file(data: &[u8], syntax: Syntax, limits: &Limits) -> Option<ParsedFile> {
    let decoded = decode_text(data, limits).ok()?;
    let parsed = parse_decoded(&decoded.text, syntax, limits).ok()?;
    let flat = flatten(&parsed, limits).ok()?;
    Some(ParsedFile {
        settings: flat.settings,
        warnings: parsed.warnings,
        warnings_dropped: parsed.warnings_dropped,
    })
}

/// How many blobs were parsed in this process (tests only: a test of sharing counts parses, which the results do not show).
#[cfg(test)]
pub(super) static BLOBS_PARSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn facts_of(key: &BlobKey, data: &Bytes, limits: &Limits) -> BlobFacts {
    #[cfg(test)]
    if key.syntax.is_some() {
        BLOBS_PARSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    BlobFacts {
        hash: normalised_hash(data, key.class),
        parsed: key.syntax.and_then(|syntax| parse_file(data, syntax, limits)),
    }
}

/// The parsed form of the distinct blobs of a batch, for the duration of one call.
pub(super) struct Blobs(BTreeMap<BlobKey, BlobFacts>);

impl Blobs {
    /// Decodes, hashes and parses every blob once. With a pool, in parallel; the table is the same either way.
    pub(super) fn build(pool: Option<&ThreadPool>, keys: BTreeMap<BlobKey, &Bytes>, limits: &Limits) -> Self {
        let work: Vec<(BlobKey, &Bytes)> = keys.into_iter().collect();
        let facts: Vec<BlobFacts> = match pool {
            Some(pool) => pool.install(|| {
                work.par_iter()
                    .map(|(key, data)| facts_of(key, data, limits))
                    .collect()
            }),
            None => work
                .iter()
                .map(|(key, data)| facts_of(key, data, limits))
                .collect(),
        };
        Self(work.into_iter().map(|(key, _)| key).zip(facts).collect())
    }

    fn of(&self, file: &LaneFile) -> Option<&BlobFacts> {
        let content = file.nfs.as_ref()?;
        self.0.get(&key_of(file, content.blob))
    }
}

/// One swimlane's files in path order, and grouped by logical file.
pub(super) struct LaneIndex<'a> {
    files: Vec<&'a LaneFile>,
    groups: BTreeMap<&'a LogicalFile, Vec<&'a LaneFile>>,
}

impl<'a> LaneIndex<'a> {
    pub(super) fn new(input: &'a LaneInput<'a>) -> Result<Self, BatchError> {
        if input.files.len() > super::MAX_LANE_FILES {
            return Err(BatchError::TooManyFiles);
        }
        let mut files: Vec<&LaneFile> = input.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        if files.windows(2).any(|w| w[0].path == w[1].path) {
            return Err(BatchError::DuplicatePath);
        }
        let mut groups: BTreeMap<&LogicalFile, Vec<&LaneFile>> = BTreeMap::new();
        for file in &files {
            groups.entry(&file.facts.logical).or_default().push(file);
        }
        Ok(Self { files, groups })
    }

    fn get(&self, path: &NfsPath) -> Option<&'a LaneFile> {
        self.files
            .binary_search_by(|f| f.path.cmp(path))
            .ok()
            .map(|i| self.files[i])
    }

    /// Every file of the swimlane, in path order.
    pub(super) fn files(&self) -> &[&'a LaneFile] {
        &self.files
    }

    /// The root `application.*` files that are on NFS, strongest first. Together they are the lowest layer of the
    /// `spring_merge` files.
    fn roots(&self) -> Vec<&'a LaneFile> {
        ROOT_FILES
            .iter()
            .filter_map(|name| {
                let file = self.get(&NfsPath::parse(name).ok()?)?;
                (file.nfs.is_some() && file.facts.tenant.is_none()).then_some(file)
            })
            .collect()
    }
}

/// The index of every swimlane of a call, built before any blob is parsed: the sort that puts the files in path order is also
/// what finds a path that is there twice, and a call with such a swimlane is refused before it costs anything. The error is
/// the first in the order of `lanes`, whatever the schedule.
pub(super) fn index_lanes<'a>(
    pool: &ThreadPool,
    lanes: &'a [LaneInput<'a>],
) -> Result<Vec<LaneIndex<'a>>, BatchError> {
    super::first_error(pool.install(|| lanes.par_iter().map(LaneIndex::new).collect()))
}

/// True when a change to `path` could change which file is the root.
fn could_be_root(path: &NfsPath) -> bool {
    path.parent().is_root() && ROOT_FILES.contains(&path.file_name())
}

/// The root files with what the groups need from them: one layer, below the base file of every `spring_merge` group.
struct RootLayer<'a> {
    /// The root files that are on NFS, strongest first.
    paths: Vec<&'a NfsPath>,
    /// The hash of the strongest one. The layer is named by it; the effective hash is of the merged settings, so it is not part
    /// of that.
    hash: ContentHash,
    /// The settings of the root files, merged (a single file's own settings, as they are). `None` when a root file did not
    /// parse: the groups then fall back to the hash of the file that is served.
    settings: Option<Cow<'a, [FlatSetting]>>,
}

impl RootLayer<'_> {
    /// True for a root file: it is a file of its own, and the root layer is not below it.
    fn covers(&self, path: &NfsPath) -> bool {
        self.paths.contains(&path)
    }
}

/// The root layer of a swimlane, or `None` when it has no root file.
fn root_layer<'a>(roots: &[&'a LaneFile], blobs: &'a Blobs) -> Option<RootLayer<'a>> {
    struct Root<'a> {
        path: &'a NfsPath,
        hash: ContentHash,
        parsed: Option<&'a ParsedFile>,
    }
    let members: Vec<Root<'a>> = roots
        .iter()
        .map(|&file| {
            let facts = blobs.of(file);
            let hash = facts
                .map(|f| f.hash)
                .or_else(|| file.nfs.as_ref().map(|c| normalised_hash(&c.bytes, file.class)))?;
            Some(Root {
                path: &file.path,
                hash,
                parsed: facts.and_then(|f| f.parsed.as_ref()),
            })
        })
        .collect::<Option<_>>()?;
    let first = members.first()?;
    let settings = if let [only] = members.as_slice() {
        only.parsed.map(|p| Cow::Borrowed(p.settings.as_slice()))
    } else {
        // `.properties` over `.yml` over `.yaml`: the three slots of `Layers` are three layers, strongest first.
        let layers: Option<Vec<Layer<'_>>> = members
            .iter()
            .map(|m| {
                Some(Layer {
                    path: m.path,
                    hash: m.hash,
                    settings: &m.parsed?.settings,
                })
            })
            .collect();
        layers.and_then(|layers| {
            let mut layers = layers.into_iter();
            let merged = effective(
                MergeMode::SpringMerge,
                &Layers {
                    tenant: layers.next(),
                    base: layers.next(),
                    root: layers.next(),
                },
            )?;
            Some(Cow::Owned(
                merged
                    .settings
                    .into_iter()
                    .map(|v| FlatSetting {
                        path: v.path,
                        value: v.value,
                        lines: v.origin.lines,
                        via_alias: false,
                    })
                    .collect(),
            ))
        })
    };
    Some(RootLayer {
        paths: members.iter().map(|m| m.path).collect(),
        hash: first.hash,
        settings,
    })
}

/// What a group is computed from.
struct Ctx<'a> {
    input: &'a LaneInput<'a>,
    rules: &'a BatchRules<'a>,
    root: Option<RootLayer<'a>>,
}

impl<'a> Ctx<'a> {
    fn new(
        input: &'a LaneInput<'a>,
        rules: &'a BatchRules<'a>,
        blobs: &'a Blobs,
        index: &LaneIndex<'a>,
    ) -> Self {
        Self {
            input,
            rules,
            root: root_layer(&index.roots(), blobs),
        }
    }
}

/// A file of a group with what is computed from it.
struct Member<'a> {
    file: &'a LaneFile,
    facts: Option<&'a BlobFacts>,
    hash: Option<ContentHash>,
    drift: Drift,
}

impl<'a> Member<'a> {
    fn new(file: &'a LaneFile, blobs: &'a Blobs) -> Self {
        let facts = blobs.of(file);
        let hash = facts
            .map(|f| f.hash)
            .or_else(|| file.nfs.as_ref().map(|c| normalised_hash(&c.bytes, file.class)));
        let drift = drift_state(&DriftInput {
            baseline: file.baseline,
            nfs: hash,
            git: file.git,
            intentional: file.intentional,
        });
        Self {
            file,
            facts,
            hash,
            drift,
        }
    }

    fn state(&self) -> super::FileState {
        super::FileState {
            path: self.file.path.clone(),
            logical: self.file.facts.logical.clone(),
            class: self.file.class,
            hash: self.hash,
            drift: self.drift,
        }
    }

    fn parsed(&self) -> Option<&'a ParsedFile> {
        self.facts.and_then(|f| f.parsed.as_ref())
    }

    fn on_nfs(&self) -> bool {
        self.file.nfs.is_some()
    }
}

fn history_view(history: &LaneHistory) -> BaseHistory<'_> {
    BaseHistory {
        tenant_commit_time: history.tenant_commit_time,
        at_tenant_commit: history.at_tenant_commit.as_ref(),
        since: &history.since,
    }
}

/// The hash of the settings of a tenant that reads a `spring_merge` file: the layers merged. `None` when a layer that exists
/// did not parse; the caller then falls back to the hash of the file that is served.
fn spring_hash(
    tenant: Option<&Member<'_>>,
    base: Option<&Member<'_>>,
    root: Option<&RootLayer<'_>>,
) -> Option<ContentHash> {
    fn layer<'m>(member: &'m Member<'_>) -> Option<Layer<'m>> {
        Some(Layer {
            path: &member.file.path,
            hash: member.hash?,
            settings: &member.parsed()?.settings,
        })
    }
    // The root files are below the base file; a base file that is one of them is a file of its own, with no root layer.
    let root = root.filter(|r| base.is_none_or(|b| !r.covers(&b.file.path)));
    let tenant = match tenant {
        Some(member) => Some(layer(member)?),
        None => None,
    };
    let base = match base {
        Some(member) => Some(layer(member)?),
        None => None,
    };
    let root = match root {
        Some(r) => Some(Layer {
            path: r.paths.first()?,
            hash: r.hash,
            settings: r.settings.as_deref()?,
        }),
        None => None,
    };
    let layers = Layers { tenant, base, root };
    effective(MergeMode::SpringMerge, &layers).map(|e| e.hash)
}

/// The effective hash of a logical file in the swimlane: what each tenant reads, agreed (`swimlane_hash`).
fn group_effective(ctx: &Ctx<'_>, mode: MergeMode, members: &[Member<'_>]) -> Option<ContentHash> {
    let base = members
        .iter()
        .find(|m| m.on_nfs() && m.file.facts.tenant.is_none());
    let one = |tenant: Option<&Member<'_>>| -> Option<ContentHash> {
        let served = tenant.or(base)?;
        if mode == MergeMode::SpringMerge {
            if let Some(hash) = spring_hash(tenant, base, ctx.root.as_ref()) {
                return Some(hash);
            }
        }
        served.hash
    };
    let tenants = ctx.input.tenants;
    if tenants.is_empty() {
        return one(None);
    }
    let per_tenant: Vec<(TenantId, ContentHash)> = tenants
        .iter()
        .filter_map(|tenant| {
            let file = members
                .iter()
                .find(|m| m.on_nfs() && m.file.facts.tenant.as_ref() == Some(tenant));
            Some((tenant.clone(), one(file)?))
        })
        .collect();
    swimlane_hash(&per_tenant)
}

/// A group's file as the checks see it. `root` is the root layer, when the file's mode merges it.
fn view<'m>(
    m: &'m Member<'_>,
    hash: ContentHash,
    mode: MergeMode,
    root: Option<&'m RootLayer<'_>>,
) -> CheckFile<'m> {
    CheckFile {
        path: &m.file.path,
        facts: &m.file.facts,
        class: m.file.class,
        hash,
        merge_mode: mode,
        structured: m.parsed().map(|p| Structured {
            settings: &p.settings,
            parse_warnings: &p.warnings,
            parse_warnings_dropped: p.warnings_dropped,
        }),
        root_settings: root
            .filter(|r| !r.covers(&m.file.path))
            .and_then(|r| r.settings.as_deref()),
        drift: Some(m.drift),
        overwrite: m.file.overwrite,
        base_history: m.file.history.as_deref().map(history_view),
    }
}

/// The findings of one group: C1 to C8 on its files that are on NFS, and C5 for those that are not.
fn group_findings(ctx: &Ctx<'_>, mode: MergeMode, members: &[Member<'_>]) -> Vec<CheckResult> {
    let root = (mode == MergeMode::SpringMerge)
        .then_some(ctx.root.as_ref())
        .flatten();
    let present: Vec<CheckFile<'_>> = members
        .iter()
        .filter(|m| m.on_nfs())
        .filter_map(|m| Some(view(m, m.hash?, mode, root)))
        .collect();
    let mut findings = checks_c1_to_c8(&CheckInput {
        tenants: ctx.input.tenants,
        tier: ctx.input.tier,
        host_rules: ctx.rules.host_rules,
        files: &present,
        limits: ctx.rules.limits,
    });
    // A file that is not on NFS is drift (deleted on NFS, new in Git) and nothing else: it has no content for the other checks.
    // C5 reads the path and the drift result only, so the file's hash is a placeholder.
    let placeholder = ContentHash::from_bytes([0; 32]);
    let before = findings.len();
    for member in members.iter().filter(|m| !m.on_nfs()) {
        findings.extend(c5_drift(&view(member, placeholder, mode, None)));
    }
    if findings.len() != before {
        super::sort_findings(&mut findings);
    }
    findings
}

/// How a logical file merges: a property source merges setting by setting, a resource is chosen whole (D82).
fn mode_of(rules: &BatchRules<'_>, logical: &LogicalFile) -> MergeMode {
    merge_mode(logical_file_role(logical, rules.application_names))
}

/// Computes one group and puts the result in the state.
fn apply(
    state: &mut LaneState,
    ctx: &Ctx<'_>,
    logical: &LogicalFile,
    members: &[Member<'_>],
    want_hashes: bool,
) -> Result<(), BatchError> {
    let mode = mode_of(ctx.rules, logical);
    let effective = if want_hashes {
        group_effective(ctx, mode, members)
    } else {
        None
    };
    let findings = group_findings(ctx, mode, members);
    state.apply_group(logical, effective, findings)
}

/// One swimlane from scratch, from its index and the table of parsed blobs.
pub(super) fn compute_lane(
    input: &LaneInput<'_>,
    index: &LaneIndex<'_>,
    rules: &BatchRules<'_>,
    blobs: &Blobs,
    want_hashes: bool,
) -> Result<LaneState, BatchError> {
    let ctx = Ctx::new(input, rules, blobs, index);
    let mut state = LaneState::empty(input.swimlane.clone());
    for (logical, files) in &index.groups {
        let members: Vec<Member<'_>> = files.iter().map(|f| Member::new(f, blobs)).collect();
        for member in &members {
            state.files.insert(member.file.path.clone(), member.state());
        }
        apply(&mut state, &ctx, logical, &members, want_hashes)?;
    }
    state.tree.finish().map_err(|_| BatchError::InconsistentTree)?;
    Ok(state)
}

/// What an incremental update of one swimlane has decided before any blob is parsed: the index, the groups to recompute and the
/// blobs they read. The blobs of all the swimlanes of a call are then parsed together, once each.
pub(super) struct UpdatePlan<'a> {
    previous: &'a LaneState,
    input: &'a LaneInput<'a>,
    index: LaneIndex<'a>,
    paths: BTreeSet<&'a NfsPath>,
    dirty: BTreeSet<&'a LogicalFile>,
    keys: BTreeMap<BlobKey, &'a Bytes>,
}

impl<'a> UpdatePlan<'a> {
    /// The blobs that the update reads.
    pub(super) fn keys(&self) -> &BTreeMap<BlobKey, &'a Bytes> {
        &self.keys
    }
}

/// Which groups of a swimlane `paths` can have changed, and which blobs they read.
pub(super) fn plan_update<'a>(
    previous: &'a LaneState,
    input: &'a LaneInput<'a>,
    rules: &BatchRules<'_>,
    paths: BTreeSet<&'a NfsPath>,
) -> Result<UpdatePlan<'a>, BatchError> {
    let index = LaneIndex::new(input)?;

    // The groups to compute: those of the affected paths, before and after.
    let mut dirty: BTreeSet<&'a LogicalFile> = BTreeSet::new();
    for path in &paths {
        if let Some(old) = previous.files.get(*path) {
            dirty.insert(&old.logical);
        }
        if let Some(file) = index.get(path) {
            dirty.insert(&file.facts.logical);
        }
    }
    // The root files are a layer of every `spring_merge` group.
    if paths.iter().any(|p| could_be_root(p)) {
        for logical in index.groups.keys() {
            if mode_of(rules, logical) == MergeMode::SpringMerge {
                dirty.insert(logical);
            }
        }
        for old in previous.files.values() {
            if mode_of(rules, &old.logical) == MergeMode::SpringMerge {
                dirty.insert(&old.logical);
            }
        }
    }

    // The blobs of what is recomputed, and the root files'.
    let mut keys = BTreeMap::new();
    for logical in &dirty {
        for file in index.groups.get(*logical).into_iter().flatten() {
            collect_key(file, &mut keys);
        }
    }
    for root in index.roots() {
        collect_key(root, &mut keys);
    }
    Ok(UpdatePlan {
        previous,
        input,
        index,
        paths,
        dirty,
        keys,
    })
}

/// One swimlane brought up to date after `plan.paths` changed, from the table of parsed blobs.
pub(super) fn finish_update(
    plan: &UpdatePlan<'_>,
    rules: &BatchRules<'_>,
    blobs: &Blobs,
) -> Result<LaneState, BatchError> {
    let UpdatePlan {
        previous,
        input,
        index,
        paths,
        dirty,
        ..
    } = plan;
    let ctx = Ctx::new(input, rules, blobs, index);
    let mut state = (*previous).clone();
    // A path that is gone has no state; the ones that are still there get theirs below, with their group.
    for path in paths {
        if index.get(path).is_none() {
            state.files.remove(*path);
        }
    }
    for logical in dirty {
        let files: &[&LaneFile] = index.groups.get(*logical).map_or(&[], Vec::as_slice);
        let members: Vec<Member<'_>> = files.iter().map(|f| Member::new(f, blobs)).collect();
        for member in &members {
            state.files.insert(member.file.path.clone(), member.state());
        }
        apply(&mut state, &ctx, logical, &members, true)?;
    }
    state.tree.finish().map_err(|_| BatchError::InconsistentTree)?;
    Ok(state)
}
