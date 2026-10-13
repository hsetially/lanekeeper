//! Shared by the parsing tests (T2): small helpers, and the text form the golden snapshots use.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use domain::{ValueFlag, ValueType};
use engine::{Body, Flat, Limits, ParseWarning, Parsed, Position, Syntax, flatten, parse};

pub fn limits() -> Limits {
    Limits::default()
}

pub fn parsed(syntax: Syntax, text: &str) -> Parsed {
    parse(text, syntax, &limits()).unwrap_or_else(|e| panic!("{syntax} did not parse: {e}\n{text}"))
}

pub fn flat(syntax: Syntax, text: &str) -> Flat {
    flatten(&parsed(syntax, text), &limits()).unwrap()
}

pub fn yaml(text: &str) -> Flat {
    flat(Syntax::Yaml, text)
}

pub fn json(text: &str) -> Flat {
    flat(Syntax::Json, text)
}

pub fn properties(text: &str) -> Flat {
    flat(Syntax::Properties, text)
}

fn ty(t: ValueType) -> &'static str {
    match t {
        ValueType::String => "string",
        ValueType::Int => "int",
        ValueType::Float => "float",
        ValueType::Bool => "bool",
        ValueType::Null => "null",
        ValueType::Other => "other",
    }
}

fn pos(p: Position) -> String {
    format!("{}:{}", p.line, p.column)
}

pub fn warning(w: &ParseWarning) -> String {
    match w {
        ParseWarning::DuplicateKey { path, first, second } => format!(
            "duplicate key {} first at {} again at {}",
            path.as_ref().map_or("(unnamed)".to_owned(), ToString::to_string),
            pos(*first),
            pos(*second)
        ),
        ParseWarning::BooleanLikeValue { path, line } => format!("boolean-like value at {path} line {line}"),
        ParseWarning::ComplexKeySkipped { at } => format!("complex key skipped at {}", pos(*at)),
        ParseWarning::InvalidMergeKey { at } => format!("invalid merge key at {}", pos(*at)),
        ParseWarning::UnusableKeySkipped { line } => format!("unusable key skipped at line {line}"),
        other => format!("{other:?}"),
    }
}

/// One line per setting: `path = "text" type[ flag] Lstart-end[ alias]`, then the warnings.
pub fn dump(all: &Flat) -> String {
    let mut out = String::new();
    for s in &all.settings {
        let flag = match s.value.flag {
            Some(ValueFlag::YamlBooleanLike) => " yaml-boolean-like",
            None => "",
        };
        let alias = if s.via_alias { " alias" } else { "" };
        writeln!(
            out,
            "{} = {:?} {}{} L{}-{}{}",
            s.path,
            s.value.text,
            ty(s.value.ty),
            flag,
            s.lines.start(),
            s.lines.end(),
            alias
        )
        .unwrap();
    }
    for w in &all.warnings {
        writeln!(out, "warning: {}", warning(w)).unwrap();
    }
    if all.warnings_dropped > 0 {
        writeln!(out, "warnings dropped: {}", all.warnings_dropped).unwrap();
    }
    out
}

/// The parse warnings, one per line.
pub fn parse_warnings(parsed: &Parsed) -> String {
    let mut out = String::new();
    for w in &parsed.warnings {
        writeln!(out, "{}", warning(w)).unwrap();
    }
    out
}

pub fn documents(parsed: &Parsed) -> usize {
    match &parsed.body {
        Body::Documents(d) => d.len(),
        Body::Properties(_) => 0,
    }
}

/// `path = text` pairs, for tests that compare two flattenings.
pub fn pairs(flat: &Flat) -> Vec<(String, String, ValueType)> {
    flat.settings
        .iter()
        .map(|s| (s.path.to_string(), s.value.text.clone(), s.value.ty))
        .collect()
}

/// The small-scale synthetic fixtures (`xtask::fixtures`), generated once per test binary into a temporary directory.
/// They need `git` on the PATH, as the generator does.
pub struct Fixtures {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub manifest: xtask::fixtures::manifest::Manifest,
}

pub fn small_fixtures() -> &'static Fixtures {
    static CELL: OnceLock<Fixtures> = OnceLock::new();
    CELL.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("small");
        let manifest = xtask::fixtures::generate(&xtask::fixtures::Config {
            scale: xtask::fixtures::Scale::Small,
            seed: xtask::fixtures::DEFAULT_SEED,
            out: root.clone(),
        })
        .unwrap_or_else(|e| panic!("cannot generate the small fixtures (is git on the PATH?): {e}"));
        Fixtures {
            _dir: dir,
            root,
            manifest,
        }
    })
}

/// Every regular file below `dir`, sorted, as paths relative to `dir` with `/`.
pub fn files_below(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}
