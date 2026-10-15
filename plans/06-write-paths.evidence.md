# Evidence 06: Write paths, approvals, token vault, PRs and notifications

Written in the FINAL step at commit `7ca194e` (T1 to T11 all committed, working tree clean). Every number below is from a run in this
session on 2026-10-15; nothing is copied from an earlier session. Nothing was fixed in this step: no product bug and no gate change
was needed, so there is no `06/FINAL: fix` commit.

**Review round 1 (head `93e5d56` reviewed, fixes in the `06/review:` commits) changed code, tests and this file.** The numbers and
tables below are the FINAL step's, taken at `7ca194e`, except where the section "Review round 1" says otherwise; that section has the
re-run after the fixes (549 tests in `hub-writes`, not 544).

## Summary

- **`just verify-06`: PASS** (literal run, exit 0, 21 m 39 s wall). All five sub-recipes ran: tools check, lint, 544 tests, 6 fuzz targets
  at 30 s each, and the filtered `bench-check` (P8 and P12.hub_writes_queries both PASS).
- **`just verify`: NOT GREEN, and not because of 06.** `cargo fmt`, `cargo clippy --workspace`, `cargo deny`, `cargo audit` and all of
  `web-verify` pass. `cargo test --workspace` has exactly one failing test, `hub/tests/pg_tls.rs`, which needs a Docker daemon
  (`LK_REQUIRE_DOCKER=1` and no daemon in this sandbox). Environment limit; I did not work around it.
- **`cargo xtask bench-check`: FAIL (exit 1), 25 pass, 2 fail, 26 unmet of 53.** The two fails are other prompts' budgets
  (`P6.index_settings` of 04 and `P12.audit_record` of 03a); the 26 unmet are budgets of prompts not built yet. This prompt's two
  budgets (`P8`, `P12.hub_writes_queries`) PASS with large margins.
- Disk was the main constraint (see "How the gates were run"). It forced staged runs of `cargo test --workspace`, and a build with debug
  info off for the second half. It changed no test, budget or gate.

## Review round 1 (the reviewer's CHANGES REQUESTED on `93e5d56`)

Fixes are separate commits, none amends an earlier one, nothing is pushed.

| Finding | Status | What changed | Proof |
|---|---|---|---|
| BLOCKING 2: `data_key_is_a_secret_that_zeroizes` is vacuous | **Fixed** (`2b8ccfc`) | `seal` takes its key from `new_data_key()` (a `Secret<[u8; 32]>`, its raw bytes in `Zeroizing`) through `seal_with`, which owns the key and drops it before it returns, on success and on every error. The old test is removed. | `data_key_is_zeroized_after_wrap`, `data_key_is_zeroized_when_kms_refuses`, `the_data_key_seal_uses_is_a_fresh_secret`. **Mutation check:** adding `std::mem::forget(dek)` in `seal_with` fails `data_key_is_zeroized_after_wrap` ("the data key outlived the call to seal"); restored. |
| BLOCKING 1: job-time use of the token against S8/D42 | **Documented; DECIDED by the human on 2026-10-15: allowed, D107** | Design unchanged. See "Assumptions" and "Deviations from the plan"; the threat model's residual risk now says it was decided. D107 is recorded on branch `claude/brave-cori-2cnesi` (commit `564948a`) and lands with main; it is not yet in this branch's `docs/decisions.md`. | `tests/threat_model.rs` (10 passed) |
| NB1: `set_intent` ignores `rows_affected` | **Fixed** (`18a7ef2`) | `set_intent` errors unless exactly one unfinished claim was updated; `carry_out` aborts before sending. | `a_request_whose_claim_was_cleared_before_the_send_does_not_send` in `tests/reconcile.rs`; it failed first (1 `WriteFile` sent), a stalling registry double lets the reconciler run mid-request. |
| NB3: expiry takes no `proposal:{id}` lease | **Fixed** (`be664fa`) | The job takes the lease per proposal and skips one whose lease is held (`ExpireReport.busy`). What is left is a command that outlasts the 60 s lease; the old race test now models that by hand. Threat-model text and proofs updated. | `the_expiry_job_skips_a_proposal_whose_approval_is_at_the_agent` (failed first: the job marked it); `a_proposal_that_expires_while_its_write_is_on_the_way_is_still_audited_and_left_expired` still passes, for the remaining case. |
| NB5: `POST /drafts` ignores its `Idempotency-Key` | **Left, as a residual risk** (`b9b518c`) | Honouring it needs a new stored-response kind, a claim replay for it, and the claim finished in the insert's transaction, in the shared idempotency store: not simple. Bounded by the open-draft limit (100) and the one-day expiry. | `a_retried_draft_with_the_same_key_makes_a_second_draft_within_the_open_limit` pins the behaviour. |
| NB10: a timed-out command can still run at the agent later | **Added to the threat model** (`b9b518c`) | Verified: `WriteFile` and `DeleteFile` have no deadline in `proto/agent.proto`, and the gateway's timeout only ends the hub's wait (`request` in `hub-platform/src/gateway/local.rs`); the command already sent is not retracted. The expected hash bounds it. A fix is a proto change (contract). | `tests/threat_model.rs` green |
| NB6, NB11: evidence only | **Fixed** (this commit) | 2 MiB against the plan's 3 MiB stated under "Assumptions"; the mapping of `write_above_3_mib_is_rejected_too_large` says it is a 2 MiB test; `paths_are_typed_before_the_pipeline` is proved by `a_request_that_is_not_well_formed_never_reaches_the_service`; `data_key_is_zeroized_after_wrap` maps to the new test. | this file |
| NB2 (lost restart/notify answers unaudited), NB4 (KMS call in a transaction), NB7, NB8, NB9, NB12 | **Left** | Documented residual risks already, or out of proportion for this round. | n/a |

