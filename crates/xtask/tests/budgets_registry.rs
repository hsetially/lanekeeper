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
        "P7.blob_get_cached",
        "P7.blob_endpoint",
        "P7.auth_middleware",
        "P7.auth_middleware_cold",
        "P10.initial_js_gzip_kb",
        "P10.route_change_ms",
        "P10.diff_6000_lines_ms",
        "P10.grid_5000_rows_fps",
        "P10.lighthouse_perf",
        "P11",
        "P11.memory_mib",
        "P12.steady_state_query_ms",
        "P12.audit_record",
        "P12.hub_writes_queries",
        "P15.spool_replay_versions_per_s",
        "P15.hub_ack_versions_per_s",
        // Prompt 05 (registry): its shares of P1, P2, P6, P7 and P15, on `hub-registry/` benchmarks.
        "P1.hub_delta_to_drift",
        "P2.hub_head_moved_to_drift",
        "P6.hub_full_recompute",
        "P6.adoption_rank",
        "P7.compare_api",
        "P15.registry_ingest_versions_per_s",
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
        "P15.hub_ack_versions_per_s",
        "P15.registry_ingest_versions_per_s",
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
    assert_eq!(upper, reg.budgets.len() - 5);
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
        (
            "P7.auth_middleware",
            2.0,
            "ms",
            "Authentication adds at most 2 ms to a request (p95), with a warm session cache and with a cold one",
        ),
        (
            "P7.auth_middleware_cold",
            2.0,
            "ms",
            "Authentication adds at most 2 ms to a request (p95), with a warm session cache and with a cold one",
        ),
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
        (
            "P12.audit_record",
            4.0,
            "ms",
            "Recording an audit event adds at most 4 ms to a transaction (p95), with another replica contending for the chain lock",
        ),
        (
            "P12.hub_writes_queries",
            50.0,
            "ms",
            "The statements the write path runs on every request take at most 50 ms (p95)",
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
        // The hub's half of the same budget: the agent replays at that rate, so the hub must take it at that rate.
        (
            "P15.hub_ack_versions_per_s",
            1_000.0,
            "versions_per_s",
            "replays at 1,000 or more versions per second",
        ),
        // Prompt 07 (MCP server): each read tool has the figure of the whole budget, measured on this layer alone.
        ("P9.list_swimlanes", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_swimlane", 300.0, "ms", "respond within 300 ms"),
        ("P9.list_files", 300.0, "ms", "respond within 300 ms"),
        ("P9.read_file", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_effective_config", 300.0, "ms", "respond within 300 ms"),
        ("P9.compare", 300.0, "ms", "respond within 300 ms"),
        ("P9.compare_tree", 300.0, "ms", "respond within 300 ms"),
        ("P9.compare_grid", 300.0, "ms", "respond within 300 ms"),
        ("P9.search_settings", 300.0, "ms", "respond within 300 ms"),
        ("P9.search_text", 300.0, "ms", "respond within 300 ms"),
        ("P9.find_inconsistencies", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_drift", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_pending_restarts", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_history", 300.0, "ms", "respond within 300 ms"),
        ("P9.search_docs", 300.0, "ms", "respond within 300 ms"),
        ("P9.read_doc", 300.0, "ms", "respond within 300 ms"),
        ("P9.grep_docs", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_docs_for_file", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_feature_status", 300.0, "ms", "respond within 300 ms"),
        ("P9.get_served_config", 300.0, "ms", "respond within 300 ms"),
        // Prompt 05 (registry). The same figures as the whole budgets, measured through the database and the HTTP layer
        // (`hub-registry/` benchmarks; bench names are unique, so the engine and the platform keep their own ids).
        (
            "P6.hub_full_recompute",
            180_000.0,
            "ms",
            "finishes within 3 minutes",
        ),
        (
            "P7.compare_api",
            100.0,
            "ms",
            "| Compare two versions of a 6,000-line file | 100 ms |",
        ),
        // The registry's half of P15: the hub takes a replayed spool at the rate the agent sends it, with the audit
        // events of the out-of-band versions inside the number.
        (
            "P15.registry_ingest_versions_per_s",
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
    // Budgets that a prompt states and `docs/performance.md` does not (the table there names no blob request, and the prompt's 20 ms
    // for parse and flatten is the engine's share of P6). They are compared with the prompt's own text instead, so a changed prompt
    // makes this fail until the registry is reconsidered, as for the others.
    let from_prompt: &[(&str, f64, &str, &str, &str)] = &[
        (
            "P7.blob_get_cached",
            1.0,
            "ms",
            "prompts/03b-hub-platform.md",
            "cached `get` under 1 ms and the blob endpoint under 30 ms (P7)",
        ),
        (
            "P7.blob_endpoint",
            30.0,
            "ms",
            "prompts/03b-hub-platform.md",
            "cached `get` under 1 ms and the blob endpoint under 30 ms (P7)",
        ),
        (
            "P6.parse_flatten",
            20.0,
            "ms",
            "prompts/04-fact-engine.md",
            "a benchmark: parse and flatten of the 6,000-line file in 20 ms or less",
        ),
        (
            "P7.engine_grid",
            150.0,
            "ms",
            "prompts/04-fact-engine.md",
            "a benchmark: grid for one logical file across 40 swimlanes in 150 ms or less, using pre-flattened inputs",
        ),
        (
            "P7.engine_text_search",
            400.0,
            "ms",
            "prompts/04-fact-engine.md",
            "a benchmark: text search over the target-scale deduplicated blob set, running on rayon, in 400 ms or less",
        ),
        // The prompt gives P5 and P6 as the whole budgets, 50 ms + 2 ms and 3 minutes. The engine entries for P5 use the full
        // figures (the engine alone must fit them); the plan's table names them, and the plan is frozen once approved.
        (
            "P5.engine_base",
            50.0,
            "ms",
            "plans/04-fact-engine.md",
            "| `incremental_recompute_base` | `P5.engine_base` | 50 ms |",
        ),
        (
            "P5.engine_per_file_per_swimlane",
            2.0,
            "ms",
            "plans/04-fact-engine.md",
            "| `incremental_recompute_per_file_per_swimlane` | `P5.engine_per_file_per_swimlane` | 2 ms |",
        ),
        // The prompt gives no number for the settings index. The plan proposed 40 ms (A1); T3 measured 45.8 ms p95 and a human
        // set 60 ms (the blockers file of prompt 04), and on 2026-10-26 a human raised it to 80 ms, so the decision that records
        // it (D136) is the source for the number. This comparison keeps it from being changed without its source.
        (
            "P6.index_settings",
            80.0,
            "ms",
            "docs/decisions.md",
            "`P6.index_settings` to 80 ms",
        ),
        // The prompt's T5 figure for the adoption ranking of one swimlane.
        (
            "P6.adoption_rank",
            5_000.0,
            "ms",
            "prompts/05-registry-read-api.md",
            "ranking takes 5 seconds or less per swimlane",
        ),
    ];
    for (id, threshold, unit, file, needle) in from_prompt {
        let prompt = std::fs::read_to_string(repo_root().join(file)).unwrap();
        let b = find(&reg, id);
        assert!(
            (b.threshold - threshold).abs() < f64::EPSILON,
            "{id}: threshold {} != {threshold}",
            b.threshold
        );
        assert_eq!(b.unit, *unit, "{id}: unit");
        assert!(prompt.contains(needle), "{id}: {file} no longer says {needle:?}");
    }
    // Every registry entry is covered above, so a new entry cannot skip the comparison with its source.
    // The registry's shares of P1 and P2 are derived from the whole budget, below (`registry_shares_of_p1_and_p2_...`).
    let derived = ["P1.hub_delta_to_drift", "P2.hub_head_moved_to_drift"];
    let covered: BTreeSet<&str> = expected
        .iter()
        .map(|e| e.0)
        .chain(from_prompt.iter().map(|e| e.0))
        .chain(derived)
        .collect();
    for b in &reg.budgets {
        assert!(
            covered.contains(b.id.as_str()),
            "{}: not compared with docs/performance.md",
            b.id
        );
    }
}

/// The registry's shares of P1 and P2 (prompt 05, plan A42). `docs/performance.md` states the whole budgets only; the share of
/// the hub is what is left of one after the other components have taken theirs, so it is derived here from the sources of
/// those components, and a change to any of them makes this fail until the share is reconsidered.
#[test]
fn registry_shares_of_p1_and_p2_are_what_the_other_components_leave() {
    let reg = registry();
    let performance = std::fs::read_to_string(repo_root().join("docs/performance.md")).unwrap();
    let decisions = std::fs::read_to_string(repo_root().join("docs/decisions.md")).unwrap();
    let close = |a: f64, b: f64| (a - b).abs() < f64::EPSILON;

    // P1: 15 s from the change to the UI. The agent's stat walk takes up to 10 s to see it (docs/performance.md, mechanism 2)
    // and it reports after the tree has been quiet for 3 s (D75). What is left is the hub's: ingest the delta, recompute,
    // publish `DriftChanged` to the outbox.
    assert!(performance.contains("Stat-walks every 10 seconds"));
    assert!(decisions.contains("quiet for 3 seconds"));
    let (whole, share) = (find(&reg, "P1"), find(&reg, "P1.hub_delta_to_drift"));
    assert!(
        close(share.threshold, whole.threshold - 10_000.0 - 3_000.0),
        "P1 {} less the 10 s walk and the 3 s quiet period is the hub's share, not {}",
        whole.threshold,
        share.threshold
    );
    assert_eq!((share.unit.as_str(), share.statistic), ("ms", whole.statistic));

    // P2: 10 s from the push to the drift, through the webhook. Prompt 03b spends the webhook, the fetch and the tree index;
    // the registry gets 1 s from `GitHeadMoved` to the drift in the outbox (a figure a human approved with the plan of
    // prompt 05, because no document divides P2).
    let (whole, share) = (find(&reg, "P2"), find(&reg, "P2.hub_head_moved_to_drift"));
    assert!(close(share.threshold, 1_000.0), "{}", share.threshold);
    assert!(share.threshold < whole.threshold);
    assert_eq!((share.unit.as_str(), share.statistic), ("ms", whole.statistic));
}

/// A budget is registered only together with the benchmark or harness that produces its result (plan Q5), and flipping
/// it is the owning prompt's contract change. So the list is written out here: a PR that registers a budget changes it,
/// in view of the reviewer. Everything else stays unregistered (reported as UNMET).
#[test]
fn only_budgets_with_a_harness_are_registered() {
    // Prompt 02 (agent): `cargo bench -p agent` produces P1, P3, P4, P4.stat_walk, P4.cpu_mcores, P4.memory_mib and
    // P15.spool_replay_versions_per_s (the spool, T9).
    // Prompt 03b (hub platform): `cargo bench -p hub-platform` produces P13 (T5, `sse_delivery`), P2 (T7, `push_to_drift_webhook`:
    // the webhook to `GitHeadMoved` part of it, which is the platform's share, plan A3), P7.blob_get_cached and P7.blob_endpoint
    // (T8, `blob_get_cached` and `blob_endpoint`, criterion), and the two hub harnesses of T8: P11 (`cargo bench -p hub --bench
    // startup`) and P11.memory_mib (`cargo bench -p hub-platform --bench memory_at_design_scale`), and P15.hub_ack_versions_per_s
    // (T11, `cargo bench -p hub-platform --bench ack_throughput`: the hub acknowledging a replayed spool).
    // Prompt 03a (identity): `cargo bench -p hub-identity` produces P7.auth_middleware and P7.auth_middleware_cold (T3) and
    // P12.audit_record (T5).
    // Prompt 07 (MCP server): `cargo bench -p hub-mcp` produces P9 (the rotation over the read tools, `hub-mcp/read_tools`) and
    // P9.<tool> for the 20 read tools (T2), on fakes at target scale: this layer's share of a call (plan A7).
    // Prompt 04 (fact engine): `cargo bench -p engine` produces P6.parse_flatten (T2), P7.compare and P7.engine_grid (T5),
    // P7.engine_text_search (T7), P6.index_settings (T3; threshold set by a human, plans/04-fact-engine.blockers.md), and P6,
    // P5.engine_base and P5.engine_per_file_per_swimlane (T8; the last is a harness that writes `target/perf-results/`).
    // Prompt 05 (registry): `cargo bench -p hub-registry` produces P5 (`incremental_recompute_base`, criterion), P5.per_file_per_swimlane,
    // P6.hub_full_recompute, P1.hub_delta_to_drift and P2.hub_head_moved_to_drift (T3; harnesses that write `target/perf-results/`),
    // P15.registry_ingest_versions_per_s (T1, `ingest_versions_per_s`: the registry applying a replayed spool over the real audit
    // log and event bus; registered once the batch calls of the ports brought it from 587 to over 1,000, see
    // plans/05-registry-read-api.blockers.md), and P6.adoption_rank (T5, `adoption_rank_per_swimlane`: `AdoptionService::start` over
    // each of the 40 swimlanes of the target scale, 120 samples; p95 48.8 ms on a quiet machine against the 5,000 ms of the prompt),
    // and the read API of T6, every one through the router over the 40 swimlanes of the target scale: P7.swimlane_list, P7.file_tree,
    // P7.file_content_cached, P7.file_content_cold, P7.compare_api, P7.grid (cold: the service has nothing cached), P7.whole_swimlane_compare,
    // P7.settings_search and P12.steady_state_query_ms (every read statement, p95 under 50 ms), and P7.text_search, whose registered case is
    // the steady state: the first search after an applied change to one swimlane on a warmed hub, with the other states (cold, rebuilt-all,
    // warm, the warm-up settled) logged by the same bench (D97, approved by the human on 2026-10-15).
    // And P14 (T7, `sentinel_attribution`: a sentinel's record attributes a change within 30 s, with the leased job running at its 5 s
    // interval, at the scale of one target swimlane with 40,000 older observations and 300,000 records; thirty samples, half with the record
    // first (they wait for the job's next run) and half with the scan first (the sink attributes in its own transaction), the pauses between
    // them drawn so that they fall on every phase of the job's clock: p95 4.7 s and at most 5.0 s on a quiet machine, against 30 s).
    // Prompt 06 (write paths): `cargo bench -p hub-writes` produces P8 (T1, `edit_applied`: the whole `edit` call through a fake
    // agent, over a real Postgres) and P12.hub_writes_queries (T1, `write_queries`: the statements of the write path at target scale).
    const REGISTERED: &[&str] = &[
        "P8",
        "P12.hub_writes_queries",
        "P6.parse_flatten",
        "P6.index_settings",
        "P7.compare",
        "P7.engine_grid",
        "P7.engine_text_search",
        "P6",
        "P5.engine_base",
        "P5.engine_per_file_per_swimlane",
        "P7.auth_middleware",
        "P7.auth_middleware_cold",
        "P12.audit_record",
        "P1",
        "P2",
        "P3",
        "P4",
        "P4.stat_walk",
        "P4.cpu_mcores",
        "P4.memory_mib",
        "P7.blob_get_cached",
        "P7.blob_endpoint",
        "P11",
        "P11.memory_mib",
        "P13",
        "P15.spool_replay_versions_per_s",
        "P15.hub_ack_versions_per_s",
        "P5",
        "P5.per_file_per_swimlane",
        "P6.hub_full_recompute",
        "P1.hub_delta_to_drift",
        "P2.hub_head_moved_to_drift",
        "P15.registry_ingest_versions_per_s",
        "P6.adoption_rank",
        "P7.swimlane_list",
        "P7.file_tree",
        "P7.file_content_cached",
        "P7.file_content_cold",
        "P7.compare_api",
        "P7.grid",
        "P7.whole_swimlane_compare",
        "P7.settings_search",
        "P7.text_search",
        "P12.steady_state_query_ms",
        "P14",
        "P9",
        "P9.list_swimlanes",
        "P9.get_swimlane",
        "P9.list_files",
        "P9.read_file",
        "P9.get_effective_config",
        "P9.compare",
        "P9.compare_tree",
        "P9.compare_grid",
        "P9.search_settings",
        "P9.search_text",
        "P9.find_inconsistencies",
        "P9.get_drift",
        "P9.get_pending_restarts",
        "P9.get_history",
        "P9.search_docs",
        "P9.read_doc",
        "P9.grep_docs",
        "P9.get_docs_for_file",
        "P9.get_feature_status",
        "P9.get_served_config",
        "P10.initial_js_gzip_kb",
        "P10.route_change_ms",
        "P10.diff_6000_lines_ms",
        "P10.grid_5000_rows_fps",
        "P10.lighthouse_perf",
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
