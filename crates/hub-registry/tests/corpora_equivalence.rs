//! The sharded text corpus answers as one corpus does (T6, P7, `read::corpora`): the same hits in the same order, the same caps, the
//! same `truncated`. The reference is the engine run on a single corpus of the same files; the candidate is the engine run on each
//! shard of those files and [`merge`]d, which is what the service does.
//!
//! Three things are held: a property test over random corpora (swimlanes that share contents, CRLF files, binary and non-UTF-8 files,
//! files with more matching lines than the per-file cap), random queries (literals and regexes, both cases, a swimlane list, a glob)
//! and random caps and ceilings; the same twenty queries as the benchmark over the target-scale fixtures (marked `#[ignore]`, run by
//! `registry-slow-test`); and the negative control: a merge that forgets the cut, the sort or the truncation flag is caught.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod support;

use std::sync::OnceLock;

use bytes::Bytes;
use domain::{ContentHash, NfsPath, ShortText, SwimlaneId, TextQuery, TextResults};
use engine::{CancelFlag, DenyMatcher, Limits, SearchCorpus, search_text};
use hub_registry::read::corpora::{effective_total, merge, shard_of};
use proptest::prelude::*;

type File = (SwimlaneId, NfsPath, ContentHash, Bytes);

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap())
}

fn file(lane: &str, path: &str, content: &[u8]) -> File {
    (
        SwimlaneId::parse(lane).unwrap(),
        NfsPath::parse(path).unwrap(),
        ports::content_hash(content),
        Bytes::copy_from_slice(content),
    )
}

/// The engine on one corpus of all the files.
fn single(files: &[File], q: &TextQuery, limits: &Limits) -> TextResults {
    let corpus = SearchCorpus::from_files(files.iter().cloned()).unwrap();
    search_text(
        &corpus,
        q,
        &DenyMatcher::default(),
        limits,
        pool(),
        &CancelFlag::new(),
    )
    .unwrap()
    .results
}

/// The engine on each shard of the files (a blob and all its places in one shard), put together.
fn sharded(files: &[File], shards: usize, q: &TextQuery, limits: &Limits) -> TextResults {
    let parts: Vec<TextResults> = (0..shards)
        .map(|s| {
            let mine = files
                .iter()
                .filter(|(_, _, hash, _)| shard_of(hash, shards) == s)
                .cloned();
            let corpus = SearchCorpus::from_files(mine).unwrap();
            search_text(
                &corpus,
                q,
                &DenyMatcher::default(),
                limits,
                pool(),
                &CancelFlag::new(),
            )
            .unwrap()
            .results
        })
        .collect();
    merge(parts, effective_total(q.max_total, limits.hits_total))
}

fn query(pattern: &str, regex: bool, case_sensitive: bool, max_total: u32, max_per_file: u32) -> TextQuery {
    TextQuery {
        pattern: pattern.to_owned(),
        regex,
        case_sensitive,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total,
        max_per_file,
    }
}

// ------------------------------------------------------------------------------------------------ random corpora

const LANES: [&str; 4] = ["sl-01", "sl-02", "sl-03", "sl-04"];
const PATHS: [&str; 9] = [
    "a.yml",
    "b.properties",
    "dir/c.yml",
    "dir/d.txt",
    "dir/sub/e.yaml",
    "f.json",
    "g.png",
    "h.xml",
    "dir/i.yml",
];
const WORDS: [&str; 10] = [
    "alpha",
    "Beta",
    "timeout: 30",
    "retries: 3",
    "enabled: true",
    "weight: 123",
    "${HOME}",
    "corp.example",
    "x",
    "retry",
];

/// A content: lines of words, in LF or CRLF, now and then with a NUL or bytes that are not UTF-8, now and then with the same line
/// many times (more matches than a per-file cap).
fn content() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec(
            prop::collection::vec(prop::sample::select(WORDS.to_vec()), 1..4),
            0..14,
        ),
        prop::bool::weighted(0.3),
        0_u8..12,
        0_usize..30,
    )
        .prop_map(|(lines, crlf, odd, repeat)| {
            let eol = if crlf { "\r\n" } else { "\n" };
            let mut text: String = lines.iter().map(|l| l.join(" ") + eol).collect();
            for _ in 0..repeat {
                text.push_str("retry: yes");
                text.push_str(eol);
            }
            let mut bytes = text.into_bytes();
            match odd {
                0 => bytes.push(0),
                1 => bytes.extend_from_slice(&[0xff, 0xfe, 0x41]),
                _ => {}
            }
            bytes
        })
}

/// The files of a few swimlanes, whose contents are drawn from a small pool so that swimlanes share them.
fn corpus() -> impl Strategy<Value = Vec<File>> {
    (
        prop::collection::vec(content(), 1..7),
        prop::collection::vec(
            (0..LANES.len(), prop::sample::select(PATHS.to_vec()), 0_usize..64),
            0..40,
        ),
    )
        .prop_map(|(pool, placed)| {
            let mut seen = std::collections::BTreeSet::new();
            placed
                .into_iter()
                .filter(|(lane, path, _)| seen.insert((*lane, *path)))
                .map(|(lane, path, pick)| file(LANES[lane], path, &pool[pick % pool.len()]))
                .collect()
        })
}

