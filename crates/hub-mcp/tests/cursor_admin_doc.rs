//! `docs/cursor-admin.md` against the contract and the code (plan 07, T4; D50, D52, D91, A9).
//!
//! The guide is for the administrator who puts the MCP server in front of Cursor. Three things in it must not be written by hand and
//! left to drift, so a test holds each one:
//!
//! - **the tool allowlist and the tool counts** are generated from the contract (`src/schemas/tools.json`, itself held equal to
//!   `docs/mcp-tools.md` by `contract_schemas`) into marked blocks, and compared with the contract's own words;
//! - **names and numbers the code owns** (the switches, the paths, the seven days, the five tokens, the 120 seconds) are the code's
//!   constants, not copies;
//! - **what is not known** stays marked: T0 of D91 is NOT RUN, and every Cursor format or menu detail that could not be checked from
//!   the sandbox is marked UNVERIFIED, with a line that tells the administrator to check it on the first install (A9).
//!
//! `UPDATE_ADMIN_DOC=1 cargo test -p hub-mcp --test cursor_admin_doc generated_blocks_equal_the_contract` (or `just mcp-admin-doc`)
//! rewrites the two generated blocks from the contract.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use hub_mcp::auth::oauth::{
    AUTHORIZATION_SERVER_PATH, RESOURCE_METADATA_PATH, RESOURCE_METADATA_ROOT_PATH, SCOPE_NAME,
};
use hub_mcp::auth::pat::{DEFAULT_CACHE_TTL, DEFAULT_LIFETIME, DEFAULT_MAX_LIVE};
use hub_mcp::auth::routes::{COLLECTION_PATH, ITEM_PATH};
use hub_mcp::config::{AuthMode, ConfigError, ConfirmMode, ContentMode, McpConfig};
use hub_mcp::limits::LimitSettings;
use serde_json::Value;

const READ_CAP_BYTES: u64 = 16 * 1024;
const WRITE_CAP_BYTES: u64 = 4 * 1024;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_repo(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn doc() -> String {
    read_repo("docs/cursor-admin.md")
}

// ----------------------------------------------------------------------------------------------------------------------------
// The contract
// ----------------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Tool {
    name: String,
    role: String,
    confirmation: String,
    cap: u64,
}

/// The tools of `src/schemas/tools.json`, in the contract's order.
fn contract() -> Vec<Tool> {
    let raw =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/schemas/tools.json")).unwrap();
    let file: Value = serde_json::from_str(&raw).unwrap();
    let tools = file["tools"].as_array().expect("tools is an array");
    let tools: Vec<Tool> = tools
        .iter()
        .map(|t| Tool {
            name: t["name"].as_str().unwrap().to_owned(),
            role: t["role"].as_str().unwrap().to_owned(),
            confirmation: t["confirmation"].as_str().unwrap().to_owned(),
            cap: t["output_cap_bytes"].as_u64().unwrap(),
        })
        .collect();
    assert!(!tools.is_empty());
    tools
}

/// Read tools and write tools, by the contract's own split: a read tool is Viewer, asks nothing and has the 16 KB cap (P9, D58); a
/// write tool needs Editor or more and has the 4 KB cap. A tool that is neither is a decision for a human, never a silent entry on
/// a list that runs without asking, so it fails the test.
fn split(tools: &[Tool]) -> (Vec<Tool>, Vec<Tool>) {
    let mut read = Vec::new();
    let mut write = Vec::new();
    for tool in tools {
        let is_read = tool.role == "viewer" && tool.confirmation == "none" && tool.cap == READ_CAP_BYTES;
        let is_write = tool.role != "viewer" && tool.cap == WRITE_CAP_BYTES;
        match (is_read, is_write) {
            (true, false) => read.push(tool.clone()),
            (false, true) => write.push(tool.clone()),
            _ => panic!(
                "tool {} is neither a read tool (viewer, no confirmation, 16 KB) nor a write tool (editor or more, 4 KB): \
                 a human decides whether it may be allowlisted",
                tool.name
            ),
        }
    }
    (read, write)
}

/// What `docs/mcp-tools.md` says, read from the document and not from `tools.json`: the tool sections' roles and confirmations, and
/// the number its allowlist sentence gives.
struct Document {
    tools: Vec<(String, String, String)>,
    allowlist_count: usize,
}

