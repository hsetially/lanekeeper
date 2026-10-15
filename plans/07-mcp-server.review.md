# 07 review, round 1

Reviewer agent output, saved verbatim by the orchestrator. Branch `agent/07-mcp-server` at `7b4677a`.

VERDICT: CHANGES REQUESTED

I found no code defect that blocks merge. The three blocking items are missing evidence or missing records, and none needs a code change. The branch is `agent/07-mcp-server` at 7b4677a. I changed nothing in the worktree.

## Blocking

1. **Gates cannot be shown green anywhere yet** (skill step 2: an unrun gate is itself blocking; AGENTS.md protocol step 4). No fix is needed in the code. A Docker-capable run (CI or the human's machine) has to supply the missing output. What is missing:
   - `just verify-07` stops at `docker-check` ("needs a running Docker daemon"). I reproduced this, so the `LK_REQUIRE_DOCKER=1 cargo test` leg and the pgvector image path never ran.
   - `just verify` did not run as one command. I ran its fmt, clippy `--workspace`, `cargo deny` and `cargo audit` steps. I did not run `cargo test --workspace` or `just web-verify`; the web app is untouched.
   - `cargo xtask bench-check` over the full registry gives 21 pass, 25 fail, 26 unmet of 72. The 21 passes are all the P9 rows. The other 51 rows belong to other prompts, whose benchmarks do not exist or were never run in this tree.
   - The libFuzzer runs of `just mcp-fuzz` were not repeated by me. A build needs about 3.1 GB and only 3.9 GB was free. I did run the committed seed replay on stable (`corpus_replay::replay_all_mcp_targets`, which passed). I also ran `cargo deny` and `cargo audit` on the fuzz crate.
   - `osv-scanner` was not run by anyone.
   - Fix: attach green output of `just verify-07`, `just verify` and `just bench && cargo xtask bench-check` from a Docker-capable run.

2. **T0 (Cursor 3.22.12 on Windows and macOS) is NOT RUN, so acceptance item 1 cannot be ticked.**
   - `just mcp-t0-status` prints NOT RUN in every cell, for both operating systems.
   - An agent cannot fix this. On the merits it should not stop the library from merging: `McpModule` exists, but `main` does not mount `/mcp` (`crates/hub/src/mcp.rs`; `docs/threat-model.md`, MCP section, last residual risks). It must stop go-live.
   - `verify-07` stays green with T0 unrun, because `mcp-t0-status` exits 0 without `--strict`.
   - Needed: the human's explicit waiver in the PR text, or the filled D91 table. The PR must say that `just mcp-t0-status --strict` is a go-live gate.

3. **The S3 relaxation (D-2) and the departure from the frozen plan on the S4 mechanism (D-1) are not recorded in `docs/decisions.md`.**
   - `docs/decisions.md` has D91 (line 305) and D92 (line 340) only.
   - D-1 and D-2 appear only in the threat model residual risks and in `plans/07-mcp-server.evidence.md`.
   - S3 says "a double-submit token plus an Origin header check". The token routes in `crates/hub-mcp/src/auth/routes.rs:133` (`check_origin`) do the Origin half only.
   - The `lanekeeper-security` skill says only a human can relax a requirement, and only by recording it in `docs/decisions.md`. The precedent is D107 on main (S8 relaxation), which has an explicit entry.
   - Fix: add one decision entry that:
     - records D-1 (`/mcp` registered as `public`, self-authenticating through the gate) and D-2 (S3 met by the Origin check only);
     - names the condition that the hub composition composes 03a's `CsrfLayer` over every non-GET route, including `/api/v1/me/mcp-tokens`;
     - names who accepted it, and when.
   - The approved Extra-path covers only the D91 append. The human has to widen the Extra-path for this entry.
   - Main has since added D107 (PR #16, which also edits `docs/security.md` S8), so `docs/decisions.md` will conflict textually at the end of the file. Merge main and re-run `mcp_tools_doc`.

## Non-blocking

Verdicts on the flagged items (items 1 to 4). None of them blocks merge on the merits.

1. **D-1 (`/mcp` is a `public` route that authenticates itself): accept.**
   - `crates/hub-mcp/src/server/mod.rs:485-486` registers POST, GET and DELETE with `.public(..)` and the gate layer.
   - `src/server/guard.rs` runs the gate before the handler on every method, in this order: length, Host, Origin, bearer token, `Users::require_active`, session owner.
   - Default deny is shown by `unmatched_path_under_mcp_is_default_deny` and `every_method_of_the_endpoint_is_gated` (`tests/mcp_auth/route_table.rs:138,189`). The hub-level `mcp_default_deny_holds` also passes.
   - The only loss: `assert_route_table` cannot prove structurally that the gate is wired. The two tests above catch a missing gate for POST, GET and DELETE, and also fail if anyone adds PUT, PATCH, OPTIONS or TRACE.
   - `SessionAuth::still_valid` is used only by the events SSE handler in the hub, so no stream revalidation is lost.
   - Suggested follow-up: the 3-line `GuardedRouter::layer` in `hub-platform`.

2. **D-2 (S3 for the personal-token routes is Origin-only): accept on the code merits, as a tracked condition.**
   - The Origin check is exact and mandatory. A missing, `null`, other-scheme or other-port Origin gets 403, before the body is read (`state_changing_requests_need_the_public_origin`, `token_routes.rs:277`).
   - The routes answer 404 unless personal-token mode is on. A token value appears only in the create response, so a forged create yields a token the attacker cannot read.
   - Nothing is exposed until the hub mounts `/mcp`. The hub composes no CSRF layer for any module today, so this gap is not specific to 07.
   - Condition: the hub integration PR must wrap `/api/v1/me/mcp-tokens` in `CsrfLayer`, and `mcp_module.rs` should then assert it. Recording it per blocking item 3 is what makes the condition visible.

3. **D92a (cold `read_file` of the largest file, p95 about 52 to 63 ms against the plan's 50 ms screen): accept.**
   - My `just mcp-bench` run gave p95 44.0 ms, and the registered 300 ms budget passes with a wide margin.
   - The cost is `engine::index_settings`, whose own accepted budget is 60 ms. It is paid once per file version per replica.
   - It is still the "parse per request" pattern the performance skill lists as an anti-pattern. The clean fix is D92's option C, a `RegistryRead` outline served from 05's `settings_index`.

4. **D92b (`get_effective_config` returns `text` null and typed values `other`): accept.**
   - The output validates against the contract schema, and secrets are redacted either way.
   - The functional gap is real. A resource file gets neither settings nor body, and `yes`/`on` are not flagged. `get_served_config` is the workaround until the follow-up contract PR.

5. **The test `the_gate_is_the_only_caller_of_the_mutating_write_methods` is a weak guard.**
   - `tests/mcp_auth/confirm.rs:1463` needs "writes" and the method name on the same line.
   - rustfmt's chain-breaking evades it: `confirm/gate.rs:373-375` has `writes` and `.upload(` on separate lines. An alias such as `let w = Arc::clone(&host.writes)` evades it too.
   - I checked by multi-line search that today only `src/confirm/gate.rs` calls the mutating methods (`edit`, `upload`, `delete`, `revert`, `restart`, `raise_pr`, `apply_draft`, `notify`). The one other use of `host.writes` is `propose_draft` in `tools/write/mod.rs:76`.
   - Fix: make it type-level. Give `ToolHost` a narrow handle without the mutating methods, and keep the full `Arc<dyn WriteService>` visible only inside `confirm`.

6. **S15 "delimited blocks" are produced only in `data_block` mode.**
   - The default `summary` mode puts text in `structuredContent` with `untrusted: true` and the label, and no delimited block. Prompt T2 and `docs/mcp-tools.md:17` say "delimited blocks".
   - This is plan A6, to be confirmed after T0. The threat model states it honestly. Treat the content-mode decision as part of the T0 go-live gate.

7. **`tools/list` is heavy and its test bound is loose.**
   - Input schemas are about 22 KB and output schemas about 59 KB for 27 tools.
   - `advertise_output_schemas` defaults to true (`src/server/mod.rs:118`).
   - The only guard is `wire > 20_000 && wire < 400_000` (`tests/mcp_auth/contract_schemas.rs:533`), which would not catch growth.
   - Tighten the bound, and decide the default after T0 shows whether Cursor puts `outputSchema` into the model's context.

8. **The `raise_pr` confirmation does not show or link the PR body.**
   - `src/confirm/message.rs:345` says "N characters, not shown here", and the link goes to the PR list.
   - Assistant-written text is posted under the user's own GitHub identity. This residual is not named in the threat model.
   - Show a quoted prefix, or link to a page that shows the body.

9. **Plan traceability and goldens differ from the frozen plan.**
   - `docs_tree::read_doc_takes_only_doc_paths` and `docs_tree::dotdot_and_backslash_are_refused` do not exist under those names. I checked that the cases are covered in `args::every_path_argument_uses_a_domain_parser` (`tests/mcp_auth/args.rs:438-452`).
   - The plan also asked for goldens of the 401 challenge, `tools/list` per role and the cursor encoding. These are inline assertions, not files under `tests/golden/`.
   - Every other test the plan names exists by name (111 names grepped).

10. **Small code points.**
    - `McpState::forget_session_owners` (`src/server/mod.rs:455`) and `Replay::run_pending_tasks` are `pub` `#[doc(hidden)]` test hooks in the production API. Calling the first from hub code would make every live session answer 404. Gate them behind `cfg(test)` or the `fakes` feature.
    - `src/auth/token.rs:190` `out.copy_from_slice(tag.as_ref())` panics if the digest length ever differs. It is 32 today; use `try_into`.
    - `tools.notify_services.enabled` has no environment key (the evidence notes this). The integration PR has to supply an on-switch.
    - The plugin package copies the skill's dev-only `evals/evals.json`.

11. **Spec chain: all checks pass except the recording gap in blocking item 3.**
    - The plan is Status APPROVED and Contract-change APPROVED. Only `7472187` (draft) and `eeceade` ("approved by a human") touch it; nothing after. The approval commit's author is the agent account, as in the normal process, so git alone cannot show the human.
    - Commits run T0, T1, T2, T3, T4, T5, T6, T7 in order. Between them are `07/blocked`, `07/decision`, five `07/fix` commits, one `07/evidence` commit and two merges of main.
    - The contract paths (`crates/domain`, `crates/ports`, `proto`, `api`, `db`, `docs/mcp-tools.md`, `design`, `skills`) are untouched. `perf/budgets.toml` changes only as 21 flips of `registered = false` to `true`, which the plan's C2 approves.
    - Extra-paths touched are exactly those the plan lists. `crates/xtask/tests/mcp_tools_doc.rs` is listed in the plan but this branch did not need to touch it; the contract PRs changed it. Shared `Cargo.toml`, `Cargo.lock` and `Justfile` are allowed.
    - The 23 new packages in `Cargo.lock` are all at least 15 days old. The youngest are `jsonschema` 0.58.3 and its `referencing`, `jsonschema-regex` and `jsonschema-value` at 0.58.3 (2026-09-30), and `rmcp` 3.5.0 (2026-09-28). The fuzz lockfile has no (name, version) pair missing from the merged lockfiles.

## Evidence re-run

- `cargo fmt --all -- --check`: exit 0.
- `cargo clippy -p hub-mcp --all-targets -- -D warnings`: exit 0.
- `cargo clippy -p hub --all-targets -- -D warnings`: exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`: finished, exit 0.
- `cargo test -p hub-mcp --lib --tests` (`LK_TEST_PG_ADMIN_URL` set, PG 16.15, no pgvector): 468 passed, 0 failed (lib 29, config 10, cursor_admin_doc 18, justfile_gate 7, mcp_auth 322, plugin_package 19, rmcp_spike 13, stub_protocol 24, t0_status 15, threat_model 11).
- `cargo test -p hub-mcp --test mcp_auth auth_pat_pg -- --nocapture`: 11 passed, no SKIPPED line. The token store, grants and audit transaction ran against the real PG 16, not skipped.
- `cargo test -p hub --test mcp_module` (PG variable unset): 5 passed.
- `cargo test -p xtask --test mcp_tools_doc --test budgets_registry`: 8 passed and 23 passed.
- `cargo deny check`: advisories ok, bans ok, licenses ok, sources ok.
- `cargo audit`: exit 0, 631 crates.
- Fuzz crate: `cargo deny --manifest-path crates/hub-mcp/fuzz/Cargo.toml check --config deny.toml` ok; `cargo audit --file crates/hub-mcp/fuzz/Cargo.lock` exit 0.
- `just mcp-bench` (`cargo bench -p hub-mcp`, then bench-check cut to the 21 P9 ids): 21 pass, 0 fail, 0 unmet.
  - Rotation `P9` p95 5.5 ms.
  - Cold `read_file` p95 44.0 ms.
  - `get_served_config` 3.2 ms; `compare_grid` 4.3 ms; every other row 2.2 ms or less.
  - Budget is 300 ms for all.
- `cargo xtask bench-check` (full registry): 21 pass, 25 fail, 26 unmet of 72. The 51 failures and unmets are other prompts' rows.
- `just verify-07`: fails at `docker-check`.
- `just mcp-t0-status`: every cell NOT RUN, Windows and macOS.
- Not run by me, for the reasons above: `cargo test --workspace`, `cargo test -p hub` whole, `just web-verify`, libFuzzer runs, `osv-scanner`, hub-platform database tests, and anything needing pgvector or Docker.
- Security trace (S# to implementation to tests):
  - S4: `src/server/guard.rs`, `src/server/handler.rs` role check, `src/server/mod.rs:485` to `route_table::*`, `tool_matrix::*`, `contract_schemas::every_documented_tool_has_a_handler_and_every_handler_a_spec`, hub `mcp_routes_pass_the_hub_route_table_check` and `mcp_default_deny_holds`.
  - S11: `src/tools/args.rs`, `src/output/cursor.rs`, `src/auth/header.rs` to `args::*`, `contract_schemas::typed_args_never_accept_what_the_schema_rejects_prop`, `server::oversized_body_is_413`, fuzz targets `mcp_tool_args`, `mcp_cursor`, `mcp_bearer_header`, `mcp_http_body`.
  - S14: `src/auth/*`, `src/confirm/{gate,decision,elicit}.rs`, `src/limits.rs` to `auth_oauth::*`, `auth_pat::*`, `auth_pat_pg::*`, `confirm::*` (end to end over the protocol and the SSE stream), `limits::*`, `output_cap::*`. The gate is the only caller of the mutating write methods (checked by hand, see non-blocking item 5).
  - S14b: `src/output/redact.rs`, `src/output/sweep.rs`, `src/tools/denied.rs` to `planted_key::*` (every text tool, both content modes, plus the sweep), `denied_files::*`, `served::get_served_config_is_redacted`, fuzz `mcp_redactor`.
  - S15: `src/output/envelope.rs`, `src/server/instructions.rs`, `src/confirm/message.rs` to `untrusted::*`, `confirm::confirmation_message_neutralises_untrusted_text`, plus golden snapshots.
  - S21: crate-level `#![forbid(unsafe_code)]`, `src/tools/error.rs`, `src/server/handler.rs` log line to `source_rules::*`, `errors::*`, `logging::*` (these include positive controls).
  - P9: `benches/read_tools.rs` drives the real server in process over fixtures at target scale; all 21 registered rows pass.
- Threat model updated: yes. `docs/threat-model.md` has a section "MCP server (07)" with assets, trust boundaries, a STRIDE table with proof cells, and residual risks including D-1, D-2, T0, D92 and Q20 and Q37. `tests/threat_model.rs` passes (11 tests).

Review verdict flips to APPROVE once blocking items 1 to 3 are resolved: a Docker-capable green run attached, the human's T0 waiver or results recorded, and D-1 and D-2 recorded in `docs/decisions.md`.

---

# 07 review, round 2

Reviewer agent output, saved by the orchestrator (condensed only in layout; findings verbatim). Branch `agent/07-mcp-server` at `11f9113`.

VERDICT: CHANGES REQUESTED

Round 1's blocking items 1 and 2 (no Docker run, T0 NOT RUN) are not repeated as blocking. The evidence file, `docs/cursor-admin.md`, the threat model and D91 all state them honestly: nothing is ticked without proof, and NOT RUN is never counted as a pass. Two small code and test defects were found, both reproduced.

## Blocking

1. **Session reservation leaks when a request is dropped.** `crates/hub-mcp/src/server/guard.rs:168-196` violates AGENTS rule 5 and S14 (per-user limits). The gate calls `state.sessions.reserve()` at line 168, then awaits `next.run(request)` at line 179. The release or owner-insert code only runs after that await. If the client disconnects before rmcp returns the response head, hyper drops the future, and the reservation is never released. The only other release path is the owner-table eviction listener (`guard.rs:53`), and no owner entry exists for a dropped request.
   - Reproduced in a scratch copy of HEAD with `Rig` set to 5 sessions per user: five initialize `oneshot` futures polled once and dropped gave `counted_sessions(alice) == 5`, `live_sessions == 0`, and a real `initialize` then answered 429 (`too_many_sessions`).
   - The count never falls until the replica restarts. At the default of 5 per user and 200 per replica, spread over enough accounts, it can also lock the whole replica out with 503.
   - No test covers a dropped request; `tests/mcp_auth/sessions.rs` only covers refused and failed responses (`a_request_that_opens_no_session_holds_no_slot`).
   - Fix: make the reservation an RAII guard that releases on `Drop` unless it was handed to the owner table. Add a test that polls the gate future once, drops it, and asserts `counted_sessions == 0`.

2. **The logging proofs are flaky and can degrade to a vacuous S10 check.** `crates/hub-mcp/tests/mcp_auth/logging.rs:67,173,256` use `tracing::subscriber::set_default`.
   - `cargo test -p hub-mcp --test mcp_auth the_` failed 5 of 15 runs at `logging.rs:312` (`one line per call: left: 0, right: 9`), only in `every_call_line_carries_the_stable_user_id_and_nothing_else_about_the_caller`. The captured log held only an rmcp WARN line, so every `hub_mcp::call` event was lost.
   - The same tests passed 20 of 20 as `logging::` alone and 20 of 20 in the full `mcp_auth` binary, so `just verify-07` usually stays green. It is still a real defect in the test setup, not the product.
   - The crate already documents the cause three times and avoids it elsewhere (`rmcp_spike.rs:358`, `stub_protocol.rs:64`, `confirm.rs:1386`: a scoped `set_default` loses events when other tests run concurrently; they use one process-wide subscriber). The three `logging.rs` tests did not follow.
   - `log_capture_has_no_tokens_args_or_output` only asserts absence, plus a line the test emits itself. When events are lost it passes without having captured any product log line, so the S10/S21 proof silently becomes a no-op.
   - Fix: use the per-thread capture in `confirm::capture` (`confirm.rs:1388`) in all three tests. Add a positive assertion that a product log line was captured in `log_capture_has_no_tokens_args_or_output`.

## Non-blocking

Verdicts on the items the orchestrator asked the reviewer to judge (none should block merge):

1. **D-1 (`/mcp` is a `public` self-authenticating route): accept.** The gate (`guard.rs:119`) runs before rmcp on POST, GET and DELETE. Its order is length, Host, Origin, bearer token, `Users::require_active`, then the session owner. The route-table pins hold: `every_mcp_route_has_a_declared_guard`, `the_routes_of_each_mode_are_exactly_these`, `unmatched_path_under_mcp_is_default_deny` and `every_method_of_the_endpoint_is_gated`. The hub's `mcp_routes_pass_the_hub_route_table_check` and `mcp_default_deny_holds` pass. The loss is that a future `public` method added under `/mcp` would not be caught structurally; the named tests would catch it.
2. **D-2 (S3 met by the Origin check only): accept, with the condition tracked.** `check_origin` (`auth/routes.rs:133`) requires the exact public origin and runs before the body is read. The cookie is SameSite=Lax, and a forged create only yields a token the attacker cannot read. Nothing is exposed because `main` never mounts `McpModule`, and the hub composes no `CsrfLayer` for any module today. The D109 condition is workable: `crates/hub-identity/src/csrf.rs` lets requests with no session cookie through, so wrapping every non-GET route will not break cookie-less `/mcp`. The PR text should make "compose `CsrfLayer` and assert it in `mcp_module.rs`" a go-live checklist item next to `just mcp-t0-status --strict`.
3. **D92a (cold `read_file` p95 about 52-63 ms against the 50 ms screen): accept.** The cost is `engine::index_settings`, whose own accepted budget is 60 ms; it is paid once per file version per replica (`tools/cache.rs` keys on the SHA-256 of the bytes). The registered 300 ms budget passes; warm calls take about 2 ms. It is still a parse-at-call-time pattern; the clean fix remains D92 option C.
4. **D92b (`get_effective_config` returns `text` null and every value typed `other`): accept, with one cheap mitigation.** For a resource file the result reads "0 of 0 settings shown" with `text: null` (`tools/read/effective.rs:153`). A model will read that as an empty file. Add a fixed sentence to the summary when `effective.role == Resource` or `text` is absent, pointing to `get_served_config`. This needs no contract change.

Other findings:

5. **Port calls without a timeout in this crate (rule 5).** `McpHandler::call` (`server/handler.rs:135-139`) and `list_tools` (`handler.rs:230-235`) await `Users::require_active` without one. rmcp has already sent the SSE head, so the hub's head timeout does not apply. `BearerAuth::authenticate` (`auth/mod.rs:106-112`, called at `guard.rs:143`) awaits the verifier and `Users` the same way. The step-5 re-check in `confirm/gate.rs` and every registry call are bounded. 03a's `Users` has a 10 s `DB_TIMEOUT`, so this is bounded in practice. Wrap the three call sites in `tokio::time::timeout(read_timeout)` and map the timeout to `Unavailable`; fold this into the fix round.
6. **`get_served_config` does superlinear CPU work on the async thread for large files.** `tools/read/served.rs:149` runs `fit_lines` over every line, while `read_doc` caps `available` at 200 lines (`docs.rs:150`). Measured in a debug build, second call: 146 KB took 27 ms, a 2 MiB file took 1.34 s; scaling by the debug-to-release ratio of the P9 bench (about 11x) puts the 2 MiB case near 120 ms of synchronous CPU in release. The P9 bench only covers the 146 KB file, so the budget is not violated at target scale. This is a rule-4 and P9 risk at the 2 MiB upper bound. Apply `DEFAULT_LINES` as `read_doc` does, which also matches the "200 lines or 16 KB" convention in `docs/mcp-tools.md`.
7. **`ToolHost.writes` is still public.** The gate-only-caller guard (`tests/mcp_auth/confirm.rs:1621-1723`) is now a whole-file scan with positive controls for broken chains, aliases and path calls, and it is adequate. A scan cannot see a macro-generated call, so keep the type-level narrow handle on the follow-up list.
8. **`tools/list` is about 112 KB with `outputSchema` on** (`server/mod.rs:118`, `advertise_output_schemas: true`), against about 31 KB without. Decide the default with the T0 results; the reviewer would default it off until T0 shows a need. The new bound `(100_000..=125_000)` at `contract_schemas.rs:537` also fails when the schemas shrink, which is odd for a size guard.
9. **Decision numbers collide with `origin/agent/05-registry-read-api`.** Branch 05 defines its own D91-D108, and its D107 already clashes with main's. `examples/t0_status.rs` parses the literal `- **D91**` heading, and about 25 files reference D91, D92 and D109. Agree the merge order; the second branch to merge must renumber.
10. **D109 provenance cannot be shown by git.** The text says "Accepted by the human ... (hsetially)", but the commit author is the agent. The human's review of the PR diff of `docs/decisions.md` is the real record; say so in the PR text.
11. **S15 delimited blocks only exist in `data_block` mode.** The default `summary` mode labels text with `untrusted: true` and `label` but does not delimit it. The threat model says so honestly; decide the mode with the T0 results.
12. **Plan traceability gaps, both disclosed in the evidence file.** `docs_tree::read_doc_takes_only_doc_paths` and `docs_tree::dotdot_and_backslash_are_refused` do not exist by name; the same cases are in `args::every_path_argument_uses_a_domain_parser` (`args.rs:438-452`). The goldens for the 401 challenge, `tools/list` per role and the cursor are inline assertions, not files. The other 138 of 140 names grep as real `fn`s.
13. **`message::raise_pr` builds its own `Redactor::new()`** (`confirm/message.rs:403`) instead of the host's. If Q20 tuning ever changes the host's `ScanConfig`, the question would be scanned with the default. The scan runs synchronously on up to 64 KiB, about 0.4 ms.
14. **Residuals to carry into the PR text:** the `raise_pr` question shows at most 240 characters of the body, the rest is posted unread under the user's GitHub identity (in the threat model); `tools.notify_services.enabled` has no environment key; the plugin package still copies the skill's dev-only `evals/evals.json`; the hub integration must give `Mcp-Session-Id` affinity (prompt 09).

## Evidence re-run (reviewer)

- `cargo fmt --all -- --check`: exit 0. `cargo clippy -p hub-mcp -p hub --all-targets -- -D warnings`: exit 0; `cargo clippy -p hub-mcp -- -D warnings` (no dev features) and `cargo check -p hub`: exit 0.
- `cargo test -p hub-mcp --lib --tests --no-fail-fast` with `LK_TEST_PG_ADMIN_URL` (PG 16.15, no pgvector): 482 passed, 0 failed, 0 SKIPPED lines (lib 36, config 10, cursor_admin_doc 18, justfile_gate 7, mcp_auth 329, plugin_package 19, rmcp_spike 13, stub_protocol 24, t0_status 15, threat_model 11); the 11 `auth_pat_pg` tests ran on the real server.
- Flake runs, same `mcp_auth` binary: `logging::` alone 0/20 failed; the full binary 0/20 failed; filter `the_` 5/15 failed (`logging.rs:312`).
- Scratch probe: five dropped initializes left `counted=5, live=0` and the next `initialize` got 429; `get_served_config` timed as in item 6.
- `cargo test -p hub --test mcp_module`: 5 passed. `cargo test -p xtask --test mcp_tools_doc --test budgets_registry`: 23 and 8 passed.
- `cargo deny check` and `cargo audit` clean (workspace and fuzz crate). All 23 new `Cargo.lock` packages at least 15 days old, none yanked.
- `just verify-07`: fails at `docker-check`. `just mcp-t0-status`: every cell NOT RUN; `--strict` exits 1.
- `cargo xtask bench-check`: 0 pass, 46 fail, 26 unmet of 72 because `target/criterion` does not exist here; `just mcp-bench` was NOT re-run (disk), the P9 numbers are the implementer's. The round-2 diff does not touch the read path.
- Not run by the reviewer: `just verify` as one command, `cargo test --workspace`, `just web-verify`, libFuzzer runs, `osv-scanner`, anything needing pgvector or Docker, a full `cargo clippy --workspace`.
- Spec chain: plan approved at `eeceade`, `git diff eeceade HEAD -- plans/07-mcp-server.md` is empty; commits T0 to T7 in order; only `perf/budgets.toml` changed among contract paths (21 flips, approved by C2); Extra-paths match the plan.
- Security trace: S4, S7, S9, S11, S14 (gap: dropped-request leak, blocking 1), S14b, S15, S21 (`logging::*` flaky, blocking 2), P9 (numbers not re-run) each traced to code and tests; threat model updated.
