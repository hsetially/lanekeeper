//! P7, the swimlane list through the API at design scale (`P7.swimlane_list`, `hub-registry/swimlane_list`, a harness, p95, 50 ms):
//! 40 swimlanes with their tenants, drift counts, finding counts and agent state, from the read bed (a real Postgres, every swimlane
//! computed), as the router answers it: the guard, the handler, the query, the JSON.
//!
//! The samples are `GET /api/v1/swimlanes` (the whole list, which is one page of 40) and, alternately, the same list in pages of 10
//! followed through their cursors (four requests, each a sample), so that the keyset paging is in the number as well.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const ROUNDS: usize = 60;

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/swimlane_list: SKIPPED (no database)");
        return;
    };
    // The first answer: 40 swimlanes, every one with a scan and drift counts.
    let list = bed.json("/api/v1/swimlanes?limit=100").await;
    assert_eq!(list["items"].as_array().unwrap().len(), 40);
    assert!(list["next"].is_null());
    assert!(
        list["items"][0]["drift_counts"]
            .as_array()
            .is_some_and(|c| !c.is_empty()),
        "{}",
        list["items"][0]
    );
    // One warm-up of each shape.
    bed.ok("/api/v1/swimlanes?limit=100").await;

    let mut samples = Vec::new();
    let mut bytes = 0;
    for _ in 0..ROUNDS {
        let whole = bed.ok("/api/v1/swimlanes?limit=100").await;
        bytes = whole.bytes;
        samples.push(whole.ms);
        let mut cursor: Option<String> = None;
        for _ in 0..4 {
            let uri = match &cursor {
                None => "/api/v1/swimlanes?limit=10".to_owned(),
                Some(c) => format!("/api/v1/swimlanes?limit=10&cursor={c}"),
            };
            let (page, ms) = {
                let started = std::time::Instant::now();
                let json = bed.json(&uri).await;
                (json, started.elapsed().as_secs_f64() * 1000.0)
            };
            samples.push(ms);
            cursor = page["next"].as_str().map(str::to_owned);
        }
        assert!(cursor.is_none(), "four pages of ten end the list");
    }
    eprintln!(
        "hub-registry/swimlane_list: {} ({bytes} bytes for the whole list)",
        summary(&samples)
    );
    let path = write_result("hub-registry/swimlane_list", "ms", &samples);
    eprintln!("hub-registry/swimlane_list: {}", path.display());
}
