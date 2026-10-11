<!-- PR title: "03a: Hub identity, sessions, authorization and audit" · branch agent/03a-hub-identity-audit · label: contract-change (the plan header says Contract-change: APPROVED) -->

## Prompt

`prompts/03a-hub-identity-audit.md` · plan: `plans/03a-hub-identity-audit.md` (approved by a human) · wave: 1

`crates/hub-identity`: Entra sign-in (authorization code with PKCE, backend-for-frontend), users, roles and access requests held in the app, server-side sessions with CSRF protection, default-deny authorization (`GuardedRouter`, role extractors, an exported route-table check), MCP and agent token verification, and the hash-chained audit log with hourly KMS-signed checkpoints, nightly verification and a Cloud Logging copy. Includes the identity and audit threat model (`docs/threat-model.md`, `## Hub identity and audit (03a)`). The crate is a library; the hub binary (prompt 09) composes it.

**Depends on a merged contract PR.** #6 `contract/03a-identity` (`DomainEvent::UserSessionsRevoked`, `Me.needs_github_token`, the `__Host-` cookie names in `api/openapi.yaml`) is merged and was a prerequisite of T3. Apart from `perf/budgets.toml` (three new registered budgets, see below), this branch's diff against `main` contains no contract path and no migration.

Full evidence, with where each result came from: `plans/03a-hub-identity-audit.evidence.md`.

## Gates

`just verify-03a` and `cargo xtask bench-check` were run by a human on a machine with Docker and the pinned nightly (WSL Ubuntu), on `c44cbdb`. The implementer sandbox has no Docker and no nightly, so those two gates were not run by the implementer; the numbers below are the human's.

| Command | Result |
|---|---|
| `just verify-03a` | PASS, exit 0, 413 s. `identity-tools-check`; `identity-lint` (fmt, clippy `-D warnings`); `identity-test` (`LK_REQUIRE_DOCKER=1`): lib 108 passed, 24 test binaries, all 0 failed, including `audit_chain` 15, `audit_verify` 18, and the new `log_capture_callsite` and `pg_teardown`; `identity-fuzz`: `identity_jwt`, `identity_cookie`, `identity_audit_event`, 30 s each, no crash; `identity-bench`. Zero "could not remove the test container" lines and no containers left behind. |
| `cargo xtask bench-check` (after `verify-03a`) | PASS, exit 0: `10 pass, 0 fail, 27 unmet of 37 budgets`. The 27 unmet rows are other prompts' budgets (not registered yet). |
| `just verify` | **Not green on every run: 2 failures, then a pass (three runs).** Run 1 exit 101 (57 s): `crates/agent/tests/kube_watch.rs:310`. Run 2 exit 101 (8 s): `crates/agent/tests/binary.rs:72`. Run 3 exit 0 (260 s), reached `web-verify`. Both failures are in prompt 02's `crates/agent`, which this PR does not touch. See "Known gaps" below. |
| `cargo fmt --all --check`, `cargo test -p xtask` | PASS (95 passed, 0 failed, 1 ignored), run by the implementer |
| Full hub-identity suite without Docker, implementer sandbox | 394 tests in 23 binaries, 0 failed, with default threads and with `--test-threads=1` |

## Acceptance checklist

- [x] `just verify-03a` passes, with the auth middleware under 2 ms and audit `record` under 3 ms in `bench-check` (`PASS P7.auth_middleware 0.004`, `PASS P7.auth_middleware_cold 0.550`, `PASS P12.audit_record 2.605`, p95 ms; P12 is noisy, see Benchmarks).
- [x] The route-table guard test exists and is exported for the hub binary (`route_table_matches_openapi_and_has_no_unguarded_route` and seven more in `tests/route_table.rs`; exported as `hub_identity::testing::{assert_hub_route_table, assert_default_deny}` behind feature `route-table`). That the hub binary calls it is prompt 09's.
- [x] Chain verification catches tampering, and the checkpoint signatures verify (`tampered_row_is_detected`, `deleted_middle_row_is_detected`, `truncated_tail_is_detected_by_checkpoint`, `forged_checkpoint_row_is_detected`, `bucket_object_mismatch_is_detected`, `checkpoint_signature_verifies_with_public_key`, `checkpoint_signature_by_other_key_fails`, proptest `chain_verifier_property_any_single_byte_change_is_detected`). Signatures are checked with a real P-256 test key; a real Cloud KMS key was not available.
- [x] Every rule in S1 to S4 has a named test (table in the evidence file; every name was checked to exist as a test function).

## Security requirements implemented (S#)

