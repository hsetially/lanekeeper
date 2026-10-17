//! P6, the nightly full recompute at design scale, as the hub runs it (`P6.hub_full_recompute`,
//! `hub-registry/full_recompute_target_scale`, a harness, p95): 40 swimlanes of about 2,000 files, 60 tenant branches, a real Postgres,
//! the in-memory Git loaded from the fixtures' repositories. [`hub_registry::Pipeline::refresh_all`] reads every swimlane's rows,
//! baselines and marks, builds the engine's input against Git, reads the content of the distinct files in batches of four swimlanes,
//! calls `engine::recompute_all`, and writes the difference with its events: the database reads and writes are inside the number, which
//! the engine's own `P6` (`engine/full_recompute_target_scale`) does not have.
//!
//! Five samples: the first is the **cold** run (every swimlane is computed for the first time: all its rows, findings and effective
//! hashes are written), and four are the nightly runs after it (the swimlanes are in memory, the database is right, and the run
//! compares and repairs nothing). The budget takes the 95th percentile of the five, which is the slowest, the cold one. Each run is
//! checked: all 40 swimlanes were computed, none failed, and a nightly run repaired nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use hub_registry::RegistryConfig;

use common::Bed;
use common::support::perf::write_result;

const SAMPLES: usize = 5;

#[tokio::main]
async fn main() {
    let Some(bed) = Bed::new(40, RegistryConfig::default()).await else {
        eprintln!("hub-registry/full_recompute_target_scale: SKIPPED (no database)");
        return;
    };
    let mut samples = Vec::new();
    for run in 0..SAMPLES {
        let report = bed.pipeline.refresh_all().await.unwrap();
        assert_eq!(report.lanes, 40, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        if run > 0 {
            assert_eq!(
                report.repaired_paths, 0,
                "a nightly run found the stored state wrong: {report:?}"
            );
            assert_eq!(report.rows_changed, 0, "{report:?}");
        } else {
            assert!(
                report.rows_changed > 60_000,
                "the cold run writes every row: {report:?}"
            );
        }
        let ms = report.took.as_secs_f64() * 1000.0;
        eprintln!(
            "hub-registry/full_recompute_target_scale: run {} ({}): {:.1} s, {} paths, {} rows changed, {} repaired",
            run + 1,
            if run == 0 { "cold" } else { "nightly" },
            ms / 1000.0,
            report.paths,
            report.rows_changed,
            report.repaired_paths
        );
        samples.push(ms);
    }
    let path = write_result("hub-registry/full_recompute_target_scale", "ms", &samples);
    eprintln!("hub-registry/full_recompute_target_scale: {}", path.display());
}
