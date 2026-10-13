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
//! | `engine_flatten` | NUL-separated pieces | a path built from any keys and indexes is read back to the same keys and indexes; any text offered as a setting path is refused or read to something that writes and reads back the same |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use std::collections::BTreeSet;

use bytes::Bytes;
use domain::{NfsPath, SettingPath, ValueFlag, ValueType};
use engine::{
    Body, DenyMatcher, Flat, Limits, LocationSource, Newline, NodeKind, ParseError, ParseWarning, Parsed,
    PathSegment, Syntax, apply_eol, classify, decode_setting_path, decode_text, detect_format,
    encode_setting_path, encode_text, flatten, index_settings, locate, normalise_eol, parse, validate,
};

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory) and its check.
pub const TARGETS: [(&str, Check); 6] = [
    ("engine_bytes", engine_bytes),
    ("engine_yaml", engine_yaml),
    ("engine_json", engine_json),
    ("engine_properties", engine_properties),
    ("engine_flatten", engine_flatten),
    ("engine_index_settings", engine_index_settings),
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

    ensure(engine_flatten(b"fine\0@1").is_ok(), || {
        "a clean input was reported".into()
    })?;
    ensure(
        engine_index_settings(b"\0a: 1\nb:\n  c: |\n    x\n  d: 2\n").is_ok(),
        || "a clean file was reported by the index check".into(),
    )?;
    Ok(())
}
