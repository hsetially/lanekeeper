//! The fact engine's section of `docs/threat-model.md` (04 T10, S24) is complete, and it stays true to the code.
//!
//! These tests read the `## Fact engine (04)` section and check what a machine can check:
//!
//! - the four parts are there (assets, trust boundaries, STRIDE, residual risks), and every STRIDE letter has a row;
//! - the section cites the engine's requirements (S11, S13, S21, S22), and every S# it cites exists in `docs/security.md`;
//! - every test or fuzz target that a row names as its proof exists in `crates/engine`, and a name must belong to a function
//!   that really is a test, so a row cannot cite a helper or a test nobody wrote;
//! - the residual risks name the limits that the engine's hand-offs recorded (tree-sitter, the reconstruction of the
//!   config-server, the three layers of the batch hash, the heuristic secret scan, hits that carry content, costs bounded in
//!   size).
//!
//! `planted_gaps_are_found` shows that each check refuses a document with the gap it is meant to catch.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const HEADING: &str = "## Fact engine (04)";
const REQUIREMENTS: [&str; 4] = ["S11", "S13", "S21", "S22"];
const STRIDE_LETTERS: [&str; 6] = ["S", "T", "R", "I", "D", "E"];
const PARTS: [&str; 4] = [
    "### Assets",
    "### Trust boundaries",
    "### STRIDE",
    "### Residual risks",
];
const MIN_RESIDUAL_RISKS: usize = 5;

