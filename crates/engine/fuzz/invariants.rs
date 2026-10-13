//! The properties the engine's fuzz targets assert (S11, S21, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/engine/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify` runs) check
//! exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a panic, which
//! libFuzzer reports as a crash.
//!
//! A refusal is never a violation: the parsers are meant to say no to most of what a fuzzer sends. The checks look at what
//! was accepted, at whether the answer is the same when asked twice, and at the limits.
//!
//! # The targets
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `engine_bytes` | any bytes | `detect_format`, `classify`, `normalise_eol`, `apply_eol` and `decode_text` never panic; normalising twice is normalising once and never makes the data longer; LF to CRLF and back is the identity on normalised data; bytes that decode are written back by `encode_text` to the very same bytes, in the format `detect_format` named |
//! | `engine_yaml` | text | `parse` and `flatten` never panic, finish within the limits and give the same answer twice; what is accepted is a graph without cycles whose positions are inside the text; every setting path is unique, within the limits, and survives decode and encode |
//! | `engine_json` | text | the same for JSON |
//! | `engine_properties` | text | the same for `.properties` (a path is the key as written) |
//! | `engine_index_settings` | a selector byte (YAML, JSON or `.properties`), then any bytes | `index_settings` and `locate` never panic and give the same answer twice; the index is there exactly when `validate` accepts the bytes, with the same settings; every range is inside the text; a YAML file is located by tree-sitter or says that it could not be, and every disagreement names a setting of the file |
//! | `engine_path_mapping` | NUL-separated pieces: an NFS path, tenant ids, a repository path, then merge-rule globs | `reverse_map`, `classify_path`, `map_path` and `MergeRules` never panic; a path that reads back to Git maps forward to the very same NFS path, under the root of its repository; a repository path maps to an NFS path that reads back to the same tenant file; the kind and the tenant of `classify_path` agree with the mapping; a merge rule list gives the same mode as asking each rule alone, first match first |
//! | `engine_flatten` | NUL-separated pieces | a path built from any keys and indexes is read back to the same keys and indexes; any text offered as a setting path is refused or read to something that writes and reads back the same |
//! | `engine_regex` | a flags byte (regex or literal, case, small caps), a pattern, a NUL, then the text searched | `search_text` never panics, finishes within the (small) regex limits and gives the same answer twice; an empty or over-long pattern is refused as such; every hit is within the caps, in swimlane, path and line order, with line text of at most the cut length that has no `\r` or `\n`, and ranges that are in order, not empty, inside the text and on character boundaries; a binary or over-long text gives no hits; for a literal, case-sensitive pattern in UTF-8 text the hits are exactly the lines that hold the pattern and the ranges exactly its occurrences |
//! | `engine_secret_scan` | a flags byte (hex, shorter and looser thresholds), then text | `secret_scan` never panics and gives the same answer twice; every match is a non-empty span inside the text, in order and not overlapping, whose length counts its characters; its marker is `[REDACTED: N chars, rule]`; and replacing every span by its marker leaves a text in which the scan finds nothing, so what the scan reports is all of what it would report |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use std::collections::BTreeSet;

use bytes::Bytes;
use domain::{
    FileClass, FileKind, LogicalFile, MergeMode, NfsPath, PathMappingRule, RepoPath, ReverseMapping,
    SettingPath, SwimlaneId, TenantId, TenantSet, TextHit, TextQuery, ValueFlag, ValueType,
};
use engine::{
    Body, CancelFlag, DenyMatcher, Flat, Limits, LocationSource, MergeRules, Newline, NodeKind, ParseError,
    ParseWarning, Parsed, PathSegment, RepoSource, ScanConfig, ScanError, SearchBlob, SearchCorpus,
    SearchError, SearchLocation, SearchOutcome, SecretMatch, Syntax, apply_eol, classify, classify_path,
    decode_setting_path, decode_text, detect_format, encode_setting_path, encode_text, flatten,
    index_settings, locate, map_path, normalise_eol, normalised_hash, parse, reverse_map, search_text,
    secret_scan, validate,
};

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory) and its check.
pub const TARGETS: [(&str, Check); 9] = [
    ("engine_bytes", engine_bytes),
    ("engine_yaml", engine_yaml),
    ("engine_json", engine_json),
    ("engine_properties", engine_properties),
    ("engine_flatten", engine_flatten),
    ("engine_index_settings", engine_index_settings),
    ("engine_path_mapping", engine_path_mapping),
    ("engine_regex", engine_regex),
    ("engine_secret_scan", engine_secret_scan),
];

/// The least number of seeds in each corpus directory: valid, truncated, oversized, alias bomb, deep nesting, wrong
/// encoding, hostile keys, and one more.
pub const MIN_SEEDS: usize = 8;

/// Small limits, so that the boundaries are easy to reach and every input finishes quickly.
pub fn fuzz_limits() -> Limits {
    Limits {
        input_bytes: 4 * 1024,
        nesting_depth: 16,
        nodes: 2_000,
        settings: 1_000,
        scalar_bytes: 512,
        setting_path_bytes: 128,
        warnings: 50,
        ..Limits::default()
    }
}

fn ensure(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what()) }
}

// --- engine_bytes ---

