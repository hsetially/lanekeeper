# Review 02: Agent (round 1)

Reviewer agent, read-only, per `prompts/REVIEW.md`. Branch `agent/02-agent`, HEAD `f351056`, base `origin/main`.
This file keeps the verdict, the findings and the commands. The traces are summarised.

**VERDICT: APPROVE**

## Blocking

None.

## Non-blocking

1. `Cargo.lock`, commit 170c4a1: `smallvec` goes from 1.16.1 to 1.15.2, and the evidence guessed the age rule was why. It is not.
   - 1.16.1 was published 2026-09-11 and is 29 days old.
   - The cause is `kube-client 4.0.0`, which pulls `serde-saphyr 0.0.27` and `granit-parser 0.0.3`. Both declare `smallvec >= 1.0.0, < 1.16.0`.
   - The downgrade is forced and justified, and `cargo audit` is clean on 1.15.2. The reason belongs in the PR text, because the T2 commit message omits it.
2. `plans/02-agent.md` was edited after approval, by f0b55a0 (Extra-paths for the budget registry).
   - The commit message says it was human-directed, and the diff is exactly the two paths plus the `docs/performance.md` sentence.
   - The reviewer cannot verify the human's request from the repo, so the human should re-confirm at merge.
   - The `02/fix:` and `02/blocked:` subjects are outside the `NN/Tk:` form. That is harmless, because `lk status` still counts 12 `02/T` commits.
