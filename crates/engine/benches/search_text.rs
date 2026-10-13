//! P7: a text search across every swimlane takes 400 ms or less (`P7.engine_text_search`, `engine/search_text_target_blobs`).
//!
//! The corpus is what the hub holds at target scale: every structured and text file of the 40 swimlanes (about 80,000 files),
//! grouped by content hash into the distinct blobs (about 23,000), each with all of its places. It is built once, as the hub
//! builds it, and is not timed. The search runs on a pool of two threads, which stands in for the 2 vCPU of a hub replica.
//!
//! One iteration is one search, and the iterations go through a fixed set of 20 queries in turn: frequent and rare and absent
//! literals, case-insensitive ones, and regexes with alternation, word boundaries, anchors and classes. The criterion
//! settings give one search per sample, so the p95 that `bench-check` reads is over single searches and the slowest queries
//! are in it. The answer of each query is checked on every iteration against the first run, so a change that makes the work
//! disappear (or the answer differ) shows up as a failure, not as a fast number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::cell::Cell;
use std::fs;
use std::hint::black_box;
use std::time::Duration;

use bytes::Bytes;
use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use domain::{FileClass, NfsPath, SwimlaneId, TextQuery};
use engine::{CancelFlag, DenyMatcher, Limits, SearchCorpus, classify, normalised_hash, search_text};

/// (pattern, regex, case-sensitive)
const QUERIES: [(&str, bool, bool); 20] = [
    // Frequent literals: found in thousands of files, so the per-file cap ends most blobs early.
    ("corp.example", false, true),
    ("true", false, true),
    ("weight", false, true),
    ("enabled", false, true),
    // Medium and rare literals.
    ("settlement", false, true),
    ("CORE_ROUTING", false, true),
    ("coreRouting", false, true),
    ("https://auth.uat.corp.example", false, true),
    ("POD_IP", false, true),
    // Absent: every byte of every blob is read.
    ("no-such-setting-xyzzy", false, true),
    // Case-insensitive.
    ("DEFAULTS", false, false),
    ("Retry", false, false),
    ("NO-SUCH-SETTING-XYZZY", false, false),
    // Regexes: word boundaries, alternation, anchors, classes, placeholders.
    (r"\bretry\b", true, true),
    (
        r"https://(?:auth|docs)\.(?:sit|uat)\.corp\.example/\w+",
        true,
        true,
    ),
    (r"^\s*weight: \d{3}$", true, true),
    (r"\$\{[A-Z_]+\}", true, true),
    (r"enabled:\s*(?:true|false)", true, false),
    (r"\b\d{4}\b", true, true),
    (r"timeout|retries|limit\d", true, true),
];

fn query(pattern: &str, regex: bool, case_sensitive: bool) -> TextQuery {
    TextQuery {
        pattern: pattern.to_owned(),
        regex,
        case_sensitive,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total: TextQuery::DEFAULT_MAX_TOTAL,
        max_per_file: TextQuery::DEFAULT_MAX_PER_FILE,
    }
}

/// Every file of every swimlane that the engine would search, read lazily so that duplicates are dropped as they are read.
fn files(deny: &DenyMatcher) -> impl Iterator<Item = (SwimlaneId, NfsPath, domain::ContentHash, Bytes)> + '_ {
    let nfs = common::fixtures().join("nfs");
    let mut lanes: Vec<_> = fs::read_dir(&nfs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    lanes.sort();
    lanes.into_iter().flat_map(move |lane| {
        let swimlane = SwimlaneId::parse(&lane.file_name().unwrap().to_string_lossy()).unwrap();
        let mut found = Vec::new();
        let mut stack = vec![lane.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found.sort();
        let swimlane = swimlane.clone();
        found.into_iter().filter_map(move |path| {
            let rel = path.strip_prefix(&lane).unwrap().to_string_lossy().into_owned();
            let nfs_path = NfsPath::parse(&rel).unwrap();
            let data = Bytes::from(fs::read(&path).unwrap());
            matches!(
                classify(&nfs_path, Some(&data), deny),
                FileClass::Structured | FileClass::Text
            )
            .then(|| {
                (
                    swimlane.clone(),
                    nfs_path,
                    normalised_hash(&data, FileClass::Binary),
                    data,
                )
            })
        })
    })
}

fn search_bench(c: &mut Criterion) {
    let deny = DenyMatcher::default();
    let corpus = SearchCorpus::from_files(files(&deny)).unwrap();
    assert!(
        corpus.location_count() > 70_000
            && corpus.blob_count() > 10_000
            && corpus.blob_count() < corpus.location_count(),
        "{corpus:?}: not the target-scale fixtures, or nothing was deduplicated"
    );
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let limits = Limits::default();
    let cancel = CancelFlag::new();
    let queries: Vec<TextQuery> = QUERIES.iter().map(|&(p, r, s)| query(p, r, s)).collect();
    let run = |q: &TextQuery| search_text(&corpus, q, &deny, &limits, &pool, &cancel).unwrap();

    // The first answers: they are what every later run must give again, and the set must have both kinds of query.
    let expected: Vec<_> = queries.iter().map(run).collect();
    let (empty, full) = (
        expected.iter().filter(|o| o.results.hits.is_empty()).count(),
        expected.iter().filter(|o| o.results.truncated).count(),
    );
    assert!(empty >= 2, "{empty} queries find nothing: the absent ones must");
    assert!(
        full >= 3,
        "{full} queries are cut by a cap: the frequent ones must be"
    );
    assert!(
        expected.iter().filter(|o| !o.results.hits.is_empty()).count() >= 12,
        "most queries should find something"
    );

    let mut group = c.benchmark_group("engine");
    // One search per sample: the p95 is over single searches, not over means of several.
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(1));
    let next = Cell::new(0_usize);
    group.bench_function("search_text_target_blobs", |b| {
        b.iter(|| {
            let i = next.get();
            next.set(i + 1);
            let k = i % queries.len();
            let outcome = black_box(&run)(black_box(&queries[k]));
            assert_eq!(outcome, expected[k], "query {k} gave another answer");
            black_box(outcome)
        });
    });
    group.finish();
}

criterion_group!(benches, search_bench);
criterion_main!(benches);
