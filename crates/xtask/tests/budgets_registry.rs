//! The budget registry `perf/budgets.toml` (prompt 01, T8; P1-P15, plan Q5).
//!
//! The registry is a contract: it must name a measurement for every budget in `docs/performance.md`, and its
//! thresholds must be the documented ones. A loosened or missing budget makes `bench-check` meaningless, so these
//! tests read the real file and the real document.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use xtask::budgets::{Direction, Registry, RegistryError, Statistic};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn registry() -> Registry {
    Registry::load(&repo_root().join("perf/budgets.toml")).unwrap()
}

fn find<'a>(reg: &'a Registry, id: &str) -> &'a xtask::budgets::Budget {
    reg.budgets
        .iter()
        .find(|b| b.id == id)
        .unwrap_or_else(|| panic!("no budget with id {id}"))
}

/// `P7.grid` belongs to `P7`; `P5` belongs to `P5`.
fn p_number(id: &str) -> u32 {
    let head = id.split('.').next().unwrap();
    head.strip_prefix('P').unwrap().parse().unwrap()
}

#[test]
fn every_p_number_has_an_entry() {
    let reg = registry();
    let present: BTreeSet<u32> = reg.budgets.iter().map(|b| p_number(&b.id)).collect();
    let missing: Vec<u32> = (1..=15).filter(|n| !present.contains(n)).collect();
    assert!(missing.is_empty(), "no registry entry for P{missing:?}");

    // The multi-part budgets get one entry per part (plan Q5, Appendix C).
    for id in [
        "P4",
        "P4.stat_walk",
        "P4.cpu_mcores",
        "P4.memory_mib",
        "P5",
        "P5.per_file_per_swimlane",
        "P7.swimlane_list",
        "P7.file_tree",
        "P7.file_content_cached",
        "P7.file_content_cold",
        "P7.compare",
        "P7.grid",
        "P7.whole_swimlane_compare",
        "P7.settings_search",
        "P7.text_search",
        "P7.docs_search",
        "P7.feature_status",
        "P10.initial_js_gzip_kb",
        "P10.route_change_ms",
        "P10.diff_6000_lines_ms",
        "P10.grid_5000_rows_fps",
        "P10.lighthouse_perf",
        "P11",
        "P11.memory_mib",
        "P12.steady_state_query_ms",
        "P15.spool_replay_versions_per_s",
    ] {
        find(&reg, id);
    }
}

#[test]
fn every_entry_has_bench_statistic_threshold_unit_scale() {
    for b in &registry().budgets {
        assert!(!b.description.trim().is_empty(), "{}: empty description", b.id);
        assert!(!b.bench.trim().is_empty(), "{}: empty bench", b.id);
        assert!(!b.unit.trim().is_empty(), "{}: empty unit", b.id);
        assert!(
            b.threshold.is_finite() && b.threshold > 0.0,
            "{}: bad threshold",
            b.id
        );
        assert!(
            matches!(b.scale.as_str(), "small" | "target"),
            "{}: bad scale {}",
            b.id,
            b.scale
        );
        // `bench` is `<group>/<function>`, a relative path with no traversal (it is joined onto a directory).
        assert!(
            !b.bench.starts_with('/') && !b.bench.contains(".."),
            "{}: unsafe bench {}",
            b.id,
            b.bench
        );
        assert!(
            b.bench.contains('/'),
            "{}: bench must be <group>/<function>",
            b.id
        );
    }
}

#[test]
fn ids_unique() {
    let reg = registry();
    let ids: BTreeSet<&str> = reg.budgets.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids.len(), reg.budgets.len(), "duplicate budget ids");
    let benches: BTreeSet<&str> = reg.budgets.iter().map(|b| b.bench.as_str()).collect();
    assert_eq!(
        benches.len(),
        reg.budgets.len(),
        "two budgets name the same bench"
    );
}

