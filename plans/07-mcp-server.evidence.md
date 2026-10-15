# Evidence 07: MCP server for Cursor 3.22.12

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proved. Tree: `b5411fc` (T0 to T7, the housekeeping `07/fix:` commits, the merges of main, and two lockfile pins made in this step), plus this file.

## How the gates were run in this sandbox (read this first)

- **There is no Docker daemon.** `just verify-07` stops at its prerequisite `docker-check` (shown below) and `just verify` was not run as one command either. Their sub-recipes were run one by one, as listed. A test that prints `SKIPPED` did not run and is not counted as passed.
- **Postgres:** a throwaway PostgreSQL 16.15 on `127.0.0.1:54331`, **without pgvector**, trust auth. `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:54331/postgres` was set for `hub-mcp`, `hub-identity` and `xtask` (db_migrations), so their database tests really ran. In that mode the harness leaves out the pgvector block of `0001_init.sql`; **anything that needs pgvector is NOT PROVEN.** `hub-platform` and `hub` need the `vector` extension and fail on this server when the variable is set, so they ran with the variable **unset**: their database tests print `SKIPPED` and did not run (counts below). `LK_REQUIRE_DOCKER=1` was not set (it would fail the harness on this server).
- **Disk (7.6 GB free at the start):** `CARGO_INCREMENTAL=0`, one target directory, one crate at a time, linked test executables over 50 MB deleted after each crate, no `cargo clean`. The fuzz build went to its own `target/fuzz-build` (3.1 GB) and was removed when the fuzz run ended. The first run of `agent` and `hub-platform` as a whole crate failed with `No space left on device` while linking (the environment, not the code); both were re-run one test target at a time (`-j1`, executables deleted after each target) and passed.
- Raw logs of this step are in `/tmp/claude-0/-home-user-lanekeeper/8074a6e4-238f-56e1-b589-39d6f4890cd0/scratchpad/ev/` (not in the repository).
- `cargo test -p <crate>` unifies features per selected crate, not over the whole workspace as `cargo test --workspace` does. `ports` was also run with `--all-features`, as `ports-verify` does.

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-07` (as one command) | **FAILED at `docker-check`, in this sandbox only** (no Docker daemon). Nothing after it ran from the recipe; the sub-recipes below were run by hand. | `verify-01 needs a running Docker daemon: db-verify starts Postgres 16 with pgvector through testcontainers` / `error: recipe docker-check failed on line 48 with exit code 1` |
| `just verify` (as one command) | **NOT RUN as one command** (disk: a workspace-wide test build links hundreds of MB per binary). Its steps are the rows `fmt`, `clippy --workspace`, `cargo test` per crate, `cargo deny check`, `cargo audit` and `just web-verify` below. | |
| `cargo xtask bench-check` (full registry) | **FAILED, 21 pass, 25 fail, 26 unmet of 72 budgets.** All 21 P9 rows pass. The 51 other rows belong to other prompts: 25 are `registered but no result` (their benchmarks were never run here, `target/criterion` holds only `hub-mcp`) and 26 are `not registered; no benchmark yet`. None of them belongs to 07. | `bench-check (default): 21 pass, 25 fail, 26 unmet of 72 budgets` |

### The sub-recipes of `just verify-07`, one by one

| Recipe (what it runs) | Result | What did not run, and why |
|---|---|---|
| `mcp-tools-check` = `agent-tools-check` (`fuzz-tools-check`) + `docker-check` | `fuzz-tools-check` **passed** (nightly-2026-09-14, cargo-fuzz 0.13.2, clang). `docker-check` **FAILED** (no Docker daemon). | Docker is the only reason the gate does not complete. |
| `mcp-lint`: `cargo fmt --all -- --check`; `cargo clippy -p hub-mcp --all-targets -- -D warnings`; `cargo clippy -p hub --all-targets -- -D warnings` | **passed** (fmt exit 0; both clippy runs `Finished`, exit 0) | |
| `mcp-test`: `LK_REQUIRE_DOCKER=1 cargo test -p hub-mcp --lib --tests` | **passed, 468 passed, 0 failed** (lib 29, config 10, cursor_admin_doc 18, justfile_gate 7, mcp_auth 322, plugin_package 19, rmcp_spike 13, stub_protocol 24, t0_status 15, threat_model 11). Run with `LK_TEST_PG_ADMIN_URL` set and `LK_REQUIRE_DOCKER` unset. A re-run with `--nocapture` printed **0** `SKIPPED` lines, so the 11 `auth_pat_pg` tests (token store on a real PG 16) really ran. | Not run with `LK_REQUIRE_DOCKER=1` (no Docker); the same tests ran against the throwaway server instead. pgvector is not used by this crate. |
| `mcp-test`: `cargo test -p hub --test mcp_module` | **passed, 5 passed** (`mcp_routes_pass_the_hub_route_table_check`, `mcp_default_deny_holds`, `the_challenge_and_the_problem_body_survive_the_hubs_error_layer`, `the_hubs_log_filter_keeps_rmcp_at_warn`, `a_mcp_response_that_streams_longer_than_the_head_timeout_is_not_cut`) | |
| `mcp-fuzz`: `cargo metadata --locked`, `cargo deny --manifest-path crates/hub-mcp/fuzz/Cargo.toml check`, `cargo audit --file crates/hub-mcp/fuzz/Cargo.lock`, then 7 targets of 30 s | **passed**, run through `just mcp-fuzz` itself (with `CARGO_TARGET_DIR=target/fuzz-build`). Deny: `advisories ok, bans ok, licenses ok, sources ok`. Audit: `Scanning crates/hub-mcp/fuzz/Cargo.lock for vulnerabilities (460 crate dependencies)`, exit 0. Runs in 31 s each, no crash, `crates/hub-mcp/fuzz/artifacts/*` empty: `mcp_bearer_header` 2,027,332; `mcp_pat_token` 3,534,773; `mcp_tool_args` 1,083,834; `mcp_cursor` 1,372,519; `mcp_http_body` 14,435; `mcp_elicitation_result` 1,481,670; `mcp_redactor` 9,506. The first full run (before the `arbitrary` pin below) was also clean. | Only 30 s per target (the recipe's `fuzz_seconds`), not a long campaign. `mcp_elicitation_result` was run in T3 from a light scratch crate; **it now runs through `just mcp-fuzz` as part of the recipe**, so that gap is closed. The fuzz crate prints 3 dead-code warnings per target (it includes two source files with `#[path]`); the crate is not linted by the gate. |
| `mcp-bench`: `cargo bench -p hub-mcp`, then `cargo xtask bench-check` on a registry cut to the 21 P9 ids | **passed, 21 pass, 0 fail, 0 unmet of 21** (two full runs, on the tree before and after the lockfile pins; numbers below) | |
| `mcp-t0-status` | **ran, exit 0, prints `overall: NOT RUN`** | The Cursor 3.22.12 checks need a human. **NOT RUN**: Windows 0 of 25 cells recorded, macOS 0 of 25. See the acceptance list. |

### The steps of `just verify`

| Step | Result | Output (tail) |
|---|---|---|
| `cargo fmt --all -- --check` | passed | exit 0, no output |
| `cargo clippy --workspace --all-targets -- -D warnings` | passed | `Finished dev profile ... in 1m 30s`, exit 0 |
| `cargo test`, one crate at a time | **passed, except one known failure** (table below) | |
| `cargo deny check` | passed | `advisories ok, bans ok, licenses ok, sources ok` (20 `duplicate` warnings and 1 `license-exception-not-encountered`; the set of duplicated crates is identical to `origin/main`'s) |
| `cargo audit` | passed | `Scanning Cargo.lock for vulnerabilities (631 crate dependencies)`, exit 0 (advisory db: 1329 advisories) |
| `just web-verify` (pnpm install --frozen-lockfile, lint, typecheck, vitest, build) | passed | `Tests 1 passed (1)`; `dist/assets/index-O9gXnvRs.js 224.10 kB, gzip 69.65 kB`; exit 0 |
| `just tools-check` | passed | `cargo-deny 0.19.9`, `cargo-audit 0.22.2`, buf 1.73.0, redocly 2.53.3 |
| `osv-scanner` (AGENTS.md rule 15, run in CI) | **NOT RUN** | not installed here, and not part of `just verify` |

`cargo test` per crate (`CARGO_INCREMENTAL=0`):

| Crate | PG variable | Result | Not run / notes |
|---|---|---|---|
| `domain` | unset | 105 passed | |
| `proto` | unset | 25 passed | |
| `engine` | unset | 538 passed, 7 ignored | the 7 `#[ignore]` tests did not run |
| `ports` | unset | 149 passed; `--all-features`: 150 passed | |
| `hub-registry`, `hub-writes`, `hub-docsearch`, `sentinel` | unset | build, 0 tests | these crates have no tests yet |
| `agent` | unset | 742 passed, 1 ignored, 55 cargo invocations (lib, each test target, doc), run one target at a time | the ignored test did not run; `agent` has 0 `SKIPPED` lines |
| `hub-identity` | set | 544 passed, 2 ignored | 442 `SKIPPED` lines are notices (`the server has no pgvector; running without it (nothing in this crate needs it)`): the tests ran against the real PG 16. The 2 ignored did not run. |
| `xtask` | set | **138 passed, 1 FAILED, 1 ignored** | the failure is `db_migrations::folder_listings_use_the_prefix_indexes`: the planner/collation difference of this server (the `EXPLAIN` shows a filter `path ~~ 'dir7/%'` where an index condition is expected). **It fails on `origin/main` too and is not 07's.** In `db_migrations` (65 passed, 1 failed) 53 tests print `pgvector checks are NOT PROVEN`: those checks did not run. |
| `hub-platform` | unset | 512 reported passed, 5 ignored, 48 cargo invocations (lib, each test target, doc), run one target at a time | **161 of the 512 printed `SKIPPED` (`cannot start the Postgres container: ... docker.sock`) and did not run: at most 351 really ran.** The database tests of this crate need Docker or pgvector. The 5 ignored did not run. |
| `hub` | unset | 129 passed, 0 failed | 1 `SKIPPED` line (`pg_tls`: a test that needs a Docker Postgres with TLS did not run). The other 128 ran, among them the 5 of `mcp_module`. |
| `hub-mcp` | set | 468 passed (row above); doc tests: 0 tests, exit 0 | |

## Acceptance checklist

From the `Acceptance` section of `prompts/07-mcp-server.md`.

- [ ] **T0 results are recorded, and the modes are set in config.** **NOT RUN (human part).** The Cursor 3.22.12 compatibility test on Windows and macOS (Entra OAuth with the fixed client ID, elicitation accept/decline/cancel, the tool allowlist) has not been done: `just mcp-t0-status` prints `overall: NOT RUN`, 0 of 25 cells on each operating system (D91 in `docs/decisions.md`, runbook `crates/hub-mcp/t0/README.md`). An agent cannot run Cursor and never fills a cell. What *is* proved, and does not tick the box: the modes exist and default as D91 says (`config::defaults_are_oauth_elicitation_summary`, `config::both_auth_modes_at_once_is_a_config_error`, `config::unknown_value_is_a_config_error`, `config::every_documented_value_is_accepted`); the server side of the T0 stub is exercised in process (`stub_protocol::*`, 24 tests, including `elicitation_accept_decline_cancel_all_complete`); the status tool reads the table (`t0_status::*`, 15 tests).
- [ ] **`just verify-07` passes.** **Not proved.** The recipe stops at `docker-check` in this sandbox. Its other sub-recipes passed (table above); the Docker daemon and the T0 human checks remain.
- [x] **P9 is within budget for every read tool.** Proof: `just mcp-bench`, twice, 21 pass of 21 (`P9`, `P9.list_swimlanes` ... `P9.get_served_config`), each p95 far below 300 ms (table below). These are this layer's share on fakes at target scale, as the budgets are registered.
- [x] **All authentication, confirmation and role tests pass.** Proof: `cargo test -p hub-mcp --lib --tests`, 468 passed, 0 failed, with the personal-token store on a real PG 16. By suite: authentication `auth_oauth::*` (22), `auth_pat::*` (22), `auth_pat_pg::*` (11, real Postgres), `auth_header::*` (8), `token_syntax::*` (11), `token_routes::*` (12); confirmation `confirm::*` (63; accept, decline, cancel, client without elicitation, draft mode, `requires_approval`, timeout, disconnect, idempotency) and `confirm_golden::*` (12); role `tool_matrix::*` (5), `route_table::*` (7), plus `crates/hub/tests/mcp_module.rs` (5).

## Security requirements implemented

All proofs below ran in this step (names as they appear in the test output). The plan's traceability table names two tests, `docs_tree::read_doc_takes_only_doc_paths` and `docs_tree::dotdot_and_backslash_are_refused`, that do not exist under those names; the same cases (`/etc/passwd`, `git/main/x`, `/git/../x`, `/git/main/.git/config`, `/uploads/`, `/git/a\b` for `read_doc`) are in `args::every_path_argument_uses_a_domain_parser`.

- **S4** (declared role on every route and tool, default deny, pending and disabled refused): `src/server` guards and `src/tools` role floor. `route_table::{every_mcp_route_has_a_declared_guard, every_method_of_the_endpoint_is_gated, routes_agree_with_openapi_and_allowances, unmatched_path_under_mcp_is_default_deny, the_routes_of_each_mode_are_exactly_these}`; `contract_schemas::{every_documented_tool_has_a_handler_and_every_handler_a_spec, advertised_roles_confirmations_caps_match_the_document, embedded_schemas_equal_the_contract_document}`; `tool_matrix::{every_tool_by_every_user_state, role_is_checked_before_arguments_are_parsed, tools_list_shows_only_callable_tools, unknown_tool_is_a_protocol_error, a_role_change_takes_effect_on_the_next_call}`; `auth_oauth::{pending_user_is_403, disabled_user_is_403, role_change_takes_effect_on_next_call}`; `confirm::editor_cannot_call_restart_service`; hub: `mcp_module::{mcp_routes_pass_the_hub_route_table_check, mcp_default_deny_holds}`.
- **S11** (typed inputs, bounded sizes, linear-time regex only): `src/tools/args.rs`, `src/output/cursor.rs`. `args::{every_path_argument_uses_a_domain_parser, content_over_2_mib_of_bytes_is_payload_too_large, pattern_over_512_bytes_is_invalid_argument, swimlane_lists_over_64_are_invalid_argument, unknown_field_is_invalid_argument, a_null_for_an_optional_argument_is_refused, ids_and_hashes_are_held_to_the_schemas_patterns, typed_args_never_accept_what_the_schema_rejects_prop, valid_examples_deserialize_into_typed_args, arbitrary_json_never_panics_the_parser}`; `auth_oauth::token_longer_than_max_is_refused_before_parsing`; `server::oversized_body_is_413`; `cursor::*` (7); fuzz `mcp_tool_args`, `mcp_cursor`, `mcp_bearer_header`, `mcp_http_body` (ran, clean).
- **S14** (token checks, role per tool, server-enforced elicitation, capped labelled output, per-user limits, no shell): `auth_oauth::{missing_header_is_401_with_resource_metadata_challenge, invalid_token_is_401, expired_token_is_401, wrong_audience_is_401, token_without_mcp_scope_is_401, unavailable_verifier_is_503_not_401, unknown_user_is_401, session_id_is_bound_to_the_user, foreign_origin_is_refused}`; `auth_pat::*` and `auth_pat_pg::*` (invalid, expired, revoked, pending-user and wrong-mode tokens); `confirm::{accept_with_confirm_true_applies_once, accept_with_confirm_false_is_declined_and_writes_nothing, decline_writes_nothing, cancel_writes_nothing, timeout_cancels_and_writes_nothing, client_without_elicitation_gets_draft_link_and_no_write, draft_mode_never_calls_a_write_method, requires_approval_user_gets_proposal_created_after_confirm, an_answer_from_another_session_is_ignored, the_gate_is_the_only_caller_of_the_mutating_write_methods, what_the_user_confirmed_is_what_happens_even_if_the_approval_rule_changes_meanwhile}`; `output_cap::{no_read_tool_exceeds_16kb_at_worst_case_items, write_tools_stay_under_4kb, paging_is_exact_prop, paging_is_exact_at_the_edges, truncated_sets_next_or_web_link, read_file_default_is_200_lines_or_16kb}`; `limits::{rate_limit_is_per_user, rate_limited_carries_retry_after, write_class_has_its_own_budget, concurrency_cap_per_user, in_flight_bytes_are_bounded, port_timeout_is_unavailable}`; `source_rules::{no_shell_or_process_in_src, no_unbounded_channel_in_src, no_blocking_fs_in_src}`.
- **S14b** (redaction, no reveal over MCP, denied files): `src/output/redact.rs`. `planted_key::{planted_key_never_appears_in_any_tool_output, a_key_cut_by_the_line_limit_of_a_window_is_hidden_in_both_modes, no_tool_input_can_reveal_a_flagged_value, late_sweep_catches_a_field_the_typed_path_missed, the_sweep_fails_closed_when_it_cannot_scan, the_sweep_is_idempotent_and_leaves_nothing_for_the_scan}`; `denied_files::{denied_file_returns_content_withheld_for_every_source, get_effective_config_refuses_a_value_that_comes_from_a_denied_file, search_text_drops_hits_in_denied_files, writes_to_a_denied_file_are_refused_before_anything_is_asked, get_served_config_withholds_a_denied_file_and_lists_nothing_else_of_it}`; `served::get_served_config_is_redacted`; `source_rules::{every_text_field_of_an_output_type_is_redacted_text, redacted_text_is_made_only_in_redact_rs}`; `context_fields::attribution_and_severity_present_in_history_drift_and_list_files`; fuzz `mcp_redactor` (ran, clean). Residual: Q20 (the scan's false negatives, six real files) blocks go-live.
- **S15** (config and doc text is data): `untrusted::{untrusted_flag_and_label_on_every_text_tool, server_instructions_state_the_data_rule, data_block_delimiter_cannot_be_forged, data_block_nonce_is_fresh_per_response, summary_never_contains_file_text_prop, in_the_default_rendering_content_holds_no_text_from_a_file}`; `confirm::confirmation_message_neutralises_untrusted_text`.
- **S21** (no unsafe, no panics, no leaked internals, no secrets in logs): `source_rules::{forbid_unsafe_present, nothing_secret_is_formatted_into_a_message}`; workspace clippy lints in `mcp-lint` (passed); `errors::{port_errors_map_to_the_contract_codes, error_messages_are_fixed_text_not_input, write_outcomes_map_to_results_and_errors, auth_errors_map_to_the_statuses_the_hub_guard_gives}`; `logging::{log_capture_has_no_tokens_args_or_output, debug_of_write_args_prints_no_content, debug_output_of_the_secret_bearing_types_is_redacted, tool_calls_log_what_was_called_and_nothing_it_carried, every_call_line_carries_the_stable_user_id_and_nothing_else_about_the_caller}`; `confirm::a_whole_confirmation_life_logs_what_happened_and_nothing_it_carried`.

## Benchmarks against budgets

Method: `just mcp-bench` (criterion, the real server in process over fakes at target scale, caches off for `read_file`), then `cargo xtask bench-check` on the P9 registry. p50, p95 and p99 are computed from criterion's `sample.json` (100 samples per bench, each sample the mean of its iterations) with the same nearest-rank rule as `xtask::bench_check::percentile`. 4 shared vCPU, release build, load average about 1.8 from other sessions during the runs. This is the second full run, on the final tree. The registered statistic is p95, budget `<= 300 ms`; no threshold in `perf/budgets.toml` was changed.

| P# | Benchmark | p50 (ms) | p95 (ms) | p99 (ms) | Budget | Result |
|---|---|---|---|---|---|---|
| P9 | `hub-mcp/read_tools` (rotation over all 20 read tools) | 4.016 | 5.979 | 6.055 | p95 <= 300 ms | PASS |
| P9.list_swimlanes | `hub-mcp/list_swimlanes` | 0.388 | 0.537 | 0.622 | p95 <= 300 ms | PASS |
| P9.get_swimlane | `hub-mcp/get_swimlane` | 0.502 | 0.707 | 0.730 | p95 <= 300 ms | PASS |
| P9.list_files | `hub-mcp/list_files` | 0.520 | 0.706 | 0.825 | p95 <= 300 ms | PASS |
| P9.read_file | `hub-mcp/read_file` (cold, largest file, see D92) | 37.343 | 62.879 | 65.532 | p95 <= 300 ms | PASS |
| P9.get_effective_config | `hub-mcp/get_effective_config` | 0.807 | 1.196 | 1.247 | p95 <= 300 ms | PASS |
| P9.compare | `hub-mcp/compare` | 0.861 | 1.319 | 1.359 | p95 <= 300 ms | PASS |
| P9.compare_tree | `hub-mcp/compare_tree` | 0.364 | 0.543 | 0.592 | p95 <= 300 ms | PASS |
| P9.compare_grid | `hub-mcp/compare_grid` | 3.701 | 5.528 | 5.986 | p95 <= 300 ms | PASS |
| P9.search_settings | `hub-mcp/search_settings` | 0.665 | 0.883 | 1.042 | p95 <= 300 ms | PASS |
| P9.search_text | `hub-mcp/search_text` | 0.870 | 1.202 | 1.450 | p95 <= 300 ms | PASS |
| P9.find_inconsistencies | `hub-mcp/find_inconsistencies` | 0.706 | 0.913 | 1.108 | p95 <= 300 ms | PASS |
| P9.get_drift | `hub-mcp/get_drift` | 0.740 | 0.837 | 1.086 | p95 <= 300 ms | PASS |
| P9.get_pending_restarts | `hub-mcp/get_pending_restarts` | 0.478 | 0.751 | 0.795 | p95 <= 300 ms | PASS |
| P9.get_history | `hub-mcp/get_history` | 0.539 | 0.784 | 0.866 | p95 <= 300 ms | PASS |
| P9.search_docs | `hub-mcp/search_docs` | 0.496 | 0.665 | 0.750 | p95 <= 300 ms | PASS |
| P9.read_doc | `hub-mcp/read_doc` | 1.833 | 2.281 | 2.429 | p95 <= 300 ms | PASS |
| P9.grep_docs | `hub-mcp/grep_docs` | 0.777 | 1.080 | 1.284 | p95 <= 300 ms | PASS |
| P9.get_docs_for_file | `hub-mcp/get_docs_for_file` | 0.506 | 0.650 | 0.879 | p95 <= 300 ms | PASS |
| P9.get_feature_status | `hub-mcp/get_feature_status` | 1.350 | 1.454 | 1.512 | p95 <= 300 ms | PASS |
| P9.get_served_config | `hub-mcp/get_served_config` | 2.409 | 3.501 | 3.654 | p95 <= 300 ms | PASS |
| (not a budget) | `hub-mcp/read_file_warm` (cache on, the normal case) | 1.797 | 2.385 | 2.473 | none | informational |

- **Cold `read_file` (D92a):** accepted by the human. Cold p95 was 46.1 ms in the first run (p50 35.8, p99 49.1) and 62.9 ms in the second (p50 37.3, p99 65.5) on the same code: the spread of a shared machine, in line with the 52 to 63 ms of D92. The cost is `engine::index_settings` (its own accepted budget `P6.index_settings` is 60 ms), paid once per file version per replica. The plan's internal 50 ms screen does not apply to this call; the registered 300 ms budget passes with a wide margin. Warm calls take about 2 ms.
- **No threshold changed:** `git diff origin/main..HEAD -- perf/budgets.toml` is 21 lines `registered = false` replaced by `registered = true` and nothing else.
- Every other tool has p99 below 7 ms (largest: `compare_grid`, 6.0 ms).
- The first run is the one before the two lockfile pins below; the pins touch only dev-only and fuzz-only crates, and the second run is on the final tree.

## New dependencies

Compared with `origin/main`: `Cargo.lock` gains 23 packages and changes none (`git show origin/main:Cargo.lock` against the branch); `crates/hub-mcp/fuzz/Cargo.lock` is a new lockfile of the fuzz crate. Dates and licences are from the crates.io API on 2026-10-15 (reachable from this sandbox). The rule is a release at least 14 days old.

| Crate or package | Version | Age in days | Why | `cargo deny` / `osv-scanner` |
|---|---|---|---|---|
| `rmcp` | =3.5.0 (2026-09-28, Apache-2.0, MSRV 1.88) | 17 | The MCP SDK of the fixed stack (AGENTS.md). Features `server`, `transport-streamable-http-server`, `elicitation`; no `macros`, `auth`, `client` or `reqwest` (the handler is written by hand so no schema is derived from a Rust type). 3.5.1 (10 days) was refused for its age. | deny ok, audit ok / osv-scanner NOT RUN |
| `pastey` | 0.2.3 (2026-05-20, MIT OR Apache-2.0) | 148 | proc-macro dependency of rmcp | deny ok / NOT RUN |
| `schemars_derive` | 1.2.2 (2026-07-27, MIT) | 80 | derive half of `schemars`, enabled by rmcp | deny ok / NOT RUN |
| `serde_derive_internals` | 0.30.0 (2026-07-25, MIT OR Apache-2.0) | 82 | dependency of `schemars_derive` | deny ok / NOT RUN |
| `sse-stream` | 0.2.6 (2026-09-03, MIT OR Apache-2.0) | 42 | server-sent-event framing of rmcp's streamable HTTP transport | deny ok / NOT RUN |
| `jsonschema` | =0.58.3 (2026-09-30, MIT) | 15 | **dev-dependency only.** Validates tool inputs and results against the schemas of `docs/mcp-tools.md` with a validator that is not ours (T2, S11). Default features off (they add an HTTP client and a TLS stack). 0.58.4 to 0.58.6 (9 to 14 days) were refused for their age. | deny ok / NOT RUN |
| `referencing`, `jsonschema-regex`, `jsonschema-value` | **0.58.3** (2026-09-30, MIT) | 15 | dev-only, required by `jsonschema` as `^0.58.3`. **They had resolved to 0.58.6 (2026-10-06, 9 days old, under the rule); fixed in this step by `07/fix: pin referencing, jsonschema-regex and jsonschema-value to 0.58.3` (`cargo update --precise`).** | deny ok / NOT RUN |
| `borrow-or-share` 0.2.4, `bytecount` 0.6.9, `email_address` 0.2.9, `fancy-regex` 0.19.2, `fluent-uri` 0.4.1, `fraction` 0.17.0, `micromap` 0.3.0, `num-cmp` 0.1.0, `outref` 0.5.2, `strum` 0.28.0, `strum_macros` 0.28.0, `unicode-general-category` 1.1.0, `uuid-simd` 0.8.0, `vsimd` 0.8.0 | as listed | 347, 499, 806, 32, 345, 54, 218, 3514, 638, 235, 235, 394, 1387, 1387 | dev-only, all transitive of `jsonschema` (`cargo tree -p hub-mcp -e dev`). Not in the runtime tree of the hub. Licences MIT, Apache-2.0 or both. | deny ok / NOT RUN |
| `arbitrary` (fuzz crate lockfile only) | **1.4.2** (2025-08-14) | 427 | transitive of `libfuzzer-sys` 0.4.13 in `crates/hub-mcp/fuzz`. **It had resolved to 1.5.0 (2026-10-06, 9 days old); fixed in this step by `07/fix: pin arbitrary to 1.4.2 in the hub-mcp fuzz lockfile`.** 1.4.2 is the version the root `fuzz/Cargo.lock` already uses. | deny ok, audit ok / NOT RUN |

Other changes to manifests: workspace entries `engine` and `hub-mcp` (path crates), and `hub-mcp.workspace = true` in `crates/hub/Cargo.toml` (an approved Extra-path). The runtime new crates are the five of rmcp's tree; the fuzz lockfile has 431 packages, of which only `rmcp`'s five, `arbitrary`, `libfuzzer-sys` and the fuzz crate are absent from `origin/main`'s lockfiles. `osv-scanner` is not installed here: NOT RUN.

## Assumptions

Accepted by the human for now:

- **D-1:** `/mcp` is a self-authenticating `public` route, not `Role(Viewer)`, because `GuardedRouter` cannot add the `WWW-Authenticate` challenge to its own 401; a 3-line `GuardedRouter::layer` in `hub-platform` would restore it.
- **D-2:** S3 for the personal-token routes is Origin-check only; the double-submit half depends on the hub composing 03a's CSRF layer.
- **D92a:** cold `read_file` of the largest file has p95 about 52 to 63 ms (46 to 63 ms in the two runs of this step) against the plan's 50 ms internal screen; accepted. The registered 300 ms budget passes.
- **D92b:** `get_effective_config` returns `text` null and typed values `other` until a later contract PR.

Choices the plan did not fix:

- Draft mode, and a client without the form capability, do not check that the draft exists.
- After the answer the gate re-reads the user and refuses if role, status or `requires_approval` changed.
- Tools that ask the user get a 185 s deadline.
- The replay map stores only successful writes.
- Denied-file outcomes: `unprocessable` with reason `denied` for `get_effective_config` and `compare_grid`; `class: denied` for `compare`; search hits in denied files are dropped; write tools refuse a denied path before drafting; `apply_change` does not check its draft's path; `notify_services` does not refuse a denied path.
- The cached scan lives in `tools/cache.rs`, not `output/scan_cache.rs`.
- The proposal summary says "Nothing was done yet." instead of "Nothing was written yet."
- `tools.notify_services.enabled` has no env key in `McpConfig::from_lookup`, and the hub does not mount `/mcp` yet.
- The plan's `docs_tree::*` test names were not used; the docs-path cases are in `args::every_path_argument_uses_a_domain_parser`.

For other prompts and humans (follow-ups):

- Prompt 09 must give `Mcp-Session-Id` affinity, or deployers set `confirm_mode = draft`.
- Prompt 06 owns the cap and the audit of `propose_change` drafts.
- Prompt 05 decides where the hash of a denied served file comes from.
- The hub integration must mount `/mcp`, compose 03a's CSRF layer and give `notify_services` an on-switch.
- The rmcp allowed-origin-without-port warning must be fixed (`:*` or an explicit port) before any rmcp bump.
- The plugin package includes the dev-only `evals/evals.json` of prompt 11's skill, and the skill is still a draft.
- Q20 (six real files for scan tuning) blocks go-live.
- The Cursor plugin format is UNVERIFIED (A9).
- `docs/interfaces.md` does not mention the new port methods (out of scope).
- **T0 on Cursor 3.22.12 (Windows and macOS) is NOT RUN** and a human must record it before go-live (`just mcp-t0-status --strict`).
- Docker is needed to run `just verify-07` whole and to prove the pgvector-dependent checks; this step proved neither.

## Deviations from the plan

None in scope or tasks. Two housekeeping commits in this step fixed lockfile entries below the 14-day rule (above). The commits of the branch since `origin/main` (this evidence file is committed after them as `07/evidence`):

```
git log --format='%h %s' origin/main..HEAD
b5411fc 07/fix: pin arbitrary to 1.4.2 in the hub-mcp fuzz lockfile (9 days old at 1.5.0)
ff8f449 07/fix: pin referencing, jsonschema-regex and jsonschema-value to 0.58.3 (9 days old at 0.58.6)
e650f2e 07/T7: served view and notify tools
b9d2c6c 07/T6: redaction and context
37123c6 07/fix: pin that a refused write is not stored in the replay map
23499de 07/fix: size the session owner table from the replica session cap
8de5861 07/fix: the per-call log line names the caller by stable user id
1d228df 07/T5: threat model
c7bae01 07/T4: distribution
5916647 07/T3: confirmation
3b5533a 07/decision: record the human's decisions on the T2 blockers
ffd02fe 07/fix: regenerate the tool schemas after the contract follow-up
6a97301 Merge main (contract/07-review-fixes, PR #14) into agent/07-mcp-server
d1d8969 07/blocked: cold read_file over the 50 ms screen, effective-config gaps
fea806d 07/T2: tools, output pipeline and limits
3d72ccf 07/T1: authentication
095c1aa Merge main (contract/07-mcp, PR #13) into agent/07-mcp-server
c75e518 07/T0: compatibility stub, runbook and mode switches
eeceade 07/plan: approved by a human
7472187 07/plan: draft
```

Contract paths: no contract path other than `perf/budgets.toml` was touched (its 21 P9 entries flipped to `registered = true`, approved by the human). Proof: `git diff --name-only origin/main..HEAD | grep -E '^(crates/domain/|crates/ports/|proto/|api/openapi.yaml|db/migrations/|docs/mcp-tools.md|perf/budgets.toml|design/handoff/)'` prints only `perf/budgets.toml`. The other files outside `crates/hub-mcp/` are the shared `Cargo.toml`, `Cargo.lock` and `Justfile`, the plan's approved Extra-paths (`crates/hub/Cargo.toml`, `crates/hub/src/lib.rs`, `crates/hub/src/mcp.rs`, `crates/hub/tests/mcp_module.rs`, `crates/xtask/tests/budgets_registry.rs`, `docs/decisions.md`, `docs/threat-model.md`), `docs/cursor-admin.md` (owned), and `plans/07-mcp-server.md` and `plans/07-mcp-server.blockers.md`. `skills/` was not edited.
