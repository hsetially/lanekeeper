//! The threat model of the hub assembly is a document that can be checked (plan 18, T8; AGENTS.md "Update the threat model"): a source
//! scan of `docs/threat-model.md`, section "Hub assembly (18)".
//!
//! - **Every requirement the prompt lists is in a STRIDE row**, and every one of the six letters has rows.
//! - **Every test the section names exists.** A name in backticks of the form `file::test` (or `file::{a, b}`) is looked for as a
//!   `fn` in `crates/hub/tests/file.rs`, `crates/hub-platform/tests/file.rs` or the crate's own sources, so a renamed or deleted
//!   test makes the document wrong visibly and not silently.
//! - **Every decision it cites is in `docs/decisions.md`**, and the decisions of the plan gate are there as decided by the human.
//! - **It states what the composition does not prove and the residual risks the plan promised** (an unaudited migration run, a seed
//!   run that does not record its operator, the buffer of the large edit bodies, the MCP session affinity, identity's second CSRF
//!   check, the admin tables with no route), and the one that came after it: the web listener's own TLS key, the one private key
//!   built from bytes (D145).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn threat_model() -> String {
    fs::read_to_string(root().join("docs/threat-model.md")).unwrap()
}

/// The text of "## Hub assembly (18)" up to the next `## ` heading.
fn section() -> String {
    let text = threat_model();
    let start = text
        .find("\n## Hub assembly (18)\n")
        .expect("the threat model has no section \"Hub assembly (18)\"");
    let body = &text[start + 1..];
    let end = body[3..].find("\n## ").map_or(body.len(), |i| i + 3);
    body[..end].to_owned()
}

/// The text of a `### ` subsection of the section, up to the next `### `.
fn subsection(section: &str, title_starts_with: &str) -> String {
    let marker = format!("\n### {title_starts_with}");
    let start = section
        .find(&marker)
        .unwrap_or_else(|| panic!("no subsection \"{title_starts_with}\""));
    let body = &section[start + 1..];
    let end = body[3..].find("\n### ").map_or(body.len(), |i| i + 3);
    body[..end].to_owned()
}

/// The rows of the STRIDE tables of a block: (letter, threat, mitigation, proof, requirements).
fn stride_rows(block: &str) -> Vec<[String; 5]> {
    block
        .lines()
        .filter(|l| l.starts_with("| "))
        .filter_map(|l| {
            let cells: Vec<String> = l
                .trim_matches('|')
                .split(" | ")
                .map(|c| c.trim().to_owned())
                .collect();
            (cells.len() == 5 && matches!(cells[0].as_str(), "S" | "T" | "R" | "I" | "D" | "E")).then(|| {
                [
                    cells[0].clone(),
                    cells[1].clone(),
                    cells[2].clone(),
                    cells[3].clone(),
                    cells[4].clone(),
                ]
            })
        })
        .collect()
}

#[test]
fn the_section_has_stride_rows_for_all_six_letters_and_for_every_requirement_of_the_prompt() {
    let section = section();
    let composition = subsection(&section, "STRIDE for the composition");
    let rows = stride_rows(&composition);
    assert!(
        rows.len() >= 25,
        "only {} STRIDE rows for the composition",
        rows.len()
    );
    for letter in ["S", "T", "R", "I", "D", "E"] {
        assert!(
            rows.iter().filter(|r| r[0] == letter).count() >= 2,
            "fewer than two {letter} rows in the STRIDE table of the composition"
        );
    }
    // The prompt lists these requirements (prompts/18-hub-assembly.md, "Security"), and P11 and P7 for the budgets it adds.
    let named: BTreeSet<String> = rows
        .iter()
        .flat_map(|r| r[4].split([',', ' ']).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    for requirement in [
        "S1", "S2", "S3", "S4", "S5", "S6", "S7", "S9", "S10", "S12", "S16", "S18", "S21",
    ] {
        assert!(
            named.contains(requirement),
            "{requirement} is not in the requirements column of any STRIDE row of the hub assembly"
        );
    }
    for budget in ["P11", "P7"] {
        assert!(composition.contains(budget), "{budget} is not named");
    }
    // Every row says how it was proved or why it cannot be.
    for row in &rows {
        assert!(
            row[3].len() >= 8 && row[2].len() >= 20 && row[1].len() >= 20,
            "a STRIDE row is a stub: {row:?}"
        );
    }
}

#[test]
fn the_section_names_what_the_composition_does_not_prove_and_the_promised_residual_risks() {
    let section = section();
    let not_proved = subsection(&section, "What the composition does not prove");
    for needle in [
        "Google, Entra or GitHub",
        "load balancers",
        "The cluster",
        "The image",
        "three replicas",
        "real browser",
    ] {
        assert!(
            not_proved.contains(needle),
            "\"What the composition does not prove\" omits {needle}"
        );
    }
    let residual = subsection(&section, "Residual risks");
    for (needle, why) in [
        ("audit chain (D138)", "an unaudited migration run"),
        (
            "operator of a seed run is not recorded (D139)",
            "the seed's operator",
        ),
        (
            "moves the swimlane",
            "a seed run that changes a swimlane's project",
        ),
        (
            "12 MiB per request is buffered",
            "the buffer of the large edit bodies (G8)",
        ),
        (
            "MCP session affinity is unverified on GCP",
            "the MCP session affinity",
        ),
        (
            "second `AuthLayer` and `CsrfLayer`",
            "identity's own inner layers (G1)",
        ),
        ("request-id generators", "three request-id generators (G17)"),
        (
            "same KMS key for both CAs is refused only when the two references are equal",
            "one key for both CAs, now refused when equal as written (D144)",
        ),
        (
            "web listener's private key is in the hub's memory",
            "the web listener's key and rustls's copy of it (D145, S5)",
        ),
        (
            "admin tables have no route",
            "the 33 admin operations with no owner (D112)",
        ),
        (
            "document search is absent",
            "the unavailable document search (D140)",
        ),
    ] {
        assert!(residual.contains(needle), "the residual risks omit {why}");
    }
}

/// Backticked spans of a block.
fn spans(block: &str) -> Vec<String> {
    block
        .split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_owned())
        .collect()
}

