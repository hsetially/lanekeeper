//! The agent's section of `docs/threat-model.md` (T8, S24) is complete, and it stays true to the code.
//!
//! A threat model that nobody checks drifts away from the code it describes. These tests read the `## Agent (02)`
//! section and check what a machine can check:
//!
//! - the four parts are there (assets, trust boundaries, STRIDE, residual risks), and every STRIDE letter has a row;
//! - the section cites the agent's requirements (S5, S6, S10, S11, S16, S17, S21, S22), and every S# it cites exists;
//! - the certificate identity is written in the form the code checks (`AGENT_SAN_PREFIX`, S5, decision A5);
//! - every test or fuzz target that a row names as its proof exists in this crate, so a row cannot cite a test that
//!   was never written (a control that is planned but not built goes in the prose and says so, never in a Proof cell).
//!
//! Prompts 02's later tasks (the spool, quiescence, deny globs and the config-server calls) extend the section. The
//! checks only ask that what is there is true, so they keep passing as rows are added.
//!
//! Each check is a function over the document text, and `planted_gaps_are_found` shows that it refuses a document with
//! the gap it is meant to catch.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const HEADING: &str = "## Agent (02)";
/// The requirements prompt 02 owns (its metadata table). The section must speak to each one.
const REQUIREMENTS: [&str; 8] = ["S5", "S6", "S10", "S11", "S16", "S17", "S21", "S22"];
const STRIDE_LETTERS: [&str; 6] = ["S", "T", "R", "I", "D", "E"];
const PARTS: [&str; 4] = [
    "### Assets",
    "### Trust boundaries",
    "### STRIDE",
    "### Residual risks",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn threat_model() -> String {
    read(&repo().join("docs/threat-model.md"))
}

// ------------------------------------------------------------------------------------------------ reading the document

/// The text of the agent's section: from its heading to the next `## ` heading.
fn agent_section(doc: &str) -> Result<String, String> {
    let mut lines = doc.lines();
    let mut found = false;
    let mut out = Vec::new();
    for line in lines.by_ref() {
        if line.trim_end() == HEADING {
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("no `{HEADING}` heading in docs/threat-model.md"));
    }
    for line in lines {
        if line.starts_with("## ") {
            break;
        }
        out.push(line);
    }
    // A leading newline, so that the first part's heading is found like the others.
    Ok(format!("\n{}", out.join("\n")))
}

/// The text under `### <part>` up to the next `### ` heading.
fn part<'a>(section: &'a str, heading: &str) -> Option<&'a str> {
    let start = section
        .lines()
        .scan(0usize, |offset, line| {
            let at = *offset;
            *offset += line.len() + 1;
            Some((at, line))
        })
        .find(|(_, line)| line.trim_end() == heading)?
        .0;
    let body = &section[start..];
    let after_heading = body.find('\n').map_or(body.len(), |n| n + 1);
    let end = body[after_heading..]
        .find("\n### ")
        .map_or(body.len(), |n| after_heading + n);
    Some(&body[after_heading..end])
}

/// One row of the STRIDE table.
#[derive(Debug)]
struct Row {
    letter: String,
    threat: String,
    mitigation: String,
    proof: String,
    requirements: String,
}

/// The rows of the table in the STRIDE part: `| letter | threat | mitigation | proof | S# |`.
fn stride_rows(stride: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    let mut header_seen = false;
    for line in stride.lines().filter(|l| l.trim_start().starts_with('|')) {
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() != 5 {
            return Err(format!(
                "a STRIDE row has {} cells, not 5 (a stray `|` in a cell?): {line}",
                cells.len()
            ));
        }
        if !header_seen {
            header_seen = true;
            continue;
        }
        if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':')) {
            continue;
        }
        rows.push(Row {
            letter: cells[0].to_owned(),
            threat: cells[1].to_owned(),
            mitigation: cells[2].to_owned(),
            proof: cells[3].to_owned(),
            requirements: cells[4].to_owned(),
        });
    }
    Ok(rows)
}

/// The backtick spans of `text`.
fn code_spans(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// Every `S<number>` token in `text` (so `S5` but not `S5x`, `SAN` or `S5a` inside a word).
fn requirement_ids(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut ids = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if bytes[i] == b'S' && starts_word {
            let digits = bytes[i + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
            let end = i + 1 + digits;
            let ends_word = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if digits > 0 && ends_word {
                ids.insert(text[i..end].to_owned());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    ids
}

// ------------------------------------------------------------------------------------------------ the code's side

/// Every function name in the Rust sources under `dirs`, and the stems of the fuzz target files.
fn known_proofs() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = read(&path);
                for line in text.lines() {
                    let mut rest = line;
                    while let Some(at) = rest.find("fn ") {
                        let before_ok = at == 0 || !rest.as_bytes()[at - 1].is_ascii_alphanumeric();
                        let after = &rest[at + 3..];
                        let name: String = after
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .collect();
                        if before_ok && !name.is_empty() {
                            out.insert(name);
                        }
                        rest = after;
                    }
                }
            }
        }
    }
    let base = crate_dir();
    let mut names = BTreeSet::new();
    for dir in ["src", "tests", "benches"] {
        walk(&base.join(dir), &mut names);
    }
    if let Ok(targets) = fs::read_dir(base.join("fuzz/fuzz_targets")) {
        for target in targets.flatten() {
            if let Some(stem) = target.path().file_stem() {
                names.insert(stem.to_string_lossy().into_owned());
            }
        }
    }
    names
}