#[test]
fn lower_bound_budgets_are_marked() {
    let reg = registry();
    for id in [
        "P10.grid_5000_rows_fps",
        "P10.lighthouse_perf",
        "P15.spool_replay_versions_per_s",
    ] {
        assert_eq!(find(&reg, id).direction, Direction::Lower, "{id}");
    }
    for b in &reg.budgets {
        if b.direction == Direction::Lower {
            assert!(
                matches!(b.statistic, Statistic::Min | Statistic::P50),
                "{}: a lower bound is checked against the worst case (min) or the median, not a high percentile",
                b.id
            );
        }
    }
    let upper = reg
        .budgets
        .iter()
        .filter(|b| b.direction == Direction::Upper)
        .count();
    assert_eq!(upper, reg.budgets.len() - 3);
}

/// The thresholds of `docs/performance.md`, entry by entry. The needle must appear in the document, so a changed
/// document makes this test fail until the registry is reconsidered (and the other way round).
#[test]
fn thresholds_match_docs_performance_md() {
    let doc = std::fs::read_to_string(repo_root().join("docs/performance.md")).unwrap();
    let reg = registry();
    // (id, threshold, unit, text that states the budget in docs/performance.md)
    let expected: &[(&str, f64, &str, &str)] = &[
        ("P1", 15_000.0, "ms", "appears in the UI within 15 seconds"),
        ("P2", 10_000.0, "ms", "within 10 seconds through the webhook"),
        ("P3", 1_024.0, "bytes_per_min", "under 1 KB per minute"),
        (
            "P4",
            20_000.0,
            "ms",
            "full rehash of 2,000 files takes under 20 seconds",
        ),
        (
            "P4.stat_walk",
            1_000.0,
            "ms",
            "A stat walk of 2,000 files takes under 1 second",
        ),
        ("P4.cpu_mcores", 50.0, "mcores", "at most 50 mCPU"),
        ("P4.memory_mib", 64.0, "mib", "64 MiB of memory"),
        ("P5", 50.0, "ms", "within 50 ms + 2 ms"),
        (
            "P5.per_file_per_swimlane",
            2.0,
            "ms",
            "50 ms + 2 ms × N × (number of affected swimlanes)",
        ),
        ("P6", 180_000.0, "ms", "finishes within 3 minutes"),
        ("P7.swimlane_list", 50.0, "ms", "| Swimlane list | 50 ms |"),
        ("P7.file_tree", 100.0, "ms", "| File tree | 100 ms |"),
        (
            "P7.file_content_cached",
            30.0,
            "ms",
            "| File content | 30 ms cached, 80 ms cold |",
        ),
        (
            "P7.file_content_cold",
            80.0,
            "ms",
            "| File content | 30 ms cached, 80 ms cold |",
        ),
        (
            "P7.compare",
            100.0,
            "ms",
            "| Compare two versions of a 6,000-line file | 100 ms |",
        ),
        (
            "P7.grid",
            150.0,
            "ms",
            "| Grid for one logical file across 40 swimlanes | 150 ms |",
        ),
        (
            "P7.whole_swimlane_compare",
            300.0,
            "ms",
            "| Whole-swimlane compare | 300 ms |",
        ),
        (
            "P7.settings_search",
            200.0,
            "ms",
            "| Settings search across all swimlanes | 200 ms |",
        ),
        (
            "P7.text_search",
            400.0,
            "ms",
            "| Text search across all swimlanes | 400 ms |",
        ),
        ("P7.docs_search", 150.0, "ms", "| Docs search | 150 ms |"),
        ("P7.feature_status", 200.0, "ms", "| Feature status | 200 ms |"),
        ("P8", 1_500.0, "ms", "acknowledged within 1.5 seconds"),
        ("P9", 300.0, "ms", "respond within 300 ms"),
        ("P10.initial_js_gzip_kb", 250.0, "kb", "under 250 KB gzipped"),
        (
            "P10.route_change_ms",
            100.0,
            "ms",
            "Route changes complete within 100 ms",
        ),
        ("P10.diff_6000_lines_ms", 500.0, "ms", "interactive within 500 ms"),
        (
            "P10.grid_5000_rows_fps",
            60.0,
            "fps",
            "scrolls at 60 frames per second",
        ),
        ("P10.lighthouse_perf", 90.0, "score", "score is at least 90"),
        ("P11", 10_000.0, "ms", "Startup takes under 10 seconds"),
        (
            "P11.memory_mib",
            1_536.0,
            "mib",
            "memory stays under 1.5 GiB per replica",
        ),
        (
            "P12.steady_state_query_ms",
            50.0,
            "ms",
            "No steady-state query takes longer than 50 ms",
        ),
        ("P13", 1_000.0, "ms", "within 1 second"),
        (
            "P14",
            30_000.0,
            "ms",
            "within 30 seconds of the change being observed",
        ),
        (
            "P15.spool_replay_versions_per_s",
            1_000.0,
            "versions_per_s",
            "replays at 1,000 or more versions per second",
        ),
    ];
    for (id, threshold, unit, needle) in expected {
        let b = find(&reg, id);
        assert!(
            (b.threshold - threshold).abs() < f64::EPSILON,
            "{id}: threshold {} != {threshold}",
            b.threshold
        );
        assert_eq!(b.unit, *unit, "{id}: unit");
        assert!(
            doc.contains(needle),
            "{id}: docs/performance.md no longer says {needle:?}"
        );
    }
    // Every registry entry is covered above, so a new entry cannot skip the comparison with the document.
    let covered: BTreeSet<&str> = expected.iter().map(|e| e.0).collect();
    for b in &reg.budgets {
        assert!(
            covered.contains(b.id.as_str()),
            "{}: not compared with docs/performance.md",
            b.id
        );
    }
}

