//! P7, the grid of one logical file across 40 swimlanes through the API, database included (`P7.grid`,
//! `hub-registry/grid_40_swimlanes`, a harness, p95, 150 ms; the engine's own share, on flattened inputs, is `P7.engine_grid`).
//!
//! The file is the 6,000-line `reference-data/catalog-big-01.yml` that every swimlane has (as its base copy or its tenant copy). The
//! two views the UI asks for are both measured, at the largest page a request can be (500 rows of 40 cells): the rows that differ, and
//! every row.
//!
//! A sample is **cold**: the hub has just started (the service's caches are empty), so the setting rows of the 40 files are read
//! from `settings_index`, merged into effective configs and the grid is made and paged (the guard, the queries, the engine's grid, the
//! JSON). The budget takes those. The same request asked again (the effective configs cached by their hashes) is logged, not budgeted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const FILE: &str = "reference-data%2Fcatalog-big-01.yml";
const COLD_RUNS: usize = 12;

#[tokio::main]
async fn main() {
    let Some(mut bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/grid_40_swimlanes: SKIPPED (no database)");
        return;
    };
    let lanes: Vec<String> = bed.bed.lanes.iter().map(|l| l.id.to_string()).collect();
    assert_eq!(lanes.len(), 40);
    let list = lanes.join(",");
    let views = [("differing", true), ("every row", false)];
    let uri = |only_differing: bool| {
        format!("/api/v1/grid?file={FILE}&swimlanes={list}&only_differing={only_differing}&limit=500")
    };
    // The answers are real grids: 40 columns of 40 cells; every row fills a page of 500 and has more behind it, and a few of the rows
    // differ (the fixtures' swimlanes differ in a handful of settings of this file, as the engine's own benchmark checks).
    let mut rows = Vec::new();
    for (what, only_differing) in views {
        let g = bed.json(&uri(only_differing)).await;
        assert_eq!(g["swimlanes"].as_array().unwrap().len(), 40, "{what}");
        let n = g["rows"]["items"].as_array().unwrap().len();
        assert_eq!(
            g["rows"]["items"][0]["cells"].as_array().unwrap().len(),
            40,
            "{what}"
        );
        rows.push((what, n, g["rows"]["next"].is_string()));
    }
    assert!(
        rows[0].1 > 0 && rows[0].1 < 500 && !rows[0].2,
        "only the differing rows: {rows:?}"
    );
    assert!(
        rows[1].1 == 500 && rows[1].2,
        "every row: a full page and more behind it: {rows:?}"
    );
    eprintln!("hub-registry/grid_40_swimlanes: first pages: {rows:?}");

    let mut cold = Vec::new();
    let mut warm = Vec::new();
    let mut bytes = 0;
    for _ in 0..COLD_RUNS {
        for (_, only_differing) in views {
            bed.restart();
            let r = bed.ok(&uri(only_differing)).await;
            bytes = bytes.max(r.bytes);
            cold.push(r.ms);
            // And again, now that the service has the effective configs.
            warm.push(bed.ok(&uri(only_differing)).await.ms);
        }
    }
    eprintln!(
        "hub-registry/grid_40_swimlanes: cold {} (largest answer {bytes} bytes)",
        summary(&cold)
    );
    eprintln!("hub-registry/grid_40_swimlanes: warm {}", summary(&warm));
    let path = write_result("hub-registry/grid_40_swimlanes", "ms", &cold);
    eprintln!("hub-registry/grid_40_swimlanes: {}", path.display());
}