fn contract_document() -> Document {
    let text = read_repo("docs/mcp-tools.md");
    let mut tools: Vec<(String, String, String)> = Vec::new();
    let mut in_tools = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_tools = line == "## Tools";
        }
        if !in_tools {
            continue;
        }
        if let Some(rest) = line.strip_prefix("### `")
            && let Some(name) = rest.strip_suffix('`')
        {
            tools.push((name.to_owned(), String::new(), String::new()));
        } else if let Some(role) = line.strip_prefix("- role: ")
            && let Some(last) = tools.last_mut()
        {
            role.trim().clone_into(&mut last.1);
        } else if let Some(confirmation) = line.strip_prefix("- confirmation: ")
            && let Some(last) = tools.last_mut()
        {
            confirmation.trim().clone_into(&mut last.2);
        }
    }
    let sentence = text
        .lines()
        .find(|l| l.starts_with("**Tool allowlist (D50)."))
        .expect("the contract has its Tool allowlist paragraph");
    let allowlist_count =
        number_before(sentence, "read tools").expect("the paragraph gives the number of read tools");
    assert!(
        sentence.contains("none of the write tools") && sentence.contains("`notify_services` is never on it"),
        "the contract's allowlist paragraph changed: {sentence}"
    );
    Document {
        tools,
        allowlist_count,
    }
}

/// The first number written just before `phrase` ("lists the 20 read tools" gives 20).
fn number_before(text: &str, phrase: &str) -> Option<usize> {
    text.match_indices(phrase)
        .find_map(|(at, _)| text[..at].split_whitespace().next_back()?.parse().ok())
}

// ----------------------------------------------------------------------------------------------------------------------------
// The guide and its generated blocks
// ----------------------------------------------------------------------------------------------------------------------------

const ALLOWLIST_BLOCK: &str = "allowlist";
const WRITE_BLOCK: &str = "write-tools";

fn begin(name: &str) -> String {
    format!("<!-- BEGIN GENERATED {name}")
}

fn end(name: &str) -> String {
    format!("<!-- END GENERATED {name} -->")
}

/// The lines between the markers of a generated block.
fn block(doc: &str, name: &str) -> Vec<String> {
    let lines: Vec<&str> = doc.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.starts_with(&begin(name)))
        .unwrap_or_else(|| panic!("no `{}` marker in docs/cursor-admin.md", begin(name)));
    let last = lines
        .iter()
        .position(|l| *l == end(name))
        .unwrap_or_else(|| panic!("no `{}` marker in docs/cursor-admin.md", end(name)));
    assert!(first < last, "the {name} markers are out of order");
    lines[first + 1..last].iter().map(|l| (*l).to_owned()).collect()
}

