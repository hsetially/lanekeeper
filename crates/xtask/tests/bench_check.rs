//! `cargo xtask bench-check` (prompt 01, T8; AC5, plan Q5).
//!
//! The planted values below are the proof that the gate can fail: a gate that has never been seen failing is not a gate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::TempDir;
use xtask::bench_check::{self, Options, Verdict, percentile};

const MS: f64 = 1_000_000.0;

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new(budgets: &str) -> Self {
        let dir = TempDir::new("bench-check");
        std::fs::write(dir.path().join("budgets.toml"), budgets).unwrap();
        std::fs::create_dir_all(dir.path().join("criterion")).unwrap();
        std::fs::create_dir_all(dir.path().join("perf-results")).unwrap();
        Self { dir }
    }

    fn options(&self, strict: bool) -> Options {
        Options {
            budgets: self.dir.path().join("budgets.toml"),
            criterion_dir: self.dir.path().join("criterion"),
            results_dir: self.dir.path().join("perf-results"),
            strict,
        }
    }

    /// Writes a criterion `sample.json` where sample `i` ran `iters[i]` iterations in `times_ns[i]` nanoseconds.
    fn criterion(&self, bench: &str, iters: &[f64], times_ns: &[f64]) {
        let dir = self.dir.path().join("criterion").join(bench).join("new");
        std::fs::create_dir_all(&dir).unwrap();
        let body = format!(
            r#"{{"sampling_mode":"Linear","iters":{},"times":{}}}"#,
            serde_json::to_string(iters).unwrap(),
            serde_json::to_string(times_ns).unwrap()
        );
        std::fs::write(dir.join("sample.json"), body).unwrap();
    }

    /// 100 samples of one iteration each, taking 1 ms, 2 ms, ... 100 ms. Its p95 is 95 ms.
    fn criterion_ramp(&self, bench: &str) {
        let iters = vec![1.0; 100];
        let times: Vec<f64> = (1..=100).map(|i| f64::from(i) * MS).collect();
        self.criterion(bench, &iters, &times);
    }

    fn result_file(&self, name: &str, body: &str) {
        std::fs::write(self.dir.path().join("perf-results").join(name), body).unwrap();
    }

    fn cli(&self, extra: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("bench-check")
            .arg("--budgets")
            .arg(self.dir.path().join("budgets.toml"))
            .arg("--criterion-dir")
            .arg(self.dir.path().join("criterion"))
            .arg("--results-dir")
            .arg(self.dir.path().join("perf-results"))
            .args(extra)
            .output()
            .unwrap()
    }
}

fn budget(
    id: &str,
    bench: &str,
    statistic: &str,
    threshold: f64,
    unit: &str,
    direction: &str,
    registered: bool,
) -> String {
    format!(
        "[[budget]]\nid = \"{id}\"\ndescription = \"test\"\nbench = \"{bench}\"\nstatistic = \"{statistic}\"\n\
         threshold = {threshold}\nunit = \"{unit}\"\nscale = \"target\"\ndirection = \"{direction}\"\nregistered = {registered}\n\n"
    )
}

fn only(report: &bench_check::Report, id: &str) -> Verdict {
    report
        .outcomes
        .iter()
        .find(|o| o.id == id)
        .unwrap()
        .verdict
        .clone()
}

#[test]
fn fails_on_planted_over_budget_value() {
    // p95 of the ramp is 95 ms, the budget is 90 ms.
    let f = Fixture::new(&budget(
        "P7.grid",
        "hub-registry/grid",
        "p95",
        90.0,
        "ms",
        "upper",
        true,
    ));
    f.criterion_ramp("hub-registry/grid");

    let report = bench_check::run(&f.options(false)).unwrap();
    assert!(report.failed());
    assert!(
        matches!(only(&report, "P7.grid"), Verdict::OverBudget { .. }),
        "{:?}",
        only(&report, "P7.grid")
    );
    assert!(report.render().contains("FAIL"));

    // The binary carries it to the exit code, which is what verify-01 and CI look at.
    let out = f.cli(&[]);
    assert!(
        !out.status.success(),
        "bench-check exited 0 on an over-budget value"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("P7.grid"));
}

