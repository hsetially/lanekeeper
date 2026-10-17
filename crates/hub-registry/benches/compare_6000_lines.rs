//! P7, the compare of two versions of a 6,000-line file through the API, database included (`P7.compare_api`,
//! `hub-registry/compare_6000_lines`, a harness, p95, 100 ms; the engine's own share is `P7.compare`).
//!
//! The file is the largest YAML of the fixtures' first swimlane (the 6,000-line `reference-data/catalog-big-01.yml`). Forty pairs of
//! versions are made of it: in the first swimlane the file as it is, in the second the file with every twentieth line changed (a number
//! is incremented, any other line gets a trailing comment), and each pair has a marker line of its own at the end, so that all eighty
//! blobs are different. As in a running hub the settings index has seen the new blobs by the time anyone compares them.
//!
//! A sample is `GET /api/v1/compare` of one pair, **cold**: the pair has never been compared, so the diff is made, and neither blob is
//! in the blob cache, so both are read from the `blobs` table (the guard, the two file rows, two blob reads, the line diff, the settings
//! diff, the JSON). The budget takes those. The same pair asked again (the diff is cached by the two hashes) is logged, not budgeted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;

use bytes::Bytes;
use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use domain::NfsPath;
use hub_registry::{RegistryConfig, RegistryMetrics, SettingsSweeper};

const PAIRS: usize = 40;

/// The text with every twentieth line changed: an integer value goes up by one, any other line gets a comment (so that it stays YAML).
fn mutate(text: &str, marker: usize) -> String {
    let mut out = String::with_capacity(text.len() + 4096);
    for (i, line) in text.lines().enumerate() {
        let changed = if i % 20 == 10 {
            match line.rsplit_once(": ") {
                Some((key, value)) if value.trim().parse::<i64>().is_ok() => {
                    format!("{key}: {}", value.trim().parse::<i64>().unwrap() + 1)
                }
                _ => format!("{line} # changed"),
            }
        } else {
            line.to_owned()
        };
        out.push_str(&changed);
        out.push('\n');
    }
    let _ = writeln!(out, "compare_marker_{marker}: {marker}");
    out
}

#[tokio::main]
async fn main() {
    let Some(mut bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/compare_6000_lines: SKIPPED (no database)");
        return;
    };
    // The largest YAML of the first swimlane.
    let (_, original) = {
        let lane = &bed.bed.lanes[0];
        let i = bed
            .bed
            .editable(0)
            .into_iter()
            .max_by_key(|&f| lane.files[f].1.len())
            .unwrap();
        lane.files[i].clone()
    };
    let text = String::from_utf8(original.to_vec()).unwrap();
    let lines = text.lines().count();
    assert!((5_000..=7_000).contains(&lines), "the file has {lines} lines");

    let mut left = Vec::new();
    let mut right = Vec::new();
    for i in 0..PAIRS {
        let path = NfsPath::parse(&format!("bench/cmp{i}.yml")).unwrap();
        left.push((
            path.clone(),
            Bytes::from(format!("{text}compare_marker_{i}: {i}\n")),
        ));
        right.push((path, Bytes::from(mutate(&text, i))));
    }
    bed.bed.put_files(0, left).await;
    bed.bed.put_files(1, right).await;
    // The index has seen the new blobs, as the worker of a running hub makes sure of.
    let (indexer, _worker) = bed
        .bed
        .rig
        .another_indexer(bed.config.clone(), RegistryMetrics::default());
    let swept = SettingsSweeper::new(indexer).sweep(|| Ok(())).await.unwrap();
    eprintln!(
        "hub-registry/compare_6000_lines: {lines} lines, {PAIRS} pairs, {} blobs indexed",
        swept.found
    );
    let (a, b) = (bed.bed.lanes[0].id.clone(), bed.bed.lanes[1].id.clone());
    let uri = |i: usize| format!("/api/v1/compare?left=nfs%3A{a}&right=nfs%3A{b}&file=bench%2Fcmp{i}.yml");

    // The answer is a real compare: lines differ and settings differ.
    let first = bed.json(&uri(0)).await;
    assert_eq!(first["identical"], false);
    let hunks = first["hunks"].as_array().unwrap().len();
    let changes = first["setting_changes"].as_array().unwrap().len();
    assert!(
        hunks >= 200 && changes >= 20,
        "{hunks} hunks, {changes} setting changes"
    );
    eprintln!("hub-registry/compare_6000_lines: {hunks} hunks, {changes} setting changes");

    // Cold: a hub that has just started, then every pair once.
    bed.restart();
    let mut cold = Vec::new();
    let mut bytes = 0;
    for i in 0..PAIRS {
        let r = bed.ok(&uri(i)).await;
        bytes = bytes.max(r.bytes);
        cold.push(r.ms);
    }
    // Warm: the same pair again, the diff cached.
    let mut warm = Vec::new();
    for round in 0..PAIRS {
        warm.push(bed.ok(&uri(round % 4)).await.ms);
    }
    eprintln!(
        "hub-registry/compare_6000_lines: cold {} (largest answer {bytes} bytes)",
        summary(&cold)
    );
    eprintln!("hub-registry/compare_6000_lines: warm {}", summary(&warm));
    let path = write_result("hub-registry/compare_6000_lines", "ms", &cold);
    eprintln!("hub-registry/compare_6000_lines: {}", path.display());
}