/// Limits the engine recorded; each needs one bullet that holds every needle.
const CARRY_FORWARD: [(&str, &[&str]); 6] = [
    ("tree-sitter is C code", &["tree-sitter", "C code", "S19"]),
    (
        "the resolution model is a reconstruction",
        &["D86", "as served", "A3"],
    ),
    (
        "the batch hash has three layers",
        &["three layers", "application-<tenant>", "property_view"],
    ),
    (
        "the secret scan is heuristic",
        &["heuristic", "Q20", "ScanConfig"],
    ),
    ("search hits carry content", &["search_text", "line text", "S14b"]),
    ("costs are bounded in size", &["size, not in time", "rate limits"]),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn document() -> String {
    read(&repo().join("docs/threat-model.md"))
}

/// The engine's section: from its heading to the next `## `.
fn section(doc: &str) -> Result<String, String> {
    let start = doc.find(HEADING).ok_or("no `## Fact engine (04)` section")?;
    let rest = &doc[start + HEADING.len()..];
    let end = rest.find("\n## ").map_or(rest.len(), |i| i);
    Ok(rest[..end].to_owned())
}

fn parts_present(section: &str) -> Result<(), String> {
    for part in PARTS {
        if !section.contains(part) {
            return Err(format!("the section has no `{part}`"));
        }
    }
    Ok(())
}

/// The rows of the STRIDE table: (letter, cells).
fn stride_rows(section: &str) -> Vec<(String, Vec<String>)> {
    let table = section
        .split("### STRIDE")
        .nth(1)
        .and_then(|rest| rest.split("\n### ").next())
        .unwrap_or("");
    table
        .lines()
        .filter(|l| l.starts_with('|'))
        .filter_map(|l| {
            let cells: Vec<String> = l
                .trim_matches('|')
                .split(" | ")
                .map(|c| c.trim().to_owned())
                .collect();
            (cells.len() >= 5 && STRIDE_LETTERS.contains(&cells[0].as_str()))
                .then(|| (cells[0].clone(), cells))
        })
        .collect()
}

fn every_letter_has_a_row(section: &str) -> Result<(), String> {
    let letters: BTreeSet<String> = stride_rows(section).into_iter().map(|(l, _)| l).collect();
    for letter in STRIDE_LETTERS {
        if !letters.contains(letter) {
            return Err(format!("no STRIDE row for {letter}"));
        }
    }
    Ok(())
}

fn requirements_cited(section: &str) -> Result<(), String> {
    for req in REQUIREMENTS {
        let cited = stride_rows(section).iter().any(|(_, cells)| {
            cells
                .last()
                .is_some_and(|s| s.split([',', ' ']).any(|t| t == req))
        });
        if !cited {
            return Err(format!("no STRIDE row cites {req}"));
        }
    }
    Ok(())
}

fn cited_requirements_exist(section: &str, security: &str) -> Result<(), String> {
    for (_, cells) in stride_rows(section) {
        for token in cells.last().map_or("", String::as_str).split([',', ' ']) {
            if token.starts_with('S') && token[1..].chars().all(|c| c.is_ascii_digit()) && token.len() > 1 {
                let listed = security.contains(&format!("- **{token} "));
                if !listed {
                    return Err(format!("{token} is not in docs/security.md"));
                }
            }
        }
    }
    Ok(())
}

/// Every function name in `crates/engine/tests`, `benches` and the fuzz targets, with whether it is a test.
fn engine_tests_and_targets() -> (BTreeSet<String>, BTreeSet<String>) {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n == "target" || n == "corpus" || n == "snapshots")
                {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"), &mut files);
    let mut tests = BTreeSet::new();
    for file in files {
        let text = read(&file);
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("fn ").or_else(|| t.strip_prefix("pub fn ")) else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            // A test has `#[test]` (or a proptest) among the attribute lines right above it, or is inside `proptest!`.
            let mut j = i;
            let mut is_test = false;
            while j > 0 {
                j -= 1;
                let above = lines[j].trim();
                if above.starts_with("#[") || above.starts_with("///") {
                    if above.starts_with("#[test]") {
                        is_test = true;
                        break;
                    }
                    continue;
                }
                break;
            }
            // Inside a `proptest!` block the functions carry no `#[test]`; those blocks start with `proptest! {` and every
            // function in them is a test. This file's own helper names are not in the set, which is what matters.
            if !is_test && text.contains("proptest! {") {
                let before = &lines[..i];
                let in_block = before.iter().rposition(|l| l.contains("proptest! {"));
                if let Some(start) = in_block {
                    is_test = !before[start..].iter().any(|l| l.starts_with('}'));
                }
            }
            if is_test {
                tests.insert(name);
            }
        }
    }
    // The fuzz targets, by the names in `fuzz/invariants.rs`.
    let invariants = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/invariants.rs"));
    let targets: BTreeSet<String> = invariants
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("(\"")
                .and_then(|r| r.split('"').next())
                .map(str::to_owned)
        })
        .filter(|n| n.starts_with("engine_"))
        .collect();
    (tests, targets)
}

fn proofs_exist(section: &str, tests: &BTreeSet<String>, targets: &BTreeSet<String>) -> Result<(), String> {
    for (letter, cells) in stride_rows(section) {
        let proof = &cells[cells.len() - 2];
        if proof.starts_with("None") {
            continue;
        }
        let names: Vec<&str> = proof.split('`').skip(1).step_by(2).collect();
        if names.is_empty() {
            return Err(format!("row {letter} names no proof"));
        }
        let mut after_fuzz = false;
        for chunk in proof.split(", ") {
            let chunk = chunk.trim();
            if chunk.starts_with("fuzz ") {
                after_fuzz = true;
            }
            let name = chunk.trim_start_matches("fuzz ").trim_matches('`');
            if after_fuzz {
                if !targets.contains(name) {
                    return Err(format!("row {letter}: no fuzz target `{name}`"));
                }
            } else if !tests.contains(name) {
                return Err(format!("row {letter}: `{name}` is not a test in crates/engine"));
            }
        }
    }
    Ok(())
}

fn residual_risks(section: &str) -> Vec<String> {
    section
        .split("### Residual risks")
        .nth(1)
        .unwrap_or("")
        .split("\n- ")
        .skip(1)
        .map(str::to_owned)
        .collect()
}

