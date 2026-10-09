# Evidence 01: Contracts, skeleton, ports, gates and fixtures

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proven.

Environment: Linux sandbox, no Docker daemon (`/var/run/docker.sock` does not exist). `just`, `cargo-deny 0.19.9`, `cargo-audit 0.22.2`, `buf 1.73.0`, `redocly 2.53.3`, `nightly-2026-09-14` and `cargo-fuzz 0.13.2` were available. Run on 2026-10-09 from the worktree head `ec0642f` (T9); one comment-only `01/fix:` commit followed (see "Gate failures and the fix").

## NOT PROVEN HERE: db-verify

**`db-verify` (every database test: roles, grants, the append-only trigger, TRUNCATE guard, outbox NOTIFY, partitions, pgvector, lz4 TOAST, the HNSW index, the migration applying on Postgres 16) was NOT proven in this environment. It must be run on a Docker host (or in CI) before merge:**

```
LK_REQUIRE_DOCKER=1 cargo test -p xtask --test db_migrations     # i.e. just db-verify
```

Why this matters for reading the other output: `just verify` and `cargo test --workspace` print `test result: ok` for `crates/xtask/tests/db_migrations.rs` (23 passed). That is **not** proof. Without `LK_REQUIRE_DOCKER=1` every test that needs a database prints `SKIPPED` and returns. I confirmed it with `cargo test -p xtask --test db_migrations -- --nocapture`: 17 `SKIPPED` lines at the time of that first check (18 now, with the new retention test), each ending `cannot start the Postgres container: failed to initialize a docker client: Socket not found: /var/run/docker.sock`. The tests that really ran without Docker are only the ones that read text (`every_expected_table_is_created_by_the_migration_text`, `migration_never_creates_roles_or_superuser_objects`, `image_matches_compose`, `pgvector_statements_are_isolated_between_markers`, `missing_docker_*`). Nothing in this file claims a database property as proven.

