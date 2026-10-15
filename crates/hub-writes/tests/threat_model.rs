//! The write paths' section of `docs/threat-model.md` (06 T8, S24) is complete, and it stays true to the code.
//!
//! A threat model that nobody checks drifts away from the code it describes. These tests read the
//! `## Write paths and token vault (06)` section and check what a machine can check:
//!
//! - the four parts are there (assets, trust boundaries, STRIDE, residual risks), and every STRIDE letter has a row;
//! - the section cites the requirements of the prompt (S4, S8, S9, S10, S11, S13, S21, S22), and every S# it cites exists in
//!   `docs/security.md`;
//! - every test, benchmark or fuzz target that a row names as its proof exists in `crates/hub-writes`, and a test name must belong
//!   to a function that really is a test, so a row cannot cite a helper or a test nobody wrote (a control that is planned but not
//!   built goes in the section's prose, and its Proof cell starts with `None`);
//! - the residual risks name the limits that the earlier tasks recorded (a landed write that is overwritten before the reconcile
//!   job looks, a restart whose answer is lost, the buffers that hold the GitHub token, the KMS call inside a transaction, a
//!   proposal that stops being pending, JSON arrays read as structs, the Teams hosts, the limits of the pull-request jobs (T9), and the
//!   parts not built yet);
//! - the numbers the section states (the lifetimes, the timeouts, the limits, the default hosts) are the ones in the code;
//! - each residual risk still describes what the code does: the places in the source that a bullet's mechanism rests on are
//!   still there, in the order the bullet says. A change to one of them has to change the model with it.
//!
//! Each check is a function over the document text (and, for the last two, over the sources), and `planted_gaps_are_found` shows
//! that each one refuses input with the gap it is meant to catch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const HEADING: &str = "## Write paths and token vault (06)";
/// The requirements prompt 06 owns (its metadata table). The section must speak to each one.
const REQUIREMENTS: [&str; 8] = ["S4", "S8", "S9", "S10", "S11", "S13", "S21", "S22"];
const STRIDE_LETTERS: [&str; 6] = ["S", "T", "R", "I", "D", "E"];
const PARTS: [&str; 4] = [
    "### Assets",
    "### Trust boundaries",
    "### STRIDE",
    "### Residual risks",
];
/// The fewest residual risks the section may list. A short list means some known limit was left out.
const MIN_RESIDUAL_RISKS: usize = 11;

/// The limits that earlier tasks recorded for the threat model. Each topic needs one residual-risk bullet that contains every
/// needle, so a topic cannot be "covered" by words scattered over unrelated bullets.
const CARRY_FORWARD: [(&str, &[&str]); 27] = [
    (
        "a landed write overwritten before the reconcile job looks is not audited as ours (T1)",
        &[
            "overwrites",
            "ReconcileWrites",
            "intent",
            "no audit event is recorded for our write",
        ],
    ),
    (
        "a restart whose answer is lost is not recorded, and a retry restarts again (T3)",
        &[
            "RestartDeployment",
            "Unavailable",
            "a retry",
            "restarts again",
            "no audit event",
        ],
    ),
    (
        "the GitHub token passes through buffers that cannot be zeroized (T5)",
        &[
            "PUT /api/v1/me/github-token",
            "Bytes",
            "hyper",
            "Authorization",
            "not wiped",
        ],
    ),
    (
        "a token save holds a user row lock and a connection across one KMS call (T5)",
        &["FOR UPDATE", "KMS", "pool connection", "db_timeout", "per-user"],
    ),
    (
        "a proposal that stops being pending between the write and its recording is still audited (T4)",
        &[
            "no longer `pending`",
            "audit event",
            "left as it is",
            "warning",
            "a_proposal_that_expires_while_its_write_is_on_the_way_is_still_audited_and_left_expired",
        ],
    ),
    (
        "serde reads a JSON array as a struct, so a reader must go through `object` (T3, T4, T5)",
        &[
            "array",
            "`object`",
            "routes/wire.rs",
            "a_json_array_is_not_an_object_for_any_reader",
        ],
    ),
    (
        "the Teams allowlist may need the tenant's host (Q9, T7)",
        &[
            "*.logic.azure.com",
            "*.api.powerplatform.com",
            "*.webhook.office.com",
            "Q9",
            "TeamsConfig::allowed_hosts",
        ],
    ),
    (
        "a rejected or conflicting write leaves no audit event",
        &["no audit event", "conflict", "RED metrics"],
    ),
    (
        "one GitHub account can be linked to several users (T5)",
        &["users.github_login", "unique", "reset_github_link"],
    ),
    (
        "notify and the config-server restart wait for the answers to Q37, Q35 and Q10 (T11)",
        &["Q37", "Q35", "Q10", "NetworkPolicy", "no authentication"],
    ),
    (
        "a notification whose answer is lost is not recorded, and a retry notifies again (T11)",
        &[
            "NotifyConfigServer",
            "Unavailable",
            "a retry",
            "notifies again",
            "no audit event",
        ],
    ),
    (
        "the config-server's 2xx is taken at its word, and the pickup states are the engine's reading of it (T11)",
        &[
            "2xx",
            "last_notified_at",
            "CONFIG_CLIENT_CACHE_TTL",
            "pickup_state",
            "its own small parser",
        ],
    ),
    (
        "auto-notify is best effort, and its queue is in memory (T11)",
        &[
            "auto_notify_queue",
            "best effort",
            "not retried",
            "auto_notify_dropped",
            "in memory",
        ],
    ),
    (
        "the config-server is found by its Deployment name (T11)",
        &[
            "config_server_deployment",
            "csp-configuration-server",
            "several",
            "no restart is offered",
        ],
    ),
    (
        "an approver sees only the first path of a notification, and no action (T11)",
        &["ProposalView", "first path", "approver", "no `action`"],
    ),
    (
        "a proposal deleted elsewhere leaves its blob reference behind (review 2)",
        &[
            "blob_refs",
            "ref_id",
            "no foreign key",
            "never deletes a proposal",
            "contract follow-up",
        ],
    ),
    (
        "the notify route's answer for an approval user is not in the contract (T11)",
        &["NotifyResult", "proposal_created", "oneOf", "contract change"],
    ),
    (
        "the reveal's limit is kept by each replica, in memory (T10)",
        &["RevealLimiter", "moka", "reveals_per_minute", "each replica"],
    ),
    (
        "a reveal needs no approval and no second person (T10)",
        &["requires_approval", "after the fact", "reason", "write it down"],
    ),
    (
        "a reveal is not replayable, so a lost response is audited twice (T10)",
        &["Idempotency-Key", "does not claim it", "second event", "secret"],
    ),
    (
        "what can be revealed is what the mask hides, and the mask is only as good as the scan (T10)",
        &["masked whole", "audited call", "does not recognise"],
    ),
    (
        "the revealed value passes through buffers that cannot be zeroized (T10)",
        &["serde_json::to_vec", "not wiped", "Secret", "`Bytes`"],
    ),
    (
        "a job uses its author's token in the background, after the request (T9)",
        &["pr_jobs", "background", "require_active", "D42", "S8"],
    ),
    (
        "a pull request that cannot be recorded is on GitHub with no link (T9)",
        &["pr_links_one_open", "no link", "no audit event of ours", "30 s"],
    ),
    (
        "raising a pull request leaves no audit event until it exists (T9)",
        &["Accepting a job writes none", "pr_jobs.failure"],
    ),
    (
        "a failed approval leaves the proposal approved, and the job's reads can lag NFS (T9)",
        &["stays `approved`", "may lag NFS"],
    ),
    (
        "`raise_pr` through `WriteService` cannot say why a job failed (T9)",
        &["WriteOutcome", "30 s", "NotAllowed", "createPr"],
    ),
];

