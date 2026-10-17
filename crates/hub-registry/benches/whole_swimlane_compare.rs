//! P7, the whole-swimlane compare through the API (`P7.whole_swimlane_compare`, `hub-registry/whole_swimlane_compare`, a harness, p95,
//! 300 ms): two swimlanes of about 2,000 files each, compared folder by folder, over the read bed.
//!
//! Both ways of comparing are in the budgeted samples, in equal number, so that the tail is the slower one. `effective:` against
//! `effective:` walks the stored directory hashes and skips the subtrees that are equal; `nfs:` against `nfs:` has no directory hashes
//! to skip by, reads both sides' files with their hashes and compares them all. Every neighbouring pair of the 40 swimlanes is
//! compared (40 pairs in a ring), a page of 500 each.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const ROUNDS: usize = 2;

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/whole_swimlane_compare: SKIPPED (no database)");
        return;
    };
    let ids: Vec<String> = bed.bed.lanes.iter().map(|l| l.id.to_string()).collect();
    let pairs: Vec<(&String, &String)> = (0..ids.len())
        .map(|i| (&ids[i], &ids[(i + 1) % ids.len()]))
        .collect();
    let uri = |kind: &str, a: &str, b: &str| {
        format!("/api/v1/compare/tree?left={kind}%3A{a}&right={kind}%3A{b}&limit=500")
    };

    // The answers are real compares: files differ, and the effective walk skipped what was equal.
    let mut differing = 0;
    let mut skipped = 0;
    for (a, b) in pairs.iter().take(5) {
        let nfs = bed.json(&uri("nfs", a, b)).await;
        differing += nfs["entries"]["items"].as_array().unwrap().len();
        let eff = bed.json(&uri("effective", a, b)).await;
        skipped += eff["skipped_identical_subtrees"].as_u64().unwrap();
    }
    assert!(differing > 0, "the swimlanes differ");
    eprintln!(
        "hub-registry/whole_swimlane_compare: {differing} differing entries in 5 nfs compares, {skipped} subtrees skipped in 5 effective ones"
    );

    let mut effective = Vec::new();
    let mut nfs = Vec::new();
    let mut biggest = 0;
    for _ in 0..ROUNDS {
        for (a, b) in &pairs {
            let r = bed.ok(&uri("effective", a, b)).await;
            biggest = biggest.max(r.bytes);
            effective.push(r.ms);
            let r = bed.ok(&uri("nfs", a, b)).await;
            biggest = biggest.max(r.bytes);
            nfs.push(r.ms);
        }
    }
    let all: Vec<f64> = effective.iter().chain(&nfs).copied().collect();
    eprintln!(
        "hub-registry/whole_swimlane_compare: effective {}",
        summary(&effective)
    );
    eprintln!("hub-registry/whole_swimlane_compare: nfs {}", summary(&nfs));
    eprintln!(
        "hub-registry/whole_swimlane_compare: both {} (largest answer {biggest} bytes)",
        summary(&all)
    );
    let path = write_result("hub-registry/whole_swimlane_compare", "ms", &all);
    eprintln!("hub-registry/whole_swimlane_compare: {}", path.display());
}