**What the data-key tests do not show.** Whether the key's memory is zeroed after the drop cannot be observed from safe Rust
(`forbid(unsafe_code)`; reading freed memory needs `unsafe`). The tests show the things that decide it: the key is a `Secret` (the
type KMS takes) and is the key the token was encrypted under; no copy of it is in the returned `Sealed`; it is dropped by the time
`seal` returns, on success and on a KMS refusal, and not before KMS has used it. That `Secret` wipes its bytes on drop is the property of
`domain::Secret` (`secrecy::SecretBox`), not shown here. The raw bytes the key is made from are in `Zeroizing`; that is not observable and
is not tested.

### Re-run after the fixes (head `b9b518c`, plus this file)

Environment as in "How the gates were run": local Postgres 16.15, `LK_REQUIRE_DOCKER=1`, `CARGO_INCREMENTAL=0`, debug info off, disk 8 to 9 GB free.

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy -p hub-writes --all-targets -- -D warnings` | PASS |
| `cargo test -p hub-writes`, one binary at a time | PASS: lib 200, actions 215, routes 42, matrix 6, edit 17, fidelity 14, idempotency 9, logs 3, explain 11, p8 1, reconcile 7, corpus_replay 9, threat_model 10, justfile_gate 5 = **549 passed, 0 failed** (544 before: lib +2, three data-key tests added and one removed; actions +2, the expiry-skip test and the draft-key test; reconcile +1); doc tests 0 |
| `cargo deny check` | PASS (`advisories ok, bans ok, licenses ok, sources ok`) |
| `just writes-bench` (release `cargo bench -p hub-writes`, then `bench-check` on this prompt's budgets) | PASS: `P8 33.904 p95 <= 1500 ms`, `P12.hub_writes_queries 9.972 p95 <= 50 ms` |

**NOT RUN in this round:** `just verify-06` as one literal invocation and its 30 s fuzz targets (the code under the fuzz targets, the
parsers, was not touched; `corpus_replay` ran), `just verify`, and the full-registry `cargo xtask bench-check`; the gaps recorded in "What was NOT RUN" still apply. `osv-scanner` is not installed. The proptest regression seed files were not touched.

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-06` (literal, one invocation) | **PASS**, exit 0, real 21 m 39 s | `15 test binaries: 544 passed, 0 failed`; `writes-fuzz: writes_request_json writes_upload_multipart writes_preview_id writes_github_token writes_github_responses writes_git_data_responses ran 30 s each without a crash`; `PASS P8 35.594 p95 <= 1500 ms`; `PASS P12.hub_writes_queries 9.324 p95 <= 50 ms`; `bench-check (default): 2 pass, 0 fail, 0 unmet of 2 budgets` |
| `just writes-tools-check` | PASS, exit 0 | `cargo-deny 0.19.9`, `cargo-audit-audit 0.22.2` as pinned; `LK_TEST_PG_ADMIN_URL` set |
| `just writes-lint` | PASS, exit 0 | `cargo fmt --all -- --check` clean; `cargo clippy -p hub-writes --all-targets -- -D warnings` finished |
| `just writes-test` | PASS, exit 0 (first run, with debug info) | 198 lib + 213 `actions` + 9 `corpus_replay` + 17 `edit` + 11 `explain` + 14 `fidelity` + 9 `idempotency` + 5 `justfile_gate` + 3 `logs` + 6 `matrix` + 1 `p8` + 6 `reconcile` + 42 `routes` + 10 `threat_model` = 544 passed, 0 failed, 0 ignored; the same 544 again inside the literal `verify-06` |
| `just writes-fuzz` | PASS, exit 0 | `cargo deny` on `crates/hub-writes/fuzz`: `advisories ok, bans ok, licenses ok, sources ok`; `cargo audit --file crates/hub-writes/fuzz/Cargo.lock`: 1329 advisories loaded, 476 crate dependencies, no vulnerability; 6 targets x 30 s, `Done 228901 / 1081765 / 288426 / 1548481 / 1120690 / 454032 runs in 31 second(s)` (in target order), no crash |
| `just writes-bench` | PASS, exit 0 | see "Benchmarks against budgets" |
| `just verify`, part `cargo fmt --all -- --check` | PASS | exit 0 |
| `just verify`, part `cargo clippy --workspace --all-targets -- -D warnings` | PASS | `Finished dev profile ... in 2m 56s`, exit 0 |
| `just verify`, part `cargo test --workspace` | **FAIL, 1 test, environment** | staged per package, see the table below; `hub/tests/pg_tls.rs::pg_pool_connects_to_a_real_tls_postgres_and_refuses_the_wrong_ca`: `LK_REQUIRE_DOCKER=1 but the Postgres container cannot start: failed to initialize a docker client: Socket not found: /var/run/docker.sock` |
| `just verify`, part `cargo deny check` | PASS | `advisories ok, bans ok, licenses ok, sources ok`, exit 0 (19 `duplicate` warnings and 1 `license-exception-not-encountered`, all warnings) |
| `just verify`, part `cargo audit` | PASS | `Loaded 1329 security advisories`, `Scanning Cargo.lock for vulnerabilities (618 crate dependencies)`, exit 0, no warnings |
| `just verify`, part `just web-verify` | PASS, exit 0 | `pnpm install --frozen-lockfile` (320 packages), `pnpm lint`, `pnpm typecheck`, `vitest --run`: `1 passed (1)`, `vite build`: `dist/assets/index-O9gXnvRs.js 224.10 kB, gzip 69.65 kB`; the web app is still the scaffold (one test) |
| `cargo xtask bench-check` (full registry, after `cargo bench --workspace --no-fail-fast`) | **FAIL, exit 1** | `bench-check (default): 25 pass, 2 fail, 26 unmet of 53 budgets`; the two FAIL rows are not 06's, see below |

