//! T8 (P5, P6, D22): the batch functions. `recompute_all` and `recompute_paths` agree with each other and with the single
//! functions, `findings_for_swimlanes` equals the swimlanes one by one, and the adoption ranking finds the planted branch.
//!
//! The fixtures are read the way the hub reads a swimlane (`support::lanes`); the small hand-made lanes show the cases that the
//! fixtures do not have (a file missing on NFS, the root file, history for C1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use domain::{
    AppName, BranchKind, CommitId, ContentHash, DriftState, FileClass, FindingKind, GitRef, LogicalFile,
    NfsPath, PathMappingRule, SwimlaneId, TenantId, TenantSet, Timestamp,
};
use engine::{
    AdoptionCandidate, BaseVersion, Baseline, BatchError, BatchRules, CheckFile, CheckInput, CheckResult,
    DenyMatcher, DriftInput, HostRules, LaneFile, LaneHistory, LaneInput, LaneState, Limits, NfsContent,
    Structured, Syntax, adoption_rank, checks_c1_to_c8, classify, classify_path, decode_text, drift_hash,
    drift_state, effective_tree_hash, findings_for_swimlanes, flatten, logical_file_role, merge_mode,
    normalised_hash, parse, recompute_all, recompute_paths, swimlane_hash,
};
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use support::lanes::{LaneData, load_branches, load_lanes};
use support::small_fixtures;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn logical(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

fn sha(data: &[u8]) -> ContentHash {
    ContentHash::from_bytes(Sha256::digest(data).into())
}

/// The rules of a test: owned, lent as `BatchRules`.
struct Rules {
    /// The applications: a file named after one is a property source (`spring_merge`), any other file is a resource.
    apps: Vec<AppName>,
    hosts: HostRules,
    limits: Limits,
}

impl Rules {
    fn whole_file() -> Self {
        Self {
            apps: Vec::new(),
            hosts: HostRules::default(),
            limits: Limits::default(),
        }
    }

    fn spring_for_yml() -> Self {
        Self {
            apps: ["app", "a", "b", "c", "d", "e", "x", "gone", "zz", "new-file"]
                .iter()
                .map(|n| AppName::parse(n).unwrap())
                .collect(),
            ..Self::whole_file()
        }
    }

    fn lend(&self) -> BatchRules<'_> {
        BatchRules {
            application_names: &self.apps,
            host_rules: &self.hosts,
            limits: &self.limits,
        }
    }
}

fn fixture_lanes() -> &'static Vec<LaneData> {
    static CELL: OnceLock<Vec<LaneData>> = OnceLock::new();
    CELL.get_or_init(|| load_lanes(&small_fixtures().root, usize::MAX))
}

fn inputs(lanes: &[LaneData]) -> Vec<LaneInput<'_>> {
    lanes.iter().map(LaneData::input).collect()
}

fn by_id<'a>(states: &'a [LaneState], id: &str) -> &'a LaneState {
    states.iter().find(|s| s.swimlane().as_str() == id).unwrap()
}

// --- Hand-made lanes -------------------------------------------------------------------------------------------------

/// Builds `LaneFile`s the way the loader does, for a swimlane with some tenants.
struct Synth {
    rule: PathMappingRule,
    tenants: TenantSet,
    all: TenantSet,
    id: SwimlaneId,
}

impl Synth {
    fn new(tenants: &[&str]) -> Self {
        let set = |names: &[&str]| TenantSet::new(names.iter().map(|n| TenantId::parse(n).unwrap())).unwrap();
        Self {
            rule: PathMappingRule::default(),
            tenants: set(tenants),
            all: set(&["sit1", "sit2", "sit3", "sit9"]),
            id: SwimlaneId::parse("sl-x").unwrap(),
        }
    }

    /// A path with its content on NFS (`None`: missing there) and in Git (`None`: not in Git). The baseline is Git's content.
    fn file(&self, path: &str, nfs_text: Option<&str>, git_text: Option<&str>) -> LaneFile {
        let path = nfs(path);
        let data = nfs_text.map(|t| Bytes::from(t.to_owned()));
        let class = classify(&path, data.as_deref(), &DenyMatcher::default());
        let facts = classify_path(&self.rule, &path, &self.all, |_| git_text.is_some()).unwrap();
        let git = git_text.map(|t| drift_hash(Some(&Bytes::from(t.to_owned())), class).unwrap());
        LaneFile {
            path,
            facts,
            class,
            nfs: data.map(|bytes| NfsContent {
                blob: sha(&bytes),
                bytes,
            }),
            git,
            baseline: git.map_or(Baseline::NotRecorded, |g| Baseline::Recorded(Some(g))),
            intentional: None,
            overwrite: None,
            history: None,
        }
    }

    fn input<'a>(&'a self, files: &'a [LaneFile]) -> LaneInput<'a> {
        LaneInput {
            swimlane: &self.id,
            tenants: &self.tenants,
            tier: None,
            files,
        }
    }
}

fn one(state: Vec<LaneState>) -> LaneState {
    assert_eq!(state.len(), 1);
    state.into_iter().next().unwrap()
}

fn all_of(synth: &Synth, files: &[LaneFile], rules: &Rules) -> LaneState {
    one(recompute_all(&pool(2), &[synth.input(files)], &rules.lend()).unwrap())
}

// --- The fixtures ----------------------------------------------------------------------------------------------------

