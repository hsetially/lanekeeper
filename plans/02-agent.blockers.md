# Blockers: prompt 02, T4

## B1. Registering P1, P3, P4 and `P4.stat_walk` in `perf/budgets.toml` breaks tests the plan's paths do not cover

**State.** Everything in T4 is in the `02/T4` commit and passes: the Merkle tree, the own cap-std walker, the delta builder, the scan loop, the four benchmarks and their harnesses, and decision D89. The one part of T4 that is **not** in that commit is the contract-change edit to `perf/budgets.toml` (flip `registered` for P1, P3, P4 and add the `P4.stat_walk` block). I did not make it, because making it turns `just verify` red and the fix is outside every path this plan opens. The harness results exist and pass; only the registry flip waits.

**What breaks.** I applied the edit to `perf/budgets.toml` for a moment, ran `cargo test -p xtask --test budgets_registry`, and restored the file (`git checkout`; it is clean in `02/T4`):

```
test nothing_is_registered_before_its_benchmark_exists ... FAILED
  crates/xtask/tests/budgets_registry.rs:295: P1: registered = true needs a benchmark that produces the result
test thresholds_match_docs_performance_md ... FAILED
  crates/xtask/tests/budgets_registry.rs:283: P4.stat_walk: not compared with docs/performance.md
test result: FAILED. 5 passed; 2 failed
```