pub fn engine_bytes(data: &[u8]) -> Result<(), String> {
    let limits = fuzz_limits();
    let data_bytes = Bytes::copy_from_slice(data);
    let format = detect_format(data);

    let path = NfsPath::parse("some/file.yml").unwrap();
    let _ = classify(&path, Some(data), &DenyMatcher::default());

    let normalised = normalise_eol(&data_bytes);
    ensure(normalised.len() <= data.len(), || {
        "normalising made the data longer".into()
    })?;
    ensure(normalise_eol(&normalised) == normalised, || {
        "normalising twice differs from once".into()
    })?;
    for newline in [Newline::Lf, Newline::Crlf] {
        let styled = apply_eol(&normalised, newline);
        ensure(normalise_eol(&styled) == normalised, || {
            format!("{newline:?} and back is not the identity")
        })?;
        ensure(apply_eol(&styled, newline) == styled, || {
            format!("applying {newline:?} twice differs from once")
        })?;
    }

    if let Ok(decoded) = decode_text(data, &limits) {
        ensure(decoded.format == format, || {
            "decode_text and detect_format disagree".into()
        })?;
        let back = encode_text(&decoded.text, decoded.format, &limits)
            .map_err(|e| format!("bytes that decode cannot be written back: {e}"))?;
        ensure(back.as_ref() == data, || {
            "bytes that decode are not written back unchanged".into()
        })?;
    }
    Ok(())
}

// --- the parsers ---

fn lines_in(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut lines = 1_u32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                lines += 1;
                i += 2;
            }
            b'\n' | b'\r' => {
                lines += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    lines
}

fn text_of(data: &[u8]) -> Option<&str> {
    std::str::from_utf8(data).ok()
}

pub fn engine_yaml(data: &[u8]) -> Result<(), String> {
    text_of(data).map_or(Ok(()), |text| parser_check(text, Syntax::Yaml))
}

pub fn engine_json(data: &[u8]) -> Result<(), String> {
    text_of(data).map_or(Ok(()), |text| parser_check(text, Syntax::Json))
}

pub fn engine_properties(data: &[u8]) -> Result<(), String> {
    text_of(data).map_or(Ok(()), |text| parser_check(text, Syntax::Properties))
}

fn parser_check(text: &str, syntax: Syntax) -> Result<(), String> {
    let limits = fuzz_limits();
    let total_lines = lines_in(text);
    let first = parse(text, syntax, &limits);
    ensure(first == parse(text, syntax, &limits), || {
        "parsing twice gives different answers".into()
    })?;
    let parsed = match first {
        Ok(parsed) => parsed,
        Err(error) => return error_is_sane(error, total_lines),
    };
    check_parsed(&parsed, total_lines, &limits)?;

    let flat = flatten(&parsed, &limits);
    ensure(flat == flatten(&parsed, &limits), || {
        "flattening twice gives different answers".into()
    })?;
    match flat {
        Ok(flat) => check_flat(&flat, syntax, total_lines, &limits),
        Err(ParseError::Limit(_)) => Ok(()),
        Err(other) => Err(format!(
            "flatten failed with something other than a limit: {other:?}"
        )),
    }
}

fn error_is_sane(error: ParseError, total_lines: u32) -> Result<(), String> {
    match error {
        ParseError::Syntax { line, .. } | ParseError::UnknownAlias { line } => {
            ensure(line >= 1 && line <= total_lines + 2, || {
                format!("an error names line {line} in a text of {total_lines} lines")
            })
        }
        ParseError::Limit(_) => Ok(()),
        ParseError::Decode(_) => Err("parse reported a decode error".into()),
    }
}

fn check_parsed(parsed: &Parsed, total_lines: u32, limits: &Limits) -> Result<(), String> {
    ensure(parsed.warnings.len() <= limits.warnings, || {
        "more warnings than the cap".into()
    })?;
    match &parsed.body {
        Body::Documents(documents) => {
            for document in documents {
                ensure(document.get(document.root()).is_some(), || {
                    "the root is not a node".into()
                })?;
                for (id, node) in document.iter() {
                    ensure(node.line >= 1 && node.end_line >= node.line, || {
                        format!("node {id:?}: lines {}..{}", node.line, node.end_line)
                    })?;
                    ensure(node.end_line <= total_lines + 1, || {
                        format!("node {id:?} ends on line {} of {total_lines}", node.end_line)
                    })?;
                    // A node refers only to nodes finished before it: the graph has no cycle.
                    let earlier = |other: engine::NodeId| -> Result<(), String> {
                        ensure(other < id && document.get(other).is_some(), || {
                            format!("node {id:?} refers to {other:?}")
                        })
                    };
                    match &node.kind {
                        NodeKind::Scalar(s) => ensure(s.text().len() <= limits.scalar_bytes, || {
                            "a scalar over the limit".into()
                        })?,
                        NodeKind::Sequence(items) => items.iter().try_for_each(|&item| earlier(item))?,
                        NodeKind::Mapping(entries) => entries.iter().try_for_each(|e| {
                            earlier(e.key)?;
                            earlier(e.value)
                        })?,
                        NodeKind::Alias(target) => earlier(*target)?,
                    }
                }
            }
        }
        Body::Properties(entries) => {
            for entry in entries {
                ensure(entry.line >= 1 && entry.end_line >= entry.line, || {
                    "bad lines on a pair".into()
                })?;
                ensure(entry.value().len() <= limits.scalar_bytes, || {
                    "a value over the limit".into()
                })?;
            }
        }
    }
    Ok(())
}