/// The reference for the findings: `checks_c1_to_c8` over the whole swimlane at once, built from the pieces the engine has.
fn reference_findings(lane: &LaneData, rules: &Rules) -> Vec<CheckResult> {
    struct Parsed {
        flat: Option<engine::Flat>,
        warnings: Vec<engine::ParseWarning>,
        dropped: usize,
    }
    let limits = Limits::default();
    let parsed: Vec<Parsed> = lane
        .files
        .iter()
        .map(|f| {
            let syntax = (f.class == FileClass::Structured)
                .then(|| Syntax::of_path(&f.path))
                .flatten();
            let (Some(syntax), Some(content)) = (syntax, f.nfs.as_ref()) else {
                return Parsed {
                    flat: None,
                    warnings: Vec::new(),
                    dropped: 0,
                };
            };
            let decoded = decode_text(&content.bytes, &limits).unwrap();
            let p = parse(&decoded.text, syntax, &limits).unwrap();
            Parsed {
                flat: Some(flatten(&p, &limits).unwrap()),
                warnings: p.warnings,
                dropped: p.warnings_dropped,
            }
        })
        .collect();
    let drifts: Vec<_> = lane
        .files
        .iter()
        .map(|f| {
            drift_state(&DriftInput {
                baseline: f.baseline,
                nfs: f.nfs.as_ref().map(|c| normalised_hash(&c.bytes, f.class)),
                git: f.git,
                intentional: f.intentional,
            })
        })
        .collect();
    let views: Vec<CheckFile<'_>> = lane
        .files
        .iter()
        .zip(&parsed)
        .zip(&drifts)
        .map(|((f, p), d)| CheckFile {
            path: &f.path,
            facts: &f.facts,
            class: f.class,
            hash: normalised_hash(&f.nfs.as_ref().unwrap().bytes, f.class),
            merge_mode: merge_mode(logical_file_role(&f.facts.logical, &rules.apps)),
            structured: p.flat.as_ref().map(|flat| Structured {
                settings: &flat.settings,
                parse_warnings: &p.warnings,
                parse_warnings_dropped: p.dropped,
            }),
            root_settings: None,
            drift: Some(*d),
            overwrite: None,
            base_history: None,
        })
        .collect();
    checks_c1_to_c8(&CheckInput {
        tenants: &lane.tenants,
        tier: None,
        host_rules: &rules.hosts,
        files: &views,
        limits: &limits,
    })
}

#[test]
fn recompute_all_findings_equal_the_checks_over_the_whole_swimlane() {
    let rules = Rules::whole_file();
    let lanes = fixture_lanes();
    assert_eq!(lanes.len(), 4);
    let states = recompute_all(&pool(2), &inputs(lanes), &rules.lend()).unwrap();
    let mut kinds = BTreeSet::new();
    for (lane, state) in lanes.iter().zip(&states) {
        assert_eq!(
            state.swimlane(),
            &lane.id,
            "results are in the order of the input"
        );
        let expected = reference_findings(lane, &rules);
        assert_eq!(state.findings(), expected, "{}", lane.id);
        kinds.extend(expected.iter().map(|r| r.kind));
    }
    // The fixtures plant the findings that the checks look for, so the comparison above is not between two empty lists.
    for kind in [
        FindingKind::Drift,
        FindingKind::RedundantTenantFile,
        FindingKind::OrphanFile,
        FindingKind::DuplicateKeys,
    ] {
        assert!(
            kinds.contains(&kind),
            "no {kind} finding in the fixtures: {kinds:?}"
        );
    }
}

#[test]
fn drift_states_are_the_planted_ones() {
    let rules = Rules::whole_file();
    let lanes = fixture_lanes();
    let states = recompute_all(&pool(2), &inputs(lanes), &rules.lend()).unwrap();
    let manifest = &small_fixtures().manifest;
    let planted = &manifest.planted;
    let mut expected: BTreeMap<(String, String), DriftState> = BTreeMap::new();
    for p in &planted.nfs_ahead {
        expected.insert((p.swimlane.clone(), p.path.clone()), DriftState::NfsAhead);
    }
    for p in &planted.git_ahead {
        expected.insert((p.swimlane.clone(), p.path.clone()), DriftState::GitAhead);
    }
    for p in &planted.untracked {
        expected.insert((p.swimlane.clone(), p.path.clone()), DriftState::Untracked);
    }
    // The orphans are files of another branch's suffix that Git does not have either.
    for o in &planted.orphans {
        expected
            .entry((o.swimlane.clone(), o.path.clone()))
            .or_insert(DriftState::Untracked);
    }
    assert!(expected.len() >= 6, "the fixtures plant drift: {expected:?}");
    let mut seen = BTreeMap::new();
    for state in &states {
        for file in state.files() {
            if file.drift.state != DriftState::InSync {
                seen.insert(
                    (state.swimlane().to_string(), file.path.to_string()),
                    file.drift.state,
                );
            }
        }
    }
    // Everything planted drifts as planted, and nothing else drifts.
    for (key, state) in &expected {
        assert_eq!(seen.get(key), Some(state), "{key:?}");
    }
    for (key, state) in &seen {
        assert!(expected.contains_key(key), "unplanted drift {key:?}: {state}");
    }
}

#[test]
fn the_tree_is_the_effective_tree_hash_of_the_effective_hashes() {
    let rules = Rules::whole_file();
    let states = recompute_all(&pool(2), &inputs(fixture_lanes()), &rules.lend()).unwrap();
    for state in &states {
        let leaves: Vec<(LogicalFile, ContentHash)> =
            state.effective_hashes().map(|(f, h)| (f.clone(), h)).collect();
        assert!(
            leaves.len() > 50,
            "{} serves {} files",
            state.swimlane(),
            leaves.len()
        );
        let expected = effective_tree_hash(&leaves).unwrap();
        assert_eq!(state.tree_hashes(), expected);
        assert_eq!(state.tree_root(), expected.root());
        assert!(state.tree_root().is_some());
        assert_eq!(state.directory_hash(&NfsPath::root()), state.tree_root());
    }
    // Two swimlanes with other tenants serve other files.
    assert_ne!(states[0].tree_root(), states[1].tree_root());
}

