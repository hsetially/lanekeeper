//! T7 (S11, P7): the text search. It is ripgrep's matcher and searcher over in-memory blobs: literal by default, a linear-time
//! regex with size limits when asked, a case option, caps in total and per file with truncation reported, binary files
//! skipped, line text without the `\r` and the match ranges in bytes of that text. A blob is searched once however many
//! swimlanes hold it, and the answer is the same on a pool of one thread and a pool of eight.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use domain::{FileClass, NfsPath, ShortText, SwimlaneId, TextHit, TextQuery};
use engine::{
    CancelFlag, DenyMatcher, Limits, SearchBlob, SearchCorpus, SearchError, SearchLocation, SearchOutcome,
    normalised_hash, search_text,
};

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

fn lane(name: &str) -> SwimlaneId {
    SwimlaneId::parse(name).unwrap()
}

fn path(p: &str) -> NfsPath {
    NfsPath::parse(p).unwrap()
}

fn loc(swimlane: &str, p: &str) -> SearchLocation {
    SearchLocation {
        swimlane: lane(swimlane),
        path: path(p),
    }
}

fn blob(content: impl Into<Vec<u8>>, locations: &[(&str, &str)]) -> SearchBlob {
    let content = Bytes::from(content.into());
    SearchBlob {
        hash: normalised_hash(&content, FileClass::Binary),
        content,
        locations: locations.iter().map(|(s, p)| loc(s, p)).collect(),
    }
}

fn corpus(blobs: Vec<SearchBlob>) -> SearchCorpus {
    SearchCorpus::new(blobs).unwrap()
}

fn query(pattern: &str) -> TextQuery {
    TextQuery {
        pattern: pattern.to_owned(),
        regex: false,
        case_sensitive: true,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total: TextQuery::DEFAULT_MAX_TOTAL,
        max_per_file: TextQuery::DEFAULT_MAX_PER_FILE,
    }
}

fn run_with(
    corpus: &SearchCorpus,
    q: &TextQuery,
    limits: &Limits,
    threads: usize,
) -> Result<SearchOutcome, SearchError> {
    search_text(
        corpus,
        q,
        &DenyMatcher::default(),
        limits,
        &pool(threads),
        &CancelFlag::new(),
    )
}

fn run(corpus: &SearchCorpus, q: &TextQuery) -> SearchOutcome {
    run_with(corpus, q, &Limits::default(), 2).unwrap()
}

/// `swimlane:path:line` of each hit, in order.
fn places(outcome: &SearchOutcome) -> Vec<String> {
    outcome
        .results
        .hits
        .iter()
        .map(|h| format!("{}:{}:{}", h.swimlane, h.path, h.line))
        .collect()
}

fn slice(hit: &TextHit, range: (u32, u32)) -> &str {
    &hit.text[range.0 as usize..range.1 as usize]
}

#[test]
fn literal_default() {
    let c = corpus(vec![blob(
        "abc\na.c\nA.C\nplain a.c in a line\n",
        &[("sit1", "svc/a.yml")],
    )]);
    // A dot is a dot: the pattern is not a regex unless asked for.
    let found = run(&c, &query("a.c"));
    assert_eq!(places(&found), ["sit1:svc/a.yml:2", "sit1:svc/a.yml:4"]);
    assert!(!found.results.truncated);
    let hit = &found.results.hits[1];
    assert_eq!(hit.text, "plain a.c in a line");
    assert_eq!(hit.ranges, [(6, 9)]);
    assert_eq!(slice(hit, hit.ranges[0]), "a.c");
    // Regex metacharacters of every kind are literal text.
    let c = corpus(vec![blob("x(y)*[z]+?{1}|^$\\\n", &[("sit1", "a.txt")])]);
    for pattern in ["(y)", "*[z]", "+?{1}", "|^$\\", "x(y)*[z]+?{1}|^$\\"] {
        assert_eq!(run(&c, &query(pattern)).results.hits.len(), 1, "{pattern}");
    }
}

