# Review of prompt 18 (hub assembly): round 2

Reviewer: independent `reviewer` agent, run per `prompts/REVIEW.md`. Base: `origin/main`. Branch head reviewed: `f972c6b` (the merge of `origin/main` that brings in PR 39). Round 1 is in `plans/18-hub-assembly.review.md`. The text below is the reviewer's report, saved as returned.

VERDICT: APPROVE

**Blocking**

None. Round 1's two blocking findings are fixed, and for every new test I made it fail by removing the control it proves, then restored the file by checksum. Every gate I could run is green, and the flake is explained and gone. The things I could not run are listed at the end of Evidence. They are environment limits, not code findings, but the literal `just verify-18`, `just verify` and osv-scanner must be green in CI before the human merges.

**Non-blocking**

1. Text that went stale when PR 39 was merged in (`ca_key_use` is green, 6 passed) still says it is red.
   - `Justfile` lines 394-396 (the `hub-test` comment).
   - `docs/decisions.md` D145 "Proof" (about line 500) and "Asked of the human" (about line 503).
   - `docs/threat-model.md` line 1027 (the S5 row).
   - `plans/18-hub-assembly.blockers.md` ("What remains"), and the evidence file, which the orchestrator will refresh.
   - Two evidence counts are also stale: `events_sse (32)` in the S2 bullet and `refuses_to_start (32)` in the S7 bullet are 33 and 33.
   - D145 says "Decided by the human, in chat". That is relayed by the orchestrator and not checkable from the repository, as D145 itself says. The human should confirm it on the PR.
2. Round-1 non-blocking 3 (equal CA references) is fixed. Its recorded residual is acceptable, with one cheap hardening idea.
   - The check compares `KeyRef` and the certificate secret names as written (`crates/hub/src/config/platform.rs` lines 76-83).
   - A project id versus a project number for one key is not caught. Neither is `.../cryptoKeyVersions/1` versus `/01`, because `CryptoKeyVersionName::parse` accepts any digits.
   - The residual is recorded in `docs/threat-model.md` under "Known residual risks": "refused only when the two references are equal as written (D144)".
   - I judge it acceptable. It needs an operator who already controls the settings to write one key two ways on purpose. The replica service's name check (`crates/hub-platform/src/gateway/replica_service.rs`) is the second line. The check is a misconfiguration guard, not a defence against an attacker.
   - Suggested hardening, not required: at start, both CA certificates are already decoded. Refuse when their public keys are equal. That catches id versus number, version spelling, and one certificate stored under two secret names.
3. Looping a single hub test is slow, because of the harness.
   - `crates/hub/tests/support/child.rs:56` runs a nested `cargo build -p hub --example hub_dev`. It resolves differently from the outer `cargo test`, so every standalone `cargo test -p hub --test process` (also `registry_writes`, `dev_login`, `docs_unavailable`) recompiles about 25 crates twice, roughly 190 s per iteration.
   - It is a one-time cost inside `just hub-test`. I ran the already-built test binary for the bulk of the flake runs.
4. Time-based negative waits remain (round-1 non-blocking 6, left as instructed).
   - `crates/hub/tests/process.rs:355` (1.5 s) is now preceded by a positive wait.
   - `identity_module.rs:638,651,691` are unchanged.
5. Round-1 non-blocking 9 still stands: commit `3e68646` (`prompts/18-hub-assembly.md`, `prompts/README.md`, the base of `docs/hub-assembly-survey.md`) rides along from the docs branch. Settle the merge order with that docs PR.
6. Open follow-ups, unchanged from round 1:
   - S11's wording in `docs/security.md` still says 4 MiB (D137 asks for a follow-up).
   - Eight traceability names in the frozen plan differ from the code. The evidence maps all eight and I confirmed each target exists.
   - P11 is measured with software edges and an empty registry.

**Evidence re-run**

Paths are relative to `/home/user/lanekeeper/.worktrees/18-hub-assembly`. The tree was identical to HEAD `f972c6b` at the end, and `git status` was empty.