/// A copy of a lane file with other content on NFS.
fn with_text(file: &LaneFile, text: &str) -> LaneFile {
    let bytes = Bytes::from(text.to_owned());
    LaneFile {
        nfs: Some(NfsContent {
            blob: sha(&bytes),
            bytes,
        }),
        ..file.clone()
    }
}

fn text_of(file: &LaneFile) -> String {
    String::from_utf8(file.nfs.as_ref().unwrap().bytes.to_vec()).unwrap()
}

/// The lanes with some changes applied, and the affected pairs.
fn mutated(lanes: &[LaneData]) -> (Vec<LaneData>, Vec<(SwimlaneId, NfsPath)>) {
    let mut out: Vec<LaneData> = lanes
        .iter()
        .map(|l| LaneData {
            id: l.id.clone(),
            tenants: l.tenants.clone(),
            files: l.files.clone(),
        })
        .collect();
    let mut affected = Vec::new();
    let mut touch = |lane: &mut LaneData, path: &str| affected.push((lane.id.clone(), nfs(path)));

    // 1. A tenant file edited on NFS (its drift, its effective hash and the tree above it change).
    {
        let lane = &mut out[0];
        let target = lane
            .files
            .iter()
            .position(|f| f.facts.tenant.is_some() && f.class == FileClass::Structured && f.git.is_some())
            .unwrap();
        let path = lane.files[target].path.to_string();
        let edited = format!("{}# edited\nbrand_new_key: 1\n", text_of(&lane.files[target]));
        lane.files[target] = with_text(&lane.files[target], &edited);
        touch(lane, &path);
    }
    // 2. A base file edited: the tenant files of the same logical file are in its group.
    {
        let lane = &mut out[1];
        let target = lane
            .files
            .iter()
            .position(|f| f.facts.tenant.is_none() && f.class == FileClass::Structured && f.git.is_some())
            .unwrap();
        let path = lane.files[target].path.to_string();
        let edited = format!("{}added_after: 2\n", text_of(&lane.files[target]));
        lane.files[target] = with_text(&lane.files[target], &edited);
        touch(lane, &path);
    }
    // 3. A file deleted.
    {
        let lane = &mut out[2];
        let target = lane
            .files
            .iter()
            .position(|f| f.facts.tenant.is_some() && f.class == FileClass::Structured)
            .unwrap();
        let removed = lane.files.remove(target);
        touch(lane, removed.path.as_str());
    }
    // 4. A file added that Git does not have.
    {
        let lane = &mut out[3];
        let template = lane
            .files
            .iter()
            .find(|f| f.class == FileClass::Structured && f.facts.tenant.is_none())
            .unwrap()
            .clone();
        let mut added = with_text(&template, "added: true\n");
        added.path = nfs("brand/new-file.yml");
        added.facts.logical = logical("brand/new-file.yml");
        added.facts.kind = domain::FileKind::Untracked;
        added.git = None;
        added.baseline = Baseline::NotRecorded;
        lane.files.push(added);
        touch(lane, "brand/new-file.yml");
    }
    (out, affected)
}

#[test]
fn paths_result_equals_full_recompute_for_touched_pairs() {
    for rules in [Rules::whole_file(), Rules::spring_for_yml()] {
        let lanes = fixture_lanes();
        let before = recompute_all(&pool(2), &inputs(lanes), &rules.lend()).unwrap();
        let (after_lanes, affected) = mutated(lanes);
        assert_eq!(affected.len(), 4);
        let full = recompute_all(&pool(2), &inputs(&after_lanes), &rules.lend()).unwrap();
        let incremental =
            recompute_paths(&pool(2), &before, &inputs(&after_lanes), &affected, &rules.lend()).unwrap();
        assert_eq!(incremental.len(), 4);
        for state in &incremental {
            let want = by_id(&full, state.swimlane().as_str());
            assert_eq!(state, want, "{}", state.swimlane());
        }
        // The changes were real: each swimlane's tree moved.
        for state in &incremental {
            let old = by_id(&before, state.swimlane().as_str());
            assert_ne!(state.tree_root(), old.tree_root(), "{}", state.swimlane());
        }
    }
}

