# Evidence 03a: Hub identity, sessions, authorization and audit

Written in the FINAL step. Real output only; nothing is ticked that was not proved, and every claim says where it comes from. Branch `agent/03a-hub-identity-audit`, HEAD before this file `c44cbdb` (`03a/FINAL: reliable log capture and bench teardown`), pushed. The branch contains `origin/main` as of `2502c7b` (the merge of contract PR #6); `git merge-base HEAD origin/main` is `2502c7b`, so the diff against `main` is exactly this prompt's work.

## Where each result comes from

The two environments are not the same, and the gates need Docker and a pinned nightly that the implementer sandbox does not have. Every result below is tagged.

- **[H]** Run by the human on a machine with Docker and the pinned nightly (WSL Ubuntu), on this branch at `c44cbdb`. The implementer did not see the raw output; the numbers and lines are quoted from the human's report.
- **[S]** Run in the implementer sandbox: no Docker daemon, no nightly toolchain, rustc 1.88.0. Postgres-backed tests ran against a locally started Postgres where the sandbox could (the human's report says "full hub-identity suite 394 tests in 23 binaries, 0 failed"); the container path, the fuzz runs and `docker`-dependent code did not run there.
- **[F]** Run for this FINAL step (cheap local checks, grep, `git diff`, crates.io lookups), on 2026-10-11.

## Gates

| Command | Where | Result | Output (tail, as reported) |
|---|---|---|---|
| `just verify-03a` | [H] | PASS, exit 0, 413 s | `identity-tools-check`, `identity-lint` (fmt and clippy `-D warnings`), `identity-test` (`LK_REQUIRE_DOCKER=1`; lib 108 passed; 24 test binaries, all 0 failed; includes `audit_chain` 15, `audit_verify` 18, and the new `log_capture_callsite` and `pg_teardown`), `identity-fuzz` (`identity_jwt`, `identity_cookie`, `identity_audit_event`, 30 s each, no crash), `identity-bench`. Zero "could not remove the test container" lines; no containers left behind. Scoped bench-check inside `identity-bench`: `PASS P7.auth_middleware 0.004 ms p95 <= 2`, `PASS P7.auth_middleware_cold 0.550 ms p95 <= 2`, `PASS P12.audit_record 2.605 ms p95 <= 3`, `3 pass, 0 fail, 0 unmet of 3 budgets`. |
| `cargo xtask bench-check` (after `verify-03a`) | [H] | PASS, exit 0 | `10 pass, 0 fail, 27 unmet of 37 budgets`. The same three 03a rows PASS (0.004, 0.550, 2.605 ms). The 27 unmet rows are other prompts' budgets (`registered = false`, no benchmark yet). |
| `just verify` (workspace) | [H] | **FAIL, FAIL, then PASS** (three runs) | Run 1: exit 101 after 57 s, `crates/agent/tests/kube_watch.rs:310`, test `no_full_report_while_the_lists_are_refused_and_the_log_has_only_the_status`, message `the status is logged: []`. Run 2: exit 101 after 8 s, `crates/agent/tests/binary.rs:72`, test `binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm` (`status.code()` is `None`, killed by SIGTERM). Run 3: exit 0 after 260 s, reached `web-verify`. Both failures are in prompt 02's `crates/agent`, which this branch does not touch (`git diff --name-only origin/main...HEAD` lists no path under `crates/agent`). Analysis and proposed patches: "Known gaps" item 1. |
| `cargo fmt --all --check` | [F] | PASS, exit 0 | no output |
| `cargo test -p xtask` | [F] | PASS, exit 0 | 95 passed, 0 failed, 1 ignored across 7 binaries (lib 3, `bench_check` 17, `budgets_registry` 7, `db_migrations` 25, `fixtures` 22 + 1 ignored, `mcp_tools_doc` 21, bins 0). `budgets_registry` covers the three new budgets (see below). |
| Full hub-identity suite, default threads and `--test-threads=1` | [S] | PASS | 394 tests in 23 binaries, 0 failed, both ways. |

`just verify` did not pass on the first two runs, and nothing was changed in between. That is a flaky pair of tests in another crate, not a pass; the PR text says so.

### What `verify-03a` runs (from the `Justfile` in this branch)

`verify-03a: identity-tools-check identity-lint identity-test identity-fuzz identity-bench`. `identity-fuzz` also runs `cargo deny --manifest-path crates/hub-identity/fuzz/Cargo.toml check` and `cargo audit --file crates/hub-identity/fuzz/Cargo.lock` first, under `set -euo pipefail`, so a finding there would have failed the [H] run (exit 0). `identity-bench` deletes `target/criterion/hub-identity` first, runs `cargo bench -p hub-identity`, cuts a registry with only the three 03a budgets and judges that with `cargo xtask bench-check --budgets`; `tests/justfile_gate.rs` keeps that list and the registered budgets in step.

## Acceptance checklist

Test names were checked in this step: every backticked identifier in the plan's traceability section was searched as a `fn` in `crates/hub-identity/**` and `crates/xtask/tests/budgets_registry.rs` (script, [F]). Of 158 identifiers, 147 are functions with a test attribute or are fuzz invariants; 10 are not test names (`Clock`, `Origin`, `PgTestBus`, `SecretSource`, `expect_used`, `unwrap_used`, `fn`, `subtle`, `tid`, `tracing`); **one test name does not exist: `no_unwrap_on_input_derived_data`** (see S21). Several plan entries are written `..._expired`; the real names are listed below.

"Passed" below means: the test is in a binary that the [H] `identity-test` run reported with 0 failed (the raw per-test lines were not supplied), and in the [S] run of the same suite.

- [x] **`just verify-03a` passes, with the auth middleware under 2 ms and audit `record` under 3 ms in `bench-check`.** Proof [H]: exit 0 in 413 s; `PASS P7.auth_middleware 0.004`, `PASS P7.auth_middleware_cold 0.550`, `PASS P12.audit_record 2.605` (p95 ms); `cargo xtask bench-check` exit 0 with the same three PASS rows. Caveat on P12: the number varies from run to run, see "Benchmarks" and "Known gaps" item 3. The benchmarks run on loopback.
- [x] **The route-table guard test exists. It is exported for the hub binary to run against its full route table.** Proof: `tests/route_table.rs`: `route_table_matches_openapi_and_has_no_unguarded_route`, `registered_role_equals_x_required_role_for_every_identity_operation`, `exported_assert_hub_route_table_is_callable_with_foreign_routes`, `an_unguarded_route_is_reported`, `default_deny_every_registered_path_without_cookie_is_401`, `unmatched_path_without_session_is_401_not_404`, `fallback_route_is_guarded`, `public_route_needs_a_reason`. Export: `hub_identity::testing::{registered_routes, assert_hub_route_table, assert_default_deny}` in `src/testing.rs`, behind feature `route-table` (`src/lib.rs:77`). **Not proved here:** that the hub binary calls them. The binary is prompt 09's; until it does, the tests run only against this crate's own routes and a foreign route list.
- [x] **Chain verification catches tampering, and the checkpoint signatures verify.** Proof (`tests/audit_verify.rs`, 18 tests; `tests/audit_checkpoint.rs`; `tests/audit_chain_prop.rs`): `tampered_row_is_detected` (trigger disabled in the test), `deleted_middle_row_is_detected`, `truncated_tail_is_detected_by_checkpoint`, `forged_checkpoint_row_is_detected`, `bucket_object_mismatch_is_detected`, `verify_report_lists_first_bad_seq_only`, `nightly_verify_passes_on_clean_chain`, `nightly_verify_mismatch_alerts_metric_and_notifier`, `nightly_verify_streams_in_bounded_batches`, `checkpoint_signature_verifies_with_public_key`, `checkpoint_signature_by_other_key_fails`, proptest `chain_verifier_property_any_single_byte_change_is_detected`. The signature check uses a test `KmsSigner` backed by a real P-256 key (`tests/support/ecdsa_kms.rs`). **Not proved:** a real Cloud KMS key and a locked GCS bucket (Known gaps item 2).
- [x] **Every rule in S1 to S4 has a named test.** Proof: the S# table below; each name exists as a test (grep, [F]) and passed ([H], [S]).

Task-level Verify lines from the prompt:

- [x] **T1** mock OIDC provider tests: wrong `tid` (`callback_rejects_wrong_tid`), replayed nonce (`callback_rejects_replayed_nonce`), expired token (`callback_rejects_expired_id_token`), unknown key id (`callback_rejects_unknown_kid_after_one_rate_limited_refresh`), both client-auth modes (`both_client_auth_modes_complete_a_login`, `client_secret_mode_posts_secret_from_secret_source`, `federated_mode_posts_assertion_and_no_secret`). Only against the mock; see Q25.
- [x] **T2** integration tests against Postgres for each rule: `tests/users.rs` (32 plan-named tests; real Postgres as `lanekeeper_app`, testcontainers).
- [x] **T3** fixation (`sign_in_issues_new_session_and_deletes_the_presented_one`, `session_id_supplied_by_attacker_is_never_adopted`), expiry (`session_expires_after_8h_idle`, `session_expires_after_12h_absolute`), cross-origin POST (`cross_origin_post_is_403`), invalidation across replicas (`disabled_user_cut_off_on_other_replica_within_1s`: two app instances, one database), middleware benchmark (`hub-identity/auth_middleware`).
- [x] **T4** route-table test (above) and token tests with forged, expired and wrong-audience tokens (`tests/tokens.rs`).
- [x] **T5** concurrent writers across two instances give one linear chain (`concurrent_writers_across_two_instances_give_one_linear_chain`), tampered row detected, checkpoint signature verifies, `record` adds under 3 ms (benchmark, [H] 2.605 ms in one run).
- [x] **T6** `tests/threat_model.rs`: section exists, six STRIDE letters, cites S1, S2, S3, S4, S7, S9, S10, S21, S22, and every proof it names exists (`every_proof_the_stride_table_names_exists`).

## Security requirements implemented

S1, S2, S3, S4, S7, S9, S10, S21 and S22 are this prompt's (`prompts/03a-hub-identity-audit.md`, plan header). S5, S6, S8, S11, S14, S14b, S19 and S24 shaped the work and are noted where used. Code is under `crates/hub-identity/src/`, tests under `crates/hub-identity/tests/` unless a path says `src/`.

- **S1** (identity, sign-in, tokens). Code: `oidc/{login,jwt,jwks,discovery,client_auth}.rs`, `http.rs`, `users/mod.rs`, `tokens/{entra,google,mod}.rs`, `guard/problem.rs`. Tests, `oidc_login.rs`: `login_redirect_has_state_nonce_and_pkce_s256`, `callback_rejects_wrong_issuer`, `callback_rejects_wrong_audience`, `callback_rejects_wrong_tid`, `callback_rejects_replayed_nonce`, `callback_rejects_state_mismatch`, `callback_rejects_expired_id_token`, `callback_rejects_not_yet_valid_id_token`, `callback_rejects_bad_signature`, `callback_rejects_alg_none_and_hs256_confusion`, `callback_rejects_unknown_kid_after_one_rate_limited_refresh`, `oversize_id_token_is_rejected_unparsed`, `callback_without_login_cookie_is_400`, `login_cookie_expires_after_10_minutes`; `src/oidc/login.rs`: `return_to_rejects_absolute_and_protocol_relative_urls`, `login_cookie_is_sealed_and_tamper_evident`, `login_cookie_of_another_key_is_rejected`; `oidc_jwks.rs`: `jwks_refresh_is_rate_limited_and_single_flight`, `jwks_cache_honours_clamped_max_age`; `src/oidc/discovery.rs`: `discovery_issuer_or_host_mismatch_is_refused`; `src/http/tests.rs`: `real_https_client_refuses_tls12_and_slow_and_oversize_bodies`, `host_outside_allowlist_is_refused`. Identity is `tid` plus `oid` (`users.rs`: `user_is_keyed_by_tid_and_oid_not_email`, `changed_email_does_not_change_identity`). Roles in the app, first sign-in pending with request, event and notification (`first_sign_in_creates_pending_user_request_event_and_notification`, `second_sign_in_creates_no_second_request`, `bootstrap_admin_is_active_admin_at_first_sign_in_only`, `bootstrap_list_does_not_promote_existing_users`, `notifier_failure_does_not_fail_sign_in`). Pending and disabled users get 403 everywhere, including MCP (`pending_and_disabled_users_fail_require_active_for_every_role`, `guard.rs`: `pending_user_gets_403_on_every_guarded_route`, `disabled_user_gets_403_on_every_guarded_route`, `problem_types_distinguish_pending_disabled_and_insufficient_role`; `tokens.rs`: `mcp_token_of_pending_or_disabled_user_is_refused`). MCP and agent tokens, `tokens.rs`: `entra_access_token_valid`, `entra_access_token_rejects_forged_signature`, `entra_access_token_rejects_expired`, `entra_access_token_rejects_not_yet_valid_and_missing_nbf`, `entra_access_token_rejects_wrong_audience`, `entra_access_token_rejects_wrong_tid`, `entra_access_token_rejects_missing_mcp_scope`, `entra_access_token_rejects_wrong_issuer`, `entra_access_token_rejects_oversize_token`, `entra_access_token_rejects_bad_algorithms_and_missing_identity`, `google_id_token_valid_returns_email`, `google_id_token_rejects_forged`, `google_id_token_rejects_expired`, `google_id_token_rejects_wrong_audience`, `google_id_token_rejects_wrong_issuer`, `google_id_token_rejects_unverified_email`, `tokens_never_appear_in_error_text`. Fuzz: `identity_jwt`.
- **S2** (server-side sessions). Code: `session/{cookie,store,cache,touch,revocation,mod}.rs`, `middleware.rs`, `routes/auth.rs`. Tests: `src/session/cookie.rs`: `session_cookie_attributes_exact`, `csrf_cookie_is_host_prefixed_secure_and_readable_by_script`, `cookie_parser_rejects_duplicates_and_oversize`; `sessions.rs`: `only_the_hash_of_the_session_id_is_stored`, `session_expires_after_8h_idle`, `session_expires_after_12h_absolute`, `activity_extends_idle_but_not_absolute`, `touch_is_batched_and_bounded`, `expired_sessions_are_swept_in_batches`, `idempotent_logout`, `disabled_user_cut_off_on_other_replica_within_1s`; `auth_routes.rs`: `sign_in_issues_new_session_and_deletes_the_presented_one`, `session_id_supplied_by_attacker_is_never_adopted`; `users.rs`: `role_change_deletes_all_sessions_in_same_transaction`, `status_change_deletes_all_sessions`, `rolled_back_change_keeps_sessions`; `src/session/revocation.rs`: `resync_flushes_cache`, `event_stream_end_disables_cache`; `src/session/cache.rs`: `cache_is_bounded`, `cache_miss_is_single_flight`. Fuzz: `identity_cookie`.
- **S3** (CSRF). Code: `csrf.rs`. Tests, `auth_layers.rs`: `post_without_csrf_header_is_403`, `post_with_header_not_matching_cookie_is_403`, `post_with_cookie_and_header_equal_but_not_session_bound_is_403`, `cross_origin_post_is_403`, `missing_origin_on_post_is_403`, `null_origin_is_403`, `put_patch_delete_are_checked_too`, `get_is_not_checked_but_never_changes_state`; `auth_routes.rs`: `logout_with_session_requires_csrf`; `src/csrf.rs`: `csrf_comparison_is_constant_time_by_construction`, `csrf_exemption_needs_a_reason`.
- **S4** (default-deny authorization). Code: `guard/{mod,router,problem}.rs`, `routes/{me,admin,mod}.rs`, `users/rules.rs`, `users/admin.rs`. Tests: the eight `route_table.rs` tests above; `guard.rs`: `role_matrix_viewer_editor_operator_admin`, `extractor_rechecks_role`; `users.rs`: `last_active_admin_cannot_be_demoted`, `last_active_admin_cannot_be_disabled`, `admin_cannot_change_own_role`, `admin_cannot_change_own_status`, `concurrent_demotion_of_two_admins_leaves_one`, `every_admin_operation_writes_exactly_one_audit_event_in_its_transaction`.
- **S7** (secrets). Code: `oidc/client_auth.rs`, `config.rs`, `oidc/login.rs` (sealing key from `SecretSource`). Tests: `oidc_login.rs`: `client_secret_mode_posts_secret_from_secret_source`, `federated_mode_posts_assertion_and_no_secret`, `federated_assertion_is_reread_on_each_exchange`; `hygiene.rs`: `secrets_are_redacted_in_debug_and_error_output`, `no_kubernetes_secret_read_in_crate`.
- **S9** (tamper-evident audit). Code: `audit/{log,checkpoint,bucket,signature,verify,shipper,metadata}.rs`, `metrics.rs`. Tests: `audit_chain.rs` (15 tests, incl. `chain_is_linear_for_sequential_records`, `concurrent_writers_across_two_instances_give_one_linear_chain`, `rolled_back_record_leaves_no_row_and_next_record_chains_correctly`, `app_role_cannot_update_or_delete_audit_rows`, `fork_is_refused_by_unique_prev_hash`, `record_hash_matches_ports_chain_hash_golden`, `canonical_json_golden`, `oversize_diff_is_rejected_before_taking_the_lock`); `audit_checkpoint.rs`: `checkpoint_signs_head_writes_bucket_and_row`, `checkpoint_is_skipped_when_head_unchanged`, `checkpoint_requires_the_lease`, `checkpoint_restart_after_bucket_write_is_idempotent`; `audit_bucket.rs`: `bucket_put_is_create_only`; `audit_verify.rs` (the verify and tamper tests listed under the checklist); `audit_shipper.rs`: `shipper_logs_only_committed_events`, `shipper_line_has_event_hash_and_no_diff_text`, `shipper_resumes_from_last_checkpoint_and_dedupes_by_hash`. Fuzz: `identity_audit_event`.
- **S10** (logs: paths, hashes, ids; no contents or secrets). Code: tracing calls across the crate; the shipper line omits the diff. Tests: `log_capture.rs`: `log_capture_has_no_token_cookie_secret_or_diff`; `hygiene.rs`: `error_display_never_contains_input`; `audit_shipper.rs` (above); the capture helper itself: `log_capture_callsite.rs`: `a_callsite_first_reached_on_a_thread_without_capture_is_still_captured`, `the_capture_wins_however_the_threads_interleave`, `captures_on_different_threads_are_isolated`, `the_guard_ends_the_capture_and_installs_nest`.
- **S21** (code). Code: `#![forbid(unsafe_code)]` in `lib.rs`; workspace lints `unwrap_used = "deny"`, `expect_used = "deny"`; `problem.rs` and `guard/problem.rs` (generic bodies). Tests: `hygiene.rs`: `forbid_unsafe_present`; `guard.rs`: `problem_json_carries_no_internals`; clippy `-D warnings` in `identity-lint` ([H] exit 0). **Deviation:** the plan named a test `no_unwrap_on_input_derived_data`; it does not exist. What stands in for it is the clippy deny lints (compile-time, in `identity-lint`) and the proptests `any_bytes_satisfy_the_jwt_invariants`, `any_bytes_satisfy_the_cookie_invariants`, `any_bytes_satisfy_the_audit_invariants`, `json_shaped_text_satisfies_the_audit_invariants`, `path_like_text_is_judged_by_the_return_to_rule` (`corpus_replay.rs`). That is weaker than a named test; the reviewer should decide whether it is enough.
- **S22** (fuzz every parser at a trust boundary). Targets `identity_jwt`, `identity_cookie`, `identity_audit_event` in `crates/hub-identity/fuzz/` (own crate and lockfile, as the agent's, decision A16). [H] `identity-fuzz`: 30 s each, no crash. Seed replay on stable runs in `just verify`: `corpus_replay_identity_jwt`, `corpus_replay_identity_cookie`, `corpus_replay_identity_audit_event`, `every_fuzz_target_has_a_corpus_and_a_replay`; `justfile_gate.rs`: `identity_fuzz_list_matches_the_fuzz_targets_directory`. The per-target run counts were not supplied.

Threat model: `docs/threat-model.md`, `## Hub identity and audit (03a)` (T6), checked by `tests/threat_model.rs`. A human signs it off before go-live (security skill, step 4).

## Benchmarks against budgets

[H], one `just verify-03a` run, loopback Postgres in a container, p95, from the scoped `bench-check` and repeated by the full `cargo xtask bench-check`:

| P# | Benchmark | Result | Budget |
|---|---|---|---|
| P7 | `hub-identity/auth_middleware` (`P7.auth_middleware`, warm cache, 100 sessions) | PASS 0.004 ms | <= 2 ms |
| P7 | `hub-identity/auth_middleware_cold` (`P7.auth_middleware_cold`, miss, one indexed read) | PASS 0.550 ms | <= 2 ms |
| P12 | `hub-identity/audit_record` (`P12.audit_record`, with a second replica contending for the chain lock) | PASS 2.605 ms | <= 3 ms |

**P12 is a range, not a single number.** [S], without Docker and with a local Postgres, fsync on: eight runs at `f912e90` gave p95 between 2.38 and 3.12 ms (one run, 3.115 ms, was over the 3 ms budget); later runs gave 1.86 to 2.26 ms. The human **accepted the 2.4 to 3.1 ms range as meeting P12** and the budget stays at 3 ms. The 2.605 ms above is inside that range. No single run is a pass or a fail by itself; the result is "inside the accepted range, with run-to-run noise that reaches the budget". `perf/budgets.toml` was not loosened.

Other P# evidence: `explain_session_lookup_is_index_scan` and `session_lookup_uses_primary_key` (sessions), `explain_audit_head_is_backward_pk_scan` (audit head), `explain_user_list_is_keyset_index_scan`, `explain_access_request_list_uses_status_index`, `lists_have_no_offset_and_cap_limit_at_500` (users) cover P12's "EXPLAIN checked in, keyset only". All benchmarks run on loopback: they bound code and query time, not network time; prompt 16 must measure staging. The nightly verification job has no budget of its own and **its run time at one million events was not measured** (the plan promised to report it; see Known gaps item 5).

### Contract and Extra-path usage

- `perf/budgets.toml`: exactly three new `[[budget]]` blocks (`P7.auth_middleware`, `P7.auth_middleware_cold`, `P12.audit_record`), all `registered = true`; no existing threshold changed (`git diff origin/main...HEAD -- perf/budgets.toml`, [F]).
- Extra-paths approved with the plan: `docs/performance.md` (two sentences, one under P7 and one under P12), `crates/xtask/tests/budgets_registry.rs` (three ids in `every_p_number_has_an_entry`, in the docs-comparison table and in `REGISTERED`). `cargo test -p xtask` passes ([F]).
- The other contract changes were made in contract PR #6 (`contract/03a-identity`: `DomainEvent::UserSessionsRevoked`, `EventKind::UserSessionsRevoked`, `Me.needs_github_token`, the `__Host-` cookie names in `api/openapi.yaml`). It is merged and was a prerequisite of T3 ([H] report). This branch's diff against `main` contains no other contract path (`crates/domain`, `crates/ports`, `proto/`, `api/`, `db/migrations/`, `docs/mcp-tools.md`, `design/handoff/**` are all absent from `git diff --name-only`). No migration was added (decision B6).
- Everything else outside `crates/hub-identity/**`: `docs/threat-model.md` (owned), `Cargo.toml`, `Cargo.lock`, `Justfile` (shared, allowed), this plan file.

## New dependencies

Rule 15 of `AGENTS.md`: only crates.io, pinned versions at least 14 days old, each justified, `cargo deny` and `osv-scanner` clean. Ages are days between the crates.io `created_at` of the exact version and 2026-10-11 ([F], crates.io API).

**Workspace pins added in `Cargo.toml`** (exact `=x.y.z`, default features off). All five of `aws-lc-rs`, `base64`, `url`, `rustls-native-certs` and `axum` were already in `Cargo.lock` at the same version through other crates (their pins were set to the lockfile versions), so none of them adds a package version to the root lock.

| Crate | Version | Age (days) | Why | In root lock before |
|---|---|---|---|---|
| `aws-lc-rs` (features `aws-lc-sys`, `alloc`, `ring-sig-verify`; dev adds `ring-io`) | 1.18.1 | 40 | The stack's one crypto provider: RS256 signature check of id and access tokens, AES-256-GCM for the login cookie, random ids, ECDSA P-256 verify for checkpoints. `dev` `ring-io` lets the mock Entra read the modulus and exponent of a generated key. | yes (through rustls) |
| `base64` (`std`) | 0.22.1 | 894 | Cookie, PKCE and JWT segment encoding. | yes |
| `url` (`std`) | 2.5.8 | 279 | Builds and checks the endpoint URLs and the redirect. | yes |
| `rustls-native-certs` | 0.8.4 | 132 | System roots for the HTTPS client (`webpki-roots` is refused by `deny.toml`). | yes |
| `axum` (`tokio`, `http1`, `query`, `matched-path`) | 0.8.9 | 180 | The web framework of the stack: the layers, extractors and `/auth`, `/me`, `/admin` routes. | yes (through tonic); its edges to `hyper`, `hyper-util`, `tokio`, `serde_path_to_error`, `serde_urlencoded`, `form_urlencoded` are new |
| `moka` (`future`) | 0.12.16 | 63 | The stack's cache: the session cache (at most 50,000 entries, 30 s, single-flight loads). | no, new |

**Packages `Cargo.lock` gains** (`git diff origin/main...HEAD -- Cargo.lock`: six packages plus `hub-identity` itself):

| Package | Version | Age (days) | Pulled in by |
|---|---|---|---|
| `moka` | 0.12.16 | 63 | direct |
| `async-lock` | 3.4.2 | 294 | `moka` |
| `crossbeam-channel` | 0.5.17 | 36 | `moka` |
| `serde_path_to_error` | 0.1.20 | 391 | `axum` (`query`) |
| `tagptr` | 0.2.0 | 1885 | `moka` |
| `uuid` | 1.26.1 | 31 | `moka` |

All six are at least 31 days old. Existing packages `sqlx-postgres` and `sqlx-sqlite` gain a `serde` edge and `sqlx-macros-core` gains `sqlx-mysql`, `sqlx-sqlite` edges from `sqlx`'s `json` feature (no new package versions).

**Not added, against the plan:** `jsonwebtoken` (the plan's decision B1 proposed it) and `openidconnect` (the prompt names it). `openidconnect` 4.0.1 needs `rsa ^0.9.2` (RUSTSEC-2023-0071). `jsonwebtoken` was dropped during T1: the `Cargo.toml` comment says it adds a provider switch that panics when none is installed (rule 2: no panics), so signature checks are done on `aws-lc-rs` directly with header, size and algorithm checks in our code (`oidc/jwt.rs`). This differs from the plan's dependency table; see "Deviations".

**Features and dev-dependencies of crates already pinned** (no new packages): `hyper` (`client`, `http1`), `hyper-util` (`tokio`), `tokio-rustls`, `rustls` (dev: `tls12`, for the test that proves the client refuses TLS 1.2), `sqlx` (`postgres`, `runtime-tokio`, `json`; dev: `migrate`), `tower` (dev: `util`), `tokio` (dev: `test-util`, `rt-multi-thread`), `tracing-subscriber` (dev: `fmt`, `std`), `criterion`, `testcontainers`, `proptest`, `insta`, `rcgen`, `tempfile`, `fastrand`, `futures`, `bytes`, `http`, `http-body-util`, `serde`, `serde_json`, `sha2`, `subtle`, `thiserror`, `zeroize`.

**Fuzz crate lock (`crates/hub-identity/fuzz/Cargo.lock`, new, 242 registry packages).** 228 of those versions are the same as in the root lock or in the agent's fuzz lock. The other 14 were resolved fresh, and **12 of them are younger than 14 days on 2026-10-11, which breaks rule 15:**

| Package | Version in the fuzz lock | Age (days) | Root and agent-fuzz lock version |
|---|---|---|---|
| `cc` | 1.7.0 | 1 | 1.4.7 |
| `smallvec` | 1.16.3 | 1 | 1.15.2 |
| `syn` | 3.0.7 | 1 | 3.0.6 |
| `zerocopy` | 0.8.62 | 3 | 0.8.58 |
| `zerocopy-derive` | 0.8.62 | 3 | 0.8.58 |
| `arbitrary` | 1.5.0 | 5 | 1.4.2 (agent fuzz lock only) |
| `either` | 1.19.0 | 5 | 1.18.0 |
| `want` | 0.3.2 | 6 | 0.3.1 |
| `mio` | 1.2.4 | 8 | 1.2.3 |
| `libc` | 0.2.190 | 9 | 0.2.189 |
| `lazy_static` | 1.5.1 | 10 | 1.5.0 |
| `yoke-derive` | 0.8.4 | 11 | 0.8.2 (root lock only) |

The other two fresh versions are old enough (`find-msvc-tools` 0.1.14, 16 days; `libfuzzer-sys` 0.4.13, 129 days). This is a defect in a committed artifact, not something this FINAL step may fix; it needs a human decision (Known gaps item 4). It does not affect the production build (the fuzz crate is outside the workspace); it affects the fuzz binaries and S19's spirit.

`cargo deny` / `osv-scanner`: [H] `identity-fuzz` ran `cargo deny ... check` and `cargo audit` on the fuzz lock under `set -e` and the recipe exited 0, so both passed there; [H] `just verify` run 3 (exit 0) runs `cargo deny check` and `cargo audit` on the root lock. The raw output tails of these were not supplied. `osv-scanner` is not part of `verify-03a` or `verify` and was not run by anyone in this prompt's evidence; CI or a human must run it.

## Assumptions

The plan was approved as a whole (`ab07acb`, "approved by a human"; the status line changed from DRAFT to APPROVED and nothing else). The plan's ambiguity table lists proposed defaults, and no per-question human answers are recorded in the repository. So each default below is "approved with the plan", and the entries marked **needs a human** were asked in the plan and have no recorded answer.

- **B1 token crate** (plan: do not use `openidconnect`; use `jsonwebtoken` on aws-lc-rs plus our own flow). Implemented without either crate: our own flow and verification on `aws-lc-rs`. See Deviations.
- **B2 outbound HTTPS.** One `HttpsClient` trait; `HyperHttpsClient` over `tokio-rustls`, TLS 1.3 only, system roots, host allowlist, 5 s timeout, 1 MiB body cap, bounded retries (GET only). The plan's request to confirm with the owners of 03b and 06 that they do not build a competing client is not done.
- **B3 login state.** Sealed 10-minute `__Host-lk_login` cookie, AES-256-GCM, key from `SecretSource`. Single use rests on Entra rejecting a reused code plus the nonce check; there is no server-side one-time record (residual risk R3 in the threat model). Needs a Secret Manager entry for the cookie key, which prompt 09 provisions.
- **B4 `#[guarded(...)]`.** A `GuardedRouter` builder that takes the role as a parameter plus `assert_default_deny`, not an attribute macro (a proc-macro crate is outside this prompt's paths).
- **B5 revocation event.** `UserSessionsRevoked` through the outbox, from contract PR #6 (merged).
- **B6 migrations.** None.
- **B7 `simple_audit`.** T2 and T3 integration tests used a test-only chain writer (`tests/support/simple_audit.rs`, per the plan) until `PgAuditLog` landed in T5. In T5 the rigs were switched to `PgAuditLog` (`tests/support/audit_rig.rs`, `guard_rig.rs`, `pg_tx.rs`); no `simple_audit` file or type remains in the tree (grep, [F]).
- **B8 Cloud Logging copy.** A leased `AuditLogShipper` writes committed events, one log line each, **without the diff** (it carries `diff_sha256` and `diff_len`), because S10 says logs hold hashes and not content. **Needs a human** to confirm that Cloud Logging may lack the diff text (the audit table keeps it).
- **B9 GCS client and checkpoint public key.** `CheckpointBucket` and `GcsCheckpointBucket` live in this crate (create-only upload with `ifGenerationMatch=0`, metadata-server token). **Needs a human** to confirm the GCS client belongs here and not in 03b.
- **B10 canonical JSON.** `ports::canonical_json` and `ports::chain_hash` are the single definition; the prompt's "sorted keys" wording is superseded by the contract.
- **B11 `TokenVerifier` and active users.** The verifier checks the token only; `McpAuthenticator` composes it with `Users::require_active` and prompt 07 calls that.
- **B12 token presence.** `GithubTokenPresence` is a one-method trait local to this crate with a Postgres `SELECT EXISTS` that never selects ciphertext. **Needs a human** if it should live in `crates/ports`.
- **B13 bootstrap admins** apply at first sign-in only. An existing user added to the list later is not promoted, and if every admin is disabled, recovery is a database intervention (recorded in the threat model). **Needs a human** for a break-glass rule.
- **B14** the `requires_approval` flag change is audited as `RoleChanged` with a short diff text; no new audit action.
- **B15** an admin cannot change their own role or status; the flag can be changed by another admin only.
- **B16** problem types `access-pending`, `access-disabled`, `insufficient-role`; the callback redirects pending and disabled users to a page prompt 08 builds.
- **B17 `Idempotency-Key` on admin writes.** Not implemented in 03a: the header is accepted and ignored, and a repeat approve or reject answers 409. No layer owns this yet. **Needs a human** to name the owner.
- **B18 federated credential.** Assertion file path and audience `api://AzureADTokenExchange` in config; tested against the mock only (Q25).
- **B19** expiry sweeps, `last_seen_at` writes and cache evictions are not audited (maintenance, not user actions).
- **B20 lock held until the caller commits.** Documented: callers call `record` last. The P12 benchmark includes a contending second instance. P12 sits in an accepted range (above).
- **B21** at most one pending access request per user and one new request per 10 minutes.
- **No rate limit on `/auth/*`.** That belongs to the ingress or WAF (prompt 09).
- Queries are runtime-checked (`sqlx::query`), not `query!`, because `query!` needs a live database or `.sqlx` metadata at compile time and the xtask harness has the same rule (comment in `crates/hub-identity/Cargo.toml`). The `lanekeeper-rust-conventions` skill prefers `query!`; the exact statements are covered by `EXPLAIN` tests instead.
- Idle time is judged on `last_seen_at`, which is written in batches (at most once a minute per session), so the 8 h idle limit is accurate to about two minutes across replicas (threat model).

## Deviations from the plan

There is no `plans/03a-hub-identity-audit.blockers.md`. The plan is frozen and was not edited after approval. These differences between the plan and the code exist and are stated here so the reviewer and the human can see them:

1. **No `jsonwebtoken`.** The plan's design note B1 and dependency table list `jsonwebtoken` (feature `aws_lc_rs`); T1 dropped it and verifies RS256 signatures with `aws-lc-rs` directly (T1 commit message and workspace `Cargo.toml` comment). Effect: no `jsonwebtoken` in the lock; more of the verifier is our own code, covered by the negative tests and the `identity_jwt` fuzz target. A human should confirm this, since the plan's B1 said "needs a human".
2. **No test named `no_unwrap_on_input_derived_data`** (S21; see above). Stand-ins are listed there.
3. **Fuzz lock** with 12 packages under 14 days old (see New dependencies).
4. **Nightly verification time at one million events** was promised in the plan's performance risks and was not measured.
5. **T4's commit** (`ae84a4b`) has a subject line only; the plan's T4 file list is what it contains. The T1 and T3 messages describe the work in more detail.

## Fixes found by the human's gate run (`c44cbdb`)

`c44cbdb` came from the first [H] gate run, not from review. Two failures, both fixed and re-run:

- `identity-test` (`audit_verify`: empty or polluted log capture). `tracing` caches interest per callsite; with exactly one live thread-local subscriber, a callsite first reached on a thread without one is cached as "nobody wants it", so a parallel test hid the production log line. `tests/support/log_capture.rs` now installs one global subscriber per binary that routes each event to the buffer of the thread that emitted it. `tests/log_capture_callsite.rs` drives the failing interleavings deterministically. sqlx's statement log then echoed the test's own tamper SQL, so the marker is a bound parameter (`AuditRig::admin_sql_bound`) and the test plants a self-check proving its search can fail.
- `identity-bench` (exit 101 at teardown). testcontainers removes a container from `Drop` through `Handle::current()`, which panics outside a runtime. `TestDb::shutdown()` now tears down inside the runtime; `Drop` is a best-effort fallback (`docker rm` as a bounded child process). `tests/pg_teardown.rs` proves the logic without Docker. `identity-bench` also clears `target/criterion/hub-identity` first. [H] re-run: exit 0, zero "could not remove the test container" lines, no containers left behind.

## Known gaps and follow-ups

Read these before merging. Nothing here is hidden behind a tick above.

1. **`just verify` is flaky on prompt 02's tests, not on 03a's.** [H] two of three runs failed, in `crates/agent`, which this branch does not modify. Both causes are understood; no patch is applied here (outside this prompt's paths, and not mine to change).
   - `crates/agent/tests/kube_watch.rs:310` (`no_full_report_while_the_lists_are_refused_and_the_log_has_only_the_status`, "the status is logged: []"): the same `tracing` callsite-interest cache race that `c44cbdb` fixed in `hub-identity`. The agent's `tests/support/log_capture.rs:28` still uses `set_default`. Reproduced in the sandbox: 5 of 80 parallel runs fail, 0 of 80 alone or with `--test-threads=1`. Proposed patch: port the routed global-subscriber helper from `crates/hub-identity/tests/support/log_capture.rs` into the agent tests.
   - `crates/agent/tests/binary.rs:72` (`binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm`, exit code `None`, killed by SIGTERM): a SIGTERM race. `shutdown_signal()` in `crates/agent/src/main.rs` is an `async fn`, so `signal(SignalKind::terminate())` registers only on its first poll inside `assemble`, after `process::run` has bound the health port; a SIGTERM that arrives in between kills the process. Reproduced only under CPU contention. Proposed patch: make `shutdown_signal` a plain `fn` that registers the handlers before the port is bound, and optionally print a readiness line.
   - Both need a fix PR against prompt 02's paths (or an extra-path approval). Until then `just verify` may need a re-run, and a green third run is not proof that the tests are fixed.
2. **Not verified anywhere** (the threat model lists these): a real Entra tenant and the GKE federated credential (Q25: the real v2 token shape, JWKS rotation, and Entra's behaviour with the GKE issuer; stop-and-escalate in the prompt, needed before go-live), Cloud KMS, a locked GCS bucket (S9 end to end), and the GKE metadata server (`AccessTokenSource`). All were tested against mocks or fakes only.
3. **P12 `audit_record` has a noisy p95 that reaches the budget.** Range 2.38 to 3.12 ms over eight [S] runs with fsync on (one at 3.115 ms, over 3 ms), 1.86 to 2.26 ms in later runs, 2.605 ms in the [H] run. The human accepted 2.4 to 3.1 ms as meeting P12. The budget stays 3 ms; the number depends on the disk's fsync cost, so prompt 16 should measure it on the staging database. If a deployment's fsync is slower, the advisory lock (held from `record` to commit) is the term that grows.
4. **Fuzz lockfile has 12 packages younger than 14 days** (table above), against rule 15. Proposed fix, for a human to approve or reject: in `crates/hub-identity/fuzz`, `cargo update -p <name> --precise <version>` for each of the twelve to the version in the last column (all exist in the root or agent-fuzz lock), then `cargo deny` and `cargo audit` on the fuzz lock, then commit. Alternatively wait: the youngest (published 2026-10-10) becomes 14 days old on 2026-10-24, and the lock could be regenerated then.
5. **Nightly verification and shipper cost at scale is unmeasured.** The plan said the PR would report the verifier's run time at one million events; no benchmark or test does that. The verifier streams by keyset in batches of 5,000 and `nightly_verify_streams_in_bounded_batches` proves the batching, not the time.
6. **Depends on prompt 09:** the hub binary running `assert_hub_route_table` and `assert_default_deny` against its full route table; the Secret Manager entry for the login-cookie key; provisioning of the KMS key, the locked bucket and the federated credential.
7. **No rate limit on `/auth/*`** (ingress or WAF, prompt 09).
8. **Admin writes have no `Idempotency-Key` layer** (B17). A repeat is safe (409 on the second approve or reject) but the header is ignored. Name an owner.
9. **The Cloud Logging copy omits the diff** (B8); the audit table keeps it. A human must confirm.
10. **Bootstrap admins apply at first sign-in only** (B13). If every admin is disabled, recovery is a database intervention.
11. **Residual risks accepted in the plan and written in the threat model:** no server-side one-time login record (R3); idle limit accurate to about two minutes (batched `last_seen_at`); a replica partitioned from Postgres can serve a cached session for up to 30 s; truncating the audit tail between two checkpoints is detected only up to the last checkpoint (at most an hour of history); a database owner who also controls the KMS signing path could forge a chain.
12. **Benchmarks are on loopback** and bound code and query time only.
13. **`osv-scanner`** was not run by anyone for this prompt.
14. **Reviewer agent** (`prompts/REVIEW.md`) has not run yet; the PR text says so.
