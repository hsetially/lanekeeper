# Evidence 02: Agent

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proved. Branch `agent/02-agent`, HEAD before this file `400cbc1` plus the merge of origin/main (contract PR) at `8f850c7`. Environment: Linux x86_64, rustc 1.88.0 (stable) for the workspace, `nightly-2026-09-14` and cargo-fuzz 0.13.2 for the fuzz crate, cargo-deny 0.19.9, cargo-audit 0.22.2. All commands ran with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`. No `02/fix:` commit was needed in the FINAL step: every gate passed on the first run.

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-02` | PASS, exit 0, wall time 35 min 47 s | See "verify-02 in detail" below. Last lines: `agent-fuzz: agent_path agent_hub_message agent_cert_chain agent_pem agent_id_token agent_spool_record ran 30 s each without a crash`, then `bench-check (default): 7 pass, 0 fail, 27 unmet of 34 budgets` |
| `just verify` | PASS, exit 0, wall time 3 min 42 s | See "verify in detail" below. Last lines: `vite v6.4.3 building for production ... dist/assets/index-O9gXnvRs.js 224.10 kB \| gzip: 69.65 kB ... built in 1.54s` |
| `cargo xtask bench-check` | PASS, exit 0 (re-run on its own after `just verify`) | `bench-check (default): 7 pass, 0 fail, 27 unmet of 34 budgets` (rows in "Benchmarks against budgets") |

The 27 UNMET rows are the budgets of other prompts (`registered = false`, "no benchmark yet"); none of them is a prompt 02 budget. Every budget this prompt registered shows PASS.

### verify-02 in detail (sub-recipes, in order)

