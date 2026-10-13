# Evidence 04: Fact engine

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proved.

## Read this first: what is NOT met

1. **"Every engine budget is met in `bench-check`" is NOT fully met.** `P6.index_settings` measures **46.348 ms p95** in the
   full `cargo xtask bench-check` below (45.811 ms in the T3 run, criterion estimate 35.5 to 37.6 ms) against the plan's proposed
   40 ms. The entry in `perf/budgets.toml` is `registered = false` with `threshold = 40`, it is not in the Justfile `engine_budgets`
   list, and `bench-check` lists it as `UNMET ... not registered; measured outside budget`. The human acknowledged this
   (`plans/04-fact-engine.blockers.md`, T3) and chose to raise the budget to 60 ms; they will amend the plan, `perf/budgets.toml`,
   the Justfile `engine_budgets` list and the `budgets_registry.rs` parity row when the draft PR is raised. **Nothing of that was
   touched here, and the entry is not registered.** Until the human applies the 60 ms change this acceptance item stays unticked.
   The other seven engine budgets pass (table below).
2. **Acceptance item 4 (the real-snapshot run) is NOT done.** It needs a human with `LK_REAL_BASE_DIR` (a
   `configuration-base-saas` checkout, default branch) and `LK_REAL_TENANT_DIR` (a `csp-tenant-data` checkout at `sit1`), then
   `just engine-real-snapshot`. Without them the recipe prints `real-snapshot: NOT RUN`, which is not a pass. The always-on
   mirrors on the synthetic fixtures pass (`checks_fixture::{c1,c2,c8,c9}_match_manifest_oracle`), but they are not the real run.
3. **The golden config-server fixtures are reconstructions (plan A3), not the originals.** `tests/golden/config-server/` holds
   synthetic files rebuilt from the documented inputs and outputs (its README says so). "Ported fixtures match byte for byte" is
   proved against our reconstruction only. Swapping in the originals needs no code change.
