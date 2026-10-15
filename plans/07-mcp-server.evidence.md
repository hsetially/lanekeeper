# Evidence 07: MCP server for Cursor 3.22.12

Written by the implementer in the FINAL step and updated in review rounds 1 and 2. Real output only; nothing is ticked that was not proved. Tree of the FINAL step: `b5411fc`. Tree after review round 1: `b347dd8`. **Tree after review round 2: `dc299b0`** (see "Review round 2" right below "Review round 1"). A row elsewhere that says nothing about round 2 was measured on an earlier tree; the "Review round 2" section says what that round touched (the session gate, the three port calls, `get_served_config`, `get_effective_config`'s summary, the question for `raise_pr`, and test code) and what it re-ran.

## How the gates were run in this sandbox (read this first)

- **There is no Docker daemon.** `just verify-07` stops at its prerequisite `docker-check` (shown below) and `just verify` was not run as one command either. Their sub-recipes were run one by one, as listed. A test that prints `SKIPPED` did not run and is not counted as passed.
- **Postgres:** a throwaway PostgreSQL 16.15 on `127.0.0.1:54331`, **without pgvector**, trust auth. `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:54331/postgres` was set for `hub-mcp`, `hub-identity` and `xtask` (db_migrations), so their database tests really ran. In that mode the harness leaves out the pgvector block of `0001_init.sql`; **anything that needs pgvector is NOT PROVEN.** `hub-platform` and `hub` need the `vector` extension and fail on this server when the variable is set, so they ran with the variable **unset**: their database tests print `SKIPPED` and did not run (counts below). `LK_REQUIRE_DOCKER=1` was not set (it would fail the harness on this server).
- **Disk (7.6 GB free at the start):** `CARGO_INCREMENTAL=0`, one target directory, one crate at a time, linked test executables over 50 MB deleted after each crate, no `cargo clean`. The fuzz build went to its own `target/fuzz-build` (3.1 GB) and was removed when the fuzz run ended. The first run of `agent` and `hub-platform` as a whole crate failed with `No space left on device` while linking (the environment, not the code); both were re-run one test target at a time (`-j1`, executables deleted after each target) and passed.
- Raw logs of this step are in `/tmp/claude-0/-home-user-lanekeeper/8074a6e4-238f-56e1-b589-39d6f4890cd0/scratchpad/ev/` (not in the repository).
- `cargo test -p <crate>` unifies features per selected crate, not over the whole workspace as `cargo test --workspace` does. `ports` was also run with `--all-features`, as `ports-verify` does.

## Review round 1 (reviewer verdict CHANGES REQUESTED at `7b4677a`, report in `plans/07-mcp-server.review.md`)

The reviewer found no code defect. **Blocking items 1 and 2 are not code and are NOT resolved: they stay open as explicit gates** (last list of "Assumptions"). Nothing was done to work around them.

| Commit | What it does | Review item |
|---|---|---|
| `426a5cb` | Merge of `origin/main` (PR #16, D107). `docs/decisions.md` conflicted at the end of the file: D91, D92 (this branch) and D107 (main) are all kept, in that order. `docs/security.md` came from main untouched. | merge request of the round |
| `3b92e8d` | **D109** in `docs/decisions.md` records D-1 and D-2: who accepted (the human who ran `/run-prompt 07`, GitHub user hsetially), when (2026-10-15, after T1 `3d72ccf`), the condition for D-2 (the hub composition wraps every non-GET route, `/api/v1/me/mcp-tokens` included, in 03a's `CsrfLayer`, and `crates/hub/tests/mcp_module.rs` then asserts it) and the reversal for D-1 (a three-line `GuardedRouter::layer` in `hub-platform`; only `server/guard.rs` changes). The rows and residual risks of the MCP section of `docs/threat-model.md` name D109. The number is D109 and not D108 because `origin/agent/05-registry-read-api` already uses D108. | blocking 3 |
| `ee743e5` | The `raise_pr` question shows the title and a quoted start of the description with its length (below). | non-blocking 8, decided by the human |
| `b347dd8` | Hygiene: a typed digest length instead of `copy_from_slice`; the test hooks gated; the gate-only-caller check made robust, with controls; the `tools/list` bound tightened. | non-blocking 10, 5, 7 |

**The description in the `raise_pr` question (`ee743e5`).** What the user reads now: `Description (supplied by the assistant), N characters, only the start: "..."` (or `N characters: "..."` when it fits whole). Choices, for the human to confirm:

- **Prefix length: 240 characters** inside the quotes (`BODY_CHARS`; an escape counts as two), less when the other lines and the link leave less of the question's budget (`MAX_CHARS` 1,500 less a margin of 100, so the guard in `finish` never decides what is read), and **none below 24**, where the line says `none of it fits here`. So the cap and the link hold for any title, path, name and link up to 512 characters (`the_start_of_the_description_gives_way_to_the_link_and_the_cap`, `the_worst_case_of_every_message_fits_and_keeps_its_link`).
- **The description is scanned for secrets first** (S14b), with the same `Redactor` as every other text: a value found is the engine's marker and the line says `1 secret-looking value hidden`; if the scan cannot finish, none of it is shown. The task did not ask for this; the PR body is typed `Opaque` ("may hold a secret") in this crate, and a question is output.
- **`quote()` now also escapes `<`, `>`, `&`, `[`, `]` and the backtick** (a link, an image, a tag, an entity and a code span each need one), for **every** quoted field: titles, paths, branch names and service names as well as the description. Names without those characters (underscores included) are unchanged. In a client that shows plain text the escapes are visible as backslashes. Bare addresses are not defused (they show their whole target as text); that and the fact that the rest of the description is posted unread are in the threat model's new residual risk.
- Goldens: `raise_pr.snap` (the description is shown), `hostile_quote.snap` (`\[31m`), and two new ones, `raise_pr_long_body.snap` and `raise_pr_hostile_body.snap`. The other 11 are unchanged. Test order, honestly: the unit tests of `message.rs` were written before the code (the first build failed on the items that did not exist yet); the golden tests and `confirm::confirmation_message_quotes_a_cleaned_start_of_the_pull_request_description` were written after it, with the goldens first run in no-update mode (4 failed against the old snapshots) before they were accepted and read line by line. To show that the tests can fail: removing the markup escapes made 3 unit tests and 4 integration and golden tests fail, and ignoring the room left for the line made the 2 size tests fail; the unmutated code was restored each time.

**Hygiene (`b347dd8`).**

- `HmacKey::lookup` returns `Result<TokenHmac, DigestLength>`; `TokenHmac::from_digest` uses `try_from`, so a wrong length is a typed error and no longer a panic (rule 2). `create` maps it to `TokenError::Internal`, `authenticate` to `AuthError::Unavailable` (fail closed). Test: `token_syntax::a_digest_of_the_wrong_length_is_an_error_and_not_a_panic`. The fuzz invariants of `mcp_pat_token` follow the new signature.
- Test hooks: `McpState::forget_session_owners` is `#[cfg(feature = "fakes")]` (the feature this crate's own dev-dependency turns on and no normal dependency of the hub does; its one user is an integration test). `Replay::run_pending_tasks` and `Replay::weighted_size` (a third hook of the same shape) are `#[cfg(test)]` (their one user is a unit test of `replay.rs`). `source_rules::a_hidden_test_hook_exists_only_in_test_builds` fails on any `#[doc(hidden)]` item without one of those gates, and failed on the ungated code before the change; `the_check_for_ungated_hooks_finds_what_it_should` holds the positive controls. No structural change was needed.
- `confirm::the_gate_is_the_only_caller_of_the_mutating_write_methods` reads the whole text of `src/` (comments and strings blanked), finds a chain that rustfmt broke over lines, an alias and a `WriteService::` path call, and exempts only `src/confirm/gate.rs` (before: all of `src/confirm/`). `confirm::the_scan_for_mutating_calls_finds_every_shape_and_only_those` proves it fails for each shape, including a multi-line call added to the real tree, and that the scan is not vacuous (it finds the gate's six calls). **The type-level handle (a narrow `ToolHost` field without the mutating methods) was not done**; it is a follow-up.
- `contract_schemas::tools_list_size_is_reported`: measured **112,340 bytes** for an Admin (26 tools, result schemas on) and **31,235** without result schemas; the bounds are now 100,000 to 125,000 and 28,000 to 35,000 (they were 20,000 to 400,000).

**Findings looked at and not acted on in code.** None was rejected as wrong; I checked each against the code first (the `copy_from_slice` panic, the two `pub` hooks, the `writes`/`.upload(` split over two lines at `gate.rs`, item 7 gave about 22 KB of input schemas and 59 KB of result schemas, and I measured 112,340 bytes for the whole list, so the bound uses the measurement). Non-blocking 1 to 4 and 11 are the reviewer's own accepts. Item 6 (S15 "delimited blocks" only in `data_block` mode) and the `advertise_output_schemas` default of item 7 wait for T0 (follow-ups below). Item 9 (goldens of the 401 challenge, `tools/list` per role and the cursor, and the two plan test names) is a follow-up. The notify on-switch and `evals/evals.json` of item 10 belong to the hub integration and to prompt 11.

**What was re-run on `b347dd8`, and what was not.**

| Command | Result |
|---|---|
| `cargo test -p xtask --test mcp_tools_doc --test budgets_registry` | **passed**: 23 and 8 (also after the merge) |
| `cargo fmt --all -- --check` | passed, exit 0 |
| `cargo clippy -p hub-mcp --all-targets -- -D warnings`; `cargo clippy -p hub --all-targets -- -D warnings`; `cargo clippy -p hub-mcp -- -D warnings` (the build without dev features) | passed, `Finished`, no warning |
| `cargo test -p hub-mcp --lib --tests` with `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:54331/postgres` (PG 16.15, no pgvector) | **passed, 482 passed, 0 failed, 0 ignored** (lib 36, config 10, cursor_admin_doc 18, justfile_gate 7, mcp_auth 329, plugin_package 19, rmcp_spike 13, stub_protocol 24, t0_status 15, threat_model 11); +14 on the 468 of the FINAL step (lib +7, mcp_auth +7). 0 `SKIPPED` lines. |
| `cargo test -p hub-mcp --test mcp_auth auth_pat_pg -- --nocapture` | 11 passed, 0 `SKIPPED`: the token store ran on the real PG 16 |
| `cargo test -p hub --test mcp_module` (PG variable unset) | 5 passed |
| **Not re-run** | `just verify-07` (Docker), `just verify`, `just bench && cargo xtask bench-check` (the read path did not change; the P9 numbers below are those of `b5411fc`), `just mcp-fuzz` (the libFuzzer run of `mcp_pat_token` was not repeated after its invariants file changed; the committed corpus is replayed on stable by `corpus_replay::*`, 21 tests, which passed), `osv-scanner`, the other crates' tests. |

One flake, seen once: `logging::every_call_line_carries_the_stable_user_id_and_nothing_else_about_the_caller` failed once in a run of 120 tests filtered with `the_`, and then passed alone three times and in four complete runs of the binary. It installs a scoped `set_default` subscriber, which the notes in `confirm.rs` say loses events when other tests run at the same moment; nothing this round changed there except an `.unwrap()` on `lookup`. It was not fixed here.

## Review round 2 (reviewer verdict CHANGES REQUESTED at `11f9113`, report in `plans/07-mcp-server.review.md`)

Two blocking defects (both reproduced by the reviewer and again here before any fix) and six non-blocking items folded in. Every finding was checked against the code first; **none was rejected**. One fix went further than the finding said, and the reason is item 1 below. No contract path was touched (`git diff --name-only 545d0f5..HEAD` lists only `crates/hub-mcp/`, `docs/threat-model.md` and, now, this file), no plan file was edited, `skills/` was not edited, and no snapshot (`*.snap`) changed.

| Commit | What it does | Review item |
|---|---|---|
| `106ff5d` | A dropped request releases its session reservation, and closes the rmcp session it had made. | blocking 1 |
| `d055d19` | The log proofs use the one process-wide capture; a positive control for the S10 proof. | blocking 2 |
| `e07854e` | Port calls are bounded in time (directory of users, identity provider). | non-blocking 5 |
| `f67bf80` | `get_served_config` fits only the first 200 lines or 16 KB. | non-blocking 6 |
| `dc299b0` | `get_effective_config` summary for a resource file; the `raise_pr` question uses the host's redactor; the `tools/list` size bound. | non-blocking 4, 13, 8 |

**Blocking 1: a dropped request kept its session slot (`106ff5d`).** Reproduced first with the reviewer's recipe (`sessions::a_dropped_initialize_releases_its_slot`: five `initialize` futures polled once and dropped): red, `counted_sessions(alice) == 5`. The slot is now held by a value, `session::Opening`, that gives it back on `Drop`; the one thing that disarms it is `Opening::hand_over`, called when the response carries the session id, with no `await` between the response and the call (so there is no window "after the head, before the owner insert": the test below drives the drop at every poll from 1 to 12 and the count never disagrees with rmcp's table). The probe found **a second leak of the same class that the finding did not name**: by the first poll rmcp has already made the session (`live_sessions() == 1`), and the client never learned its id. Releasing only the count would have let one user fill rmcp's table with sessions nobody can reach, up to the 30-minute idle expiry and past the per-user cap of five, bounded only by the replica ceiling. So the opening also records the session its request made (a task-local that `CappedSessionManager::create_session` writes, since rmcp makes the session inside the request's own task) and closes it when the request is dropped or its handshake fails. Other exits, reviewed in the code (the first is also tested): the refused-by-limit return holds nothing; the non-success/no-id response drops the opening (releases and closes); a panic unwinds through the drop; the owner-table eviction listener releases exactly once per handed-over entry (`forget_session_owners` test) and its `Replaced`/`Size` causes cannot double-release because each entry holds exactly one hand-over. Tests (`tests/mcp_auth/sessions.rs`, `server::session::tests`): `a_dropped_initialize_releases_its_slot`; `a_dropped_initialize_leaves_no_session_behind_in_rmcp` (replica of five, five dropped, then five real sessions are accepted); `a_request_dropped_at_any_poll_leaves_the_count_equal_to_the_sessions`; `an_opened_session_keeps_its_slot_until_the_owner_table_forgets_it`; five unit tests of `Opening` and the closer (refused holds nothing, dropped gives back, handed over keeps, only the request's own session is closed, no runtime does not panic). Mutation: not closing the session fails two of them. **Residual:** between the drop and the spawned close running, the count is one short of rmcp's table (microseconds, one per dropped request); rmcp's own ceiling (`CappedSessionManager`) covers it. Where no runtime exists at drop time the session ends at its idle expiry.

**Blocking 2: the logging proofs lost events (`d055d19`).** Reproduced first: the filter `the_` of the `mcp_auth` binary failed **7 of 20** runs at `logging.rs:312` (`one line per call: left 0, right 9`) before the change. The per-thread capture of `confirm.rs` moved to its own module (`tests/mcp_auth/capture.rs`, with the `hub_platform` target that `logging.rs` had) and the three `logging.rs` tests use it; no test in the crate calls `set_default` any more (`crates/hub/tests/telemetry.rs` does, but it is the hub's test, outside 07's paths). `log_capture_has_no_tokens_args_or_output` now makes a call in each mode and asserts that **exactly the two `hub_mcp::call` lines** of those calls were captured, so the absence checks cannot become vacuous; with that target switched off in the capture the assertion fails (checked). **Flake proof, on `dc299b0`: the filter `the_` (133 tests) 0 of 30 runs failed; the whole `mcp_auth` binary (346 tests) 0 of 6 runs failed, 0 `SKIPPED` lines in each.**

**Port calls bounded (`e07854e`).** `ToolHost::require_active` wraps `Users::require_active` in the read timeout (5 s by default, `LimitSettings::read_timeout`); `McpHandler::call` and `list_tools` use it, and so does the confirmation gate's step-5 check (it had its own copy). `BearerAuth::with_timeout` (wired from the same setting in `McpState::new`) bounds the whole verdict of a request, the credential and then the user, in one wait; `still_valid` fails closed. A timeout is `AuthError::Unavailable`: the gate's existing 503, a tool's existing `unavailable`, never a pass. One visible change: `tools/list` answered "not allowed" for every directory error, including `Unavailable`; it now answers an internal "the service is unavailable, try again shortly" for `Unavailable` (and so for a timeout) and keeps "not allowed" for the rest. Tests (`limits::*`, paused clock, a directory and an identity provider that are silent for good, an outer 60 s timeout so that a missing bound fails instead of hanging): `a_directory_that_never_answers_makes_a_tool_call_unavailable`, `..._makes_tools_list_unavailable`, `..._is_a_503_at_the_gate_and_never_a_pass` (with a 2 s setting, not the default), `an_identity_provider_that_never_answers_is_a_503_and_opens_no_session`, `a_directory_that_never_answers_fails_the_stream_check_closed`. Removing the wait in `BearerAuth` fails three of them (the gate, the identity provider and the stream check); the four directory tests were red before the change (they waited for the 60 s outer timeout).

**`get_served_config` fits a window (`f67bf80`).** `RedactedText::window(max_lines, max_bytes)` returns the whole lines from the top that fit 200 lines and the cap, reading one byte past the cap at most; the tool takes it in the same blocking task as the scan and the line count and fits only that part. Output and cursor semantics are the contract's: no cursor, a web link when truncated, "k of N lines" with N the whole file. **Behaviour change, the one the contract states** ("200 lines or 16 KB"): a file of short lines shows at most 200 lines, not as many as 16 KB holds (red before: 1,676 lines of a 2 MiB file). Tests: `served::a_two_mib_served_file_shows_the_first_200_lines_and_says_how_many_there_are` (2 MiB is the scan's own input limit, `engine::Limits::input_bytes`; a larger file is withheld whole by the existing fail-closed path), `served::long_lines_of_a_served_file_stop_at_the_16_kb_cap_on_a_line_boundary`, `redact::a_window_is_whole_lines_from_the_top_within_both_limits`, a property test against `first_lines` for `\n` and `\r\n` files, and the work counted and not timed: for a 2 MiB text of many short lines, of few long lines and of one line, the window is at most 200 lines and 16 KB, the fitting renders at most 10 candidates and none cuts more than 16 KB. No wall-clock bound is claimed or asserted.

**Benchmarks, re-run (the disk allowed it: 6.3 GB free before the release build, the rule was 5 GB).** `just mcp-bench` on tree `f67bf80` (the last commit that changed the read path; the working tree held only test-file edits): **`bench-check (default): 21 pass, 0 fail, 0 unmet of 21 budgets`**. p50, p95 and p99 from criterion's `sample.json`, same method as the table in "Benchmarks against budgets" (4 shared vCPU, release build):

| Benchmark | p50 (ms) | p95 (ms) | p99 (ms) | Before (b5411fc) p95 |
|---|---|---|---|---|
| P9 `read_tools` (rotation over 20 tools) | 3.607 | 4.399 | 5.041 | 5.979 |
| `list_swimlanes` | 0.387 | 0.523 | 0.619 | 0.537 |
| `get_swimlane` | 0.512 | 0.646 | 0.695 | 0.707 |
| `list_files` | 0.526 | 0.796 | 0.853 | 0.706 |
| `read_file` (cold, D92) | 35.336 | 47.620 | 58.761 | 62.879 |
| `get_effective_config` | 0.824 | 1.136 | 1.292 | 1.196 |
| `compare` | 0.864 | 1.184 | 1.363 | 1.319 |
| `compare_tree` | 0.353 | 0.464 | 0.547 | 0.543 |
| `compare_grid` | 3.818 | 5.121 | 6.032 | 5.528 |
| `search_settings` | 0.676 | 0.810 | 0.939 | 0.883 |
| `search_text` | 0.896 | 1.469 | 1.496 | 1.202 |
| `find_inconsistencies` | 0.745 | 0.918 | 1.072 | 0.913 |
| `get_drift` | 0.722 | 0.945 | 1.090 | 0.837 |
| `get_pending_restarts` | 0.499 | 0.754 | 0.834 | 0.751 |
| `get_history` | 0.523 | 0.640 | 0.782 | 0.784 |
| `search_docs` | 0.519 | 0.728 | 0.810 | 0.665 |
| `read_doc` | 1.834 | 2.384 | 2.528 | 2.281 |
| `grep_docs` | 0.777 | 1.318 | 1.374 | 1.080 |
| `get_docs_for_file` | 0.495 | 0.818 | 0.844 | 0.650 |
| `get_feature_status` | 1.341 | 1.574 | 1.946 | 1.454 |
| **`get_served_config`** | **1.736** | **2.356** | **2.895** | **3.501** |
| (informational) `read_file_warm` | 1.536 | 1.816 | 1.950 | 2.385 |

All 21 budgets are p95 <= 300 ms; none was changed. The two runs are on a shared machine, so differences of tens of percent in the small rows are noise; the one row the change was for, `get_served_config`, went from p95 3.5 ms to 2.4 ms on its 146 KB fixture (the fixture is below the 2 MiB where the old cost mattered; the 2 MiB case is covered by the tests above, not by a benchmark). **Honest limit:** these numbers are for `f67bf80`. `dc299b0` came after the run and changed only the summary text of `get_effective_config` for a resource file or a file without settings (its benchmark fixture is a property source with settings, which takes the unchanged branch), a signature on the write-side question for `raise_pr`, and test code; it was not re-measured. `target/release` and `target/criterion` were deleted after the run, so `cargo xtask bench-check` on the full registry would again report the other prompts' rows and this one's as having no result until `just mcp-bench` is run.

**Small fixes (`dc299b0`).** (a) `get_effective_config`: for a resource file, or a file with no settings, the summary adds a fixed sentence (it is not an empty file, its body is not part of this view, `get_served_config` returns what the config-server serves); `read_tools::get_effective_config_of_a_resource_file_points_to_the_served_view` (red before: the bare "0 of 0 settings shown."). No contract change, D92b stands. (b) `message::raise_pr` takes the host's `Redactor`, the gate passes `&host.redactor`, and `Redactor::with_scan` builds one with a configuration of its own. Proofs: `message::tests::the_description_is_scanned_with_the_redactor_it_is_given` (a redactor with tight limits changes the same question; fails if the message goes back to a private default, checked) and `source_rules::the_host_builds_the_only_redactor_and_the_gate_gives_it_to_the_message` with `the_check_for_a_second_redactor_finds_what_it_should` as positive controls (red against the mutation: it found a second `Redactor::new()`). (c) `contract_schemas::tools_list_size_is_reported` keeps its upper bounds (125,000 and 35,000 bytes; measured 112,340 and 31,235) and has loose floors (20,000 and 8,000) that only catch a list with nothing in it; the full list must only be larger than the lean one.

**What was run on `dc299b0`, and what was not.**

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | passed, exit 0 |
| `cargo clippy -p hub-mcp -p hub --all-targets -- -D warnings`; `cargo clippy -p hub-mcp -- -D warnings` (no dev features); `cargo check -p hub` | passed, no warning |
| `cargo test -p hub-mcp --lib --tests` with `LK_TEST_PG_ADMIN_URL=postgres://postgres@127.0.0.1:54331/postgres` (PG 16.15, no pgvector) | **passed, 505 passed, 0 failed, 0 ignored, 0 `SKIPPED`** (lib 42, config 10, cursor_admin_doc 18, justfile_gate 7, mcp_auth 346, plugin_package 19, rmcp_spike 13, stub_protocol 24, t0_status 15, threat_model 11); +23 on the 482 of round 1 (lib +6, mcp_auth +17: sessions 4, limits 5, served 2, redact 3, read_tools 1, source_rules 2). The 11 `auth_pat_pg` tests ran on the real server. |
| Flake proof, the same `mcp_auth` binary | filter `the_` 0 of 30 failed (133 tests); whole binary 0 of 6 failed (346 tests); before the fix `the_` failed 7 of 20 |
| `cargo test -p hub --test mcp_module` (PG variable unset) | 5 passed |
| `cargo test -p xtask --test mcp_tools_doc --test budgets_registry` | 23 and 8 passed |
| `just mcp-bench` | 21 pass of 21 on `f67bf80` (table above) |
| **Not re-run** | `just verify-07` (no Docker), `just verify` as one command, `cargo xtask bench-check` on the full registry (the other prompts' rows have no result here), `just mcp-fuzz` (no fuzz target or its invariants file changed in round 2), `cargo test --workspace`, `just web-verify`, `osv-scanner`, `cargo deny`/`cargo audit` (no dependency changed: `git diff 545d0f5..HEAD -- Cargo.toml Cargo.lock` is empty), pgvector-dependent checks. The throwaway databases `lk_t_*` were dropped afterwards. |

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
| `mcp-test`: `LK_REQUIRE_DOCKER=1 cargo test -p hub-mcp --lib --tests` | **passed, 482 passed, 0 failed after review round 1** (lib 36, config 10, cursor_admin_doc 18, justfile_gate 7, mcp_auth 329, plugin_package 19, rmcp_spike 13, stub_protocol 24, t0_status 15, threat_model 11); the FINAL step had 468 (lib 29, mcp_auth 322). Run with `LK_TEST_PG_ADMIN_URL` set and `LK_REQUIRE_DOCKER` unset. A re-run with `--nocapture` printed **0** `SKIPPED` lines, so the 11 `auth_pat_pg` tests (token store on a real PG 16) really ran. | Not run with `LK_REQUIRE_DOCKER=1` (no Docker); the same tests ran against the throwaway server instead. pgvector is not used by this crate. |
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
| `hub-mcp` | set | 482 passed after round 1 (468 in the FINAL step; row above); doc tests: 0 tests, exit 0 | |

## Acceptance checklist

From the `Acceptance` section of `prompts/07-mcp-server.md`.

- [ ] **T0 results are recorded, and the modes are set in config.** **NOT RUN (human part).** The Cursor 3.22.12 compatibility test on Windows and macOS (Entra OAuth with the fixed client ID, elicitation accept/decline/cancel, the tool allowlist) has not been done: `just mcp-t0-status` prints `overall: NOT RUN`, 0 of 25 cells on each operating system (D91 in `docs/decisions.md`, runbook `crates/hub-mcp/t0/README.md`). An agent cannot run Cursor and never fills a cell. What *is* proved, and does not tick the box: the modes exist and default as D91 says (`config::defaults_are_oauth_elicitation_summary`, `config::both_auth_modes_at_once_is_a_config_error`, `config::unknown_value_is_a_config_error`, `config::every_documented_value_is_accepted`); the server side of the T0 stub is exercised in process (`stub_protocol::*`, 24 tests, including `elicitation_accept_decline_cancel_all_complete`); the status tool reads the table (`t0_status::*`, 15 tests).
- [ ] **`just verify-07` passes.** **Not proved.** The recipe stops at `docker-check` in this sandbox. Its other sub-recipes passed (table above); the Docker daemon and the T0 human checks remain.
- [x] **P9 is within budget for every read tool.** Proof (re-run in review round 2 on `f67bf80`: 21 pass of 21, `get_served_config` p95 2.4 ms; see "Review round 2"): `just mcp-bench`, twice, 21 pass of 21 (`P9`, `P9.list_swimlanes` ... `P9.get_served_config`), each p95 far below 300 ms (table below). These are this layer's share on fakes at target scale, as the budgets are registered.
- [x] **All authentication, confirmation and role tests pass.** Proof: `cargo test -p hub-mcp --lib --tests`, 505 passed, 0 failed (after review round 2; 482 after round 1), with the personal-token store on a real PG 16. By suite: authentication `auth_oauth::*` (22), `auth_pat::*` (22), `auth_pat_pg::*` (11, real Postgres), `auth_header::*` (8), `token_syntax::*` (12), `token_routes::*` (12); confirmation `confirm::*` (37, recounted; the FINAL step said 63, which was wrong; accept, decline, cancel, client without elicitation, draft mode, `requires_approval`, timeout, disconnect, idempotency, the cleaned description, the gate-only-caller scan and its controls) and `confirm_golden::*` (14), plus `confirm::message` in the lib (20); role `tool_matrix::*` (5), `route_table::*` (7), plus `crates/hub/tests/mcp_module.rs` (5).

## Security requirements implemented

All proofs below ran in this step (names as they appear in the test output). The plan's traceability table names two tests, `docs_tree::read_doc_takes_only_doc_paths` and `docs_tree::dotdot_and_backslash_are_refused`, that do not exist under those names; the same cases (`/etc/passwd`, `git/main/x`, `/git/../x`, `/git/main/.git/config`, `/uploads/`, `/git/a\b` for `read_doc`) are in `args::every_path_argument_uses_a_domain_parser`.

- **S3** (CSRF) is **only partly met** for the personal-token routes, by D-2 and D109: the exact `Origin` check (`token_routes::state_changing_requests_need_the_public_origin`) and not the double-submit token. The condition and who accepted it are in D109 of `docs/decisions.md`.
- **S4** (declared role on every route and tool, default deny, pending and disabled refused): `src/server` guards and `src/tools` role floor. `route_table::{every_mcp_route_has_a_declared_guard, every_method_of_the_endpoint_is_gated, routes_agree_with_openapi_and_allowances, unmatched_path_under_mcp_is_default_deny, the_routes_of_each_mode_are_exactly_these}`; `contract_schemas::{every_documented_tool_has_a_handler_and_every_handler_a_spec, advertised_roles_confirmations_caps_match_the_document, embedded_schemas_equal_the_contract_document}`; `tool_matrix::{every_tool_by_every_user_state, role_is_checked_before_arguments_are_parsed, tools_list_shows_only_callable_tools, unknown_tool_is_a_protocol_error, a_role_change_takes_effect_on_the_next_call}`; `auth_oauth::{pending_user_is_403, disabled_user_is_403, role_change_takes_effect_on_next_call}`; `confirm::editor_cannot_call_restart_service`; D-1 and D109: `/mcp` is a `public` route that authenticates itself; hub: `mcp_module::{mcp_routes_pass_the_hub_route_table_check, mcp_default_deny_holds}`.
- **S11** (typed inputs, bounded sizes, linear-time regex only): `src/tools/args.rs`, `src/output/cursor.rs`. `args::{every_path_argument_uses_a_domain_parser, content_over_2_mib_of_bytes_is_payload_too_large, pattern_over_512_bytes_is_invalid_argument, swimlane_lists_over_64_are_invalid_argument, unknown_field_is_invalid_argument, a_null_for_an_optional_argument_is_refused, ids_and_hashes_are_held_to_the_schemas_patterns, typed_args_never_accept_what_the_schema_rejects_prop, valid_examples_deserialize_into_typed_args, arbitrary_json_never_panics_the_parser}`; `auth_oauth::token_longer_than_max_is_refused_before_parsing`; `server::oversized_body_is_413`; `cursor::*` (7); fuzz `mcp_tool_args`, `mcp_cursor`, `mcp_bearer_header`, `mcp_http_body` (ran, clean).
- **S14** (token checks, role per tool, server-enforced elicitation, capped labelled output, per-user limits, no shell): `auth_oauth::{missing_header_is_401_with_resource_metadata_challenge, invalid_token_is_401, expired_token_is_401, wrong_audience_is_401, token_without_mcp_scope_is_401, unavailable_verifier_is_503_not_401, unknown_user_is_401, session_id_is_bound_to_the_user, foreign_origin_is_refused}`; `auth_pat::*` and `auth_pat_pg::*` (invalid, expired, revoked, pending-user and wrong-mode tokens); `confirm::{the_scan_for_mutating_calls_finds_every_shape_and_only_those, accept_with_confirm_true_applies_once, accept_with_confirm_false_is_declined_and_writes_nothing, decline_writes_nothing, cancel_writes_nothing, timeout_cancels_and_writes_nothing, client_without_elicitation_gets_draft_link_and_no_write, draft_mode_never_calls_a_write_method, requires_approval_user_gets_proposal_created_after_confirm, an_answer_from_another_session_is_ignored, the_gate_is_the_only_caller_of_the_mutating_write_methods, what_the_user_confirmed_is_what_happens_even_if_the_approval_rule_changes_meanwhile}`; `output_cap::{no_read_tool_exceeds_16kb_at_worst_case_items, write_tools_stay_under_4kb, paging_is_exact_prop, paging_is_exact_at_the_edges, truncated_sets_next_or_web_link, read_file_default_is_200_lines_or_16kb}`; `limits::{rate_limit_is_per_user, rate_limited_carries_retry_after, write_class_has_its_own_budget, concurrency_cap_per_user, in_flight_bytes_are_bounded, port_timeout_is_unavailable}`; `source_rules::{no_shell_or_process_in_src, no_unbounded_channel_in_src, no_blocking_fs_in_src}`.
- **S14b** (redaction, no reveal over MCP, denied files): `src/output/redact.rs`. `planted_key::{planted_key_never_appears_in_any_tool_output, a_key_cut_by_the_line_limit_of_a_window_is_hidden_in_both_modes, no_tool_input_can_reveal_a_flagged_value, late_sweep_catches_a_field_the_typed_path_missed, the_sweep_fails_closed_when_it_cannot_scan, the_sweep_is_idempotent_and_leaves_nothing_for_the_scan}`; `denied_files::{denied_file_returns_content_withheld_for_every_source, get_effective_config_refuses_a_value_that_comes_from_a_denied_file, search_text_drops_hits_in_denied_files, writes_to_a_denied_file_are_refused_before_anything_is_asked, get_served_config_withholds_a_denied_file_and_lists_nothing_else_of_it}`; `served::get_served_config_is_redacted`; `source_rules::{every_text_field_of_an_output_type_is_redacted_text, redacted_text_is_made_only_in_redact_rs}`; `context_fields::attribution_and_severity_present_in_history_drift_and_list_files`; `confirm::message::tests::{a_secret_in_the_description_is_hidden_before_it_is_quoted, a_description_that_the_secret_scan_could_not_finish_on_is_not_shown}` and `confirm::confirmation_message_quotes_a_cleaned_start_of_the_pull_request_description` (the PR description in the question); fuzz `mcp_redactor` (ran, clean). Residual: Q20 (the scan's false negatives, six real files) blocks go-live.
- **S15** (config and doc text is data): `untrusted::{untrusted_flag_and_label_on_every_text_tool, server_instructions_state_the_data_rule, data_block_delimiter_cannot_be_forged, data_block_nonce_is_fresh_per_response, summary_never_contains_file_text_prop, in_the_default_rendering_content_holds_no_text_from_a_file}`; `confirm::{confirmation_message_neutralises_untrusted_text, confirmation_message_quotes_a_cleaned_start_of_the_pull_request_description}`; in the lib `confirm::message::tests::{quote_defuses_what_a_client_may_render_as_markup, quote_is_bounded_and_always_closes_prop, the_start_of_the_description_is_cleaned_like_every_other_text_of_the_assistant, a_long_description_is_cut_at_the_prefix_length_and_says_so, the_start_of_the_description_gives_way_to_the_link_and_the_cap}`; goldens `golden_raise_pr_with_a_long_description`, `golden_raise_pr_with_a_hostile_description`.
- **Review round 2 additions.** **S14 / rule 5** (per-user and per-replica session limits hold when a client goes away mid-request; every port a request waits on is bounded): `sessions::{a_dropped_initialize_releases_its_slot, a_dropped_initialize_leaves_no_session_behind_in_rmcp, a_request_dropped_at_any_poll_leaves_the_count_equal_to_the_sessions, an_opened_session_keeps_its_slot_until_the_owner_table_forgets_it}`, `limits::{a_directory_that_never_answers_makes_a_tool_call_unavailable, a_directory_that_never_answers_makes_tools_list_unavailable, a_directory_that_never_answers_is_a_503_at_the_gate_and_never_a_pass, an_identity_provider_that_never_answers_is_a_503_and_opens_no_session, a_directory_that_never_answers_fails_the_stream_check_closed}`. **S14b**: the question for `raise_pr` scans with the host's redactor (`message::tests::the_description_is_scanned_with_the_redactor_it_is_given`, `source_rules::the_host_builds_the_only_redactor_and_the_gate_gives_it_to_the_message`). **S10/S21**: the log proofs no longer lose events and cannot pass without a captured product line (`logging::log_capture_has_no_tokens_args_or_output`). **P9/rule 4**: the work of `get_served_config` is bounded by 200 lines and 16 KB (`redact::a_window_of_a_two_mib_text_is_as_small_as_the_limits_say`, `served::a_two_mib_served_file_shows_the_first_200_lines_and_says_how_many_there_are`). The threat model's denial-of-service row names all of these.
- **S21** (no unsafe, no panics, no leaked internals, no secrets in logs): `source_rules::{forbid_unsafe_present, nothing_secret_is_formatted_into_a_message}`; workspace clippy lints in `mcp-lint` (passed); `errors::{port_errors_map_to_the_contract_codes, error_messages_are_fixed_text_not_input, write_outcomes_map_to_results_and_errors, auth_errors_map_to_the_statuses_the_hub_guard_gives}`; `logging::{log_capture_has_no_tokens_args_or_output, debug_of_write_args_prints_no_content, debug_output_of_the_secret_bearing_types_is_redacted, tool_calls_log_what_was_called_and_nothing_it_carried, every_call_line_carries_the_stable_user_id_and_nothing_else_about_the_caller}`; `confirm::a_whole_confirmation_life_logs_what_happened_and_nothing_it_carried`; no panic on a digest of the wrong length: `token_syntax::a_digest_of_the_wrong_length_is_an_error_and_not_a_panic`; no test hook in the crate's API: `source_rules::{a_hidden_test_hook_exists_only_in_test_builds, the_check_for_ungated_hooks_finds_what_it_should}`.

## Benchmarks against budgets

**Superseded in part by review round 2:** the whole table below was re-run on `f67bf80` and is repeated, with a "before" column, in "Review round 2". The text and the table below are the run on `b5411fc`.

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

- **D-1:** `/mcp` is a self-authenticating `public` route, not `Role(Viewer)`, because `GuardedRouter` cannot add the `WWW-Authenticate` challenge to its own 401; a 3-line `GuardedRouter::layer` in `hub-platform` would restore it. **Recorded as D109 in `docs/decisions.md`** (round 1), with who accepted it and when.
- **D-2:** S3 for the personal-token routes is Origin-check only; the double-submit half depends on the hub composing 03a's CSRF layer (every non-GET route, `/api/v1/me/mcp-tokens` included, and `crates/hub/tests/mcp_module.rs` then asserts it). **Recorded as D109**, same entry.
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

From review round 1:

- **The type-level handle for the gate** (review non-blocking 5): give `ToolHost` a narrow handle without the mutating methods, and keep the full `Arc<dyn WriteService>` visible only inside `confirm`. Until then the guard is a source scan with controls (it finds a broken chain, an alias and a path call; it does not see a call that a macro writes).
- **Goldens for the 401 challenge, `tools/list` per role and the cursor encoding** (review 9): they are inline assertions today, not files under `tests/golden/`; and the two plan test names `docs_tree::*` do not exist.
- **The content-mode decision after T0** (review 6, plan A6): S15 "delimited blocks" are produced only in `data_block` mode; the default `summary` mode labels the text (`untrusted: true`) and does not delimit it. Decide with the T0 results; the threat model says so. The `advertise_output_schemas` default (the `tools/list` is 112 KB with result schemas and 31 KB without) is decided with the same results (review 7).
- **The notify on-switch:** `tools.notify_services.enabled` has no environment key; the hub integration gives it one.
- **The dev-only `evals/evals.json`** of prompt 11's skill is still copied into the plugin package.
- **Decision numbers collide across branches.** `origin/agent/05-registry-read-api` defines its own D91, D92, D107 and D108 (different decisions from this branch's D91 and D92, from main's D107 (PR #16), and from D109 here). Whoever merges 05 and 07 renumbers one side; the references in `docs/threat-model.md`, `crates/hub-mcp` and `plans/` name D91, D92 and D109 of this branch.
- The description in the `raise_pr` question is shown in part only, and `quote()` leaves bare addresses alone (threat model, residual risks).

From review round 2 (carried, with the earlier follow-ups above, for the PR text and the humans):

- **The type-level handle for the gate.** `ToolHost.writes` is still a public field holding the full `Arc<dyn WriteService>`; the gate-only-caller guard is a whole-file scan with positive controls, and a scan cannot see a call that a macro writes. A narrow handle without the mutating methods, visible outside `confirm` only as the narrow type, is the structural fix.
- **`advertise_output_schemas` default.** `tools/list` is about 112 KB with result schemas and 31 KB without (an Admin, 26 tools). The reviewer would default it off until T0 shows a need; decide with the T0 results.
- **S15 delimited blocks exist only in `data_block` mode.** The default `summary` mode labels the text (`untrusted: true`, `label`) and does not delimit it. Decide the content mode with the T0 results.
- **D109 provenance.** Git shows the agent as the author of the commit that records D-1 and D-2. The text names the human who accepted (hsetially); the record is the human's review of the `docs/decisions.md` diff in the PR, and the PR text should say so.
- **Decision numbers collide with `origin/agent/05-registry-read-api`.** D91, D92, D107, D108 and D109 are used by both branches for different decisions; the second branch to merge renumbers. `crates/hub-mcp/examples/t0_status.rs` parses the literal `- **D91**` heading of `docs/decisions.md` and about 25 files name D91, D92 and D109, so a renumber must follow those references.
- **The `raise_pr` question shows at most 240 characters of the body.** The rest is posted unread under the user's GitHub identity (threat model, residual risks).
- **`tools.notify_services.enabled` has no environment key** in `McpConfig::from_lookup`; the hub integration gives it one.
- **The plugin package copies the dev-only `evals/evals.json`** of prompt 11's skill.
- **Hub integration, go-live checklist items** (next to `just mcp-t0-status --strict`): prompt 09 gives `Mcp-Session-Id` affinity (or deployers set `confirm_mode = draft`), and the hub composes 03a's `CsrfLayer` over the personal-token routes and asserts it in `crates/hub/tests/mcp_module.rs` (D109's condition for S3). Nothing mounts `McpModule` in `main` yet.
- **Found in round 2:** (1) `crates/hub/tests/telemetry.rs` uses a scoped `set_default` subscriber, the pattern that made `logging::` flaky; it is the hub's test and outside 07's paths, so it was not touched. (2) A served file over 2 MiB (the secret scan's input limit) is withheld whole by the existing fail-closed path: no byte is shown, and the message points to the web UI. (3) `tools/list` now says "unavailable" and no longer "not allowed" when the directory cannot answer (see "Port calls bounded"). (4) After a dropped `initialize`, the session slot is returned a moment before rmcp finishes closing the session it made (see blocking 1).

**Still open, as explicit gates (review blocking items 1 and 2 of round 1; nothing here resolves them):**

1. A Docker-capable run of `just verify-07`, `just verify` and `just bench && cargo xtask bench-check`, with the output attached to the PR (CI or the human's machine). The libFuzzer runs, `osv-scanner` and pgvector-dependent checks belong to the same run.
2. The T0 results for Cursor 3.22.12 on Windows and macOS (the filled D91 table), or the human's explicit waiver recorded in the PR text. **`just mcp-t0-status --strict` is a go-live gate:** `just verify-07` stays green with T0 unrun, because `mcp-t0-status` exits 0 without `--strict`.

## Deviations from the plan

None in scope or tasks. Two housekeeping commits in the FINAL step fixed lockfile entries below the 14-day rule (above). The commits of the branch since `origin/main` (this evidence file is committed after them as `07/evidence`; the round-2 fixes are the first five lines, then the round-2 review report and the round-1 evidence, the round-1 commits, the review report `6c18fa4` and the FINAL evidence `7b4677a`):

```
git log --format='%h %s' origin/main..HEAD
dc299b0 07/fix: small output and message fixes
f67bf80 07/fix: get_served_config stops fitting every line of a large file
e07854e 07/fix: port calls are bounded in time
d055d19 07/fix: the log proofs use the process-wide capture
106ff5d 07/fix: a dropped request releases its session reservation
545d0f5 07/review: round 2
11f9113 07/evidence: update after review round 1
b347dd8 07/fix: hygiene from review round 1
ee743e5 07/fix: the raise_pr confirmation shows a quoted prefix of the PR body
3b92e8d 07/fix: record the human's decisions D-1 and D-2 in docs/decisions.md
426a5cb Merge main (PR #16, D107) into agent/07-mcp-server
6c18fa4 07/review: round 1
7b4677a 07/evidence: gates, S#, P9 numbers and new dependencies (Docker-less sandbox)
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