fn check_flat(flat: &Flat, syntax: Syntax, total_lines: u32, limits: &Limits) -> Result<(), String> {
    ensure(flat.settings.len() <= limits.settings, || {
        "more settings than the limit".into()
    })?;
    ensure(flat.warnings.len() <= limits.warnings, || {
        "more warnings than the cap".into()
    })?;
    let mut seen = BTreeSet::new();
    for s in &flat.settings {
        let path = s.path.as_str();
        ensure(seen.insert(path.to_owned()), || {
            format!("the path {path:?} appears twice")
        })?;
        ensure(path.len() <= limits.setting_path_bytes, || {
            "a path over the limit".into()
        })?;
        ensure(!path.chars().any(char::is_control), || {
            format!("a control character in the path {path:?}")
        })?;
        ensure(s.lines.start() >= 1 && s.lines.end() <= total_lines + 1, || {
            format!(
                "{path}: lines {}..{} in a text of {total_lines}",
                s.lines.start(),
                s.lines.end()
            )
        })?;
        ensure(s.value.text.len() <= limits.scalar_bytes.max(2), || {
            format!("{path}: a value over the limit")
        })?;
        if s.value.flag == Some(ValueFlag::YamlBooleanLike) {
            ensure(
                syntax == Syntax::Yaml
                    && s.value.ty == ValueType::String
                    && matches!(
                        s.value.text.to_ascii_lowercase().as_str(),
                        "yes" | "no" | "on" | "off"
                    ),
                || format!("{path}: a flag on something that is not yes, no, on or off"),
            )?;
        }
        if syntax == Syntax::Properties {
            continue;
        }
        let decoded =
            decode_setting_path(&s.path).ok_or_else(|| format!("the path {path:?} cannot be read back"))?;
        ensure(
            encode_setting_path(decoded.document, &decoded.segments) == path,
            || format!("the path {path:?} does not write back to itself"),
        )?;
    }
    Ok(())
}

// --- engine_regex ---

/// The pool every search of the fuzzer runs on: one thread, built once.
fn pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("a pool of one thread")
    })
}

/// Small regex limits and a short line cut, so that the boundaries are easy to reach.
pub fn search_limits() -> Limits {
    Limits {
        regex_size_limit: 64 * 1024,
        regex_dfa_size_limit: 256 * 1024,
        regex_nest_limit: 8,
        hit_line_bytes: 64,
        ..fuzz_limits()
    }
}

const SEARCH_PLACES: [(&str, &str); 2] = [("sit1", "a.txt"), ("sit2", "b.yml")];

/// The text with `\r\n` and a lone `\r` turned into `\n`, as the search reads it.
fn lines_as_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn engine_regex(data: &[u8]) -> Result<(), String> {
    let Some((&flags, rest)) = data.split_first() else {
        return Ok(());
    };
    let split = rest.iter().position(|&b| b == 0);
    let (pattern, haystack) = split.map_or((rest, &rest[rest.len()..]), |p| (&rest[..p], &rest[p + 1..]));
    let Ok(pattern) = std::str::from_utf8(pattern) else {
        return Ok(());
    };
    let limits = search_limits();
    let query = TextQuery {
        pattern: pattern.to_owned(),
        regex: flags & 1 != 0,
        case_sensitive: flags & 2 != 0,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total: if flags & 4 != 0 {
            3
        } else {
            TextQuery::DEFAULT_MAX_TOTAL
        },
        max_per_file: if flags & 8 != 0 {
            2
        } else {
            TextQuery::DEFAULT_MAX_PER_FILE
        },
    };
    let content = Bytes::copy_from_slice(haystack);
    let corpus = SearchCorpus::new(vec![SearchBlob {
        hash: normalised_hash(&content, FileClass::Binary),
        content,
        locations: SEARCH_PLACES
            .iter()
            .map(|(s, p)| SearchLocation {
                swimlane: SwimlaneId::parse(s).unwrap(),
                path: NfsPath::parse(p).unwrap(),
            })
            .collect(),
    }])
    .map_err(|e| e.to_string())?;
    let run = || {
        search_text(
            &corpus,
            &query,
            &DenyMatcher::default(),
            &limits,
            pool(),
            &CancelFlag::new(),
        )
    };
    let first = run();
    ensure(first == run(), || {
        "searching twice gives different answers".into()
    })?;
    match first {
        Err(SearchError::EmptyPattern) => ensure(pattern.is_empty(), || {
            "a pattern that is not empty was refused as empty".into()
        }),
        Err(SearchError::PatternTooLong) => ensure(pattern.len() > limits.regex_pattern_bytes, || {
            "a pattern within the limit was refused as too long".into()
        }),
        Err(SearchError::InvalidPattern) => ensure(
            !pattern.is_empty() && pattern.len() <= limits.regex_pattern_bytes,
            || "an empty or over-long pattern was refused as invalid".into(),
        ),
        Err(other) => Err(format!("a search without a glob or a flag failed with {other:?}")),
        Ok(outcome) => {
            ensure(
                pattern.len() <= limits.regex_pattern_bytes && !pattern.is_empty(),
                || "an empty or over-long pattern was searched".into(),
            )?;
            check_search(&outcome, &query, &limits)?;
            check_literal_oracle(&outcome, &query, &limits, haystack)
        }
    }
}

