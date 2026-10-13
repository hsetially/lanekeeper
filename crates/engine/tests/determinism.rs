//! T8 (acceptance 3): the output is deterministic. Every public function runs on the small fixtures, twice, on pools of one,
//! two and eight threads, and everything it gives is written to a transcript that is compared byte for byte.
//!
//! The transcript holds a line per result: a label and the SHA-256 of the result written out in full, values included (the
//! types' `Debug` leaves values out on purpose, S13, so the test writes them itself where it must). A line that differs names
//! the function that gave another answer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::OnceLock;

use bytes::Bytes;
use domain::{
    BranchKind, ContentHash, DriftState, FileClass, LogicalFile, MergeMode, NfsPath, PathMappingRule,
    SwimlaneId, TextQuery,
};
use engine::{
    AdoptionCandidate, Baseline, BatchRules, BinaryFacts, CancelFlag, DenyMatcher, DriftInput, EffectChange,
    EffectInput, Effective, Flat, GridColumn, GridOptions, HostRules, LaneState, Layer, Layers, Limits,
    MergeRules, ScanConfig, SearchCorpus, Syntax, TreeHashes, adoption_rank, apply_eol, binary_compare,
    classify, decode_text, detect_format, dominant_eol, drift_state, effect_preview, effective,
    effective_tree_hash, findings_for_swimlanes, flatten, grid, index_settings, normalise_eol,
    normalised_hash, parse, recompute_all, recompute_paths, search_text, secret_scan, semantic_diff,
    swimlane_hash, text_diff,
};
use sha2::{Digest, Sha256};
use support::lanes::{LaneData, load_branches, load_lanes};
use support::small_fixtures;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

fn lanes() -> &'static Vec<LaneData> {
    static CELL: OnceLock<Vec<LaneData>> = OnceLock::new();
    CELL.get_or_init(|| load_lanes(&small_fixtures().root, usize::MAX))
}

/// A line per result: the label, and the hash of the result written in full.
#[derive(Default)]
struct Transcript(String);

impl Transcript {
    fn add(&mut self, label: &str, written: &str) {
        let digest: [u8; 32] = Sha256::digest(written.as_bytes()).into();
        let _ = writeln!(self.0, "{label} {}", ContentHash::from_bytes(digest));
    }
}

/// A flattened file written out with its values.
fn flat_text(flat: &Flat) -> String {
    support::dump(flat)
}

fn flat_of(data: &[u8], syntax: Syntax) -> Option<Flat> {
    let limits = Limits::default();
    let decoded = decode_text(data, &limits).ok()?;
    flatten(&parse(&decoded.text, syntax, &limits).ok()?, &limits).ok()
}

fn effective_text(e: &Effective) -> String {
    let mut out = format!("{:?} {:?}\n", e.mode, e.hash);
    for s in &e.sources {
        let _ = writeln!(out, "source {} {} {}", s.path, s.rank, s.hash);
    }
    for v in &e.settings {
        let _ = writeln!(
            out,
            "{} = {:?} {:?} from {} {} {:?}",
            v.path, v.value.text, v.value.ty, v.origin.file, v.origin.rank, v.origin.lines
        );
    }
    out
}

fn tree_text(t: &TreeHashes) -> String {
    t.iter().fold(String::new(), |mut out, (d, h)| {
        let _ = writeln!(out, "{d} {h}");
        out
    })
}