fn text_query() -> impl Strategy<Value = TextQuery> {
    (
        prop::sample::select(vec![
            ("retry", false, true),
            ("RETRY", false, false),
            ("timeout", false, true),
            ("zzz-absent", false, true),
            ("x", false, true),
            (r"\bretry\b", true, true),
            (r"^\s*weight: \d{3}$", true, true),
            (r"timeout|retries|alpha", true, false),
            (r"\$\{[A-Z]+\}", true, true),
        ]),
        prop::collection::vec(0..LANES.len(), 0..4),
        prop::option::of(prop::sample::select(vec!["*.yml", "dir/*", "*.txt", "**/*.yaml"])),
        1_u32..60,
        1_u32..8,
    )
        .prop_map(
            |((pattern, regex, case), lanes, glob, max_total, max_per_file)| TextQuery {
                pattern: pattern.to_owned(),
                regex,
                case_sensitive: case,
                swimlanes: lanes
                    .into_iter()
                    .map(|l| SwimlaneId::parse(LANES[l]).unwrap())
                    .collect(),
                path_glob: glob.map(|g| ShortText::parse(g).unwrap()),
                max_total,
                max_per_file,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn the_shards_answer_as_one_corpus_does(
        files in corpus(),
        q in text_query(),
        shards in 1_usize..9,
        ceiling_total in prop::sample::select(vec![5_usize, 25, 1_000]),
        ceiling_per_file in prop::sample::select(vec![2_usize, 6, 50]),
    ) {
        let limits = Limits { hits_total: ceiling_total, hits_per_file: ceiling_per_file, ..Limits::default() };
        let want = single(&files, &q, &limits);
        let got = sharded(&files, shards, &q, &limits);
        prop_assert_eq!(got, want, "{} shards, {} files", shards, files.len());
    }
}

/// The corpus that makes the cap, the overflow and the sort all matter: many places, each with several hits.
fn crowded() -> Vec<File> {
    let many = "retry: 1\n".repeat(30);
    let mut files = Vec::new();
    for lane in LANES {
        for (i, path) in ["a.yml", "dir/b.yml", "dir/c.txt"].into_iter().enumerate() {
            files.push(file(lane, path, format!("{many}own{i}\n").as_bytes()));
        }
        files.push(file(lane, "shared.yml", many.as_bytes()));
    }
    files
}

#[test]
fn a_cut_by_the_total_a_cut_inside_a_file_and_an_overflow_are_each_reported_as_the_one_corpus_reports_them() {
    let files = crowded();
    let limits = Limits::default();
    for shards in [1, 2, 3, 8] {
        for (total, per_file) in [(1, 1), (7, 3), (16, 4), (500, 20), (1_000, 50), (500, 30)] {
            let q = query("retry", false, true, total, per_file);
            let want = single(&files, &q, &limits);
            let got = sharded(&files, shards, &q, &limits);
            assert_eq!(got, want, "{shards} shards, total {total}, per file {per_file}");
        }
    }
    // Exactly at the total and with nothing after: not truncated, in both.
    let few = vec![file("sl-01", "a.yml", b"retry\nretry\n")];
    let q = query("retry", false, true, 2, 5);
    let want = single(&few, &q, &limits);
    assert!(!want.truncated && want.hits.len() == 2);
    assert_eq!(sharded(&few, 4, &q, &limits), want);
}

/// The answer of each shard to the query itself, as the service gets them.
fn parts_of(files: &[File], shards: usize, q: &TextQuery, limits: &Limits) -> Vec<TextResults> {
    (0..shards)
        .map(|s| {
            let mine = files
                .iter()
                .filter(|(_, _, h, _)| shard_of(h, shards) == s)
                .cloned();
            search_text(
                &SearchCorpus::from_files(mine).unwrap(),
                q,
                &DenyMatcher::default(),
                limits,
                pool(),
                &CancelFlag::new(),
            )
            .unwrap()
            .results
        })
        .collect()
}

/// A hit in each of twelve distinct files, over four swimlanes: no shard has more than a few, and none of them cuts anything of its own.
fn ones() -> Vec<File> {
    let mut files = Vec::new();
    for (i, lane) in LANES.iter().enumerate() {
        for (j, path) in ["a.yml", "dir/b.yml", "dir/c.txt"].into_iter().enumerate() {
            files.push(file(lane, path, format!("retry {i}-{j}\n").as_bytes()));
        }
    }
    files
}

#[test]
fn a_merge_that_leaves_out_the_cut_the_order_or_the_total_is_not_the_answer() {
    let limits = Limits::default();
    // The sort: the swimlanes are spread over the shards, so each shard's hits are in order and the shards' are not.
    let files = ones();
    let q = query("retry", false, true, 5, 4);
    let want = single(&files, &q, &limits);
    assert!(want.truncated && want.hits.len() == 5);
    let parts = parts_of(&files, 4, &q, &limits);
    assert_eq!(merge(parts.clone(), 5), want);
    let unsorted: Vec<_> = parts.iter().flat_map(|p| p.hits.clone()).take(5).collect();
    assert_ne!(
        unsorted, want.hits,
        "a concatenation of the shards' answers is not the answer"
    );
    // The cut: the shards between them hold more than the total.
    let all: usize = parts.iter().map(|p| p.hits.len()).sum();
    assert!(all > 5, "{all}");
    // The flag: with each shard asked for fewer hits than the total, none cuts anything, and the total is cut only by the merge.
    let mut small = q.clone();
    small.max_total = 2;
    let parts = parts_of(&files, 4, &small, &limits);
    assert!(
        parts.iter().all(|p| !p.truncated || p.hits.len() == 2),
        "no shard has a reason to say so beyond its own cut"
    );
    let mut asked = q.clone();
    asked.max_total = 4;
    let want = single(&files, &asked, &limits);
    assert!(want.truncated);
    let parts = parts_of(&files, 12, &asked, &limits);
    let flag_of_the_shards = parts.iter().any(|p| p.truncated);
    assert!(
        !flag_of_the_shards,
        "twelve shards of one hit each: nothing cut inside a shard"
    );
    assert_eq!(
        merge(parts, 4),
        want,
        "the sum of the hits is what says the total cut something"
    );
}

// ------------------------------------------------------------------------------------------------ the target-scale fixtures

/// The engine's twenty queries (the same as `engine/search_text_target_blobs` and `hub-registry/search_text_all_swimlanes`).
const QUERIES: [(&str, bool, bool); 20] = [
    ("corp.example", false, true),
    ("true", false, true),
    ("weight", false, true),
    ("enabled", false, true),
    ("settlement", false, true),
    ("CORE_ROUTING", false, true),
    ("coreRouting", false, true),
    ("https://auth.uat.corp.example", false, true),
    ("POD_IP", false, true),
    ("no-such-setting-xyzzy", false, true),
    ("DEFAULTS", false, false),
    ("Retry", false, false),
    ("NO-SUCH-SETTING-XYZZY", false, false),
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

#[test]
#[ignore = "target scale: run by `just registry-slow-test`"]
fn the_twenty_benchmark_queries_over_the_target_fixtures_give_the_same_answers_sharded_and_whole() {
    let fx = support::fixtures::target();
    let deny = DenyMatcher::default();
    let mut files: Vec<File> = Vec::new();
    for lane in fx.swimlanes() {
        let swimlane = SwimlaneId::parse(&lane).unwrap();
        for (path, data) in fx.files(&lane) {
            let class = engine::classify(&path, Some(&data), &deny);
            if matches!(class, domain::FileClass::Structured | domain::FileClass::Text) {
                files.push((swimlane.clone(), path, ports::content_hash(&data), data));
            }
        }
    }
    let whole = SearchCorpus::from_files(files.iter().cloned()).unwrap();
    assert!(whole.location_count() > 70_000 && whole.blob_count() < whole.location_count());
    let limits = Limits {
        hits_total: 1_000,
        hits_per_file: 50,
        ..Limits::default()
    };
    let lanes: Vec<SwimlaneId> = fx
        .swimlanes()
        .iter()
        .take(3)
        .map(|l| SwimlaneId::parse(l).unwrap())
        .collect();
    let mut cut = 0;
    for shards in [1_usize, 8, 16] {
        let corpora: Vec<SearchCorpus> = (0..shards)
            .map(|s| {
                SearchCorpus::from_files(
                    files
                        .iter()
                        .filter(|(_, _, h, _)| shard_of(h, shards) == s)
                        .cloned(),
                )
                .unwrap()
            })
            .collect();
        for &(pattern, regex, case) in &QUERIES {
            for (total, per_file, swimlanes) in [
                (500_u32, 20_u32, Vec::new()),
                (7, 2, Vec::new()),
                (500, 20, lanes.clone()),
            ] {
                let mut q = query(pattern, regex, case, total, per_file);
                q.swimlanes = swimlanes;
                let want = search_text(&whole, &q, &deny, &limits, pool(), &CancelFlag::new())
                    .unwrap()
                    .results;
                let parts: Vec<TextResults> = corpora
                    .iter()
                    .map(|c| {
                        search_text(c, &q, &deny, &limits, pool(), &CancelFlag::new())
                            .unwrap()
                            .results
                    })
                    .collect();
                let got = merge(parts, effective_total(q.max_total, limits.hits_total));
                cut += usize::from(want.truncated);
                assert_eq!(
                    got, want,
                    "{shards} shards, {pattern:?}, total {total}, per file {per_file}"
                );
            }
        }
    }
    assert!(
        cut > 10,
        "{cut} of the answers were cut: the caps were not exercised"
    );
}