Round-1 findings:
- **Blocking 1 (B1), fixed.**
  - `cargo test -p hub-platform --test ca_key_use`: 6 passed.
  - `ALLOWED` holds `("web_tls.rs","decode")` through PR 39 (`f52775d`, merge `5ae2ec0`). `git diff origin/main...HEAD` no longer touches that file.
  - `Justfile` `hub-test` runs it on its own line (`just --dry-run hub-test` shows it), and `justfile_gate::verify_18_runs_every_sub_recipe` holds the line (8 passed).
  - D145 and the threat-model rows exist.
- **Blocking 2, fixed:** `body_limits::at_most_eight_large_bodies_are_read_at_once`.
  - `body_limits` is 5 passed, and 8 of 8 repeat runs pass.
  - Each mutation of `crates/hub/src/limits.rs` was restored, with sha256 `f6e26981fd36bcd1003dea63ae5fcf4723a6677e24dde0beeb76271abe5a345b` before and after and `git diff` clean:
    - Acquire removed: red at `body_limits.rs:469`, `5 MiB, no declared length: left 200 right 429`.
    - Permit dropped at once, so never held: red at the same assertion.
    - Declared-small bodies counted: red at `:512`, `declared at exactly 4 MiB: 429`.
  - The header comment and D137's "Proof" line now describe the middleware test. The old unit test is described as proving tokio's semaphore.
- **Non-blocking 1, fixed:** `route_table_full::every_real_state_changing_route_refuses_a_session_without_the_token_and_the_origin`.
  - It walks the real `Running::routes()`, with five variants per route.
  - With the `CsrfLayer` line removed from `compose.rs`: red, `POST /hooks/github with the cookies only: 503, want 403`. Restored (sha256 `7b6b646b...`).
  - `route_table_full` is 8 passed, and 3 of 3 repeat runs pass. `csrf_all_routes` is 5 passed.
- **Non-blocking 2, fixed:** `dev_login::the_dev_login_refuses_a_database_that_is_not_on_this_machine`.
  - With the check disabled: red, `left Some(1), right Some(2)` for `db.example.test`. Restored (sha256 `7dee49ac...`).
  - `dev_login` is 3 passed.
  - The helpers still cannot reach the release binary or the image:
    - `crates/hub/src` has no reference to `dev_login`, `hub_dev`, `soft_kms` or `LK_DEV_`.
    - The Dockerfile builds only `cargo build --release --locked -p hub`.
    - `release_build` is 9 passed.
    - The `hub-lint` release-graph check passed.
- **Non-blocking 3, fixed:**
  - With both equality checks removed from `platform.rs`, three tests are red: `config::the_two_cas_may_not_share_a_key_or_a_certificate`, `refuses_to_start::a_second_ca_that_shares_the_first_ones_key_or_certificate_is_refused_by_name` (`left Some(1), right Some(2)`), and `corpus_replay` (2 failed). Restored (sha256 `b16cec69...`).
  - `refuses_to_start` is 33 passed and `corpus_replay` is 19 passed.
  - The residual is recorded and judged acceptable (Non-blocking 2).
- **Non-blocking 4, fixed:** `binding_valid` now defaults to false.
  - With the default flipped to true: `events_sse::an_implementation_that_names_a_session_and_cannot_answer_for_it_fails_closed` is red. Restored (sha256 `7b6a46f7...`).
  - `events_sse` is 33 passed, and `hub-platform` `--lib` is 32 passed.
  - Implementers checked:
    - `BearerAuth` (`hub-mcp/src/auth/mod.rs:127`), the hub-mcp test double `Fails`, and `FakeSessionAuth` return no binding, so `binding_valid` is never asked. The 346 hub-mcp `mcp_auth` tests pass.
    - `CookieAuth` (`hub/src/session_auth.rs:116,128`) and the test `BoundAuth` override both methods.
    - `DenySessions` overrides neither.
    - `NamesASessionOnly` is the new fail-closed case.
  - `git diff origin/main...HEAD -- crates/hub-platform` touches exactly `session_auth.rs`, `events/sse.rs` and `tests/events_sse.rs`, the three approved files.