fn replace_block(doc: &str, name: &str, body: &[String]) -> String {
    let lines: Vec<&str> = doc.lines().collect();
    let first = lines.iter().position(|l| l.starts_with(&begin(name))).unwrap();
    let last = lines.iter().position(|l| *l == end(name)).unwrap();
    let mut out: Vec<String> = lines[..=first].iter().map(|l| (*l).to_owned()).collect();
    out.extend(body.iter().cloned());
    out.extend(lines[last..].iter().map(|l| (*l).to_owned()));
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn render_allowlist(read: &[Tool]) -> Vec<String> {
    let mut out = vec![
        String::new(),
        format!(
            "Allowlist these {} read tools, and nothing else. Each is Viewer-level, asks nothing and changes nothing.",
            read.len()
        ),
        String::new(),
    ];
    out.extend(read.iter().map(|t| format!("- `{}`", t.name)));
    out.push(String::new());
    out
}

fn render_write_tools(write: &[Tool]) -> Vec<String> {
    let asking = write.iter().filter(|t| t.confirmation == "required").count();
    let mut out = vec![
        String::new(),
        format!(
            "These {} write tools are never allowlisted. {asking} of them change something outside the hub's database, and the server \
             asks the user before each of them acts (see \"What a write looks like\").",
            write.len()
        ),
        String::new(),
        "| Tool | Lowest role | The server asks the user |".to_owned(),
        "|---|---|---|".to_owned(),
    ];
    out.extend(write.iter().map(|t| {
        let asks = if t.confirmation == "required" {
            "yes"
        } else {
            "no, it only records a draft"
        };
        format!("| `{}` | {} | {asks} |", t.name, t.role)
    }));
    out.push(String::new());
    out
}

/// The lines of the doc outside its generated blocks.
fn prose(doc: &str) -> String {
    let mut inside = false;
    let mut out = String::new();
    for line in doc.lines() {
        if line.starts_with("<!-- BEGIN GENERATED") {
            inside = true;
        } else if line.starts_with("<!-- END GENERATED") {
            inside = false;
        } else if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The heading level of a Markdown line (`## Title` is 2), or 0 when the line is not a heading.
fn heading_level(line: &str) -> usize {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
        hashes
    } else {
        0
    }
}

/// The text under the heading `title` (any level), up to the next heading of the same or a higher level. A `#` line inside a code
/// fence is a comment, not a heading.
fn section(doc: &str, title: &str) -> String {
    let mut in_fence = false;
    let mut found: Option<usize> = None;
    let mut out = String::new();
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        let level = if in_fence { 0 } else { heading_level(line) };
        match found {
            None => {
                if level > 0 && line.trim_start_matches('#').trim() == title {
                    found = Some(level);
                }
            }
            Some(start_level) => {
                if level > 0 && level <= start_level {
                    break;
                }
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    assert!(found.is_some(), "docs/cursor-admin.md has no heading `{title}`");
    out
}

/// The tool names in a generated block's bullet list.
fn bullets(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("- `"))
        .filter_map(|l| l.strip_suffix('`'))
        .map(str::to_owned)
        .collect()
}

// ----------------------------------------------------------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_contract_document_and_the_embedded_contract_agree_on_who_is_read_only() {
    let embedded = contract();
    let (read, _) = split(&embedded);
    let document = contract_document();
    let from_document: BTreeSet<&str> = document
        .tools
        .iter()
        .filter(|(_, role, confirmation)| role == "viewer" && confirmation == "none")
        .map(|(name, _, _)| name.as_str())
        .collect();
    let from_embedded: BTreeSet<&str> = read.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        from_embedded, from_document,
        "tools.json and docs/mcp-tools.md disagree on the read tools: run `just mcp-schemas`"
    );
    assert_eq!(
        document.allowlist_count,
        read.len(),
        "docs/mcp-tools.md says its allowlist lists {} read tools, the contract has {}",
        document.allowlist_count,
        read.len()
    );
}

#[test]
fn generated_blocks_equal_the_contract() {
    let (read, write) = split(&contract());
    let mut text = doc();
    let want_allowlist = render_allowlist(&read);
    let want_write = render_write_tools(&write);
    if std::env::var_os("UPDATE_ADMIN_DOC").is_some() {
        text = replace_block(&text, ALLOWLIST_BLOCK, &want_allowlist);
        text = replace_block(&text, WRITE_BLOCK, &want_write);
        fs::write(repo_root().join("docs/cursor-admin.md"), &text).unwrap();
    }
    assert_eq!(
        block(&text, ALLOWLIST_BLOCK),
        want_allowlist,
        "the allowlist block is stale: run `just mcp-admin-doc`"
    );
    assert_eq!(
        block(&text, WRITE_BLOCK),
        want_write,
        "the write-tools block is stale: run `just mcp-admin-doc`"
    );
}

#[test]
fn allowlist_equals_the_contracts_read_tools() {
    let (read, _) = split(&contract());
    let listed = bullets(&block(&doc(), ALLOWLIST_BLOCK));
    let expected: Vec<String> = read.iter().map(|t| t.name.clone()).collect();
    assert_eq!(
        listed, expected,
        "the allowlist is not the contract's read tools, in the contract's order"
    );
    let unique: BTreeSet<&String> = listed.iter().collect();
    assert_eq!(unique.len(), listed.len(), "a tool is listed twice");
    assert_eq!(
        listed.len(),
        contract_document().allowlist_count,
        "the allowlist does not have the number of tools docs/mcp-tools.md gives"
    );
}

#[test]
fn allowlist_has_no_write_tool() {
    let (_, write) = split(&contract());
    let listed = bullets(&block(&doc(), ALLOWLIST_BLOCK));
    for tool in &write {
        assert!(
            !listed.contains(&tool.name),
            "the write tool {} is on the allowlist",
            tool.name
        );
    }
    assert!(
        !listed.iter().any(|n| n == "notify_services"),
        "notify_services is never allowlisted"
    );
    // The text around the list must not list a write tool as allowed either: the allowlist block holds the bullets and nothing else.
    for line in block(&doc(), ALLOWLIST_BLOCK) {
        for tool in &write {
            assert!(
                !line.contains(&tool.name),
                "the allowlist block mentions the write tool {}: {line}",
                tool.name
            );
        }
    }
    // The never-allowlisted list has every write tool, `notify_services` among them.
    let never = block(&doc(), WRITE_BLOCK).join("\n");
    for tool in &write {
        assert!(
            never.contains(&format!("`{}`", tool.name)),
            "{} is missing from the write-tools block",
            tool.name
        );
    }
    assert!(never.contains("`notify_services`"));
}

#[test]
fn counts_in_the_prose_equal_the_contract() {
    let tools = contract();
    let (read, write) = split(&tools);
    let asking = write.iter().filter(|t| t.confirmation == "required").count();
    let text = prose(&doc());
    for (phrase, want) in [
        ("read tools", read.len()),
        ("write tools", write.len()),
        ("tools in all", tools.len()),
        ("tools ask", asking),
    ] {
        for (at, _) in text.match_indices(phrase) {
            if let Some(n) = text[..at]
                .split_whitespace()
                .next_back()
                .and_then(|w| w.parse::<usize>().ok())
            {
                assert_eq!(n, want, "the guide says {n} {phrase}; the contract has {want}");
            }
        }
    }
}

#[test]
fn both_modes_and_the_never_combine_rule_are_stated() {
    let text = doc();
    let oauth = section(&text, "OAuth with elicitation (the default)");
    assert!(oauth.contains("LK_MCP_AUTH=oauth"), "{oauth}");
    assert!(oauth.contains("elicitation"));
    let token = section(&text, "Personal-token mode");
    assert!(token.contains("LK_MCP_AUTH=personal_token"), "{token}");
    assert!(token.contains("LANEKEEPER_TOKEN"));
    let never = section(&text, "Why the two modes never combine");
    for needle in [
        "D52",
        "auth",
        "headers",
        "LK_MCP_AUTH=oauth,personal_token",
        "refuses to start",
    ] {
        assert!(
            never.contains(needle),
            "the never-combine section does not say `{needle}`: {never}"
        );
    }
    // The headline rule is also in the status summary an administrator reads first.
    assert!(text.contains("never combined") || text.contains("never combine"));
}

#[test]
fn the_never_combine_claim_is_what_the_crate_does() {
    // The guide says the server refuses to start with both modes. This is the crate's own check.
    let both =
        McpConfig::from_lookup(|name| (name == "LK_MCP_AUTH").then(|| "oauth,personal_token".to_owned()));
    assert_eq!(both, Err(ConfigError::BothAuthModes));
    let reversed =
        McpConfig::from_lookup(|name| (name == "LK_MCP_AUTH").then(|| "personal_token,oauth".to_owned()));
    assert_eq!(reversed, Err(ConfigError::BothAuthModes));
}

#[test]
fn documented_switches_are_the_ones_the_crate_reads() {
    let text = doc();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (at, _) in text.match_indices("LK_MCP_") {
        let rest = &text[at..];
        let Some(eq) = rest.find('=') else { continue };
        let name = &rest[..eq];
        if !name.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
            continue;
        }
        let value: String = rest[eq + 1..]
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || matches!(c, '_' | ','))
            .collect();
        if value.is_empty() {
            continue;
        }
        let setting = format!("{name}={value}");
        let lookup = |n: &str| (n == name).then(|| value.clone());
        let parsed = McpConfig::from_lookup(lookup);
        if setting == "LK_MCP_AUTH=oauth,personal_token" {
            assert_eq!(parsed, Err(ConfigError::BothAuthModes), "{setting}");
        } else {
            let config = parsed
                .unwrap_or_else(|e| panic!("the guide documents {setting}, which the crate refuses: {e}"));
            match name {
                "LK_MCP_AUTH" => assert_eq!(config.auth_mode.to_string(), value),
                "LK_MCP_CONFIRM" => assert_eq!(config.confirm_mode.to_string(), value),
                "LK_MCP_CONTENT" => assert_eq!(config.content_mode.to_string(), value),
                other => panic!("the guide documents a switch the crate does not read: {other}"),
            }
        }
        seen.insert(setting);
    }
    for must in [
        "LK_MCP_AUTH=oauth",
        "LK_MCP_AUTH=personal_token",
        "LK_MCP_CONFIRM=elicitation",
        "LK_MCP_CONFIRM=draft",
        "LK_MCP_CONTENT=summary",
        "LK_MCP_CONTENT=data_block",
    ] {
        assert!(seen.contains(must), "the guide never shows {must}");
    }
    // The defaults the guide states are the crate's.
    let defaults = McpConfig::default();
    assert_eq!(defaults.auth_mode, AuthMode::Oauth);
    assert_eq!(defaults.confirm_mode, ConfirmMode::Elicitation);
    assert_eq!(defaults.content_mode, ContentMode::Summary);
    assert!(section(&text, "OAuth with elicitation (the default)").contains("default"));
}

