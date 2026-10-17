//! P7, the text search across all swimlanes through the API (`P7.text_search`, `hub-registry/search_text_all_swimlanes`, a harness,
//! p95, 400 ms; the engine's own share is `P7.engine_text_search`): the engine's twenty queries (frequent, rare and absent literals,
//! case-insensitive ones, regexes) over the stored blobs of the 40 swimlanes of the read bed, as the router answers them, with the
//! database read of the blobs and the HTTP layer inside the number (the registry entry of the budget says so).
//!
//! **What the budget measures (decision D97, approved by the human on 2026-10-15): the steady state.** The hub is warm (the text corpora
//! hold every swimlane), a file of ONE swimlane has a new version, and the first search after that is timed. The search does not wait
//! for the background warm-up (it is not running in this case: the worst case, a user who searches right after the change): it reads
//! the changed swimlane's file list and makes again only the shards whose places changed. 40 samples, two for each query, each on a
//! different swimlane, each held to the answer of a service that reuses nothing. Its result file is
//! `hub-registry_search_text_all_swimlanes.json`.
//!
//! The other states are measured and logged, each in a result file of its own, and none is part of the budget:
//!
//! - **steady, warm-up settled**: the same change, with the warm-up task running and given time to take it in before the search (20).
//! - **warm**: the corpora are current; the search itself (60).
//! - **rebuilt-all**: the version of every swimlane has moved (the files have not), so every file list is read again (40).
//! - **rebuilt from nothing**: the service has nothing cached, the blob cache has the blobs (40).
//! - **cold**: the hub has just started and has not warmed up: every file list, all the distinct blobs (about 23,000) read from the
//!   `blobs` table through the real blob store with an empty cache, every shard made (40, a restart before each).
//!
//! The answers are checked, so work that disappears shows as a failure and not as speed: the unchanged-data states against the first
//! answer of each query, the steady states against a service that has nothing cached.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

mod common;