Related for the human and prompt 09: CI runs `cargo test --workspace` without `LK_REQUIRE_DOCKER=1` (only the `db-verify` recipe sets it), so a CI runner without Docker would also skip silently. The plan (Q7) said to set it in CI too; `.github/workflows/ci.yml` is not owned by 01, so it is not done. Set `LK_REQUIRE_DOCKER=1` in CI.

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-01` | **FAIL, exit 1, at `docker-check` (expected: no Docker daemon).** Not weakened. The recipes before it passed: `tools-check`, `fuzz-tools-check`. The recipes after it (`domain-verify` ... `fuzz-smoke`) did not run in this invocation; they were run one by one (next table). | `pnpm exec buf --version` -> `1.73.0`; `pnpm exec redocly --version` -> `2.53.3`; `verify-01 needs a running Docker daemon: db-verify starts Postgres 16 with pgvector through testcontainers`; `error: recipe 'docker-check' failed on line 48 with exit code 1` |
| `just verify` | **PASS, exit 0** | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (255 passed, 0 failed, 1 ignored; re-run after the round 1 fix; the ignored one is the 80,000-file target-scale test, run in `fixtures-verify`), `cargo deny check` -> `advisories ok, bans ok, licenses ok, sources ok`, `cargo audit` (1296 advisories, 402 crates, no finding), then web: eslint, `tsc --noEmit`, `vitest` (1 passed), `vite build` -> `built in 1.40s`. Last line of the run: `exit=0`. |
| `cargo xtask bench-check` | **exit 0** (see "Benchmarks against budgets") | `bench-check (default): 0 pass, 0 fail, 33 unmet of 33 budgets`; `exit=0` |

`cargo deny check` printed warnings but no error: 12 `duplicate` warnings (`bans.multiple-versions = "warn"`, for example `getrandom`, `hashbrown`, `syn`, `windows-sys`, `winnow`), and 4 `license-not-encountered` / `license-exception-not-encountered` warnings. The last one is the `libfuzzer-sys` NCSA exception, which the root workspace does not contain (it is checked against `fuzz/Cargo.toml` inside `fuzz-smoke`).

### Every sub-recipe of verify-01, run one by one

| Recipe | Exit | What it proved (tail) |
|---|---|---|
| `tools-check` | 0 | cargo-deny 0.19.9 and cargo-audit 0.22.2 match the pins; `pnpm install --frozen-lockfile`; `buf` 1.73.0; `redocly` 2.53.3 |
| `fuzz-tools-check` | 0 | nightly-2026-09-14, cargo-fuzz 0.13.2 and a C compiler present |
| `docker-check` | **1** | no Docker daemon. The only failing recipe, and the reason for `verify-01` stopping. |
| `domain-verify` | 0 | `cargo test -p domain`: nfs_path 14, mapping 13, compare_ref 8, ids 9, enums 6, events 5, dto 14, secret 6, corpus_replay 2, all passed |
| `ports-verify` | 0 | `cargo test -p ports --all-features` (contract_shape 12, fakes_behaviour 11, fakes_conformance 17, route_guard 14, openapi_conventions 10, all passed) and `cargo clippy -p ports --all-features --all-targets -- -D warnings` clean |
| `proto-verify` | 0 | `pnpm exec buf lint proto` clean; `cargo test -p proto` roundtrip 21 passed; clippy `-D warnings` clean |
| `openapi-verify` | 0 | `redocly lint api/openapi.yaml --config .redocly.yaml`: `Woohoo! Your API description is valid.`; `openapi_conventions` 10 passed; clippy clean |
| `db-verify` | **not run** | needs Docker. Recipe unchanged. See the section above. |
| `mcp-doc-verify` | 0 | `cargo test -p xtask --test mcp_tools_doc`: 21 passed; clippy clean |
| `perf-verify` | 0 | `bench-check-selftest: the planted over-budget value was rejected`; `bench_check` 17 passed, `budgets_registry` 7 passed; clippy clean; `cargo xtask bench-check` exit 0 |
| `fixtures-verify` | 0 | `fixtures` 22 passed; `fixtures-verify: two same-seed runs give an identical manifest (including every Git ref id) and identical NFS trees`; `cargo test -p xtask --test fixtures -- --ignored`: `target_scale_counts_match_design_scale ... ok` (28.25 s) |
| `fuzz-smoke` | 0 | `cargo metadata --locked`, `cargo deny --manifest-path fuzz/Cargo.toml check`, `cargo audit --file fuzz/Cargo.lock`, then 30 s per target: `nfs_path` 3,819,597 runs, `compare_ref` 5,734,493 runs, `path_mapping_reverse` 488,116 runs, no crash. Last line: `fuzz-smoke: nfs_path compare_ref path_mapping_reverse ran 30 s each without a crash` |

### Gate failures and the fix

- No gate failed for a real reason. The one failure is the Docker limit above.
- One `01/fix:` commit (`6015a3b`): `deny.toml` said the NCSA exception is "Checked by `just fuzz-deps-check`", a recipe that does not exist (the check runs at the start of `fuzz-smoke`). Comment only; no policy change. `cargo deny check` and `cargo deny --manifest-path fuzz/Cargo.toml check --config deny.toml` were re-run afterwards: both `advisories ok, bans ok, licenses ok, sources ok`.

## Acceptance checklist

- [ ] `just verify-01` and `just verify` pass. **Partly.** `just verify` passes (exit 0). `just verify-01` stops at `docker-check` (exit 1) because there is no Docker daemon. All its other sub-recipes pass when run one by one, but `db-verify` was not run, so the gate as a whole is not proven here. Run `just verify-01` on a Docker host.
- [x] Every trait in `docs/interfaces.md` exists, with a fake and conformance tests. Proof: `ports/tests/contract_shape.rs::every_interfaces_md_trait_is_covered` and `all_ports_are_object_safe_send_sync_static`; `ports/tests/fakes_conformance.rs`: `fake_{agent_gateway,report_sink,blob_store,git_reader,event_bus,leases,kms_signer,kms_envelope,secret_source,notifier,token_verifier,users,audit_log,registry_read,write_service,doc_search,sentinel_sink}_conforms` (17 ports, 17 passed); fake behaviour in `fakes_behaviour.rs` (`gateway_timeout_takes_exactly_the_timeout_on_the_tokio_clock`, `lease_can_be_taken_the_moment_the_clock_passes_expiry`, `blob_store_refuses_new_content_when_full_but_still_accepts_known_content`, `tampering_with_a_committed_audit_entry_breaks_the_chain_at_that_entry`). Run in `ports-verify` and `just verify`. Note: `ports::conformance::<port>(impl)` is exercised against the fakes only; real implementations run it later.
- [x] The proto passes its linter and tests. Proof: `pnpm exec buf lint proto` clean; `proto/tests/roundtrip.rs` 21 passed, including `every_message_roundtrips`, `scan_delta_over_3mib_is_rejected`, `transport::zstd_is_negotiated_over_duplex_channel`, `transport::a_server_without_zstd_refuses_a_zstd_client`, `reserved_and_unknown_oneof_variants_are_ignored`, `join_tokens_never_reach_debug_output`; clippy `-D warnings` clean.
- [x] The OpenAPI spec passes its linter and tests. Proof: `redocly lint` valid; `openapi_conventions.rs` 10 passed: `every_operation_has_valid_x_required_role`, `every_non_get_requires_csrf_and_idempotency_key`, `every_list_uses_keyset_with_limit_max_500`, `content_endpoints_have_etag`, `no_agent_join_token_endpoint`, `blobs_cache_control_private_immutable`, `errors_are_problem_json_and_every_operation_has_a_default`, `secrets_are_write_only`, `roles_follow_the_approved_inventory`, `schemas_carry_the_required_fields`.
- [ ] The migrations pass their tests, and the database-role permission tests pass. **NOT PROVEN.** `db/migrations/0001_init.sql` exists and the text-only tests pass, but no test that needs Postgres ran (17 `SKIPPED`, 18 after the round 1 fix). The tests that must pass on a Docker host: `migration_applies_on_pg16`, `extensions_and_lz4_toast_are_active`, `app_cannot_update_audit_events`, `app_cannot_delete_audit_events`, `app_cannot_truncate_audit_events`, `trigger_blocks_update_and_delete_even_for_owner`, `app_cannot_run_ddl`, `app_has_only_insert_select_on_audit_events`, `every_table_has_a_grant_decision`, `outbox_insert_notifies_on_commit_only`, `pr_links_unique_open_per_swimlane_path_hash`, `audit_prev_hash_cannot_fork`, `sentinel_partition_function_is_the_only_ddl_path`, `sentinel_partition_function_refuses_short_retention`, `approvals_reject_self_approval`, `idempotency_and_webhook_replay_keys_are_unique`, `migration_refuses_to_run_as_another_role`.
- [x] Fixture generation is deterministic. Proof: `fixtures.rs::same_seed_gives_identical_manifest_and_tree` and `different_seed_differs`; `fixtures-verify` ran `gen-fixtures --scale small` twice (seed 20260101: 4 swimlanes, 6 tenant branches, 80 base files, 492 NFS files), `cmp` of both `manifest.json` (includes every Git ref id) and `diff -r` of both `nfs/` trees were empty.
- [x] Target scale matches `docs/performance.md`. Proof: `fixtures.rs::target_scale_counts_match_design_scale` (`--ignored`, run by `fixtures-verify`, 28.25 s, passed; 40 swimlanes, 60 tenant branches, about 80,000 files). Shapes: `contains_{channel_folders,nested_service_folders,crlf_files,yaml_anchors,duplicate_keys,xsl,images,extensionless_files,six_thousand_line_yaml,tenant_forks_with_missing_entries,redundant_copies,duplicate_names_in_non_channel_folders}`, `nfs_uses_tenant_suffix_naming`, `git_repos_are_valid_and_shaped_like_the_real_ones`, `no_real_hosts_or_people_appear_in_fixtures` (all passed).
- [x] `bench-check` fails a planted over-budget value. Proof: `just bench-check-selftest` (inside `perf-verify`) ran the binary on `crates/xtask/testdata/bench-planted` (budget 10 ms, committed samples 50 ms): `bench-check-selftest: the planted over-budget value was rejected` (exit 1 required, got 1); tests `fails_on_planted_over_budget_value`, `committed_planted_testdata_is_rejected_by_the_binary`, `fails_on_missing_result_when_strict`, `respects_lower_bound_direction`, `p95_is_computed_from_criterion_samples`, `passes_within_budget`.

Per-task Verify results beyond the acceptance list:

- [x] T1 `just verify` on the skeleton: exit 0 (above).
- [x] T2 `NfsPath` rejects every traversal form and mapping round-trips: `nfs_path.rs::{rejects_every_traversal_form, structured_traversal_never_accepted, never_yields_dotdot_prop}`, `mapping.rs::{forward_matches_example, reverse_roundtrip_prop, hyphenated_names_keep_their_hyphens, extensionless_files}`.
- [x] T7 `docs/mcp-tools.md` lint: `mcp_tools_doc.rs` 21 passed (25 tools, role floor equal to REST, no notify tool, caps, bounded inputs).
- [x] T9 `just fuzz-smoke` clean (above).

## Security requirements implemented

- **S4** (route-guard harness): `crates/ports/src/conformance/route_guard.rs` (`assert_route_table(routes, spec)`, reads `x-required-role` from `api/openapi.yaml`). Tests in `ports/tests/route_guard.rs`: `unguarded_route_fails`, `role_mismatch_with_openapi_fails`, `spec_operation_without_route_fails_unless_allowed`, `route_missing_from_spec_fails_unless_allowed`, `pending_and_disabled_users_are_denied_before_handler`, `all_guarded_and_matching_passes`, plus three `should_panic` tests proving the harness itself catches a bad gate (`a_gate_that_lets_pending_users_through_fails_the_harness`, `a_gate_that_ignores_disabled_status_fails_the_harness`, `a_gate_that_ignores_the_role_floor_fails_the_harness`). The real routes are wired by 03a, 05 and 07.
- **S11** (typed validators): `crates/domain/src/paths.rs` (`NfsPath`, `RepoPath`, `DocPath`), `mapping.rs` (`PathMappingRule`, `TenantSet`), `compare_ref.rs`, `git.rs`, `ids.rs`. Tests: `nfs_path.rs::{rejects_every_traversal_form, rejects_nul_and_control_chars, rejects_over_1024_bytes, rejects_a_component_over_255_bytes, normalises_separators, never_yields_dotdot_prop}`, `compare_ref.rs::{parses_all_five_forms, rejects_malformed_refs, roundtrip_prop}`, `mapping.rs::{only_exact_known_tenant_suffix_is_stripped, longest_known_tenant_wins, reverse_never_panics_and_forward_inverts_it}`, `ids.rs::tenant_id_rejects_invalid_chars`; `proto/tests/roundtrip.rs::hostile_paths_never_become_nfs_paths` (the proto-to-domain edge).
- **S19** (dependency policy): `deny.toml`, `.cargo/audit.toml` (one approved ignore), exact pins in `Cargo.toml`, committed `Cargo.lock`, `fuzz/Cargo.lock` and `pnpm-lock.yaml`, compose image pinned by digest. Proof: `cargo deny check` and `cargo audit` in `just verify` (clean), and the same two against `fuzz/` in `fuzz-smoke` (clean).
- **S21** (code rules, no secret leakage): `crates/domain/src/secret.rs`. Tests: `secret.rs::{debug_is_redacted, display_is_not_implemented, serialize_is_not_implemented, compare_is_constant_time_api, zeroize_on_drop, expose_gives_the_value_only_on_request}`; `errors_do_not_leak_inputs` in `nfs_path.rs`, `compare_ref.rs`, `dto.rs` (ids) and `ports/tests/contract_shape.rs`; `fakes_behaviour.rs::secret_source_debug_never_prints_values`; `proto/tests/roundtrip.rs::join_tokens_never_reach_debug_output`. Workspace lints (`forbid(unsafe_code)`, denied `unwrap`/`expect`/`panic`/`todo`/`unimplemented`/`dbg`) pass under `clippy -D warnings`.
- **S22** (fuzz skeleton): `fuzz/fuzz_targets/{nfs_path,compare_ref,path_mapping_reverse}.rs`, invariants in `fuzz/invariants.rs`, 57 seed inputs in `fuzz/corpus/`. Proof: `just fuzz-smoke` clean (run counts above); `domain/tests/corpus_replay.rs::{replays_all_seed_corpora, every_target_has_a_fuzz_binary_and_a_corpus}` replays the seeds on stable.
- **Written but not proven here: S9** (audit append-only, role split). `0001_init.sql` grants `lanekeeper_app` only INSERT and SELECT on `audit_events`, has update/delete/truncate triggers and `UNIQUE(prev_hash)`; the fake audit chain is proven (`ports/tests/contract_shape.rs::audit_canonical_json_and_chain_hash_are_pinned`, `fakes_behaviour.rs::tampering_with_a_committed_audit_entry_breaks_the_chain_at_that_entry`, `fake_audit_log_conforms`), but the real database protections are the unrun db-verify tests.
- Shaping only (no proof owed by 01): S5/S25 (`PeerKind`, sentinel messages in `proto/agent.proto`), S7/S8 (`Secret`, write-only secrets in OpenAPI: `secrets_are_write_only`), S14/S14b (`mcp_tools_doc.rs::file_and_doc_text_tools_label_their_text_as_data`, `the_secret_flag_never_has_a_reveal_path`).
- Threat model: `docs/threat-model.md` has a STRIDE section for the 01 deliverables (extra path approved in the plan).

## Benchmarks against budgets

01 owns the registry, not the benchmarks. `perf/budgets.toml` has 33 entries (P1-P15 with sub-rows); every one is `registered = false` because the owning prompt supplies the benchmark. Thresholds are tested against `docs/performance.md` (`budgets_registry.rs::thresholds_match_docs_performance_md`).

`cargo xtask bench-check` (default, not `--strict`), exit 0:

```
UNMET P1                                            - p95 <= 15000 ms  not registered; no benchmark yet
UNMET P2                                            - p95 <= 10000 ms  not registered; no benchmark yet
...
UNMET P15.spool_replay_versions_per_s               - min >= 1000 versions_per_s  not registered; no benchmark yet
bench-check (default): 0 pass, 0 fail, 33 unmet of 33 budgets
```

What the 33 UNMET entries mean: no P# is proven at all at this stage. "UNMET" is a warning for an unregistered budget; it does not fail the default gate (Q5(c)) but does fail with `--strict`, which prompt 16 and the release gate use. A pass here says only that the mechanism works (the planted over-budget value is rejected, a registered entry without a result fails), not that any budget is met. Each owning prompt must add a benchmark, flip `registered = true` through a contract-change PR, and show real numbers.

| P# | Benchmark | Result | Budget |
|---|---|---|---|
| P1-P15 (33 rows) | none yet (named in `perf/budgets.toml`) | not measured | as in `docs/performance.md` (for example P1 p95 <= 15000 ms, P7.grid p95 <= 150 ms, P10.grid_5000_rows_fps min >= 60 fps) |

## New dependencies

Ages are days from the crates.io / npm publish date to 2026-10-09. Every version is an exact pin and at least 14 days old. Transitive crates that were younger than 14 days were lowered with `cargo update --precise` (T2: `libc`, `zerocopy`; T3: `either`, `smallvec`, `yoke-derive`; T4: `cc`, `find-msvc-tools`, `h2`, `hyper`, `lazy_static`, `mio`, `pulldown-cmark-to-cmark`, `tokio-util`, `unicase`, `want`, `zstd-sys`; T6: `jiff`, `js-sys`, `powerfmt`, `serde_with`, `tokio-rustls`, the `wasm-bindgen` family; T9: `arbitrary` 1.5.0 -> 1.4.2). I did not re-audit every transitive age in this step; `cargo deny` and `cargo audit` are clean.

| Crate or package | Version | Age in days | Why | `cargo deny` / audit |
|---|---|---|---|---|
| serde / serde_json | 1.0.229 / 1.0.151 | 83 / 81 | DTOs and wire shapes | clean |
| thiserror | 2.0.20 | 62 | typed error enums (rule 3) | clean |
| serde-saphyr | 1.1.0 | 55 | YAML reader for the OpenAPI conventions test and route-guard harness (Q8); pure safe Rust; `serde_yaml` is denied | clean |
| toml | 0.9.8 | 365 | read `perf/budgets.toml` in xtask | clean |
| hex | 0.4.3 | 2046 | content hashes | clean |
| secrecy / zeroize / subtle | 0.10.3 / 1.9.0 / 2.6.1 | 730 / 119 / 837 | `Secret<T>` (S7, S21) | clean |
| sha2 | 0.10.9 | 527 | content hash and audit chain | clean |
| bytes / futures / async-trait / tokio / tracing | 1.12.1 / 0.3.34 / 0.1.92 / 1.53.1 / 0.1.44 | 93 / 59 / 62 / 81 / 295 | port traits (object-safe async), bounded channels, timeouts | clean |
| **sqlx** | 0.8.6 (default features off; `postgres` in `ports` (optional) and `xtask`; `migrate`, `runtime-tokio` in `xtask`) | 508 | `ports::Tx::from_pg` (Q3) and the migration tests. **No TLS feature**: sqlx's rustls roots pull `webpki-roots` (CDLA-Permissive-2.0), which `deny.toml` rejects. 0.9 needs rustc 1.94 (toolchain is 1.88). Production TLS to Postgres is a decision for a later prompt. | clean, with the approved `RUSTSEC-2023-0071` ignore below |
| tonic / tonic-prost / tonic-prost-build / prost / prost-types | 0.14.6 x3 / 0.14.4 x2 | 155 / 124 | gRPC with zstd (T4) | clean |
| protox | 0.9.1 | 311 | pure-Rust protobuf compiler, so no `protoc` binary (Q10) | clean |
| tokio-stream / hyper-util / tower | 0.1.19 / 0.1.21 / 0.5.3 | 79 / **15** / 270 | dev-only duplex-channel gRPC tests in `crates/proto`. hyper-util is only one day over the 14-day line. | clean |
| proptest / insta / static_assertions | 1.11.0 / 1.48.0 / 1.1.0 | 199 / 120 / 2532 | property tests, golden wire names, compile-time `!Serialize` checks | clean |
| **testcontainers** | 0.28.0 (default features off; dev-dependency of `xtask`) | 64 | real Postgres 16 + pgvector for db-verify | clean |
| **libfuzzer-sys** | 0.4.13 (`fuzz/` only, own lockfile, outside the workspace) | 127 | the cargo-fuzz runtime for S22. Transitive in `fuzz/Cargo.lock`: `cc`, `arbitrary` 1.4.2 (421 days), `shlex`, `jobserver`, `find-msvc-tools`, `libc`, `getrandom`, `r-efi`, `cfg-if`. | clean via `cargo deny --manifest-path fuzz/Cargo.toml check` with the exception below |
| cargo-fuzz (tool) | 0.13.2 | 122 | pinned in the `Justfile`; needs nightly | n/a (tool) |
| nightly toolchain | nightly-2026-09-14 | 25 | cargo-fuzz sanitizer flags; main workspace stays on stable 1.88 | n/a |
| cargo-deny / cargo-audit (tools) | 0.19.9 / 0.22.2 | not checked | pinned in the `Justfile` | n/a |
| @bufbuild/buf / @redocly/cli (root `package.json`) | 1.73.0 / 2.53.3 | 28 / 22 | `buf lint` for `proto/`, `redocly lint` for `api/` (Q9). pnpm 10.34.6. `pnpm` warns "Ignored build scripts: @bufbuild/buf"; the binary works without them (`buf --version` ran). | lockfile is frozen; `osv-scanner` was **not** run here |

### Items that need a human

1. **NCSA licence exception in `deny.toml` (needs human confirmation).** `libfuzzer-sys` bundles LLVM's libFuzzer, which is also under the NCSA licence, and `cargo deny` rejects that because NCSA is not in the allow list. I added a scoped exception (`[[licenses.exceptions]] crate = "libfuzzer-sys" allow = ["NCSA"]`) instead of allowing NCSA globally. The crate is a build-time tool of `fuzz/`, never shipped. NCSA is a permissive licence (University of Illinois/NCSA, MIT-like) but this is a licence-policy change, so a human must accept it. Fallback if refused: drop the exception and run the fuzz crate without `cargo deny` (weaker S19), or replace libFuzzer.
2. **`.cargo/audit.toml` ignore of RUSTSEC-2023-0071** (`rsa`, Marvin attack, no fixed release). Approved by the user as an extra path for T3. `rsa` is in `Cargo.lock` only because sqlx's `macros` feature lists `sqlx-mysql`. Evidence it is never compiled: `cargo tree -i rsa -e all --target all` prints `warning: nothing to print.` (re-run in this step). Remove the ignore if that command ever prints a path.
3. **sqlx without TLS** (above): Postgres connections have no TLS feature compiled. Fine for the tests and the local container; the production choice (sqlx `tls-rustls-aws-lc-rs` plus a licence decision on `webpki-roots`' CDLA-Permissive-2.0, or CloudNativePG in-cluster plaintext) belongs to prompt 09 and a human.
4. **serde-saphyr** is a new, relatively young YAML crate (55 days). It is used only by tests and the conformance harness (`ports` features `conformance`/dev), not by product code in this prompt. Prompt 04 should confirm it as the YAML crate or choose another.
5. **testcontainers** needs a Docker socket at test time; it is dev-only.
6. `osv-scanner` (named in AGENTS.md rule 15) was not run; only `cargo deny` and `cargo audit` were available and run.

## Assumptions

The plan's open questions (the plan's own Q1-Q25, not `docs/open-questions.md`) were approved with the plan as written (commit `2153623`, "approved by a human"); no answers or overrides were recorded, so every default was used except where listed in "Deviations". Status per question:

| Q | Topic | Default used? |
|---|---|---|
| Q1 | No previous bundle; endpoint and table lists from Appendix A/B | Yes, with additions (see deviations: `settings_index_blobs`) |
| Q2 | Resolved signatures live in rustdoc; `EventBus` merged with `publish_in_tx` | Yes (`every_interfaces_md_trait_is_covered`) |
| Q3 | Opaque `ports::Tx` with `from_pg` (feature `postgres`) and `in_memory` | Yes |
| Q4 | Ports use domain types; validating conversions in `crates/proto/src/convert.rs` | Yes |
| Q5 | P1-P15 plus sub-rows, `direction` field, `--strict` semantics | Yes (33 rows, `direction = "lower"` for fps, Lighthouse, spool replay) |
| Q6 | Roles pre-exist (`deploy/dev/init.sql`); migration refuses to run as another role | Yes (`migration_refuses_to_run_as_another_role` exists; unrun here) |
| Q7 | `LK_REQUIRE_DOCKER=1` makes missing Docker a failure, else skip | Partly: set in `db-verify` only; **not** set for `cargo test --workspace` or CI (see top) |
| Q8 | A maintained serde YAML crate behind test/conformance features | Yes: serde-saphyr 1.1.0 |
| Q9 | `buf` and `redocly` from a root `package.json` and `pnpm-lock.yaml` | Yes |
| Q10 | `protox`, generated code not committed | Yes |
| Q11 | `PeerKind` on `JoinRequest`, unspecified rejected | Yes (`a_sentinel_join_yields_a_sentinel_subject`, `join_requires_a_known_kind_...`) |
| Q12 | `more` and `part` on `ScanDelta`, 3 MiB / 4 MiB limits | Yes (`more = 8`, `part = 9`) |
| Q13 | `NfsPath`: collapse `//`, drop `.`, reject `..`, NUL, control chars, backslash, no Unicode normalisation | Yes, plus a 255-byte component limit (deviation) |
| Q14 | Mapping edge cases, longest known tenant wins | Yes (`longest_known_tenant_wins`, `dotfiles_and_multi_dot_names`) |
| Q15 | `GitRef` with `check-ref-format` rules | Yes (`git_refs_accept_branches_tags_and_commit_ids`) |
| Q16 | `Deserialize` for `Secret<String>` only, never `Serialize` | Yes |
| Q17 | `pgvector/pgvector:pg16` pinned by digest in dev and tests; production image is 09's | Yes (`deploy/dev/compose.yaml`, `image_matches_compose`; `db/migrations/README.md` lists required server features). Still needs the human to confirm 09 owns the image. |
| Q18 | `SECURITY DEFINER maintain_sentinel_partitions` with fixed `search_path` | Yes in the SQL (`SET search_path = pg_catalog, pg_temp`); its test is unrun here |
| Q19 | Explicit per-table grants, TRUNCATE trigger, `UNIQUE(prev_hash)` | Yes in the SQL; tests unrun here |
| Q20 | `git fast-import` for identical object ids; manifest, NFS bytes and ref ids compared | Yes |
| Q21 | 40 swimlanes, 60 branches `sit1..sit60`, some swimlanes with a second tenant, about 80,000 files | Yes for the counts (proven); the second-tenant set is in `fixtures/mod.rs`, not separately tested by me |
| Q22 | No notify MCP tool; read tools Viewer; writes Editor, `restart_service` Operator; all writes `confirmation: required` | Partly: **`propose_change` is `confirmation: none`** (deviation) |
| Q23 | No replica `Forward` RPC in 01 | Yes. 03b raises a `replica.proto` contract change. |
| Q24 | Pinned nightly, fuzz crate outside the workspace, seeds replay on stable | Yes (`nightly-2026-09-14`, cargo-fuzz 0.13.2) |
| Q25 | Docs lint test for `docs/mcp-tools.md` | Yes (`mcp_tools_doc.rs`) |

