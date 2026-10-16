//! The registry's section of `docs/threat-model.md` (05 T10, S24) is complete, and it stays true to the code.
//!
//! A threat model that nobody checks drifts away from the code it describes. These tests read the `## Registry and read API (05)` section
//! and check what a machine can check:
//!
//! - the four parts are there (assets, trust boundaries, STRIDE, residual risks), in order, and every STRIDE letter has a row;
//! - the section cites the registry's requirements (S4, S5, S9, S10, S11, S13, S15, S17, S21, S22, S25), and every S# it cites exists in
//!   `docs/security.md`;
//! - every test, benchmark or fuzz target that a row names as its proof exists in `crates/hub-registry`, and a test name must belong to a
//!   function that really is a test, so a row cannot cite a helper or a test nobody wrote (a control that is planned but not built goes in
//!   the row's prose, and its Proof cell starts with `None`);
//! - the residual risks name the limits that the registry's tasks and decisions recorded: each topic of [`CARRY_FORWARD`] needs one bullet
//!   that holds all of its words, so a topic cannot be "covered" by words scattered over unrelated bullets, and the gaps against a security
//!   requirement are named as gaps.
//!
//! Each check is a function over the document text, and `planted_gaps_are_found` shows that it refuses a document with the gap it is meant
//! to catch (a misspelled proof, a missing letter, a requirement nobody defined, a residual risk that was dropped).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const HEADING: &str = "## Registry and read API (05)";
/// The requirements prompt 05 owns or shapes. The section must speak to each one.
const REQUIREMENTS: [&str; 11] = [
    "S4", "S5", "S9", "S10", "S11", "S13", "S15", "S17", "S21", "S22", "S25",
];
const STRIDE_LETTERS: [&str; 6] = ["S", "T", "R", "I", "D", "E"];
const PARTS: [&str; 4] = [
    "### Assets",
    "### Trust boundaries",
    "### STRIDE",
    "### Residual risks",
];
/// The fewest residual risks the section may list. The registry has many limits; a short list means some were left out.
const MIN_RESIDUAL_RISKS: usize = 24;