/// Everything the engine's functions give on the fixtures, on this pool.
#[allow(clippy::too_many_lines)]
fn transcript(pool: &rayon::ThreadPool) -> String {
    let limits = Limits::default();
    let deny = DenyMatcher::default();
    let mut t = Transcript::default();
    let lanes = lanes();

    // Bytes (T1): every file of the first swimlane.
    let mut bytes_text = String::new();
    for file in &lanes[0].files {
        let data = &file.nfs.as_ref().unwrap().bytes;
        let _ = writeln!(
            bytes_text,
            "{} {:?} {:?} {:?} {:?}",
            file.path,
            classify(&file.path, Some(data), &deny),
            detect_format(data),
            dominant_eol(data),
            normalised_hash(data, file.class),
        );
        if let Ok(decoded) = decode_text(data, &limits) {
            let _ = writeln!(bytes_text, "{}", decoded.text.len());
        }
        let lf = normalise_eol(data);
        let _ = writeln!(bytes_text, "{:?}", apply_eol(&lf, dominant_eol(data)) == *data);
    }
    t.add("bytes", &bytes_text);

    // Parsing and the settings index (T2, T3): every structured file of every swimlane, one result per distinct blob.
    let mut seen: BTreeSet<ContentHash> = BTreeSet::new();
    let (mut flats, mut indexes) = (String::new(), String::new());
    for lane in lanes {
        for file in lane.files.iter().filter(|f| f.class == FileClass::Structured) {
            let content = file.nfs.as_ref().unwrap();
            let Some(syntax) = Syntax::of_path(&file.path) else {
                continue;
            };
            if !seen.insert(content.blob) {
                continue;
            }
            match flat_of(&content.bytes, syntax) {
                Some(flat) => {
                    let _ = writeln!(flats, "{}\n{}", file.path, flat_text(&flat));
                }
                None => {
                    let _ = writeln!(flats, "{} not readable", file.path);
                }
            }
            if let Ok(index) = index_settings(&content.bytes, syntax, &limits) {
                for row in &index.rows {
                    let _ = writeln!(indexes, "{} {} {:?}", file.path, row.path, row.lines);
                }
                let _ = writeln!(indexes, "{:?} {:?}", index.located_by, index.warnings);
            }
        }
    }
    t.add("parse+flatten", &flats);
    t.add("index_settings", &indexes);

    // Path facts (T4): what the loader read from the mapping.
    let facts: String = lanes
        .iter()
        .flat_map(|l| l.files.iter().map(|f| format!("{} {:?}\n", f.path, f.facts)))
        .collect();
    t.add("classify_path", &facts);

    // Effective config, diffs, effect previews (T4, T5, T6): the tenant files against their base files in the first swimlane.
    let first = &lanes[0];
    let mut effectives = String::new();
    let (mut text_diffs, mut semantic, mut previews, mut leaves) =
        (String::new(), String::new(), String::new(), Vec::new());
    for file in first.files.iter().filter(|f| f.facts.tenant.is_some()) {
        let Some(base) = first
            .files
            .iter()
            .find(|b| b.facts.tenant.is_none() && b.facts.logical == file.facts.logical)
        else {
            continue;
        };
        let (tenant_bytes, base_bytes) = (
            &file.nfs.as_ref().unwrap().bytes,
            &base.nfs.as_ref().unwrap().bytes,
        );
        match file.class {
            FileClass::Structured | FileClass::Text => {
                let (a, b) = (
                    decode_text(base_bytes, &limits),
                    decode_text(tenant_bytes, &limits),
                );
                if let (Ok(a), Ok(b)) = (a, b) {
                    let diff = text_diff(&a.text, &b.text, &limits);
                    let _ = writeln!(text_diffs, "{} {diff:?}", file.path);
                }
            }
            _ => {
                let facts = |f: &engine::LaneFile| BinaryFacts {
                    hash: normalised_hash(&f.nfs.as_ref().unwrap().bytes, f.class),
                    size: u64::try_from(f.nfs.as_ref().unwrap().bytes.len()).unwrap(),
                };
                let _ = writeln!(
                    text_diffs,
                    "{} {:?}",
                    file.path,
                    binary_compare(Some(facts(base)), Some(facts(file)))
                );
            }
        }
        let (Some(syntax), true) = (Syntax::of_path(&file.path), file.class == FileClass::Structured) else {
            continue;
        };
        let (Some(tf), Some(bf)) = (flat_of(tenant_bytes, syntax), flat_of(base_bytes, syntax)) else {
            continue;
        };
        for change in semantic_diff(&bf.settings, &tf.settings) {
            let _ = writeln!(
                semantic,
                "{} {} {:?} {:?} {:?}",
                file.path, change.path, change.kind, change.left, change.right
            );
        }
        let layer = |f: &engine::LaneFile, flat: &Flat| -> (NfsPath, ContentHash, Vec<engine::FlatSetting>) {
            (
                f.path.clone(),
                normalised_hash(&f.nfs.as_ref().unwrap().bytes, f.class),
                flat.settings.clone(),
            )
        };
        let (tp, th, ts) = layer(file, &tf);
        let (bp, bh, bs) = layer(base, &bf);
        for mode in [MergeMode::WholeFile, MergeMode::SpringMerge] {
            let layers = Layers {
                tenant: Some(Layer {
                    path: &tp,
                    hash: th,
                    settings: &ts,
                }),
                base: Some(Layer {
                    path: &bp,
                    hash: bh,
                    settings: &bs,
                }),
                root: None,
            };
            if let Some(e) = effective(mode, &layers) {
                effectives.push_str(&effective_text(&e));
                leaves.push((file.facts.logical.clone(), e.hash));
            }
            for change in [
                EffectChange::Delete,
                EffectChange::Write(Layer {
                    path: &tp,
                    hash: th,
                    settings: &bs,
                }),
            ] {
                let preview = effect_preview(
                    &EffectInput {
                        mode,
                        target: &file.facts,
                        tenants: &first.tenants,
                        layers,
                        change,
                    },
                    &limits,
                );
                let _ = writeln!(previews, "{} {preview:?} {:?}", file.path, preview.changes);
            }
        }
    }
    assert!(
        !leaves.is_empty(),
        "the fixtures have tenant files with base files"
    );
    t.add("effective", &effectives);
    t.add("text_diff+binary_compare", &text_diffs);
    t.add("semantic_diff", &semantic);
    t.add("effect_preview", &previews);
    leaves.sort();
    leaves.dedup_by(|a, b| a.0 == b.0);
    let tree = effective_tree_hash(&leaves).unwrap();
    t.add("effective_tree_hash", &tree_text(&tree));
    t.add(
        "swimlane_hash",
        &format!(
            "{:?}",
            swimlane_hash(
                &first
                    .tenants
                    .iter()
                    .map(|x| (x.clone(), leaves[0].1))
                    .collect::<Vec<_>>()
            )
        ),
    );

    // Drift (T5): every combination of the three hashes and the marks.
    let mut drifts = String::new();
    let hashes = [
        None,
        Some(ContentHash::from_bytes([1; 32])),
        Some(ContentHash::from_bytes([2; 32])),
    ];
    for baseline in [Baseline::NotRecorded]
        .into_iter()
        .chain(hashes.iter().map(|h| Baseline::Recorded(*h)))
    {
        for nfs in hashes {
            for git in hashes {
                let _ = writeln!(
                    drifts,
                    "{:?}",
                    drift_state(&DriftInput {
                        baseline,
                        nfs,
                        git,
                        intentional: None
                    })
                );
            }
        }
    }
    t.add("drift_state", &drifts);

    // The grid (T5): the logical file with the most settings, across the swimlanes that have it.
    let mut by_logical: BTreeMap<&LogicalFile, Vec<(&SwimlaneId, Effective)>> = BTreeMap::new();
    for lane in lanes {
        for file in lane
            .files
            .iter()
            .filter(|f| f.class == FileClass::Structured && f.facts.tenant.is_none())
        {
            let Some(flat) =
                Syntax::of_path(&file.path).and_then(|s| flat_of(&file.nfs.as_ref().unwrap().bytes, s))
            else {
                continue;
            };
            let layers = Layers {
                tenant: None,
                base: Some(Layer {
                    path: &file.path,
                    hash: normalised_hash(&file.nfs.as_ref().unwrap().bytes, file.class),
                    settings: &flat.settings,
                }),
                root: None,
            };
            if let Some(e) = effective(MergeMode::WholeFile, &layers) {
                by_logical
                    .entry(&file.facts.logical)
                    .or_default()
                    .push((&lane.id, e));
            }
        }
    }
    let (_, columns) = by_logical
        .iter()
        .max_by_key(|(_, cols)| (cols.len(), cols.first().map_or(0, |c| c.1.settings.len())))
        .unwrap();
    let grid_columns: Vec<GridColumn<'_>> = columns
        .iter()
        .map(|(swimlane, e)| GridColumn {
            swimlane,
            effective: Some(e),
        })
        .collect();
    let mut grids = String::new();
    for only_differing in [true, false] {
        let options = GridOptions {
            channel: None,
            only_differing,
            after: None,
            max_rows: 100_000,
        };
        let result = grid(&grid_columns, &options, &limits).unwrap();
        for row in &result.rows {
            let cells: Vec<String> = row
                .cells
                .iter()
                .map(|c| c.map_or("-".to_owned(), |v| format!("{:?}@{:?}", v.value.text, v.lines)))
                .collect();
            let _ = writeln!(grids, "{} {}", row.path, cells.join(" | "));
        }
        let _ = writeln!(grids, "{} truncated {}", result.rows.len(), result.truncated);
    }
    t.add("grid", &grids);

    // Text search (T7): literal, regex, case-insensitive, with caps small enough to truncate.
    let corpus = SearchCorpus::from_files(lanes.iter().flat_map(|lane| {
        lane.files
            .iter()
            .filter(|f| matches!(f.class, FileClass::Structured | FileClass::Text))
            .map(|f| {
                let c = f.nfs.as_ref().unwrap();
                (lane.id.clone(), f.path.clone(), c.blob, c.bytes.clone())
            })
    }))
    .unwrap();
    let mut searches = String::new();
    for (pattern, regex, case_sensitive, max_total, max_per_file) in [
        ("corp.example", false, true, 500, 20),
        ("corp.example", false, true, 7, 2),
        ("ENABLED", false, false, 500, 20),
        (r"\bretry\b", true, true, 50, 3),
        (r"^\s*weight: \d+$", true, true, 500, 20),
        ("no-such-setting-xyzzy", false, true, 500, 20),
    ] {
        let query = TextQuery {
            pattern: pattern.to_owned(),
            regex,
            case_sensitive,
            swimlanes: Vec::new(),
            path_glob: None,
            max_total,
            max_per_file,
        };
        let outcome = search_text(&corpus, &query, &deny, &limits, pool, &CancelFlag::new()).unwrap();
        let _ = writeln!(searches, "{pattern}: {outcome:?}");
    }
    t.add("search_text", &searches);

    // Secret scan (T7): every text file of the first swimlane.
    let mut scans = String::new();
    for file in lanes[0]
        .files
        .iter()
        .filter(|f| matches!(f.class, FileClass::Structured | FileClass::Text))
    {
        if let Ok(decoded) = decode_text(&file.nfs.as_ref().unwrap().bytes, &limits) {
            let _ = writeln!(
                scans,
                "{} {:?}",
                file.path,
                secret_scan(&decoded.text, &ScanConfig::default(), &limits)
            );
        }
    }
    t.add("secret_scan", &scans);

    // The batch functions (T8).
    let merge = MergeRules::new(MergeMode::WholeFile, &[("*.yml", MergeMode::SpringMerge)]).unwrap();
    let hosts = HostRules::default();
    let rules = BatchRules {
        merge_rules: &merge,
        host_rules: &hosts,
        limits: &limits,
    };
    let inputs: Vec<_> = lanes.iter().map(LaneData::input).collect();
    let states = recompute_all(pool, &inputs, &rules).unwrap();
    t.add("recompute_all", &lane_states_text(&states));
    let findings = findings_for_swimlanes(pool, &inputs, &rules).unwrap();
    t.add("findings_for_swimlanes", &format!("{findings:#?}"));
    let affected: Vec<(SwimlaneId, NfsPath)> = lanes
        .iter()
        .map(|l| (l.id.clone(), l.files[l.files.len() / 2].path.clone()))
        .collect();
    let again = recompute_paths(pool, &states, &inputs, &affected, &rules).unwrap();
    t.add("recompute_paths", &lane_states_text(&again));
    let branches = load_branches(&small_fixtures().root);
    let candidates: Vec<AdoptionCandidate<'_>> = branches
        .iter()
        .map(|b| AdoptionCandidate {
            branch: &b.name,
            kind: BranchKind::Deployable,
            tenant: &b.tenant,
            files: &b.files,
        })
        .collect();
    let mut ranks = String::new();
    for lane in lanes {
        let ours: Vec<(NfsPath, ContentHash)> = lane
            .files
            .iter()
            .filter(|f| f.facts.tenant.is_some())
            .map(|f| {
                (
                    f.path.clone(),
                    normalised_hash(&f.nfs.as_ref().unwrap().bytes, f.class),
                )
            })
            .collect();
        let ranked = adoption_rank(pool, &PathMappingRule::default(), &ours, &candidates).unwrap();
        let _ = writeln!(ranks, "{} {ranked:?}", lane.id);
    }
    t.add("adoption_rank", &ranks);
    t.0
}