Other assumptions:

- Docker-dependent tests skip rather than fail unless `LK_REQUIRE_DOCKER=1` (Q7), so a plain `cargo test` works on a machine without Docker.
- `cargo xtask bench-check` is run without `--strict` in `verify-01`; unregistered budgets are warnings.
- The fixtures need `git` on `PATH` (dev tool only).
- Generated protobuf code is not committed; it is built by `crates/proto/build.rs`.

## Deviations from the plan

None of these is in a `plans/01-contracts.blockers.md` (no such file; none blocked the work). They are listed here for the reviewer and the human.

1. **`propose_change` has `confirmation: none`** (plan Q22 said all six write tools `required`). `propose_change` only records a draft with no side effect (prompt 07 T3); the confirmation happens at `apply_change` (D52). The other five write tools are `required`. `docs/mcp-tools.md` says so (line 64) and `mcp_tools_doc.rs::confirmation_rule_follows_d52` pins it.
2. **`bench-check --self-test` flag replaced by the `bench-check-selftest` recipe.** The plan's `fails_on_planted...` proof used a flag; instead the Justfile recipe runs `cargo xtask bench-check --budgets ... --criterion-dir ... --results-dir ...` on `crates/xtask/testdata/bench-planted` and requires exit code 1 (not 0, not a usage error). Same property, no test-only flag in the shipped binary.
3. **The manifest insta snapshot was skipped.** The plan listed an insta golden of `manifest.json` at `--scale small`. Determinism is proven by `same_seed_gives_identical_manifest_and_tree` and the `cmp`/`diff -r` in `fixtures-verify` instead. The oracle in the manifest is checked by `planted_drift_cases_exist_and_are_real`. Not a golden, so a silent change to the generator's output would not be caught by a snapshot review.
4. **`settings_index_blobs` table added beyond Appendix B.** The migration creates 52 tables: the Appendix B inventory plus `settings_index_blobs`, a once-per-blob marker so a blob is parsed once (P-series "parse each unique blob once"). Needs the human to approve the table with the contract.
5. **`NfsPath` has a 255-byte per-component limit** in addition to the 1,024-byte total (Q13 only specified 1,024). Matches the usual NFS/ext4 name limit; `nfs_path.rs::rejects_a_component_over_255_bytes`, `MAX_COMPONENT_BYTES = 255`. A real file name above 255 bytes cannot exist on NFS, so this rejects nothing legitimate.
6. **T2 domain additions beyond the prompt's type list**, needed so ports and proto could be typed without raw strings: `ScanDelta::MAX_BYTES` (3 MiB), `ScanDelta::MAX_ENTRIES`, `ScanDelta::payload_bytes`; `DocPath`, `LineRange`, `GitRef`, `ServiceRef`, `Page`/cursor types, `Attribution` and confidence/severity enums, the read and write DTOs, the agent and sentinel wire mirrors, and 15 `DomainEvent` variants (10 base plus 5 for D72-D80). All are contract surface in `crates/domain`.
7. **sqlx TLS feature left out** because `webpki-roots` (CDLA-Permissive-2.0) is rejected by `deny.toml`; the plan's risk list had assumed `tls-rustls-aws-lc-rs`. See "Items that need a human", point 3.
8. **`.cargo/audit.toml` ignore for RUSTSEC-2023-0071.** Added during T3 as an extra path, approved by the user (commit `ca6c822`); it was not in the original plan text.
9. **Smaller departures**: `LK_REQUIRE_DOCKER=1` is set only inside `db-verify`, not "in `verify-01` and CI" as Q7 proposed; CI wiring for `buf`, `redocly`, Docker tests and nightly fuzz is handed to prompt 09 as the plan said. A stale comment in `deny.toml` was corrected in the `01/fix:` commit.

