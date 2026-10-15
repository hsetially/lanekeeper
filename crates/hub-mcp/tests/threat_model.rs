//! Plan 07, T5 (S24): the MCP server's section of `docs/threat-model.md` is complete, and it stays true to the code.
//!
//! A threat model that nobody checks drifts away from the code it describes. These tests read the `## MCP server (07)` section and
//! check what a machine can check:
//!
//! - the four parts are there, in order, and every STRIDE letter has a row; every row names a threat, a mitigation and the
//!   requirements it answers (`mcp_section_has_all_parts_and_six_stride_letters`);
//! - every S# the section cites exists in `docs/security.md`, and the section speaks to each requirement this prompt owns
//!   (`every_cited_s_number_exists`);
//! - every test, benchmark or fuzz target that a Proof cell names exists, and a name must belong to a function that really is a
//!   test, so a row cannot cite a helper, an ignored test or a test nobody wrote. A proof that lives in another crate is written
//!   `crate-dir:test_name` and is looked up in that crate (`every_named_proof_exists`);
//! - the residual risks carry the deviations that a human accepted for now, and the limits that earlier steps recorded
//!   (`the_residual_risks_carry_the_accepted_deviations_and_the_known_limits`), and the tests that D-1 and D-2 name in their own
//!   words exist (`the_tests_the_two_deviations_name_exist`);
//! - the numbers the section states are the code's own (`the_numbers_in_the_section_are_the_ones_in_the_code`), and every tool that
//!   acts on the world is named (`every_tool_that_acts_on_the_world_is_named`);
//! - a control that is not built yet is marked `None (T6)` or `None (T7)` **exactly while** the code that would build it is
//!   missing, and the same holds for each gap that the section reports: when the code changes, the section has to change with it
//!   (`pending_controls_and_gaps_are_marked_exactly_while_the_code_has_them`).
//!
//! `planted_gaps_are_found` shows that each check refuses a document with the gap it is meant to catch.
//!
//! The tests need no database and no network. A proof the section marks as running against Postgres is a test that prints
//! `SKIPPED` without one unless `LK_REQUIRE_DOCKER=1` (which `just verify-07` sets); this file checks that it exists, not that it
//! ran.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hub_mcp::auth::pat::{DEFAULT_CACHE_TTL, DEFAULT_LIFETIME, DEFAULT_MAX_LIVE};
use hub_mcp::clock::SystemClock;
use hub_mcp::confirm::message::MAX_CHARS;
use hub_mcp::confirm::replay;
use hub_mcp::limits::{Class, LimitSettings, Limits};
use hub_mcp::schemas::{Contract, READ_CAP_BYTES, WRITE_CAP_BYTES};
use hub_mcp::server::MAX_BODY_BYTES;

const HEADING: &str = "## MCP server (07)";
const COMPONENT_LINE: &str = "- MCP server (07)";
const STRIDE_LETTERS: [&str; 6] = ["S", "T", "R", "I", "D", "E"];
const PARTS: [&str; 4] = [
    "### Assets",
    "### Trust boundaries",
    "### STRIDE",
    "### Residual risks",
];
/// The requirements this prompt owns (S4, S11, S14, S14b, S15, S21) and the ones that shape its work. The section speaks to each.
const REQUIREMENTS: [&str; 12] = [
    "S1", "S3", "S4", "S7", "S9", "S10", "S11", "S14", "S14b", "S15", "S21", "S22",
];
/// Fewer than this many residual risks means the known limits of the component are not all written down.
const MIN_RESIDUAL_RISKS: usize = 18;
/// Other crates whose tests a Proof cell may cite as `crate-dir:test_name`.
const OTHER_CRATES: [&str; 3] = ["hub", "hub-identity", "xtask"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo() -> PathBuf {
    crate_dir().join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn threat_model() -> String {
    read(&repo().join("docs/threat-model.md"))
}

// ------------------------------------------------------------------------------------------------ reading the document

/// The text of this component's section: from its heading to the next `## ` heading. The heading must be there exactly once.
fn section_of(doc: &str) -> Result<String, String> {
    let headings = doc.lines().filter(|l| l.trim_end() == HEADING).count();
    match headings {
        0 => return Err(format!("no `{HEADING}` heading in docs/threat-model.md")),
        1 => {}
        n => {
            return Err(format!(
                "{n} `{HEADING}` headings in docs/threat-model.md; one section holds it all"
            ));
        }
    }
    let mut inside = false;
    let mut out = Vec::new();
    for line in doc.lines() {
        if inside {
            if line.starts_with("## ") {
                break;
            }
            out.push(line);
        } else if line.trim_end() == HEADING {
            inside = true;
        }
    }
    // A leading newline, so that the first part's heading is found like the others.
    Ok(format!("\n{}", out.join("\n")))
}

/// The text under `### <part>` up to the next `### ` heading.
fn part<'a>(section: &'a str, heading: &str) -> Option<&'a str> {
    let mut offset = 0_usize;
    let mut start = None;
    for line in section.lines() {
        if line.trim_end() == heading {
            start = Some(offset);
            break;
        }
        offset += line.len() + 1;
    }
    let body = &section[start?..];
    let after_heading = body.find('\n').map_or(body.len(), |n| n + 1);
    let end = body[after_heading..]
        .find("\n### ")
        .map_or(body.len(), |n| after_heading + n);
    Some(&body[after_heading..end])
}

