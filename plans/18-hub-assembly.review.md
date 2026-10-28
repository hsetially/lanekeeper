# Review of prompt 18 (hub assembly): round 1

Reviewer: independent `reviewer` agent, run per `prompts/REVIEW.md`. Base: `origin/main`. Branch head reviewed: `aab5c71` (the evidence commit). The text below is the reviewer's report, saved as returned.

VERDICT: CHANGES REQUESTED

**Blocking**

1. Known and already escalated as B1, not a new finding: `just verify` is red, so the PR cannot be approved yet. `cargo test -p hub-platform --test ca_key_use` fails with `Finding { file: "web_tls.rs", line: 129, what: "PrivateKeyDer:: in decode" }`.
   - `crates/hub/src/web_tls.rs:129` (T1, plan G3) trips S5's source scan. The scan's `ALLOWED` list (`crates/hub-platform/tests/ca_key_use.rs:71`) holds one function.
   - I reproduced it. The blockers file and the evidence describe it accurately: one finding, same file and line, `hub-test` does not run this test (so verify-18's sub-recipes are green while `just verify` is red), and the three options are stated correctly.
   - What remains is the human's choice. If A is chosen, three follow-ups: put `ca_key_use.rs` in Extra-paths (or a 03b follow-up PR); add a decisions entry saying S5's "no private key from bytes" means the CA key and that the web key is the one other key the hub holds (S6, S7); add `cargo test -p hub-platform --test ca_key_use` to `hub-test` in the Justfile.

2. `crates/hub/src/limits.rs:336-343`: the in-flight bound `LARGE_BODIES_IN_FLIGHT` (8 permits, D137) has no test that goes through `enforce_body_cap`. It violates AGENTS rule 13 and the lanekeeper-security rule that every control has a test that fails if the control is removed.
   - The only test is the unit test at `limits.rs:506`. It takes permits straight off `limits.large_bodies`, so it tests tokio's semaphore, not the middleware.
   - `crates/hub/tests/body_limits.rs:9-12` says "the number of large bodies in flight is bounded" is proved. D137's "Proof" cites the same unit test. Neither is true of the middleware.
   - Failing scenario: delete the acquire and all 516 hub tests still pass. One Editor then opens 100 undeclared-length 12 MiB `PUT /files` requests at once, and a replica buffers about 1.2 GiB instead of about 100 MiB (P11 budgets 1,536 MiB per replica).
   - Fix: add `body_limits::at_most_eight_large_bodies_are_read_at_once`.
     - Hold 8 eligible-session bodies open: a streamed body with no length that never finishes.
     - Assert the 9th large body gets 429 with Retry-After, and that a body declared at 4 MiB or less is unaffected.
     - Finish one body and assert the next large body succeeds.
     - Correct the header comment and D137's "Proof" line.

**Non-blocking**

1. `crates/hub/tests/csrf_all_routes.rs:146` walks `Hub.routes` of the identity rig: identity, the event stream, MCP and a probe module. It does not walk the real registry and write routes, though the plan's traceability row and prompt T4 name them.
   - Coverage there is structural: the layers wrap the whole router, and removing `CsrfLayer` would fail the test.
   - One real writes route is covered at `registry_writes.rs:423` (forged token gives 403).
   - Cheap improvement: run the same walk over the `Running::routes()` that `route_table_full` already boots.
2. `crates/hub/examples/support/dev_login.rs:156-176` (dev only, example target):
   - It upserts an Admin user and creates a session with no audit event.
   - It checks that the listener is loopback but not that the database is.
   - Any local page can `GET /` to sign the browser in (login CSRF).
   - It is not reachable from the release binary or the image. `src/` has no reference to `dev_login`, `hub_dev`, `soft_kms` or `LK_DEV_`. `release_build` passes (9 tests), `hub-lint`'s release-graph check passes, and the Dockerfile builds only `-p hub`.
   - Suggest refusing a non-loopback database host and adding a one-line "never point at a shared database" note.
3. `crates/hub/src/config/platform.rs:71-73`: `LK_AGENT_CA_KMS_REF` and `LK_REPLICA_CA_KMS_REF` (and the two certificate refs) may be equal. That silently collapses the "each listener trusts only its own CA" separation to the replica service's SAN-prefix check. Refuse equal values.
4. `crates/hub-platform/src/session_auth.rs:67`: `binding_valid` defaults to true. An implementation that overrides `stream_binding` and forgets `binding_valid` fails open. Consider one provided method returning an enum.
5. `crates/hub/fuzz/invariants.rs:824,864`: the replacement for the "error repeats the secret" invariant does not weaken protection. `PemError` is a fieldless Copy enum, and an exhaustive-match test holds `FIXED_PEM_ERRORS` to it. The runtime assertion is now true by construction; the protection is the type's shape.
   - The two product fixes (a second certificate marker refused, an end-before-start CA certificate refused) have tests that would fail without them: `ca_init::a_second_certificate_marker_that_the_pem_grammar_skips_is_still_refused` and `corpus_replay::a_ca_certificate_that_ends_before_it_begins_is_refused_and_the_invariants_agree`.
6. Time-based negative waits, against "no sleeps": `crates/hub/tests/identity_module.rs:638,651,691` and `crates/hub/tests/process.rs:335`. They assert that nothing happens in a window, and are acceptable but flake-prone.
7. Traceability names in the frozen plan that differ in the code. The evidence maps most of them.
   - The S18 HMAC row (`registry_writes::the_webhook_still_requires_a_valid_hmac_through_the_composed_hub`) does not exist. The check is the tail of the P2 test (`registry_writes.rs:693`, forged signature gives 401 or 403). A stand-alone test would be clearer.
   - `without_the_replica_name` is `without_the_replica_name_or_address_or_audience_or_agent_hosts`.
   - `one_audit_event_per_changed_row_in_the_same_transaction` is `the_rows_and_their_events_are_one_transaction`.
8. `docs/decisions.md` D138 and D144 say "Decided by the human at the approval gate" for G9, G11, G21, G22 and G26. Those were not marked "needs a human"; plan approval covers them. Reword.
9. The branch carries `3e68646`, the work-order and survey commit from `origin/docs/18-hub-assembly-prompt`. It adds `prompts/18-hub-assembly.md`, `prompts/README.md` and the base of `docs/hub-assembly-survey.md`. It is not an implementer scope violation and drops out of the diff if that PR merges first. The commits `18/fix:`, `18/blocked:`, `18/evidence:` and `18/answers:` are not `NN/Tk:` task commits; the eight task commits are in order T1 to T8.
10. `crates/hub/src/lib.rs:3` still says "Owned by prompt 03b".
11. P11 is measured with software edges and an empty registry (no agent rows), so "a database of the fixture size" is not literally met. The bench says so honestly. The real Google latency and the whole-hub memory at design scale belong to prompts 09 and 16. S11's wording in `docs/security.md` still says 4 MiB (already noted in D137).

**Evidence re-run**

- Scope: `git diff origin/main...HEAD --name-only` outside `crates/hub` is only the approved Extra-paths and shared files. `docs/mcp-tools.md` is untouched, and there is no new `AuditAction`. The CC1 diff in `perf/budgets.toml` is exactly two additive entries, nothing loosened.
- Plan: the plan file changed only in `8ca3452` (draft) and `53cb876` (approved by a human), and is unchanged since (`git diff 53cb876 HEAD` is empty). Extra-paths and Contract-change match the diff.
- `just hub-lint`: exit 0. fmt, clippy (`-p hub --all-targets --all-features -D warnings`) and the release-graph check (none of the 7 test features) pass. `cargo deny check` is ok. `cargo audit` reports 10 allowed warnings (zstd-safe, D135).
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0, but the result was cached (identical fingerprint to the implementer's tree).
- `cargo test -p hub --all-features`, 36 runs one at a time (`LK_REQUIRE_DOCKER=1`, `CARGO_INCREMENTAL=0`, big binaries deleted after each): 516 passed, 0 failed, matching the evidence. lib 235, bins 0, plus the 34 files in `crates/hub/tests/`. Notable counts:
  - body_limits 4, csrf_all_routes 5, route_table_full 7, identity_module 16
  - migrate 18, seed 15, refuses_to_start 32, process 8
  - platform_cluster 10, registry_writes 6, docs_unavailable 8, mcp_module 11
  - release_build 9, hardening 7, corpus_replay 19, web_tls 6
- `LK_REQUIRE_DOCKER=1 cargo test -p hub-platform --test events_sse`: 32 passed (the G2 per-session stream tests included).
- `cargo test -p xtask --test budgets_registry`: 8 passed.
- `cargo test -p hub-platform --test ca_key_use`: FAILED, 5 passed and 1 failed. B1 reproduced with the evidence's exact finding.
- `just hub-fuzz`: exit 0. `cargo deny` and `cargo audit` on `crates/hub/fuzz/Cargo.lock` are ok. All four targets ran 30 s without a crash (1,712,733 / 354,861 / 218,013 / 15,732 runs). The fuzz lockfile matches the root lockfile by version and checksum except `arbitrary` 1.4.2 and `libfuzzer-sys` 0.4.13.
- `just hub-bench`: exit 0. P7.auth_composed is 0.027 ms p95 (budget 2), P7.auth_composed_cold 1.177 ms (budget 2), P11 152.6 ms (budget 10,000).
- `cargo bench -p hub-platform --bench sse_delivery --bench push_to_drift_webhook`: P13 p95 4.61 ms (budget 1,000), P2 p95 25.5 ms (budget 10,000). No regression against the evidence's 5.4 and 32.4 ms.
- `cargo xtask bench-check` (whole registry): 11 pass, 62 fail, 2 unmet.
  - Every FAIL I sampled is "registered but no result" (other prompts' benchmarks, not run); I did not list all 62.
  - The 2 unmet are P7.docs_search and P7.feature_status (prompt 14).
  - I freshly re-ran P2, P7.auth_composed, P7.auth_composed_cold, P11 and P13.
  - P12.* (3), P2.hub_head_moved_to_drift, P11.memory_mib and P10 come from the implementer's earlier result files and were NOT re-run by me; the crates behind the P12 and P2.hub_head_moved_to_drift budgets are untouched by the diff.
- Dependencies: serde-saphyr 1.1.0 (2026-08-15), x509-parser 0.18.1, zeroize 1.9.0, fastrand 2.5.0, pem 3.0.6, criterion 0.8.2 and url 2.5.8 are all at least 14 days old per the crates.io API, none yanked. The root `Cargo.lock` gains 13 edge lines and no package, as the evidence says.
- NOT RUN: the literal `just verify-18`. About 24 GB of hub test binaries do not fit on this disk, so `hub-lint`, `hub-test` (per target) and `hub-fuzz` were run separately. `hub-tools-check` was not run as its own recipe (docker, nightly and cargo-fuzz were exercised by the other runs).
- NOT RUN: `just verify` as one command. Run: fmt, workspace clippy, `cargo deny`, `cargo audit`, and the hub, `events_sse`, `budgets_registry` and `ca_key_use` tests. Not re-run: the remaining workspace tests of untouched crates, and `web-verify` (`web/` is untouched).
- NOT RUN: osv-scanner, kind, helm, and the prompt-09 chart checks (not installed or not available here).
- Security trace (impl file to test):
  - S1: `crates/hub/src/session_auth.rs` to `identity_module::pending_and_disabled_users_get_403_on_every_composed_route` and `::sign_in_against_a_local_identity_provider_gives_a_working_cookie`.
  - S2: `session_auth.rs:116-161` plus `crates/hub-platform/src/events/sse.rs` to `identity_module::sign_out_ends_the_session_and_its_live_stream`, `::a_role_change_on_one_replica_ends_sessions_on_the_other_within_a_second`, and `events_sse` (32).
  - S3: `compose.rs:258-265` to `csrf_all_routes::*` and `mcp_module::the_personal_token_routes_need_the_csrf_token`.
  - S4: `compose.rs`, `allowances.rs`, `static_files.rs:138-150` to `route_table_full::*` and `identity_module::default_deny_holds_with_static_files_on`.
  - S5: `wiring/grpc.rs`, `wiring/platform.rs`, `ca_init.rs` to `platform_cluster::each_listener_trusts_only_its_own_ca`, `::a_ca_certificate_that_does_not_belong_to_the_kms_key_refuses_to_start`, `seed::a_seeded_agent_joins_through_the_agent_listener`. The `ca_key_use` scan is red (B1).
  - S6: `web_tls.rs`, `serve.rs::tls13_resolver_config`, `db.rs` verify-full to `web_tls::*`, `pg_tls` (5), `migrate::the_migrator_connection_verifies_the_server`, `platform_cluster::both_grpc_listeners_require_client_certificates`.
  - S7: `config/*`, `web_tls::decode` to `hardening` (7), `process::no_planted_secret_appears_in_the_process_output`, `refuses_to_start` (32), `hub_pem` fuzz.
  - S9: `identity.rs` audit jobs, `seed/apply.rs` to `identity_module::the_checkpoint_and_verify_jobs_run_on_one_replica_and_stop_on_shutdown`, `registry_writes::every_write_has_exactly_one_audit_event_and_the_chain_verifies`, `seed::the_rows_and_their_events_are_one_transaction`. `hub migrate` writes no audit event by the human's D138.
  - S10: fixed-text errors to `process::no_planted_secret_appears_in_the_process_output`, `migrate::the_binary_applies_the_migrations_prints_the_versions_and_never_a_credential`, `seed::the_binary_seeds_prints_ids_and_column_names_and_never_a_value_or_a_credential`, `identity_module::no_secret_and_no_personal_data_reach_the_logs_through_the_composed_hub`.
  - S12: `route_table_full::every_composed_route_carries_the_security_headers`.
  - S16: `HARDENING.md` and the Dockerfile to `hardening` (planted-violation cases included).
  - S18: `hardening` network test, `limits.rs` webhook 1 MiB (`body_limits::the_webhook_body_is_capped_at_1_mib`), forged-signature assertion in `registry_writes::push_to_drift_through_the_composed_hub_meets_p2`.
  - S21: workspace lints and `errors::problem_bodies_have_no_internals_for_every_composed_module`.
- Body caps (D137): `body_limits` 4 of 4 pass. The 12 MiB + 64 KiB cap needs an Active user of at least Editor with a live session, on exactly `PUT /files` and `POST /drafts`. An unauthenticated or CSRF-failing request is capped at 4 MiB or refused before buffering. The webhook is 1 MiB. The in-flight bound is untested (Blocking 2).
- `UnavailableDocs`: all 8 `DocSearch` methods return `Err(DocError::Unavailable)`. `release_build::the_only_docsearch_in_the_hub_is_the_named_unavailable_one` holds it to that, and `docs_unavailable` (8) passes.
- Threat model updated: yes, section "Hub assembly (18)" in `docs/threat-model.md`, guarded by `threat_model` (4 tests). It does not mention B1, which came after it was written.
- Cleanup: I deleted `target/debug`, large executables under `target/release`, `crates/hub/fuzz/target` and `target/fuzz-corpus/hub_*` (all build artifacts). `git status` is clean.