### `cargo test --workspace`, package by package (`cargo test -p <pkg>`, doc tests included)

| Package | Result |
|---|---|
| domain | 97 passed, 0 failed |
| ports | 122 passed, 0 failed |
| proto | 25 passed, 0 failed |
| engine | 538 passed, 0 failed, 7 ignored (the `#[ignore]`d target-scale and real-snapshot tests, 04's) |
| xtask | 123 passed, 0 failed, 1 ignored |
| sentinel, hub-registry, hub-docsearch, hub-mcp | 0 tests (stubs), exit 0 |
| agent | 742 passed, 0 failed, 1 ignored |
| hub-identity | 544 passed, 0 failed, 2 ignored |
| hub-platform | 512 passed, 0 failed, 5 ignored |
| hub-writes (`cargo test -p hub-writes`, with doc tests) | 544 passed, 0 failed |
| hub (`--no-fail-fast`, so no later binary is skipped) | 123 passed, **1 failed** (`pg_tls`, Docker), 0 ignored; every other `hub` test binary passes, including `tls`, `limits`, `guard`, `headers`, `static_files` |

Total: 3,370 passed, 1 failed (Docker), 16 ignored by their owners.

### The two over-budget rows in the full `bench-check` (not this prompt's)

| Budget | Owner | Measured p95 (two separate `bench-check` runs) | Budget | Note |
|---|---|---|---|---|
| `P6.index_settings` (`engine/index_settings_6000_lines`) | 04 | 71.472 ms, then 65.319 ms (criterion mean 51.3 ms, interval 49.9 to 53.0) | 60 ms | `plans/04-fact-engine.evidence.md` already records this one as measured outside its budget (46.348 ms p95 against the then-proposed 40 ms). This sandbox is slower than that run. |
| `P12.audit_record` (`hub-identity/audit_record`) | 03a | 4.124 ms, then 4.569 ms (criterion mean 1.80 ms, interval 1.65 to 1.97) | 3 ms | `plans/03a-hub-identity-audit.evidence.md` rows R3 record the same flapping near 3 ms (3.521 FAIL, 2.802 PASS, 3.183 FAIL) on a shared Postgres. |

I did not touch `crates/engine`, `crates/hub-identity` or `perf/budgets.toml` in this step (their thresholds are contract). A human decides
whether these are noise on a 4-core shared VM or real regressions; I reran each benchmark once more and the result did not move into budget.

## What was NOT RUN, and why

| Item | Why |
|---|---|
| `hub/tests/pg_tls.rs` (and `just docker-check`) | The `docker` client is installed but there is no daemon (`Socket not found: /var/run/docker.sock`). `pg_tls` starts a TLS Postgres through testcontainers. It fails, as designed, under `LK_REQUIRE_DOCKER=1`. 06 does not own it and does not touch `crates/hub`. |
| All testcontainers paths in the Docker mode | Replaced by a local Postgres 16.15 (Ubuntu package, pgvector and pg_trgm present), started with `pg_ctlcluster 16 main start` and reached through `LK_TEST_PG_ADMIN_URL`. This is the documented alternative in `writes-tools-check`. Postgres was stopped afterwards. |
| `just verify` as one literal invocation | The sandbox has about 5 GB of free disk and 11 GB of cached `target/debug`. A single `cargo test --workspace` failed with `No space left on device` at link time (agent, hub-identity, hub-platform and hub test binaries; first attempt). I therefore ran every part of `rust-verify` and `web-verify` separately, per package, with the commands above. The union is the same set of tests and checks. A literal run would stop at `pg_tls` anyway and never reach `web-verify`. |
| `just fuzz-smoke` (the root `fuzz/` targets of 01 to 04) | Not part of `verify-06` or `verify`, and owned by other prompts. 06's own six fuzz targets ran (above). |
| `osv-scanner` | AGENTS rule 15 names it, but it is not installed here, and neither `just verify` nor `verify-06` calls it. `cargo deny` and `cargo audit` ran instead. |
| `cargo sqlx prepare --check` | `hub-writes` uses runtime-checked queries (`sqlx::query` without the macros; the manifest says "no `.sqlx` metadata needed to build"), so there is no metadata of this crate to check. |
| Real GitHub (CRLF through the Git Data API, fine-grained tokens, the expiry header) | Not reachable from the sandbox. The plan's stop condition "GitHub's Git Data API cannot preserve CRLF" is proved only against the mock (`crlf_survives_the_git_data_api_blob` passes); **the manual check against the real API is NOT RUN** and still needs a human. |
| Real Cloud KMS, real Teams Workflows webhook, real agent on NFS | Not reachable. KMS, Teams and the agent are the `ports` fakes, a local webhook server and a fake agent. |
| Multi-replica P8 on a real network | The benchmark is loopback; `edit_through_a_second_replica_meets_p8` models the extra internal hop with a delay. Prompt 16 measures staging. |
| Flamegraph / stop-condition measurements | Not needed: P8 passed with a margin of about 40x. |

## How the gates were run (environment)

- `CARGO_TARGET_DIR=/home/user/lanekeeper/.worktrees/contract-06-writes/target`, `CARGO_INCREMENTAL=0`,
  `LK_TEST_PG_ADMIN_URL=postgres://postgres:...@127.0.0.1:5432/postgres`, `LK_REQUIRE_DOCKER=1` for every test and bench run.
- Disk (252 GB volume, about 5.8 GB free at the start). Test executables were deleted between packages. After the first workspace pass ran out
  of space, I deleted `target/debug` and `target/release` (rebuildable) and rebuilt with `CARGO_PROFILE_DEV_DEBUG=0
  CARGO_PROFILE_TEST_DEBUG=0` for the four packages that had failed (agent, hub-identity, hub-platform, hub), the full
  `cargo test -p hub-writes`, the benchmarks, and the literal `just verify-06`. Debug info does not change what a test asserts or what a
  release benchmark measures. The first `writes-test`, the fuzz run, `writes-lint`, the workspace `clippy` and the test runs of domain, ports, proto, engine, xtask, sentinel, hub-registry, hub-docsearch and hub-mcp ran with the default profile.
- Benchmarks ran in release (`cargo bench`, profile `release` with `lto = "thin"`, `codegen-units = 1`). `cargo xtask gen-fixtures --scale target`
  (80,120 NFS files, 40 swimlanes, 60 tenant branches, 22.5 s) was generated for the other crates' benchmarks.
  The full-registry run used `cargo bench --workspace --no-fail-fast` (the first plain `just bench` stopped at the agent bench because the
  target-scale fixtures did not exist yet; after generating them every bench completed, exit 0).
- The 26 UNMET rows are budgets whose benchmarks belong to prompts 05 (registry), 07 to 14 (UI, MCP, docs, adoption) that do not exist yet
  (`registered = false` in `perf/budgets.toml`): P5, P5.per_file_per_swimlane, P7.swimlane_list, file_tree, file_content_*, grid,
  whole_swimlane_compare, settings_search, text_search, docs_search, feature_status, compare_api, P9, P10.*, P12.steady_state_query_ms, P14,
  P1.hub_delta_to_drift, P2.hub_head_moved_to_drift, P6.hub_full_recompute, P6.adoption_rank, P15.registry_ingest_versions_per_s.
  None of them is 06's.

## Acceptance checklist

- [x] `just verify-06` passes, and P8 is within budget. Proof: literal `just verify-06`, exit 0; `PASS P8 35.594 p95 <= 1500 ms`
  (criterion mean 22.6 ms); `tests/justfile_gate.rs` (5 tests, including that every sub-recipe is in the gate and the test recipe
  refuses to skip without Docker) passes.
- [x] The full action x user matrix passes, and idempotency, 409 and 423 are tested. Proof (all passed in the `verify-06` run):
  `matrix_every_action_by_role_and_status`, `viewer_gets_403_on_every_write_route`, `pending_and_disabled_users_fail_before_any_side_effect`;
  idempotency `same_key_same_request_replays_stored_response_and_writes_once`, `same_key_different_request_is_422_and_writes_nothing`,
  `concurrent_same_key_executes_once`, `idempotency_rows_expire_after_24_hours`, `transient_outcomes_are_not_stored`;
  409 `edit_conflict_is_409_with_current_hash_and_writes_nothing`, `upload_conflict_is_409`, `delete_conflict_is_409`,
  `revert_conflict_is_409`, `approval_on_changed_file_marks_stale_and_writes_nothing`, `duplicate_pr_is_409_with_link`;
  423 `unconfirmed_baseline_returns_locked_423_and_writes_nothing`, `every_file_action_is_locked_until_baseline_confirmed`.
- [x] The vault's associated-data binding and zeroization are tested, and the leak test passes. Proof: `swapped_associated_data_fails_decryption`,
  `ciphertext_moved_to_another_user_fails`, `fresh_data_key_per_token`, `plaintext_token_is_zeroized_after_use`,
  `data_key_is_zeroized_after_wrap`, `data_key_is_zeroized_when_kms_refuses`, `the_data_key_seal_uses_is_a_fresh_secret` (the old
  `data_key_is_a_secret_that_zeroizes` never called `seal` and was removed in review round 1; see below for what the new tests do
  and do not show), `decrypt_happens_only_inside_the_owners_request`, `pr_uses_the_callers_own_token`,
  `token_never_appears_in_logs_errors_audit_or_responses`, `classic_token_is_rejected_with_the_policy_message`.
- [x] Every write produces exactly one chained audit event. Proof: `every_applied_write_has_exactly_one_chained_audit_event` (`tests/edit.rs`,
  per action, chain verified with `ports::verify_chain`), `edit_writes_nfs_then_commits_audit_registration_event_and_response_together`,
  `audit_failure_rolls_back_the_registration_the_event_and_the_response`, `an_audit_failure_rolls_the_whole_save_back`,
  `the_pull_request_has_exactly_one_chained_audit_event_with_its_address`, `saving_a_token_writes_exactly_one_chained_audit_event_without_the_token`,
  `rejecting_records_the_reason_and_one_audit_event_and_writes_nothing`, `every_reveal_is_audited`, `notify_is_audited_and_requires_operator`,
  `approval_event_names_author_and_approver`; and `assert_eq!(rig.chain().await.len(), 0, "no audit event")` in `tests/matrix.rs` for
  every rejected or conflicting write.
- [x] Per-task verify items (T1 to T11) are covered by the named tests of the plan's traceability table. I checked all 119 backticked
  identifiers of that table against the run: 53 spot-checked test names (the ones cited in this file) all exist and pass.
  **Twelve plan-named tests do not exist under those exact names** (`data_key_is_zeroized_after_wrap` now does, since review round 1) (the tests were renamed or split during implementation); their
  behaviour is covered by differently named, passing tests, and `tests/threat_model.rs::every_proof_cell_names_an_existing_test` passes for the
  threat-model section. The reviewer should confirm the mapping:

  | Plan name | Test that carries it |
  |---|---|
  | `audit_event_and_change_commit_together`, `audit_failure_rolls_back_the_whole_commit_transaction` | `edit_writes_nfs_then_commits_audit_registration_event_and_response_together`, `audit_failure_rolls_back_the_registration_the_event_and_the_response` |
  | `rejected_and_conflicting_writes_record_no_audit_event` | the `no audit event` chain-length assertions in `tests/matrix.rs` and `edit_conflict_is_409_with_current_hash_and_writes_nothing`; no single test has this name |
  | `cards_exist_for_every_kind_and_hold_no_file_content` | `a_card_exists_for_every_kind_and_is_a_valid_adaptive_card`, `a_card_holds_the_notification_and_fixed_words_and_nothing_else` |
  | `data_key_is_zeroized_after_wrap` | now a test of that name in `src/vault/crypto.rs` (review round 1): it runs the real `seal_with`, watches the key with a drop probe and a KMS spy. The first mapping, `data_key_is_a_secret_that_zeroizes`, was vacuous (it never called `seal`) and is gone. |
  | `denied_path_is_refused` | `denied_path_is_refused_and_nothing_is_sent`, `writes_to_denied_paths_are_refused` |
  | `write_above_3_mib_is_rejected_too_large` | `write_above_the_limit_is_rejected_too_large_and_the_limit_itself_passes`: **a 2 MiB-limit test, not a 3 MiB one** (see "Assumptions": A3 is not met as the plan proposed) |
  | `edit_registers_expected_hash_before_write_file` | `edit_writes_nfs_then_commits_audit_registration_event_and_response_together` (registration is in tx 2), `edit_conflict_is_409_with_current_hash_and_writes_nothing` |
  | `errors_carry_no_input_text` | `errors_are_fixed_words_that_hold_no_input`, `errors_and_debug_hold_nothing`, `errors_are_copy_and_carry_no_input_text` |
  | `problem_responses_hold_no_internal_detail` | `every_token_error_is_a_fixed_problem`, `reveal_failures_are_fixed_problems_that_quote_nothing`, `outcomes_that_are_not_successes_are_problems_with_the_fields_the_contract_names` |
  | `request_bodies_are_capped_and_unknown_fields_refused` | `bodies_are_limited_before_they_are_read`, `a_body_over_the_cap_is_refused_unread`, `a_form_that_is_not_exactly_the_contracts_is_refused` |
  | `paths_are_typed_before_the_pipeline` | proved by `a_request_that_is_not_well_formed_never_reaches_the_service` in `tests/routes.rs`: a traversing, an absolute and an empty path through edit, delete, upload, notify, drafts, pull request and reveal each get a 4xx and the (scripted) service is never called; plus `an_edit_is_read_into_typed_values_and_the_applied_outcome_is_200` and the `NfsPath` fuzz target `writes_request_json`. (The FINAL step's note called this the weakest mapping; it was wrong.) |

## Security requirements implemented

S4 (guard on every route: `route_table_matches_openapi_for_writes`, `viewer_gets_403_on_every_write_route`, `only_admin_approves`,
`author_cannot_approve_own_proposal`, `restart_and_notify_need_operator`), S8 (token vault: the tests above), S9 (one chained audit event
per write), S10 (`write_path_logs_hold_paths_hashes_and_ids_only`, `teams_webhook_url_never_logged`, `revealed_value_never_logged`,
`token_never_appears_in_logs_errors_audit_or_responses`), S11 (typed paths, body caps, size limit, deny globs;
`write_above_the_limit_is_rejected_too_large_and_the_limit_itself_passes`, `writes_to_denied_paths_are_refused`), S13 (value-based secret
check: `secret_in_content_blocks_and_reports_lines_only`), S21 (workspace clippy `-D warnings` with `forbid(unsafe_code)`, error tests above),
S22 (six fuzz targets, 12 to 48 seeds each, replayed on stable by `tests/corpus_replay.rs`, 9 tests; 30 s on the pinned nightly each,
no crash). The threat-model sections for the write paths and the vault are in `docs/threat-model.md` (T8) and checked by
`tests/threat_model.rs` (10 tests).

## Benchmarks against budgets

| P# | Benchmark | Result (p95, from `bench-check`) | Budget | Verdict |
|---|---|---|---|---|
| P8 | `hub-writes/edit_applied` (whole `edit` call, Postgres, fake agent, 100 edits round-robin over 1 KB to 150 KB files incl. the 6,000-line one) | 35.594 ms in the literal `verify-06`; 40.020 ms and 33.751 ms in two other runs (criterion means 22.6, 20.6 and 20.6 ms) | <= 1500 ms | PASS (about 42x margin on the literal run) |
| P12 | `hub-writes/write_queries` (`P12.hub_writes_queries`: claim, lock read, open-PR lookup, proposal list, expiry scan, credential load at target-scale row counts) | 9.324 ms in the literal `verify-06`; 11.719 ms and 10.889 ms in two other runs (criterion means 1.67, 1.80 ms) | <= 50 ms | PASS |
| P8 multi-replica | `edit_through_a_second_replica_meets_p8` (gateway fake with a hop delay) | passes in `tests/p8.rs` (6 s) | 1500 ms | PASS (loopback only) |
| P12 plans | `explain_*` tests in `tests/explain.rs` (11 tests) assert no sequential scan on the hot queries | 11 passed | n/a | PASS |

Both benchmarks are loopback numbers; they bound hub code and database time only. Also measured, not 06's, in the same full run: P1 9,703 ms
(<= 15,000), P8 above, P11 70.8 ms, P11.memory 735 MiB (<= 1536), P13 7.1 ms, P15 rows pass.

## New dependencies

All sit in `Cargo.toml` / `Cargo.lock` of the workspace (pinned with `=` where they are direct) and were added in this branch. Ages are
from the crates.io `created_at` of the pinned version, against 2026-10-15. `cargo deny check` and `cargo audit` pass on the final lockfile
(`ring` is not in hub-writes' graph, per the manifest comment). `osv-scanner` NOT RUN (not installed).

| Crate | Version | Published | Age (days) | Why | `cargo deny` / `cargo audit` |
|---|---|---|---|---|---|
| octocrab (direct, default features off) | =0.54.2 | 2026-09-14 | 31 | The stack's GitHub client (T5, T6, plan A9); a TLS 1.3-only, https-only service is injected with `with_service` | pass / pass |
| hyper-rustls (direct, `http1`, `aws-lc-rs`) | =0.27.10 | 2026-09-20 | 25 | The connector for that service, over the hub's own rustls `ClientConfig` (already in the lockfile through tonic) | pass / pass |
| jsonwebtoken (transitive of octocrab, `jwt-aws-lc-rs`) | 10.4.0 | 2026-05-11 | 157 | listed as new by the manifest comment; octocrab's JWT crypto, on aws-lc-rs | pass / pass |
| simple_asn1 (transitive) | 0.6.4 | 2026-02-12 | 245 | jsonwebtoken | pass / pass |
| snafu, snafu-derive (transitive) | 0.8.9 | 2025-09-03 | 407 | listed as new by the manifest comment; pulled in by octocrab | pass / pass |
| http-serde (transitive) | 2.1.1 | 2024-05-27 | 507 | listed as new by the manifest comment; pulled in by octocrab | pass / pass |
| cargo_metadata, cargo-platform, camino (transitive) | 0.23.1, 0.3.2, 1.2.6 | 2025-11-12, 2025-12-11, 2026-09-15 | 337, 308, 30 | listed as new by the manifest comment; pulled in by octocrab's dependency set | pass / pass |
| zeroize_derive (transitive) | 1.5.0 | 2026-06-12 | 125 | `zeroize` derive, S8 | pass / pass |

All are at least 14 days old (youngest: hyper-rustls 25 days, camino 30 days). The fuzz crate has its own lockfile
(`crates/hub-writes/fuzz/Cargo.lock`, 476 crates) that passed `cargo deny` and `cargo audit` in `writes-fuzz`. The web app gained no packages.

## Assumptions

- The sandbox Postgres 16.15 (Ubuntu build, pgvector and pg_trgm available, default `fsync`) stands in for the Docker-run Postgres
  with pgvector. The tests migrate as `lanekeeper_migrator` and run as `lanekeeper_app` (`tests/support/pg.rs`), so the grants are the production ones.
- Run-to-run variance of the benchmarks on this 4-core shared VM is large for tiny budgets (3 ms, 60 ms). The P8 and P12 margins for 06
  (about 40x and 5x) are far outside it.
- Plan assumptions A1 to A22 are unchanged by this step; the open ones that need a human (A1, A3, A4, A5, A7, A9, A11, A12, A13, A15, A16,
  A21) are listed in the plan and in the threat model's residual risks.

- **A3 (size limit): the limit is 2 MiB, not the plan's proposed 3 MiB.** `WriteConfig::max_content_bytes` defaults to
  `engine::Limits::DEFAULT.input_bytes`, which is `2 * 1024 * 1024`, and `validate` refuses a larger setting. This matches
  `docs/performance.md` ("files up to 2 MiB"), but it is not what A3 proposed. The plan test `write_above_3_mib_is_rejected_too_large`
  is therefore the 2 MiB-limit test `write_above_the_limit_is_rejected_too_large_and_the_limit_itself_passes`. A human should confirm
  2 MiB (A3 asked to confirm 3 MiB) and the OpenAPI wording.
- **DECIDED by the human on 2026-10-15: allowed, D107. A PR job uses its author's GitHub token at job time (S8, D42, D77).** S8 and D42 say the token is
  decrypted only inside its owner's own request. A pull request is made by a job (`PrJobRunner`, lease-driven, D77) that can run
  minutes after the request, on another replica, or after a restart. Its `ctx_for` (`src/prs/runner.rs`) rebuilds the author's
  `WriteCtx` from the directory, and the vault's `with_token` decrypts the stored token for it. Failing scenario: the author signs
  out, or their session is revoked, while they are still an active Editor; the job still opens the token and uses it to call GitHub.
  What the code re-checks before each use: `with_token` calls `authorize(.., Role::Editor)`, which asks the directory again, so an
  author who was disabled, demoted below Editor or removed makes the job fail with no call to GitHub; and a token past its expiry is
  not opened. It does **not** re-check that a session exists. The design is unchanged: the human decided to allow job-time use for PR
  jobs only (D107, which relaxes S8 and D42 for them; every other use of the token stays inside the owner's own request). D107 is
  recorded on branch `claude/brave-cori-2cnesi` (commit `564948a`) and lands with main; until main is merged into this branch it is
  not in this branch's `docs/decisions.md`. The residual risk stays in the threat model: signing out does not stop a job, demoting or
  disabling the user does.

## Deviations from the plan

**DECIDED by the human on 2026-10-15: allowed, D107: job-time use of the author's token** (S8/D42 against D77), as written under "Assumptions". It is a
deviation from the letter of S8 and D42 that the plan did not list, and that D107 now allows for PR jobs. **A3:** 2 MiB instead of 3 MiB (above).

In the code, otherwise, none. Two process deviations, both from the environment: (1) `cargo test --workspace` was run package by package because of
disk; (2) the debug-info setting differed between the first and the second half of the run (see "How the gates were run"). No test,
budget, gate, hook or contract file was changed in this step.
