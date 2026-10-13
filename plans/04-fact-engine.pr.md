04: Fact engine

## Prompt

`prompts/04-fact-engine.md` · plan: `plans/04-fact-engine.md` (approved by a human, with Extra-paths and the `perf/budgets.toml` contract change) · wave: 1

**Not ready to merge as is. Three human steps remain, listed first so they are not missed:**

1. **Raise `P6.index_settings` to 60 ms and register it.** T3 measured 45.8 ms p95 (46.3 ms in the final run) against the plan's own proposed 40 ms (the prompt gives no number); tree-sitter's C parse of the 6,000-line fixture is 14 to 37 ms by itself. The human chose 60 ms (blocker: `plans/04-fact-engine.blockers.md`). The change was deferred to this PR and is **not applied**. It needs, together:
   - the plan's table row, contract list and A1 row (40 to 60);
   - `perf/budgets.toml` (`threshold = 60`, `registered = true`);
   - the Justfile `engine_budgets` list;
   - the `from_prompt` parity row **and** the `REGISTERED` list in `crates/xtask/tests/budgets_registry.rs` (the reviewer found the second one; without it `only_budgets_with_a_harness_are_registered` fails).
2. **Run the real-snapshot expectations (acceptance item 4):** `LK_REAL_BASE_DIR=… LK_REAL_TENANT_DIR=… just engine-real-snapshot` (working-tree checkouts with git history for C1) and paste the output here. Without the env vars the recipe prints `NOT RUN`, which is not a pass.
3. **Run `osv-scanner`.** It is not part of `just verify` and was not installed where this ran. AGENTS.md rule 15 requires it. `cargo deny` and `cargo audit` pass.

## Gates

| Command | Result |
|---|---|
| `just verify-04` | PASS, exit 0. 519 tests passed, 0 failed, 15 ignored (opt-in target-scale and real-snapshot tests). All 10 engine fuzz targets and the 3 domain `fuzz-smoke` targets ran 30 s each with no crash. Engine-only bench-check: `7 pass, 0 fail, 0 unmet of 7`. |
| `just verify` | PASS, exit 0. fmt, clippy `-D warnings`, 2676 workspace tests, `cargo deny` (advisories, bans, licenses, sources ok), `cargo audit` (1310 advisories), web lint, typecheck, vitest and build. Re-run by the reviewer, also PASS. |
| `cargo xtask bench-check` (full registry) | **exit 1**: `7 pass, 17 fail, 22 unmet of 46`. Not caused by this branch: the 17 FAIL rows are exactly the 17 ids that `main:perf/budgets.toml` already registers (P1 to P4, P7.blob_*, P7.auth_*, P11, P12.audit_record, P13, P15), which fail with "registered but no result" because the agent and platform bench recipes were not run. This branch adds 7 PASS rows and 1 UNMET row (`P6.index_settings`). |

Full output: `plans/04-fact-engine.evidence.md`.

## Acceptance checklist

- [ ] `just verify-04` passes, and every engine budget is met in `bench-check`. **Partly:** `verify-04` passes and 7 of 8 engine budgets pass. `P6.index_settings` is listed `UNMET ... not registered; measured outside budget` until human step 1.
- [x] Fuzz targets exist for YAML, JSON and `.properties`, flattening, path mapping and regex options; `just fuzz-smoke` runs clean. Ten engine targets (own crate `crates/engine/fuzz/`, plan A2), 15 to 98 seeds each, replayed on stable by `corpus_replay_all_engine_targets`.
- [x] Output is deterministic: `determinism::everything_twice_is_byte_identical` (every public function, pools of 1, 2 and 8 threads) and `source_rules` (no random-order maps, clocks or I/O).
- [ ] The opt-in real-snapshot expectations are met. **NOT RUN**, human step 2. The synthetic mirrors `checks_fixture::{c1,c2,c4,c8,c9}_matches_manifest_oracle` pass but do not count.

## Security requirements implemented (S#)

- **S11** (typed, bounded input; linear-time regex): `src/search.rs`, `src/parse/*`, `src/limits.rs`. Tests `public_api_takes_no_raw_string_paths`, `parse_limits::*`, `catastrophic_pattern_runs_linear`; fuzz `engine_regex`, `engine_path_mapping`.
- **S13** (value-based secret check; lines and lengths only): `src/secret_scan.rs`. Tests `match_never_contains_value`, `marker_format`; fuzz `engine_secret_scan`.
- **S21** (no unsafe, no panics, no leaked internals): `forbid_unsafe_present`, workspace clippy lints, `display_and_debug_never_echo_input`.
- **S22** (fuzz every parser at a trust boundary): the ten targets; `index_settings_never_hands_rejected_input_to_tree_sitter` keeps the C parser off input the Rust parser rejected.
- **S24** (threat model): `docs/threat-model.md` `## Fact engine (04)`, six STRIDE rows plus residual risks, checked by `threat_model::engine_section_has_all_six_stride_letters_and_cites_s11_s13_s21_s22`.

## Benchmarks against budgets (P#)

p95, shared 4 vCPU box (noisy), 2-thread pool.