/// The value of `pub const AGENT_SAN_PREFIX: &str = "...";` in the identity module.
fn san_prefix_in_code() -> String {
    let source = read(&crate_dir().join("src/identity/cert.rs"));
    let line = source
        .lines()
        .find(|l| l.contains("pub const AGENT_SAN_PREFIX"))
        .expect("AGENT_SAN_PREFIX is defined in src/identity/cert.rs");
    let quoted = line.split('"').nth(1).expect("the constant is a string literal");
    quoted.to_owned()
}

/// Every S# that `docs/security.md` defines (`- **S5 Agent identity.**`).
fn defined_requirements() -> BTreeSet<String> {
    let security = read(&repo().join("docs/security.md"));
    security
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("- **"))
        .filter_map(|l| l.split([' ', '.', '*']).next())
        .filter(|id| id.len() > 1 && id.starts_with('S') && id[1..].chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

// ------------------------------------------------------------------------------------------------ the checks

/// Problems with the shape: the section exists and has its four parts, in order, each with content.
fn shape_problems(doc: &str) -> Vec<String> {
    let section = match agent_section(doc) {
        Ok(section) => section,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    let mut last = 0;
    for heading in PARTS {
        match section.find(&format!("\n{heading}\n")) {
            None => problems.push(format!("the Agent section has no `{heading}` part")),
            Some(at) => {
                if at < last {
                    problems.push(format!("`{heading}` is out of order"));
                }
                last = at;
                let body = part(&section, heading).unwrap_or_default();
                if body.lines().filter(|l| !l.trim().is_empty()).count() < 3 {
                    problems.push(format!("`{heading}` is nearly empty"));
                }
            }
        }
    }
    if let Some(risks) = part(&section, "### Residual risks") {
        let items = risks.lines().filter(|l| l.starts_with("- **")).count();
        if items < 8 {
            problems.push(format!(
                "only {items} residual risks are listed; each known limit of the agent belongs here"
            ));
        }
    }
    problems
}

/// Problems with the STRIDE table: every letter has a row, and every row names a mitigation, a proof and a requirement.
fn stride_problems(doc: &str) -> Vec<String> {
    let Ok(section) = agent_section(doc) else {
        return vec!["no Agent section".to_owned()];
    };
    let Some(stride) = part(&section, "### STRIDE") else {
        return vec!["no STRIDE part".to_owned()];
    };
    let rows = match stride_rows(stride) {
        Ok(rows) => rows,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    for letter in STRIDE_LETTERS {
        if !rows.iter().any(|r| r.letter == letter) {
            problems.push(format!("the STRIDE table has no `{letter}` row"));
        }
    }
    for row in &rows {
        if !STRIDE_LETTERS.contains(&row.letter.as_str()) {
            problems.push(format!("`{}` is not a STRIDE letter", row.letter));
        }
        let label: String = row.threat.chars().take(48).collect();
        if row.threat.is_empty() || row.mitigation.is_empty() {
            problems.push(format!("a row has no threat or no mitigation: {label}"));
        }
        if requirement_ids(&row.requirements).is_empty() {
            problems.push(format!("the row `{label}` cites no S#"));
        }
        if code_spans(&row.proof).is_empty() && !row.proof.starts_with("None") {
            problems.push(format!(
                "the row `{label}` names no proof; write `None` and why if there is no automated check"
            ));
        }
    }
    problems
}

/// Problems with the requirements: each of the agent's is cited, and each cited S# is real.
fn requirement_problems(doc: &str, defined: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = agent_section(doc) else {
        return vec!["no Agent section".to_owned()];
    };
    let cited = requirement_ids(&section);
    let mut problems = Vec::new();
    for id in REQUIREMENTS {
        if !cited.contains(id) {
            problems.push(format!("the Agent section never cites {id}"));
        }
    }
    for id in &cited {
        if !defined.contains(id) {
            problems.push(format!(
                "the Agent section cites {id}, which docs/security.md does not define"
            ));
        }
    }
    problems
}

/// Problems with the certificate identity: the form the code checks is the form the document states, and the old form
/// is not stated as current.
fn san_problems(doc: &str, prefix: &str) -> Vec<String> {
    let Ok(section) = agent_section(doc) else {
        return vec!["no Agent section".to_owned()];
    };
    let mut problems = Vec::new();
    let stated = format!("{prefix}<id>");
    if !section.contains(&stated) {
        problems.push(format!(
            "the Agent section does not state the certificate identity `{stated}`"
        ));
    }
    if section.contains("spiffe://lanekeeper/agent/") {
        problems.push(
            "the Agent section states the old identity form `spiffe://lanekeeper/agent/...`".to_owned(),
        );
    }
    problems
}

/// Problems with the proofs: every name in a Proof cell is a function or a fuzz target of this crate.
fn proof_problems(doc: &str, known: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = agent_section(doc) else {
        return vec!["no Agent section".to_owned()];
    };
    let Some(stride) = part(&section, "### STRIDE") else {
        return vec!["no STRIDE part".to_owned()];
    };
    let rows = match stride_rows(stride) {
        Ok(rows) => rows,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    for row in &rows {
        for name in code_spans(&row.proof) {
            if !known.contains(name) {
                problems.push(format!(
                    "the proof `{name}` is not a test, bench or fuzz target of the agent"
                ));
            }
        }
    }
    problems
}

// ------------------------------------------------------------------------------------------------ the tests

#[test]
fn agent_section_has_assets_boundaries_stride_and_residual_risks() {
    let problems = shape_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn stride_table_has_all_six_letters_and_every_row_is_complete() {
    let problems = stride_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn section_cites_s5_s6_s10_s11_s16_s17_s21_s22_and_only_real_requirements() {
    let defined = defined_requirements();
    assert!(
        REQUIREMENTS.iter().all(|id| defined.contains(*id)),
        "docs/security.md no longer defines one of {REQUIREMENTS:?}: {defined:?}"
    );
    let problems = requirement_problems(&threat_model(), &defined);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn s5_certificate_identity_in_the_document_is_the_one_the_code_checks() {
    let prefix = san_prefix_in_code();
    assert_eq!(
        prefix, "spiffe://lanekeeper/swimlane/",
        "the code's SAN form is not the S5 form (decision A5)"
    );
    let problems = san_problems(&threat_model(), &prefix);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_proof_the_stride_table_names_exists() {
    let known = known_proofs();
    assert!(
        known.len() > 200,
        "the scan of the crate's sources found only {} names",
        known.len()
    );
    let problems = proof_problems(&threat_model(), &known);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn planted_gaps_are_found() {
    let doc = threat_model();
    let defined = defined_requirements();
    let known = known_proofs();
    let prefix = san_prefix_in_code();
    // The real document passes every check, so each failure below is caused by the plant.
    assert!(shape_problems(&doc).is_empty());
    assert!(stride_problems(&doc).is_empty());
    assert!(requirement_problems(&doc, &defined).is_empty());
    assert!(san_problems(&doc, &prefix).is_empty());
    assert!(proof_problems(&doc, &known).is_empty());

    // No section at all.
    let without = doc.replace(HEADING, "## Something else");
    assert!(!shape_problems(&without).is_empty());

    // A part missing.
    let no_residual = doc.replace("### Residual risks", "### Leftovers");
    assert!(
        shape_problems(&no_residual)
            .iter()
            .any(|p| p.contains("Residual risks"))
    );

    // A STRIDE letter missing: rename every `| E |` row of the agent's table.
    let section = agent_section(&doc).unwrap();
    let stride = part(&section, "### STRIDE").unwrap();
    let no_e = stride.replace("\n| E |", "\n| X |");
    let doc_no_e = doc.replace(stride, &no_e);
    assert!(stride_problems(&doc_no_e).iter().any(|p| p.contains("`E`")));

    // A requirement the section does not cite, and one that docs/security.md does not define.
    let mut defined_without_s22 = defined.clone();
    defined_without_s22.remove("S22");
    assert!(
        requirement_problems(&doc, &defined_without_s22)
            .iter()
            .any(|p| p.contains("S22"))
    );
    let no_s17 = doc.replace("S17", "S-17");
    assert!(
        requirement_problems(&no_s17, &defined)
            .iter()
            .any(|p| p.contains("S17"))
    );

    // The wrong certificate identity.
    assert!(!san_problems(&doc, "spiffe://lanekeeper/agent/").is_empty());
    let old_form = doc.replace(&format!("{prefix}<id>"), "spiffe://lanekeeper/agent/<swimlane>");
    assert!(san_problems(&old_form, &prefix).len() >= 2);

    // A proof that was never written.
    let first_proof =
        code_spans(&stride_rows(part(&section, "### STRIDE").unwrap()).unwrap()[0].proof)[0].to_owned();
    let invented = doc.replacen(&format!("`{first_proof}`"), "`a_test_nobody_wrote`", 1);
    assert!(
        proof_problems(&invented, &known)
            .iter()
            .any(|p| p.contains("a_test_nobody_wrote"))
    );

    // A row with a stray pipe is reported instead of silently misread.
    let stray = stride.replacen("\n| S |", "\n| S | extra |", 1);
    assert_ne!(stray, stride, "the plant found no `| S |` row");
    let doc_stray = doc.replace(stride, &stray);
    assert!(!stride_problems(&doc_stray).is_empty());
}