1. `agent-tools-check`: cargo-deny 0.19.9, cargo-audit 0.22.2, nightly-2026-09-14, cargo-fuzz 0.13.2 and a C++ compiler present. PASS.
2. `agent-fixtures`: target-scale fixtures already present at `target/fixtures/target` (`manifest.json` dated before this run), so not regenerated. PASS.
3. `agent-lint`: `cargo fmt --all -- --check` and `cargo clippy -p agent --all-targets -- -D warnings`, no warnings. PASS.
4. `agent-test`: `cargo test -p agent --lib --tests`, 54 test binaries, **742 passed, 0 failed, 1 ignored** (the ignored one is `p1_change_visible_real_timings`, which step 5 runs).
5. `agent-slow-test`: `cargo test -p agent --release --tests -- --ignored`: `p1_change_visible_real_timings ... ok`, `1 passed; 0 failed`, finished in 123.86 s (real 10 s walk, 3 s quiet period, 30 s maximum deferral, 12 phase offsets).
6. `agent-fuzz`: `cargo +nightly-2026-09-14 metadata --locked` on `crates/agent/fuzz/Cargo.toml` ok; `cargo deny --manifest-path crates/agent/fuzz/Cargo.toml check` printed `advisories ok, bans ok, licenses ok, sources ok` (it also printed eleven `duplicate` warnings, for example `hashbrown` x3, `syn` x2, `getrandom` x2; the repo's `deny.toml` sets duplicates to warn); `cargo audit --file crates/agent/fuzz/Cargo.lock` scanned 301 crates with no findings; then each target ran 30 s without a crash:

   | Target | Runs in 31 s | Final coverage line |
   |---|---|---|
   | `agent_path` | 8,004 | `cov: 5581 ft: 7632 corp: 274/26Kb` |
   | `agent_hub_message` | 35,056 | `cov: 8538 ft: 17510 corp: 607/476Kb` |
   | `agent_cert_chain` | 321,039 | `cov: 2018 ft: 5710 corp: 440/229Kb` |
   | `agent_pem` | 744,703 | `cov: 1479 ft: 3833 corp: 457/182Kb` |
   | `agent_id_token` | 10,630,187 | `cov: 115 ft: 311 corp: 105/11254b` |
   | `agent_spool_record` | 8,384 | `cov: 6762 ft: 12105 corp: 303/87Kb` |

   No file appeared in `crates/agent/fuzz/artifacts/`; `git status` was clean afterwards. The run counts are read from the six `Done N runs` lines in the order the recipe runs the targets.
7. `agent-bench`: `cargo bench -p agent` (six benches: `full_rehash`, `stat_walk`, `idle_traffic`, `oob_change`, `spool_replay`, `steady_state`), then `cargo xtask bench-check` (table below). PASS.

### verify in detail

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 103 test binaries (including 12 doc-test runs), **1003 passed, 0 failed, 2 ignored**. The two ignored: `p1_change_visible_real_timings` (run by `agent-slow-test`, above) and `target_scale_counts_match_design_scale` (80,000 files; prompt 01's `fixtures-verify`, not run here). The merged contract tests ran and passed: `scan_delta_with_gap_roundtrips`, `gap_ending_before_it_starts_is_rejected`, `notify_result_roundtrips`, `notify_status_must_be_an_http_status`, and the ports conformance case `gateway_passes_a_notify_reply_through` through `fake_agent_gateway_conforms`.
- `cargo deny check`: `advisories ok, bans ok, licenses ok, sources ok`.
- `cargo audit`: loaded 1296 advisories, scanned `Cargo.lock` (500 crate dependencies), no findings, exit 0.
- `web-verify`: `pnpm install --frozen-lockfile` ("Lockfile is up to date", done in 10.6 s), `pnpm lint` (eslint, no output), `pnpm typecheck`, `pnpm test --run` (1 file, 1 test passed), `pnpm build` (vite, built in 1.54 s). npm registry only.

## Acceptance checklist

- [x] `just verify-02` passes, and P1, P3 and P4 are within budget according to `bench-check`. Proof: `just verify-02` exit 0; `cargo xtask bench-check` rows `PASS P1 9590.115 p95 <= 15000 ms`, `PASS P3 672 p95 <= 1024 bytes_per_min`, `PASS P4 6.336 p95 <= 20000 ms`, `PASS P4.stat_walk 2.975 p95 <= 1000 ms`. Caveat (assumption A15, still open for the pilot): the harness runs on local disk with an in-process agent, not on a real NFS mount.
- [x] Idle traffic is under 1 KB per minute, measured with the fake hub over 10 minutes (P3). Proof: `p3_idle_traffic_under_1kb_per_min` (`tests/scan_session.rs`, 10 virtual minutes through a counting duplex, TLS 1.3 ciphertext in both directions, decision A14) passed in `agent-test`; the harness bench `agent/idle_traffic_bytes_per_min` measured `[672, 672, 672, 672, 672, 672, 672, 672, 672, 672]` bytes in ten 1-minute windows, 672 against 1,024.
- [x] Every file operation goes through cap-std, and the traversal and fuzz tests pass. Proof: `all_file_access_via_cap_std` (`tests/source_rules.rs`: source scan, no `std::fs`, `tokio::fs` or `File::open` outside an allow-list of startup readers), `traversal_corpus_never_escapes_root`, `symlinked_file_refused`, `symlinked_dir_refused` (`tests/fileops.rs`), `corpus_replay_agent_path` (`tests/corpus_replay.rs`) all `ok`; fuzz `agent_path` ran 8,004 runs in 30 s with no crash and the outside-root sentinel invariant held (`crates/agent/fuzz/invariants.rs`).
- [x] The RBAC manifest grants no Secret list. Proof: `rbac_manifest_has_no_secret_list`, `rbac_lint_rejects_planted_secret_list`, `rbac_secret_rules_are_named_and_minimal`, `fake_api_log_has_no_configmap_and_only_named_secret_calls` (`tests/kube_rbac.rs`) all `ok`, against the YAML block in `crates/agent/RBAC.md`.

Task-level Verify lines, each shown by a passing named test in the `agent-test` run (test names are in the plan's traceability table and exist at the files listed in "Security requirements implemented" below): T1 `settings_*`, `startup_*`; T2 `join_*`, `renewal_*`, `expired_certificate_recovers_by_rejoin`; T3 `hub_restarted_20_times_agent_recovers_memory_flat`; T4 `merkle_root_independent_of_walk_order`, `any_single_file_change_changes_root`, criterion `full_rehash_2000_files` and `stat_walk_2000_files`, `p1_change_visible_scaled` and the real-timing run; T5 the fileops tests; T6 the `kube_*` tests; T7 `healthz_ok_while_loops_tick`, `readyz_only_while_connected`, `shutdown_flushes_and_exits`; T8 `tests/threat_model.rs`; T9 `spool_*`; T10 `copy_of_600_files_during_job_gives_at_most_two_deltas_all_tagged`, `window_*`; T11 `deny*`; T12 `notify_*`, `fetch_served_*`. Every test name listed in `plans/02-agent.md` was checked to exist as a `fn` in the tree (136 of 147 backticked identifiers in the plan are test or function names; the other 11 are not tests: `bench_check`, `fake_*` support modules, `fuzz_seconds`, `new_root`, `since_root`, `root_squash`, `spawn_blocking`, `unrepresentable_name`, `v1_xx`), and all test functions in the plan's tables were seen `... ok` in either the `agent-test` or the `verify` output, except as noted: `agent_path`, `agent_hub_message` and `agent_spool_record` are fuzz targets, run by `agent-fuzz`; `current_hash`, `fetch_served` and `into_proto` are plain functions, not tests; `gateway_passes_a_notify_reply_through` is a conformance case that runs inside `fake_agent_gateway_conforms` (ok in `just verify`); `p1_change_visible_real_timings` ran in `agent-slow-test`.

## Security requirements implemented

Code locations are under `crates/agent/src/`; tests are named as in the plan and live under `crates/agent/tests/` unless stated.

- **S5** (join, identity, certificate): `identity/` (`joiner.rs`, `idtoken.rs`, `jointoken.rs`, `key.rs`, `csr.rs`, `cert.rs`, `store.rs`, `schedule.rs`); SAN prefix is the single constant `AGENT_SAN_PREFIX`. Tests: `join_with_workload_identity_token`, `join_token_fallback_only_without_workload_identity`, `join_mode_token_never_calls_metadata_server`, `private_key_only_written_to_cert_secret`, `identity_debug_is_redacted`, `tokens_never_in_logs`, `rejects_issued_cert_with_wrong_key_san_or_lifetime`, `issued_cert_san_must_be_swimlane_form`, `agent_san_defined_in_one_constant`, `renewal_at_half_lifetime_over_stream`, `renewal_retry_backoff_then_rejoin_under_ten_percent`, `expired_certificate_recovers_by_rejoin`, `renewed_cert_used_on_next_connect`. Fuzz: `agent_cert_chain`, `agent_pem`, `agent_id_token`.
- **S6** (transport): `transport/tls.rs` (TLS 1.3 only, pinned hub CA), `transport/grpc.rs`, `transport/dial.rs`, `backoff.rs`, `transport/outbox.rs`. Tests: `client_config_is_tls13_only`, `tls12_only_server_is_refused`, `server_cert_from_other_ca_is_refused`, `join_has_no_client_cert_connect_has`, `stream_uses_zstd`, `backoff_full_jitter_capped_at_60s` (`src/backoff.rs`, proptest), `outbound_queue_is_bounded` (`src/transport/outbox.rs`), `hub_restarted_20_times_agent_recovers_memory_flat`, `invalid_hub_message_is_dropped_stream_survives`.
- **S10** (logs: paths, hashes, ids only): `ops/logging.rs` (JSON lines, Kubernetes client targets silenced), `ops/metrics.rs` (closed label sets). Tests: `log_capture_contains_only_paths_and_hashes` (`tests/scan_session.rs`), `metrics_labels_are_low_cardinality` (`tests/ops_metrics.rs`), `tokens_never_in_logs`, `no_env_values_outside_allowlist_in_logs_or_stream`, `denied_file_bytes_never_on_any_outbound_message_spool_or_log`.
- **S11** (typed paths and bounded input before any I/O): `dispatch.rs`, `config.rs` (`Tunables` clamping), `fileops.rs`, domain `NfsPath`. Tests: `hostile_command_is_answered_denied_before_io`, `write_over_2mib_refused`, `tunables_clamp_hub_values` (`src/config.rs`), `unrepresentable_names_are_reported_not_dropped`, `error_replies_carry_codes_only`; fuzz `agent_hub_message`.
- **S16** (hardening): `crates/agent/Dockerfile`, `crates/agent/HARDENING.md`, `ops/`. Tests: `dockerfile_is_distroless_nonroot_pinned`, `hardening_contract_is_s16_complete`, `hardening_contract_egress_is_hub_kube_configserver_metadata_dns_only`, `no_fs_writes_outside_spool_and_tmp`. The rendered-chart policy test is prompt 09's. The Docker build itself was NOT run (see below).
- **S17** (no shell or exec, symlinks, traversal, minimal RBAC, deny globs, env allowlist): `fileops.rs`, `root.rs`, `tree/walk.rs` (own cap-std walker, D89), `kube/` (`watch.rs`, `restart.rs`, `env.rs`, `trim.rs`), `deny.rs`, `configserver.rs`, `RBAC.md`. Tests: `no_shell_or_exec_in_source`, `all_file_access_via_cap_std`, `symlinked_file_refused`, `symlinked_dir_refused`, `traversal_corpus_never_escapes_root`, `symlinks_are_not_followed_and_are_reported`, `rbac_manifest_has_no_secret_list`, `rbac_lint_rejects_planted_secret_list`, `rbac_secret_rules_are_named_and_minimal`, `restart_refused_outside_configured_namespaces`, `fake_api_log_has_no_configmap_and_only_named_secret_calls`, `denied_read_write_delete_refused`, `default_deny_globs_cannot_be_removed`, `hub_deny_globs_are_added_not_replaced`, `denied_hash_streaming_memory_bounded`, `env_values_only_for_allowlist`, `secret_named_env_never_reported`, `env_value_over_256_bytes_or_control_chars_dropped`, `notify_only_on_hub_command`, `fetch_served_url_cannot_escape`.
- **S21** (no unsafe, no panics, no leaked internals): `#![forbid(unsafe_code)]` in `lib.rs` and `main.rs`, workspace clippy lints (denied `unwrap`, `expect`, `panic!` and the rest) in `agent-lint` and `just verify`. Tests: `forbid_unsafe_present`, `error_replies_carry_codes_only`, `identity_debug_is_redacted`, `binary_errors_never_print_environment_values`.
- **S22** (fuzz every parser at a trust boundary): six targets in `crates/agent/fuzz/fuzz_targets/` (`agent_path`, `agent_hub_message`, `agent_cert_chain`, `agent_pem`, `agent_id_token`, `agent_spool_record`), each with a corpus, replayed on stable by `corpus_replay_*` in `tests/corpus_replay.rs`, and run 30 s each by `agent-fuzz` (table above). `tests/justfile_gate.rs` fails if the Justfile target list and `fuzz_targets/` drift apart. Note: `just fuzz-smoke` (prompt 01) does not run this crate (decision A16); prompt 15 or 09 must add `agent-fuzz` to CI.

Threat model: `docs/threat-model.md` section `## Agent (02)` (125 lines, STRIDE table, residual risks); `tests/threat_model.rs` checks the six STRIDE letters and the S# citations.

## Benchmarks against budgets

Source: `cargo xtask bench-check` (default scale) after `cargo bench -p agent`, fixtures from `cargo xtask gen-fixtures --scale target`, swimlane 1 (2,003 files), local disk, release build.

| P# | Benchmark | Result | Budget |
|---|---|---|---|
| P1 | `agent/oob_change_visible` (12 change instants, real 10 s walk, 3 s quiet, 30 s deferral; samples ms: 9590, 8758, 7923, 7088, 6255, 5424, 4590, 3755, 3922, 3090, 3260, 3426) | PASS 9590.115 ms p95 | <= 15000 ms |
| P3 | `agent/idle_traffic_bytes_per_min` (ten 1-minute windows, TLS 1.3 ciphertext both directions) | PASS 672 bytes p95 | <= 1024 bytes/min |
| P4 | `agent/full_rehash_2000_files` (criterion; `time: [5.9609 ms 6.0704 ms 6.1862 ms]`) | PASS 6.336 ms p95 | <= 20000 ms |
| P4.stat_walk | `agent/stat_walk_2000_files` (criterion; `time: [2.3762 ms 2.4287 ms 2.4830 ms]`) | PASS 2.975 ms p95 | <= 1000 ms |
| P4.cpu_mcores | `agent/steady_state_cpu_mcores` (24 windows of 5 s, 40 Deployments, 80 Pods, 2,003 files) | PASS 4 mCPU p95 | <= 50 mCPU |
| P4.memory_mib | `agent/steady_state_memory_mib` (11.8 MiB growth plus 11.0 MiB idle binary) | PASS 22.766 MiB max | <= 64 MiB |
| P15 | `agent/spool_replay_versions_per_s` (5 runs of 18,000 versions: 423357, 382516, 433197, 401445, 410826) | PASS 382516.156 versions/s min | >= 1000 versions/s |

Caveats, stated rather than hidden: (1) all numbers are local disk, in-process, no NFS latency (assumption A15); the stat walk over a real NFS export may exceed 1 s and needs pilot numbers. (2) The P1 samples show the walk interval is the dominant term; the worst sample is 9.6 s of the 15 s budget. (3) The P15 harness replays from a local volume without the one-hour virtual outage; the outage and the A to B to C order are proved by `spool_replays_a_b_c_in_order_after_one_hour_outage` in virtual time.

The fresh-checkout concern: `perf/budgets.toml` registers seven prompt 02 rows; a checkout that has not run `cargo bench -p agent` shows them as failing `bench-check`, by design ("a registered budget without a result fails").

## New dependencies

All version pins are exact (`=x.y.z`) with `default-features = false` in `[workspace.dependencies]`. Age is days from the crates.io `created_at` of that exact version to 2026-10-10 (read from `https://crates.io/api/v1/crates/<name>/<version>` for every one of the 99 package versions that `Cargo.lock` gains against origin/main). Result: the youngest new package in the lockfile is `pest*` 2.9.2 at 19 days; none is under 14 days.

| Crate or package | Version | Age in days | Why | `cargo deny` / `cargo audit` |
|---|---|---|---|---|
| `cap-std` | 4.0.3 | 51 | Every agent file access goes through a capability `Dir` (S17) | clean |
| `globset` | 0.4.20 | 67 | Ignore, deny, sync-Job and Helm-hint globs (decision A1, D89); the `ignore` crate is not a dependency | clean |
| `rcgen` | 0.14.10 | 43 | P-256 key and CSR on aws-lc-rs, `zeroize` (S5) | clean (aws-lc-sys needed no `deny.toml` exception at these versions) |
| `x509-parser` | 0.18.1 | 247 | Check the issued certificate before storing it (S5); `verify` features off | clean |
| `pem` | 3.0.6 | 365 | Read and write the certificate Secret | clean |
| `kube` (+ `kube-client`, `kube-core`, `kube-runtime`) | 4.0.0 | 116 | Kubernetes client, watchers and reflectors (T2, T6) with `rustls-tls` and `aws-lc-rs`; 4.2.0 needs rustc 1.89 and the toolchain is pinned to 1.88 | clean |
| `k8s-openapi` | 0.28.0 | 117 | Kubernetes types, feature `v1_35` (a guess at the GKE minor, see assumptions) | clean |
| `crc32fast` | 1.5.0 | 454 | Spool record checksums (T9) | clean |
| `prometheus-client` | 0.25.1 | 39 | `/metrics` text format (T7) | clean |
| `tracing-subscriber` (+ `tracing-serde` 0.2.0) | 0.3.23 | 211 | JSON log lines (T7); no `env-filter` (avoids a regex engine) | clean |
| `rayon` (+ `rayon-core` 1.13.0) | 1.12.0 | 179 | Dedicated hashing and walking pool, never the global pool | clean |
| `criterion` (dev) | 0.8.2 | 248 | Benchmarks (rule 14); no HTML report, no global rayon feature | clean |
| `serde-saphyr` (dev) | 0.0.27 | 137 | RBAC and HARDENING YAML lint in tests | clean |
| Already in the lockfile on origin/main, now used by the agent (no new package version): `hyper` 1.11.1, `http` 1.5.0, `http-body` 1.1.0, `http-body-util` 0.1.5, `fastrand` 2.5.0, `time` 0.3.55, `rustls` 0.23.45, `tokio-rustls` 0.26.5, `tempfile` 3.27.0 | as stated | not re-measured | plain-HTTP clients and server (no `reqwest`, which would add a second TLS stack), jitter, own rustls config so TLS 1.3 is the only version | clean |

`cargo tree -i ring -p agent` prints `warning: nothing to print`: `ring` is not in the agent's dependency graph (it remains in the workspace lockfile through other crates). `cargo deny check` and `cargo audit` are clean on `Cargo.lock` (500 crates) and on the fuzz crate's own lockfile (301 crates). `osv-scanner` was NOT run (not installed).

Lockfile notes: `pest`, `pest_derive`, `pest_generator`, `pest_meta` are held at 2.9.2 (2.9.3 was one day old when T2 resolved); `smallvec` was lowered from 1.16.1 (origin/main) to 1.15.2 by commit `170c4a1` (T2); that commit message does not mention it. Prompt 01's evidence says transitive crates younger than 14 days were lowered with `cargo update --precise` (`smallvec` among them), so the likely reason is the age rule, but I did not confirm it and I did not look up the age of 1.16.1. I flag it for the reviewer: it is the only package whose version goes down in `Cargo.lock`, and `cargo deny` and `cargo audit` are clean with it.

## Assumptions and decisions

The plan's decisions A1 to A27 govern (`plans/02-agent.md`, "Ambiguities and open questions"). Decided by a human: A1 own cap-std walker (recorded as D89), A2 RBAC `get` on exactly two named Secrets, A4 new `NotifyResult` message, A5 certificate SAN `spiffe://lanekeeper/swimlane/<id>`, A12 `WriteFile` never creates parent directories, A14 P3 counts TLS ciphertext in both directions, A16 own fuzz crate `crates/agent/fuzz/`, A17 `crates/agent` owns the Dockerfile and `HARDENING.md`, A25 contract edits in their own PR, A26 budget registration stays in this PR (and the human's later direction to add `crates/xtask/tests/budgets_registry.rs` and `docs/performance.md` to the Extra-paths, which unblocked blocker B1: commit `f0b55a0`, fix `bdd420e`). Defaults taken without a human answer: A3 `SpoolGap`, A6 P1 timing scheme, A7 spool dedup only of adjacent equal versions, A8 per-message exact `Ack`, A9 ignore globs `.nfs*` and `.lanekeeper-tmp-*`, A10 the `LK_*` configuration list, A11 the Merkle byte encoding (pinned by `merkle_encoding_golden`, to be relayed to prompt 05), A13 tree leaves for skipped, denied and symlink files, A15 local-disk harness fidelity, A18 extra fuzz targets, A19 never report values for secret-looking names or values over 256 bytes or with control characters, A20 sync-Job and Helm-hint detection by name globs and `helm.sh/chart` labels (a guess until Q3 is answered), A21 window events re-sent in the first full report after reconnect, A22 reconnect after a certificate renewal, A23 a single-use join token cannot self-heal after 24 h, A24 open questions Q1, Q10, Q11, Q19, Q26, Q37.

Assumptions made by the implementers (from their hand-offs):

- **kube 4.0.0, not 4.2.0.** kube 4.2.0 needs rustc 1.89 and `rust-toolchain.toml` pins 1.88; 4.0.0 (2026-06-16) is the newest that builds. `k8s-openapi` 0.28.0.
- **k8s-openapi feature `v1_35` is a guess at the GKE minor version.** It must match the cluster's API minor; confirm on the pilot cluster.
- **`pest*` held at 2.9.2 in `Cargo.lock`** (2.9.3 was one day old); `smallvec` at 1.15.2 (above).
- **Own hyper HTTP/2 client instead of tonic `Channel`** (T3): the agent builds its own rustls config so TLS 1.3 is the only version and the pinned hub CA is the only root; tonic's TLS features are not used. The `HubTransport` trait remains the seam for a later WebSocket transport.
- **The `scan_session.rs` outage test was rewritten in T9** (`what_changed_while_disconnected_is_found_through_the_heartbeat_root` became `what_changed_while_disconnected_is_replayed_and_still_found_through_the_heartbeat_root`) because it asserted pre-spool behaviour: it required `deltas_skipped_offline() >= 2` and no stale delta queued for the new connection. With the spool, changes found during an outage are read and spooled, so the test now asserts `deltas_pushed() >= 2`, two spool entries, and that the spool replays both deltas oldest first after the reconnect while the heartbeat root still differs from the hub's. It is a change of expected behaviour required by T9, not a loosened check; the reviewer should confirm it with `git show e3f9542 -- crates/agent/tests/scan_session.rs`.
- **Non-2xx `FetchServed` bodies are dropped** (T12, `configserver.rs`): the reply carries the status and an empty body ("only a success carries a file; any other body is the server's own page"), so a config-server error page cannot carry content to the hub.
- **Text and binary `Accept` classification** (T12, `configserver::is_binary`): by file extension, with the classes the T12 implementer took from `docs/domain-model.md`: `.yml .yaml .json .properties .xsl .xml .txt` and files without an extension are text (no `Accept` header sent), every other extension is binary and gets `Accept: application/octet-stream`. The prompt says only "binary files use octet-stream", so the extension rule is an assumption.
- **The spool segment keeps pre-denial bytes until acked** (T11): bytes of a file that was spooled before a deny glob made its path denied stay in the segment file until it is acked and purged. They are stripped again when such a record is replayed (`a_record_written_before_the_glob_is_stripped_when_it_is_replayed`), so they are never sent, but they do sit on the spool volume until then. Listed as the one residual way denied bytes can be on that volume in the `## Agent (02)` section of `docs/threat-model.md`.
- **T6 assumptions about terminating pods, init containers and ReplicaSets** (`kube/trim.rs`, `kube/projection.rs`, as read from the code, not from a hand-off text): a Pod being deleted (deletion timestamp set) or in phase `Succeeded` or `Failed` is not kept ("it is not serving"), so it leaves the report rather than showing as terminating; a Deployment's environment variable names and allowlisted values are read from `spec.template.spec.containers` only, so init-container variables are not reported (this affects check C10 on the hub side); no ReplicaSet is watched, and a Pod is matched to its Deployment by the Deployment's label selector (more than 32 selector terms, or an unknown operator, matches nothing) instead of through owner references.
- **Three more fuzz targets than the plan listed** (`agent_cert_chain`, `agent_pem`, `agent_id_token`) were added in the T5 commit (`a1f3444`) beside `agent_path` and `agent_hub_message`, for the certificate, PEM and ID-token parsers; `agent_spool_record` followed in T9. `agent_fuzz_targets` in the Justfile lists all six and `tests/justfile_gate.rs` keeps the list and the directory in step.

## Contract and Extra-path usage

- **`docs/decisions.md`** (named Extra-path): one appended line, **D89**, recording decision A1. D89 was the next free number; the plan's wording was used.
- **`deny.toml`**: listed as an Extra-path but **NOT needed and not changed** (`git diff origin/main...HEAD --name-only -- deny.toml` is empty). `cargo deny check` passes without an `aws-lc-sys` exception at the pinned versions.
- **`perf/budgets.toml`** (contract path, the one contract-change on this branch): differs from origin/main only in `registered` fields (`P1`, `P3`, `P4`, `P4.cpu_mcores`, `P4.memory_mib`, `P15.spool_replay_versions_per_s`: `false` to `true`, six hunks) plus the one new `P4.stat_walk` block (`agent/stat_walk_2000_files`, p95, 1000 ms, target scale, registered). No threshold changed.
- **`crates/xtask/tests/budgets_registry.rs`** (Extra-path added by the human after blocker B1): `nothing_is_registered_before_its_benchmark_exists` is replaced by `only_budgets_with_a_harness_are_registered` (an explicit list of the seven registered ids), `P4.stat_walk` added to the id list and to the `docs/performance.md` comparison table. Both tests pass in `cargo test --workspace`.
- **`docs/performance.md`** (Extra-path): one sentence added to P4, "A stat walk of 2,000 files takes under 1 second." (approved wording from blocker B1, option A).
- **Contract PR note.** The contract additions are not part of this branch's diff; they came in by the merge of origin/main (`8f850c7`): commits `67acb04` (`SpoolGap` on `ScanDelta`), `c2a2717` (`NotifyResult` reply for `NotifyConfigServer`) and `4d6aca4` (certificate SAN wording, S5) are on main. The plan miscounted `ScanDelta` literals: there are five, not four. `crates/domain/tests/dto.rs` was added by the human to the contract PR. `git diff origin/main...HEAD -- proto crates/domain crates/ports crates/proto db api docs/mcp-tools.md` is empty (0 bytes).

## NOT RUN / not provable here

- **`docker build` of `crates/agent/Dockerfile`**: no Docker daemon in this environment. The Dockerfile and `HARDENING.md` are linted by `tests/hardening.rs` only (distroless final image, numeric non-root user, both bases digest-pinned, no shell). The image has never been built here.
- **Base-image digest verification**: the two `@sha256:` digests in the Dockerfile are as recorded by the T7 implementer ("read from the registries on 2026-10-10"); this step did not re-resolve them against the registries. Only the lint (format, presence, pinned) was checked.
- **`db-verify`** and **`verify-01`'s `docker-check`**: need Docker (Postgres 16 with pgvector in testcontainers); not run. `verify-01` as a whole was not run; `just verify-02` and `just verify` were.
- **`osv-scanner`**: not installed; not run. (`cargo deny` and `cargo audit` were run and are clean.)
- **A real NFS mount test**: no `tests/nfs_real_mount.rs` exists and nothing here mounts NFS. Rename atomicity, attribute caching and the two "stop and escalate" conditions of the prompt (NFS behaviour breaking the stated guarantees) are therefore not proved; the 15-minute full rehash is the designed mitigation for attribute caching and is exercised only on local disk (`full_rehash_finds_a_change_with_restored_mtime`).
- **GKE metadata server on a real node (Q26)**: only a fake metadata server was used; Workload Identity ID-token retrieval is not proved on a real node. The token fallback is built and tested against the fake.
- **`just fixtures-verify`** (80,000-file ignored test `target_scale_counts_match_design_scale`): not run; prompt 01's gate.
- **`just fuzz-smoke`** (prompt 01's three domain targets): not run here; not part of `verify-02` or `verify`.
- **P1, P3, P4, P15 on real NFS and real cluster scale**: local-disk, in-process numbers only (A15).
- **Rendered Helm chart policy tests**: prompt 09's.

## Deviations from the plan

- Registration of the budgets could not be done inside the approved paths (blocker B1, `plans/02-agent.blockers.md`, commit `51aeb97`); resolved by the human adding `crates/xtask/tests/budgets_registry.rs` and `docs/performance.md` to the Extra-paths (`f0b55a0`) and applied in `02/fix:` `bdd420e`.
- `deny.toml` was approved as an Extra-path but not needed.
- `agent-fuzz` runs six targets, not the three the plan named (A18's `agent_hub_message` plus three parser targets added in T5).
- The `smallvec` lock downgrade (above) is not in the plan.