/// Every `fn name` that is declared in the files a module name could mean.
fn declared_functions(module: &str) -> Option<String> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<PathBuf> = vec![
        crate_dir.join("tests").join(format!("{module}.rs")),
        crate_dir
            .join("../hub-platform/tests")
            .join(format!("{module}.rs")),
        crate_dir.join("src").join(format!("{module}.rs")),
    ];
    let dir = crate_dir.join("src").join(module);
    if dir.is_dir() {
        for entry in fs::read_dir(dir).unwrap() {
            files.push(entry.unwrap().path());
        }
    }
    let found: Vec<String> = files
        .iter()
        .filter(|f| f.is_file())
        .map(|f| fs::read_to_string(f).unwrap())
        .collect();
    (!found.is_empty()).then(|| found.join("\n"))
}

#[test]
fn every_test_the_section_names_exists() {
    let section = section();
    let mut checked = 0;
    let mut missing = Vec::new();
    for span in spans(&section) {
        // `module::name`, `module::*` or `module::{a, b}`; anything else in backticks is not a reference to a test.
        let Some((module, rest)) = span.split_once("::") else {
            continue;
        };
        if module.is_empty()
            || !module
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            continue;
        }
        let Some(source) = declared_functions(module) else {
            continue;
        };
        let names: Vec<&str> = if let Some(list) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            list.split(',').map(str::trim).collect()
        } else {
            vec![rest.trim()]
        };
        for name in names {
            if name == "*" {
                checked += 1;
                continue;
            }
            if !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                continue;
            }
            checked += 1;
            if !source.contains(&format!("fn {name}(")) {
                missing.push(format!("{module}::{name}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the threat model names tests that do not exist: {missing:?}"
    );
    assert!(
        checked >= 60,
        "only {checked} test names were checked: the scan is not reading the section"
    );
}

#[test]
fn every_decision_the_section_cites_is_in_the_decisions_file_and_the_plan_gate_ones_are_the_humans() {
    let decisions = fs::read_to_string(root().join("docs/decisions.md")).unwrap();
    let section = section();
    let mut cited = BTreeSet::new();
    let bytes = section.as_bytes();
    for (i, window) in bytes.windows(2).enumerate() {
        if window[0] == b'D' && window[1].is_ascii_digit() {
            let digits: String = section[i + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if digits.len() == 3 {
                cited.insert(digits);
            }
        }
    }
    for n in &cited {
        let n: u32 = n.parse().unwrap();
        // D136 is the budgets' decision on `main`; this branch does not carry it and the section does not cite it.
        assert!(
            decisions.contains(&format!("- **D{n}**")) || n < 100,
            "the section cites D{n}, which docs/decisions.md does not have"
        );
    }
    for n in 138..=144 {
        let entry = decisions
            .lines()
            .find(|l| l.starts_with(&format!("- **D{n}**")))
            .unwrap_or_else(|| panic!("D{n} is not in docs/decisions.md"));
        assert!(
            entry.contains("Decided by the human, at the approval gate of plan 18"),
            "D{n} does not say who decided it"
        );
    }
    for n in [137, 138, 139, 140, 141, 142, 145] {
        assert!(cited.contains(&n.to_string()), "the section does not cite D{n}");
    }
    // D145 is the human's choice after the plan gate (option A of blocker B1), recorded by an agent from the orchestrator's relay.
    let d145 = decisions
        .lines()
        .find(|l| l.starts_with("- **D145**"))
        .expect("D145 is not in docs/decisions.md");
    assert!(
        d145.contains("Decided by the human, in chat, on 2026-10-27") && d145.contains("pull/39"),
        "D145 does not say who decided it and where the allowlist line is"
    );
}