#[test]
fn regex_option() {
    let c = corpus(vec![blob(
        "timeout: 30\ntimeout: abc\nretries: 5\nTimeout: 7\n",
        &[("sit1", "svc/a.yml")],
    )]);
    let mut q = query(r"^timeout: \d+$");
    q.regex = true;
    let found = run(&c, &q);
    assert_eq!(places(&found), ["sit1:svc/a.yml:1"]);
    assert_eq!(found.results.hits[0].ranges, [(0, 11)]);
    // Alternation and a word boundary.
    q.pattern = r"\b(?:retries|abc)\b".to_owned();
    let found = run(&c, &q);
    assert_eq!(places(&found), ["sit1:svc/a.yml:2", "sit1:svc/a.yml:3"]);
    // Several matches in one line are several ranges.
    let c = corpus(vec![blob("a1 b22 c333\n", &[("sit1", "a.txt")])]);
    q.pattern = r"\d+".to_owned();
    assert_eq!(run(&c, &q).results.hits[0].ranges, [(1, 2), (4, 6), (8, 11)]);
}

#[test]
fn case_option() {
    let c = corpus(vec![blob(
        "Timeout\ntimeout\nTIMEOUT\nTimeOut\n",
        &[("sit1", "a.txt")],
    )]);
    let mut q = query("timeout");
    assert_eq!(run(&c, &q).results.hits.len(), 1);
    q.case_sensitive = false;
    assert_eq!(run(&c, &q).results.hits.len(), 4);
    // The option applies to a regex too, and to non-ASCII letters.
    q.regex = true;
    q.pattern = "time(out)".to_owned();
    assert_eq!(run(&c, &q).results.hits.len(), 4);
    let c = corpus(vec![blob(
        "Straße\nSTRASSE\nstraße\nSTRAẞE\n",
        &[("sit1", "a.txt")],
    )]);
    let mut q = query("straße");
    q.case_sensitive = false;
    assert_eq!(
        places(&run(&c, &q)),
        ["sit1:a.txt:1", "sit1:a.txt:3", "sit1:a.txt:4"]
    );
}

#[test]
fn total_and_per_file_caps_report_truncation() {
    let many = "hit\n".repeat(30);
    let c = corpus(vec![
        blob(many.clone(), &[("sit1", "a.yml")]),
        blob(format!("{many}x\n"), &[("sit1", "b.yml")]),
        blob("one hit\n", &[("sit1", "c.yml")]),
    ]);
    // Per file: 20 of 30, and the file is marked truncated.
    let found = run(&c, &query("hit"));
    assert!(found.results.truncated);
    let per_file = |name: &str| {
        found
            .results
            .hits
            .iter()
            .filter(|h| h.path.as_str() == name)
            .count()
    };
    assert_eq!(
        (per_file("a.yml"), per_file("b.yml"), per_file("c.yml")),
        (20, 20, 1)
    );
    // Exactly at the per-file cap is not truncated.
    let at_cap = corpus(vec![blob("hit\n".repeat(20), &[("sit1", "a.yml")])]);
    assert!(!run(&at_cap, &query("hit")).results.truncated);
    let over = corpus(vec![blob("hit\n".repeat(21), &[("sit1", "a.yml")])]);
    let found = run(&over, &query("hit"));
    assert!(found.results.truncated);
    assert_eq!(found.results.hits.len(), 20);
    // Total: the first hits in (swimlane, path, line) order, then truncated.
    let mut q = query("hit");
    q.max_total = 25;
    let found = run(&c, &q);
    assert!(found.results.truncated);
    assert_eq!(found.results.hits.len(), 25);
    assert_eq!(found.results.hits[19].path.as_str(), "a.yml");
    assert_eq!(found.results.hits[20].path.as_str(), "b.yml");
    // Exactly the total of what exists is not truncated.
    let mut q = query("hit");
    q.max_total = 3;
    let few = corpus(vec![blob("hit\nhit\nhit\n", &[("sit1", "a.yml")])]);
    assert!(!run(&few, &q).results.truncated);
    q.max_total = 2;
    let found = run(&few, &q);
    assert!(found.results.truncated);
    assert_eq!(found.results.hits.len(), 2);
}

