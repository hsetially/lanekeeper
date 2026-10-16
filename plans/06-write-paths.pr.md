# 06: Write paths, approvals, token vault, PRs and notifications

Prompt `prompts/06-write-paths.md` · plan: `plans/06-write-paths.md` (approved by a human) · evidence: `plans/06-write-paths.evidence.md` · branch `agent/06-write-paths` · wave: not recorded in the plan

Do not merge this PR yet. Three things still need a human first (the section after "Decided"), and the gates must be re-run at the final head. The fourth item that used to be here, the job-time use of the author's token, was decided by the human on 2026-10-15 (D107, see "Decided" below).

## Summary

Prompt 06 adds `crates/hub-writes`: every way the hub changes a file, a service or a repository, and the GitHub token vault behind pull requests. The crate is a library. It does not wire itself into `crates/hub` (plan A15; see "Contract follow-ups needed"). One commit per task, none amended:

| Task | Commit | What it adds |
|---|---|---|
| T1 | `f28efc9` | The write pipeline: authorize from the directory, idempotency claim, editing lock (423), validation with the secret scan, approval step, execute in two short transactions with no connection held across the agent call. Reconcile job for a write that landed without a commit. The `verify-06` recipe, the P8 benchmark and the new `P12.hub_writes_queries` budget. |
| T2 | `d0503ce`, `3a6cb4a` | Byte-faithful writes: line endings, BOM and encoding of the target file are kept; the style changes only on an explicit `change_eol`. A proptest regression seed is kept. |
| T3 | `8904a6b` | Edit, upload, delete, revert and restart; stateless HMAC effect-preview ids; the REST routes (`mount`); three fuzz targets and a corpus replay on stable. |
| T4 | `5bb9cd8` | Proposals, approvals (an admin who is not the author), 72 h expiry job, drafts. |
| T5 | `1f92515` | The GitHub token vault: fresh AES-256-GCM data key per token wrapped by KMS, associated data = user id + credential id, fine-grained tokens only, login linking, `/me/github-token` routes, credential cleanup job, two more fuzz targets. |
| T6 | `569a410` | Pull requests, synchronous half: reverse path mapping, explicit base or tenant target, warnings, Git Data API calls, 100-file cap. |
| T7 | `0d1283a` | Teams notifier: Adaptive Cards, allowlisted webhook, bounded retries with jitter, circuit breaker; a failing webhook never fails an action. |
| T8 | `737ab6c` | Threat model section "Write paths and token vault (06)" and `tests/threat_model.rs`, which fails if a Proof cell names a test that does not exist. |
| T9 | `042bf45` | PR links and PR jobs: persisted, resumable, one open PR per change, webhook-driven state, SSE progress. |
| T10 | `d7677fd` | Masked values and the audited reveal action; deny-glob enforcement at every write door. |
| T11 | `7ca194e` | Notify services (Operator only, one audited door to the unauthenticated `/update-resources`), auto-notify batching, config-server restart offer when C11 is open, pickup states. |
| FINAL | `93e5d56` | Evidence file. |
| Review round 1 | `2b8ccfc`, `18a7ef2`, `be664fa`, `b9b518c`, `e52e13e` | Fixes and documentation for the reviewer's findings (see "Reviewer agent verdict"). |

