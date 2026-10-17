//! P7, the file tree through the API at design scale (`P7.file_tree`, `hub-registry/file_tree`, a harness, p95, 100 ms): the entries
//! of a folder, with the worst state among the files below each folder, from the read bed (40 swimlanes of about 2,000 files, every
//! swimlane computed), as the router answers it.
//!
//! Every swimlane is asked four things, three rounds over: its root (a page of 100), its largest top-level folder and its largest
//! second-level folder (pages of 500, the most a page can be), and its root again with a drift filter. Each request is a sample.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::collections::BTreeMap;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const ROUNDS: usize = 3;

/// The folder with the most files at this depth (1: `a`, 2: `a/b`), by its path.
fn largest_folder(lane: &common::Lane, depth: usize) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (path, _) in &lane.files {
        let parts: Vec<&str> = path.as_str().split('/').collect();
        if parts.len() > depth {
            *counts.entry(parts[..depth].join("/")).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(name, n)| (*n, std::cmp::Reverse(name.clone())))
        .map(|(name, _)| name)
        .expect("a folder at that depth")
}

fn enc(path: &str) -> String {
    path.replace('/', "%2F")
}

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/file_tree: SKIPPED (no database)");
        return;
    };
    let mut requests: Vec<(String, &str)> = Vec::new();
    for lane in &bed.bed.lanes {
        let id = lane.id.as_str();
        let (d1, d2) = (largest_folder(lane, 1), largest_folder(lane, 2));
        requests.push((format!("/api/v1/swimlanes/{id}/tree?limit=100"), "root"));
        requests.push((
            format!("/api/v1/swimlanes/{id}/tree?prefix={}&limit=500", enc(&d1)),
            "folder",
        ));
        requests.push((
            format!("/api/v1/swimlanes/{id}/tree?prefix={}&limit=500", enc(&d2)),
            "subfolder",
        ));
        requests.push((
            format!("/api/v1/swimlanes/{id}/tree?drift=untracked&limit=100"),
            "root, filtered",
        ));
    }
    // Every answer is a real listing, and the folders are real ones.
    for (uri, what) in &requests {
        let answer = bed.json(uri).await;
        let items = answer["items"].as_array().unwrap();
        if *what != "root, filtered" {
            assert!(!items.is_empty(), "{what}: {uri}");
        }
    }
    let mut samples = Vec::new();
    let mut by_kind: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut biggest = 0;
    for _ in 0..ROUNDS {
        for (uri, what) in &requests {
            let r = bed.ok(uri).await;
            biggest = biggest.max(r.bytes);
            samples.push(r.ms);
            by_kind.entry(what).or_default().push(r.ms);
        }
    }
    for (what, ms) in &by_kind {
        eprintln!("hub-registry/file_tree: {what}: {}", summary(ms));
    }
    eprintln!(
        "hub-registry/file_tree: all: {} (largest answer {biggest} bytes)",
        summary(&samples)
    );
    let path = write_result("hub-registry/file_tree", "ms", &samples);
    eprintln!("hub-registry/file_tree: {}", path.display());
}