fn days(d: Duration) -> u64 {
    d.as_secs() / 86_400
}

#[test]
fn documented_numbers_and_paths_are_the_ones_in_the_code() {
    let text = doc();
    let limits = LimitSettings::default();
    for phrase in [
        format!("{} days", days(DEFAULT_LIFETIME)),
        format!("{DEFAULT_MAX_LIVE} live tokens"),
        format!("within {} seconds", DEFAULT_CACHE_TTL.as_secs()),
        format!("waits {} seconds", limits.confirm_timeout.as_secs()),
        format!("{} sessions per user", limits.sessions_per_user),
        SCOPE_NAME.to_owned(),
        RESOURCE_METADATA_PATH.to_owned(),
        RESOURCE_METADATA_ROOT_PATH.to_owned(),
        AUTHORIZATION_SERVER_PATH.to_owned(),
        COLLECTION_PATH.to_owned(),
        format!("DELETE {ITEM_PATH}"),
        "`/mcp`".to_owned(),
        "offline_access".to_owned(),
        "lkpat_".to_owned(),
        "draft_link".to_owned(),
        "notify_services".to_owned(),
        "tools.notify_services.enabled".to_owned(),
    ] {
        assert!(text.contains(&phrase), "the guide never says `{phrase}`");
    }
}