/// A budget is registered only together with the benchmark or harness that produces its result (plan Q5), and flipping
/// it is the owning prompt's contract change. So the list is written out here: a PR that registers a budget changes it,
/// in view of the reviewer. Everything else stays unregistered (reported as UNMET).
#[test]
fn only_budgets_with_a_harness_are_registered() {
    // Prompt 02 (agent): `cargo bench -p agent` produces P1, P3, P4, P4.stat_walk, P4.cpu_mcores, P4.memory_mib and
    // P15.spool_replay_versions_per_s (the spool, T9).
    const REGISTERED: &[&str] = &[
        "P1",
        "P3",
        "P4",
        "P4.stat_walk",
        "P4.cpu_mcores",
        "P4.memory_mib",
        "P15.spool_replay_versions_per_s",
    ];
    let reg = registry();
    let registered: BTreeSet<&str> = reg
        .budgets
        .iter()
        .filter(|b| b.registered)
        .map(|b| b.id.as_str())
        .collect();
    let expected: BTreeSet<&str> = REGISTERED.iter().copied().collect();
    assert_eq!(
        registered, expected,
        "registered = true needs a benchmark that produces the result, and this list updated in the same PR"
    );
}

#[test]
fn parse_rejects_bad_registries() {
    let ok = r#"
[[budget]]
id = "P1"
description = "d"
bench = "a/b"
statistic = "p95"
threshold = 1
unit = "ms"
scale = "target"
registered = false
"#;
    assert!(Registry::parse(ok).is_ok());

    let dup = format!("{ok}{ok}");
    assert!(matches!(
        Registry::parse(&dup),
        Err(RegistryError::DuplicateId(_))
    ));

    for bad in [
        ok.replace("p95", "p96"),
        ok.replace("threshold = 1", "threshold = 0"),
        ok.replace("threshold = 1", "threshold = -3"),
        ok.replace("\"a/b\"", "\"../etc/passwd\""),
        ok.replace("\"a/b\"", "\"/abs/path\""),
        ok.replace("\"a/b\"", "\"nogroup\""),
        ok.replace("registered = false", "registered = false\nsurprise = 1"),
        ok.replace("scale = \"target\"", "scale = \"huge\""),
        ok.replace("id = \"P1\"", "id = \"Q1\""),
    ] {
        assert!(Registry::parse(&bad).is_err(), "accepted: {bad}");
    }
}