#[test]
fn paths_touch_only_the_swimlanes_that_are_named() {
    let rules = Rules::whole_file();
    let lanes = fixture_lanes();
    let before = recompute_all(&pool(2), &inputs(lanes), &rules.lend()).unwrap();
    let (after_lanes, affected) = mutated(lanes);
    let only_first = &affected[..1];
    let result = recompute_paths(
        &pool(2),
        &before,
        &inputs(&after_lanes),
        only_first,
        &rules.lend(),
    )
    .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].swimlane(), &only_first[0].0);
    // An affected path that did not change gives the earlier state back.
    let nothing = recompute_paths(
        &pool(2),
        &before,
        &inputs(lanes),
        &[(lanes[0].id.clone(), lanes[0].files[0].path.clone())],
        &rules.lend(),
    )
    .unwrap();
    assert_eq!(&nothing[0], &before[0]);
    // No affected path, nothing to do.
    assert!(
        recompute_paths(&pool(2), &before, &inputs(lanes), &[], &rules.lend())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn findings_for_many_swimlanes_equal_per_swimlane() {
    let rules = Rules::whole_file();
    let lanes = fixture_lanes();
    let together = findings_for_swimlanes(&pool(2), &inputs(lanes), &rules.lend()).unwrap();
    assert_eq!(together.len(), lanes.len());
    let states = recompute_all(&pool(2), &inputs(lanes), &rules.lend()).unwrap();
    for (i, lane) in lanes.iter().enumerate() {
        let alone = findings_for_swimlanes(&pool(1), &[lane.input()], &rules.lend()).unwrap();
        assert_eq!(alone.len(), 1);
        assert_eq!(together[i], alone[0], "{}", lane.id);
        assert_eq!(together[i].swimlane, lane.id);
        assert_eq!(together[i].findings, states[i].findings(), "{}", lane.id);
    }
    assert!(together.iter().any(|f| !f.findings.is_empty()));
}

// --- Cases the fixtures do not have ----------------------------------------------------------------------------------

#[test]
fn several_tenants_agree_on_a_hash_or_are_hashed_together() {
    let synth = Synth::new(&["sit1", "sit2"]);
    let files = [
        synth.file("svc/a.yml", Some("k: base\n"), Some("k: base\n")),
        synth.file("svc/a-sit1.yml", Some("k: one\n"), Some("k: one\n")),
        synth.file("svc/b.yml", Some("k: b\n"), Some("k: b\n")),
        synth.file("svc/b-sit1.yml", Some("k: b2\n"), Some("k: b2\n")),
        synth.file("svc/b-sit2.yml", Some("k: b2\n"), Some("k: b2\n")),
        // Not a tenant of the swimlane: nobody reads it, so it is not served.
        synth.file("svc/c-sit9.yml", Some("k: c\n"), Some("k: c\n")),
    ];
    let state = all_of(&synth, &files, &Rules::whole_file());
    let hash = |text: &str| normalised_hash(&Bytes::from(text.to_owned()), FileClass::Structured);
    let tenant = |n: &str| TenantId::parse(n).unwrap();
    // sit1 reads its own file, sit2 reads base: they disagree.
    assert_eq!(
        state.effective_hash(&logical("svc/a.yml")),
        swimlane_hash(&[
            (tenant("sit1"), hash("k: one\n")),
            (tenant("sit2"), hash("k: base\n"))
        ])
    );
    // Both tenants read the same content: the swimlane has that hash.
    assert_eq!(state.effective_hash(&logical("svc/b.yml")), Some(hash("k: b2\n")));
    // Served by nobody.
    assert_eq!(state.effective_hash(&logical("svc/c-sit9.yml")), None);
    assert_eq!(state.effective_hash(&logical("svc/c.yml")), None);
    let leaves: Vec<_> = state
        .effective_hashes()
        .map(|(f, _)| f.as_str().to_owned())
        .collect();
    assert_eq!(leaves, ["svc/a.yml", "svc/b.yml"]);
}

#[test]
fn a_swimlane_without_tenants_serves_the_base_files() {
    let synth = Synth::new(&[]);
    let files = [synth.file("svc/a.yml", Some("k: 1\n"), Some("k: 1\n"))];
    let state = all_of(&synth, &files, &Rules::whole_file());
    assert_eq!(
        state.effective_hash(&logical("svc/a.yml")),
        Some(normalised_hash(&Bytes::from("k: 1\n"), FileClass::Structured))
    );
}

#[test]
fn a_file_missing_on_nfs_is_drift_and_nothing_else() {
    let synth = Synth::new(&["sit1"]);
    let mut files = vec![
        synth.file("svc/a.yml", Some("k: 1\n"), Some("k: 1\n")),
        // Git and the baseline have it, NFS lost it.
        synth.file("svc/gone.yml", None, Some("k: 2\n")),
    ];
    let rules = Rules::whole_file();
    let state = all_of(&synth, &files, &rules);
    let gone = state.file(&nfs("svc/gone.yml")).unwrap();
    assert_eq!(gone.hash, None);
    assert_eq!(gone.drift.state, DriftState::NfsAhead);
    assert_eq!(state.effective_hash(&logical("svc/gone.yml")), None);
    let findings = state.findings();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::Drift);
    assert_eq!(findings[0].path.as_ref().unwrap().as_str(), "svc/gone.yml");

    // It comes back: the incremental recompute drops the finding, like a full one.
    let before = state;
    files[1] = synth.file("svc/gone.yml", Some("k: 2\n"), Some("k: 2\n"));
    let affected = [(synth.id.clone(), nfs("svc/gone.yml"))];
    let next = recompute_paths(
        &pool(1),
        &[before],
        &[synth.input(&files)],
        &affected,
        &rules.lend(),
    )
    .unwrap();
    assert_eq!(next[0], all_of(&synth, &files, &rules));
    assert!(next[0].findings().is_empty());
    assert!(next[0].effective_hash(&logical("svc/gone.yml")).is_some());
}

#[test]
fn the_root_file_is_a_layer_of_every_spring_merge_file() {
    let synth = Synth::new(&["sit1"]);
    let rules = Rules::spring_for_yml();
    let mut files = vec![
        synth.file(
            "application.yml",
            Some("shared:\n  a: 1\n"),
            Some("shared:\n  a: 1\n"),
        ),
        synth.file("svc/app.yml", Some("own: 1\n"), Some("own: 1\n")),
        synth.file("svc/app-sit1.yml", Some("own: 2\n"), Some("own: 2\n")),
        synth.file("other/x.yml", Some("x: 1\n"), Some("x: 1\n")),
    ];
    let before = all_of(&synth, &files, &rules);

    // The root changes; nothing else does.
    files[0] = synth.file(
        "application.yml",
        Some("shared:\n  a: 2\n"),
        Some("shared:\n  a: 1\n"),
    );
    let affected = [(synth.id.clone(), nfs("application.yml"))];
    let next = recompute_paths(
        &pool(2),
        std::slice::from_ref(&before),
        &[synth.input(&files)],
        &affected,
        &rules.lend(),
    )
    .unwrap();
    let full = all_of(&synth, &files, &rules);
    assert_eq!(next[0], full);
    // The merged settings of the other files moved with it: this is what a kernel that only looked at the root's own group misses.
    for file in ["svc/app.yml", "other/x.yml"] {
        assert_ne!(
            before.effective_hash(&logical(file)),
            full.effective_hash(&logical(file)),
            "{file}"
        );
    }
    // Under `whole_file` the root is not a layer of anything.
    let flat = Rules::whole_file();
    let a = all_of(&synth, &files, &flat);
    assert_eq!(
        a.effective_hash(&logical("other/x.yml")),
        Some(normalised_hash(&Bytes::from("x: 1\n"), FileClass::Structured))
    );
}