S1 (Entra code flow with PKCE, issuer, audience, `tid`, nonce, expiry, signature via cached JWKS; identity is `tid` plus `oid`; pending and disabled users get 403 everywhere including MCP; MCP and agent token checks), S2 (server-side sessions, `__Host-lk_session`, only the SHA-256 stored, 8 h idle and 12 h absolute, rotation at sign-in, revocation on every replica), S3 (double-submit token bound to the session plus exact `Origin`), S4 (every route declares a role, default-deny, last active admin cannot be removed, no self role or status change), S7 (secrets in `Secret<T>`, client secret from `SecretSource` or a federated assertion re-read on every exchange), S9 (advisory-locked hash chain in the caller's transaction, hourly KMS checkpoint to a create-only bucket, nightly verification with alert, Cloud Logging copy), S10 (log-capture test: no token, cookie, secret or diff), S21 (`forbid(unsafe_code)`, clippy denies, generic problem+json bodies), S22 (fuzz targets `identity_jwt`, `identity_cookie`, `identity_audit_event` with seed replay in `just verify`). Each S# is traced to code and tests in the evidence file.

S21: the plan's test `no_unwrap_on_input_derived_data` was missing at the first FINAL step and is now in `tests/hygiene.rs` (with a planted-violation test). It checks that the workspace lint denies are in force and that production source under `src/` has no `.unwrap()`, `.expect(`, `panic!(` or similar. The clippy denies and the corpus proptests remain the other halves.

## Benchmarks against budgets (P#)

Run by the human, one `just verify-03a` run, loopback Postgres in a container, p95:

| P# | Result | Budget |
|---|---|---|
| P7 `P7.auth_middleware` (warm cache) | PASS 0.004 ms | <= 2 ms |
| P7 `P7.auth_middleware_cold` | PASS 0.550 ms | <= 2 ms |
| P12 `P12.audit_record` (second replica contending for the lock) | PASS 2.605 ms | <= 3 ms |

**P12 varies from run to run.** Without Docker, with fsync on, eight implementer runs at `f912e90` gave p95 between 2.38 and 3.12 ms (one at 3.115 ms, just over 3 ms); later runs gave 1.86 to 2.26 ms. The human accepted the 2.4 to 3.1 ms range as meeting P12, and the budget stays at 3 ms. Treat 2.605 ms as one sample inside that range, not as a margin. Everything is on loopback; prompt 16 must measure staging. The nightly verifier's run time at one million events was not measured.

`perf/budgets.toml`: exactly three new `registered = true` blocks, no threshold changed. Extra-paths (approved with the plan): `docs/performance.md` (two sentences) and `crates/xtask/tests/budgets_registry.rs` (three ids).

## New dependencies and why

Six package versions are new in `Cargo.lock`, all at least 31 days old (crates.io dates, checked 2026-10-11): `moka` 0.12.16 (63 d; the stack's cache, for the session cache), and its dependencies `async-lock` 3.4.2 (294 d), `crossbeam-channel` 0.5.17 (36 d), `tagptr` 0.2.0 (1885 d), `uuid` 1.26.1 (31 d); and `serde_path_to_error` 0.1.20 (391 d), pulled in by `axum`'s `query` feature.

New direct workspace pins, each already in the lock at the same version: `aws-lc-rs` 1.18.1 (40 d; the stack's crypto provider: RS256 verify, AES-256-GCM login cookie, random ids, ECDSA P-256 verify), `axum` 0.8.9 (180 d; the stack's web framework), `base64` 0.22.1 (894 d), `url` 2.5.8 (279 d), `rustls-native-certs` 0.8.4 (132 d; system roots, because `webpki-roots` is refused by `deny.toml`). Features were enabled on `hyper`, `hyper-util`, `sqlx`, `tokio` and others already pinned.

**Not used, against the plan:** `jsonwebtoken` (the plan's B1 proposed it; dropped in T1 because it adds a provider switch that panics when none is installed) and `openidconnect` (RUSTSEC-2023-0071 through `rsa`). RS256 verification is done on `aws-lc-rs` directly. A human should confirm this.

**Rule 15 on the fuzz lockfile: found and fixed.** `crates/hub-identity/fuzz/Cargo.lock` had been resolved fresh and held 12 package versions younger than 14 days on 2026-10-11 (crates.io `created_at`). Commit `03a/FINAL: pin fuzz lock to dependencies at least 14 days old` moved each with `cargo update --precise` to the version already in the root lock and in the agent's fuzz lock: `cc` 1.7.0 (2026-10-10) to 1.4.7 (2026-09-18), `smallvec` 1.16.3 (2026-10-10) to 1.15.2, `syn` 3.0.7 (2026-10-10) to 3.0.6 (2026-09-16), `zerocopy` and `zerocopy-derive` 0.8.62 (2026-10-08) to 0.8.58 (2026-09-24), `arbitrary` 1.5.0 (2026-10-06) to 1.4.2, `either` 1.19.0 (2026-10-06) to 1.18.0, `want` 0.3.2 (2026-10-05) to 0.3.1, `mio` 1.2.4 (2026-10-03) to 1.2.3, `libc` 0.2.190 (2026-10-02) to 0.2.189, `lazy_static` 1.5.1 (2026-10-01) to 1.5.0, `yoke-derive` 0.8.4 (2026-09-30) to 0.8.2 (which adds `synstructure` 0.13.2, from 2025, also in the root lock). All 243 packages in the lock were then checked against the crates.io API: none was published after 2026-09-27; the newest is `find-msvc-tools` 0.1.14 (2026-09-25). The root `Cargo.lock` diff against `origin/main` (six added packages) was checked the same way and was already clean. No `Cargo.toml` changed. Details are in the evidence file. **Not re-verified here:** the libFuzzer run. The pinned nightly is not installed in the sandbox, so a human must re-run `just identity-fuzz` (or `just verify-03a`) on the new lock.

`cargo deny` and `cargo audit`: part of `identity-fuzz` (fuzz lock) and `just verify` (root lock), both exit 0 in the human's runs. After the fuzz lock fix both were run again on the new fuzz lock in the sandbox: `cargo deny ... check` exit 0 and `cargo audit --file` exit 0; `cargo metadata --locked` exit 0. `osv-scanner` was not run.

## Assumptions

The plan was approved as a whole; its ambiguity table holds proposed defaults and no per-question answers are recorded. Defaults taken:

- **B1** own OIDC flow and token verification on `aws-lc-rs` (no `openidconnect`, no `jsonwebtoken`). **B2** one `HttpsClient` (hyper over tokio-rustls, TLS 1.3 only, host allowlist, 5 s, 1 MiB). **B3** login state in a sealed 10-minute cookie (no server-side one-time record, residual risk R3). **B4** a `GuardedRouter` builder, not an attribute macro. **B5** `UserSessionsRevoked` through the outbox (contract PR #6). **B6** no migration.
- **B7** T2 and T3 tests used a test-only chain writer until `PgAuditLog` in T5; no trace of it remains.
- **B8** the Cloud Logging copy omits the diff (carries `diff_sha256` and `diff_len`), because S10 keeps content out of logs. The audit table keeps the diff. **Needs a human.**
- **B9** the GCS checkpoint bucket client lives in this crate. **Needs a human** (or move to 03b). **B10** `ports::canonical_json` is the one canonical form. **B11** `McpAuthenticator` composes the verifier with `Users::require_active`. **B12** `GithubTokenPresence` is local to this crate (`SELECT EXISTS`, never the ciphertext); **needs a human** if it belongs in `crates/ports`.
- **B13** bootstrap admins apply at first sign-in only; if every admin is disabled, recovery is a database intervention. **Needs a human** for a break-glass rule.
- **B14** the `requires_approval` flag change is audited as `RoleChanged`. **B15** no self role or status change. **B16** problem types `access-pending`, `access-disabled`, `insufficient-role`.
- **B17** no `Idempotency-Key` layer on admin writes; the header is ignored and a repeat answers 409. **Needs an owner.**
- **B18** federated credential tested against the mock only (Q25). **B19** sweeps, `last_seen_at` writes and evictions are not audited. **B20** callers call `record` last, because the chain lock is held to commit. **B21** one pending access request per user and one new request per 10 minutes.
- Queries are runtime-checked (`sqlx::query`), not `query!`, with `EXPLAIN` tests on the hot statements.
- Idle time is judged on batched `last_seen_at`, so the 8 h limit is accurate to about two minutes across replicas.

## Known gaps and follow-ups

1. **`just verify` is flaky on prompt 02's tests**, not 03a's. `kube_watch.rs:310` is the `tracing` callsite-interest race that `c44cbdb` fixed in this crate's log-capture helper; the agent's `tests/support/log_capture.rs:28` still uses `set_default` (5 of 80 parallel runs fail, 0 of 80 alone or with `--test-threads=1`). `binary.rs:72` is a SIGTERM race: `shutdown_signal()` in `crates/agent/src/main.rs` is an `async fn`, so the handler registers only on first poll, after the health port is bound; seen only under CPU contention. Proposed, not applied: port the routed global-subscriber helper into the agent tests, and make `shutdown_signal` a plain `fn` that registers before the port is bound. A third green run does not show they are fixed. They need a fix against prompt 02's paths.
2. **Not verified anywhere:** a real Entra tenant and the GKE federated credential (Q25), Cloud KMS, a locked GCS bucket, the GKE metadata server. Mocks and fakes only.
3. **P12 noise** (above). Measure on the staging database.
4. **Fuzz lock** age problem: fixed (above); the nightly libFuzzer run on the new lock is still to be done by a human.
5. **Depends on prompt 09:** the hub binary running `assert_hub_route_table` and `assert_default_deny` against its full route table; the Secret Manager entry for the login-cookie key; the KMS key, locked bucket and federated credential.
6. **No rate limit on `/auth/*`** (ingress or WAF, prompt 09). **No `Idempotency-Key` layer** on admin writes (B17). **Cloud Logging copy omits the diff** (B8). **Bootstrap admins at first sign-in only** (B13).
7. Nightly verification run time at one million events: not measured (the plan said it would be reported).
8. `osv-scanner` not run.

## Reviewer agent verdict

Not run yet. `prompts/REVIEW.md` has not been run on this branch; paste the VERDICT block here when it has.

## Labels

`contract-change`: the plan's `Contract-change: APPROVED` covers `perf/budgets.toml` (three registered budgets) on this branch; the event and OpenAPI changes were merged separately in #6.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01KZpVwUsXXdTwM5FXWWCRKa