3. Once this merges, `cargo xtask bench-check` (and so `just verify-01`'s `perf-verify`) fails on a fresh checkout until `cargo bench -p agent` has run, because seven budgets are now registered.
   - `just bench` is `cargo bench --workspace`, and the agent benches panic if `target/fixtures/target` is missing.
   - CI (`ci.yml`) does not run benches, so CI stays green. A26 accepted this. State it in the PR text.
4. The plan promised three insta goldens (Merkle encoding, `ClusterReport` projection, notify request). Only `merkle_encoding_golden` exists.
   - The other two are covered by exact-byte assertions: `notify_posts_form_header_and_relative_paths` asserts the full body, and `watch_driven_cluster_report` asserts the fields.
   - The prompt does not require goldens, so this is only a deviation from the plan.
5. `docs/threat-model.md`, the "hub settings burn CPU" row says the hub "can slow the agent, never speed it past the budgets". `Tunables` clamps `scan_interval` to 5 to 300 s, so a hub can halve the 10 s default. The wording should be fixed; the budgets still hold.
6. The nightly build of the fuzz crate (nightly-2026-09-14) warns `unreachable_code` on `match never {}` after an `Infallible` await, at `src/scan.rs:351`, `src/app.rs:357`, `:370` and `:379`. Stable 1.88 is clean, but a toolchain bump under `-D warnings` will fail.
7. `crates/agent/Dockerfile` has never been built, because there is no Docker daemon. The reviewer resolved both base-image digests and they match (distroless at gcr.io; the rust image via mirror.gcr.io, because Docker Hub rate-limited). Risks CI must show: `aws-lc-sys` compiling in the builder, and the `rust-toolchain.toml` components (`rustfmt`, `clippy`) being fetched in the image.
8. The Merkle byte encoding must be relayed to prompt 05, and `docs/domain-model.md` needs a line (plan: "needs relaying"). The PR text must carry it.
9. P1, P3, P4 and P15 numbers are local-disk and in-process (A15). The `agent-fuzz` recipe is not in CI or `just fuzz-smoke` (A16). Both are stated in the evidence and the threat model.

## Scope and spec chain

- Contract paths (`proto`, `crates/domain`, `crates/ports`, `crates/proto`, `db`, `api`, `docs/mcp-tools.md`, `design`): the diff against `origin/main` is empty, in both three-dot and two-dot form. `deny.toml` is untouched.
- Files outside `crates/agent/`: `Cargo.toml`, `Cargo.lock`, `Justfile`, `crates/xtask/tests/budgets_registry.rs`, `docs/decisions.md`, `docs/performance.md`, `docs/threat-model.md`, `perf/budgets.toml` and `plans/02-agent.*`. All are on the plan's list or are shared root files.
- `perf/budgets.toml`: six `registered` flips plus the one `P4.stat_walk` block, and no thresholds changed.
- `docs/decisions.md`: a single appended D89 line, word for word the plan's wording. `docs/performance.md`: the one sentence added to P4.
- The approval commit 050a57a exists. There are 12 `02/T1` to `T12` commits in order, each one task.
- All 134 test, bench and fuzz identifiers in the traceability table exist as `fn`s. About 15 were read: they assert real behaviour and none is vacuous.
- The T9 rewrite of `what_changed_while_disconnected_*` is legitimate. It asserts more (two spooled entries, replay in order, heartbeat root still differs, `RequestDelta` still answered) and weakens nothing.
- `budgets_registry.rs`: `nothing_is_registered_...` became `only_budgets_with_a_harness_are_registered`, an explicit list of exactly the seven ids. That is stricter, and the human approved the path.

## Dependencies (rule 15)

- All 99 new registry versions in `Cargo.lock` were looked up on crates.io. The youngest is `pest*` 2.9.2 at 18 days (2.9.3 is 1 day old and is held back). None is yanked.
- The fuzz crate's lock adds only `libfuzzer-sys` 0.4.13 and `arbitrary` 1.4.2.
- `kube` is 4.0.0 (4.2.0 needs rustc 1.89). `cargo tree -i ring -p agent` prints nothing, so `ring` is not in the agent graph.

## Security trace (S# to implementation to named test)

- **S5:** `identity/joiner.rs`, `cert.rs`, `key.rs`, `store.rs` -> `join_token_fallback_only_without_workload_identity`, `join_mode_token_never_calls_metadata_server`, `issued_cert_san_must_be_swimlane_form`, `rejects_issued_cert_with_wrong_key_san_or_lifetime`, `private_key_only_written_to_cert_secret`, `identity_debug_is_redacted`, `tokens_never_in_logs`.
- **S6:** `transport/tls.rs` (TLS 1.3 only, pinned CA only, resumption off, no client cert on Join) -> `client_config_is_tls13_only`, `tls12_only_server_is_refused`, `server_cert_from_other_ca_is_refused`, `stream_uses_zstd`.
- **S10:** `ops/logging.rs` -> `log_capture_contains_only_paths_and_hashes`, `metrics_labels_are_low_cardinality`, `kube_status_messages_never_reach_the_log`.
- **S11:** `dispatch.rs`, `fileops.rs` (hash compared with `ct_eq`, 2 MiB cap, no parent creation) -> `hostile_command_is_answered_denied_before_io`, `hash_mismatch_leaves_file_untouched`, `crlf_and_bom_roundtrip_byte_exact`, `write_does_not_create_parent_directories`.
- **S16:** `Dockerfile`, `HARDENING.md` -> `dockerfile_is_distroless_nonroot_pinned`, `hardening_contract_is_s16_complete`.
- **S17:** no shell or exec (`no_shell_or_exec_in_source`); cap-std (`root.rs`, `tree/walk.rs`, `fileops::locate`: lstat, open, same dev and inode) -> `all_file_access_via_cap_std`, `symlinked_file_refused`, `symlinked_dir_refused`, `traversal_corpus_never_escapes_root`; RBAC (`rbac_manifest_has_no_secret_list`, `rbac_lint_rejects_planted_secret_list`, `rbac_secret_rules_are_named_and_minimal`); the deny list shared by walker, delta builder, `FileOps`, spool and `FetchServed` (`denied_file_bytes_never_on_any_outbound_message_spool_or_log`, which plants markers and has a self-test); the config-server client (`fetch_served_url_cannot_escape`, `fetch_served_rate_limit_5_per_s`, `config_server_calls_only_from_the_dispatcher`); the env allowlist (`env_values_only_for_allowlist`, `secret_named_env_never_reported`).
- **S21:** `forbid_unsafe_present`, `error_replies_carry_codes_only`; clippy denies `unwrap`, `expect` and `panic`.
- **S22:** six fuzz targets, all 6 x 30 s clean in `agent-fuzz`; seed replay on stable in `tests/corpus_replay.rs`. Spool record parsing bounds the length by `MAX_PAYLOAD` before any allocation, checks CRC32 and validates metadata.
- Threat model: `## Agent (02)` has all six STRIDE letters, and `tests/threat_model.rs` checks that every Proof cell names an existing test. Spot checks against the code held (4 parallel commands, the 11-minute scanner stall, `valueFrom` never read). The residual risks are honest, including the FIFO swap, the NFS compare-and-swap gap, deny-by-name, and pre-denial bytes in an unacked spool segment.

## P# trace (benchmark to `bench-check` row, re-run)

| P# | Benchmark | Result | Budget |
|---|---|---|---|
| P1 | `agent/oob_change_visible` (real 10/3/30 s timings) | PASS 9587.5 ms | <= 15000 |
| P3 | `agent/idle_traffic_bytes_per_min` | PASS 672 | <= 1024 |
| P4 | `agent/full_rehash_2000_files` | PASS 6.8 ms | <= 20000 |
| P4.stat_walk | `agent/stat_walk_2000_files` | PASS 2.9 ms | <= 1000 |
| P4.cpu_mcores | `agent/steady_state_cpu_mcores` | PASS 4 | <= 50 |
| P4.memory_mib | `agent/steady_state_memory_mib` | PASS 24.4 | <= 64 |
| P15 | `agent/spool_replay_versions_per_s` | PASS 393913/s | >= 1000 |

## Checklist

- Scope, plan and commits; contract-path diff empty; dependencies (age, deny, audit, no `ring`); security trace; performance trace; tests behavioural and not weakened; threat model updated; open questions at defaults and configurable (Q3, Q10, Q11, Q19, Q26, Q37): **pass**.
- `docker build`, `osv-scanner`, `just verify-01` (needs Docker), `just fuzz-smoke` and a real NFS mount test: **not run**.

## Commands run by the reviewer

`cargo fmt --all -- --check` (exit 0); `cargo clippy --workspace --all-targets -- -D warnings` (exit 0); `cargo test -p agent --lib --tests` (54 binaries, 742 passed, 0 failed, 1 ignored); `cargo deny check` (ok); `cargo audit` (1296 advisories, 500 crates, no findings); `just verify` (exit 0, 1003 passed, 0 failed, 2 ignored); `just verify-02` (exit 0, 2196 s, includes `p1_change_visible_real_timings` ok in 123.78 s and 6 fuzz targets x 30 s); `cargo xtask bench-check` (exit 0, 7 pass, 0 fail, 27 unmet); `cargo tree -i ring -p agent` ("nothing to print"); crates.io age lookup of every new lock version (youngest 18 days). `git status` was clean after all runs.
