//! The result files the harness benchmarks write (T4): the shape `cargo xtask bench-check` reads.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use support::perf::{percentile, write_result_in};

#[test]
fn nearest_rank_percentiles() {
    let v: Vec<f64> = (1..=20).map(f64::from).collect();
    assert!((percentile(&v, 0.95) - 19.0).abs() < f64::EPSILON);
    assert!((percentile(&v, 1.0) - 20.0).abs() < f64::EPSILON);
    assert!((percentile(&v, 0.0) - 1.0).abs() < f64::EPSILON);
    assert!((percentile(&v, 0.50) - 10.0).abs() < f64::EPSILON);
    // Twelve samples: the 95th percentile is the largest, as bench-check computes it.
    let twelve: Vec<f64> = (1..=12).map(f64::from).collect();
    assert!((percentile(&twelve, 0.95) - 12.0).abs() < f64::EPSILON);
}

#[test]
fn a_result_file_names_the_bench_the_unit_and_every_statistic_a_budget_may_ask_for() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = write_result_in(
        dir.path(),
        "agent/oob_change_visible",
        "ms",
        &[3.0, 1.0, 2.0, 100.0],
    );
    assert_eq!(path.file_name().unwrap(), "agent_oob_change_visible.json");
    let body: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(body["bench"], "agent/oob_change_visible");
    assert_eq!(body["unit"], "ms");
    for (statistic, expected) in [
        ("min", 1.0),
        ("p50", 2.0),
        ("p95", 100.0),
        ("p99", 100.0),
        ("max", 100.0),
    ] {
        assert!(
            (body["values"][statistic].as_f64().unwrap() - expected).abs() < 1e-9,
            "{statistic}"
        );
    }
    assert!((body["values"]["mean"].as_f64().unwrap() - 26.5).abs() < 1e-9);
    assert_eq!(body["samples"].as_array().unwrap().len(), 4);
}