/// Every number written just before `unit` in `text`.
fn numbers_before(text: &str, unit: &str) -> Vec<u64> {
    text.match_indices(unit)
        .filter_map(|(at, _)| text[..at].split_whitespace().next_back()?.parse().ok())
        .collect()
}

#[test]
fn every_number_with_a_unit_in_the_guide_is_a_number_of_the_code() {
    // `documented_numbers_and_paths_are_the_ones_in_the_code` shows that the code's number is written somewhere; this shows that no
    // other number is written next to the same unit, so a stale copy cannot hide behind a right one.
    let text = doc();
    let limits = LimitSettings::default();
    let allowed: [(&str, Vec<u64>); 5] = [
        (" days", vec![days(DEFAULT_LIFETIME)]),
        (" live tokens", vec![DEFAULT_MAX_LIVE as u64]),
        (
            " seconds",
            vec![DEFAULT_CACHE_TTL.as_secs(), limits.confirm_timeout.as_secs()],
        ),
        (" minutes", vec![limits.session_idle.as_secs() / 60]),
        (" sessions per user", vec![u64::from(limits.sessions_per_user)]),
    ];
    for (unit, ok) in allowed {
        let found = numbers_before(&text, unit);
        assert!(!found.is_empty(), "the guide never gives a number {unit}");
        for n in found {
            assert!(ok.contains(&n), "the guide says {n}{unit}; the code has {ok:?}");
        }
    }
}

#[test]
fn draft_mode_is_explained() {
    let text = doc();
    let draft = section(&text, "Draft mode");
    for needle in [
        "LK_MCP_CONFIRM=draft",
        "draft_link",
        "web_link",
        "elicitation",
        "nothing is written",
        "C2",
    ] {
        assert!(
            draft.contains(needle),
            "the draft-mode section does not say `{needle}`"
        );
    }
    let write = section(&text, "What a write looks like");
    for needle in ["confirm", "accept", "decline", "cancel"] {
        assert!(
            write.contains(needle),
            "the write section does not say `{needle}`"
        );
    }
}

