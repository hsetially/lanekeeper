//! P7, the text search across all swimlanes through the API (`P7.text_search`, `hub-registry/search_text_all_swimlanes`, a harness,
//! p95, 400 ms; the engine's own share is `P7.engine_text_search`): the engine's twenty queries (frequent, rare and absent literals,
//! case-insensitive ones, regexes) over the stored blobs of the 40 swimlanes of the read bed, as the router answers them, with the
//! database read of the blobs and the HTTP layer inside the number (the registry entry of the budget says so).
//!
//! Three states of the hub are measured and all three are logged:
//!
//! - **cold**: the hub has just started. The corpus is built for the request: the file lists of the 40 swimlanes, all the distinct
//!   blobs (about 23,000) read from the `blobs` table through the real blob store whose cache is empty, the corpus made, the search
//!   run, the JSON. Every query is run twice in this state (a restart before each), 40 samples. **The budget takes these**, because the
//!   budget's own text puts the database read of the blobs in the number, and because the cold search is the slowest the user meets.
//! - **rebuilt**: the corpus is built again (as it is after any change to any swimlane, because it names the versions of all of them)
//!   with the blobs in the blob cache. 40 samples.
//! - **warm**: the corpus is cached; the search itself. 60 samples.
//!
//! The answers are checked against the first answer of each query, so work that disappears shows as a failure and not as speed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

/// The engine's queries (pattern, regex, case-sensitive).
const QUERIES: [(&str, bool, bool); 20] = [
    ("corp.example", false, true),
    ("true", false, true),
    ("weight", false, true),
    ("enabled", false, true),
    ("settlement", false, true),
    ("CORE_ROUTING", false, true),
    ("coreRouting", false, true),
    ("https://auth.uat.corp.example", false, true),
    ("POD_IP", false, true),
    ("no-such-setting-xyzzy", false, true),
    ("DEFAULTS", false, false),
    ("Retry", false, false),
    ("NO-SUCH-SETTING-XYZZY", false, false),
    (r"\bretry\b", true, true),
    (
        r"https://(?:auth|docs)\.(?:sit|uat)\.corp\.example/\w+",
        true,
        true,
    ),
    (r"^\s*weight: \d{3}$", true, true),
    (r"\$\{[A-Z_]+\}", true, true),
    (r"enabled:\s*(?:true|false)", true, false),
    (r"\b\d{4}\b", true, true),
    (r"timeout|retries|limit\d", true, true),
];

fn enc(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn uri(q: &(&str, bool, bool)) -> String {
    format!(
        "/api/v1/search/text?pattern={}&regex={}&case_sensitive={}&max_total=500&max_per_file=20",
        enc(q.0),
        q.1,
        q.2
    )
}

#[tokio::main]
async fn main() {
    let Some(mut bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/search_text_all_swimlanes: SKIPPED (no database)");
        return;
    };
    // The first answers (made cold), which every later answer must equal; the set has both kinds of query.
    bed.restart();
    let mut expected: Vec<serde_json::Value> = Vec::new();
    for q in &QUERIES {
        expected.push(bed.json(&uri(q)).await);
    }
    let empty = expected
        .iter()
        .filter(|a| a["hits"].as_array().unwrap().is_empty())
        .count();
    let cut = expected.iter().filter(|a| a["truncated"] == true).count();
    let found = expected
        .iter()
        .filter(|a| !a["hits"].as_array().unwrap().is_empty())
        .count();
    assert!(
        empty >= 2 && cut >= 3 && found >= 12,
        "{empty} empty, {cut} cut, {found} found"
    );
    eprintln!(
        "hub-registry/search_text_all_swimlanes: {found} queries find something, {cut} are cut by a cap, {empty} find nothing"
    );

    let check = |k: usize, answer: &serde_json::Value| {
        assert_eq!(answer, &expected[k], "query {k} gave another answer");
    };

    // Cold: a hub that has just started, then one query, twice for every query.
    let mut cold = Vec::new();
    for _ in 0..2 {
        for (k, q) in QUERIES.iter().enumerate() {
            bed.restart();
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            cold.push(r.0);
        }
    }
    // Rebuilt: the service has nothing cached (as after a change to any swimlane); the blob cache has the blobs.
    let mut rebuilt = Vec::new();
    for _ in 0..2 {
        for (k, q) in QUERIES.iter().enumerate() {
            bed.service.clear_caches();
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            rebuilt.push(r.0);
        }
    }
    // Warm: the corpus is cached.
    let mut warm = Vec::new();
    for _ in 0..3 {
        for (k, q) in QUERIES.iter().enumerate() {
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            warm.push(r.0);
        }
    }
    eprintln!("hub-registry/search_text_all_swimlanes: cold {}", summary(&cold));
    eprintln!(
        "hub-registry/search_text_all_swimlanes: rebuilt (blobs in the blob cache) {}",
        summary(&rebuilt)
    );
    eprintln!(
        "hub-registry/search_text_all_swimlanes: warm (corpus cached) {}",
        summary(&warm)
    );
    write_result("hub-registry/search_text_all_swimlanes_rebuilt", "ms", &rebuilt);
    write_result("hub-registry/search_text_all_swimlanes_warm", "ms", &warm);
    let path = write_result("hub-registry/search_text_all_swimlanes", "ms", &cold);
    eprintln!("hub-registry/search_text_all_swimlanes: {}", path.display());
}