## What a human must do before merge

1. Run `just verify-01` on a Docker host (or in CI with Docker) and confirm `db-verify` passes: this is the only part of prompt 01 not proven here.
2. Accept or reject the NCSA exception for `libfuzzer-sys`.
3. Approve the contract points called out in the plan: `PeerKind` (Q11), `more`/`part` (Q12), the budget `direction` field (Q5), `propose_change` confirmation none, `settings_index_blobs`, and the definer function `maintain_sentinel_partitions` (Q18).
4. Set `LK_REQUIRE_DOCKER=1` for the CI test step (prompt 09), and decide the production Postgres image and TLS.

## Round 1 review fix (S9, S21)

Blocking finding 1: `maintain_sentinel_partitions(keep_days)` accepted 1 to 3650, so `lanekeeper_app` could drop sentinel partitions still inside the 90-day life.

- Fix: `db/migrations/0001_init.sql` (unmerged, edited in place) now has `min_keep_days CONSTANT integer := 90` in the function body. `keep_days` below 90, NULL or above 3650 raises SQLSTATE `22023`. The floor is not a parameter, setting or app-writable row.
- New test `sentinel_partition_function_refuses_short_retention`: as `lanekeeper_app`, `keep_days` of 1, 2, 30, 62, 89 and NULL is refused (`22023`) and partition and row counts are unchanged; `maintain_sentinel_partitions(90)` then drops only the partition from 6 months ago (created 0, dropped 1), keeps the 2-months-ago and last-month partitions and their row; 365 drops nothing. `sentinel_partition_function_is_the_only_ddl_path` still passes (it already uses 90 and checks 0 and 100000 are refused).
- Mutation proof: with the floor changed to `1`, `sentinel_partition_function_refuses_short_retention` FAILS (`expected failure: SELECT * FROM maintain_sentinel_partitions(1): PgQueryResult { rows_affected: 1 }`); the same failure was seen first, before the fix (red), and the test passes with the floor at 90. The file was restored after the mutation.
- Docs: `docs/threat-model.md` (Elevation of privilege row, the D row on `keep_days`, and the residual-risk bullet) and `db/migrations/README.md` now state the real behaviour.
- Non-blocking 6 (harness race): `run_init` now takes a session advisory lock around the init SQL. With `LK_TEST_PG_ADMIN_URL`, `--test-threads=4` and `--test-threads=8` (5 runs) and `--test-threads=1` all gave 23 passed, 0 failed.
- Run against a throwaway local PostgreSQL 16 without pgvector (`LK_TEST_PG_ADMIN_URL`): 23 passed. This is still **not** a Docker run: the pgvector block, `extensions_and_lz4_toast_are_active`'s pgvector checks and the container-image path remain NOT PROVEN, and `db-verify` / `just verify-01` was **not** run (no Docker daemon). `just verify`: exit 0, 255 passed, 0 failed, 1 ignored. `cargo xtask bench-check`: exit 0, 0 pass, 0 fail, 33 unmet of 33 (unchanged).