#[test]
fn the_query_caps_are_held_to_the_limits() {
    // A caller cannot ask for more than the limits allow: the result set is bounded whatever the query says.
    let c = corpus(vec![blob("hit\n".repeat(50), &[("sit1", "a.yml")])]);
    let mut q = query("hit");
    q.max_total = u32::MAX;
    q.max_per_file = u32::MAX;
    let limits = Limits {
        hits_total: 7,
        hits_per_file: 5,
        ..Limits::default()
    };
    let found = run_with(&c, &q, &limits, 2).unwrap();
    assert_eq!(found.results.hits.len(), 5);
    assert!(found.results.truncated);
    // Zero asks for nothing, and says that there was something.
    q.max_per_file = 0;
    let found = run(&c, &q);
    assert!(found.results.hits.is_empty());
    assert!(found.results.truncated);
    let nothing = run(&c, &query("absent"));
    assert!(nothing.results.hits.is_empty() && !nothing.results.truncated);
}

#[test]
fn binary_skipped() {
    let mut with_nul = b"find me\n".to_vec();
    with_nul.extend_from_slice(b"\0\x01\x02 find me\n");
    let c = corpus(vec![
        blob(with_nul.clone(), &[("sit1", "svc/data.yml")]),
        blob(with_nul, &[("sit1", "svc/receipt.bmp")]),
        blob("find me\n", &[("sit1", "svc/receipt-ci_1.bmp")]),
        blob(
            "find me\n",
            &[("sit1", "certs/server.pem"), ("sit1", "svc/private-notes.txt")],
        ),
        blob("find me\n", &[("sit1", "svc/ok.yml")]),
        // A byte order mark that declares UTF-16 over bytes that are not UTF-16.
        blob(b"\xFF\xFEfind me!!".to_vec(), &[("sit1", "svc/bad16.txt")]),
        // Not UTF-8 after a UTF-8 mark.
        blob(
            b"\xEF\xBB\xBFfind me \xFF\n".to_vec(),
            &[("sit1", "svc/bad8.txt")],
        ),
    ]);
    let found = run(&c, &query("find me"));
    // Only the plain text file: a NUL makes a file binary, a binary extension and a denied name are never opened, and a
    // mark that the bytes do not honour is not text.
    assert_eq!(places(&found), ["sit1:svc/ok.yml:1"]);
    assert_eq!(found.stats.blobs_binary, 3, "{:?}", found.stats);
    assert_eq!(found.stats.blobs_searched, 1);
}

#[test]
fn text_has_no_cr_and_ranges_valid() {
    let c = corpus(vec![blob(
        "key: value\r\nother: thing value\r\nlast: value",
        &[("sit1", "svc/crlf.yml")],
    )]);
    let found = run(&c, &query("value"));
    assert_eq!(found.results.hits.len(), 3);
    for hit in &found.results.hits {
        assert!(!hit.text.contains(['\r', '\n']), "{:?}", hit.text);
        for &(s, e) in &hit.ranges {
            assert!(s < e && e as usize <= hit.text.len());
            assert_eq!(slice(hit, (s, e)), "value");
        }
    }
    let lines: Vec<u32> = found.results.hits.iter().map(|h| h.line).collect();
    assert_eq!(lines, [1, 2, 3]);
    // `$` is the end of the line in a CRLF file, and `^` the start.
    let mut q = query("value$");
    q.regex = true;
    assert_eq!(run(&c, &q).results.hits.len(), 3);
    q.pattern = "^other".to_owned();
    assert_eq!(places(&run(&c, &q)), ["sit1:svc/crlf.yml:2"]);
    // The same in a file with bare `\n` endings.
    let lf = corpus(vec![blob(
        "key: value\nother: thing value\n",
        &[("sit1", "svc/lf.yml")],
    )]);
    q.pattern = "value$".to_owned();
    assert_eq!(run(&lf, &q).results.hits.len(), 2);
    // A lone `\r` ends a line too, as it does for the parsers, and mixed endings are lines all the same.
    let c = corpus(vec![blob(
        "a\rvalue\rb\r\nvalue\n\nvalue",
        &[("sit1", "svc/cr.txt")],
    )]);
    let found = run(&c, &query("value"));
    let seen: Vec<(u32, &str)> = found
        .results
        .hits
        .iter()
        .map(|h| (h.line, h.text.as_str()))
        .collect();
    assert_eq!(seen, [(2, "value"), (4, "value"), (6, "value")]);
    q.pattern = "value$".to_owned();
    assert_eq!(run(&c, &q).results.hits.len(), 3);
}