/// One row of the STRIDE table: `| letter | threat | mitigation | proof | S# |`.
#[derive(Debug, Clone)]
struct Row {
    letter: String,
    threat: String,
    mitigation: String,
    proof: String,
    requirements: String,
}

fn stride_rows(stride: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    let mut header_seen = false;
    for line in stride.lines().filter(|l| l.trim_start().starts_with('|')) {
        let trimmed = line.trim();
        let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
        let inner = inner.strip_suffix('|').unwrap_or(inner);
        let cells: Vec<&str> = inner.split('|').map(str::trim).collect();
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

/// Every requirement id in `text`: `S` and digits, and the `b` of `S14b`. `S5x`, `SAN` and `SHA-256` are not ids.
fn requirement_ids(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut ids = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if bytes[i] == b'S' && starts_word {
            let digits = bytes[i + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
            let mut end = i + 1 + digits;
            // The `b` of `S14b` belongs to the id when the word ends right after it.
            if digits > 0
                && bytes.get(end) == Some(&b'b')
                && bytes.get(end + 1).is_none_or(|b| !b.is_ascii_alphanumeric())
            {
                end += 1;
            }
            let ends_word = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if digits > 0 && ends_word {
                ids.insert(text[i..end].to_owned());
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    ids
}

/// Every requirement that `docs/security.md` defines (`- **S14b Redaction.**`).
fn defined_requirements() -> BTreeSet<String> {
    let security = read(&repo().join("docs/security.md"));
    security
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("- **"))
        .filter_map(|l| l.split([' ', '.', '*']).next())
        .filter(|id| {
            let digits = id.get(1..).unwrap_or("").trim_end_matches('b');
            id.starts_with('S') && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_owned)
        .collect()
}

/// The residual risks, one string per bullet.
fn residual_risks(section: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in part(section, "### Residual risks").unwrap_or("").lines() {
        if line.starts_with("- **") {
            items.push(line.to_owned());
        } else if let Some(last) = items.last_mut() {
            if !line.trim().is_empty() {
                last.push(' ');
                last.push_str(line.trim());
            }
        }
    }
    items
}

// ------------------------------------------------------------------------------------------------ the code's side

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The name of the function a line declares, if it declares one.
fn fn_name(line: &str) -> Option<String> {
    let t = line.trim_start();
    let t = t
        .strip_prefix("pub(crate) ")
        .or_else(|| t.strip_prefix("pub(super) "))
        .or_else(|| t.strip_prefix("pub "))
        .unwrap_or(t);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let rest = t.strip_prefix("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The functions of one source file, as `(name, is_a_test)`. A test has `#[test]` or `#[tokio::test..]` among the attribute lines
/// right above it (and no `#[ignore]`), or sits inside a `proptest! { .. }` block.
fn functions_in(text: &str) -> Vec<(String, bool)> {
    let lines: Vec<&str> = text.lines().collect();
    let blocks: Vec<(usize, usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("proptest!"))
        .map(|(start, l)| {
            let at = indent(l);
            let end = (start + 1..lines.len())
                .find(|&m| indent(lines[m]) == at && lines[m].trim_start().starts_with('}'))
                .unwrap_or(lines.len());
            (start, end, at)
        })
        .collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(name) = fn_name(line) else { continue };
        let (mut marked, mut ignored) = (false, false);
        let mut j = i;
        while j > 0 {
            j -= 1;
            let above = lines[j].trim();
            if above.starts_with("#[test]") || above.starts_with("#[tokio::test") {
                marked = true;
            } else if above.starts_with("#[ignore") {
                ignored = true;
            } else if above.starts_with("#[") || above.starts_with("///") || above.starts_with("//") {
                // Another attribute or a comment between the attributes and the function.
            } else {
                break;
            }
        }
        let in_block = blocks
            .iter()
            .any(|&(start, end, at)| start < i && i < end && indent(line) > at);
        out.push((name, (marked || in_block) && !ignored));
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skip = path.file_name().is_some_and(|n| {
                [
                    "target",
                    "corpus",
                    "snapshots",
                    "golden",
                    "proptest-regressions",
                    "artifacts",
                ]
                .iter()
                .any(|s| n == *s)
            });
            if !skip {
                walk(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The names of the real tests under `dirs` of a crate (`src`, `tests`).
fn tests_under(crate_root: &Path, dirs: &[&str]) -> BTreeSet<String> {
    let mut files = Vec::new();
    for dir in dirs {
        walk(&crate_root.join(dir), &mut files);
    }
    files
        .iter()
        // No proof cites a threat-model test, and this file's own samples must not become names a proof could cite.
        .filter(|f| f.file_name().is_some_and(|n| n != "threat_model.rs"))
        .flat_map(|f| functions_in(&read(f)))
        .filter(|(_, is_test)| *is_test)
        .map(|(name, _)| name)
        .collect()
}

/// What a Proof cell of this crate may name: its tests, the functions of its benchmarks, and its fuzz targets by file name.
fn known_proofs() -> BTreeSet<String> {
    let root = crate_dir();
    let mut names = tests_under(&root, &["src", "tests"]);
    let mut benches = Vec::new();
    walk(&root.join("benches"), &mut benches);
    names.extend(
        benches
            .iter()
            .flat_map(|f| functions_in(&read(f)))
            .map(|(n, _)| n),
    );
    if let Ok(targets) = fs::read_dir(root.join("fuzz/fuzz_targets")) {
        for target in targets.flatten() {
            if let Some(stem) = target.path().file_stem() {
                names.insert(stem.to_string_lossy().into_owned());
            }
        }
    }
    names
}

/// The tests of the other crates a Proof cell may cite.
fn other_crate_tests() -> Vec<(String, BTreeSet<String>)> {
    OTHER_CRATES
        .iter()
        .map(|dir| {
            let root = repo().join("crates").join(dir);
            ((*dir).to_owned(), tests_under(&root, &["src", "tests"]))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ the checks

/// Problems with the shape: the section exists once and has its four parts, in order, each with content; every STRIDE letter has a
/// row; every row is complete; there are enough residual risks.
fn shape_problems(doc: &str) -> Vec<String> {
    let section = match section_of(doc) {
        Ok(section) => section,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    let mut last = 0;
    for heading in PARTS {
        match section.find(&format!("\n{heading}\n")) {
            None => problems.push(format!("the section has no `{heading}` part")),
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
    let items = residual_risks(&section).len();
    if items < MIN_RESIDUAL_RISKS {
        problems.push(format!(
            "only {items} residual risks are listed (at least {MIN_RESIDUAL_RISKS}); each known limit of the component belongs here"
        ));
    }
    let Some(stride) = part(&section, "### STRIDE") else {
        problems.push("no STRIDE part".to_owned());
        return problems;
    };
    let rows = match stride_rows(stride) {
        Ok(rows) => rows,
        Err(problem) => {
            problems.push(problem);
            return problems;
        }
    };
    for letter in STRIDE_LETTERS {
        if !rows.iter().any(|r| r.letter == letter) {
            problems.push(format!("the STRIDE table has no `{letter}` row"));
        }
    }
    for row in &rows {
        let label: String = row.threat.chars().take(48).collect();
        if !STRIDE_LETTERS.contains(&row.letter.as_str()) {
            problems.push(format!("`{}` is not a STRIDE letter", row.letter));
        }
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

/// Problems with the requirements: each one of the prompt's is cited, and each cited S# is real.
fn requirement_problems(doc: &str, defined: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
    };
    let cited = requirement_ids(&section);
    let mut problems = Vec::new();
    for id in REQUIREMENTS {
        if !cited.contains(id) {
            problems.push(format!("the section never cites {id}"));
        }
    }
    for id in &cited {
        if !defined.contains(id) {
            problems.push(format!(
                "the section cites {id}, which docs/security.md does not define"
            ));
        }
    }
    problems
}

/// Problems with the proofs: every name in a Proof cell is a test, a benchmark function or a fuzz target of this crate, or
/// `crate-dir:name` for a test of one of the other crates.
fn proof_problems(doc: &str, known: &BTreeSet<String>, others: &[(String, BTreeSet<String>)]) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
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
        for span in code_spans(&row.proof) {
            match span.split_once(':') {
                Some((krate, name)) => match others.iter().find(|(dir, _)| dir == krate) {
                    None => problems.push(format!("`{span}` names the crate `{krate}`, which a proof may not cite")),
                    Some((_, tests)) if !tests.contains(name) => {
                        problems.push(format!("the proof `{span}` is not a test of {krate}"));
                    }
                    Some(_) => {}
                },
                None if !known.contains(span) => problems.push(format!(
                    "the proof `{span}` is not a test, benchmark or fuzz target of hub-mcp (or it is a helper, or ignored)"
                )),
                None => {}
            }
        }
    }
    problems
}

/// Problems with the tests that the two accepted deviations (D-1 and D-2) name in their own words: they exist, like the ones in a
/// Proof cell. A name is a code span of lower-case words joined by at least two underscores, with or without a `crate-dir:` prefix.
fn deviation_proof_problems(
    doc: &str,
    known: &BTreeSet<String>,
    others: &[(String, BTreeSet<String>)],
) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
    };
    let mut problems = Vec::new();
    let mut named = 0;
    for bullet in residual_risks(&section)
        .iter()
        .filter(|b| b.starts_with("- **D-1") || b.starts_with("- **D-2"))
    {
        for span in code_spans(bullet) {
            let (krate, name) = span.split_once(':').map_or((None, span), |(k, n)| (Some(k), n));
            let is_a_name = name.matches('_').count() >= 2
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !is_a_name {
                continue;
            }
            named += 1;
            let found = match krate {
                None => known.contains(name),
                Some(k) => others.iter().any(|(dir, tests)| dir == k && tests.contains(name)),
            };
            if !found {
                problems.push(format!("a deviation names `{span}`, which is not a test"));
            }
        }
    }
    if named < 8 {
        problems.push(format!(
            "the deviations name only {named} tests; D-1 lists the tests that prove the route refuses a caller"
        ));
    }
    problems
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The statements about numbers that the section makes, as the code has them: `(what, the words the section must contain)`.
fn stated_facts() -> Vec<(&'static str, String)> {
    let l = LimitSettings::default();
    let deadline = Limits::new(l.clone(), Arc::new(SystemClock)).call_deadline(Class::Write, true);
    let mib = 1024 * 1024;
    vec![
        (
            "read rate",
            format!("{} reads a minute (burst {})", l.read_per_minute, l.read_burst),
        ),
        (
            "write rate",
            format!("{} writes a minute (burst {})", l.write_per_minute, l.write_burst),
        ),
        (
            "concurrency",
            format!(
                "{} calls at once per user and {} per replica",
                l.calls_per_user, l.calls_per_replica
            ),
        ),
        (
            "in-flight bytes",
            format!("{} MiB of file bytes in flight", l.inflight_bytes / mib),
        ),
        (
            "timeouts",
            format!(
                "{} s for a read, {} s for a write and {} s for a confirmation",
                l.read_timeout.as_secs(),
                l.write_timeout.as_secs(),
                l.confirm_timeout.as_secs()
            ),
        ),
        (
            "the deadline of a call that asks",
            format!("{} s for a call that asks", deadline.as_secs()),
        ),
        (
            "sessions",
            format!(
                "{} sessions per user and {} per replica",
                l.sessions_per_user, l.sessions_per_replica
            ),
        ),
        (
            "session idle time",
            format!("{} minutes idle", l.session_idle.as_secs() / 60),
        ),
        (
            "token lifetime",
            format!("{} days", DEFAULT_LIFETIME.as_secs() / 86_400),
        ),
        ("live tokens", format!("{DEFAULT_MAX_LIVE} live tokens per user")),
        ("token cache", format!("{} seconds", DEFAULT_CACHE_TTL.as_secs())),
        (
            "output caps",
            format!(
                "{} KB for a read tool and {} KB for a write tool",
                READ_CAP_BYTES / 1024,
                WRITE_CAP_BYTES / 1024
            ),
        ),
        (
            "question length",
            format!("{} characters", thousands(MAX_CHARS as u64)),
        ),
        (
            "replay memory",
            format!(
                "{} minutes and {} MiB",
                replay::TTL.as_secs() / 60,
                replay::MAX_BYTES / mib
            ),
        ),
        (
            "request body",
            format!("{} MiB request body", MAX_BODY_BYTES / (1024 * 1024)),
        ),
        (
            "owner table",
            format!(
                "twice the cap and at least {} entries",
                thousands(owners_capacity(1))
            ),
        ),
    ]
}

/// The capacity of the table that remembers who opened each session, for a replica cap: the code's own function.
fn owners_capacity(sessions_per_replica: u32) -> u64 {
    hub_mcp::server::owners_capacity(sessions_per_replica)
}

/// Problems with the stated numbers.
fn fact_problems(doc: &str, facts: &[(&str, String)]) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
    };
    facts
        .iter()
        .filter(|(_, words)| !section.contains(words.as_str()))
        .map(|(what, words)| format!("the section does not state {what} as the code has it: `{words}`"))
        .collect()
}

/// The tools the section must name: every tool that acts on the world (it asks the user first), and `propose_change`, which records
/// a draft without asking.
fn tools_to_name() -> Vec<String> {
    let contract = Contract::embedded().expect("the embedded contract loads");
    let names: Vec<String> = contract
        .tools()
        .iter()
        .filter(|t| t.acts_on_the_world() || t.name() == "propose_change")
        .map(|t| t.name().to_owned())
        .collect();
    assert!(
        names.len() >= 7,
        "the contract lists {} tools that write",
        names.len()
    );
    names
}

fn tool_problems(doc: &str, tools: &[String]) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
    };
    tools
        .iter()
        .filter(|t| !section.contains(&format!("`{t}`")))
        .map(|t| format!("the section never names the tool `{t}`"))
        .collect()
}

/// A limit that earlier steps recorded, or a control that the plan has and the code does not yet: each needs one residual risk
/// that holds every needle.
const CARRY_FORWARD: [(&str, &[&str]); 12] = [
    (
        "D-1: /mcp is a public route that authenticates itself",
        &[
            "D-1",
            "/mcp",
            "`public`",
            "Role(Viewer)",
            "rule 9",
            "WWW-Authenticate",
            "GuardedRouter::layer",
        ],
    ),
    (
        "D-2: S3 for the personal-token routes is Origin only",
        &["D-2", "S3", "Origin", "double-submit", "CSRF layer", "reviewer"],
    ),
    (
        "prompt injection reaching a write",
        &[
            "prompt injection",
            "approves without reading",
            "allowlist",
            "elicitation",
            "link",
        ],
    ),
    (
        "secrets leaving through output",
        &["S14b", "Q20", "false negative", "ScanConfig"],
    ),
    (
        "personal-token mode is the weaker credential",
        &["personal-token", "bearer", "7 days", "revocation", "no rotation"],
    ),
    (
        "sessions live in one replica's memory",
        &["A5", "one replica", "affinity", "prompt 09", "fail closed"],
    ),
    (
        "the choices of T3 that the plan did not fix",
        &[
            "T3",
            "draft mode",
            "does not check",
            "185 s",
            "only successful",
            "requires_approval",
        ],
    ),
    (
        "T0 has not been run",
        &["D91", "NOT RUN", "OAuth", "elicitation", "Cursor 3.22.12"],
    ),
    (
        "the Cursor plugin format is unverified",
        &["A9", "UNVERIFIED", "plugin"],
    ),
    (
        "get_effective_config is incomplete, and the denied-file hash is open",
        &["get_effective_config", "D92", "denied", "hash"],
    ),
    (
        "the hub binary does not mount the server",
        &["does not mount /mcp yet", "McpModule", "compose"],
    ),
    (
        "rmcp logs whole requests and warns about origins without a port",
        &["rmcp", "debug", "S10", "without a port", ":*"],
    ),
];

fn carry_forward_problems(doc: &str) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
    };
    let risks = residual_risks(&section);
    CARRY_FORWARD
        .iter()
        .filter(|(_, needles)| !risks.iter().any(|r| needles.iter().all(|n| r.contains(n))))
        .map(|(topic, _)| format!("no residual risk covers: {topic}"))
        .collect()
}

/// The words in the section that say a control is missing or a gap is open, each paired with whether the code still has the gap.
type Gaps = Vec<(&'static str, bool)>;

/// What the code is like today, for the markers that must come and go with it.
fn code_today() -> Gaps {
    let root = crate_dir();
    // The owner table forgets the owner of a live session once a replica may hold more sessions than the table holds. The gap is
    // open while the validator accepts a replica cap that the table cannot hold (the caps are the edges of what the validator takes).
    let owner_table_gap = [1_u32, 200, 4_095, 4_096, 4_097, 50_000, 100_000]
        .into_iter()
        .any(|cap| {
            let settings = LimitSettings {
                sessions_per_replica: cap,
                ..LimitSettings::default()
            };
            settings.validate().is_ok() && owners_capacity(cap) < u64::from(cap)
        });
    // The one log line of a tool call: the text from `tracing::info!(` with the target `hub_mcp::call` to its message.
    let handler = read(&root.join("src/server/handler.rs"));
    let log_line = handler
        .split("tracing::info!(")
        .skip(1)
        .find(|call| call.contains("target: \"hub_mcp::call\""))
        .and_then(|call| call.split("\"tool call\"").next())
        .expect("the tool call log line is in src/server/handler.rs");
    let hub_src = repo().join("crates/hub/src");
    let mut hub_files = Vec::new();
    walk(&hub_src, &mut hub_files);
    let mounted = hub_files.iter().any(|f| {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        name != "mcp.rs" && name != "lib.rs" && {
            let text = read(f);
            text.contains("McpModule") || text.contains("McpState")
        }
    });
    let missing = |path: &str| !root.join(path).exists();
    vec![
        ("None (T6): the final sweep", missing("src/output/sweep.rs")),
        (
            "None (T6): the planted-key matrix",
            missing("tests/mcp_auth/planted_key.rs"),
        ),
        (
            "None (T6): the denied-file matrix",
            missing("tests/mcp_auth/denied_files.rs"),
        ),
        (
            "None (T6): the redactor fuzz target",
            missing("fuzz/fuzz_targets/mcp_redactor.rs"),
        ),
        (
            "None (T7): notify_services through the gate",
            missing("src/tools/write/notify.rs"),
        ),
        ("The owner table", owner_table_gap),
        // The call line must name the caller (`user_id`); the section must not say that it does not.
        ("no user id", !log_line.contains("user_id")),
        (
            "prompt 09 does not mention it yet",
            !read(&repo().join("prompts/09-deployment-ci.md")).contains("Mcp-Session-Id"),
        ),
        ("does not mount /mcp yet", !mounted),
        (
            "T0 is NOT RUN",
            read(&repo().join("docs/decisions.md")).contains("| NOT RUN | NOT RUN |"),
        ),
    ]
}

fn marker_problems(doc: &str, code: &Gaps) -> Vec<String> {
    let Ok(section) = section_of(doc) else {
        return vec!["no section".to_owned()];
    };
    code.iter()
        .copied()
        .filter_map(|(marker, still_true)| match (still_true, section.contains(marker)) {
            (true, false) => Some(format!("the code still has this gap, and the section no longer says `{marker}`")),
            (false, true) => Some(format!(
                "the section says `{marker}`, and the code has changed: update the row and the residual risk"
            )),
            _ => None,
        })
        .collect()
}

fn listing_problems(doc: &str) -> Vec<String> {
    if doc.lines().any(|l| l.trim_end() == COMPONENT_LINE) {
        Vec::new()
    } else {
        vec![format!("the component list no longer names `{COMPONENT_LINE}`")]
    }
}

// ------------------------------------------------------------------------------------------------ the tests

#[test]
fn mcp_section_has_all_parts_and_six_stride_letters() {
    let problems = shape_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_cited_s_number_exists() {
    let defined = defined_requirements();
    assert!(
        REQUIREMENTS.iter().all(|id| defined.contains(*id)),
        "docs/security.md no longer defines one of {REQUIREMENTS:?}: {defined:?}"
    );
    let problems = requirement_problems(&threat_model(), &defined);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_named_proof_exists() {
    let known = known_proofs();
    assert!(
        known.len() > 300,
        "the scan of the crate's sources found only {} names",
        known.len()
    );
    let others = other_crate_tests();
    for (dir, tests) in &others {
        assert!(
            tests.len() > 20,
            "the scan of {dir} found only {} tests",
            tests.len()
        );
    }
    let problems = proof_problems(&threat_model(), &known, &others);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_tests_the_two_deviations_name_exist() {
    let problems = deviation_proof_problems(&threat_model(), &known_proofs(), &other_crate_tests());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_residual_risks_carry_the_accepted_deviations_and_the_known_limits() {
    let problems = carry_forward_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_numbers_in_the_section_are_the_ones_in_the_code() {
    let problems = fact_problems(&threat_model(), &stated_facts());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_tool_that_acts_on_the_world_is_named() {
    let problems = tool_problems(&threat_model(), &tools_to_name());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn pending_controls_and_gaps_are_marked_exactly_while_the_code_has_them() {
    let problems = marker_problems(&threat_model(), &code_today());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_component_is_still_listed_at_the_top_of_the_document() {
    let problems = listing_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_helpers_that_read_the_code_see_what_a_test_is() {
    // The proof check is only as good as its idea of a test.
    let source = "\
#[test]
fn plain() {}

#[tokio::test(start_paused = true)]
async fn asynchronous() {}

/// A comment.
#[test]
#[ignore = \"slow\"]
fn ignored() {}

fn helper() {}

mod inner {
    proptest! {
        fn in_a_block(x in 0..1) {}
    }

    fn after_the_block() {}
}
";
    let found: Vec<(String, bool)> = functions_in(source);
    let is_test = |name: &str| found.iter().find(|(n, _)| n == name).map(|(_, t)| *t);
    assert_eq!(is_test("plain"), Some(true));
    assert_eq!(is_test("asynchronous"), Some(true));
    assert_eq!(is_test("ignored"), Some(false));
    assert_eq!(is_test("helper"), Some(false));
    assert_eq!(is_test("in_a_block"), Some(true));
    assert_eq!(is_test("after_the_block"), Some(false));

    let ids = requirement_ids("S14b, S14 and S3. Not S5x, SAN, S256x or SHA-256; S9.");
    assert_eq!(
        ids.iter().map(String::as_str).collect::<Vec<_>>(),
        ["S14", "S14b", "S3", "S9"]
    );
}

/// `doc` with its MCP section changed by `change`, which must change something.
fn edited(doc: &str, change: impl Fn(&str) -> String) -> String {
    let section = section_of(doc).unwrap();
    let changed = change(&section);
    assert_ne!(changed, section, "the plant changed nothing");
    doc.replace(&section, &changed)
}

#[test]
fn planted_gaps_are_found() {
    let doc = threat_model();
    let defined = defined_requirements();
    let known = known_proofs();
    let others = other_crate_tests();
    let facts = stated_facts();
    let tools = tools_to_name();
    let code = code_today();
    // The real document passes every check, so each failure below is caused by the plant.
    assert!(shape_problems(&doc).is_empty());
    assert!(requirement_problems(&doc, &defined).is_empty());
    assert!(proof_problems(&doc, &known, &others).is_empty());
    assert!(carry_forward_problems(&doc).is_empty());
    assert!(fact_problems(&doc, &facts).is_empty());
    assert!(tool_problems(&doc, &tools).is_empty());
    assert!(marker_problems(&doc, &code).is_empty());
    assert!(listing_problems(&doc).is_empty());

    // No section at all, and two of them.
    assert!(!shape_problems(&doc.replace(HEADING, "## Something else")).is_empty());
    let twice = format!("{doc}\n{HEADING}\n\nanother\n");
    assert!(shape_problems(&twice).iter().any(|p| p.contains("headings")));

    // A part missing.
    let no_residual = edited(&doc, |s| s.replace("### Residual risks", "### Leftovers"));
    assert!(
        shape_problems(&no_residual)
            .iter()
            .any(|p| p.contains("Residual risks"))
    );

    // Too few residual risks: every bullet renamed.
    let thin = edited(&doc, |s| {
        let risks = part(s, "### Residual risks").unwrap();
        s.replace(risks, &risks.replace("\n- **", "\n* **"))
    });
    assert!(shape_problems(&thin).iter().any(|p| p.contains("residual risks")));

    // A STRIDE letter missing: rename every `| E |` row of the table.
    let no_e = edited(&doc, |s| s.replace("\n| E |", "\n| X |"));
    assert!(shape_problems(&no_e).iter().any(|p| p.contains("`E`")));

    // A row with a stray pipe is reported instead of silently misread.
    let stray = edited(&doc, |s| s.replacen("\n| S |", "\n| S | extra |", 1));
    assert!(!shape_problems(&stray).is_empty());

    // A row with no proof and no `None`.
    let section = section_of(&doc).unwrap();
    let rows = stride_rows(part(&section, "### STRIDE").unwrap()).unwrap();
    let first_proof = rows
        .iter()
        .find_map(|r| code_spans(&r.proof).first().map(|s| (*s).to_owned()))
        .unwrap();
    let proofless = edited(&doc, |s| {
        let row = rows.iter().find(|r| r.proof.contains(&first_proof)).unwrap();
        s.replacen(&row.proof, "nothing", 1)
    });
    assert!(
        shape_problems(&proofless)
            .iter()
            .any(|p| p.contains("names no proof"))
    );

    // A requirement the section does not cite, and one that docs/security.md does not define.
    let mut without_s22 = defined.clone();
    without_s22.remove("S22");
    assert!(
        requirement_problems(&doc, &without_s22)
            .iter()
            .any(|p| p.contains("S22"))
    );
    let no_s14b = edited(&doc, |s| s.replace("S14b", "S-14b"));
    assert!(
        requirement_problems(&no_s14b, &defined)
            .iter()
            .any(|p| p.contains("S14b"))
    );

    // A proof that was never written, one that is a helper, one in a crate that may not be cited, and a test of another crate that
    // does not exist.
    for (name, wanted) in [
        ("a_test_nobody_wrote", "a_test_nobody_wrote"),
        ("json_body", "json_body"),
        ("engine:marker_format", "engine"),
        ("hub:a_test_nobody_wrote", "not a test of hub"),
    ] {
        let invented = edited(&doc, |s| {
            s.replacen(&format!("`{first_proof}`"), &format!("`{name}`"), 1)
        });
        assert!(
            proof_problems(&invented, &known, &others)
                .iter()
                .any(|p| p.contains(wanted)),
            "{name}"
        );
    }

    // A test that a deviation names and nobody wrote, and a deviation that names no tests at all.
    assert!(deviation_proof_problems(&doc, &known, &others).is_empty());
    let invented = edited(&doc, |s| {
        s.replace("every_mcp_route_has_a_declared_guard", "a_guard_nobody_wrote")
    });
    assert!(
        deviation_proof_problems(&invented, &known, &others)
            .iter()
            .any(|p| p.contains("a_guard_nobody_wrote"))
    );
    let bare = edited(&doc, |s| {
        let risks = part(s, "### Residual risks").unwrap();
        let bullet = risks.lines().find(|l| l.starts_with("- **D-1")).unwrap();
        s.replace(bullet, "- **D-1: nothing is named.** None.")
    });
    assert!(
        deviation_proof_problems(&bare, &known, &others)
            .iter()
            .any(|p| p.contains("name only"))
    );

    // A residual risk that is dropped.
    let dropped = edited(&doc, |s| s.replace("D-1", "D-one"));
    assert!(carry_forward_problems(&dropped).iter().any(|p| p.contains("D-1")));

    // A number that disagrees with the code.
    let slower = edited(&doc, |s| s.replace("60 reads a minute", "600 reads a minute"));
    assert!(
        fact_problems(&slower, &facts)
            .iter()
            .any(|p| p.contains("read rate"))
    );

    // A tool that acts on the world and is not named.
    let unnamed = edited(&doc, |s| s.replace("`restart_service`", "`restart`"));
    assert!(
        tool_problems(&unnamed, &tools)
            .iter()
            .any(|p| p.contains("restart_service"))
    );

    // A marker that the code no longer supports, and a gap that the section no longer reports.
    let fixed: Gaps = code
        .iter()
        .map(|&(marker, still_true)| (marker, still_true && marker != "None (T6): the final sweep"))
        .collect();
    assert!(
        marker_problems(&doc, &fixed)
            .iter()
            .any(|p| p.contains("final sweep"))
    );
    let forgotten = edited(&doc, |s| s.replace("None (T6): the planted-key matrix", "Done"));
    assert!(
        marker_problems(&forgotten, &code)
            .iter()
            .any(|p| p.contains("planted-key matrix"))
    );

    // The component dropped from the list.
    let unlisted = doc.replace(&format!("{COMPONENT_LINE}\n"), "");
    assert!(!listing_problems(&unlisted).is_empty());
}