- **Non-blocking 5:** no change was needed, as I said in round 1.
- **Non-blocking 6, flake of `process::a_second_signal_exits_at_once`:**
  - `LK_REQUIRE_DOCKER=1 cargo test -p hub --test process a_second_signal_exits_at_once`: 4 of 4 passed, then I stopped, because each run took about 190 s (Non-blocking 3).
  - The built test binary, run directly 30 times: 30 of 30 passed.
  - The same binary under 8 CPU burners (load average about 8 on 4 vCPU): 20 of 20 passed. That is 54 of 54 with the fix.
  - With the new positive wait removed (temporary edit of `process.rs`, restored, sha256 `429fa0b0...`), the same load gave 19 of 20. The one failure was the old symptom: `the hub left while a request was in flight, before any second signal`, with the log showing `stop requested` then `stopped`.
  - So the cause is the race the implementer named: a TERM arrives before the hub has read the request head, and a graceful shutdown closes that idle connection. The wait fixes it.
  - The wait is not vacuous: `lanekeeper_http_requests_in_flight` is counted only on the web router (`edge.rs:65`), not on the ops listener that `/metrics` is read from.
  - The whole `process` target in the full run: 8 passed.
- **Non-blocking 7, fixed:** `registry_writes::the_webhook_still_requires_a_valid_hmac_through_the_composed_hub`.
  - With the HMAC check bypassed in `crates/hub-platform/src/git/webhook.rs` (`if false && !digest_matches(...)`): red, `no signature header: 202, want 401`. Restored (sha256 `a74a47ed...`).
  - `registry_writes` is 7 passed, and 3 of 3 repeat runs pass.
- **Non-blocking 8, fixed:** D138, D142 and D144 are reworded; `threat_model` is 4 passed.
- **Non-blocking 9:** left (Non-blocking 5 above).
- **Non-blocking 10, fixed:** the `lib.rs` header.
- **Non-blocking 11:** left (Non-blocking 6 above).

Gates and scope:
- Scope: outside `crates/hub` the diff is only the approved Extra-paths plus the shared files. `perf/budgets.toml` is exactly the two additive CC1 entries. No other contract path is touched.
- Plan: changed only in `8ca3452` and `53cb876` (approved by a human). `git diff 53cb876 HEAD -- plans/18-hub-assembly.md` is empty.
- Commits: T1 to T8 are in order, and later commits are `18/fix`, `18/evidence`, `18/blocked`, `18/review`, `18/answers` plus two merges.
- Traceability: all 63 names in the plan's traceability tables exist as `fn`s, except the 8 renames the evidence maps. All 8 mapped targets exist.
- `just hub-lint`: exit 0.
  - fmt, `clippy -p hub --all-targets --all-features` (cached, 0.54 s), the release graph with none of the 7 test features.
  - `cargo deny` ok (`advisories ok, bans ok, licenses ok, sources ok`).
  - `cargo audit`: 10 allowed zstd-safe warnings.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0, a fresh 4m00s run.
- `cargo test -p hub --all-features`, one target at a time with `CARGO_INCREMENTAL=0`, `LK_REQUIRE_DOCKER=1` and big binaries deleted after each: 522 passed, 0 failed.
  - `--lib` is 236, plus all 35 files of `crates/hub/tests/`, matching the evidence's 522.
  - Counts: body_limits 5, csrf_all_routes 5, route_table_full 8, refuses_to_start 33, hardening 7, justfile_gate 8, process 8, registry_writes 7, corpus_replay 19, identity_module 16, dev_login 3, threat_model 4, release_build 9, platform_cluster 10, mcp_module 11, migrate 18, seed 15, and the remaining files.