#[test]
fn dedup_blob_fans_out_to_every_location() {
    // One blob held by three paths in two swimlanes is searched once and reported at each place, in order.
    let c = corpus(vec![
        blob(
            "needle one\nnothing\nneedle two\n",
            &[
                ("sit2", "svc/b.yml"),
                ("sit1", "svc/b.yml"),
                ("sit1", "svc/a.yml"),
            ],
        ),
        blob("needle three\n", &[("sit1", "svc/c.yml")]),
    ]);
    let found = run(&c, &query("needle"));
    assert_eq!(
        places(&found),
        [
            "sit1:svc/a.yml:1",
            "sit1:svc/a.yml:3",
            "sit1:svc/b.yml:1",
            "sit1:svc/b.yml:3",
            "sit1:svc/c.yml:1",
            "sit2:svc/b.yml:1",
            "sit2:svc/b.yml:3",
        ]
    );
    assert_eq!(found.stats.blobs_searched, 2);
    // The per-file cap is per place: each of the three paths gets its own.
    let c = corpus(vec![blob(
        "needle\n".repeat(30),
        &[("sit1", "a.yml"), ("sit1", "b.yml")],
    )]);
    let mut q = query("needle");
    q.max_per_file = 10;
    q.max_total = 15;
    let found = run(&c, &q);
    assert_eq!(found.results.hits.len(), 15);
    assert!(found.results.truncated);
    assert_eq!(found.results.hits[10].path.as_str(), "b.yml");
}

#[test]
fn the_swimlane_and_path_filters_narrow_the_search() {
    let text = "value\n";
    let c = corpus(vec![blob(
        text,
        &[
            ("sit1", "core/a.yml"),
            ("sit2", "core/a.yml"),
            ("sit3", "core/a.yml"),
            ("sit1", "core/deep/b.properties"),
            ("sit1", "a.xml"),
        ],
    )]);
    let mut q = query("value");
    q.swimlanes = vec![lane("sit3"), lane("sit1"), lane("sit1")];
    assert_eq!(
        places(&run(&c, &q)),
        [
            "sit1:a.xml:1",
            "sit1:core/a.yml:1",
            "sit1:core/deep/b.properties:1",
            "sit3:core/a.yml:1"
        ]
    );
    q.swimlanes = vec![lane("sit1")];
    // A glob without a slash is matched against the name, at any depth.
    q.path_glob = Some(ShortText::parse("*.yml").unwrap());
    assert_eq!(places(&run(&c, &q)), ["sit1:core/a.yml:1"]);
    q.path_glob = Some(ShortText::parse("a.*").unwrap());
    assert_eq!(places(&run(&c, &q)), ["sit1:a.xml:1", "sit1:core/a.yml:1"]);
    // A glob with a slash is matched against the whole path.
    q.path_glob = Some(ShortText::parse("core/**").unwrap());
    assert_eq!(
        places(&run(&c, &q)),
        ["sit1:core/a.yml:1", "sit1:core/deep/b.properties:1"]
    );
    q.path_glob = Some(ShortText::parse("core/*.yml").unwrap());
    assert_eq!(places(&run(&c, &q)), ["sit1:core/a.yml:1"]);
    // An empty glob is no glob; a broken one is an error that does not repeat it.
    q.path_glob = Some(ShortText::parse("").unwrap());
    assert_eq!(run(&c, &q).results.hits.len(), 3);
    q.path_glob = Some(ShortText::parse("core/[a-").unwrap());
    let err = run_with(&c, &q, &Limits::default(), 2).unwrap_err();
    assert_eq!(err, SearchError::InvalidGlob);
    assert!(!format!("{err} {err:?}").contains("core"));
    // A swimlane that holds nothing gives nothing.
    q.path_glob = None;
    q.swimlanes = vec![lane("sit9")];
    assert!(run(&c, &q).results.hits.is_empty());
}