fn residual_risks_cover_the_carry_forward(section: &str) -> Result<(), String> {
    let risks = residual_risks(section);
    if risks.len() < MIN_RESIDUAL_RISKS {
        return Err(format!(
            "{} residual risks, need at least {MIN_RESIDUAL_RISKS}",
            risks.len()
        ));
    }
    for (topic, needles) in CARRY_FORWARD {
        if !risks.iter().any(|r| needles.iter().all(|n| r.contains(n))) {
            return Err(format!("no residual risk covers: {topic}"));
        }
    }
    Ok(())
}

fn all_checks(doc: &str, security: &str) -> Result<(), String> {
    let section = section(doc)?;
    let (tests, targets) = engine_tests_and_targets();
    parts_present(&section)?;
    every_letter_has_a_row(&section)?;
    requirements_cited(&section)?;
    cited_requirements_exist(&section, security)?;
    proofs_exist(&section, &tests, &targets)?;
    residual_risks_cover_the_carry_forward(&section)
}

#[test]
fn engine_section_has_all_six_stride_letters_and_cites_s11_s13_s21_s22() {
    let doc = document();
    let security = read(&repo().join("docs/security.md"));
    let section = section(&doc).unwrap();
    parts_present(&section).unwrap();
    every_letter_has_a_row(&section).unwrap();
    requirements_cited(&section).unwrap();
    cited_requirements_exist(&section, &security).unwrap();
}

#[test]
fn every_proof_the_section_names_exists() {
    let doc = document();
    let section = section(&doc).unwrap();
    let (tests, targets) = engine_tests_and_targets();
    assert!(tests.len() > 100, "found only {} tests", tests.len());
    proofs_exist(&section, &tests, &targets).unwrap();
}

#[test]
fn the_residual_risks_name_the_limits_the_engine_recorded() {
    let doc = document();
    residual_risks_cover_the_carry_forward(&section(&doc).unwrap()).unwrap();
}

#[test]
fn the_component_list_names_the_engine() {
    assert!(document().contains("- Engine (04)"));
}

#[test]
fn planted_gaps_are_found() {
    let doc = document();
    let security = read(&repo().join("docs/security.md"));
    assert!(all_checks(&doc, &security).is_ok(), "the real document passes");

    // No section at all.
    assert!(all_checks(&doc.replace(HEADING, "## Something else"), &security).is_err());
    // A part is missing.
    assert!(
        all_checks(
            &doc.replace(
                "### Trust boundaries\n\n- **Callers",
                "### Boundaries\n\n- **Callers"
            ),
            &security
        )
        .is_err()
    );
    // A STRIDE letter loses its row: rename the E row's letter.
    let section_text = section(&doc).unwrap();
    let broken = section_text.replace("| E | A parser flaw", "| X | A parser flaw");
    assert_ne!(broken, section_text);
    assert!(doc.replace(&section_text, &broken) != doc);
    assert!(all_checks(&doc.replace(&section_text, &broken), &security).is_err());
    // A proof that names a test nobody wrote.
    let invented = section_text.replace("`forbid_unsafe_present`", "`a_test_nobody_wrote`");
    assert!(all_checks(&doc.replace(&section_text, &invented), &security).is_err());
    // A proof that names a helper, not a test.
    let helper = section_text.replace("`forbid_unsafe_present`", "`crate_dir`");
    assert!(all_checks(&doc.replace(&section_text, &helper), &security).is_err());
    // A fuzz target that does not exist.
    let fuzz = section_text.replace("fuzz `engine_index_settings`", "fuzz `engine_nothing`");
    assert!(all_checks(&doc.replace(&section_text, &fuzz), &security).is_err());
    // A requirement that is dropped.
    let dropped = section_text
        .replace("| S11, S22 |", "| S22 |")
        .replace("| S11 |", "| S22 |");
    assert!(all_checks(&doc.replace(&section_text, &dropped), &security).is_err());
    // A residual risk that is dropped.
    let fewer = section_text.replace("- **tree-sitter is C code", "- **tree-sitter is a library");
    assert!(all_checks(&doc.replace(&section_text, &fewer), &security).is_err());
}
