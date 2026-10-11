# Evidence 03b: Hub platform

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proved by a named test, bench or fuzz target.

Measured on `agent/03b-hub-platform`, 2026-10-11, Linux x86_64, rustc 1.88.0, Docker 29.8.2 (a hand-started `dockerd`), Postgres 16 with pgvector through testcontainers. The first version of this file was written at `a4b8f09` (T11). It was revised in the round 1 FIX step (`03b/fix:` commits on top of `f1b04e4`); every number below is from the fixed tree unless it says otherwise.

## Read this first: what ran, and what differs from the recipe

1. **`just verify-03b` ran as one recipe and passed (exit 0), after the fix.** `just`, `nightly-2026-09-14` and `cargo-fuzz 0.13.2` are present on this machine now (they were not in the implementer's first run). The recipe ran `platform-tools-check`, `platform-lint`, `platform-test`, `platform-slow-test`, `platform-fuzz` and `platform-bench`, in order, with `LK_REQUIRE_DOCKER=1`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0`. Two earlier attempts in this FIX step failed on my own change and were fixed (not skipped): the first on `crates/hub/tests/git_route.rs` (the webhook's route-table reason string exists in both `routes.rs` and `webhook.rs`, and I had edited one), the second on `cargo fmt --check` of that edit. The third run is the one recorded here.
2. **The libFuzzer half ran.** `platform-fuzz` checked the fuzz lockfile (`cargo metadata --locked`, `cargo deny`, `cargo audit --file`) and then fuzzed `hub_agent_message`, `hub_forward_request`, `hub_join_csr`, `hub_sentinel_message` and `hub_webhook_payload` for 30 s each on `nightly-2026-09-14` ("Done ... runs in 31 second(s)" five times; the recipe ends `platform-fuzz: ... ran 30 s each without a crash`). The reviewer's round 1 re-run of the whole recipe gave the same result (exit 0, 674 tests, five targets x 30 s, no crash); that re-run is what first proved items 1 and 4 of the acceptance list; mine repeats it on the fixed tree. The stable corpus replay (`corpus_replay`, 16 tests) also passes inside `platform-test`.
3. **Debug info was turned off for the debug-profile builds** (`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, environment only; no file changed), because of disk space (about 11 GB free). This changes binary size only, not behaviour. Release-profile runs (slow tests, benches) are unaffected.
4. `osv-scanner` is not installed (the Justfile says so as well); `cargo audit` and `cargo deny` stand in for it. `docker build` of `crates/hub/Dockerfile` was not run (the Dockerfile and `HARDENING.md` are linted by `hub_dockerfile_is_hardened` and `hardening_md_*` tests, which passed). The three opt-in real-Google-project tests in `gcp_real.rs` are `#[ignore]` and did not run (they need a project to spend on).

## Round 1 review: what changed

- **Blocking (lockfile ages).** `prodash 31.0.1`, `quinn-proto 0.11.19` and `quinn-udp 0.5.16` were younger than 14 days. Re-checked on crates.io on 2026-10-11 (31.0.1: 2026-10-01; 0.11.19 and 0.5.16: 2026-09-30). Downgraded with `cargo update -p ... --precise` to `prodash 31.0.0` (2026-01-08, 276 days), `quinn-proto 0.11.18` (2026-09-14, 27 days) and `quinn-udp 0.5.15` (2026-07-04, 99 days), in the workspace `Cargo.lock` and in `crates/hub-platform/fuzz/Cargo.lock`, which held the same three young pins (the reviewer's list named only the main lockfile). The lockfile diff is only those three versions and their checksums, plus `windows-sys` (0.52.0 and 0.48.0) and `getrandom 0.3.4` selections that older releases need and that the lockfile already held for other crates; nothing was added.
- **New dependency-age check (S19, AGENTS.md rule 15).** Every registry entry that the branch adds relative to the base branch `claude/run-prompt-03b-m6fany` (99 in the main lockfile; the fuzz lockfile adds `arbitrary 1.4.2`, 422 days, and `libfuzzer-sys 0.4.13`, 128 days, beyond that) was looked up on crates.io by version. Result: none younger than 14 days. The youngest are `rustls-platform-verifier 0.7.1` and `rustls-platform-verifier-android 0.2.0` (16 days, 2026-09-24), `faster-hex 0.10.1` and `tracing-opentelemetry 0.34.0` (17 days), and `opentelemetry 0.33.0` (22 days). `cargo deny` and `cargo audit` do not enforce age, so this check is manual; the script is not committed.
- **Replay guard (reviewer non-blocking 5, fixed).** The guard keyed on the unsigned `X-GitHub-Delivery` header, so a captured signed body could be replayed under a changed id. It now keys on the HMAC-SHA256 of the verified body (`hmac-sha256:<hex>`, 76 characters, inside the column's 128-character limit, so no migration). GitHub's redelivery has the same body and so the same key, and the T11 rule is unchanged: a failed `pull_request` update forgets that key and answers 503, and the redelivery is applied (`pr_state_change_audit_is_in_the_same_transaction` still passes). The header is still required. New tests, red before the change and green after: `webhook_replay_with_a_changed_delivery_header_is_still_a_replay` (push) and an added case in `pull_request_webhook_uses_the_same_hmac_and_replay_guard` (a pull request, with its state and audit rows unchanged). `webhook_signature_compare_is_constant_time` scans the source of the function that compares; the comparison now lives in `digest_matches`, so the scan reads that function (and `verify_signature`, and checks the request path calls it): the same prohibitions, applied to where the code is. `docs/threat-model.md` now says what the guard covers and states its limits (pruning after seven days; two byte-identical signed bodies count as one delivery). Behaviour change to note: two distinct genuine deliveries with a byte-identical body would be treated as one; GitHub bodies carry commit ids and timestamps, so this is not expected, and a dropped push is covered by the 60 s poll.
- **`crates/hub-platform/fuzz/.gitignore`** added (`target/`, `artifacts/`, `coverage/`, as in `crates/agent/fuzz`); the untracked `crates/hub-platform/fuzz/target/` (2.2 GB, rebuilt by the fuzz recipe) is deleted again after the run.
- **`Cargo.toml` comment** near the `gix` pin now says ring is absent from `hub-platform`'s graph and present in the `hub` binary through sqlx's database-leg TLS (A15). `docs/decisions.md` was not touched (not an owned path).

## Gates

| Command (run by hand, in recipe order) | Result | Output (tail) |
|---|---|---|
| `platform-tools-check`: `docker info`, `cargo deny --version`, `cargo audit --version` | pass | docker 29.8.2 reachable; `cargo-deny 0.19.9`, `cargo-audit-audit 0.22.2` (the pinned versions) |
| `platform-lint`: `cargo fmt --all -- --check` | pass | exit 0 |
| `platform-lint`: `cargo clippy -p hub-platform -p hub --all-targets --all-features -- -D warnings` | pass | `Finished dev profile`, exit 0 |
| `platform-test`: `LK_REQUIRE_DOCKER=1 cargo test -p hub-platform -p hub --all-features` | pass | 66 test binaries, **618 passed, 0 failed, 4 ignored** (3 real-GCP tests; 1 child-process helper `tls_scenarios_child`, run by its parent test). Includes `crates/hub/tests/pg_tls.rs::pg_pool_connects_to_a_real_tls_postgres_and_refuses_the_wrong_ca ... ok` with Docker up and `LK_REQUIRE_DOCKER=1`. |
| `platform-slow-test`: `LK_REQUIRE_DOCKER=1 cargo test -p hub-platform --release --all-features --test events_bus --test gateway_cluster --test git_sync --test lease_race --test outbox -- --test-threads=1` | pass | events_bus 8 passed (31.0 s), gateway_cluster 13 (56.2 s), git_sync 12 (35.8 s), lease_race 9 (34.7 s), outbox 15 (67.4 s); 57 passed, 0 failed |
| `platform-fuzz` | pass | `cargo metadata --locked`, `cargo deny` on the fuzz manifest and `cargo audit --file crates/hub-platform/fuzz/Cargo.lock` ok; then 5 targets x 30 s on `nightly-2026-09-14` with `cargo-fuzz 0.13.2` (1.87M, 1.98M, 1.27M, 2.11M and 0.57M runs), no crash: `platform-fuzz: hub_agent_message hub_forward_request hub_join_csr hub_sentinel_message hub_webhook_payload ran 30 s each without a crash` |
| `platform-bench`: `cargo bench -p agent`, `cargo bench -p hub-platform -p hub`, `cargo xtask bench-check` (target-scale fixtures already in `target/fixtures/target`) | pass | all three exit 0; table below. **`just verify-03b` as a whole: exit 0.** |
| `just verify`, `rust-verify`: `cargo fmt --all -- --check` | pass | exit 0 |
| `just verify`: `cargo clippy --workspace --all-targets -- -D warnings` | pass | `Finished dev profile`, exit 0 |
| `just verify`: `LK_REQUIRE_DOCKER=1 cargo test --workspace` | pass | 166 test binaries, **1622 passed, 0 failed, 6 ignored**. `crates/agent/tests/binary.rs` (see assumption 10) and `kube_watch.rs` (assumption 9), both flaky and not 03b's, passed in this run. |
| `just verify`: `cargo deny check` | pass | `advisories ok, bans ok, licenses ok, sources ok`. Warnings only: 20 `duplicate` (base64, getrandom, hashbrown, rand, syn, winnow and others, all pre-existing or transitive) and one `license-exception-not-encountered` for `libfuzzer-sys` (it is only in the fuzz lockfiles, where it is encountered). |
| `just verify`: `cargo audit` | pass | 1296 advisories loaded, 599 crates scanned, exit 0, no vulnerabilities and no warnings (after the downgrade the lockfile still has 599 crates) |
| `cargo tree -p hub-platform -i ring -e normal` (after the downgrade) | pass | `nothing to print`: no `ring` in `hub-platform`'s graph |
| `cargo audit --file crates/agent/fuzz/Cargo.lock` (extra, 02's lockfile) | pass | 301 crates, exit 0 |
| `just verify`, `web-verify`: `cd web && pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test --run && pnpm build` | pass | 1 test file, 1 test passed; `vite build` ok (`dist/assets/index-*.js` 224.10 kB, 69.65 kB gzip) |
| `cargo xtask bench-check` (also run alone after `just verify`) | exit 0: 14 pass, 0 fail, 23 unmet of 37 | the 23 unmet are budgets of other prompts (`not registered; no benchmark yet`), none belongs to 02 or 03b |

`cargo deny check` and `cargo audit` are clean after the round 1 fix, for the main lockfile and the hub-platform fuzz lockfile (the `platform-fuzz` recipe runs both on the latter). The agent fuzz lockfile was last audited at T11 (301 crates, clean) and is not touched by this round.

### Docker and database housekeeping

A leftover exited `pgvector/pgvector:pg16` container (exit 255 when the disk filled) and 7 unused volumes were removed. After the runs `docker ps -a` is empty. Each Postgres-backed test starts its own testcontainers Postgres (`tests/support/pg.rs`; `LK_TEST_PG_ADMIN_URL`, which would point at a shared server, was not set), and its `lk_t_*` database lives and dies with that container. No throwaway Postgres is left running and no `lk_t_*` database remains.

## Acceptance checklist

- [x] `just verify-03b` passes (proof: the whole recipe exited 0 on the fixed tree in this FIX step, with `nightly-2026-09-14`, `cargo-fuzz 0.13.2`, Docker up and `LK_REQUIRE_DOCKER=1`: 618 tests in `platform-test`, 57 in `platform-slow-test`, five fuzz targets x 30 s, benches and `bench-check`. The reviewer's round 1 re-run of the recipe on `e22419b` also exited 0, and was the first run to prove it).
- [x] P2, P13 and the blob-endpoint budget are within limits in `bench-check` (proof: `cargo xtask bench-check`: `P2 32.638 ms p95 <= 10000`, `P13 6.201 ms p95 <= 1000`, `P7.blob_endpoint 0.007 ms p95 <= 30`, plus `P7.blob_get_cached 0.001 <= 1`, `P11 40.235 <= 10000`, `P11.memory_mib 746.152 <= 1536`, `P15.hub_ack_versions_per_s 20851 >= 1000`).
- [x] The three-instance test for routing passes (proof: `gateway_cluster::three_hubs_agent_moves_during_traffic_no_request_lost`, in the debug run and again in the release `platform-slow-test`).
- [x] The three-instance test for events passes (proof: `three_hubs_event_reaches_every_subscriber_within_1s`, `three_hubs_committed_event_delivered_once_per_subscriber`, `slow_consumer_gets_resync_instead_of_blocking_the_bus`; debug and release runs).
- [x] The three-instance test for leases passes (proof: `three_hubs_race_for_a_lease_exactly_one_holds_it`, `lease_taken_over_after_holder_dies`; debug and release runs).
- [x] The CA key is only ever used through `KmsSigner`; a test fails if any code path loads a private CA key (proof: `ca_key_use` tests, among them `ca_key_is_only_used_through_kms_signer_source_scan`, with the planted-`KeyPair::from_pem` negative case, and the spy-signer and spy-secret-source cases; all pass).
- [x] Webhook verification passes (proof: `webhook_bad_signature_is_401_and_does_nothing`, `webhook_replayed_delivery_is_ignored`, `webhook_replay_with_a_changed_delivery_header_is_still_a_replay`, `webhook_non_push_event_is_ignored`, `webhook_signature_compare_is_constant_time`, `webhook_requires_valid_hmac_over_the_exact_body`, `webhook_payload_fields_are_typed_*`, `pull_request_webhook_uses_the_same_hmac_and_replay_guard`).
- [x] The fuzz targets pass (proof: `platform-fuzz` inside `just verify-03b`, 5 targets x 30 s, no crash, on `nightly-2026-09-14`; the reviewer's round 1 run of the same recipe agrees; the committed-corpus replay `corpus_replay_hub_agent_message`, `_hub_forward_request`, `_hub_sentinel_message`, `_hub_join_csr`, `_hub_webhook_payload` passes on stable).
- [x] T1 verify: header test against every route class, oversized bodies rejected (proof: `headers_on_every_route_class`, `oversized_body_is_rejected_413_before_handler`).
- [x] T2 verify: conformance against the fakes (proof: `secret_manager_source_conformance`, `kms_signer_conformance`, `kms_envelope_wrap_binds_aad`). The opt-in real-KMS test exists in `gcp_real.rs` (ignored, not run here).
- [x] T3 verify: join tests for every credential path; forged, expired and wrong-swimlane certificates rejected (proof: `join_*` tests, `forged_expired_and_wrong_swimlane_certs_are_rejected_{forged,expired,wrong_swimlane}`). Its fuzz-target half is the unticked item above.
- [x] T4 verify: three hub instances, an agent moves during traffic, no request lost without an error result (proof: `three_hubs_agent_moves_during_traffic_no_request_lost`).
- [x] T5 verify: delivery across three instances under 1 s, slow consumer gets `Resync` (proof: `three_hubs_event_reaches_every_subscriber_within_1s`, `slow_consumer_gets_resync_instead_of_blocking_the_bus`; bench `sse_delivery` p95 5.3 ms).
- [x] T6 verify: two instances race, exactly one holds; takeover after expiry (proof: `three_hubs_race_for_a_lease_exactly_one_holds_it`, `lease_taken_over_after_holder_dies`).
- [x] T7 verify: webhook tests, a push reaches `GitHeadMoved` within 10 s, `diff_trees` benchmark (proof: the webhook tests above, `push_reaches_git_head_moved_within_10s_on_every_replica`, `poll_fallback_reaches_git_head_moved_within_75s`, criterion `hub-platform/diff_trees` 3.45 ms at target-scale fixtures; `diff_trees` has no budget row in `perf/budgets.toml`).
- [x] T8 verify: conformance, cached `get` under 1 ms, blob endpoint under 30 ms (proof: `blob_store_conformance`, `blob_endpoint_etag_equals_hash_and_is_immutable`, `blob_endpoint_if_none_match_is_304`, `blob_cache_is_bounded_by_weight`; `P7.blob_get_cached`, `P7.blob_endpoint` PASS).
- [x] T9: platform sections of `docs/threat-model.md` filled (proof: `threat_model` test file passes; the section is in the T9 commit).
- [x] T10 verify: a rolled-back transaction emits no event, a committed one is delivered exactly once per subscriber (proof: `rolled_back_transaction_emits_no_event`, `committed_event_is_delivered_exactly_once_per_subscriber`, `late_commit_with_lower_id_is_not_skipped`, `outbox_prune_deletes_rows_older_than_1h_in_batches`).
- [x] T11 verify: sentinel join with a wrong service account, ack ordering, `pull_request` webhook handling (proof: `sentinel_join_with_wrong_service_account_is_refused`, `ack_is_sent_only_after_delta_commits`, `ack_order_follows_delta_order`, `failed_delta_sends_no_ack`, `pull_request_*` tests).

## Security requirements implemented

Each is proved by the named tests, all of which passed in the runs above (618 tests in `hub-platform` and `hub`).

- **S5** agent identity: `crates/hub-platform/src/join/` (token hashing and single use in `token.rs`, Google ID-token path in `google.rs`, limits in `limits.rs`), `src/ca/` (CA through `KmsSigner`, 24 h certificates, SAN built by the hub). Tests: `join_with_google_id_token_issues_24h_cert_with_swimlane_san`, `join_google_email_must_equal_registered_service_account`, `join_token_is_single_use`, `join_token_expires_after_24h`, `join_token_is_bound_to_one_swimlane`, `join_token_is_stored_and_compared_hashed`, `join_failures_are_indistinguishable`, `csr_san_and_subject_are_ignored`, `sentinel_join_with_wrong_service_account_is_refused`, `sentinel_cert_is_not_accepted_on_agent_service_and_reverse`, `renewal_at_half_lifetime_*`, `connect_hello_swimlane_must_equal_certificate_san`, `connect_requires_hello_first`, `ca_key_is_only_used_through_kms_signer_source_scan`.
- **S6** transport: `src/tls.rs`, `src/listener.rs`, `src/server_identity.rs`, `crates/hub/src/db.rs`. Tests: `agent_listener_refuses_tls12`, `forged_expired_and_wrong_swimlane_certs_are_rejected_*`, `replica_listener_refuses_agent_cert`, `replica_forward_requires_hub_san`, `web_listener_is_tls13_only`, `pg_pool_refuses_a_server_that_offers_no_tls`, `pg_pool_refuses_wrong_ca_and_wrong_host_name`, `pg_pool_connects_to_a_real_tls_postgres_and_refuses_the_wrong_ca` (the plan's `pg_pool_refuses_wrong_ca_and_plaintext` became these three).
- **S7** secrets: `src/gcp/` (Secret Manager, KMS, 5-minute `ttl_cache.rs`, retries). Tests: `secret_manager_source_conformance`, `secret_cache_expires_after_5_minutes_and_is_bounded`, `secret_value_never_appears_in_debug_or_logs`, `workload_identity_token_is_cached_and_never_logged`, `kms_envelope_wrap_binds_aad`, `kms_signer_conformance`, `kms_calls_retry_with_jitter_and_time_out`.
- **S10** data handling: `log_capture_has_no_secret_content_or_env_value`.
- **S11** input validation: `crates/hub/src/edge.rs` limits, `src/git/payload.rs`, `src/blobs/endpoint.rs`, `src/events/sse.rs`, proto size limits. Tests: `oversized_body_is_rejected_413_before_handler`, `json_nesting_depth_is_limited`, `sse_query_bounds_are_enforced`, `blob_hash_must_be_64_hex`, `webhook_payload_fields_are_typed_*`, `proto_decode_is_size_limited`.
- **S12** output safety: `crates/hub/src/edge.rs` headers. Tests: `headers_on_every_route_class`, `csp_nonce_is_fresh_per_response_and_matches_the_html`, `csp_has_no_unsafe_inline_unsafe_eval_or_third_party_origin`, `blob_is_never_rendered_inline`.
- **S16** containers: `crates/hub/Dockerfile`, `HARDENING.md`. Tests: `hub_dockerfile_is_hardened`, `hardening_md_pod_contract_is_complete`.
- **S18** network: `HARDENING.md` network policy, `src/gcp/http.rs` host allowlist, `src/git/webhook.rs` HMAC and HMAC-keyed replay guard. Tests: `hardening_md_network_policy_denies_by_default_and_lists_only_allowed_egress`, `google_http_refuses_hosts_outside_the_allowlist`, `webhook_requires_valid_hmac_over_the_exact_body`, `webhook_replay_with_a_changed_delivery_header_is_still_a_replay`.
- **S21** code: `#![forbid(unsafe_code)]` in both crates; workspace clippy denies `unwrap`/`expect`/`panic` (clippy passed for the workspace); `errors_are_problem_json_without_internals`.
- **S22** fuzzing: five targets and committed corpora; each ran 30 s on the pinned nightly without a crash in `platform-fuzz`, and the corpora replay on stable in `cargo test` (item 2 above).
- Shaping the work: **S4** `platform_route_table_is_guarded_and_matches_openapi`, `sse_and_blob_require_viewer_and_active_user`; **S9** `join_audit_event_is_in_the_same_transaction`, `pr_state_change_audit_is_in_the_same_transaction`; **S19** `cargo deny`, `cargo audit` clean, plus the manual 14-day age check of every added lockfile entry (round 1 fix above).

## Benchmarks against budgets

Full `cargo xtask bench-check` output from the round 1 FIX run (`just verify-03b`, re-run alone after `just verify`; every registered budget has a result). The figures differ from the first version of this file by run-to-run noise on one machine; P2 is the 03b budget that moved most (34.0 to 32.6 ms), P11 startup rose from 24.7 to 40.2 ms, still 0.4 % of its budget:

| P# | Benchmark | Result | Budget | Status |
|---|---|---|---|---|
| P1 | agent (02) | 9599.707 ms p95 | 15000 ms | PASS (64 % of budget; the closest margin of any budget here) |
| P2 | `hub-platform/push_to_drift_webhook` | 32.638 ms p95 | 10000 ms | PASS (measures webhook in to `GitHeadMoved` on another replica over the file transport; drift step and production HTTPS leg not included, plan A3) |
| P3 | agent (02) | 672 bytes/min p95 | 1024 | PASS |
| P4 | agent (02) | 24.734 ms p95 | 20000 ms | PASS |
| P4.stat_walk | agent (02) | 4.114 ms p95 | 1000 ms | PASS |
| P4.cpu_mcores | agent (02) | 4 p95 | 50 | PASS |
| P4.memory_mib | agent (02) | 22.359 max | 64 | PASS |
| P7.blob_get_cached | `hub-platform/blob_get_cached` | 0.001 ms p95 | 1 ms | PASS |
| P7.blob_endpoint | `hub-platform/blob_endpoint` | 0.007 ms p95 | 30 ms | PASS |
| P11 | `hub/startup` | 40.235 ms p95 | 10000 ms | PASS |
| P11.memory_mib | `hub/memory_at_design_scale_mib` | 746.152 MiB max | 1536 MiB | PASS (platform share only; the 512 MiB blob cache is included; prompt 16 repeats it with every crate) |
| P13 | `hub-platform/sse_delivery` | 6.201 ms p95 | 1000 ms | PASS |
| P15.spool_replay_versions_per_s | agent (02) | 282819 min | >= 1000 | PASS |
| P15.hub_ack_versions_per_s | `hub-platform/ack_throughput` | 20851 min | >= 1000 | PASS |

Not budgets, measured for information at the first run (not repeated in the round 1 numbers above): criterion `hub-platform/diff_trees` 3.45 ms, `tree_index_cold` 5.14 ms.

The other 23 rows of the check (P5, P5.per_file_per_swimlane, P6, P7.swimlane_list through P7.feature_status, P8, P9, P10.*, P12.steady_state_query_ms, P14) print `UNMET ... not registered; no benchmark yet`. They belong to prompts 05 to 17 and are `registered = false` in `perf/budgets.toml`; the check exits 0 for them. No real failure.

Caveats on the numbers: the P2, P13, P11 and blob figures come from one machine run, on a hub talking to a local Postgres over loopback, so network latency to a real CloudNativePG, GitHub and Cloud KMS is not in them. P13 did not need the escalation the prompt reserves for NOTIFY.

## New dependencies

Direct additions to `[workspace.dependencies]` (versions are exact pins; ages from the crates.io `created_at` of the pinned version, counted to 2026-10-11):

| Crate or package | Version | Age in days | Why | `cargo deny` / `osv-scanner` |
|---|---|---|---|---|
| axum | 0.8.9 | 180 | the web edge, already in the graph through tonic at this version | deny ok; osv-scanner not run |
| moka | 0.12.16 | 63 | bounded caches: rate-limit buckets, blob cache, tree-index cache | deny ok |
| aws-lc-rs | 1.18.1 | 40 | request ids and CSP nonces from the one crypto provider already used by rustls and rcgen | deny ok |
| opentelemetry, opentelemetry_sdk, opentelemetry-otlp | 0.33.0 | 23 | OTLP trace export for Cloud Trace (T1) | deny ok |
| tracing-opentelemetry | 0.34.0 | 18 | bridge from `tracing` spans | deny ok |
| base64 | 0.22.1 | 894 | Secret Manager and KMS speak base64 in JSON; the version most of the graph already uses | deny ok |
| rustls-native-certs | 0.8.4 | 132 | platform roots for `*.googleapis.com` (not `webpki-roots`, whose licence is not allowed) | deny ok |
| gix | 0.87.1 | 48 | Git mirrors (T7); features `sha1`, `max-performance-safe`, `revision`, `blocking-http-transport-reqwest-rust-tls` | deny ok (needs the `deny.toml` change, A16) |
| sqlx (hub only: feature `tls-rustls-ring-native-roots`) | 0.8.6 (unchanged pin) | 510 | Postgres TLS with full verification (S6, A15) | deny ok |

The lockfile gained 99 new `name@version` entries (the transitive trees of gix, reqwest, the OpenTelemetry crates and their friends; 0 removed). **Round 1 fix:** the first version of this file listed `prodash 31.0.1`, `quinn-proto 0.11.19` and `quinn-udp 0.5.16` as younger than 14 days and left them; the reviewer made it blocking and they are now `prodash 31.0.0`, `quinn-proto 0.11.18` and `quinn-udp 0.5.15`, all older than 14 days (see "Round 1 review" above). After the fix, no entry the branch adds to either lockfile is younger than 14 days; the youngest is 16 days. The age check is manual (crates.io `created_at` per name and version); `cargo deny` and `cargo audit` do not enforce it.

## Assumptions

Known items for the PR text. A15, A16 and A19 refer to the plan's question ids.

1. **A15, sqlx and ring.** The hub binary enables `tls-rustls-ring-native-roots` for the database leg only, because every sqlx variant on aws-lc-rs failed `cargo deny` or pulled `webpki-roots`. Consequence: `ring 0.17.14` is compiled into the `hub` binary (`cargo tree -p hub -i ring`), so rustls has two providers compiled in there. The hub's own listeners, the CA and rcgen use aws-lc-rs. `hub-platform` alone has no ring. Not proved by a test: which provider the `gix`/reqwest leg picks when both are present (the HTTPS mirror test `git_mirror_http` passes, so it does work).
2. **A16, gix.** `gix` with `blocking-http-transport-reqwest-rust-tls` brings `reqwest 0.13.5` as a transitive dependency only; there is no direct `reqwest` use. Human decision recorded in `plans/03b-hub-platform.blockers.md`: `deny.toml` `[graph].targets` is narrowed to `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`. Side effect: the Windows, macOS, Android and wasm32 dependency graphs are no longer checked by `cargo deny` for any crate.
3. **A19, webhook body cap.** 1 MiB, the number in `api/openapi.yaml`, not the 4 MiB of the plan's proposed default. A push webhook over 1 MiB gets 413 and the 60 s poll picks the change up.
4. **`BlobPolicy::allowed` is `async`** (the real policy lives in 05's registry and has to read the database).
5. **Sentinel join is audited as `AuditAction::AgentJoined`** (no sentinel-specific action exists in `ports::audit::AuditAction`).
6. **Contract follow-up on `ports::conformance::event_bus`.** The suite assumes `publish` is visible to a subscriber synchronously; with the outbox relay (D80) delivery is asynchronous, so `crates/hub-platform/tests/outbox.rs` runs the suite through a `Settled` wrapper that waits for the relay. The suite should grow an async-settle hook in a `contract-change` PR.
7. **The hub binary still refuses to start** without the real 03a (identity) and 05 (registry) modules (`crates/hub/src/main.rs`). It does not yet wire the replica listener, the agent listener, the job runner or the webhook `PrLinks` implementation; those are built and tested in `hub-platform` but are composed when 03a and 05 land. Everything measured above that involves "three hubs" composes the pieces in test code.
8. **`web/index.html` has no `__CSP_NONCE__` placeholder.** The hub injects the per-response nonce at that placeholder (A14); prompt 08 must add it, or served pages will carry a nonce that no tag uses.
9. **CI-flaky agent test, not 03b's:** `crates/agent/tests/kube_watch.rs no_full_report_while_the_lists_are_refused_and_the_log_has_only_the_status` fails intermittently in CI. It passed in the local workspace run above.
10. **Second flaky agent test, not 03b's:** `crates/agent/tests/binary.rs:72` (`binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm`) failed once in the reviewer's first `just verify` (exit status `None`: SIGTERM probably arrived before the handler was installed) and passed on rerun; in the reviewer's isolated reruns it failed 1 of 3. It passed in my `just verify` run. It belongs to prompt 02 and needs a follow-up there.

**Open items for a human, unchanged by the round 1 fix (not fixed on purpose; my reading of each):**

- **A19, webhook cap 1 MiB vs the plan's 4 MiB.** A decision: 1 MiB is the contract's number and stricter than S11. Needs a human nod.
- **`api/openapi.yaml` lists 400, 403 and 413 for the webhook; the code answers a bare 401 for a bad signature** (as the plan says), which the contract covers only under `default`. A real mismatch between a contract file and the code, not a decision; the contract is a human's to change.
- **`docs/decisions.md` has no note for A15 (two TLS providers in the hub binary) and A16 (narrowed `deny.toml` targets).** Not an owned path. Needs a human or a docs PR.
- **No no-op `Notifier` exists** although the prompt's Interfaces row says 03b implements it. A gap against the prompt, not a decision; nothing consumes it yet.
- **The limits layer (`crates/hub/src/limits.rs`) buffers up to 4 MiB of any `application/json` body and scans its depth before the webhook's own 1 MiB check or its HMAC.** A real, bounded weakness: an unauthenticated sender can make the hub buffer 4 MiB per request, limited by the per-IP bucket and the 30 s timeout. Worth a follow-up (exempt `/hooks/github` or pass it the 1 MiB cap).
- **Relay residuals (reviewer NB2):** the gap read happens only on the first loop iteration, so with a backlog above one batch and a concurrent publisher an order inversion is still possible (no event is lost; the next poll delivers it), and the regression test `events_bus.rs:111` is probabilistic. A real but narrow defect and a test-quality point.
- **Smaller points from the review (item 12):** a batch the sentinel sink rejects as `Invalid` or `TooLarge` gets no ack and no stream end (07's side); the contract-route question (the plan proposed a separate `contract:` PR for `AuditAction::PrStateChanged` and the budget entries, but they landed on this branch under the approved `Contract-change`) needs the human to confirm the merge route.


## Deviations from the plan

All are recorded in `plans/03b-hub-platform.blockers.md` or listed above (assumption numbers):

- `deny.toml` `[graph].targets` narrowed (human-approved, A16): beyond the plan's "one `aws-lc-sys` exception" description of `Extra-paths: deny.toml`.
- sqlx on ring for the database leg (A15, assumption 1).
- Webhook cap 1 MiB, not 4 MiB (A19, assumption 3).
- `pg_pool_refuses_wrong_ca_and_plaintext` was split into three named tests (S6 section).
- The replay guard keys on the HMAC of the body, not on the delivery header as the plan's wording and the first threat-model text said (reviewer non-blocking 5; behaviour and wording corrected in round 1). `webhook_deliveries.delivery_id` therefore holds `hmac-sha256:<hex>` keys, not GitHub delivery ids; no migration was needed.