#[test]
fn long_lines_are_cut_on_a_character_boundary() {
    // 511 ASCII bytes, then a two-byte letter that straddles the 512-byte cut, then the match.
    let line = format!("{}é{}", "a".repeat(511), "needle");
    let c = corpus(vec![blob(format!("{line}\n"), &[("sit1", "a.txt")])]);
    let hit = &run(&c, &query("needle")).results.hits[0];
    assert_eq!(hit.text.len(), 511, "cut back to the boundary before the letter");
    assert!(hit.ranges.is_empty(), "the match is past the cut");
    // A match that starts before the cut and ends after it is cut with the line.
    let line = format!("{}needle and more", "a".repeat(508));
    let c = corpus(vec![blob(format!("{line}\n"), &[("sit1", "a.txt")])]);
    let hit = &run(&c, &query("needle")).results.hits[0];
    assert_eq!(hit.text.len(), 512);
    assert_eq!(hit.ranges, [(508, 512)]);
    // A line of 512 bytes is whole.
    let c = corpus(vec![blob(format!("{}\n", "n".repeat(512)), &[("sit1", "a.txt")])]);
    assert_eq!(run(&c, &query("n")).results.hits[0].text.len(), 512);
    // The cut is the limit's.
    let limits = Limits {
        hit_line_bytes: 10,
        ..Limits::default()
    };
    let c = corpus(vec![blob("0123456789abcdef\n", &[("sit1", "a.txt")])]);
    let hit = &run_with(&c, &query("a"), &limits, 1).unwrap().results.hits[0];
    assert_eq!(hit.text, "0123456789");
    // Many matches in a line are capped; the ranges are still in order and in the text.
    let c = corpus(vec![blob(
        format!("{}\n", "a ".repeat(200)),
        &[("sit1", "a.txt")],
    )]);
    let hit = &run(&c, &query("a")).results.hits[0];
    assert!(hit.ranges.len() <= 64 && hit.ranges.len() >= 2);
    assert!(hit.ranges.windows(2).all(|w| w[0].1 <= w[1].0));
    assert!(hit.ranges.iter().all(|&(_, e)| e as usize <= hit.text.len()));
}

#[test]
fn other_encodings_are_searched_as_text() {
    let utf16: Vec<u8> = "\u{feff}key: wert\r\nname: Müller\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let latin1 = b"name: M\xFCller\nkey: wert\n".to_vec();
    let bom = "\u{feff}name: Müller\n".as_bytes().to_vec();
    let c = corpus(vec![
        blob(utf16, &[("sit1", "utf16.txt")]),
        blob(latin1, &[("sit1", "latin1.txt")]),
        blob(bom, &[("sit1", "bom.yml")]),
    ]);
    let found = run(&c, &query("Müller"));
    assert_eq!(
        places(&found),
        ["sit1:bom.yml:1", "sit1:latin1.txt:1", "sit1:utf16.txt:2"]
    );
    // The mark is not part of the first line.
    assert_eq!(found.results.hits[0].text, "name: Müller");
    assert_eq!(found.results.hits[2].text, "name: Müller");
    let mut q = query("^key");
    q.regex = true;
    assert_eq!(places(&run(&c, &q)), ["sit1:latin1.txt:2", "sit1:utf16.txt:1"]);
}

#[test]
fn an_extensionless_text_file_is_searched_and_a_big_file_is_not() {
    let c = corpus(vec![
        blob("needle\n", &[("sit1", "bin/run")]),
        blob(b"needle\0".to_vec(), &[("sit1", "bin/other")]),
    ]);
    assert_eq!(places(&run(&c, &query("needle"))), ["sit1:bin/run:1"]);
    let limits = Limits {
        input_bytes: 16,
        ..Limits::default()
    };
    let c = corpus(vec![
        blob("needle\n", &[("sit1", "small.txt")]),
        blob("needle\n".repeat(3), &[("sit1", "big.txt")]),
    ]);
    let found = run_with(&c, &query("needle"), &limits, 1).unwrap();
    assert_eq!(places(&found), ["sit1:small.txt:1"]);
    assert_eq!(found.stats.blobs_oversized, 1);
}

#[test]
fn a_pattern_is_checked_before_any_work() {
    let c = corpus(vec![blob("x\n", &[("sit1", "a.txt")])]);
    let limits = Limits::default();
    let err = |q: &TextQuery| run_with(&c, q, &limits, 1).unwrap_err();
    assert_eq!(err(&query("")), SearchError::EmptyPattern);
    assert_eq!(err(&query(&"a".repeat(513))), SearchError::PatternTooLong);
    assert!(run_with(&c, &query(&"a".repeat(512)), &limits, 1).is_ok());
    // A line break cannot be searched for in a line search.
    assert_eq!(err(&query("a\nb")), SearchError::InvalidPattern);
    let mut q = query("a\\nb");
    q.regex = true;
    assert_eq!(err(&q), SearchError::InvalidPattern);
    q.pattern = "(unclosed".to_owned();
    assert_eq!(err(&q), SearchError::InvalidPattern);
    // The error never repeats the pattern.
    let secret = "SECRET-MARKER-9d2f(";
    let mut q = query(secret);
    q.regex = true;
    let e = err(&q);
    assert!(!format!("{e} {e:?}").contains("SECRET"));
}