/// The limits that the registry's tasks and decisions recorded. Each topic needs one residual-risk bullet that contains every needle.
const CARRY_FORWARD: [(&str, &[&str]); 27] = [
    (
        "parallel swimlane commits are not atomic together",
        &["persist_concurrency", "parallel", "no atomicity"],
    ),
    (
        "the adoption base-commit search and the unscanned secret scan",
        &["base commit", "untagged", "unscanned"],
    ),
    (
        "raw text, blobs, diff hunks and search lines are not masked (A28)",
        &["A28", "diff hunks", "S14b", "gap against"],
    ),
    (
        "only_differing hides secret-to-secret changes",
        &["only_differing", "secret to a secret"],
    ),
    (
        "a short needle is a repeatable scan",
        &["one or two character needle", "220", "290 ms"],
    ),
    ("the corpora's memory", &["67.9 MiB", "256 MiB", "three replicas"]),
    (
        "staleness assumes the swimlane's version moves with its files",
        &["swimlane's version", "moves with or after"],
    ),
    (
        "a cursor is forgeable for the same query only",
        &["cursor", "same query", "signature"],
    ),
    (
        "the served route's slots are shared and a viewer can fill them",
        &["16", "MCP", "served", "slots"],
    ),
    ("two drift lists, two orders", &["two orders", "REST", "by path"]),
    (
        "alert-hold chaining is capped and the window events are the agent's",
        &["D102", "compromised agent", "window"],
    ),
    (
        "A33: baselines at the wrong commit if Git moves between the sync and the close",
        &["A33", "between the sync", "close"],
    ),
    (
        "sync_windows has no retention yet",
        &["sync_windows", "retention"],
    ),
    (
        "denied files: old versions stay, compare_tree and FileObserved name them",
        &["blob store", "compare_tree", "FileObserved", "D79"],
    ),
    (
        "pickup state trusts what the agent reports, and has no per-tenant targeting",
        &["pod start", "per-tenant", "config-server start"],
    ),
    (
        "tool-write registrations (D120)",
        &["D120", "TTL", "identical content"],
    ),
    ("attribution (D123)", &["D123", "sentinel", "not used up"]),
    (
        "severity rules are raise-only, stale after a change, and some defaults never fire",
        &["raise-only", "change_kinds", "setting_glob", "stale"],
    ),
    (
        "a lying agent can raise or hide a C12 for its own swimlane only",
        &["lying", "its own swimlane", "C12"],
    ),
    (
        "property-source merges are not verified as served, and the sample is 200 a night",
        &["property source", "not verified", "200"],
    ),
    (
        "the structure checks lag and a restart forgets them",
        &["C9", "full pass", "ten minutes"],
    ),
    (
        "drift alerts are at most once and skip git_ahead and unknown",
        &["D98", "at most once", "unknown"],
    ),
    (
        "roles are global: a viewer reads every swimlane",
        &["D35", "every swimlane", "viewer"],
    ),
    (
        "findings are not audited (rule 8 read as state changes of decisions)",
        &["D93", "rule 8", "findings"],
    ),
    (
        "there is no retention yet: storage grows",
        &["retention", "blob_refs", "grows"],
    ),
    (
        "an agent's times and facts are its own",
        &["observed_at", "clamped", "backdate"],
    ),
    (
        "gaps against a security requirement, named",
        &["Gaps against requirements", "S13", "S14b", "S9"],
    ),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn threat_model() -> String {
    read(&repo().join("docs/threat-model.md"))
}

// ------------------------------------------------------------------------------------------------ reading the document

/// The text of the registry's section: from its heading to the next `## ` heading.
fn registry_section(doc: &str) -> Result<String, String> {
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

/// The names a proof cell may cite: the functions that are tests (`#[test]`, `#[tokio::test]`, also inside `proptest!`) in
/// `crates/hub-registry` (its `src`, `tests` and the benchmarks' own tests), and the file stems of the benchmarks and of the fuzz targets.
fn known_proofs() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "snapshots") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.extend(test_functions(&read(&path)));
            }
        }
    }
    let mut names = BTreeSet::new();
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    for dir in ["src", "tests"] {
        walk(&base.join(dir), &mut names);
    }
    for dir in ["benches", "fuzz/fuzz_targets"] {
        if let Ok(files) = fs::read_dir(base.join(dir)) {
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "rs")
                    && let Some(stem) = path.file_stem()
                {
                    names.insert(stem.to_string_lossy().into_owned());
                }
            }
        }
    }
    names
}

/// The functions in `source` whose attributes include a test attribute.
fn test_functions(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut is_test = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test") || trimmed.starts_with("#[tokio::test") {
            is_test = true;
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("//") || trimmed.is_empty() {
            continue;
        }
        let rest = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        if is_test && let Some(after) = rest.strip_prefix("fn ") {
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                found.push(name);
            }
        }
        is_test = false;
    }
    found
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
    let section = match registry_section(doc) {
        Ok(section) => section,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    let mut last = 0;
    for heading in PARTS {
        match section.find(&format!("\n{heading}\n")) {
            None => problems.push(format!("the registry section has no `{heading}` part")),
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
        if items < MIN_RESIDUAL_RISKS {
            problems.push(format!(
                "only {items} residual risks are listed; each known limit of the registry belongs here"
            ));
        }
    }
    problems
}

/// Problems with the STRIDE table: every letter has a row, and every row names a mitigation, a proof and a requirement.
fn stride_problems(doc: &str) -> Vec<String> {
    let Ok(section) = registry_section(doc) else {
        return vec!["no registry section".to_owned()];
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
        if row.proof.starts_with("None") && !code_spans(&row.proof).is_empty() {
            problems.push(format!(
                "the row `{label}` says `None` but names a proof in backticks; a built control has no `None`"
            ));
        }
    }
    problems
}

/// Problems with the requirements: each of the registry's is cited, and each cited S# is real.
fn requirement_problems(doc: &str, defined: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = registry_section(doc) else {
        return vec!["no registry section".to_owned()];
    };
    let cited = requirement_ids(&section);
    let mut problems = Vec::new();
    for id in REQUIREMENTS {
        if !cited.contains(id) {
            problems.push(format!("the registry section never cites {id}"));
        }
    }
    for id in &cited {
        if !defined.contains(id) {
            problems.push(format!(
                "the registry section cites {id}, which docs/security.md does not define"
            ));
        }
    }
    problems
}

/// Problems with the proofs: every name in a Proof cell is a test, a benchmark or a fuzz target of the registry.
fn proof_problems(doc: &str, known: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = registry_section(doc) else {
        return vec!["no registry section".to_owned()];
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
                    "the proof `{name}` is not a test, bench or fuzz target of hub-registry"
                ));
            }
        }
    }
    problems
}