/// Numbers the section states, with where the code holds them: (the words in the section, the file under the repository root, the
/// text the file must still contain).
const FACTS: [(&str, &str, &str); 30] = [
    (
        "60 s",
        "crates/hub-writes/src/config.rs",
        "stale_claim_after: Duration::from_secs(60)",
    ),
    (
        "24 hours",
        "crates/hub-writes/src/config.rs",
        "idempotency_ttl: Duration::from_secs(24 * 3600)",
    ),
    (
        "72 hours",
        "crates/hub-writes/src/config.rs",
        "proposal_ttl: Duration::from_secs(72 * 3600)",
    ),
    (
        "drafts after a day",
        "crates/hub-writes/src/config.rs",
        "draft_ttl: Duration::from_secs(24 * 3600)",
    ),
    (
        "`db_timeout` (10 s)",
        "crates/hub-writes/src/config.rs",
        "db_timeout: Duration::from_secs(10)",
    ),
    (
        "at most eight validations",
        "crates/hub-writes/src/config.rs",
        "max_concurrent_checks: 8",
    ),
    (
        "write-preview-key",
        "crates/hub-writes/src/config.rs",
        "pub const PREVIEW_KEY_SECRET: &str = \"write-preview-key\";",
    ),
    (
        "30 days",
        "crates/hub-writes/src/vault/mod.rs",
        "pub const UNSEEN_DAYS: i64 = 30;",
    ),
    (
        "ten minutes",
        "crates/domain/src/write.rs",
        "pub const TTL_SECS: u64 = 600;",
    ),
    (
        "12 MiB",
        "crates/hub-writes/src/routes/mod.rs",
        "pub const MAX_EDIT_BODY: usize = 12 * 1024 * 1024 + 64 * 1024;",
    ),
    (
        "*.logic.azure.com",
        "crates/hub-writes/src/teams/mod.rs",
        "\"*.logic.azure.com\".to_owned()",
    ),
    (
        "*.api.powerplatform.com",
        "crates/hub-writes/src/teams/mod.rs",
        "\"*.api.powerplatform.com\".to_owned()",
    ),
    (
        "*.webhook.office.com",
        "crates/hub-writes/src/teams/mod.rs",
        "\"*.webhook.office.com\".to_owned()",
    ),
    (
        "migration 0004",
        "db/migrations/0004_write_additions.sql",
        "REVOKE UPDATE, DELETE ON approvals FROM lanekeeper_app;",
    ),
    (
        "five tries",
        "crates/hub-writes/src/prs/runner.rs",
        "pub const MAX_ATTEMPTS: i32 = 5;",
    ),
    (
        "a step takes at most 60 s",
        "crates/hub-writes/src/prs/runner.rs",
        "const STEP_TIMEOUT: Duration = Duration::from_secs(60);",
    ),
    (
        "a job nobody has touched for 30 s",
        "crates/hub-writes/src/prs/runner.rs",
        "const STALE_AFTER: Duration = Duration::from_secs(30);",
    ),
    (
        "at most four jobs in the background",
        "crates/hub-writes/src/prs/runner.rs",
        "const MAX_RUNNING: usize = 4;",
    ),
    (
        "At most 30 reveals",
        "crates/hub-writes/src/config.rs",
        "reveals_per_minute: 30",
    ),
    (
        "table of 10,000 users",
        "crates/hub-writes/src/config.rs",
        "reveal_tracked_users: 10_000",
    ),
    (
        "(16 KiB)",
        "crates/hub-writes/src/routes/mod.rs",
        "pub const MAX_SMALL_BODY: usize = 16 * 1024;",
    ),
    (
        "for five seconds before it publishes",
        "crates/hub-writes/src/prs/links.rs",
        "const ANNOUNCE_LEASE: Duration = Duration::from_secs(5);",
    ),
    (
        "a window of two seconds",
        "crates/hub-writes/src/config.rs",
        "auto_notify_window: Duration::from_secs(2)",
    ),
    (
        "1,024 written paths",
        "crates/hub-writes/src/config.rs",
        "auto_notify_queue: 1_024",
    ),
    (
        "at most eight calls at once",
        "crates/hub-writes/src/autonotify.rs",
        "const FLUSH_PARALLELISM: usize = 8;",
    ),
    (
        "`csp-configuration-server` unless",
        "crates/hub-writes/src/config.rs",
        "config_server_deployment: \"csp-configuration-server\".to_owned()",
    ),
    (
        "up to 2,000",
        "crates/hub-writes/src/ops/restart.rs",
        "pub const MAX_RESTART_OFFER: usize = 2_000;",
    ),
    (
        "default of 20 minutes",
        "crates/engine/src/resolution/pickup.rs",
        "pub const DEFAULT_CACHE_TTL_SECS: u32 = 20 * 60;",
    ),
    (
        "about 605 KiB",
        "crates/hub-writes/src/routes/mod.rs",
        "pub const MAX_NOTIFY_BODY: usize = 100 * (6 * 1024 + 8) + 4096;",
    ),
    (
        "at most 100 paths",
        "crates/domain/src/write.rs",
        "pub const MAX_PATHS: usize = 100;",
    ),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn read_in_repo(relative: &str) -> String {
    read(&repo().join(relative))
}

fn threat_model() -> String {
    read_in_repo("docs/threat-model.md")
}

// ------------------------------------------------------------------------------------------------ reading the document

/// The text of the section: from its heading to the next `## ` heading.
fn write_section(doc: &str) -> Result<String, String> {
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

/// Is `id` a requirement id: `S`, digits, and the one lower-case `b` that `S14b` has?
fn is_requirement_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix('S') else {
        return false;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && matches!(&rest[digits..], "" | "b")
}

/// Every requirement id in `text` (so `S5` and `S14b`, but not `SAN`, `S5x` or the `S` inside a word).
fn requirement_ids(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut ids = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if bytes[i] == b'S' && starts_word {
            let digits = bytes[i + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
            let mut end = i + 1 + digits;
            if digits > 0 && bytes.get(end) == Some(&b'b') {
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

// ------------------------------------------------------------------------------------------------ the code's side

/// The names a proof cell may cite: the functions that are tests (`#[test]`, `#[tokio::test]`, also inside `proptest!`) in
/// `crates/hub-writes`, and the file stems of its benchmarks and of its fuzz targets. This file is left out, so that the sample
/// source in its own test does not count.
fn known_proofs() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "threat_model.rs")
            {
                out.extend(test_functions(&read(&path)));
            }
        }
    }
    let mut names = BTreeSet::new();
    let base = repo().join("crates/hub-writes");
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

/// Every S# that `docs/security.md` defines (`- **S5 Agent identity.**`, `- **S14b Redaction.**`).
fn defined_requirements() -> BTreeSet<String> {
    let security = read_in_repo("docs/security.md");
    security
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("- **"))
        .filter_map(|l| l.split([' ', '.', '*']).next())
        .filter(|id| is_requirement_id(id))
        .map(str::to_owned)
        .collect()
}

// ------------------------------------------------------------------------------------------------ the checks

/// Problems with the shape: the section exists and has its four parts, in order, each with content.
fn shape_problems(doc: &str) -> Vec<String> {
    let section = match write_section(doc) {
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
    if let Some(risks) = part(&section, "### Residual risks") {
        let items = risks.lines().filter(|l| l.starts_with("- **")).count();
        if items < MIN_RESIDUAL_RISKS {
            problems.push(format!(
                "only {items} residual risks are listed; each known limit of the write paths belongs here"
            ));
        }
    }
    problems
}

/// Problems with the STRIDE table: every letter has a row, and every row names a mitigation, a proof and a requirement.
fn stride_problems(doc: &str) -> Vec<String> {
    let Ok(section) = write_section(doc) else {
        return vec!["no write paths section".to_owned()];
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

/// Problems with the requirements: each of the prompt's is cited, and each cited S# is real.
fn requirement_problems(doc: &str, defined: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = write_section(doc) else {
        return vec!["no write paths section".to_owned()];
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

/// Problems with the proofs: every name in a Proof cell is a test, a benchmark or a fuzz target of this crate.
fn proof_problems(doc: &str, known: &BTreeSet<String>) -> Vec<String> {
    let Ok(section) = write_section(doc) else {
        return vec!["no write paths section".to_owned()];
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
                    "the proof `{name}` is not a test, bench or fuzz target of hub-writes"
                ));
            }
        }
    }
    // A residual risk may name the test that holds its behaviour. A long snake_case word in backticks is read as such a name.
    if let Some(risks) = part(&section, "### Residual risks") {
        for name in code_spans(risks) {
            let looks_like_a_test = name.len() >= 24
                && name.contains('_')
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
            if looks_like_a_test && !known.contains(name) {
                problems.push(format!(
                    "the residual risks name the test `{name}`, which is not a test, bench or fuzz target of hub-writes"
                ));
            }
        }
    }
    problems
}

/// Problems with the residual risks: each carried-forward limit has a bullet that holds all of its words.
fn carry_forward_problems(doc: &str) -> Vec<String> {
    let Ok(section) = write_section(doc) else {
        return vec!["no write paths section".to_owned()];
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

/// Problems with the numbers: each fact the section states is still in the code, and the section still states it.
fn fact_problems(doc: &str, source: &dyn Fn(&str) -> String) -> Vec<String> {
    let Ok(section) = write_section(doc) else {
        return vec!["no write paths section".to_owned()];
    };
    let mut problems = Vec::new();
    for (words, file, needle) in FACTS {
        if !section.contains(words) {
            problems.push(format!("the section no longer says `{words}`"));
        }
        if !source(file).contains(needle) {
            problems.push(format!(
                "`{file}` no longer has `{needle}`; the section says `{words}`, so change the section with the code"
            ));
        }
    }
    problems
}

/// The text of `source` from `start` up to (not including) the next `end`, or to its end.
fn between<'a>(source: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = source.find(start)?;
    let rest = &source[from..];
    let to = rest[start.len()..]
        .find(end)
        .map_or(rest.len(), |n| start.len() + n);
    Some(&rest[..to])
}

/// Are the needles in `text` in this order?
fn in_order(text: &str, needles: &[&str]) -> bool {
    let mut at = 0;
    for needle in needles {
        match text[at..].find(needle) {
            Some(found) => at += found + needle.len(),
            None => return false,
        }
    }
    true
}

/// Problems with the anchors: the places in the source that the residual risks rest on, in the order each bullet describes.
fn anchor_problems(source: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut problems = Vec::new();
    let mut check = |ok: bool, risk: &str, what: &str| {
        if !ok {
            problems.push(format!(
                "residual risk `{risk}` no longer matches the code: {what}. Change the section and the code together."
            ));
        }
    };

    // 1. The reconcile job clears a claim when another version of the file is there, and the intent holds no bytes.
    let reconcile = source("crates/hub-writes/src/reconcile.rs");
    check(
        in_order(
            &reconcile,
            &[
                "hash == intent.new_hash",
                "// Another version is there.",
                "Ok(Ok(AgentReply::File { .. })) => clear(hub, key).await",
            ],
        ),
        "a landed write that someone else overwrites",
        "`resolve` in reconcile.rs records a write only when the file has the hash that was sent, and clears the claim for any other version",
    );
    let idempotency = source("crates/hub-writes/src/idempotency.rs");
    let intent = between(&idempotency, "pub(crate) struct Intent {", "\n}").unwrap_or_default();
    check(
        !intent.is_empty() && !intent.contains("Bytes") && !intent.contains("bytes:"),
        "a landed write that someone else overwrites",
        "the `Intent` struct holds a hash and not the bytes that were sent",
    );

    // 2. A restart whose answer is lost releases the claim and records nothing.
    let restart = source("crates/hub-writes/src/ops/restart.rs");
    let ambiguous = between(&restart, "Sent::Ambiguous", "Sent::Landed").unwrap_or_default();
    check(
        ambiguous.contains("Abort::release(WriteError::Unavailable)")
            && !ambiguous.contains("record(")
            && !ambiguous.contains("mark_ambiguous"),
        "a restart whose answer is lost",
        "`send_and_record` in ops/restart.rs releases the claim and answers `Unavailable` for an ambiguous restart, with no audit event",
    );

    // 3. The token route reads the body as `Bytes` and keeps it for the whole call.
    let routes = source("crates/hub-writes/src/routes/mod.rs");
    let token_route = between(
        &routes,
        "Method::PUT, GITHUB_TOKEN_PATH",
        "Method::DELETE, GITHUB_TOKEN_PATH",
    )
    .unwrap_or_default();
    check(
        in_order(
            token_route,
            &[
                "body: Result<Bytes, BytesRejection>",
                "me::put(&s, user.0.0, &headers, &body).await",
            ],
        ),
        "the GitHub token passes through buffers that cannot be zeroized",
        "the PUT route for the token reads the body as `Bytes` and holds it while the vault works",
    );

    // 4. A token save locks the user's row, calls KMS and commits, all in one transaction.
    let vault = source("crates/hub-writes/src/vault/mod.rs");
    let store_fn = between(&vault, "    async fn store(", "\n    }\n").unwrap_or_default();
    check(
        in_order(
            store_fn,
            &[
                "begin()",
                "store::lock_user(",
                "crypto::seal(",
                "audit",
                "tx.commit()",
            ],
        ),
        "a token save holds a user row lock and a connection across one KMS call",
        "`store` in vault/mod.rs begins a transaction, locks the user's row, seals the token through KMS, records the audit event and commits",
    );
    check(
        vault.contains("hub.within(\"vault_put\", self.store("),
        "a token save holds a user row lock and a connection across one KMS call",
        "the whole transaction is inside `within` (`db_timeout`)",
    );
    let vault_store = source("crates/hub-writes/src/vault/store.rs");
    check(
        vault_store.contains("FROM users WHERE tid = $1 AND oid = $2 FOR UPDATE"),
        "a token save holds a user row lock and a connection across one KMS call",
        "`LOCK_USER` in vault/store.rs is a `SELECT ... FOR UPDATE` on the user's row",
    );

    // 5. A proposal that is no longer pending is left as it is, with a warning, after the write was audited.
    let follow = source("crates/hub-writes/src/pipeline/follow.rs");
    check(
        in_order(
            &follow,
            &[
                "store::lock_status(",
                "tracing::warn!(",
                "no longer pending",
                "return Ok(());",
                "store::insert_decision(",
            ],
        ),
        "a proposal that stops being pending",
        "`follow::apply` returns with a warning, before it writes the decision, when the proposal is not pending",
    );

    // 6. The JSON readers go through `object`, which refuses what is not an object.
    let wire = source("crates/hub-writes/src/routes/wire.rs");
    check(
        wire.matches("serde_json::from_slice").count() == 1 && wire.contains("fn object<T"),
        "serde reads a JSON array as a struct",
        "routes/wire.rs has exactly one `serde_json::from_slice`, inside `object`, which every JSON reader calls",
    );
    check(
        wire.matches("= object(body)?;").count() >= 7,
        "serde reads a JSON array as a struct",
        "the seven JSON readers (edit, revert, restart, draft, approve, reject and the token) read through `object`",
    );

    // 7. A destination off the allowlist is dropped with a warning and no error.
    let teams = source("crates/hub-writes/src/teams/mod.rs");
    check(
        in_order(
            &teams,
            &[
                "if !self.webhook.accepts(url.expose())",
                "the Teams destination is not an address that may be called",
                "return Ok(Route::Off);",
            ],
        ),
        "the Teams allowlist may need the tenant's host",
        "a destination off the allowlist is logged and dropped without an error",
    );

    // 8. A reveal checks the door and the role, counts the attempt, reads, finds, records and commits, and only then answers; it
    //    claims no idempotency key and knows nothing of approval.
    let reveal = source("crates/hub-writes/src/reveal.rs");
    let desk = between(&reveal, "async fn reveal(hub: &Inner", "\n}\n").unwrap_or_default();
    check(
        in_order(
            desk,
            &[
                "ctx.via != Via::Ui",
                "authorize(hub, ctx, Role::Editor)",
                ".take(&user.id",
                "\"reveal_read\"",
                "\"reveal_find\"",
                "\"reveal_audit\"",
                "tx.commit()",
                "Ok(Revealed { value: found.value })",
            ],
        ),
        "a reveal is not replayable, so a response that is lost is audited twice",
        "`reveal` in reveal.rs checks the door and the role, counts the attempt, reads, finds, records and commits the event, and only then returns the value",
    );
    check(
        !reveal.contains("claim(") && !reveal.contains("requires_approval"),
        "a reveal is not replayable, so a response that is lost is audited twice",
        "reveal.rs claims no idempotency key and knows nothing of approval",
    );

    // 9. The limit is a bounded in-memory table of windows, one for each replica.
    check(
        reveal.contains("windows: moka::sync::Cache<UserId, Arc<Mutex<VecDeque<i64>>>>")
            && source("crates/hub-writes/src/service.rs").contains("pub reveals: RevealLimiter,"),
        "the reveal's limit is kept by each replica",
        "`RevealLimiter` is a `moka` table of per-user windows held by the hub in memory",
    );

    // 10. What can be revealed is decided by the mask's own function.
    check(
        reveal.contains("view::marker(side.flags.as_ref(), found.lines)?")
            && source("crates/hub-writes/src/proposals/view.rs")
                .contains("None => Some(UNSCANNED.to_owned())"),
        "what can be revealed is what the mask hides",
        "`find` asks `view::marker`, which also masks a file the scan could not read whole",
    );

    // 11. The value is a `Secret` until the body is written, and the body is serialised from a plain buffer.
    check(
        reveal.contains("Secret::new(found.value.text.clone())")
            && source("crates/hub-writes/src/routes/reveal.rs").contains("serde_json::to_vec(&Shown {"),
        "the revealed value passes through buffers",
        "the value is a `Secret` from `find` and is serialised with `serde_json::to_vec`",
    );

    // 12. A notification whose answer is lost releases the claim and records nothing; the config-server's `2xx` is what makes it
    //     a notification.
    let notify = source("crates/hub-writes/src/ops/notify.rs");
    let lost = between(&notify, "Sent::Ambiguous", "Sent::Landed").unwrap_or_default();
    check(
        lost.contains("Abort::release(WriteError::Unavailable)")
            && !lost.contains("record(")
            && !lost.contains("mark_ambiguous"),
        "a notification whose answer is lost",
        "`execute` in ops/notify.rs releases the claim and answers `Unavailable` for an ambiguous notification, with no audit event",
    );
    check(
        source("crates/hub-writes/src/pipeline/execute.rs").contains("if (200..300).contains(&status) {"),
        "the config-server's 2xx is taken at its word",
        "`send_notify` in execute.rs counts a notification as made exactly when the config-server's status is a `2xx`",
    );

    // 13. Auto-notify: the switch is read after the write has committed and before anything is queued; the batcher reads the role
    //     when the window closes and never announces for a user who needs approval; a full queue drops the path.
    let record_fn = between(
        &source("crates/hub-writes/src/pipeline/execute.rs"),
        "pub(crate) async fn record(hub: &Inner",
        "\n}\n",
    )
    .unwrap_or_default()
    .to_owned();
    check(
        in_order(
            &record_fn,
            &[
                "AutoItem::of(&rec.event)",
                "record_inner(hub, rec)",
                "Committed::Done(_)",
                "hub.auto.offer(hub, item).await",
            ],
        ),
        "auto-notify is best effort",
        "`record` in execute.rs offers a path to the auto-notify queue only after the transaction that recorded the write has committed",
    );
    let auto = source("crates/hub-writes/src/autonotify.rs");
    let announce = between(&auto, "async fn announce(", "\n}\n").unwrap_or_default();
    check(
        in_order(
            announce,
            &[
                "authorize_user(hub, &user, Role::Operator)",
                "user.requires_approval",
                "notify_as(",
                "AuditVia::System",
            ],
        ),
        "auto-notify is best effort",
        "`announce` in autonotify.rs reads the role when the window closes, skips a user who needs approval, and notifies through `notify_as` with `via = system`",
    );
    let offer = between(&auto, "pub async fn offer(", "\n    }\n").unwrap_or_default();
    check(
        in_order(
            offer,
            &[
                "self.enabled.load(",
                "\"auto_notify_switch\"",
                "self.sender.try_send(item)",
                "self.dropped.fetch_add(",
            ],
        ),
        "auto-notify is best effort",
        "`offer` in autonotify.rs does nothing until a batcher exists, reads the swimlane's switch, and drops and counts a path that does not fit the bounded queue",
    );

    // 14. A notification leaves alone a service that waits for the config-server to restart, and reads a lifetime it cannot parse as
    //     the default.
    let pickup = source("crates/hub-writes/src/ops/pickup.rs");
    let apply = between(&pickup, "pub(crate) async fn apply(", "\n}\n").unwrap_or_default();
    let page = between(&pickup, "async fn update_page(", "\n}\n").unwrap_or_default();
    check(
        in_order(
            page,
            &[
                "PickupState::NeedsConfigServerRestart.as_str()",
                "continue;",
                "recompute(",
            ],
        ) && in_order(apply, &["update_page(", "bus"]),
        "the config-server's 2xx is taken at its word",
        "`update_page` in ops/pickup.rs skips a service that waits for the config-server's restart before it asks the engine, and `apply` publishes the change through the outbox",
    );
    check(
        pickup.contains("_ => DEFAULT_CACHE_TTL_SECS,"),
        "the config-server's 2xx is taken at its word",
        "`cache_ttl_secs` in ops/pickup.rs reads a lifetime it cannot parse, or zero, as the default",
    );

    // 15. The config-server restart is offered only for exactly one Deployment of the configured name, while C11 is open.
    let restart_offer =
        between(&restart, "pub(crate) async fn config_server_offer(", "\n}\n").unwrap_or_default();
    check(
        in_order(
            restart_offer,
            &[
                "authorize(hub, ctx, Role::Operator)",
                "C11_OPEN",
                "CONFIG_SERVER_OF",
                "let [(namespace, name)] = found.as_slice() else {",
            ],
        ),
        "the config-server is found by its Deployment name",
        "`config_server_offer` in ops/restart.rs authorizes an Operator, checks that C11 is open, and offers exactly one Deployment of the configured name",
    );

    // 16. This crate never deletes a proposal, so no `blob_refs` row of a proposal is left behind by it.
    check(
        [
            "crates/hub-writes/src/proposals/store.rs",
            "crates/hub-writes/src/proposals/expire.rs",
            "crates/hub-writes/src/proposals/approve.rs",
        ]
        .iter()
        .all(|f| !source(f).contains("DELETE FROM proposals")),
        "a proposal that is deleted elsewhere",
        "the proposal code only moves `status` and never issues `DELETE FROM proposals`, which is why only a delete by another crate can leave its `blob_refs` row behind",
    );

    problems
}

fn real_source(relative: &str) -> String {
    read_in_repo(relative)
}

// ------------------------------------------------------------------------------------------------ the tests

#[test]
fn section_has_assets_boundaries_stride_and_residual_risks() {
    let problems = shape_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn stride_table_has_all_six_letters_and_every_row_is_complete() {
    let problems = stride_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn section_cites_s4_s8_s9_s10_s11_s13_s21_s22_and_only_real_requirements() {
    let defined = defined_requirements();
    assert!(
        REQUIREMENTS.iter().all(|id| defined.contains(*id)),
        "docs/security.md no longer defines one of {REQUIREMENTS:?}: {defined:?}"
    );
    assert!(
        defined.contains("S14b"),
        "the parser of docs/security.md no longer finds S14b: {defined:?}"
    );
    let problems = requirement_problems(&threat_model(), &defined);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_proof_cell_names_an_existing_test() {
    let known = known_proofs();
    assert!(
        known.len() > 300,
        "the scan of the crate's sources found only {} tests, benches and fuzz targets",
        known.len()
    );
    let problems = proof_problems(&threat_model(), &known);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// `QueryProbe` claims, finishes and purges idempotency rows for any key on the hub's own pool, for the benchmark of P12. Anything
/// that held a `WriteHub` with it could finish a live write's claim with a stored conflict, so it is not part of the library that
/// the hub links: only this crate's own tests and benchmarks switch the feature on.
#[test]
fn the_query_probe_is_compiled_only_for_the_crates_own_tests_and_benches() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = read(&here.join("Cargo.toml"));
    let lib = read(&here.join("src/lib.rs"));
    let (own, dev) = manifest
        .split_once("[dev-dependencies]")
        .expect("Cargo.toml has a [dev-dependencies] section");
    assert!(own.contains("\nbench-probe = []"), "the feature is declared");
    assert!(own.contains("\ndefault = []"), "and is not a default");
    assert_eq!(
        own.matches("bench-probe").count(),
        1,
        "and is named nowhere else in the sections that build the library"
    );
    assert!(
        dev.contains("features = [\"fuzz-hooks\", \"test-doubles\", \"bench-probe\"]"),
        "the crate's own tests and benches switch it on"
    );
    assert!(
        lib.contains("#[cfg(feature = \"bench-probe\")]\nmod probe;"),
        "the module is behind it"
    );
    assert!(
        lib.contains("#[cfg(feature = \"bench-probe\")]\npub use probe::QueryProbe;"),
        "and so is the export"
    );
    // No other crate of the workspace turns it on (the hub, the fuzz crate).
    let crates = repo().join("crates");
    let mut users = Vec::new();
    for entry in fs::read_dir(&crates).expect("crates/ is there").flatten() {
        for manifest in [
            entry.path().join("Cargo.toml"),
            entry.path().join("fuzz/Cargo.toml"),
        ] {
            if manifest.exists() && read(&manifest).contains("bench-probe") {
                users.push(manifest);
            }
        }
    }
    assert_eq!(
        users,
        vec![crates.join("hub-writes/Cargo.toml")],
        "who switches it on"
    );
}

#[test]
fn residual_risks_cover_every_limit_the_earlier_tasks_recorded() {
    let problems = carry_forward_problems(&threat_model());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn numbers_in_the_section_are_the_ones_in_the_code() {
    let problems = fact_problems(&threat_model(), &real_source);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn residual_risks_still_describe_what_the_code_does() {
    let problems = anchor_problems(&real_source);
    assert!(problems.is_empty(), "{problems:#?}");
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
fn requirement_ids_read_the_b_of_s14b_and_nothing_else() {
    let found = requirement_ids("S4, S14b and S9. Not SAN, S5x, S14bb or XS7.");
    assert_eq!(
        found.iter().map(String::as_str).collect::<Vec<_>>(),
        ["S14b", "S4", "S9"]
    );
    assert!(is_requirement_id("S14b") && is_requirement_id("S9"));
    assert!(!is_requirement_id("S") && !is_requirement_id("Sb") && !is_requirement_id("S1c"));
}

#[test]
fn planted_gaps_are_found() {
    let doc = threat_model();
    let defined = defined_requirements();
    let known = known_proofs();
    // The real document and sources pass every check, so each failure below is caused by the plant.
    assert!(shape_problems(&doc).is_empty());
    assert!(stride_problems(&doc).is_empty());
    assert!(requirement_problems(&doc, &defined).is_empty());
    assert!(proof_problems(&doc, &known).is_empty());
    assert!(carry_forward_problems(&doc).is_empty());
    assert!(fact_problems(&doc, &real_source).is_empty());
    assert!(anchor_problems(&real_source).is_empty());

    // No section at all.
    let without = doc.replace(HEADING, "## Something else");
    assert!(!shape_problems(&without).is_empty());

    // A part missing.
    let section = write_section(&doc).unwrap();
    let no_residual = doc.replace("### Residual risks", "### Leftovers");
    assert!(
        shape_problems(&no_residual)
            .iter()
            .any(|p| p.contains("Residual risks"))
    );

    // A STRIDE letter missing: rename every `| E |` row of the table.
    let stride = part(&section, "### STRIDE").unwrap();
    let no_e = stride.replace("\n| E |", "\n| X |");
    assert_ne!(no_e, stride, "the plant found no `| E |` row");
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
    let no_s13 = doc.replace("S13", "S-13");
    assert!(
        requirement_problems(&no_s13, &defined)
            .iter()
            .any(|p| p.contains("S13"))
    );

    // A proof that was never written, and one that is a helper and not a test.
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
    let helper = doc.replacen(&format!("`{first_proof}`"), "`rig_with_a`", 1);
    assert!(
        proof_problems(&helper, &known)
            .iter()
            .any(|p| p.contains("rig_with_a")),
        "a function that is not a test must not count as a proof"
    );

    // A residual risk that names a test nobody wrote.
    let ghost = doc.replace(
        "a_proposal_that_expires_while_its_write_is_on_the_way_is_still_audited_and_left_expired",
        "a_test_for_the_expiry_that_nobody_wrote_at_all",
    );
    assert!(
        proof_problems(&ghost, &known)
            .iter()
            .any(|p| p.contains("a_test_for_the_expiry_that_nobody_wrote_at_all")),
        "a test named in a residual risk must exist"
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
    let no_proof = stride.replacen(whole_cell.as_str(), "it works", 1);
    assert_ne!(no_proof, stride, "the plant found no proof cell");
    assert!(
        stride_problems(&doc.replace(stride, &no_proof))
            .iter()
            .any(|p| p.contains("names no proof"))
    );

    // A row with a stray pipe is reported instead of silently misread.
    let stray = stride.replacen("\n| S |", "\n| S | extra |", 1);
    assert_ne!(stray, stride, "the plant found no `| S |` row");
    assert!(!stride_problems(&doc.replace(stride, &stray)).is_empty());

    // A carried-forward limit that is dropped, one whose words are spread over several bullets, and one for each of the seven
    // that the lead of the task named.
    let no_hosts = doc.replace("*.webhook.office.com", "the third host");
    assert!(
        carry_forward_problems(&no_hosts)
            .iter()
            .any(|p| p.contains("Teams allowlist"))
    );
    let no_array = doc.replace("a_json_array_is_not_an_object_for_any_reader", "the test");
    assert!(
        carry_forward_problems(&no_array)
            .iter()
            .any(|p| p.contains("JSON array"))
    );
    for (topic, needles) in CARRY_FORWARD {
        let without_needle = doc.replacen(needles[0], "REDACTED-BY-THE-PLANT", usize::MAX);
        assert!(
            carry_forward_problems(&without_needle)
                .iter()
                .any(|p| p.contains(topic)),
            "dropping `{}` must be found for {topic}",
            needles[0]
        );
    }

    // A number that changed in the code, and one that changed in the section.
    let changed_code = |relative: &str| {
        let text = real_source(relative);
        if relative.ends_with("reconcile.rs") {
            return text;
        }
        text.replace("Duration::from_secs(60)", "Duration::from_secs(61)")
    };
    assert!(
        fact_problems(&doc, &changed_code)
            .iter()
            .any(|p| p.contains("stale_claim_after"))
    );
    let changed_doc = doc.replace("72 hours", "73 hours");
    assert!(
        fact_problems(&changed_doc, &real_source)
            .iter()
            .any(|p| p.contains("72 hours"))
    );

    // A mechanism that the code stopped having: each anchor is refused when its source changes.
    let plant = |relative: &'static str, from: &'static str, to: &'static str| {
        move |asked: &str| {
            let text = real_source(asked);
            if asked == relative {
                assert!(text.contains(from), "the plant found no `{from}` in {relative}");
                text.replace(from, to)
            } else {
                text
            }
        }
    };
    for (relative, from, to, risk) in [
        (
            "crates/hub-writes/src/reconcile.rs",
            "Ok(Ok(AgentReply::File { .. })) => clear(hub, key).await",
            "Ok(Ok(AgentReply::File { .. })) => Resolved::Raced",
            "overwrites",
        ),
        (
            "crates/hub-writes/src/idempotency.rs",
            "pub new_hash: ContentHash,\n    pub user: UserId,",
            "pub new_hash: ContentHash,\n    pub bytes: bytes::Bytes,\n    pub user: UserId,",
            "overwrites",
        ),
        (
            "crates/hub-writes/src/ops/restart.rs",
            "Err(Abort::release(WriteError::Unavailable))\n        }\n        Sent::Landed(_)",
            "Err(Abort::keep(WriteError::Unavailable))\n        }\n        Sent::Landed(_)",
            "restart",
        ),
        (
            "crates/hub-writes/src/vault/mod.rs",
            "crypto::seal(&*self.kms, token, &aad)",
            "crypto::seal_after_commit(&*self.kms, token, &aad)",
            "KMS call",
        ),
        (
            "crates/hub-writes/src/pipeline/follow.rs",
            "\"an approved proposal was no longer pending when its write was recorded\"",
            "\"gone\"",
            "stops being pending",
        ),
        (
            "crates/hub-writes/src/routes/wire.rs",
            "let body: RejectBody = object(body)?;",
            "let body: RejectBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;",
            "JSON array",
        ),
        (
            "crates/hub-writes/src/reveal.rs",
            "tx.commit().await.map_err(|e| Internal::db(\"reveal_audit\", &e))?;",
            "tx.rollback().await.ok();",
            "audited twice",
        ),
        (
            "crates/hub-writes/src/reveal.rs",
            "let mut tx = hub\n                .pool",
            "let mut claim_a_key = hub.claim(\n                .pool",
            "audited twice",
        ),
        (
            "crates/hub-writes/src/reveal.rs",
            "windows: moka::sync::Cache<UserId, Arc<Mutex<VecDeque<i64>>>>",
            "windows: SharedTable",
            "kept by each replica",
        ),
        (
            "crates/hub-writes/src/reveal.rs",
            "view::marker(side.flags.as_ref(), found.lines)?",
            "Some(view::marker(side.flags.as_ref(), found.lines))?",
            "what can be revealed",
        ),
        (
            "crates/hub-writes/src/reveal.rs",
            "Secret::new(found.value.text.clone())",
            "found.value.text.clone()",
            "the revealed value passes through",
        ),
        (
            "crates/hub-writes/src/teams/mod.rs",
            "return Ok(Route::Off);\n        }\n        Ok(Route::On",
            "return Err(NotifyError::Rejected);\n        }\n        Ok(Route::On",
            "Teams allowlist",
        ),
        (
            "crates/hub-writes/src/ops/notify.rs",
            "Err(Abort::release(WriteError::Unavailable))\n        }\n        Sent::Landed(_)",
            "Err(Abort::keep(WriteError::Unavailable))\n        }\n        Sent::Landed(_)",
            "answer is lost",
        ),
        (
            "crates/hub-writes/src/pipeline/execute.rs",
            "if (200..300).contains(&status) {",
            "if status != 0 {",
            "2xx is taken at its word",
        ),
        (
            "crates/hub-writes/src/pipeline/execute.rs",
            "hub.auto.offer(hub, item).await",
            "hub.auto.offer_before_commit(hub, item).await",
            "auto-notify is best effort",
        ),
        (
            "crates/hub-writes/src/autonotify.rs",
            "if user.requires_approval {",
            "if false {",
            "auto-notify is best effort",
        ),
        (
            "crates/hub-writes/src/autonotify.rs",
            "self.sender.try_send(item)",
            "self.sender.send(item)",
            "auto-notify is best effort",
        ),
        (
            "crates/hub-writes/src/ops/pickup.rs",
            "if before == PickupState::NeedsConfigServerRestart.as_str() {",
            "if false {",
            "2xx is taken at its word",
        ),
        (
            "crates/hub-writes/src/ops/pickup.rs",
            "_ => DEFAULT_CACHE_TTL_SECS,",
            "_ => 1,",
            "2xx is taken at its word",
        ),
        (
            "crates/hub-writes/src/ops/restart.rs",
            "let [(namespace, name)] = found.as_slice() else {",
            "let Some((namespace, name)) = found.first() else {",
            "Deployment name",
        ),
        (
            "crates/hub-writes/src/proposals/store.rs",
            "pub(crate) const INSERT_SQL: &str = INSERT;",
            "pub(crate) const INSERT_SQL: &str = INSERT;\nconst GONE: &str = \"DELETE FROM proposals WHERE id = $1\";",
            "deleted elsewhere",
        ),
    ] {
        let planted = plant(relative, from, to);
        assert!(
            anchor_problems(&planted).iter().any(|p| p.contains(risk)),
            "changing `{from}` in {relative} must be found"
        );
    }
}