mod regex_limits {
    use super::*;

    #[test]
    fn pattern_over_512_bytes_rejected() {
        let c = corpus(vec![blob("x\n", &[("sit1", "a.txt")])]);
        let mut q = query(&"a|".repeat(256));
        q.regex = true;
        assert_eq!(q.pattern.len(), 512);
        assert!(run_with(&c, &q, &Limits::default(), 1).is_ok());
        q.pattern.push('a');
        assert_eq!(
            run_with(&c, &q, &Limits::default(), 1).unwrap_err(),
            SearchError::PatternTooLong
        );
        // The limit is the one in `Limits`.
        let tight = Limits {
            regex_pattern_bytes: 8,
            ..Limits::default()
        };
        q.pattern = "abcdefgh".to_owned();
        assert!(run_with(&c, &q, &tight, 1).is_ok());
        q.pattern = "abcdefghi".to_owned();
        assert_eq!(
            run_with(&c, &q, &tight, 1).unwrap_err(),
            SearchError::PatternTooLong
        );
    }

    #[test]
    fn size_limit_and_dfa_limit_enforced() {
        let c = corpus(vec![blob("aaaa\n", &[("sit1", "a.txt")])]);
        let mut q = query("[a-z]{1000}");
        q.regex = true;
        // Compiles under the default size limit...
        assert!(run_with(&c, &q, &Limits::default(), 1).is_ok());
        // ...and is refused under a small one, with an error that does not say why in the pattern's words.
        let small = Limits {
            regex_size_limit: 10 * 1024,
            ..Limits::default()
        };
        assert_eq!(
            run_with(&c, &q, &small, 1).unwrap_err(),
            SearchError::InvalidPattern
        );
        // A longer one is over the default too.
        q.pattern = "(?:[a-z]{1000}){30}".to_owned();
        assert_eq!(
            run_with(&c, &q, &Limits::default(), 1).unwrap_err(),
            SearchError::InvalidPattern
        );
        // The DFA limit bounds memory while searching, not the pattern: a tiny cache still gives the right answer.
        let tiny = Limits {
            regex_dfa_size_limit: 1,
            ..Limits::default()
        };
        let c = corpus(vec![blob("foo bar baz\nqux\nfoo\n", &[("sit1", "a.txt")])]);
        q.regex = true;
        q.pattern = r"\bfoo\b|b\w+z".to_owned();
        let small = run_with(&c, &q, &tiny, 1);
        let normal = run_with(&c, &q, &Limits::default(), 1).unwrap();
        match small {
            Ok(found) => assert_eq!(found.results, normal.results),
            Err(e) => assert_eq!(e, SearchError::InvalidPattern),
        }
        assert_eq!(normal.results.hits.len(), 2);
    }

    #[test]
    fn nest_limit_enforced() {
        let c = corpus(vec![blob("x\n", &[("sit1", "a.txt")])]);
        let nested = |depth: usize| format!("{}x{}", "(".repeat(depth), ")".repeat(depth));
        let mut q = query(&nested(50));
        q.regex = true;
        assert!(run_with(&c, &q, &Limits::default(), 1).is_ok());
        q.pattern = nested(51);
        assert_eq!(
            run_with(&c, &q, &Limits::default(), 1).unwrap_err(),
            SearchError::InvalidPattern
        );
    }

    #[test]
    fn catastrophic_pattern_runs_linear() {
        // A backtracking engine would take longer than the age of the universe on these; this one reads the line once.
        let long = format!("{}b\n", "a".repeat(200_000));
        let c = corpus(vec![blob(long, &[("sit1", "a.txt")])]);
        let limits = Limits {
            input_bytes: 4 * 1024 * 1024,
            ..Limits::default()
        };
        for pattern in ["(a+)+$", "(a|aa)+$", "(a*)*b$", "(x+x+)+y", "(a|a)*c"] {
            let mut q = query(pattern);
            q.regex = true;
            let found = run_with(&c, &q, &limits, 1).unwrap();
            assert!(found.results.hits.len() <= 1, "{pattern}");
        }
    }
}