| P# | Benchmark | Result | Budget | Status |
|---|---|---|---|---|
| P6 | `engine/full_recompute_target_scale` (40 lanes, about 80,000 files) | 1308 ms | 180000 ms | PASS |
| P6.parse_flatten | `engine/parse_flatten_6000_lines` | 9.2 ms | 20 ms | PASS |
| P6.index_settings | `engine/index_settings_6000_lines` | **46.3 ms** | 40 ms proposed, 60 ms chosen, not applied | **UNMET, not registered** |
| P7.compare | `engine/compare_6000_lines` (inline ranges on) | 24.5 ms | 100 ms | PASS |
| P7.engine_grid | `engine/grid_40_swimlanes` | 47.5 ms | 150 ms | PASS |
| P7.engine_text_search | `engine/search_text_target_blobs` | 151.4 ms | 400 ms | PASS |
| P5.engine_base | `engine/incremental_recompute_base` | 3.3 ms | 50 ms | PASS |
| P5.engine_per_file_per_swimlane | `engine/incremental_recompute_per_file_per_swimlane` | 0.14 ms | 2 ms | PASS |

The P6 bench does not time `index_settings`, so its pass does not include the tree-sitter cost (reviewer note 5). The blocker file estimates 4 to 5 CPU-seconds to index every unique blob once, well inside 180 s.

## New dependencies and why

Pinned exactly, each at least 14 days old when added. `cargo deny` and `cargo audit` clean; **`osv-scanner` not run** (human step 3).

| Crate | Version | Why |
|---|---|---|
| granit-parser | =1.3.0 (2026-09-16) | Event-level YAML parser with marks, anchors and nesting limits; chosen over saphyr-parser by benchmark (D90) |
| json-event-parser | =0.2.3 | Strict JSON tokeniser, no I/O, stack limit |
| tree-sitter | =0.26.13 (2026-08-23) | Setting path to line range cross-check for YAML (D54); only sees input the Rust parser accepted |
| tree-sitter-yaml | =0.7.2 (2025-10-07) | The YAML grammar |
| similar | =2.7.0 | Line diff (already in the lockfile through insta) |
| grep-regex / grep-searcher / grep-matcher | =0.1.14 / =0.1.17 / =0.1.9 | ripgrep matcher stack: linear-time regex with limits over in-memory slices |

Transitive entries under 14 days old in the fuzz lockfile (serde_json, tree-sitter-language) were pinned back to older versions.

## Assumptions

Plan assumptions A1 to A22 stand as approved. Worth a human look:

- **A3:** the golden config-server fixtures are reconstructions from the documented inputs and outputs (labelled in their README), not the originals. "Byte for byte" is proved against our reconstruction only.
- **A2:** engine fuzz targets live in `crates/engine/fuzz/` with their own lockfile; CI must add `engine-fuzz`.
- **A5:** severity rules keep `drift_states`, `file_classes` and `root_file` engine-internal; no OpenAPI change. Callers 05 and 06 must treat them as read-only on default rows.
- **Not in the prompt or docs (chosen in T2, please confirm or amend `docs/domain-model.md`):** document prefix `#<n>/` is 0-based; a single-value file gets path `$`; unusual first keys are bracket-quoted; empty `[]` and `{}` are leaf settings of type Other; a leading-zero integer such as `0755` is a string; block-scalar ranges exclude trailing blank lines; JSON and `.properties` column numbers are 0.
- **Tree-sitter is a cross-check, not the source of ranges** (reviewer note 7): rows always keep the parser's range and a disagreement adds `LocationMismatch`. The prompt's wording is "locations come from tree-sitter".
- **Deviations beyond the plan's wording:** `Limits::diff_cells` (a plain Myers fallback took 4.1 s on two 10,000-line files; adversarial pairs now take at most about 230 ms or return `too_large`); `drift_state` takes `Baseline { NotRecorded, Recorded(..) }`; `grid` borrows its cells and pages by keyset; `file_role` takes the tenant set (A6); no serde on `SecretMatch`; no `json_pos.rs` or `props_pos.rs`.
- T3's commit also fixed four block-scalar line off-by-one errors in T2's parser (regression test `block_scalars_are_on_the_lines_from_their_indicator_to_their_last_line_of_text`), so the one-commit-per-task trace is slightly blurred. A `04/blocked:` commit sits between T3 and T4.

**Open follow-ups for other prompts (reviewer notes 5, 6, 9, 10):**
- The batch effective hash uses tenant, base and root `application.*` layers only. `application-<tenant>.*` is covered by `property_view` and C10 but not by the per-file hash, so a change to it will not move that file's hash. Disclosed in the threat model; 05 should go through `property_view` or the engine should add the layer. A human should decide which.
- Search hits carry unredacted line text; 05 and 07 must pass them through `secret_scan`.
- The secret scan skips hex digests and UUIDs by default (`hex_is_random = false`), so a 64-hex key would pass. Tune once Q20 is answered, before 07 goes live.
- A root `application.*` edit recomputes every `spring_merge` group (about 247 ms per swimlane); a hub-owned parsed-blob cache in 05 would remove it.
- The first `just verify` of the final step hit a full disk (59 GB in this worktree's `target/`); only that worktree's regenerable incremental cache was deleted and the whole recipe re-run. Not a code failure.

## Reviewer agent verdict

```
VERDICT: APPROVE
Blocking: none.
Non-blocking: 11 notes (see plans/04-fact-engine.review.md): acceptance items 1 and 4 open for the human,
osv-scanner not run, full bench-check exit 1 is pre-existing, P6 bench does not time index_settings,
batch hash omits application-<tenant>.*, tree-sitter is a cross-check, hex digests skipped by the secret scan.
```

Round 1 of at most 3. The approval is conditional on human steps 1 and 2 above.

## Labels

`contract-change`: the plan's Contract-change was approved by a human (`perf/budgets.toml` only: `P6` and `P7.compare` flipped to registered, five new entries registered, and `P6.index_settings` added unregistered). Extra-paths approved with the plan: `docs/decisions.md` (D90), `docs/threat-model.md`, `crates/xtask/tests/budgets_registry.rs`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