- `platform_cluster`: 6 of 6 runs passed, 10 tests each.
- `cargo test -p hub-platform --test events_sse`: 33 passed.
- `cargo test -p hub-platform` (lib 32 and all 51 test targets): all pass.
  - The first pass hit ENOSPC halfway. I discarded it and re-ran from `blob_store` on.
  - Those failures were disk-full artifacts and all pass on re-run.
- `cargo test -p hub-mcp`: lib 42, `mcp_auth` 346, config 10, `cursor_admin_doc` 18, `justfile_gate` 7, `plugin_package` 19, `rmcp_spike` 13, `stub_protocol` 24, `t0_status` 15, `threat_model` 11, all pass.
- `cargo test -p xtask --test budgets_registry`: 8 passed.
- `just hub-fuzz`: exit 0. `hub_cli_args`, `hub_pem`, `hub_seed_file` and `hub_settings` ran 30 s each without a crash (1,565,333 / 404,120 / 181,856 / 22,791 runs). `cargo deny` on the fuzz lockfile is ok, and so is `cargo audit` on it.
- `just hub-bench`: exit 0.
  - P7.auth_composed 0.040 ms p95 (budget 2).
  - P7.auth_composed_cold 1.420 ms p95 (budget 2).
  - P11 143.2 ms p95 (budget 10,000).
- `cargo bench -p hub-platform --bench sse_delivery --bench push_to_drift_webhook`: P13 5.07 ms p95 (budget 1,000), P2 23.5 ms p95 (budget 10,000).
- `cargo xtask bench-check` on the whole registry: 11 pass, 62 fail, 2 unmet.
  - All 62 FAILs are "registered but no result", from other prompts' benchmarks. I checked this: zero FAILs have another reason.
  - The 2 unmet are P7.docs_search and P7.feature_status (prompt 14).
  - I freshly re-ran P2, P7.auth_composed, P7.auth_composed_cold, P11 and P13.
  - P11.memory_mib, P12.audit_record, P12.hub_writes_queries, P12.steady_state_query_ms, P2.hub_head_moved_to_drift and P10 come from earlier result files and were not re-run. The fix round did not touch them.
- Security trace for the fix round:
  - S2: `events_sse::an_implementation_that_names_a_session_and_cannot_answer_for_it_fails_closed`.
  - S3: `route_table_full::every_real_state_changing_route_...`.
  - S5: `config::the_two_cas_may_not_share_a_key_or_a_certificate` and `ca_key_use` (6 passed).
  - S11: `body_limits::at_most_eight_large_bodies_are_read_at_once`.
  - S18: `registry_writes::the_webhook_still_requires_a_valid_hmac_through_the_composed_hub`.
- Threat model updated: yes, the "Hub assembly (18)" section. It has the new S5 rows, the web-key residual, the webhook row, the `binding_valid` row and the dev-login database note, all guarded by `threat_model` (4 passed).

NOT RUN:
- The literal `just verify-18` and `just verify` as single commands. About 24 GB of debug test binaries do not fit in about 5 GB. I ran their parts: `hub-lint` (fmt, clippy, release graph, deny, audit), the four `hub-test` commands (the first per target), `hub-fuzz`, `hub-bench`, and workspace clippy.
- `hub-tools-check` as its own recipe. Its Docker, nightly and cargo-fuzz checks ran through `hub-fuzz`'s `fuzz-tools-check`.
- Tests of crates with an empty diff against `origin/main` and no use of the changed `hub-platform` files: domain, ports, engine, hub-identity, hub-registry, hub-writes, hub-docsearch, agent, sentinel, proto, and xtask apart from `budgets_registry`. Also doc tests and `hub --bins` (0 tests).
- `web-verify` (`web/` is untouched).
- osv-scanner, kind, helm, kubeconform, conftest and the prompt-09 chart checks, which are not installed.
- Cleanup: I deleted the git-ignored `crates/hub/fuzz/target` (3.9 GB), big debug test executables and `target/debug/incremental`, all build artifacts. I touched no tracked file except the temporary mutations above, each restored and checked by sha256 and `git diff`.