#[test]
fn cancel_flag_stops_work() {
    let blobs: Vec<SearchBlob> = (0..200)
        .map(|i| blob(format!("needle {i}\n"), &[("sit1", &format!("f{i:03}.txt"))]))
        .collect();
    let c = corpus(blobs);
    let flag = CancelFlag::new();
    assert!(!flag.is_cancelled());
    let q = query("needle");
    let found = search_text(
        &c,
        &q,
        &DenyMatcher::default(),
        &Limits::default(),
        &pool(2),
        &flag,
    )
    .unwrap();
    assert_eq!(found.results.hits.len(), 200);
    // Cancelled before the start: no work is done, and no partial answer is given.
    flag.cancel();
    assert!(flag.is_cancelled());
    let err = search_text(
        &c,
        &q,
        &DenyMatcher::default(),
        &Limits::default(),
        &pool(2),
        &flag,
    )
    .unwrap_err();
    assert_eq!(err, SearchError::Cancelled);
    // A bad query is still reported as a bad query, so that a caller does not retry it.
    let bad = search_text(
        &c,
        &query(""),
        &DenyMatcher::default(),
        &Limits::default(),
        &pool(1),
        &flag,
    );
    assert!(matches!(
        bad,
        Err(SearchError::EmptyPattern | SearchError::Cancelled)
    ));
}

#[test]
fn the_answer_does_not_depend_on_the_pool() {
    // Three swimlanes of 60 files with one shared and many distinct blobs: the same bytes whatever the thread count.
    let mut blobs = Vec::new();
    let mut shared_locations = Vec::new();
    for s in 0..3 {
        for f in 0..60 {
            shared_locations.push((format!("sit{s}"), format!("svc{}/file{f:02}.yml", f % 4)));
        }
    }
    let shared_refs: Vec<(&str, &str)> = shared_locations
        .iter()
        .map(|(s, p)| (s.as_str(), p.as_str()))
        .collect();
    blobs.push(blob("hit a\nmiss\nhit b\n", &shared_refs));
    for f in 0..300 {
        blobs.push(blob(
            format!("line\nhit {f}\n{}\nhit again\n", "x".repeat(f)),
            &[("sit1", &format!("own/{f:03}.txt"))],
        ));
    }
    let c = corpus(blobs);
    let mut q = query("hit");
    q.max_total = 150;
    let reference = run_with(&c, &q, &Limits::default(), 1).unwrap();
    assert_eq!(reference.results.hits.len(), 150);
    assert!(reference.results.truncated);
    for threads in [2, 3, 8] {
        let got = run_with(&c, &q, &Limits::default(), threads).unwrap();
        assert_eq!(got.results, reference.results, "{threads} threads");
        assert_eq!(got.stats, reference.stats, "{threads} threads");
    }
}

#[test]
fn the_debug_output_of_a_blob_has_no_content() {
    let b = blob("SECRET-MARKER-4a7c: hunter2\n", &[("sit1", "a.yml")]);
    assert!(!format!("{b:?}").contains("SECRET"));
    let c = corpus(vec![b]);
    assert!(!format!("{c:?}").contains("SECRET"));
}

#[test]
fn from_files_groups_equal_content_into_one_blob() {
    let a = Bytes::from_static(b"needle\n");
    let b = Bytes::from_static(b"other needle\n");
    let h = |d: &Bytes| normalised_hash(d, FileClass::Binary);
    let files = vec![
        (lane("sit2"), path("x.txt"), h(&a), a.clone()),
        (lane("sit1"), path("x.txt"), h(&a), a.clone()),
        (lane("sit1"), path("y.txt"), h(&b), b.clone()),
    ];
    let c = SearchCorpus::from_files(files).unwrap();
    assert_eq!(c.blob_count(), 2);
    assert_eq!(c.location_count(), 3);
    assert_eq!(
        places(&run(&c, &query("needle"))),
        ["sit1:x.txt:1", "sit1:y.txt:1", "sit2:x.txt:1"]
    );
}
