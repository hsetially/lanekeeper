//! P7, the settings search across all swimlanes through the API (`P7.settings_search`, `hub-registry/search_settings_all_swimlanes`,
//! a harness, p95, 200 ms): a part of a setting path, over the settings index of the read bed (40 swimlanes, about 23,000 distinct
//! blobs, every setting of every structured file), a page of 100, as the router answers it.
//!
//! The needles are twenty, fixed for the run: ten parts of real setting paths taken from the index by a repeatable sample, five common
//! words that match a large share of the paths, and five that match nothing (so that the whole index is looked at). Each needle is
//! asked five times over; every request is a sample. The needles are what they were when the bench was first written, whatever they
//! cost: the two-character `id` is among them, and the contract allows one character (`minLength: 1`).
//!
//! Short needles that match little (one or two characters, absent or rare) cannot use the trigram index, which needs three: they are the
//! one kind of request the index does not serve, and they are not part of the budget's workload. They are measured here all the same,
//! as a series of their own (`hub-registry/search_settings_short_needles`, no budget), so that the cost is on record and not hidden.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const ROUNDS: usize = 5;

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

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/search_settings_all_swimlanes: SKIPPED (no database)");
        return;
    };
    let indexed: i64 = sqlx::query_scalar("SELECT count(*) FROM settings_index")
        .fetch_one(&bed.bed.rig.pool)
        .await
        .unwrap();
    assert!(indexed > 500_000, "the bed's index has {indexed} settings");
    // Parts of real paths: the last segment of ten paths taken by a repeatable sample, at least three characters.
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT setting_path FROM settings_index TABLESAMPLE SYSTEM (0.05) REPEATABLE (7) LIMIT 2000",
    )
    .fetch_all(&bed.bed.rig.pool)
    .await
    .unwrap();
    let mut needles: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            p.rsplit(['.', '[', ']'])
                .find(|s| s.len() >= 3)
                .map(str::to_lowercase)
        })
        .collect();
    needles.sort();
    needles.dedup();
    let step = (needles.len() / 10).max(1);
    let mut chosen: Vec<String> = needles.into_iter().step_by(step).take(10).collect();
    assert_eq!(chosen.len(), 10, "the sample gave too few distinct words");
    chosen.extend(["name", "id", "url", "enabled", "timeout"].map(str::to_owned));
    chosen.extend(
        [
            "zzzqqqxx",
            "no-such-setting-xyzzy",
            "qqqq9999",
            "xxyyzz-absent",
            "nothing-like-this",
        ]
        .map(str::to_owned),
    );
    let mut samples = Vec::new();
    let mut found = Vec::new();
    for needle in &chosen {
        let answer = bed
            .json(&format!("/api/v1/search/settings?q={}&limit=100", enc(needle)))
            .await;
        found.push((needle.clone(), answer["items"].as_array().unwrap().len()));
    }
    let hits = found.iter().filter(|(_, n)| *n > 0).count();
    assert!(hits >= 12, "most needles find something: {found:?}");
    assert!(
        found.iter().filter(|(_, n)| *n == 0).count() >= 3,
        "and the absent ones find nothing: {found:?}"
    );
    eprintln!("hub-registry/search_settings_all_swimlanes: needles and hits on the first page: {found:?}");
    let mut per_needle: Vec<(String, f64)> = Vec::new();
    for _ in 0..ROUNDS {
        for needle in &chosen {
            let r = bed
                .ok(&format!("/api/v1/search/settings?q={}&limit=100", enc(needle)))
                .await;
            samples.push(r.ms);
            per_needle.push((needle.clone(), r.ms));
        }
    }
    per_needle.sort_by(|a, b| b.1.total_cmp(&a.1));
    eprintln!(
        "hub-registry/search_settings_all_swimlanes: the slowest requests: {:?}",
        &per_needle[..5]
    );
    eprintln!(
        "hub-registry/search_settings_all_swimlanes: {}",
        summary(&samples)
    );
    let path = write_result("hub-registry/search_settings_all_swimlanes", "ms", &samples);
    eprintln!("hub-registry/search_settings_all_swimlanes: {}", path.display());

    // The series that is on record and has no budget: one and two characters, from dense to absent.
    let mut short = Vec::new();
    let mut per_short: Vec<(&str, f64)> = Vec::new();
    for _ in 0..ROUNDS {
        for needle in ["e", "a", "x", "zq", "q9", "xj", "-z", "ww"] {
            let r = bed
                .ok(&format!("/api/v1/search/settings?q={}&limit=100", enc(needle)))
                .await;
            short.push(r.ms);
            per_short.push((needle, r.ms));
        }
    }
    per_short.sort_by(|a, b| b.1.total_cmp(&a.1));
    eprintln!(
        "hub-registry/search_settings_short_needles (no budget): the slowest requests: {:?}",
        &per_short[..4]
    );
    eprintln!(
        "hub-registry/search_settings_short_needles (no budget): {}",
        summary(&short)
    );
    let path = write_result("hub-registry/search_settings_short_needles", "ms", &short);
    eprintln!("hub-registry/search_settings_short_needles: {}", path.display());
}