Outside `crates/hub-writes` this branch changes: `Cargo.toml`, `Cargo.lock`, `Justfile`, `perf/budgets.toml` (flip `P8` to registered, add `P12.hub_writes_queries`; plan G12), `crates/xtask/tests/budgets_registry.rs` and `docs/performance.md` (approved Extra-paths), and `docs/threat-model.md`. `deny.toml` was not changed. The other contract changes this work needed (domain types, ports, OpenAPI, migrations) landed earlier in `contract/06-writes` (hsetially/lanekeeper#12).

## Decided by the human (2026-10-15): job-time use of the author's token (D107)

**Decision.** A pull-request job may use its author's GitHub token at job time. This is **D107**. It relaxes S8 and D42 for PR jobs only; every other use of the token stays inside the owner's own request. D107 is recorded on branch `claude/brave-cori-2cnesi` (commit `564948a`) and lands with main. Until main is merged into this branch it is not in this branch's `docs/decisions.md`.

**The conflict it settles.** S8 and D42 say the token is "decrypted only inside the owning user's request" and "used only inside that user's own authenticated request". D77 says "PR creation runs as a persisted background job with states". The job (`PrJobRunner`, lease-driven) can run minutes after the request that asked for it, on another replica, or after a restart. Its `ctx_for` (`src/prs/runner.rs`) rebuilds the author's context from the directory, and the vault's `with_token` decrypts the stored token for it. The design and the code are unchanged.

**The controls that stay (all in the code):**

- every step re-reads the user from the directory and requires an active user with the Editor role or above (`with_token` calls `authorize(.., Role::Editor)`); a disabled, demoted or removed author makes the job fail with no call to GitHub;
- a token past its expiry is refused (not opened);
- one lease per job;
- tries are bounded (5);
- a step is limited to 60 s;
- the token is never logged, audited or returned.

The associated data binds a ciphertext to its owner, so the job can never open another user's credential, and an admin's approval makes the job for the proposal's author, so the admin's token is never touched. Proofs: `a_job_whose_author_was_disabled_meanwhile_fails_without_touching_github`, `a_user_who_requires_approval_gets_a_proposal_and_the_approval_makes_the_job`, `an_expired_token_is_not_decrypted`, `pr_uses_the_callers_own_token`.

**What is not checked (the residual risk that stays).** That a session for the author still exists. An author who is demoted or disabled stops the job; an author who only signed out, or whose session was revoked while still an active Editor, does not. The threat model's residual risk "A job uses its author's token in the background" keeps this, and now says it was decided (D107).

## STILL NEEDS A HUMAN BEFORE MERGE

### 1. Human sign-off of `docs/threat-model.md` for this component (S9, S24)

T8 added the section "Write paths and token vault (06)" with its STRIDE table and residual risks. A human signs it off before go-live. The residual risks new in this round, besides the D107 residual risk above, that the signer should read: a command that timed out at the hub can still run at the agent later, with no audit event, because `WriteFile` and `DeleteFile` have no deadline on the wire (the expected hash bounds it); `POST /drafts` ignores its `Idempotency-Key`, so a retry makes a second draft (bounded by 100 open drafts and one-day expiry); a restart or notify whose answer is lost is not audited and a retry repeats it; a landed write that someone overwrites before the reconcile job looks is not audited as ours; the token passes through buffers that cannot be zeroized; the reveal limit is per replica; and notify and the config-server restart stay off the real system until Q37 and Q10 are answered.

### 2. Does the 2 MiB write limit stand?

The plan (A3) proposed 3 MiB. The code enforces **2 MiB**: `WriteConfig::max_content_bytes` defaults to `engine::Limits::DEFAULT.input_bytes` (`2 * 1024 * 1024`), and `validate` refuses a larger setting. That matches `docs/performance.md` ("files up to 2 MiB") but not A3. The OpenAPI text still says uploads up to 25 MiB (see "Contract follow-ups needed"). A human should confirm 2 MiB and the OpenAPI wording.

### 3. Two budgets fail in the full registry, and one test needs Docker

`cargo xtask bench-check` exits 1 because of two budgets that are **not 06's**: `P6.index_settings` (prompt 04) and `P12.audit_record` (prompt 03a). Whether these are noise on a 4-core shared VM or real regressions is a human call; 06 did not touch `crates/engine`, `crates/hub-identity` or `perf/budgets.toml` thresholds for them. Numbers are under "Gates". `just verify` also stops at `hub/tests/pg_tls.rs`, which needs a Docker daemon that the sandbox does not have. 06 does not own it and does not touch `crates/hub`.

### The gates must be re-run at the final head

The last literal `just verify-06` pass was at `7ca194e`. Later commits (review round 1) changed only `crates/hub-writes` source and tests, `docs/threat-model.md` and the evidence file. After them I re-ran only the subset listed under "Gates", not `just verify-06` as one invocation, not `just verify`, not the full `cargo xtask bench-check`, and not the fuzz targets. Run `just verify-06`, `just verify` and `cargo xtask bench-check` at the final head before merge.

## Prompt

`prompts/06-write-paths.md` · plan `plans/06-write-paths.md` (Status: APPROVED; Contract-change: APPROVED; Extra-paths: `crates/xtask/tests/budgets_registry.rs`, `docs/performance.md`, `deny.toml`) · wave: not recorded in the plan.

## Gates (results)

All numbers are copied from `plans/06-write-paths.evidence.md`. Environment: local Postgres 16.15 (Ubuntu package, pgvector and pg_trgm present) via `LK_TEST_PG_ADMIN_URL`, `LK_REQUIRE_DOCKER=1`, `CARGO_INCREMENTAL=0`; disk was tight, so `cargo test --workspace` ran package by package and the second half of the run built with debug info off.

### At `7ca194e` (the FINAL step)

| Command | Result |
|---|---|
| `just verify-06` (literal, one invocation) | **PASS**, exit 0, real 21 m 39 s. `15 test binaries: 544 passed, 0 failed`; six fuzz targets ran 30 s each without a crash (`writes_request_json`, `writes_upload_multipart`, `writes_preview_id`, `writes_github_token`, `writes_github_responses`, `writes_git_data_responses`); `PASS P8 35.594 p95 <= 1500 ms`; `PASS P12.hub_writes_queries 9.324 p95 <= 50 ms`; `bench-check (default): 2 pass, 0 fail, 0 unmet of 2 budgets` |
| `just verify`: `cargo fmt --all -- --check` | PASS |
| `just verify`: `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `just verify`: `cargo test --workspace` | **FAIL, 1 test: `hub/tests/pg_tls.rs::pg_pool_connects_to_a_real_tls_postgres_and_refuses_the_wrong_ca`.** `LK_REQUIRE_DOCKER=1 but the Postgres container cannot start: failed to initialize a docker client: Socket not found: /var/run/docker.sock`. Environment limit; not worked around. Everything else passed: 3,370 passed, 1 failed, 16 ignored by their owners. |
| `just verify`: `cargo deny check` | PASS (`advisories ok, bans ok, licenses ok, sources ok`) |
| `just verify`: `cargo audit` | PASS (1329 advisories, 618 crate dependencies) |
| `just verify`: `just web-verify` | PASS (the web app is still the scaffold, one test) |
| `cargo xtask bench-check` (full registry) | **FAIL, exit 1: `25 pass, 2 fail, 26 unmet of 53 budgets`** |

The two failing rows (not 06's), from two separate `bench-check` runs each:

| Budget | Owner | Measured p95 | Budget |
|---|---|---|---|
| `P6.index_settings` (`engine/index_settings_6000_lines`) | 04 | 71.472 ms, then 65.319 ms (criterion mean 51.3 ms) | 60 ms |
| `P12.audit_record` (`hub-identity/audit_record`) | 03a | 4.124 ms, then 4.569 ms (criterion mean 1.80 ms) | 3 ms |

Each benchmark was rerun once more and did not move into budget. `plans/04-fact-engine.evidence.md` and `plans/03a-hub-identity-audit.evidence.md` already record the same two flapping near their limits. The 26 unmet budgets belong to prompts 05 and 07 to 14 (benchmarks not built yet, `registered = false`); none is 06's.

### After review round 1 (head `b9b518c`, plus the evidence file)

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy -p hub-writes --all-targets -- -D warnings` | PASS |
| `cargo test -p hub-writes`, one binary at a time | PASS: **549 passed, 0 failed** (lib 200, actions 215, routes 42, matrix 6, edit 17, fidelity 14, idempotency 9, logs 3, explain 11, p8 1, reconcile 7, corpus_replay 9, threat_model 10, justfile_gate 5); 544 before the fixes |
| `cargo deny check` | PASS |
| `just writes-bench` | PASS: `P8 33.904 p95 <= 1500 ms`, `P12.hub_writes_queries 9.972 p95 <= 50 ms` |

### NOT RUN

| Item | Why |
|---|---|
| `just verify-06` as one literal invocation **after review round 1** | Not re-run. Last literal pass: `7ca194e`. |
| The 30 s fuzz targets after review round 1 | Not re-run. The parsers under them were not touched; `corpus_replay` (9 tests) ran. |
| `just verify` as one literal invocation (at any head) | Disk: one `cargo test --workspace` failed with `No space left on device` at link time. Each part was run separately instead (table above). A literal run would stop at `pg_tls` anyway. |
| `just verify` and the full-registry `cargo xtask bench-check` after review round 1 | Not re-run. |
| `hub/tests/pg_tls.rs`, `just docker-check`, all testcontainers paths in Docker mode | No Docker daemon in the sandbox. A local Postgres 16.15 stood in (the documented alternative in `writes-tools-check`). |
| **Real GitHub: CRLF through the Git Data API, fine-grained token behaviour, the expiry header** | Not reachable. The prompt's stop condition "GitHub's API can't preserve CRLF" is proved only against the mock (`crlf_survives_the_git_data_api_blob`). **The manual check against the real API is NOT RUN and needs a human.** |
| Real Cloud KMS, real Teams Workflows webhook, real agent on NFS | Not reachable; the `ports` fakes, a local webhook server and a fake agent were used. |
| Multi-replica P8 on a real network | The benchmark is loopback; `edit_through_a_second_replica_meets_p8` models the extra hop with a delay. Prompt 16 measures staging. |
| `osv-scanner` | Not installed here. AGENTS rule 15 names it; neither `just verify` nor `verify-06` calls it. `cargo deny` and `cargo audit` ran instead. |
| `just fuzz-smoke` (root `fuzz/` targets of 01 to 04) | Not part of `verify-06` or `verify`; other prompts'. 06's own six targets ran. |
| `cargo sqlx prepare --check` | `hub-writes` uses runtime-checked queries (no `.sqlx` metadata of this crate to check). |

## Acceptance checklist

- [ ] `just verify-06` passes, and P8 is within budget. Proved at `7ca194e` (literal run, exit 0, `PASS P8 35.594 p95 <= 1500 ms`; `tests/justfile_gate.rs`, 5 tests). **Left unticked until the recipe is re-run at the final head.** P8 was re-measured after the fixes: `33.904 p95`.
- [x] The full action x user matrix passes, and idempotency, 409 and 423 are tested. Proof: `matrix_every_action_by_role_and_status`, `viewer_gets_403_on_every_write_route`, `pending_and_disabled_users_fail_before_any_side_effect`; idempotency `same_key_same_request_replays_stored_response_and_writes_once`, `same_key_different_request_is_422_and_writes_nothing`, `concurrent_same_key_executes_once`, `idempotency_rows_expire_after_24_hours`, `transient_outcomes_are_not_stored`; 409 `edit_conflict_is_409_with_current_hash_and_writes_nothing`, `upload_conflict_is_409`, `delete_conflict_is_409`, `revert_conflict_is_409`, `approval_on_changed_file_marks_stale_and_writes_nothing`, `duplicate_pr_is_409_with_link`; 423 `unconfirmed_baseline_returns_locked_423_and_writes_nothing`, `every_file_action_is_locked_until_baseline_confirmed`. (Run at `7ca194e`; `cargo test -p hub-writes` passes with 549 at `b9b518c`.)
- [x] The vault's associated-data binding and zeroization are tested, and the leak test passes. Proof: `swapped_associated_data_fails_decryption`, `ciphertext_moved_to_another_user_fails`, `fresh_data_key_per_token`, `plaintext_token_is_zeroized_after_use`, `data_key_is_zeroized_after_wrap`, `data_key_is_zeroized_when_kms_refuses`, `the_data_key_seal_uses_is_a_fresh_secret`, `decrypt_happens_only_inside_the_owners_request`, `pr_uses_the_callers_own_token`, `token_never_appears_in_logs_errors_audit_or_responses`, `classic_token_is_rejected_with_the_policy_message`. Limit of the evidence: whether the data key's memory is zero after the drop cannot be observed in safe Rust (`forbid(unsafe_code)`). The tests show the key is a `Secret`, is the key the token was encrypted under, is not in the returned `Sealed`, and is dropped by the time `seal` returns, on success and on a KMS refusal. A mutation check (`std::mem::forget(dek)` in `seal_with`) fails `data_key_is_zeroized_after_wrap`. Wiping on drop is a property of `domain::Secret` (`secrecy::SecretBox`), not shown here. This item covers the vault as built; the job-time use of the token is decided (D107, see "Decided by the human" above).
- [x] Every write produces exactly one chained audit event. Proof: `every_applied_write_has_exactly_one_chained_audit_event` (per action, chain verified with `ports::verify_chain`), `edit_writes_nfs_then_commits_audit_registration_event_and_response_together`, `audit_failure_rolls_back_the_registration_the_event_and_the_response`, `an_audit_failure_rolls_the_whole_save_back`, `the_pull_request_has_exactly_one_chained_audit_event_with_its_address`, `saving_a_token_writes_exactly_one_chained_audit_event_without_the_token`, `rejecting_records_the_reason_and_one_audit_event_and_writes_nothing`, `every_reveal_is_audited`, `notify_is_audited_and_requires_operator`, `approval_event_names_author_and_approver`; and the `no audit event` chain-length assertions in `tests/matrix.rs` for every rejected or conflicting write. Known gaps, all in the threat model: a restart or notify whose answer is lost, a landed write that is overwritten before the reconcile job looks, and a command that runs at the agent after its deadline.
- [x] Per-task verify items (T1 to T11) are covered by named tests. All 119 backticked identifiers of the plan's traceability table were checked against the run. **Twelve plan-named tests do not exist under those exact names** (renamed or split); a differently named, passing test carries each (mapping in the evidence file, "Acceptance checklist"), and `tests/threat_model.rs::every_proof_cell_names_an_existing_test` passes for the threat-model section. The reviewer is asked to confirm the mapping. One of the twelve changes meaning: `write_above_3_mib_is_rejected_too_large` is a 2 MiB-limit test (open item 2).
- [ ] `just verify` passes. **No.** Fails only at `hub/tests/pg_tls.rs` (needs Docker); everything else passed at `7ca194e`. Not re-run after review round 1.
- [ ] `cargo xtask bench-check` passes. **No.** `25 pass, 2 fail, 26 unmet of 53` at `7ca194e`. 06's two budgets pass; the two failures are 04's and 03a's (open item 3).
- [ ] Manual check that the real GitHub Git Data API preserves CRLF. **NOT RUN.** Needs a human with a real token.
- [ ] Threat model signed off by a human (S9, S24). **Pending** (open item 1).

## Security requirements implemented (S#)

- **S4** guard on every route and the service re-reads the role: `route_table_matches_openapi_for_writes`, `viewer_gets_403_on_every_write_route`, `only_admin_approves`, `author_cannot_approve_own_proposal`, `restart_and_notify_need_operator`.
- **S8** GitHub token vault: the tests under the acceptance checklist. The job-time use of the token is decided by the human (2026-10-15): allowed, D107, which relaxes S8 and D42 for PR jobs only (see "Decided by the human" above).
- **S9** one chained audit event per write, in the transaction that registers it.
- **S10** no contents or secrets in logs: `write_path_logs_hold_paths_hashes_and_ids_only`, `teams_webhook_url_never_logged`, `revealed_value_never_logged`, `token_never_appears_in_logs_errors_audit_or_responses`.
- **S11** typed paths, body caps, size limit, deny globs: `write_above_the_limit_is_rejected_too_large_and_the_limit_itself_passes`, `writes_to_denied_paths_are_refused`.
- **S13** value-based secret check: `secret_in_content_blocks_and_reports_lines_only`.
- **S21** workspace clippy `-D warnings` with `forbid(unsafe_code)`; error tests that hold no input text.
- **S22** six fuzz targets, 12 to 48 seeds each, replayed on stable by `tests/corpus_replay.rs` (9 tests); 30 s each on the pinned nightly at `7ca194e`, no crash.
- Also referenced in the threat model's S# column: **S14b** (masked values and the reveal action), **S17** (denied paths, cap-std second wall), **S18** (allowlisted hosts for GitHub and Teams).

The threat-model section and its 10 checking tests are in `docs/threat-model.md` and `tests/threat_model.rs`.

## Benchmarks against budgets (P#)

| P# | Benchmark | p95 | Budget | Verdict |
|---|---|---|---|---|
| P8 | `hub-writes/edit_applied` (whole `edit` call: Postgres, fake agent, 100 edits round-robin over 1 KB to 150 KB files including the 6,000-line one) | 35.594 ms in the literal `verify-06` at `7ca194e` (criterion mean 22.6 ms); 33.904 ms after review round 1; 40.020 and 33.751 ms in two other runs | <= 1500 ms | PASS |
| P12.hub_writes_queries | `hub-writes/write_queries` (claim, lock read, open-PR lookup, proposal list, expiry scan, credential load at target-scale row counts) | 9.324 ms in the literal `verify-06` (criterion mean 1.67 ms); 9.972 ms after review round 1; 11.719 and 10.889 ms in two other runs | <= 50 ms | PASS |
| P8, multi-replica | `edit_through_a_second_replica_meets_p8` (gateway fake with a hop delay) | passes in `tests/p8.rs` | 1500 ms | PASS, loopback only |
| P12 plans | 11 `explain_*` tests assert no sequential scan on the hot queries | 11 passed | n/a | PASS |

Both benchmarks are loopback numbers: they bound hub code and database time only. Prompt 16 measures staging. Full-registry results (and the two budgets that fail, not 06's) are under "Gates".

## New dependencies and why

Pinned versions are at least 14 days old (ages from the crates.io `created_at` of the pinned version, against 2026-10-15). `cargo deny check` and `cargo audit` pass on the final lockfile; `osv-scanner` NOT RUN (not installed). `ring` is not in `hub-writes`' graph.

| Crate | Version | Published | Age (days) | Why |
|---|---|---|---|---|
| octocrab (direct, default features off) | =0.54.2 | 2026-09-14 | 31 | The stack's GitHub client (T5, T6; plan A9). A TLS 1.3-only, https-only service is injected with `with_service`. |
| hyper-rustls (direct, `http1`, `aws-lc-rs`) | =0.27.10 | 2026-09-20 | 25 | The connector for that service, over the hub's own rustls `ClientConfig` (already in the lockfile through tonic). |
| jsonwebtoken (transitive of octocrab, `jwt-aws-lc-rs`) | 10.4.0 | 2026-05-11 | 157 | octocrab's JWT crypto, on aws-lc-rs. |
| simple_asn1 (transitive) | 0.6.4 | 2026-02-12 | 245 | jsonwebtoken. |
| snafu, snafu-derive (transitive) | 0.8.9 | 2025-09-03 | 407 | Pulled in by octocrab. |
| http-serde (transitive) | 2.1.1 | 2024-05-27 | 507 | Pulled in by octocrab. |
| cargo_metadata, cargo-platform, camino (transitive) | 0.23.1, 0.3.2, 1.2.6 | 2025-11-12, 2025-12-11, 2026-09-15 | 337, 308, 30 | Pulled in by octocrab's dependency set. |
| zeroize_derive (transitive) | 1.5.0 | 2026-06-12 | 125 | `zeroize` derive (S8). |

Youngest: hyper-rustls at 25 days and camino at 30 days. The fuzz crate has its own lockfile (`crates/hub-writes/fuzz/Cargo.lock`, 476 crates), which passed `cargo deny` and `cargo audit` in `writes-fuzz` at `7ca194e`. The web app gained no packages. `deny.toml` needed no change.

## Assumptions (and deviations from the plan)

**Deviations from the plan**

- **A3, size limit: 2 MiB, not 3 MiB.** See open item 2. The plan test `write_above_3_mib_is_rejected_too_large` is now `write_above_the_limit_is_rejected_too_large_and_the_limit_itself_passes`, a 2 MiB-limit test.
- **S8/D42 against D77: job-time use of the author's token.** Not listed in the plan. Decided by the human on 2026-10-15: allowed, D107 (see "Decided by the human" above).
- **Twelve plan-named tests were renamed or split** (mapping in the evidence file). `data_key_is_zeroized_after_wrap` now exists under its plan name (review round 1). The weakest mapping the FINAL step flagged, `paths_are_typed_before_the_pipeline`, is proved by `a_request_that_is_not_well_formed_never_reaches_the_service` and the `writes_request_json` fuzz target.
- Two process deviations, both from the environment: `cargo test --workspace` ran package by package because of disk, and the debug-info setting differed between the first and second half of the run. No test, budget, gate, hook or contract file was changed to get a pass.

**Assumptions**

- The sandbox Postgres 16.15 stands in for the Docker-run Postgres with pgvector. Tests migrate as `lanekeeper_migrator` and run as `lanekeeper_app`, so the grants are the production ones.
- Benchmark variance on this 4-core shared VM is large for tiny budgets (3 ms, 60 ms). 06's margins (about 40x on P8, about 5x on P12.hub_writes_queries) are far outside it.
- Plan assumptions A1 to A22 are as written in the plan. The ones the plan marks as needing a human, and still open: A1, A3, A4, A5, A7, A9, A11, A12, A13, A15, A16, A21. They are listed in the plan and, where they matter, in the threat model's residual risks. A21 matters most for go-live: Q37 (config-server NetworkPolicy and approval to call `/update-resources`) and Q10 (ArgoCD and the restart annotation) have no default, so notify and restart stay off the real system until a human answers them.
- Review round 1 items left as they are: NB2 (lost restart and notify answers are unaudited), NB4 (a KMS call inside a token-save transaction), NB5 (`POST /drafts` ignores its `Idempotency-Key`; pinned by `a_retried_draft_with_the_same_key_makes_a_second_draft_within_the_open_limit`), NB7 to NB9 and NB12. NB5 and NB10 are residual risks in the threat model. The rest are documented there or judged out of proportion for the round.

## Reviewer agent verdict

The reviewer ran `prompts/REVIEW.md` on this branch twice. I do not have the reviewer's VERDICT block as text in the repository; what follows is the outcome as reported to me, and the evidence file's account of the fixes.

**Round 1, head `93e5d56`: CHANGES REQUESTED, 2 blocking items.**

| Blocking item | Status |
|---|---|
| 2: `data_key_is_a_secret_that_zeroizes` was vacuous (it never called `seal`) | **Fixed** in `2b8ccfc`. `seal` now takes its key from `new_data_key()` through `seal_with`, which owns the key and drops it before it returns. New tests `data_key_is_zeroized_after_wrap`, `data_key_is_zeroized_when_kms_refuses`, `the_data_key_seal_uses_is_a_fresh_secret`; the old test is removed. A mutation check fails the new test as it should. |
| 1: job-time use of the token against S8/D42 | **Resolved by the human decision D107 (2026-10-15): job-time use is allowed.** The design is unchanged; no code fix was needed. See "Decided by the human" above. |

Non-blocking findings fixed: NB1 `set_intent` ignores `rows_affected` (`18a7ef2`, test `a_request_whose_claim_was_cleared_before_the_send_does_not_send`); NB3 expiry takes no `proposal:{id}` lease (`be664fa`, test `the_expiry_job_skips_a_proposal_whose_approval_is_at_the_agent`); NB6 and NB11 evidence wording. NB10 (a timed-out command can still run at the agent) is added to the threat model in `b9b518c`; a fix is a proto change.

**Round 2: APPROVE for the fix commits** (the five `06/review:` commits `2b8ccfc`, `18a7ef2`, `be664fa`, `b9b518c`, `e52e13e`, reviewed as `93e5d56..e52e13e`; the later `06/PR: text` commit was not part of that review). The reviewer's recommendation was not to merge before a decision on the S8/D42 versus D77 question; the human has now decided it (D107, 2026-10-15), so that blocker is resolved. **Merge is still held by the other open items:** the threat-model sign-off (S9, S24), the 2 MiB limit, the two non-06 bench failures, and the gates re-run at the final head. Approval of the fixes is not approval to merge before those are done. No re-review after D107 is recorded; the decision changed no code.

## Contract follow-ups needed

None of these is changed here; contract paths change only through a `contract-change` PR approved by a human. Each is also in the threat model's residual risks or the evidence file.

**OpenAPI (`api/openapi.yaml`)**
- `Proposal` and `ProposalView` need an `action`, a nullable `new_hash`, and all paths. Today an approver sees only the first path of a multi-path proposal (a notify of ten paths shows one path, a zero content hash and no hunks).
- `GithubCredential` needs `expires_at`.
- `WriteOutcome` needs a variant for an accepted job, or a failure reason. Today `raise_pr` through `WriteService` waits up to 30 s and answers `Applied`, `Rejected` (every failure is `NotAllowed`) or `Unavailable`.
- `createPr` needs its `409 duplicate_pr` and its `200 proposal_created` responses documented (a user who requires approval gets the proposal outcome).
- The `200` of `POST /swimlanes/{id}/notify` needs a `oneOf` so it can carry `proposal_created` (as `WriteResult` has); a generated client that validates against `NotifyResult` alone would reject it.
- The upload text "at most 25 MiB" disagrees with the enforced 2 MiB (open item 2).
- Reveal does not replay a stored response for a repeated idempotency key, which differs from the contract wording. It cannot: the stored answer would be the secret. A retry after a lost response makes a second reveal and a second audit event. The contract wording should say so.

**Proto (`proto/agent.proto`)**
- `WriteFile` and `DeleteFile` have no deadline. A command that timed out at the hub can run at the agent later, after the reconcile job cleared the claim, and so with no audit event. The expected hash bounds it. A deadline in the command is the fix.

**`crates/hub` (not in 06's paths; plan A15)**
- Mount the routes (`hub_writes::routes::mount` with `hub_platform::GuardedRouter`) and run the route-table check against the OpenAPI.
- Wire the `auto_notifier`.
- Run the PR job runner under a lease.
- Run the PR state watcher (the `PrStateChanged` consumer).
- Wire the vault and its cleanup job.

## Labels

`contract-change`: the plan's Contract-change was approved by a human, and this branch changes the contract path `perf/budgets.toml` (flip `P8`, add `P12.hub_writes_queries`). The other contract changes landed in hsetially/lanekeeper#12 (`contract/06-writes`), which merged before T1.

Generated with Claude Code (https://claude.ai/code/session_01VZSY4Po3dCZbLmuvw3murA)