#[test]
fn passes_within_budget() {
    let f = Fixture::new(&budget(
        "P7.grid",
        "hub-registry/grid",
        "p95",
        96.0,
        "ms",
        "upper",
        true,
    ));
    f.criterion_ramp("hub-registry/grid");
    let report = bench_check::run(&f.options(false)).unwrap();
    assert!(!report.failed());
    assert!(matches!(only(&report, "P7.grid"), Verdict::Pass { .. }));
    assert!(f.cli(&[]).status.success());
    assert!(f.cli(&["--strict"]).status.success());
}

#[test]
fn boundary_value_equal_to_threshold_passes() {
    let f = Fixture::new(&budget(
        "P7.grid",
        "hub-registry/grid",
        "p95",
        95.0,
        "ms",
        "upper",
        true,
    ));
    f.criterion_ramp("hub-registry/grid");
    assert!(!bench_check::run(&f.options(false)).unwrap().failed());
}

#[test]
fn fails_on_missing_result_when_strict() {
    // Not registered, no result: an UNMET warning by default, a failure with --strict (plan Q5 c).
    let f = Fixture::new(&budget(
        "P9",
        "hub-mcp/read_tools",
        "p95",
        300.0,
        "ms",
        "upper",
        false,
    ));
    let lax = bench_check::run(&f.options(false)).unwrap();
    assert!(!lax.failed());
    assert!(matches!(only(&lax, "P9"), Verdict::Unmet { .. }));
    assert!(lax.render().contains("UNMET"));
    assert!(f.cli(&[]).status.success());

    let strict = bench_check::run(&f.options(true)).unwrap();
    assert!(strict.failed());
    assert!(!f.cli(&["--strict"]).status.success());
}

#[test]
fn fails_on_registered_entry_without_result() {
    let f = Fixture::new(&budget(
        "P9",
        "hub-mcp/read_tools",
        "p95",
        300.0,
        "ms",
        "upper",
        true,
    ));
    let report = bench_check::run(&f.options(false)).unwrap();
    assert!(report.failed());
    assert!(matches!(only(&report, "P9"), Verdict::MissingResult));
    assert!(!f.cli(&[]).status.success());
}

#[test]
fn unregistered_entries_do_not_fail_the_default_gate_even_when_over_budget() {
    // The owning prompt has not registered it yet. The number is shown, the gate stays green, --strict fails.
    let f = Fixture::new(&budget(
        "P7.grid",
        "hub-registry/grid",
        "p95",
        10.0,
        "ms",
        "upper",
        false,
    ));
    f.criterion_ramp("hub-registry/grid");
    let report = bench_check::run(&f.options(false)).unwrap();
    assert!(!report.failed());
    assert!(report.render().contains("UNMET"));
    assert!(bench_check::run(&f.options(true)).unwrap().failed());
}