#[test]
fn every_root_file_is_a_layer_of_the_property_sources_and_properties_beats_yml_beats_yaml() {
    let synth = Synth::new(&["sit1"]);
    let rules = Rules::spring_for_yml();
    let same = |path: &str, text: &str| synth.file(path, Some(text), Some(text));
    // Three root files, each with something of its own and one key that all three define: `.properties` wins, then `.yml`.
    let split = vec![
        same(
            "application.properties",
            "key=from-properties\nfrom.properties=p\n",
        ),
        same("application.yml", "key: from-yml\nfrom:\n  yml: y\n  both: yml\n"),
        same(
            "application.yaml",
            "key: from-yaml\nfrom:\n  yaml: z\n  both: yaml\n",
        ),
        same("svc/app.yml", "own: x\n"),
        same("svc/app-sit1.yml", "own: t\nfrom:\n  yaml: tenant\n"),
    ];
    // The same settings in one root file.
    let one = vec![
        same(
            "application.yml",
            "key: from-properties\nfrom:\n  properties: p\n  yml: y\n  yaml: z\n  both: yml\n",
        ),
        same("svc/app.yml", "own: x\n"),
        same("svc/app-sit1.yml", "own: t\nfrom:\n  yaml: tenant\n"),
    ];
    let (a, b) = (all_of(&synth, &split, &rules), all_of(&synth, &one, &rules));
    let app = logical("svc/app.yml");
    assert!(a.effective_hash(&app).is_some());
    assert_eq!(
        a.effective_hash(&app),
        b.effective_hash(&app),
        "the effective settings of a property source must be those of all the root files, merged"
    );
    // A tenant setting that overrides a setting of a root file other than the strongest is an override and not a dead one.
    assert!(
        !a.findings()
            .iter()
            .any(|f| f.kind == FindingKind::MissedBaseChanges),
        "{:?}",
        a.findings()
    );

    // Changing the weaker root file moves what the services read, and the incremental recompute agrees with the full one.
    let mut changed = split.clone();
    changed[1] = same(
        "application.yml",
        "key: from-yml\nfrom:\n  yml: changed\n  both: yml\n",
    );
    let affected = [(synth.id.clone(), nfs("application.yml"))];
    let next = recompute_paths(
        &pool(2),
        std::slice::from_ref(&a),
        &[synth.input(&changed)],
        &affected,
        &rules.lend(),
    )
    .unwrap();
    assert_eq!(next[0], all_of(&synth, &changed, &rules));
    assert_ne!(next[0].effective_hash(&app), a.effective_hash(&app));

    // Each root file is a file of its own: another root file is not a layer of it.
    let alone = vec![same(
        "application.yml",
        "key: from-yml\nfrom:\n  yml: y\n  both: yml\n",
    )];
    let (with_others, without) = (all_of(&synth, &split, &rules), all_of(&synth, &alone, &rules));
    let root = logical("application.yml");
    assert_eq!(with_others.effective_hash(&root), without.effective_hash(&root));
}

#[test]
fn base_history_reaches_check_c1() {
    let synth = Synth::new(&["sit1"]);
    let version = |sha: &str, time: i64, text: &str| BaseVersion {
        commit: CommitId::parse(sha).unwrap(),
        time: Timestamp::from_unix_millis(time),
        content: Some(Bytes::from(text.to_owned())),
    };
    let history = Arc::new(LaneHistory {
        tenant_commit_time: Timestamp::from_unix_millis(1_000),
        at_tenant_commit: Some(version(&"a".repeat(40), 500, "k: 1\n")),
        since: vec![version(&"b".repeat(40), 2_000, "k: 1\nadded: 2\n")],
    });
    let mut copy = synth.file("svc/a-sit1.yml", Some("k: 1\n"), Some("k: 1\n"));
    copy.history = Some(history);
    let files = [
        synth.file("svc/a.yml", Some("k: 1\nadded: 2\n"), Some("k: 1\nadded: 2\n")),
        copy,
    ];
    let findings = all_of(&synth, &files, &Rules::whole_file()).findings();
    let c1: Vec<_> = findings
        .iter()
        .filter(|f| f.kind == FindingKind::MissedBaseChanges)
        .collect();
    assert_eq!(c1.len(), 1, "{findings:?}");
    assert_eq!(c1[0].path.as_ref().unwrap().as_str(), "svc/a-sit1.yml");
}

#[test]
fn tier_and_host_rules_reach_check_c6() {
    let synth = Synth::new(&["sit1"]);
    let tier = domain::ShortText::parse("uat").unwrap();
    let rules = Rules {
        hosts: HostRules::new(vec![
            engine::HostRule {
                tier: domain::ShortText::parse("dev").unwrap(),
                matcher: engine::HostMatcher::Contains(".dev.corp.example".to_owned()),
            },
            engine::HostRule {
                tier: tier.clone(),
                matcher: engine::HostMatcher::Contains(".uat.corp.example".to_owned()),
            },
        ])
        .unwrap(),
        ..Rules::whole_file()
    };
    let files = [synth.file(
        "svc/a.yml",
        Some("video: https://media.dev.corp.example/x\n"),
        Some("video: https://media.dev.corp.example/x\n"),
    )];
    let with_tier = LaneInput {
        tier: Some(&tier),
        ..synth.input(&files)
    };
    let found = recompute_all(&pool(1), &[with_tier], &rules.lend()).unwrap();
    assert!(
        found[0]
            .findings()
            .iter()
            .any(|f| f.kind == FindingKind::EnvironmentHostMismatch),
        "{:?}",
        found[0].findings()
    );
    // Without a tier, C6 is off.
    let without = recompute_all(&pool(1), &[synth.input(&files)], &rules.lend()).unwrap();
    assert!(without[0].findings().is_empty());
}

