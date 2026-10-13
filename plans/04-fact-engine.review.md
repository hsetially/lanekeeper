# 04 review, round 1

Reviewer output, saved verbatim in substance by the orchestrator.

VERDICT: APPROVE

This approval is conditional on two human steps that the evidence file also names. The two human-owned items are the 60 ms amendment and the real-snapshot run. Neither can be done or verified by a reviewer.

## Blocking

None.

## Non-blocking

1. **Acceptance item 1 is unmet until the human applies the 60 ms amendment.**
   - `P6.index_settings` is `registered = false` at `threshold = 40` in `perf/budgets.toml`. Reviewer measured 40.201 ms p95; the evidence file has 46.348 ms and the blockers file 45.8 ms.
   - Leaving it unregistered is safe. `bench-check` prints it as `UNMET ... not registered; measured outside budget`, so it cannot pass silently. The engine's `engine-bench` list excludes it, and `justfile_gate` fails loudly if the Justfile and the registry disagree.
   - Registering it takes more than the four edits listed (plan row, `budgets.toml`, Justfile `engine_budgets`, parity row). `REGISTERED` in `crates/xtask/tests/budgets_registry.rs` (around line 436) must also gain `"P6.index_settings"`, otherwise `only_budgets_with_a_harness_are_registered` fails.
   - The `from_prompt` row in `budgets_registry.rs` pins the string `| `index_settings_6000_lines` | `P6.index_settings` | 40 ms |` in the plan. That row, the plan and `budgets.toml` must change together.
2. **Acceptance item 4 is not done.** It needs a human run of `just engine-real-snapshot` with `LK_REAL_BASE_DIR` and `LK_REAL_TENANT_DIR`; per plan A4 the output goes in the PR. The synthetic mirrors `checks_fixture::{c1,c2,c4,c8,c9}_matches_manifest_oracle` pass but do not count. The config-server golden fixtures are reconstructions (plan A3), so "byte for byte" is proved against our reconstruction only.
3. **osv-scanner was not run, and the reviewer could not run it** (not installed; `just verify` does not run it). AGENTS.md rule 15 requires it, so a human or CI must run it before merge. `cargo deny` and `cargo audit` pass (1310 advisories loaded); `just engine-fuzz` also runs both on the fuzz lockfile.
   - New crates and ages, checked against crates.io where reachable: granit-parser 1.3.0 is 2026-09-16 (27 days); tree-sitter 0.26.13 is 2026-08-23; tree-sitter-yaml 0.7.2 is 2025-10-07; grep-searcher 0.1.17 is 2026-07-15; tree-sitter-language 0.1.7 is 2026-02-01; streaming-iterator 0.1.9 is 2023.
   - json-event-parser, grep-regex and grep-matcher were not checked against crates.io (evidence file: json-event-parser 2026-02-06, grep-regex 2025-10-16). The fuzz crate's 649-line `Cargo.lock` was not inspected for transitive ages.
4. **The full `cargo xtask bench-check` exit 1 is pre-existing and not caused by this branch.** The 17 FAIL rows are exactly the 17 ids that `main:perf/budgets.toml` already registers (P1 to P4, P4.*, P7.blob_*, P7.auth_*, P11*, P12.audit_record, P13, P15.*); they fail with "registered but no result" because the agent and platform bench recipes were not run. The branch adds 7 PASS rows and 1 UNMET row (`P6.index_settings`). The other UNMET rows are other prompts' unregistered budgets.
5. **The P6 benchmark does not time `index_settings`.** `benches/full_recompute.rs` and `recompute_all` parse and hash each blob but never call tree-sitter or `index_settings`. `docs/performance.md` mechanism 3 puts the settings index inside the nightly work. The blockers file estimates 4 to 5 CPU-seconds for indexing all unique blobs; P6 passes at 1.7 s against 180 s either way. Folding the index step into the P6 bench, or into 05's nightly bench, would make the number honest.
6. **Known correctness gap in the batch effective hash**, disclosed in `docs/threat-model.md` under residual risks. `batch/lane.rs` hashes tenant, base and root `application.*` layers only. `application-<tenant>.*` (rank 2 in D82) is in `property_view` but not in the per-file tree hash, so a change to it that overrides a service file will not move that file's effective or tree hash. A human should decide whether 05 must go through `property_view` or the engine must add the layer.
7. **Tree-sitter is a cross-check, not the source of ranges.** In `index/settings.rs` every `SettingRow.lines` comes from the parser; tree-sitter only adds `LocationMismatch` warnings. The prompt says "locations come from tree-sitter". Tree-sitter costs about 40% of the 6,000-line `index_settings` time (blockers file option 2); the module docs describe it accurately.
8. **T3's commit also fixed four block-scalar off-by-one errors in T2's parser.** Documented in the blockers file; regression test `block_scalars_are_on_the_lines_from_their_indicator_to_their_last_line_of_text`. The one-commit-per-task trace is slightly blurred. The extra `04/blocked:` commit between T3 and T4 looks fine.
9. **The secret scan skips hex digests and UUIDs by default (`hex_is_random = false`).** A 64-hex AES key would pass. Documented as tunable (A18, Q20) and listed as heuristic in the threat model. Tune before 07 goes live.
10. **Search hits carry line text unredacted.** The threat model states callers (05, 07) must pass hits through `secret_scan`. Make sure 05 and 07 pick that up.
11. **Trivial naming drift from the plan's test names:** `c1_match_manifest_oracle` is `c1_matches_manifest_oracle`; `fuzz_target_list_matches_directory` is `engine_fuzz_list_matches_the_fuzz_targets_directory`; `only_differing_default` is `only_differing_is_what_the_default_shows`. Behaviour is covered.