fn lane_states_text(states: &[LaneState]) -> String {
    let mut out = String::new();
    for s in states {
        let _ = writeln!(out, "{s:?}");
        let _ = writeln!(out, "{}", tree_text(&s.tree_hashes()));
        for (file, hash) in s.effective_hashes() {
            let _ = writeln!(out, "{file} {hash}");
        }
    }
    out
}

#[test]
fn everything_twice_is_byte_identical() {
    let reference = transcript(&pool(1));
    let labels: Vec<&str> = reference.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(
        labels,
        [
            "bytes",
            "parse+flatten",
            "index_settings",
            "classify_path",
            "effective",
            "text_diff+binary_compare",
            "semantic_diff",
            "effect_preview",
            "effective_tree_hash",
            "swimlane_hash",
            "drift_state",
            "grid",
            "search_text",
            "secret_scan",
            "recompute_all",
            "findings_for_swimlanes",
            "recompute_paths",
            "adoption_rank",
        ],
        "the transcript covers the engine's public functions"
    );
    for threads in [1, 2, 8] {
        for run in 0..2 {
            let again = transcript(&pool(threads));
            if again != reference {
                let first = again
                    .lines()
                    .zip(reference.lines())
                    .find(|(a, b)| a != b)
                    .map_or("(length)", |(a, _)| a.split(' ').next().unwrap_or(""));
                panic!("run {run} on {threads} threads differs from the first, first at {first:?}");
            }
        }
    }
}