use std::fmt::Write as _;
use std::time::Duration;

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
#[allow(clippy::too_many_lines)]
async fn main() {
    let config = RegistryConfig {
        // The warm-up variant: look every 100 ms, build at once.
        warmup_poll: Duration::from_millis(100),
        warmup_debounce: Duration::ZERO,
        ..RegistryConfig::default()
    };
    let Some(mut bed) = Box::pin(ReadBed::new(40, config)).await else {
        eprintln!("hub-registry/search_text_all_swimlanes: SKIPPED (no database)");
        return;
    };
    // The first answers (made cold), which every later answer on the same data must equal; the set has both kinds of query.
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

    // Cold: a hub that has just started and has not warmed up, then one query, twice for every query.
    let mut cold = Vec::new();
    for _ in 0..2 {
        for (k, q) in QUERIES.iter().enumerate() {
            bed.restart();
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            cold.push(r.0);
        }
    }
    // Rebuilt from nothing: the service has nothing cached (the corpora and the file lists are gone); the blob cache has the blobs.
    let mut rebuilt_from_nothing = Vec::new();
    for _ in 0..2 {
        for (k, q) in QUERIES.iter().enumerate() {
            bed.service.clear_caches();
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            rebuilt_from_nothing.push(r.0);
        }
    }
    // Warm: the corpora are current.
    let mut warm = Vec::new();
    for _ in 0..3 {
        for (k, q) in QUERIES.iter().enumerate() {
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            warm.push(r.0);
        }
    }
    // Rebuilt-all: the version of every swimlane has moved (nothing in the files has), so every file list is read again.
    let mut rebuilt_all = Vec::new();
    for _ in 0..2 {
        for (k, q) in QUERIES.iter().enumerate() {
            sqlx::query("UPDATE swimlane_registry SET version = version + 1")
                .execute(&bed.bed.rig.pool)
                .await
                .unwrap();
            let r = bed.get_json_timed(&uri(q)).await;
            check(k, &r.1);
            rebuilt_all.push(r.0);
        }
    }

    // Steady state, the registered case: the hub is warm (every swimlane is in the corpora), and a file of ONE swimlane has a new
    // version; the first search after it takes that in itself (the warm-up is not running: this is the worst case, a user who searches
    // right after the change). One search for each query twice; every answer is held to what a service that reuses nothing gives.
    let mut steady = Vec::new();
    bed.restart();
    let _ = bed.json(&uri(&QUERIES[0])).await;
    assert!(bed.service.text_corpora_are_current().await.unwrap());
    for round in 0..2 * QUERIES.len() {
        let (lane, k) = ((round * 7) % 40, round % QUERIES.len());
        let editable = bed.bed.editable(lane);
        let file = editable[round % editable.len()];
        bed.bed.edit(lane, &[file]).await;
        assert!(
            !bed.service.text_corpora_are_current().await.unwrap(),
            "the edit moved the swimlane's version, so the corpora are behind it"
        );
        let r = bed.get_json_timed(&uri(&QUERIES[k])).await;
        let reference = bed.json_from_scratch(&uri(&QUERIES[k])).await;
        assert_eq!(
            r.1, reference,
            "round {round}, query {k}: the answer from the reused corpora is not the one from nothing"
        );
        steady.push(r.0);
    }
    // The same with the warm-up running and settled before the search: what a user meets when the hub has had a moment.
    let mut steady_warmed = Vec::new();
    let mut catch_up = Vec::new();
    let warmup = bed.service.spawn_warmup();
    for round in 0..QUERIES.len() {
        let (lane, k) = ((round * 11 + 3) % 40, round % QUERIES.len());
        let editable = bed.bed.editable(lane);
        let file = editable[(round + 5) % editable.len()];
        bed.bed.edit(lane, &[file]).await;
        let started = std::time::Instant::now();
        while !bed.service.text_corpora_are_current().await.unwrap() {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the warm-up did not catch up"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        catch_up.push(started.elapsed().as_secs_f64() * 1000.0);
        let r = bed.get_json_timed(&uri(&QUERIES[k])).await;
        let reference = bed.json_from_scratch(&uri(&QUERIES[k])).await;
        assert_eq!(r.1, reference, "warmed round {round}, query {k}");
        steady_warmed.push(r.0);
    }
    let weight = bed.service.text_corpora_weight().unwrap();
    warmup.shutdown().await;

    let name = "hub-registry/search_text_all_swimlanes";
    eprintln!(
        "{name}: STEADY (the registered case: one swimlane changed, the warm-up not running) {}",
        summary(&steady)
    );
    eprintln!("{name}: steady, warm-up settled {}", summary(&steady_warmed));
    eprintln!(
        "{name}: the warm-up took a change in {} after the edit (poll 100 ms, no debounce); the corpora hold {:.1} MiB",
        summary(&catch_up),
        weight as f64 / 1_048_576.0
    );
    eprintln!("{name}: warm (corpora current) {}", summary(&warm));
    eprintln!(
        "{name}: rebuilt-all (every swimlane's version moved) {}",
        summary(&rebuilt_all)
    );
    eprintln!(
        "{name}: rebuilt from nothing (blobs in the blob cache) {}",
        summary(&rebuilt_from_nothing)
    );
    eprintln!("{name}: cold (just started, no warm-up) {}", summary(&cold));
    write_result(
        "hub-registry/search_text_all_swimlanes_steady_warmed",
        "ms",
        &steady_warmed,
    );
    write_result("hub-registry/search_text_all_swimlanes_warm", "ms", &warm);
    write_result(
        "hub-registry/search_text_all_swimlanes_rebuilt_all",
        "ms",
        &rebuilt_all,
    );
    write_result(
        "hub-registry/search_text_all_swimlanes_rebuilt",
        "ms",
        &rebuilt_from_nothing,
    );
    write_result("hub-registry/search_text_all_swimlanes_cold", "ms", &cold);
    let path = write_result("hub-registry/search_text_all_swimlanes", "ms", &steady);
    eprintln!("{name}: {}", path.display());
}
