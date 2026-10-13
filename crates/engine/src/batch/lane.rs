//! The work behind the batch functions: the table of parsed blobs, one swimlane's index, and the computation of a group.

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
use crate::parse::{FlatSetting, ParseWarning, Syntax, flatten, parse};
use crate::tree_hash::{normalised_hash, swimlane_hash};

/// The root property files, in the order that the interim model prefers them (`.properties` beats `.yml` at equal rank).
/// T10 replaces this with the config-server's resolution.
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
    let parsed = parse(&decoded.text, syntax, limits).ok()?;
    let flat = flatten(&parsed, limits).ok()?;
    Some(ParsedFile {
        settings: flat.settings,
        warnings: parsed.warnings,
        warnings_dropped: parsed.warnings_dropped,
    })
}

fn facts_of(key: &BlobKey, data: &Bytes, limits: &Limits) -> BlobFacts {
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

    /// The root `application.*` file that is the lowest layer of the `spring_merge` files.
    fn root(&self) -> Option<&'a LaneFile> {
        ROOT_FILES.iter().find_map(|name| {
            let file = self.get(&NfsPath::parse(name).ok()?)?;
            (file.nfs.is_some() && file.facts.tenant.is_none()).then_some(file)
        })
    }
}

/// True when a change to `path` could change which file is the root.
fn could_be_root(path: &NfsPath) -> bool {
    path.parent().is_root() && ROOT_FILES.contains(&path.file_name())
}

/// The root file with what the groups need from it.
#[derive(Clone, Copy)]
struct RootLayer<'a> {
    file: &'a LaneFile,
    hash: ContentHash,
    parsed: Option<&'a ParsedFile>,
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
        let root = index.root().and_then(|file| {
            let facts = blobs.of(file);
            let hash = facts
                .map(|f| f.hash)
                .or_else(|| file.nfs.as_ref().map(|c| normalised_hash(&c.bytes, file.class)))?;
            Some(RootLayer {
                file,
                hash,
                parsed: facts.and_then(|f| f.parsed.as_ref()),
            })
        });
        Self { input, rules, root }
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
    root: Option<RootLayer<'_>>,
) -> Option<ContentHash> {
    fn layer<'m>(member: &'m Member<'_>) -> Option<Layer<'m>> {
        Some(Layer {
            path: &member.file.path,
            hash: member.hash?,
            settings: &member.parsed()?.settings,
        })
    }
    // The root file is below the base file; when the base file is the root file it is the same layer, once.
    let root = root.filter(|r| base.is_none_or(|b| b.file.path != r.file.path));
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
            path: &r.file.path,
            hash: r.hash,
            settings: &r.parsed?.settings,
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
            if let Some(hash) = spring_hash(tenant, base, ctx.root) {
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

/// A group's file as the checks see it. `root_settings` is the root file's, when the file's mode merges it.
fn view<'m>(
    m: &'m Member<'_>,
    hash: ContentHash,
    mode: MergeMode,
    root_settings: Option<(&LaneFile, &'m ParsedFile)>,
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
        root_settings: root_settings
            .filter(|(root, _)| root.path != m.file.path)
            .map(|(_, p)| p.settings.as_slice()),
        drift: Some(m.drift),
        overwrite: m.file.overwrite,
        base_history: m.file.history.as_deref().map(history_view),
    }
}

/// The findings of one group: C1 to C8 on its files that are on NFS, and C5 for those that are not.
fn group_findings(ctx: &Ctx<'_>, mode: MergeMode, members: &[Member<'_>]) -> Vec<CheckResult> {
    let root_settings = (mode == MergeMode::SpringMerge)
        .then(|| ctx.root.and_then(|r| r.parsed.map(|p| (r.file, p))))
        .flatten();
    let present: Vec<CheckFile<'_>> = members
        .iter()
        .filter(|m| m.on_nfs())
        .filter_map(|m| Some(view(m, m.hash?, mode, root_settings)))
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

/// Computes one group and puts the result in the state.
fn apply(
    state: &mut LaneState,
    ctx: &Ctx<'_>,
    logical: &LogicalFile,
    members: &[Member<'_>],
    want_hashes: bool,
) -> Result<(), BatchError> {
    let mode = ctx.rules.merge_rules.mode_for(logical);
    let effective = if want_hashes {
        group_effective(ctx, mode, members)
    } else {
        None
    };
    let findings = group_findings(ctx, mode, members);
    state.apply_group(logical, effective, findings)
}

/// One swimlane from scratch.
pub(super) fn compute_lane(
    input: &LaneInput<'_>,
    rules: &BatchRules<'_>,
    blobs: &Blobs,
    want_hashes: bool,
) -> Result<LaneState, BatchError> {
    let index = LaneIndex::new(input)?;
    let ctx = Ctx::new(input, rules, blobs, &index);
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

/// One swimlane brought up to date after `paths` changed.
pub(super) fn update_lane(
    previous: &LaneState,
    input: &LaneInput<'_>,
    rules: &BatchRules<'_>,
    paths: &BTreeSet<&NfsPath>,
) -> Result<LaneState, BatchError> {
    let index = LaneIndex::new(input)?;

    // The groups to compute: those of the affected paths, before and after.
    let mut dirty: BTreeSet<&LogicalFile> = BTreeSet::new();
    for path in paths {
        if let Some(old) = previous.files.get(*path) {
            dirty.insert(&old.logical);
        }
        if let Some(file) = index.get(path) {
            dirty.insert(&file.facts.logical);
        }
    }
    // The root file is a layer of every `spring_merge` group.
    if paths.iter().any(|p| could_be_root(p)) {
        for logical in index.groups.keys() {
            if rules.merge_rules.mode_for(logical) == MergeMode::SpringMerge {
                dirty.insert(logical);
            }
        }
        for old in previous.files.values() {
            if rules.merge_rules.mode_for(&old.logical) == MergeMode::SpringMerge {
                dirty.insert(&old.logical);
            }
        }
    }

    // The blobs of what is recomputed, and the root file's.
    let mut keys = BTreeMap::new();
    for logical in &dirty {
        for file in index.groups.get(*logical).into_iter().flatten() {
            collect_key(file, &mut keys);
        }
    }
    if let Some(root) = index.root() {
        collect_key(root, &mut keys);
    }
    let blobs = Blobs::build(None, keys, rules.limits);

    let ctx = Ctx::new(input, rules, &blobs, &index);
    let mut state = previous.clone();
    // A path that is gone has no state; the ones that are still there get theirs below, with their group.
    for path in paths {
        if index.get(path).is_none() {
            state.files.remove(*path);
        }
    }
    for logical in dirty {
        let files: &[&LaneFile] = index.groups.get(logical).map_or(&[], Vec::as_slice);
        let members: Vec<Member<'_>> = files.iter().map(|f| Member::new(f, &blobs)).collect();
        for member in &members {
            state.files.insert(member.file.path.clone(), member.state());
        }
        apply(&mut state, &ctx, logical, &members, true)?;
    }
    state.tree.finish().map_err(|_| BatchError::InconsistentTree)?;
    Ok(state)
}