#[test]
fn search_truncation_is_deterministic() {
    let limits = Limits::default();
    let deny = DenyMatcher::default();
    let corpus = SearchCorpus::from_files(lanes().iter().flat_map(|lane| {
        lane.files
            .iter()
            .filter(|f| f.class == FileClass::Structured)
            .map(|f| {
                let c = f.nfs.as_ref().unwrap();
                (lane.id.clone(), f.path.clone(), c.blob, c.bytes.clone())
            })
    }))
    .unwrap();
    let query = |max_total, max_per_file| TextQuery {
        pattern: "corp.example".to_owned(),
        regex: false,
        case_sensitive: true,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total,
        max_per_file,
    };
    for (max_total, max_per_file) in [(5, 1), (13, 2), (40, 20)] {
        let q = query(max_total, max_per_file);
        let first = search_text(&corpus, &q, &deny, &limits, &pool(1), &CancelFlag::new()).unwrap();
        assert!(
            first.results.truncated,
            "caps {max_total}/{max_per_file} must cut something"
        );
        assert_eq!(first.results.hits.len(), max_total as usize);
        // The kept hits are the first in swimlane, path and line order, whichever thread finished first.
        let order: Vec<_> = first
            .results
            .hits
            .iter()
            .map(|h| (h.swimlane.clone(), h.path.clone(), h.line))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "hits are in order");
        for threads in [1, 2, 3, 8] {
            for _ in 0..5 {
                let again =
                    search_text(&corpus, &q, &deny, &limits, &pool(threads), &CancelFlag::new()).unwrap();
                assert_eq!(again, first, "{threads} threads, caps {max_total}/{max_per_file}");
            }
        }
    }
}