#[test]
fn personal_token_mode_states_the_origin_only_csrf_deviation() {
    // Accepted by the human after T1 (the hub composes no CSRF layer yet): the token routes check Origin and nothing else, so the
    // double-submit half of S3 is not met. The guide says so, with the residual risk and who finishes it.
    let token = section(&doc(), "Personal-token mode");
    for needle in [
        "S3",
        "Origin",
        "double-submit",
        "CSRF",
        "not implemented",
        "integration",
    ] {
        assert!(
            token.contains(needle),
            "the personal-token section does not say `{needle}`"
        );
    }
    let limits = section(&doc(), "Known limits and accepted deviations");
    assert!(limits.contains("S3") && limits.contains("Origin"));
}

#[test]
fn unverified_items_are_marked_and_the_admin_is_told_to_check_on_the_first_install() {
    let text = doc();
    assert!(text.contains("UNVERIFIED"));
    assert!(
        text.contains("first install"),
        "the guide must tell the administrator to check each UNVERIFIED item on the first install"
    );
    // Every part that depends on a Cursor format or a Cursor menu carries the mark itself, so that it cannot be skimmed past.
    for title in [
        "The plugin package",
        "Marketplace entry",
        "MCP allowlist URL",
        "Tool allowlist",
        "First install checklist",
    ] {
        let body = section(&text, title);
        let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
        assert!(
            first.contains("UNVERIFIED"),
            "the section `{title}` depends on Cursor details that could not be checked, and must open with UNVERIFIED; it opens: {first}"
        );
    }
}

/// True while any cell of the D91 table in `docs/decisions.md` is NOT RUN.
fn d91_has_not_run_cells() -> bool {
    let decisions = read_repo("docs/decisions.md");
    let start = decisions.find("- **D91**").expect("D91 is in docs/decisions.md");
    let end = decisions[start..]
        .find("- **D92**")
        .map_or(decisions.len(), |e| start + e);
    decisions[start..end]
        .lines()
        .any(|l| l.trim_start().starts_with('|') && l.contains("NOT RUN"))
}

#[test]
fn t0_is_reported_as_not_run_exactly_while_d91_says_so() {
    let text = doc();
    let status = section(&text, "Status: what is verified and what is not");
    assert!(status.contains("D91"));
    let says_not_run = status.contains("**NOT RUN: the Cursor 3.22.12 compatibility test");
    assert_eq!(
        says_not_run,
        d91_has_not_run_cells(),
        "the guide's status and the D91 table disagree about whether the Cursor 3.22.12 checks (T0) were run: update the guide from D91"
    );
}

/// True when a file of the hub binary other than the module itself mentions the MCP module (the composition root builds it).
fn hub_binary_mounts_mcp() -> bool {
    let src = repo_root().join("crates/hub/src");
    fs::read_dir(src).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        path.extension().is_some_and(|ext| ext == "rs")
            && name != "mcp.rs"
            && name != "lib.rs"
            && fs::read_to_string(&path).unwrap().contains("McpModule")
    })
}

#[test]
fn the_guide_says_whether_the_hub_binary_mounts_the_mcp_module_as_the_code_does() {
    let status = section(&doc(), "Status: what is verified and what is not");
    let says_unmounted = status.contains("NOT MOUNTED YET");
    assert_eq!(
        says_unmounted,
        !hub_binary_mounts_mcp(),
        "the guide's claim about the hub binary mounting /mcp is out of date"
    );
}

#[test]
fn troubleshooting_covers_the_answers_the_server_gives() {
    let trouble = section(&doc(), "Troubleshooting");
    for needle in [
        "401",
        "WWW-Authenticate",
        "invalid_token",
        "403",
        "503",
        "404",
        "Mcp-Session-Id",
        "429",
        "rate_limited",
        "draft_link",
        "AADSTS50011",
        "409",
        "Host",
        "Origin",
        "forbidden",
    ] {
        assert!(
            trouble.contains(needle),
            "the troubleshooting section never mentions `{needle}`"
        );
    }
}

#[test]
fn plugin_readme_marks_the_cursor_format_details_unverified() {
    let readme =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugin/README.md")).unwrap();
    assert!(readme.contains("UNVERIFIED"));
    assert!(readme.contains("first install"));
    for file in [
        "plugin.json",
        "mcp.json",
        "package.sh",
        "mcp.oauth.json",
        "mcp.pat.json",
        "skills/lanekeeper",
    ] {
        assert!(readme.contains(file), "plugin/README.md never mentions {file}");
    }
    assert!(
        readme.contains("A9"),
        "the README should cite the plan's ambiguity A9"
    );
    assert!(readme.contains("docs/cursor-admin.md"));
}