#[test]
fn respects_lower_bound_direction() {
    let budgets = budget("P10.grid_fps", "web/grid_fps", "min", 60.0, "fps", "lower", true);
    let f = Fixture::new(&budgets);

    f.result_file(
        "grid.json",
        r#"{"bench":"web/grid_fps","unit":"fps","value":61.5}"#,
    );
    let ok = bench_check::run(&f.options(false)).unwrap();
    assert!(!ok.failed(), "61.5 fps is above the 60 fps floor");
    assert!(matches!(only(&ok, "P10.grid_fps"), Verdict::Pass { .. }));

    f.result_file(
        "grid.json",
        r#"{"bench":"web/grid_fps","unit":"fps","value":59.9}"#,
    );
    let bad = bench_check::run(&f.options(false)).unwrap();
    assert!(bad.failed(), "59.9 fps is below the floor");
    assert!(matches!(only(&bad, "P10.grid_fps"), Verdict::UnderFloor { .. }));

    // A value far above the floor is not "over budget".
    f.result_file(
        "grid.json",
        r#"{"bench":"web/grid_fps","unit":"fps","value":10000}"#,
    );
    assert!(!bench_check::run(&f.options(false)).unwrap().failed());

    // An upper-bound budget treats the same 59.9 the other way round.
    let f2 = Fixture::new(&budget("P1", "web/x", "max", 60.0, "ms", "upper", true));
    f2.result_file("x.json", r#"{"bench":"web/x","unit":"ms","value":59.9}"#);
    assert!(!bench_check::run(&f2.options(false)).unwrap().failed());
    f2.result_file("x.json", r#"{"bench":"web/x","unit":"ms","value":60.1}"#);
    assert!(bench_check::run(&f2.options(false)).unwrap().failed());
}

#[test]
fn p95_is_computed_from_criterion_samples() {
    // Per-iteration time is times[i] / iters[i]. 1..=100 ms gives p95 = 95 ms by nearest rank.
    let f = Fixture::new(
        &(budget("A", "g/p50", "p50", 50.0, "ms", "upper", true)
            + &budget("B", "g/p95", "p95", 95.0, "ms", "upper", true)
            + &budget("C", "g/p99", "p99", 99.0, "ms", "upper", true)
            + &budget("D", "g/max", "max", 100.0, "ms", "upper", true)
            + &budget("E", "g/min", "min", 1.0, "ms", "lower", true)
            + &budget("F", "g/mean", "mean", 50.5, "ms", "upper", true))
            .replace("id = \"A\"", "id = \"P1\"")
            .replace("id = \"B\"", "id = \"P2\"")
            .replace("id = \"C\"", "id = \"P3\"")
            .replace("id = \"D\"", "id = \"P4\"")
            .replace("id = \"E\"", "id = \"P5\"")
            .replace("id = \"F\"", "id = \"P6\""),
    );
    for b in ["g/p50", "g/p95", "g/p99", "g/max", "g/min", "g/mean"] {
        f.criterion_ramp(b);
    }
    let report = bench_check::run(&f.options(false)).unwrap();
    let measured = |id: &str| {
        report
            .outcomes
            .iter()
            .find(|o| o.id == id)
            .unwrap()
            .measured
            .unwrap()
    };
    assert!((measured("P1") - 50.0).abs() < 1e-9, "p50 {}", measured("P1"));
    assert!((measured("P2") - 95.0).abs() < 1e-9, "p95 {}", measured("P2"));
    assert!((measured("P3") - 99.0).abs() < 1e-9, "p99 {}", measured("P3"));
    assert!((measured("P4") - 100.0).abs() < 1e-9, "max {}", measured("P4"));
    assert!((measured("P5") - 1.0).abs() < 1e-9, "min {}", measured("P5"));
    assert!((measured("P6") - 50.5).abs() < 1e-9, "mean {}", measured("P6"));
    assert!(!report.failed());
}

#[test]
fn criterion_time_is_divided_by_iterations() {
    // Linear sampling: sample i runs i iterations. 10 ms per iteration whatever the count.
    let iters: Vec<f64> = (1..=20).map(f64::from).collect();
    let times: Vec<f64> = iters.iter().map(|i| i * 10.0 * MS).collect();
    let f = Fixture::new(&budget("P1", "g/linear", "p95", 10.0, "ms", "upper", true));
    f.criterion("g/linear", &iters, &times);
    let report = bench_check::run(&f.options(false)).unwrap();
    assert!((report.outcomes[0].measured.unwrap() - 10.0).abs() < 1e-9);
}

#[test]
fn percentile_is_nearest_rank() {
    let v: Vec<f64> = (1..=20).map(f64::from).collect();
    assert!((percentile(&v, 0.95) - 19.0).abs() < f64::EPSILON); // ceil(0.95 * 20) = 19
    assert!((percentile(&v, 0.50) - 10.0).abs() < f64::EPSILON);
    assert!((percentile(&v, 1.0) - 20.0).abs() < f64::EPSILON);
    assert!((percentile(&v, 0.0) - 1.0).abs() < f64::EPSILON);
    assert!((percentile(&[7.0], 0.95) - 7.0).abs() < f64::EPSILON);
}

#[test]
fn unit_mismatch_in_a_result_file_fails_a_registered_entry() {
    let f = Fixture::new(&budget(
        "P4.memory_mib",
        "agent/mem",
        "max",
        64.0,
        "mib",
        "upper",
        true,
    ));
    f.result_file("mem.json", r#"{"bench":"agent/mem","unit":"bytes","value":1}"#);
    let report = bench_check::run(&f.options(false)).unwrap();
    assert!(
        report.failed(),
        "1 byte must not pass a 64 MiB budget by being read in the wrong unit"
    );
    assert!(matches!(
        only(&report, "P4.memory_mib"),
        Verdict::BadResult { .. }
    ));
}

#[test]
fn result_file_statistics_map_is_used_for_the_budget_statistic() {
    let f = Fixture::new(&budget(
        "P13",
        "hub-platform/sse",
        "p95",
        1000.0,
        "ms",
        "upper",
        true,
    ));
    f.result_file(
        "sse.json",
        r#"{"bench":"hub-platform/sse","unit":"ms","values":{"p50":100,"p95":1500,"p99":2000}}"#,
    );
    let report = bench_check::run(&f.options(false)).unwrap();
    assert!(report.failed());
    assert!((report.outcomes[0].measured.unwrap() - 1500.0).abs() < 1e-9);

    // The file names a statistic the budget does not ask for.
    f.result_file(
        "sse.json",
        r#"{"bench":"hub-platform/sse","unit":"ms","statistic":"p50","value":1}"#,
    );
    assert!(matches!(
        only(&bench_check::run(&f.options(false)).unwrap(), "P13"),
        Verdict::BadResult { .. }
    ));
}

#[test]
fn malformed_results_fail_closed() {
    let f = Fixture::new(&budget("P1", "g/x", "p95", 10.0, "ms", "upper", true));
    for (iters, times) in [
        (vec![], vec![]),            // no samples
        (vec![1.0, 1.0], vec![1.0]), // length mismatch
        (vec![0.0], vec![5.0]),      // zero iterations
        (vec![1.0], vec![-5.0]),     // negative time
    ] {
        f.criterion("g/x", &iters, &times);
        let r = bench_check::run(&f.options(false)).unwrap();
        assert!(r.failed(), "accepted iters={iters:?} times={times:?}");
        assert!(matches!(only(&r, "P1"), Verdict::BadResult { .. }));
    }
    // Not JSON at all.
    let dir = f.dir.path().join("criterion/g/x/new");
    std::fs::write(dir.join("sample.json"), "not json").unwrap();
    assert!(bench_check::run(&f.options(false)).unwrap().failed());

    // Two result files claiming the same bench are ambiguous.
    let f = Fixture::new(&budget("P1", "g/y", "p95", 10.0, "ms", "upper", true));
    f.result_file("a.json", r#"{"bench":"g/y","unit":"ms","value":1}"#);
    f.result_file("b.json", r#"{"bench":"g/y","unit":"ms","value":2}"#);
    assert!(bench_check::run(&f.options(false)).unwrap().failed());
}

#[test]
fn result_files_that_are_not_json_do_not_hide_a_missing_result() {
    let f = Fixture::new(&budget("P1", "g/z", "p95", 10.0, "ms", "upper", true));
    f.result_file("junk.json", "{{{");
    f.result_file("notes.txt", "ignored");
    assert!(bench_check::run(&f.options(false)).unwrap().failed());
}

#[test]
fn a_bad_registry_is_an_error_not_a_pass() {
    let f = Fixture::new("[[budget]]\nid = \"nope\"\n");
    assert!(bench_check::run(&f.options(false)).is_err());
    assert!(!f.cli(&[]).status.success());
}

fn planted_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/bench-planted")
}

/// The data behind `just bench-check-selftest`: a registered budget of 10 ms against samples of 50 ms.
#[test]
fn committed_planted_testdata_is_rejected_by_the_binary() {
    let dir = planted_dir();
    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["bench-check", "--budgets"])
        .arg(dir.join("budgets.toml"))
        .arg("--criterion-dir")
        .arg(dir.join("criterion"))
        .arg("--results-dir")
        .arg(dir.join("perf-results-absent"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("PLANTED"));
}

#[test]
fn cli_rejects_unknown_arguments() {
    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["bench-check", "--no-such-flag"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
