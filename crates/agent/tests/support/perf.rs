//! Result files for `cargo xtask bench-check` (T4): the measurements that criterion does not make, written as
//! `target/perf-results/<bench>.json` with the statistics the budgets ask for.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use serde_json::json;

/// Nearest-rank percentile of an ascending slice, the same rule `bench-check` applies to its own samples.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    assert!(!sorted.is_empty(), "no samples");
    #[allow(clippy::cast_precision_loss)]
    let n = sorted.len() as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rank = ((p * n) - 1e-9).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

/// Where results go: `$CARGO_TARGET_DIR/perf-results`, or the workspace's `target/perf-results`.
pub fn results_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    target.join("perf-results")
}

/// Write the result of a measurement. `bench` is the name the budget uses (`agent/oob_change_visible`), `unit` must
/// match the budget's unit, `samples` are the raw measurements.
pub fn write_result(bench: &str, unit: &str, samples: &[f64]) -> PathBuf {
    write_result_in(&results_dir(), bench, unit, samples)
}

/// [`write_result`] into a chosen directory.
pub fn write_result_in(dir: &std::path::Path, bench: &str, unit: &str, samples: &[f64]) -> PathBuf {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    #[allow(clippy::cast_precision_loss)]
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let body = json!({
        "bench": bench,
        "unit": unit,
        "values": {
            "min": percentile(&sorted, 0.0),
            "p50": percentile(&sorted, 0.50),
            "p95": percentile(&sorted, 0.95),
            "p99": percentile(&sorted, 0.99),
            "max": percentile(&sorted, 1.0),
            "mean": mean,
        },
        "samples": samples,
    });
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{}.json", bench.replace('/', "_")));
    std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    path
}