## Spec chain

- **Plan approval:** `d6c9b0e` flips Status and Contract-change to APPROVED, same convention as 01, 02, 03a and 03b. `git log -- plans/04-fact-engine.md` shows only `c06bda1` and `d6c9b0e`; no later commit touches the plan.
- **Traceability:** every test name in the traceability table exists except three renamed (item 11). Fuzz targets exist for all 10 names, each with 15 to 98 seeds. No sleeps, no tautological asserts, no `HashMap`, `Instant` or env access in `src/`.
- **Commit order:** `NN/T1` to `T10` in plan order, followed by `04/evidence`.
- **Scope:** besides `crates/engine/**` the diff touches `Cargo.toml`, `Cargo.lock`, `Justfile` (allowed), `docs/decisions.md` (D90), `docs/threat-model.md` (`## Fact engine (04)`), `crates/xtask/tests/budgets_registry.rs` and `perf/budgets.toml`. The first three non-allowed paths are plan Extra-paths; the fourth is the plan's Contract-change. `domain`, `ports`, `proto`, `api`, `db`, `docs/mcp-tools.md` and `design` are untouched.
- **`perf/budgets.toml` diff:** only the `P6` and `P7.compare` flips plus six new blocks, as planned. `P6.index_settings` is the one block left `registered = false`, which deviates from the plan and is disclosed.

## Evidence re-run

- `just verify-04`: PASS, exit 0. Clippy `-D warnings` clean; 519 tests passed, 0 failed; all 10 engine fuzz targets ran 30 s each clean; the 3 domain targets in `fuzz-smoke` ran clean. Final engine-only bench-check `7 pass, 0 fail, 0 unmet of 7`: P6 1673.7 ms, P6.parse_flatten 11.7, P7.compare 26.7, P7.engine_grid 41.8, P7.engine_text_search 156.3, P5.engine_base 2.4, P5.engine_per_file_per_swimlane 0.10.
- `just verify`: PASS, exit 0. Workspace ran 2676 tests, 0 failed; `cargo deny` advisories, bans, licenses and sources ok; `cargo audit` loaded 1310 advisories; web lint, typecheck, vitest and build passed.
- `cargo xtask bench-check` (full registry): exit 1, `7 pass, 17 fail, 22 unmet of 46`. The 17 FAIL rows are pre-existing (item 4). Engine rows: 7 PASS plus `P6.index_settings` UNMET at 40.201 ms against 40 ms.
- Real-snapshot run: not run (needs a human). osv-scanner: not installed here, not run.

## Security trace

- **S11:** `src/search.rs`, `src/parse/*`, `src/limits.rs`; tests `public_api_takes_no_raw_string_paths`, `parse_limits::*`, `catastrophic_pattern_runs_linear`; fuzz `engine_regex`, `engine_path_mapping`.
- **S13:** `src/secret_scan.rs`; tests `match_never_contains_value`, `marker_format`; fuzz `engine_secret_scan`.
- **S21:** `#![forbid(unsafe_code)]` plus workspace clippy denies; tests `errors::display_and_debug_never_echo_input`, `forbid_unsafe_present`.
- **S22:** the 10 engine fuzz targets; `corpus_replay_all_engine_targets`; tree-sitter input path `index_settings_never_hands_rejected_input_to_tree_sitter`.
- **S24:** `docs/threat-model.md` `## Fact engine (04)` has all six STRIDE rows and residual risks, checked by `threat_model::engine_section_has_all_six_stride_letters_and_cites_s11_s13_s21_s22`. Threat model updated: yes.