#[test]
fn batch_matches_single_threaded_reference() {
    let limits = Limits::default();
    let hosts = HostRules::default();
    for merge in [
        MergeRules::default(),
        MergeRules::new(MergeMode::WholeFile, &[("*.yml", MergeMode::SpringMerge)]).unwrap(),
    ] {
        let rules = BatchRules {
            merge_rules: &merge,
            host_rules: &hosts,
            limits: &limits,
        };
        let inputs: Vec<_> = lanes().iter().map(LaneData::input).collect();
        // The reference: one swimlane at a time, on one thread.
        let one = pool(1);
        let reference: Vec<LaneState> = inputs
            .iter()
            .map(|lane| {
                recompute_all(&one, std::slice::from_ref(lane), &rules)
                    .unwrap()
                    .remove(0)
            })
            .collect();
        for threads in [1, 2, 3, 8] {
            let together = recompute_all(&pool(threads), &inputs, &rules).unwrap();
            assert_eq!(together, reference, "{threads} threads");
            assert_eq!(
                lane_states_text(&together),
                lane_states_text(&reference),
                "{threads} threads"
            );
            // The same files in the opposite order are the same swimlane.
            let mut reversed: Vec<LaneData> = lanes()
                .iter()
                .map(|l| LaneData {
                    id: l.id.clone(),
                    tenants: l.tenants.clone(),
                    files: l.files.clone(),
                })
                .collect();
            for lane in &mut reversed {
                lane.files.reverse();
            }
            let reversed_inputs: Vec<_> = reversed.iter().map(LaneData::input).collect();
            let flipped = recompute_all(&pool(threads), &reversed_inputs, &rules).unwrap();
            assert_eq!(
                flipped, reference,
                "{threads} threads, files in the opposite order"
            );
        }
    }
}

#[test]
fn a_state_does_not_depend_on_the_buffers_it_was_computed_from() {
    // A state holds hashes and findings, never content: a copy of the swimlane with the same blobs under other `Bytes` buffers
    // gives the same state.
    let limits = Limits::default();
    let merge = MergeRules::default();
    let hosts = HostRules::default();
    let rules = BatchRules {
        merge_rules: &merge,
        host_rules: &hosts,
        limits: &limits,
    };
    let original = &lanes()[1];
    let mut copy = LaneData {
        id: original.id.clone(),
        tenants: original.tenants.clone(),
        files: original.files.clone(),
    };
    for file in &mut copy.files {
        let content = file.nfs.as_mut().unwrap();
        content.bytes = Bytes::copy_from_slice(&content.bytes);
    }
    let a = recompute_all(&pool(2), &[original.input()], &rules).unwrap();
    let b = recompute_all(&pool(2), &[copy.input()], &rules).unwrap();
    assert_eq!(a, b);
    let states: Vec<DriftState> = a[0].files().map(|f| f.drift.state).collect();
    assert!(states.contains(&DriftState::InSync));
}