/// What every answer of `search_text` has, whatever the query.
fn check_search(outcome: &SearchOutcome, query: &TextQuery, limits: &Limits) -> Result<(), String> {
    let hits = &outcome.results.hits;
    ensure(hits.len() <= query.max_total as usize, || {
        format!("{} hits over the total cap of {}", hits.len(), query.max_total)
    })?;
    let stats = outcome.stats;
    ensure(
        stats.blobs_searched + stats.blobs_binary + stats.blobs_oversized <= 1,
        || format!("{stats:?} for one blob"),
    )?;
    let mut per_place: BTreeSet<(String, String)> = BTreeSet::new();
    let mut count = 0_usize;
    let mut previous: Option<(String, String, u32)> = None;
    for hit in hits {
        check_hit(hit, limits)?;
        let place = (hit.swimlane.to_string(), hit.path.to_string());
        let key = (place.0.clone(), place.1.clone(), hit.line);
        if let Some(before) = &previous {
            ensure(*before < key, || {
                format!("hits out of order: {before:?} then {key:?}")
            })?;
            if (before.0.as_str(), before.1.as_str()) != (place.0.as_str(), place.1.as_str()) {
                count = 0;
            }
        }
        count += 1;
        ensure(count <= query.max_per_file as usize, || {
            format!("{count} hits in one place over the cap of {}", query.max_per_file)
        })?;
        per_place.insert(place);
        previous = Some(key);
    }
    Ok(())
}

fn check_hit(hit: &TextHit, limits: &Limits) -> Result<(), String> {
    ensure(hit.line >= 1, || "a hit on line 0".into())?;
    ensure(hit.text.len() <= limits.hit_line_bytes, || {
        format!(
            "a line of {} bytes over the cut of {}",
            hit.text.len(),
            limits.hit_line_bytes
        )
    })?;
    ensure(!hit.text.contains(['\r', '\n']), || {
        "a line break in a hit's text".into()
    })?;
    ensure(hit.ranges.len() <= 64, || "more than 64 ranges in a hit".into())?;
    let mut end_before = 0_u32;
    for &(start, end) in &hit.ranges {
        ensure(start < end && start >= end_before, || {
            format!("ranges out of order or empty: {:?}", hit.ranges)
        })?;
        ensure(hit.text.get(start as usize..end as usize).is_some(), || {
            format!("the range {start}..{end} is not inside the text or not on character boundaries")
        })?;
        end_before = end;
    }
    Ok(())
}

/// For a literal, case-sensitive pattern in UTF-8 text, the answer can be worked out with `str`: the lines that hold the
/// pattern, and the places in them where it occurs.
fn check_literal_oracle(
    outcome: &SearchOutcome,
    query: &TextQuery,
    limits: &Limits,
    haystack: &[u8],
) -> Result<(), String> {
    let hits = &outcome.results.hits;
    // A byte order mark that says UTF-16 makes NUL bytes part of the text, and the text is read as such.
    if haystack.starts_with(&[0xFF, 0xFE]) || haystack.starts_with(&[0xFE, 0xFF]) {
        return Ok(());
    }
    if haystack.contains(&0) || haystack.len() > limits.input_bytes {
        ensure(hits.is_empty(), || "hits in a binary or over-long text".into())?;
        ensure(outcome.stats.blobs_searched == 0, || {
            "a binary or over-long text was searched".into()
        })?;
        return Ok(());
    }
    let Ok(text) = std::str::from_utf8(haystack) else {
        return Ok(());
    };
    if query.regex || !query.case_sensitive || text.starts_with('\u{feff}') {
        return Ok(());
    }
    let lf = lines_as_lf(text);
    let pattern = query.pattern.as_str();
    let expected: Vec<(u32, &str)> = lf
        .split('\n')
        .zip(1_u32..)
        .filter(|(line, _)| line.contains(pattern))
        .map(|(line, n)| (n, line))
        .collect();
    let at = |lane: &str| -> Vec<&TextHit> { hits.iter().filter(|h| h.swimlane.as_str() == lane).collect() };
    for (lane, _) in SEARCH_PLACES {
        let got = at(lane);
        if !outcome.results.truncated {
            ensure(got.len() == expected.len(), || {
                format!(
                    "{} hits in {lane} for {} lines that hold the pattern",
                    got.len(),
                    expected.len()
                )
            })?;
        }
        for (hit, &(n, line)) in got.iter().zip(&expected) {
            ensure(hit.line == n, || {
                format!("a hit on line {} where line {n} is next", hit.line)
            })?;
            let mut cut = line.len().min(limits.hit_line_bytes);
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            ensure(hit.text == line[..cut], || {
                format!("line {n}: the text differs from the line")
            })?;
            let ranges: Vec<(u32, u32)> = line
                .match_indices(pattern)
                .filter(|(s, _)| *s < cut)
                .take(64)
                .map(|(s, m)| (s as u32, (s + m.len()).min(cut) as u32))
                .collect();
            ensure(hit.ranges == ranges, || {
                format!("line {n}: ranges {:?} for occurrences at {ranges:?}", hit.ranges)
            })?;
        }
    }
    Ok(())
}

// --- engine_secret_scan ---

pub fn engine_secret_scan(data: &[u8]) -> Result<(), String> {
    let Some((&flags, rest)) = data.split_first() else {
        return Ok(());
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return Ok(());
    };
    let config = ScanConfig {
        hex_is_random: flags & 1 != 0,
        min_random_len: if flags & 2 != 0 { 16 } else { 32 },
        min_entropy_millibits: if flags & 4 != 0 { 3_000 } else { 4_000 },
        max_matches: 64,
        ..ScanConfig::default()
    };
    let limits = fuzz_limits();
    let first = secret_scan(text, &config, &limits);
    ensure(first == secret_scan(text, &config, &limits), || {
        "scanning twice gives different answers".into()
    })?;
    match first {
        Err(ScanError::TooLarge) => ensure(text.len() > limits.input_bytes, || {
            "a text within the limit was refused as too large".into()
        }),
        // More than the cap is an error, and the cap is what the config said.
        Err(ScanError::TooManyMatches) => Ok(()),
        Ok(matches) => check_matches(text, &matches, &config),
    }
}