4. **The full `cargo xtask bench-check` exits 1**, but not because of the engine: see the gates table and the output. 17 other
   budgets (prompts 02, 03a, 03b) have "registered but no result" because their harnesses (`just platform-bench` and the agent's
   recipes) were not run in this step, and the rest are unregistered budgets of later prompts. The engine's own rows are the
   ones listed under Benchmarks.

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-04` | PASS, exit 0 | 519 tests passed, 0 failed, 15 ignored (the `#[ignore]`d target-scale and real-snapshot tests). `engine-fuzz: engine_bytes engine_yaml engine_json engine_properties engine_flatten engine_index_settings engine_path_mapping engine_regex engine_secret_scan engine_render ran 30 s each without a crash`. `fuzz-smoke: nfs_path compare_ref path_mapping_reverse ran 30 s each without a crash`. Last line: `bench-check (default): 7 pass, 0 fail, 0 unmet of 7 budgets` (the seven ids in `engine_budgets`; `P6.index_settings` is not among them because it is not registered). |
| `just verify` | PASS, exit 0 | fmt, clippy, workspace tests, `cargo deny` (`advisories ok, bans ok, licenses ok, sources ok`), `cargo audit` (loaded 1310 advisories), web lint, typecheck, vitest (1 passed) and build all ran. A first run failed with `No space left on device` at link time (disk 100% full, 59 GB in this worktree's `target/`); I deleted only this worktree's `target/debug/incremental` (regenerable cache), re-ran the whole recipe from the start, and it passed. Not a code failure. |
| `cargo xtask bench-check` | **FAIL, exit 1** | `bench-check (default): 7 pass, 17 fail, 22 unmet of 46 budgets`. Full output below. |

### `cargo xtask bench-check`, full output

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s
     Running `target/debug/xtask bench-check`
FAIL  P1                                            - p95 <= 15000 ms  registered but no result
FAIL  P2                                            - p95 <= 10000 ms  registered but no result
FAIL  P3                                            - p95 <= 1024 bytes_per_min  registered but no result
FAIL  P4                                            - p95 <= 20000 ms  registered but no result
FAIL  P4.stat_walk                                  - p95 <= 1000 ms  registered but no result
FAIL  P4.cpu_mcores                                 - p95 <= 50 mcores  registered but no result
FAIL  P4.memory_mib                                 - max <= 64 mib  registered but no result
UNMET P5                                            - p95 <= 50 ms  not registered; no benchmark yet
UNMET P5.per_file_per_swimlane                      - p95 <= 2 ms  not registered; no benchmark yet
PASS  P6                                     1308.294 p95 <= 180000 ms  
PASS  P6.parse_flatten                          9.151 p95 <= 20 ms  
UNMET P6.index_settings                        46.348 p95 <= 40 ms  not registered; measured outside budget
UNMET P7.swimlane_list                              - p95 <= 50 ms  not registered; no benchmark yet
UNMET P7.file_tree                                  - p95 <= 100 ms  not registered; no benchmark yet
UNMET P7.file_content_cached                        - p95 <= 30 ms  not registered; no benchmark yet
UNMET P7.file_content_cold                          - p95 <= 80 ms  not registered; no benchmark yet
PASS  P7.compare                               24.509 p95 <= 100 ms  
PASS  P7.engine_grid                           47.475 p95 <= 150 ms  
PASS  P7.engine_text_search                   151.352 p95 <= 400 ms  
PASS  P5.engine_base                            3.325 p95 <= 50 ms  
PASS  P5.engine_per_file_per_swimlane           0.137 p95 <= 2 ms  
UNMET P7.grid                                       - p95 <= 150 ms  not registered; no benchmark yet
UNMET P7.whole_swimlane_compare                     - p95 <= 300 ms  not registered; no benchmark yet
UNMET P7.settings_search                            - p95 <= 200 ms  not registered; no benchmark yet
UNMET P7.text_search                                - p95 <= 400 ms  not registered; no benchmark yet
UNMET P7.docs_search                                - p95 <= 150 ms  not registered; no benchmark yet
UNMET P7.feature_status                             - p95 <= 200 ms  not registered; no benchmark yet
FAIL  P7.blob_get_cached                            - p95 <= 1 ms  registered but no result
FAIL  P7.auth_middleware                            - p95 <= 2 ms  registered but no result
FAIL  P7.blob_endpoint                              - p95 <= 30 ms  registered but no result
FAIL  P7.auth_middleware_cold                       - p95 <= 2 ms  registered but no result
UNMET P8                                            - p95 <= 1500 ms  not registered; no benchmark yet
UNMET P9                                            - p95 <= 300 ms  not registered; no benchmark yet
UNMET P10.initial_js_gzip_kb                        - max <= 250 kb  not registered; no benchmark yet
UNMET P10.route_change_ms                           - p95 <= 100 ms  not registered; no benchmark yet
UNMET P10.diff_6000_lines_ms                        - p95 <= 500 ms  not registered; no benchmark yet
UNMET P10.grid_5000_rows_fps                        - min >= 60 fps  not registered; no benchmark yet
UNMET P10.lighthouse_perf                           - p50 >= 90 score  not registered; no benchmark yet
FAIL  P11                                           - p95 <= 10000 ms  registered but no result
FAIL  P11.memory_mib                                - max <= 1536 mib  registered but no result
FAIL  P12.audit_record                              - p95 <= 3 ms  registered but no result
UNMET P12.steady_state_query_ms                     - p95 <= 50 ms  not registered; no benchmark yet
FAIL  P13                                           - p95 <= 1000 ms  registered but no result
UNMET P14                                           - p95 <= 30000 ms  not registered; no benchmark yet
FAIL  P15.spool_replay_versions_per_s               - min >= 1000 versions_per_s  registered but no result
FAIL  P15.hub_ack_versions_per_s                    - min >= 1000 versions_per_s  registered but no result
bench-check (default): 7 pass, 17 fail, 22 unmet of 46 budgets
```

Reading it: the 17 `FAIL` rows are P1 to P4 (prompt 02, the agent), P7.blob_*, P7.auth_*, P11, P12.audit_record, P13, P15
(prompts 03a and 03b). Their results are written by `just agent-bench`-style and `just platform-bench` recipes that were not part
of this step, so the whole-registry check cannot pass from prompt 04's gate alone (the same reason the Justfile's `engine-bench`
filters to the engine ids). The unmet rows are other prompts' budgets plus `P6.index_settings`. I did not run those recipes and
did not change any of the rows.

## Acceptance checklist

- [ ] `just verify-04` passes, and every engine budget is met in `bench-check`. **Partly.** `just verify-04` passes (above) and 7
  of the 8 engine budgets pass. `P6.index_settings` is NOT met (46.348 ms p95 against the proposed 40 ms) and is listed UNMET and
  unregistered. This stays unticked until the human applies the 60 ms change (plan, `perf/budgets.toml`, Justfile `engine_budgets`,
  `budgets_registry.rs` row) and a bench-check shows it registered and passing.
- [x] Fuzz targets exist for the YAML, JSON and `.properties` handling, flattening, path mapping and regex options, and
  `just fuzz-smoke` runs clean. Proof: ten targets in `crates/engine/fuzz/` (`engine_bytes`, `engine_yaml`, `engine_json`,
  `engine_properties`, `engine_flatten`, `engine_index_settings`, `engine_path_mapping`, `engine_regex`, `engine_secret_scan`,
  `engine_render`) ran 30 s each without a crash in `just verify-04`; `fuzz-smoke` (the domain targets) ran clean; the stable replay
  `corpus_replay.rs::corpus_replay_all_engine_targets` and the Justfile gate test pass in `engine-test`.
- [x] Output is deterministic. Proof: `tests/determinism.rs::everything_twice_is_byte_identical` (all public functions, pools of 1,
  2 and 8 threads), `tests/source_rules.rs` (no map or set with random order, no clock or I/O), all in the 519 passing tests of
  `verify-04`.
- [ ] The opt-in real-snapshot expectations are met. **NOT RUN.** Needs a human with `LK_REAL_BASE_DIR` and `LK_REAL_TENANT_DIR`
  (`just engine-real-snapshot`); see "Read this first" item 2. The always-on synthetic mirrors pass but do not count.

## Security requirements implemented

- S11 (typed inputs, bounded input, linear-time regex): `source_rules::public_api_takes_no_raw_string_paths`, `parse_limits::*`,
  `search::catastrophic_pattern_runs_linear` and the regex limit tests, `paths::*`; fuzz `engine_path_mapping`, `engine_regex`.
- S13 (value-based secret check, lines and lengths only): `secret_scan::match_never_contains_value` and the rule tests in
  `tests/secret_scan.rs`; fuzz `engine_secret_scan`.
- S21 (no unsafe, no panics, no leaked internals): `source_rules::forbid_unsafe_present`, workspace clippy lints (`engine-lint`),
  `errors::display_and_debug_never_echo_input`, `parse_limits` hostile-input tests.
- S22 (fuzz every parser at a trust boundary): the ten targets above; `corpus_replay_all_engine_targets`.
- S24 (threat model section): `docs/threat-model.md` `## Fact engine (04)`, checked by `threat_model::engine_section_has_all_six_stride_letters_and_cites_s11_s13_s21_s22`.
- S19 shaped the dependency choices (14-day rule): see New dependencies.

All of these tests are in the passing `engine-test` run. I did not re-run each by name beyond that gate.

## Benchmarks against budgets

Machine: shared 4 vCPU, noisy (the blocker file records the spread). The full `cargo xtask bench-check` reads the result files written
by the `just verify-04` run (`cargo bench -p engine`), so its seven engine numbers are the same as the ones `verify-04` printed.

| P# | Benchmark | Result (p95) | Budget | Status |
|---|---|---|---|---|
| P6 | `engine/full_recompute_target_scale` | 1308.294 ms | 180000 ms | PASS |
| P6.parse_flatten | `engine/parse_flatten_6000_lines` | 9.151 ms | 20 ms | PASS |
| P6.index_settings | `engine/index_settings_6000_lines` | **46.348 ms** (T3 run: 45.811; criterion estimate 35.5 to 37.6 ms) | 40 ms proposed; human chose 60 ms, not yet applied | **UNMET, not registered** |
| P7.compare | `engine/compare_6000_lines` (inline ranges on) | 24.509 ms | 100 ms | PASS |
| P7.engine_grid | `engine/grid_40_swimlanes` | 47.475 ms | 150 ms | PASS |
| P7.engine_text_search | `engine/search_text_target_blobs` | 151.352 ms | 400 ms | PASS |
| P5.engine_base | `engine/incremental_recompute_base` | 3.325 ms | 50 ms | PASS |
| P5.engine_per_file_per_swimlane | `engine/incremental_recompute_per_file_per_swimlane` | 0.137 ms | 2 ms | PASS |

## New dependencies

Pins and ages are as recorded in the workspace `Cargo.toml` comments at the T2 to T7 commits; ages are as of those commits, all
at least 14 days then.

| Crate or package | Version | Age in days | Why | `cargo deny` / `osv-scanner` |
|---|---|---|---|---|
| granit-parser | =1.3.0 (2026-09-16) | 27 | Event-level YAML parser with marks, anchors, alias events, nesting limits, `forbid(unsafe_code)`; chosen over saphyr-parser by benchmark (D90); already in the lockfile via serde-saphyr | `cargo deny` ok (in `just verify`); osv-scanner not run here |
| json-event-parser | =0.2.3 (2026-02-06) | >200 | Strict JSON tokeniser with no I/O and a stack limit | `cargo deny` ok; osv-scanner not run here |
| tree-sitter | =0.26.13 (2026-08-23) | 51 | Setting path to line range for YAML, D54; fed only input the Rust parser accepted | `cargo deny` ok; osv-scanner not run here |
| tree-sitter-yaml | =0.7.2 (2025-10-07) | >300 | The YAML grammar | `cargo deny` ok; osv-scanner not run here |
| similar | =2.7.0 | already in the lockfile (insta) | Patience line diff | `cargo deny` ok |
| grep-regex | =0.1.14 (2025-10-16) | >300 | ripgrep matcher over the `regex` crate, linear time with limits | `cargo deny` ok; osv-scanner not run here |
| grep-searcher | =0.1.17 (2026-07-15) | 90 | Searches one in-memory slice with line numbers | `cargo deny` ok; osv-scanner not run here |
| grep-matcher | =0.1.9 (2026-07-15) | 90 | Trait between the two above | `cargo deny` ok; osv-scanner not run here |
| xtask (path, dev-dependency) | workspace | n/a | Small-scale synthetic fixtures in tests and benches | n/a |

`just verify` runs `cargo deny` and `cargo audit` (both clean in the log). `osv-scanner` is not part of `just verify` and I did not
run it separately; the PR should state that, or a human should run it.

## Assumptions

All plan assumptions A1 to A22 stand as approved, with these notes for the PR:

- A1: the `P6.index_settings` threshold and registration are the human's pending change (see above).
- A2: the engine fuzz crate is `crates/engine/fuzz/` with its own lockfile; CI (prompt 15 or 09) must add `engine-fuzz`.
- A3: golden config-server fixtures are reconstructions.
- A4: the real-snapshot run is for a human.
- A5, A21: no contract change beyond `perf/budgets.toml`, plus `docs/decisions.md` (D90) and `docs/threat-model.md`.
- Environment: the first `just verify` attempt hit a full disk; see the gates table. The disk was 100% full when I started this
  step, so a CI-like clean machine may behave differently on build time but not on results.

## Deviations from the plan

- `P6.index_settings` is not registered and exceeds the plan's 40 ms (45.8 ms and 46.348 ms p95). Recorded in
  `plans/04-fact-engine.blockers.md` (commit `eb87ef8`). Resolution is the human's: raise to 60 ms and register.
- Acceptance item 4 and the original config-server goldens (A3) need a human, as the plan said they would.