1. `nothing_is_registered_before_its_benchmark_exists` asserts that **no** budget is registered, for ever. It was right for prompt 01 (no benchmark existed). The first prompt to register anything has to change it, and the comment in `perf/budgets.toml` says flipping is "that prompt's contract-change". The test lives in `crates/xtask`, which prompts 01 and 16 own (`scripts/lk-lib.mjs` ownership table; the guard would refuse an edit there for 02).
2. `thresholds_match_docs_performance_md` requires every registry entry to be compared with a sentence in `docs/performance.md`, and the new `P4.stat_walk` (1,000 ms, from T4's "stat walk <= 1 s") has none. `docs/performance.md` is a document no prompt owns, and it is not in the plan's Extra-paths either. Changing it is also a statement of a budget, which only a human should make.

The plan's header lists two Extra-paths (`deny.toml`, `docs/decisions.md`) and the contract path `perf/budgets.toml`. The plan's contract item 1 did not notice that the registry has two guards in other crates. I stop here rather than work around it (no edit to `crates/xtask` or `docs/` through the shell, no weakened test).

**Measurements (the harness is real and passes).** `cargo xtask bench-check` against a scratch copy of the registry with the four rows registered, results from `cargo bench -p agent` in release mode on a copy of swimlane 1 of `cargo xtask gen-fixtures --scale target` (2,003 files, local disk, so assumption A15 applies):

```
PASS  P1                                     9587.583 p95 <= 15000 ms
PASS  P3                                          672 p95 <= 1024 bytes_per_min
PASS  P4                                        6.426 p95 <= 20000 ms
PASS  P4.stat_walk                              2.904 p95 <= 1000 ms
bench-check (default): 4 pass, 0 fail, 30 unmet of 34 budgets
```

P1 is twelve real-time changes spread over the 10 s walk interval, through the real walker, the worker pool and a TLS 1.3 stream: 9,588 / 8,756 / 7,921 / 7,090 / 6,253 / 5,423 / 4,588 / 3,753 / 2,921 / 2,088 / 1,253 / 419 ms. The worst case is the walk interval minus the offset plus about 5 ms of work, so the p95 sits just under 10 s against the 15 s budget. P3 is 672 bytes in every one of ten one-minute windows (TLS 1.3 ciphertext, both directions, decision A14). The unchanged `perf/budgets.toml` in `02/T4` gives `0 pass, 0 fail, 33 unmet`, so `cargo xtask bench-check` is green on this branch as it stands.

**The change, ready to apply.** I checked the patch below in a throwaway `git worktree` of `02/T4` (since removed): with it applied, `cargo test -p xtask --test budgets_registry --test bench_check` passes (7 and 17 tests) and `cargo xtask bench-check` exits 0 with the four PASS rows above. The new test turns "nothing is registered" into "exactly this list is registered", so a registration still needs a reviewed edit in the same PR; T7 (`P4.cpu_mcores`, `P4.memory_mib`) and T9 (`P15`) add their ids to that list when they register.

```diff
--- a/perf/budgets.toml
+++ b/perf/budgets.toml
@@ -27,7 +27,7 @@
 threshold = 15000
 unit = "ms"
 scale = "target"
-registered = false
+registered = true
 
 [[budget]]
 id = "P2"
@@ -47,7 +47,7 @@
 threshold = 1024
 unit = "bytes_per_min"
 scale = "target"
-registered = false
+registered = true
 
 [[budget]]
 id = "P4"
@@ -57,7 +57,17 @@
 threshold = 20000
 unit = "ms"
 scale = "target"
-registered = false
+registered = true
+
+[[budget]]
+id = "P4.stat_walk"
+description = "Agent stat walk of 2,000 files"
+bench = "agent/stat_walk_2000_files"
+statistic = "p95"
+threshold = 1000
+unit = "ms"
+scale = "target"
+registered = true
 
 [[budget]]
 id = "P4.cpu_mcores"
--- a/crates/xtask/tests/budgets_registry.rs
+++ b/crates/xtask/tests/budgets_registry.rs
@@ -46,6 +46,7 @@
     // The multi-part budgets get one entry per part (plan Q5, Appendix C).
     for id in [
         "P4",
+        "P4.stat_walk",
         "P4.cpu_mcores",
         "P4.memory_mib",
         "P5",
@@ -164,6 +165,12 @@
             "ms",
             "full rehash of 2,000 files takes under 20 seconds",
         ),
+        (
+            "P4.stat_walk",
+            1_000.0,
+            "ms",
+            "A stat walk of 2,000 files takes under 1 second",
+        ),
         ("P4.cpu_mcores", 50.0, "mcores", "at most 50 mCPU"),
         ("P4.memory_mib", 64.0, "mib", "64 MiB of memory"),
         ("P5", 50.0, "ms", "within 50 ms + 2 ms"),
@@ -288,16 +295,25 @@
     }
 }
 
+/// A budget is registered only together with the benchmark or harness that produces its result (plan Q5), and flipping
+/// it is the owning prompt's contract change. So the list is written out here: a PR that registers a budget changes it,
+/// in view of the reviewer. Everything else stays unregistered (reported as UNMET).
 #[test]
-fn nothing_is_registered_before_its_benchmark_exists() {
-    // Prompt 01 writes no benchmark. Flipping `registered` is the owning prompt's contract change (plan Q5).
-    for b in &registry().budgets {
-        assert!(
-            !b.registered,
-            "{}: registered = true needs a benchmark that produces the result",
-            b.id
-        );
-    }
+fn only_budgets_with_a_harness_are_registered() {
+    // Prompt 02 (agent): `cargo bench -p agent` produces P1, P3, P4 and P4.stat_walk.
+    const REGISTERED: &[&str] = &["P1", "P3", "P4", "P4.stat_walk"];
+    let reg = registry();
+    let registered: BTreeSet<&str> = reg
+        .budgets
+        .iter()
+        .filter(|b| b.registered)
+        .map(|b| b.id.as_str())
+        .collect();
+    let expected: BTreeSet<&str> = REGISTERED.iter().copied().collect();
+    assert_eq!(
+        registered, expected,
+        "registered = true needs a benchmark that produces the result, and this list updated in the same PR"
+    );
 }
 
 #[test]
--- a/docs/performance.md
+++ b/docs/performance.md
@@ -22,7 +22,7 @@
 **Agent**
 
 - **P3** Idle traffic from each agent stays under 1 KB per minute: one heartbeat carrying a Merkle root.
-- **P4** Each agent uses at most 50 mCPU and 64 MiB of memory at steady state. A full rehash of 2,000 files takes under 20 seconds.
+- **P4** Each agent uses at most 50 mCPU and 64 MiB of memory at steady state. A full rehash of 2,000 files takes under 20 seconds. A stat walk of 2,000 files takes under 1 second.
 
 **Recompute**
```

**Options for a human.**

- **A (preferred).** Add `crates/xtask/tests/budgets_registry.rs` and `docs/performance.md` to this plan's Extra-paths (and say that the `P4.stat_walk` sentence is approved wording). Then the patch above goes in as a `02/fix:` commit on this branch, and acceptance item 1 (`bench-check` shows P1, P3, P4 within budget) is met on the branch. The plan's reviewer check "perf/budgets.toml differs only in `registered` fields and the one new block" still holds.
- **B.** Leave the registry unregistered on this branch and register in a small follow-up `contract:` PR owned by prompt 01 after this PR merges. Until then P1, P3 and P4 show UNMET in `bench-check`, and acceptance item 1 can only be shown with the scratch registry above.
- **C.** Drop the `P4.stat_walk` block (keep the 1 s check as the assertion it is today in the `stat_walk` benchmark, and in the plan's evidence) and register only P1, P3 and P4. That still needs the `crates/xtask/tests/budgets_registry.rs` change for `nothing_is_registered_before_its_benchmark_exists`, but not the `docs/performance.md` edit.

Whatever is chosen, T7 and T9 meet the same two guards when they register `P4.cpu_mcores`, `P4.memory_mib` and `P15.spool_replay_versions_per_s`; with option A the test's list is the one place they extend.

## Notes that are not blockers

- **D89** is the next free number in `docs/decisions.md` (D88 was the last; no remote branch uses D89). Written in `02/T4` with the plan's wording.
- **Gate steps not run here:** `osv-scanner` (not installed). `just verify-02` does not exist yet (T7). I ran the individual steps of the gate that exist: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo audit` (all pass), and the benchmarks above.