/// The (start, end) byte offsets of each line of the text, with the same line ends as the scan: `\n`, `\r\n` and `\r`.
fn line_table(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let (mut lines, mut start, mut i) = (Vec::new(), 0, 0);
    while i < b.len() {
        match b[i] {
            b'\r' | b'\n' => {
                lines.push((start, i));
                i += if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push((start, b.len()));
    lines
}

/// The byte span of a match, or `None` when it is not inside the text.
fn span_of(text: &str, lines: &[(usize, usize)], m: &SecretMatch) -> Option<(usize, usize)> {
    let (from, to) = *lines.get((m.line as usize).checked_sub(1)?)?;
    let line = &text[from..to];
    let start = line
        .char_indices()
        .nth((m.column as usize).checked_sub(1)?)
        .map(|(i, _)| i)
        .or_else(|| (line.chars().count() == (m.column as usize) - 1).then_some(line.len()))?;
    let tail = &line[start..];
    let len = m.length as usize;
    let end = if tail.chars().count() < len {
        return None;
    } else {
        tail.char_indices().nth(len).map_or(tail.len(), |(i, _)| i)
    };
    Some((from + start, from + start + end))
}

fn check_matches(text: &str, matches: &[SecretMatch], config: &ScanConfig) -> Result<(), String> {
    ensure(matches.len() <= config.max_matches, || {
        "more matches than the cap".into()
    })?;
    let lines = line_table(text);
    let mut spans = Vec::new();
    let mut end_before = 0_usize;
    for m in matches {
        ensure(m.length >= 1 && m.column >= 1 && m.line >= 1, || {
            format!("an empty or unplaced match {m:?}")
        })?;
        let (start, end) =
            span_of(text, &lines, m).ok_or_else(|| format!("a match outside the text: {m:?}"))?;
        ensure(start >= end_before, || {
            format!("matches out of order or overlapping at {m:?}")
        })?;
        ensure(text[start..end].chars().count() == m.length as usize, || {
            format!("{m:?}: the length does not count characters")
        })?;
        ensure(
            m.marker() == format!("[REDACTED: {} chars, {}]", m.length, m.rule.name()),
            || "a marker that is not the marker format".into(),
        )?;
        end_before = end;
        spans.push((start, end, m.marker()));
    }
    // What the scan reports is all of it: hide every span, and the scan has nothing left to report.
    let mut redacted = text.to_owned();
    for (start, end, marker) in spans.iter().rev() {
        redacted.replace_range(*start..*end, marker);
    }
    let again = secret_scan(
        &redacted,
        &ScanConfig {
            max_matches: usize::MAX,
            ..config.clone()
        },
        &Limits {
            input_bytes: usize::MAX,
            ..fuzz_limits()
        },
    )
    .map_err(|e| format!("the redacted text could not be scanned: {e}"))?;
    ensure(again.is_empty(), || {
        format!(
            "{} matches are left after redacting {} spans: {again:?}",
            again.len(),
            spans.len()
        )
    })
}

// --- engine_index_settings ---

/// The syntax a selector byte stands for.
pub fn syntax_of_selector(selector: u8) -> Syntax {
    match selector % 3 {
        0 => Syntax::Yaml,
        1 => Syntax::Json,
        _ => Syntax::Properties,
    }
}

pub fn engine_index_settings(data: &[u8]) -> Result<(), String> {
    let Some((&selector, body)) = data.split_first() else {
        return Ok(());
    };
    let syntax = syntax_of_selector(selector);
    let limits = fuzz_limits();

    let first = index_settings(body, syntax, &limits);
    ensure(first == index_settings(body, syntax, &limits), || {
        "indexing twice gives different answers".into()
    })?;
    // The index is there exactly when the bytes are acceptable, and holds the settings `validate` counts.
    let index = match (first, validate(body, syntax, &limits)) {
        (Ok(index), Ok(validation)) => {
            ensure(index.rows.len() == validation.settings, || {
                format!("{} rows for {} settings", index.rows.len(), validation.settings)
            })?;
            index
        }
        (Err(a), Err(b)) => {
            ensure(a == b, || {
                format!("index_settings fails with {a:?} and validate with {b:?}")
            })?;
            return Ok(());
        }
        (a, b) => {
            return Err(format!(
                "index_settings is {} and validate is {}",
                if a.is_ok() { "ok" } else { "an error" },
                if b.is_ok() { "ok" } else { "an error" }
            ));
        }
    };

    let text =
        decode_text(body, &limits).map_err(|e| format!("an index for bytes that do not decode: {e}"))?;
    let total_lines = lines_in(&text.text);
    ensure(index.rows.len() <= limits.settings, || {
        "more rows than the limit".into()
    })?;
    ensure(index.warnings.len() <= limits.warnings, || {
        "more warnings than the cap".into()
    })?;
    let mut seen = BTreeSet::new();
    for row in &index.rows {
        let path = row.path.as_str();
        ensure(seen.insert(path.to_owned()), || {
            format!("the path {path:?} appears twice")
        })?;
        ensure(path.len() <= limits.setting_path_bytes, || {
            "a path over the limit".into()
        })?;
        ensure(
            row.lines.start() >= 1 && row.lines.start() <= row.lines.end(),
            || format!("{path}: lines {}..{}", row.lines.start(), row.lines.end()),
        )?;
        ensure(row.lines.end() <= total_lines + 1, || {
            format!(
                "{path}: lines {}..{} in a text of {total_lines}",
                row.lines.start(),
                row.lines.end()
            )
        })?;
        ensure(row.value.text.len() <= limits.scalar_bytes.max(2), || {
            format!("{path}: a value over the limit")
        })?;
    }

    // Where the ranges come from, and what the warnings say about it.
    let mismatches: Vec<&Option<domain::SettingPath>> = index
        .warnings
        .iter()
        .filter_map(|w| match w {
            ParseWarning::LocationMismatch { path } => Some(path),
            _ => None,
        })
        .collect();
    match (syntax, index.located_by) {
        (Syntax::Yaml, LocationSource::TreeSitter) => {
            for path in &mismatches {
                let named = path
                    .as_ref()
                    .ok_or_else(|| "tree-sitter read the file and the warning has no setting".to_owned())?;
                ensure(seen.contains(named.as_str()), || {
                    "a disagreement names a setting that the file does not have".into()
                })?;
            }
        }
        (Syntax::Yaml, LocationSource::Parser) => {
            // tree-sitter could not read the file: the file says so, unless the warnings are full.
            ensure(
                mismatches.iter().any(|p| p.is_none()) || index.warnings_dropped > 0,
                || "tree-sitter could not read a YAML file and nothing says so".into(),
            )?;
        }
        (_, LocationSource::Parser) => {
            ensure(mismatches.is_empty(), || {
                "a location warning on a file that tree-sitter does not read".into()
            })?;
        }
        (_, LocationSource::TreeSitter) => return Err("tree-sitter located a file that is not YAML".into()),
    }

    // `locate` is the index, asked for one setting.
    for row in index.rows.iter().take(8) {
        ensure(index.locate(&row.path) == Some(row.lines), || {
            format!("{}: the index does not find its own row", row.path)
        })?;
        ensure(
            locate(body, syntax, &row.path, &limits) == Ok(Some(row.lines)),
            || format!("{}: locate disagrees with the index", row.path),
        )?;
    }
    Ok(())
}

// --- engine_flatten ---

pub fn engine_flatten(data: &[u8]) -> Result<(), String> {
    let text = String::from_utf8_lossy(data);
    // 1. Keys and indexes built from the pieces are read back as they were.
    let mut document = None;
    let mut segments = Vec::new();
    for (i, piece) in text.split('\0').enumerate() {
        if i == 0 {
            if let Some(n) = piece.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()) {
                document = Some(n);
                continue;
            }
        }
        match piece.strip_prefix('@').and_then(|d| d.parse::<u32>().ok()) {
            Some(n) => segments.push(PathSegment::Index(n)),
            None => segments.push(PathSegment::Key(piece.to_owned())),
        }
    }
    let encoded = encode_setting_path(document, &segments);
    ensure(!encoded.chars().any(char::is_control), || {
        format!("a control character in {encoded:?}")
    })?;
    if let Ok(path) = SettingPath::parse(&encoded) {
        let decoded = decode_setting_path(&path).ok_or_else(|| format!("{encoded:?} cannot be read back"))?;
        ensure(
            decoded.document == document && decoded.segments == segments,
            || format!("{encoded:?} is read back as something else"),
        )?;
    }

    // 2. Whatever text is offered as a path is refused, or read to something that writes and reads back the same.
    if let Ok(path) = SettingPath::parse(&text) {
        if let Some(decoded) = decode_setting_path(&path) {
            let again = encode_setting_path(decoded.document, &decoded.segments);
            if let Ok(again_path) = SettingPath::parse(&again) {
                let back = decode_setting_path(&again_path)
                    .ok_or_else(|| format!("{again:?} (from {text:?}) cannot be read back"))?;
                ensure(back == decoded, || {
                    format!("{text:?} is read differently after one round trip")
                })?;
            }
        }
    }
    Ok(())
}

// --- engine_path_mapping ---

/// The most pieces of an input that are asked about as files, and the most merge-rule globs it may carry (each glob is
/// compiled twice, in the list and alone), so that an input stays quick.
const MAX_PIECES: usize = 40;
const MAX_GLOBS: usize = 10;

pub fn engine_path_mapping(data: &[u8]) -> Result<(), String> {
    let text = String::from_utf8_lossy(data);
    let mut pieces = text.split('\0');
    let nfs_text = pieces.next().unwrap_or("");
    let tenant_text = pieces.next().unwrap_or("");
    let repo_text = pieces.next().unwrap_or("");
    let globs: Vec<&str> = pieces.take(MAX_GLOBS).collect();

    let tenant_ids: Vec<TenantId> = tenant_text
        .split_whitespace()
        .take(TenantSet::MAX_TENANTS + 2)
        .filter_map(|t| TenantId::parse(t).ok())
        .collect();
    // More than the most a swimlane can have is refused, as it is everywhere.
    let tenants = TenantSet::new(tenant_ids.iter().cloned()).unwrap_or_default();
    let rule = PathMappingRule::default();

    if let Ok(nfs) = NfsPath::parse(nfs_text) {
        let first = reverse_map(&rule, &nfs, &tenants);
        ensure(first == reverse_map(&rule, &nfs, &tenants), || {
            "reading a path back to Git twice gives different answers".into()
        })?;
        // A name that is not a valid repository path (`.git`) is refused, and that is all there is to check.
        if let Ok(mapping) = first {
            check_reverse(&rule, &nfs, &mapping)?;
            for in_git in [true, false] {
                let facts = classify_path(&rule, &nfs, &tenants, |m| {
                    // The closure is asked about exactly what the name was read as.
                    in_git && *m == mapping
                })
                .map_err(|e| format!("classify_path refused what reverse_map accepted: {e}"))?;
                let (expected_kind, tenant, logical) = match &mapping {
                    ReverseMapping::Base { logical, .. } => (FileKind::Base, None, logical.clone()),
                    ReverseMapping::Tenant { tenant, logical, .. } => {
                        (FileKind::Tenant, Some(tenant.clone()), logical.clone())
                    }
                };
                ensure(
                    facts.kind
                        == if in_git {
                            expected_kind
                        } else {
                            FileKind::Untracked
                        },
                    || format!("classify_path says {:?} (in git: {in_git})", facts.kind),
                )?;
                ensure(facts.tenant == tenant && facts.logical == logical, || {
                    "classify_path disagrees with the mapping on the tenant or the logical file".into()
                })?;
            }
        }
    }

    // The other way: a repository path to the NFS tree, and back.
    if let Ok(repo) = RepoPath::parse(repo_text) {
        if let Ok(nfs) = map_path(&rule, &RepoSource::Base, &repo) {
            ensure(
                repo.as_str().strip_prefix("config/") == Some(nfs.as_str()),
                || "a base file is not its repository path without the root".into(),
            )?;
            forward_after_reverse(&rule, &nfs, &tenants)?;
        }
        for tenant in tenant_ids.iter().take(4) {
            let Ok(nfs) = map_path(&rule, &RepoSource::Tenant(tenant.clone()), &repo) else {
                continue;
            };
            forward_after_reverse(&rule, &nfs, &tenants)?;
            // Alone in its swimlane, the tenant's own file reads back to the same branch and the same path.
            let alone = TenantSet::new([tenant.clone()]).map_err(|e| e.to_string())?;
            match reverse_map(&rule, &nfs, &alone) {
                Ok(ReverseMapping::Tenant {
                    tenant: back,
                    repo_path,
                    ..
                }) => ensure(back == *tenant && repo_path == repo, || {
                    "a tenant file does not read back to its branch and path".into()
                })?,
                // `-<tenant>` right after nothing (`-sit1.yml`) is a base name, and the mapping says so.
                Ok(ReverseMapping::Base { .. }) => {}
                Err(e) => return Err(format!("a mapped tenant file cannot be read back: {e}")),
            }
        }
    }

    merge_rules_agree_with_single_rules(&globs, &text)
}

/// A mapping of `nfs` back to Git must map forward to `nfs` again, under the right root.
fn check_reverse(rule: &PathMappingRule, nfs: &NfsPath, mapping: &ReverseMapping) -> Result<(), String> {
    let (forward, root, logical) = match mapping {
        ReverseMapping::Base { repo_path, logical } => {
            (map_path(rule, &RepoSource::Base, repo_path), "config/", logical)
        }
        ReverseMapping::Tenant {
            tenant,
            repo_path,
            logical,
        } => (
            map_path(rule, &RepoSource::Tenant(tenant.clone()), repo_path),
            "data/config/",
            logical,
        ),
    };
    let repo_path = match mapping {
        ReverseMapping::Base { repo_path, .. } | ReverseMapping::Tenant { repo_path, .. } => repo_path,
    };
    ensure(repo_path.as_str().starts_with(root), || {
        format!("a repository path outside {root}")
    })?;
    ensure(
        !repo_path
            .as_str()
            .split('/')
            .any(|c| matches!(c, "" | "." | ".." | ".git")),
        || "a repository path with a component that must never be there".into(),
    )?;
    ensure(forward.as_ref() == Ok(nfs), || {
        "a path read back to Git does not map forward to itself".into()
    })?;
    ensure(
        !logical.as_str().is_empty() && logical.as_str().len() <= nfs.as_str().len(),
        || "a logical file longer than the path it comes from".into(),
    )?;
    ensure(
        LogicalFile::parse(logical.as_str()).as_ref() == Ok(logical),
        || "a logical file that does not parse as itself".into(),
    )
}

fn forward_after_reverse(rule: &PathMappingRule, nfs: &NfsPath, tenants: &TenantSet) -> Result<(), String> {
    match reverse_map(rule, nfs, tenants) {
        Ok(mapping) => check_reverse(rule, nfs, &mapping),
        // Only a name that would be a `.git` component is refused.
        Err(_) => Ok(()),
    }
}

/// A list of merge rules gives, for any file, what the first rule that matches it gives when asked alone.
fn merge_rules_agree_with_single_rules(globs: &[&str], text: &str) -> Result<(), String> {
    let mode = |i: usize| {
        if i % 2 == 0 {
            MergeMode::SpringMerge
        } else {
            MergeMode::WholeFile
        }
    };
    let opposite = |m: MergeMode| match m {
        MergeMode::SpringMerge => MergeMode::WholeFile,
        MergeMode::WholeFile => MergeMode::SpringMerge,
    };
    let rules: Vec<(&str, MergeMode)> = globs.iter().enumerate().map(|(i, g)| (*g, mode(i))).collect();
    let Ok(all) = MergeRules::new(MergeMode::WholeFile, &rules) else {
        // A refusal is fine, but it must be a refusal of a single glob or of the number, never of nothing.
        return ensure(
            rules.len() > MergeRules::MAX_RULES
                || rules
                    .iter()
                    .any(|(g, m)| MergeRules::new(opposite(*m), &[(*g, *m)]).is_err()),
            || "a list of rules is refused although every rule alone is fine".into(),
        );
    };
    // Each rule alone, built once: it matches a file exactly when it gives its own mode against the opposite default.
    let mut alones = Vec::with_capacity(rules.len());
    for (glob, m) in &rules {
        alones.push((
            *m,
            MergeRules::new(opposite(*m), &[(*glob, *m)]).map_err(|e| format!("a rule alone: {e}"))?,
        ));
    }
    // The files to ask about: the pieces of the input that are paths.
    for candidate in text.split('\0').take(MAX_PIECES) {
        let Ok(file) = LogicalFile::parse(candidate) else {
            continue;
        };
        ensure(all.mode_for(&file) == all.mode_for(&file), || {
            "a rule list answers twice differently".into()
        })?;
        let expected = alones
            .iter()
            .find(|(m, alone)| alone.mode_for(&file) == *m)
            .map_or(MergeMode::WholeFile, |(m, _)| *m);
        ensure(all.mode_for(&file) == expected, || {
            "the rule list does not give what the first matching rule gives".into()
        })?;
    }
    Ok(())
}

// --- the checks catch what they exist to catch ---

/// The replay is not blind: a result that breaks a rule is reported. Used by `tests/corpus_replay.rs`.
pub fn planted_harm_is_detected() -> Result<(), String> {
    let limits = fuzz_limits();
    let parsed = parse("a: 1\nb: 2\n", Syntax::Yaml, &limits).map_err(|e| e.to_string())?;
    let flat = flatten(&parsed, &limits).map_err(|e| e.to_string())?;
    check_flat(&flat, Syntax::Yaml, 2, &limits)?;

    let mut twice = flat.clone();
    twice.settings.push(twice.settings[0].clone());
    ensure(check_flat(&twice, Syntax::Yaml, 2, &limits).is_err(), || {
        "a path that appears twice was not reported".into()
    })?;

    let mut outside = flat.clone();
    outside.settings[0].lines = domain::LineRange::new(40, 41).map_err(|e| e.to_string())?;
    ensure(check_flat(&outside, Syntax::Yaml, 2, &limits).is_err(), || {
        "lines outside the text were not reported".into()
    })?;

    let mut bad_path = flat;
    bad_path.settings[0].path = SettingPath::parse("a[").map_err(|e| e.to_string())?;
    ensure(check_flat(&bad_path, Syntax::Yaml, 2, &limits).is_err(), || {
        "a path that cannot be read back was not reported".into()
    })?;

    // A reverse mapping that does not map forward to its own path is reported.
    let nfs = NfsPath::parse("a/b-sit1.yml").map_err(|e| e.to_string())?;
    let rule = PathMappingRule::default();
    let set =
        TenantSet::new([TenantId::parse("sit1").map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
    let honest = reverse_map(&rule, &nfs, &set).map_err(|e| e.to_string())?;
    check_reverse(&rule, &nfs, &honest)?;
    let lying = ReverseMapping::Base {
        repo_path: RepoPath::parse("config/a/other.yml").map_err(|e| e.to_string())?,
        logical: LogicalFile::parse("a/other.yml").map_err(|e| e.to_string())?,
    };
    ensure(check_reverse(&rule, &nfs, &lying).is_err(), || {
        "a mapping that does not lead back was not reported".into()
    })?;
    ensure(
        engine_path_mapping(b"a/b-sit1.yml\0sit1\0data/config/a/b.yml\0*.yml").is_ok(),
        || "a clean path mapping input was reported".into(),
    )?;

    // The search checks catch a hit that breaks a rule, and the scan checks a match outside the text.
    let limits_for_search = search_limits();
    let hit = |text: &str, ranges: Vec<(u32, u32)>| TextHit {
        swimlane: SwimlaneId::parse("sit1").unwrap(),
        path: NfsPath::parse("a.txt").unwrap(),
        line: 1,
        text: text.to_owned(),
        ranges,
    };
    check_hit(&hit("fine", vec![(0, 2)]), &limits_for_search)?;
    for (name, bad) in [
        ("a carriage return", hit("a\rb", vec![])),
        ("a range outside the text", hit("ab", vec![(1, 9)])),
        ("an empty range", hit("ab", vec![(1, 1)])),
        ("ranges out of order", hit("abcd", vec![(2, 3), (0, 1)])),
        ("a range inside a character", hit("\u{e9}", vec![(0, 1)])),
        ("a line over the cut", hit(&"x".repeat(65), vec![])),
    ] {
        ensure(check_hit(&bad, &limits_for_search).is_err(), || {
            format!("a hit with {name} was not reported")
        })?;
    }
    let text = "a: ghp_";
    let lines = line_table(text);
    ensure(
        span_of(
            text,
            &lines,
            &SecretMatch {
                line: 1,
                column: 4,
                length: 9,
                rule: engine::SecretRule::Jwt,
            },
        )
        .is_none(),
        || "a match outside the text was not reported".into(),
    )?;
    ensure(engine_regex(b"\x03fine\0a fine line\n").is_ok(), || {
        "a clean search input was reported".into()
    })?;
    ensure(engine_secret_scan(b"\0password: x\n").is_ok(), || {
        "a clean scan input was reported".into()
    })?;

    ensure(engine_flatten(b"fine\0@1").is_ok(), || {
        "a clean input was reported".into()
    })?;
    ensure(
        engine_index_settings(b"\0a: 1\nb:\n  c: |\n    x\n  d: 2\n").is_ok(),
        || "a clean file was reported by the index check".into(),
    )?;
    Ok(())
}