#[test]
fn inputs_that_are_not_valid_are_refused_before_any_work() {
    let synth = Synth::new(&["sit1"]);
    let rules = Rules::whole_file();
    let p = pool(2);
    let a = synth.file("svc/a.yml", Some("k: 1\n"), None);

    let twice = [a.clone(), a.clone()];
    assert_eq!(
        recompute_all(&p, &[synth.input(&twice)], &rules.lend()).unwrap_err(),
        BatchError::DuplicatePath
    );
    let once = [a.clone()];
    assert_eq!(
        recompute_all(&p, &[synth.input(&once), synth.input(&once)], &rules.lend()).unwrap_err(),
        BatchError::DuplicateSwimlane
    );
    // A file that is a directory of another: no tree.
    let clash = [
        synth.file("svc", Some("k: 1\n"), None),
        synth.file("svc/x.yml", Some("k: 1\n"), None),
    ];
    assert_eq!(
        recompute_all(&p, &[synth.input(&clash)], &rules.lend()).unwrap_err(),
        BatchError::InconsistentTree
    );
    // A swimlane the earlier states do not have.
    let state = all_of(&synth, &once, &rules);
    let other = SwimlaneId::parse("sl-other").unwrap();
    assert_eq!(
        recompute_paths(
            &p,
            &[state.clone()],
            &[synth.input(&once)],
            &[(other, nfs("svc/a.yml"))],
            &rules.lend()
        )
        .unwrap_err(),
        BatchError::UnknownSwimlane
    );
    // An affected swimlane that is missing from the new inputs.
    assert_eq!(
        recompute_paths(
            &p,
            &[state],
            &[],
            &[(synth.id.clone(), nfs("svc/a.yml"))],
            &rules.lend()
        )
        .unwrap_err(),
        BatchError::UnknownSwimlane
    );
    // The limits.
    let many: Vec<&Synth> = vec![&synth; engine::MAX_BATCH_SWIMLANES + 1];
    let lanes: Vec<LaneInput<'_>> = many.iter().map(|s| s.input(&once)).collect();
    assert_eq!(
        recompute_all(&p, &lanes, &rules.lend()).unwrap_err(),
        BatchError::TooManySwimlanes
    );
}

#[test]
fn every_input_is_checked_before_any_swimlane_is_computed() {
    let synth = Synth::new(&["sit1"]);
    let rules = Rules::whole_file();
    let p = pool(2);
    let second = SwimlaneId::parse("sl-second").unwrap();
    // The first swimlane fails while it is computed (a file that is a directory of another); the second is not what the
    // function takes (a path twice). The second is refused first: nothing is parsed or computed for a call that has it.
    let clash = [
        synth.file("svc", Some("k: 1\n"), None),
        synth.file("svc/x.yml", Some("k: 1\n"), None),
    ];
    let a = synth.file("svc/a.yml", Some("k: 1\n"), None);
    let twice = [a.clone(), a];
    let lanes = [
        synth.input(&clash),
        LaneInput {
            swimlane: &second,
            ..synth.input(&twice)
        },
    ];
    for _ in 0..4 {
        assert_eq!(
            recompute_all(&p, &lanes, &rules.lend()).unwrap_err(),
            BatchError::DuplicatePath
        );
        assert_eq!(
            findings_for_swimlanes(&p, &lanes, &rules.lend()).unwrap_err(),
            BatchError::DuplicatePath
        );
    }
}

#[test]
fn debug_output_never_shows_file_content() {
    let synth = Synth::new(&["sit1"]);
    let secret = "password: hunter2-very-secret\n";
    let files = [synth.file("svc/a.yml", Some(secret), None)];
    let state = all_of(&synth, &files, &Rules::whole_file());
    let shown = format!("{:?} {:?} {:?}", files[0], state, state.findings());
    assert!(!shown.contains("hunter2"), "{shown}");
}

// --- The adoption ranking --------------------------------------------------------------------------------------------

/// The tenant-suffixed files of a swimlane, with their hashes, as the caller of the ranking passes them.
fn tenant_files(lane: &LaneData) -> Vec<(NfsPath, ContentHash)> {
    lane.files
        .iter()
        .filter(|f| f.facts.tenant.is_some())
        .map(|f| {
            (
                f.path.clone(),
                normalised_hash(&f.nfs.as_ref().unwrap().bytes, f.class),
            )
        })
        .collect()
}

#[test]
fn adoption_rank_picks_planted_branch() {
    let branches = load_branches(&small_fixtures().root);
    assert_eq!(branches.len(), 6);
    let candidates: Vec<AdoptionCandidate<'_>> = branches
        .iter()
        .map(|b| AdoptionCandidate {
            branch: &b.name,
            kind: BranchKind::Deployable,
            tenant: &b.tenant,
            files: &b.files,
        })
        .collect();
    let rule = PathMappingRule::default();
    let single: Vec<&LaneData> = fixture_lanes().iter().filter(|l| l.tenants.len() == 1).collect();
    assert!(single.len() >= 2);
    for lane in single {
        let ranked = adoption_rank(&pool(2), &rule, &tenant_files(lane), &candidates).unwrap();
        assert_eq!(ranked.len(), 6);
        let planted = lane.tenants.iter().next().unwrap();
        assert_eq!(&ranked[0].tenant, planted, "{}: {ranked:?}", lane.id);
        assert!(
            ranked[0].similarity > ranked[1].similarity,
            "{}: the planted branch is clearly first: {ranked:?}",
            lane.id
        );
        assert!(ranked[0].similarity.ratio() > 0.8, "{ranked:?}");
    }
    // A swimlane of two tenants is explained by those two branches.
    let double = fixture_lanes().iter().find(|l| l.tenants.len() == 2).unwrap();
    let ranked = adoption_rank(&pool(2), &rule, &tenant_files(double), &candidates).unwrap();
    let top_two: BTreeSet<&TenantId> = ranked[..2].iter().map(|s| &s.tenant).collect();
    let planted: BTreeSet<&TenantId> = double.tenants.iter().collect();
    assert_eq!(top_two, planted, "{ranked:?}");
}

