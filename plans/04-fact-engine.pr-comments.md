# 04 PR #10: triage of the automated review (cubic-dev-ai)

The 30 comments came from a page-fetch summary, so they were paraphrased. Each was checked against the code it names, by
file and description (line numbers had moved). A fix has a test that failed first (the commit says which), except where noted.

Commits (all on top of `712d339`, nothing rewritten):

- `8b4940a` `04/fix: root files merge, diff cost, decoded size, shared blob parses`
- `bc7458e` `04/fix: checks C1, C2, C6, C9, C10, C11, effect preview, line counting, adoption equality`
- `c499287` `04/fix: fuzz oracle, seed, docs, bench inputs and the real-snapshot exit status`

Counts: 30 items, 27 FIXED (19 in code, tests, fuzz or build files; 8 in docs or contracts only: 4, 7, 8, 9, 10, 11, 18, 29), 3 NOT A BUG / IRRELEVANT (3, 12, 28), 0 DEFERRED.

All paths are under `crates/engine/` unless they start with `Justfile`.

| # | Where | Verdict | Evidence |
|---|---|---|---|
| 1 | `src/batch/lane.rs` `root()` | FIXED (`8b4940a`) | True. `root()` returned the first of `application.properties`, `.yml`, `.yaml` that existed, while `property_view` merges all of them (`.properties` over `.yml` over `.yaml`). Now every root file is one merged lowest layer (`root_layer`); a root file is a file of its own, with no root layer below it. Test `every_root_file_is_a_layer_of_the_property_sources_and_properties_beats_yml_beats_yaml` (also checks the incremental path and C1's root settings). |
| 2 | `src/diff/align.rs` `leaf_cost` | FIXED (`8b4940a`) | True. Myers' loop runs to half the combined length, so its bill is the square of that, not the product of the sides: 49,003 lines against 2 cost 150,000 "cells" and about 600 million steps. `leaf_cost` is now `((old + new + 1) / 2 + 1)^2`, which is the product for equal sides (every existing boundary is unchanged). Test `a_skewed_pair_is_costed_by_the_work_myers_does_and_not_by_the_product_of_its_sides`. |
| 3 | `src/resolution/pickup.rs` | NOT A BUG | `live_by = changed_at + TTL` is the latest a client can serve the old file: a cache filled after `changed_at` already holds the new file, and one filled before expires before that. It is the "by HH:MM" upper bound, not a per-cache expiry. Doc and a comment now say so (`bc7458e`). |
| 4 | `src/index/locate.rs` doc | FIXED (docs, `c499287`) | The wording was loose. Behaviour is by design (`index/mod.rs`: when only tree-sitter cannot confirm, the row keeps the parser's range and `index_settings` warns; `locate` returns the range). The doc now separates the parser's error from the second reader's warning. |
| 5 | `src/parse/validate.rs` | FIXED (`8b4940a`) | True. `decode_text` holds the file's bytes to `input_bytes`, but `parse` held the decoded text to it again, and Latin-1 (up to 2x) or UTF-16 (up to 1.5x) text is longer than its file. New `parse_decoded` (text up to twice the limit) is used by `validate`, `index_settings`, the batch and `settings_of_blob`; `render_file` likewise. The text APIs (`parse`, `render`, `secret_scan`) keep their limit on text, and the file limit on bytes is untouched. Tests `the_size_limit_is_on_the_file_and_not_on_what_decoding_makes_of_it`, `a_latin1_file_within_the_limit_is_rendered_although_its_text_is_longer`. |
| 6 | `src/checks/c10.rs` | FIXED (`bc7458e`) | True. `tenants` followed the order of `views`. Now in tenant order (a set). Test `..._the_tenants_of_a_finding_do_not_depend_on_the_order_of_the_views`. |
| 7 | `src/limits.rs` module doc | FIXED (docs, `c499287`) | True: a diff gives `too_large`, a search skips or cuts, a check caps its lists. The module doc now lists what each kind of function does. |
| 8 | `src/resolution/mod.rs` doc | FIXED (docs, `c499287`) | True: `render_file` serves binary and denied files as they are. The doc says so. |
| 9 | `benches/index_settings.rs` | FIXED (docs, `c499287`) | Doc comment now says the budget is being raised from the proposed 40 ms to 60 ms by a human. No registry value, plan row, Justfile list or parity row was touched. |
| 10 | `fuzz/README.md` | FIXED (docs, `c499287`) | True: `check_flat` skips the decode and encode round trip for `.properties` (a path is the key as written). README row now says so, as `invariants.rs` already did. |
| 11 | `src/resolution/mod.rs` | FIXED (docs, `c499287`); behaviour unchanged | True as a fact and deliberate: a non-plain `${...}` is skipped whole only up to 256 bytes so the scan stays linear (AGENTS.md rule 5; `render.rs` already documented it). In a longer nested form the inner plain `${KEY}` is a placeholder. The module doc stated it without the bound and now states it. Whether to skip longer forms, or resolve nested ones as Spring does, is a plan A10 question, not changed here. |
| 12 | `src/parse/scalar.rs` | NOT A BUG | YAML 1.2.2 section 10.3.2 core schema: `[-+]? \. (inf\|Inf\|INF)` but `\. (nan\|NaN\|NAN)` with no sign. `+.NaN` and `-.NaN` are text; the code checks `text` for NaN and `body` for infinity on purpose. Pinned by `a_sign_belongs_to_infinity_and_not_to_nan` (green before and after) and a comment. |
| 13 | `Justfile` `engine-real-snapshot` | FIXED (`c499287`) | True. The recipe printed NOT RUN and exited 0. It now exits 1 (the recipe is opt-in and not in `verify-04`, and `real_snapshot.rs` already panics when unset). `tests/justfile_gate.rs` checks the line after NOT RUN is `exit 1`. |
| 14 | `src/effect.rs` | FIXED (`bc7458e`) | True for a tenant's copy (`channels-sit1.yml` has the logical name `channels.yml`). `docs/domain-model.md` says the list is "the base `channels.yml`", so only a base target warns. Test extended in `channels_yml_change_warns_that_the_config_server_must_restart`. |
| 15 | `src/batch/mod.rs` | FIXED (`8b4940a`) | True: duplicate paths were found inside `compute_lane`, after every blob was parsed. `recompute_all` and `findings_for_swimlanes` now build every swimlane's index first (`index_lanes`, the same sort, no extra work) and refuse before parsing. Test `every_input_is_checked_before_any_swimlane_is_computed`. |
| 16 | `src/batch/mod.rs` | FIXED (`8b4940a`) | True: `update_lane` built its own blob table per swimlane, so bytes held by forty swimlanes were parsed forty times. `recompute_paths` now plans every affected swimlane, parses the union of their blobs once, then finishes each. Unit test `a_blob_that_many_swimlanes_share_is_parsed_once_by_the_incremental_recompute` counts parses (40 before, 1 after). |
| 17 | `src/checks/c2.rs` | FIXED (`bc7458e`) | True. `flatten` leaves out `.properties` pairs with an empty or control-character key, and non-scalar YAML keys, so two files differing only in those had equal settings and C2 said "can be deleted". C2 now needs byte equality when either file left a pair out (or dropped warnings). The batch passes the warnings of parse and flatten (`Structured::parse_warnings` documents it). Test `c2_does_not_call_a_file_the_same_when_pairs_the_engine_left_out_differ`. |
| 18 | `fuzz/fuzz_targets/engine_path_mapping.rs` | FIXED (docs, `c499287`) | True: `MergeRules` went in T10. Header describes the real input and functions. |
| 19 | `fuzz/corpus/engine_json/bom_then_invalid` | FIXED (`c499287`) | True: the seed held 0xFF after the mark, so `text_of` dropped it. Now a mark plus `{"a" 1}`. Test `the_json_seed_with_a_mark_and_bad_json_reaches_the_parser`. The `latin1_bytes` seeds of `engine_yaml` and `engine_properties` have the same property by design (the byte-reading targets `engine_bytes` and `engine_index_settings` cover wrong encodings); the README says so. |
| 20 | `src/checks/c11.rs` | FIXED (`bc7458e`) | True: `channels` of the "folder not listed at the start" finding was sorted and deduplicated but not capped. Now capped like the folders. Test `..._every_list_is_capped_and_the_folders_are_counted` (also a cap of zero). |
| 21 | `benches/grid.rs` | FIXED (`c499287`) | True: the last tenant copy in directory order was chosen. Now the first in name order. |
| 22 | `src/batch/adoption.rs` | FIXED (`bc7458e`) | True: derived `Eq` was structural while `Ord` compared fractions (1/2 vs 2/4). `PartialEq` now uses `cmp`. Unit tests `equal_fractions_are_equal_and_compare_equal`, `eq_agrees_with_ord_for_every_pair`. |
| 23 | `fuzz/invariants.rs` | FIXED (`c499287`) | True: only the length was checked. `check_rendered_text` renders the template again with a marker for each key, cuts it into the pieces between placeholders, checks they rebuild the template, and checks the rendered text equals the pieces with the view's values. Planted harms (swapped values, a changed character, a dropped unresolved placeholder, a cut text) are reported by `planted_harm_is_detected`. The replay of all `engine_render` seeds passes with it. |
| 24 | `src/checks/c9.rs` | FIXED (`bc7458e`) | True: with `locations_per_finding = 0` the first folder was taken from the truncated list and the finding was dropped. Now it comes from the full set. Test `checks_c9_a_cap_of_zero_lists_nothing_and_still_reports_the_name`. |
| 25 | `src/checks/c9.rs` | FIXED (`bc7458e`) | True: the path was built from the logical name, so a folder with only `x-sit1.yml` gave `a/x.yml`, which is not a file. It is now the first copy that exists, as the doc said. Test `checks_c9_the_path_is_a_file_that_is_there_and_not_the_name_of_a_copy_that_is_not`. |
| 26 | `src/checks/c1.rs` | FIXED (`bc7458e`) | True: a list over a map and a map over a list were reported as dead overrides, but `effective` replaces in both directions. `Defined` now also knows the paths that have keys below them, and `is_override` mirrors `wipe` and `no_lists`. Test `c1_spring_merge_a_list_over_a_map_and_a_map_over_a_list_are_live`. |
| 27 | `src/checks/c6.rs` | FIXED (`bc7458e`) | True: `found.contains` per host, quadratic in the hosts of one value (a value holds thousands). Now a `BTreeSet` for membership; order is still first appearance. Test `c6_counts_each_host_of_a_value_once_however_often_the_value_names_it` passes before and after: the change is algorithmic and has no observable behaviour to turn red. |
| 28 | `benches/full_recompute.rs` | NOT A BUG / IRRELEVANT | Criterion's minimum is 10 samples and `bench-check` uses the nearest-rank percentile, which for 10 samples is the maximum: conservative, against a budget of 180 s for a 1.7 s run. More samples would only slow the gate. A comment says so (`c499287`). |
| 29 | `src/checks/c7.rs` | FIXED (contract, `bc7458e`) | The engine compares what it is given. `Overwrite::previous_nfs` did not say which hash it is; it is the N of the drift states, a `normalised_hash`, like `CheckFile::hash`. Documented, and pinned by `c7_compares_the_hash_for_comparing_with_the_hash_for_comparing` (CRLF against LF is silent). No engine behaviour changed. |
| 30 | `src/resolution/render.rs` | FIXED (`bc7458e`) | True: lines in a skipped expression counted `\n` only, so a lone `\r` was missed. One `ends_line` rule now serves both places. Test `lines_are_counted_the_same_inside_and_outside_an_expression_that_is_skipped`. |

## DEFERRED

None. Nothing needed a contract change, a plan change or a decision beyond the files already in scope.

## Notes for the human

- Item 11 and the `latin1_bytes` seeds are left as designed; the doc says what they do.
- `perf/budgets.toml`, the plan, `P6.index_settings`, its `engine_budgets` entry and its `budgets_registry.rs` row were not touched.

## Gates after the fixes (head `c499287`)

- `just verify-04`: PASS, exit 0 (lint, `engine-test`, all 10 engine fuzz targets 30 s each clean, the 3 `fuzz-smoke` targets, `engine-bench`).
- `just verify` (the Justfile changed): PASS, exit 0. Workspace 2,695 tests, 0 failed (2,676 before); `cargo deny` advisories, bans, licenses and sources ok; web checks pass.
- `cargo test -p engine --lib --tests`: 538 passed, 0 failed (519 before). `tests/determinism.rs` passes.
- `cargo xtask bench-check` on the full registry was not re-run: it needs every other prompt's benchmarks and has the 17 pre-existing FAIL rows
  the review recorded. The engine's own budgets (the `engine-bench` recipe) all pass.

Engine benchmarks, `just engine-bench`, p95 against the budget (before = the evidence file):

| Budget | Before | After | Budget |
|---|---|---|---|
| P6 full recompute | 1673.7 ms | 1532.8 ms | 180,000 ms |
| P6.parse_flatten | 11.7 ms | 14.3 ms | 20 ms |
| P7.compare | 26.7 ms | 22.3 ms | 100 ms |
| P7.engine_grid | 41.8 ms | 42.6 ms | 150 ms |
| P7.engine_text_search | 156.3 ms | 137.3 ms | 400 ms |
| P5.engine_base | 2.4 ms | 3.6 ms | 50 ms |
| P5.engine_per_file_per_swimlane | 0.10 ms | 0.15 ms | 2 ms |

The p95s of this machine move by that much from run to run (shared, 4 vCPU), so the two paths that changed were measured
again side by side, old binary (`712d339`) and new, alternating, three rounds, criterion means:

- `incremental_recompute_base`: old 1.85, 1.88, 1.71 ms; new 2.37, 1.87, 1.93 ms. At most 0.1 to 0.2 ms more, probably the three pool hand-offs (plan, parse, finish) in place of one for a single swimlane; no budget is near.
- `incremental_recompute_largest_file`: old 25.2, 26.3, 23.6 ms; new 27.4, 29.6, 24.5 ms.
- `incremental_recompute_root_file`: old 155.9, 138.1, 122.1 ms; new 112.2, 118.6, 121.0 ms.
- per pair of `per_file_per_swimlane`: old about 0.09 ms, new about 0.08 ms.
- `P6.parse_flatten` alone, three runs after the gate: mean 6.5 to 6.7 ms (the code on that path changed by one length comparison; the 14.3 ms p95 above was the machine).

P6.index_settings is unchanged: still `registered = false` at 40 ms in `perf/budgets.toml`, a human step.