/// Problems with the residual risks: each carried-forward limit has a bullet that holds all of its words.
fn carry_forward_problems(doc: &str) -> Vec<String> {
    let Ok(section) = registry_section(doc) else {
        return vec!["no registry section".to_owned()];
    };
    let Some(risks) = part(&section, "### Residual risks") else {
        return vec!["no Residual risks part".to_owned()];
    };
    let bullets: Vec<&str> = risks.split("\n- **").skip(1).collect();
    CARRY_FORWARD
        .iter()
        .filter(|(_, needles)| !bullets.iter().any(|b| needles.iter().all(|n| b.contains(n))))
        .map(|(topic, needles)| {
            format!("no residual risk covers {topic} (one bullet with all of {needles:?})")
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ the tests

#[test]
fn registry_section_has_assets_boundaries_stride_and_residual_risks() {
    let problems = shape_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn stride_table_has_all_six_letters_and_every_row_is_complete() {
    let problems = stride_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn section_cites_the_registrys_requirements_and_only_real_ones() {
    let defined = defined_requirements();
    assert!(
        REQUIREMENTS.iter().all(|id| defined.contains(*id)),
        "docs/security.md no longer defines one of {REQUIREMENTS:?}: {defined:?}"
    );
    let problems = requirement_problems(&threat_model(), &defined);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_proof_the_stride_table_names_exists() {
    let known = known_proofs();
    assert!(
        known.len() > 900,
        "the scan of the crate's sources found only {} tests, benches and fuzz targets",
        known.len()
    );
    let problems = proof_problems(&threat_model(), &known);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn residual_risks_cover_every_limit_the_tasks_and_decisions_recorded() {
    let problems = carry_forward_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_component_list_names_the_registry() {
    assert!(threat_model().contains("- Registry and read API (05)"));
}

#[test]
fn test_functions_finds_tests_and_not_helpers() {
    let source = "\
fn helper() {}
#[test]
fn plain() {}
#[tokio::test]
#[ignore = \"needs a cloud project\"]
async fn with_other_attributes() {}
proptest! {
    #[test]
    fn inside_a_macro(x in 0..3) {}
}
// #[test]
fn commented_out_attribute_does_not_count() {}
#[cfg(test)]
fn gated_helper() {}
";
    assert_eq!(
        test_functions(source),
        ["plain", "with_other_attributes", "inside_a_macro"]
    );
}

#[test]
fn planted_gaps_are_found() {
    let doc = threat_model();
    let defined = defined_requirements();
    let known = known_proofs();
    // The real document passes every check, so each failure below is caused by the plant.
    assert!(shape_problems(&doc).is_empty());
    assert!(stride_problems(&doc).is_empty());
    assert!(requirement_problems(&doc, &defined).is_empty());
    assert!(proof_problems(&doc, &known).is_empty());
    assert!(carry_forward_problems(&doc).is_empty());

    // No section at all.
    let without = doc.replace(HEADING, "## Something else");
    assert!(!shape_problems(&without).is_empty());

    // A part missing.
    let section = registry_section(&doc).unwrap();
    let no_residual = doc.replace(
        "### Residual risks\n\n- **Parallel",
        "### Leftovers\n\n- **Parallel",
    );
    assert!(
        shape_problems(&no_residual)
            .iter()
            .any(|p| p.contains("Residual risks")),
        "the plant found no `### Residual risks` heading of the registry's section"
    );

    // A STRIDE letter missing: rename every `| E |` row of the registry's table.
    let stride = part(&section, "### STRIDE").unwrap();
    let no_e = stride.replace("\n| E |", "\n| X |");
    assert_ne!(no_e, stride, "the plant found no `| E |` row");
    let doc_no_e = doc.replace(stride, &no_e);
    assert!(stride_problems(&doc_no_e).iter().any(|p| p.contains("`E`")));

    // A requirement the section does not cite, and one that docs/security.md does not define.
    let mut defined_without_s25 = defined.clone();
    defined_without_s25.remove("S25");
    assert!(
        requirement_problems(&doc, &defined_without_s25)
            .iter()
            .any(|p| p.contains("S25"))
    );
    let no_s22 = section.replace("S22", "S-22");
    assert_ne!(no_s22, section);
    let doc_no_s22 = doc.replace(&section, &no_s22);
    assert!(
        requirement_problems(&doc_no_s22, &defined)
            .iter()
            .any(|p| p.contains("S22"))
    );

    // A proof that was never written (a misspelled one), and one that is a helper and not a test.
    let rows = stride_rows(stride).unwrap();
    let first_proof = rows
        .iter()
        .find_map(|r| code_spans(&r.proof).first().copied())
        .unwrap()
        .to_owned();
    let invented = doc.replacen(&format!("`{first_proof}`"), "`a_test_nobody_wrote`", 1);
    assert!(
        proof_problems(&invented, &known)
            .iter()
            .any(|p| p.contains("a_test_nobody_wrote"))
    );
    let misspelled = doc.replacen(&format!("`{first_proof}`"), &format!("`{first_proof}x`"), 1);
    assert!(
        proof_problems(&misspelled, &known)
            .iter()
            .any(|p| p.contains(&format!("{first_proof}x"))),
        "a proof with one letter too many must not count"
    );
    let helper = doc.replacen(&format!("`{first_proof}`"), "`fn_that_is_not_a_test`", 1);
    assert!(!proof_problems(&helper, &known).is_empty());
    assert!(
        !known.contains("row") && !known.contains("part") && !known.contains("repo"),
        "this file's own helpers are not proofs"
    );
    let helper = doc.replacen(&format!("`{first_proof}`"), "`registry_section`", 1);
    assert!(
        proof_problems(&helper, &known)
            .iter()
            .any(|p| p.contains("registry_section")),
        "a function that is not a test must not count as a proof"
    );

    // A `None` that still names a proof, and a row with no proof at all.
    let none_with_name = doc.replacen(&format!("`{first_proof}`"), &format!("None (`{first_proof}`)"), 1);
    assert!(
        stride_problems(&none_with_name)
            .iter()
            .any(|p| p.contains("says `None`"))
    );
    let whole_cell = &rows[0].proof;
    assert!(
        whole_cell.contains(&first_proof),
        "the first row is the first proof's row"
    );
    let no_proof = doc.replacen(whole_cell, "nothing at all", 1);
    assert!(
        stride_problems(&no_proof)
            .iter()
            .any(|p| p.contains("names no proof"))
    );

    // A residual risk that is dropped, and a gap that is no longer called a gap.
    let without_parallel = doc.replacen("persist_concurrency", "persist_threads", 1);
    assert_ne!(without_parallel, doc);
    assert!(
        carry_forward_problems(&without_parallel)
            .iter()
            .any(|p| p.contains("parallel swimlane commits")),
        "a bullet that lost one of its words is found"
    );
    let softened = section.replace("Gaps against requirements", "Notes on requirements");
    assert_ne!(softened, section);
    assert!(
        carry_forward_problems(&doc.replace(&section, &softened))
            .iter()
            .any(|p| p.contains("gaps against a security requirement")),
        "a section that stops naming its gaps as gaps is refused"
    );
    let fewer = section.replace("\n- **", "\n- ");
    assert!(
        shape_problems(&doc.replace(&section, &fewer))
            .iter()
            .any(|p| p.contains("residual risks are listed")),
        "bullets that are no longer risks are not counted"
    );
}