#[test]
fn templates_never_ranked() {
    let branches = load_branches(&small_fixtures().root);
    let lane = fixture_lanes().iter().find(|l| l.tenants.len() == 1).unwrap();
    let ours = tenant_files(lane);
    let planted = lane.tenants.iter().next().unwrap();
    let twin = branches.iter().find(|b| &b.tenant == planted).unwrap();
    // Branches that look exactly like the swimlane, of every kind that is not a deployable branch.
    let names: Vec<GitRef> = ["templates/core", "templates/common-docs", "master", "feature/x"]
        .iter()
        .map(|n| GitRef::parse(n).unwrap())
        .collect();
    let kinds = [
        BranchKind::Template,
        BranchKind::CommonDocs,
        BranchKind::Default,
        BranchKind::Other,
    ];
    let mut candidates: Vec<AdoptionCandidate<'_>> = names
        .iter()
        .zip(kinds)
        .map(|(branch, kind)| AdoptionCandidate {
            branch,
            kind,
            tenant: &twin.tenant,
            files: &twin.files,
        })
        .collect();
    candidates.push(AdoptionCandidate {
        branch: &twin.name,
        kind: BranchKind::Deployable,
        tenant: &twin.tenant,
        files: &twin.files,
    });
    let ranked = adoption_rank(&pool(2), &PathMappingRule::default(), &ours, &candidates).unwrap();
    assert_eq!(ranked.len(), 1, "{ranked:?}");
    assert_eq!(ranked[0].branch, twin.name);
    // With no deployable branch at all there is nothing to propose.
    let only_templates = &candidates[..4];
    assert!(
        adoption_rank(&pool(2), &PathMappingRule::default(), &ours, only_templates)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn similarity_is_exact_and_ties_go_to_the_branch_name() {
    let rule = PathMappingRule::default();
    let h = |n: u8| ContentHash::from_bytes([n; 32]);
    let repo = |s: &str| domain::RepoPath::parse(s).unwrap();
    let tenant_a = TenantId::parse("sit1").unwrap();
    let tenant_b = TenantId::parse("sit2").unwrap();
    // The swimlane holds three files of sit1 and three of sit2 (different names), so every branch can share at most 3 of 6.
    let ours = vec![
        (nfs("a-sit1.yml"), h(1)),
        (nfs("b-sit1.yml"), h(2)),
        (nfs("c-sit1.yml"), h(3)),
        (nfs("a-sit2.yml"), h(4)),
        (nfs("b-sit2.yml"), h(5)),
        (nfs("c-sit2.yml"), h(6)),
    ];
    // z-branch: shares 3 of 3 (union 6) = 1/2. a-branch: shares 3 of 3 too, in another tenant: also 1/2. Equal fractions with
    // other numerators and denominators must tie as well: m-branch shares 1 of 1 and has 1 file, union 6 + 1 - 1 = 6: 1/6.
    let files_a = vec![
        (repo("data/config/a.yml"), h(1)),
        (repo("data/config/b.yml"), h(2)),
        (repo("data/config/c.yml"), h(3)),
    ];
    let files_b = vec![
        (repo("data/config/a.yml"), h(4)),
        (repo("data/config/b.yml"), h(5)),
        (repo("data/config/c.yml"), h(6)),
    ];
    // One more file in the branch that the swimlane does not have: 3 shared of 7 against 3 of 6.
    let mut files_extra = files_a.clone();
    files_extra.push((repo("data/config/zz.yml"), h(9)));
    let z = GitRef::parse("z-branch").unwrap();
    let a = GitRef::parse("a-branch").unwrap();
    let m = GitRef::parse("m-branch").unwrap();
    let candidates = [
        AdoptionCandidate {
            branch: &z,
            kind: BranchKind::Deployable,
            tenant: &tenant_a,
            files: &files_a,
        },
        AdoptionCandidate {
            branch: &a,
            kind: BranchKind::Deployable,
            tenant: &tenant_b,
            files: &files_b,
        },
        AdoptionCandidate {
            branch: &m,
            kind: BranchKind::Deployable,
            tenant: &tenant_a,
            files: &files_extra,
        },
    ];
    for threads in [1, 2, 8] {
        let ranked = adoption_rank(&pool(threads), &rule, &ours, &candidates).unwrap();
        let order: Vec<&str> = ranked.iter().map(|s| s.branch.as_str()).collect();
        // a-branch and z-branch tie at 3/6 and go by name; m-branch has 3/7 and is last.
        assert_eq!(order, ["a-branch", "z-branch", "m-branch"], "{threads} threads");
        assert_eq!(ranked[0].similarity, ranked[1].similarity);
        assert_eq!(
            (ranked[0].similarity.shared(), ranked[0].similarity.union()),
            (3, 6)
        );
        assert_eq!(
            (ranked[2].similarity.shared(), ranked[2].similarity.union()),
            (3, 7)
        );
    }
    // 2/4 and 3/6 are the same fraction: they tie, which a comparison of rounded floats could not promise.
    let half_a = engine_similarity(&rule, 2, 4);
    let half_b = engine_similarity(&rule, 3, 6);
    assert_eq!(half_a.cmp(&half_b), std::cmp::Ordering::Equal);
}

/// A similarity of `shared / union` made through the ranking: `shared` files in common and `union - shared` more in the branch.
fn engine_similarity(rule: &PathMappingRule, shared: usize, union: usize) -> engine::Similarity {
    let tenant = TenantId::parse("sit1").unwrap();
    let branch = GitRef::parse("b").unwrap();
    let hash = |n: usize| ContentHash::from_bytes([u8::try_from(n).unwrap(); 32]);
    let ours: Vec<(NfsPath, ContentHash)> = (0..shared)
        .map(|n| (nfs(&format!("f{n}-sit1.yml")), hash(n + 1)))
        .collect();
    let mut theirs: Vec<(domain::RepoPath, ContentHash)> = (0..shared)
        .map(|n| {
            (
                domain::RepoPath::parse(&format!("data/config/f{n}.yml")).unwrap(),
                hash(n + 1),
            )
        })
        .collect();
    for n in 0..(union - shared) {
        theirs.push((
            domain::RepoPath::parse(&format!("data/config/extra{n}.yml")).unwrap(),
            hash(100 + n),
        ));
    }
    let candidates = [AdoptionCandidate {
        branch: &branch,
        kind: BranchKind::Deployable,
        tenant: &tenant,
        files: &theirs,
    }];
    let ranked = adoption_rank(&pool(1), rule, &ours, &candidates).unwrap();
    assert_eq!(
        (ranked[0].similarity.shared(), ranked[0].similarity.union()),
        (shared as u64, union as u64)
    );
    ranked[0].similarity
}

#[test]
fn adoption_rank_takes_no_more_than_its_limits() {
    let rule = PathMappingRule::default();
    let branch = GitRef::parse("sit1").unwrap();
    let tenant = TenantId::parse("sit1").unwrap();
    let candidate = AdoptionCandidate {
        branch: &branch,
        kind: BranchKind::Deployable,
        tenant: &tenant,
        files: &[],
    };
    let many = vec![candidate; engine::MAX_ADOPTION_CANDIDATES + 1];
    assert_eq!(
        adoption_rank(&pool(1), &rule, &[], &many).unwrap_err(),
        BatchError::TooManyCandidates
    );
    let ok = vec![candidate; engine::MAX_ADOPTION_CANDIDATES];
    assert_eq!(adoption_rank(&pool(1), &rule, &[], &ok).unwrap().len(), ok.len());
}

// --- Incremental equals full, on random swimlanes --------------------------------------------------------------------

const PATHS: [&str; 12] = [
    "application.yml",
    "svc/a.yml",
    "svc/a-sit1.yml",
    "svc/a-sit2.yml",
    "svc/b.yml",
    "svc/b-sit1.yml",
    "svc/sub/c.yml",
    "svc/sub/c-sit1.yml",
    "svc/d-sit9.yml",
    "conf/e.properties",
    "img/x.png",
    "svc",
];
const TEXTS: [&str; 7] = [
    "k: 1\n",
    "k: 2\n",
    "a:\n  b: 1\n",
    "a:\n  b: 2\n  c: x\n",
    "list:\n  - 1\n  - 2\n",
    "dup: 1\ndup: 2\n",
    "not: [valid\n",
];

/// For each path: absent, or (text index, Git: none / same / other text).
type Slot = Option<(usize, Option<usize>)>;

fn slots() -> impl Strategy<Value = Vec<Slot>> {
    proptest::collection::vec(
        proptest::option::weighted(0.7, (0..TEXTS.len(), proptest::option::of(0..TEXTS.len()))),
        PATHS.len(),
    )
}

fn lane_of(synth: &Synth, slots: &[Slot]) -> Vec<LaneFile> {
    PATHS
        .iter()
        .zip(slots)
        // `svc` as a file would clash with `svc/...` (no tree: its own test), so it is never listed.
        .filter(|(path, slot)| **path != "svc" || slot.is_none())
        .filter_map(|(path, slot)| {
            slot.map(|(n, git)| synth.file(path, Some(TEXTS[n]), git.map(|g| TEXTS[g])))
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Whatever changed between two versions of a swimlane, `recompute_paths` on the changed paths gives what `recompute_all`
    /// gives on the new version, for both merge models, and the incremental tree is the tree of the effective hashes.
    #[test]
    fn incremental_equals_full_prop(before in slots(), after in slots(), spring in any::<bool>(), extra in 0..3_usize) {
        let synth = Synth::new(&["sit1", "sit2"]);
        let rules = if spring { Rules::spring_for_yml() } else { Rules::whole_file() };
        let (a, b) = (lane_of(&synth, &before), lane_of(&synth, &after));
        let state = all_of(&synth, &a, &rules);
        let mut affected: Vec<(SwimlaneId, NfsPath)> = PATHS
            .iter()
            .zip(before.iter().zip(&after))
            .filter(|(_, (x, y))| x != y)
            .map(|(p, _)| (synth.id.clone(), nfs(p)))
            .collect();
        // Paths that did not change may be listed too.
        for p in PATHS.iter().take(extra) {
            affected.push((synth.id.clone(), nfs(p)));
        }
        let full = all_of(&synth, &b, &rules);
        let next = recompute_paths(&pool(2), &[state], &[synth.input(&b)], &affected, &rules.lend());
        if affected.is_empty() {
            prop_assert!(next.unwrap().is_empty());
        } else {
            let next = one(next.unwrap());
            prop_assert_eq!(&next, &full);
            let leaves: Vec<(LogicalFile, ContentHash)> =
                next.effective_hashes().map(|(f, h)| (f.clone(), h)).collect();
            prop_assert_eq!(next.tree_hashes(), effective_tree_hash(&leaves).unwrap());
        }
    }
}
