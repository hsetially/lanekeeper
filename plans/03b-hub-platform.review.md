# Review 03b: round 1

Reviewer: `reviewer` subagent (fresh context, read-only). Base: `claude/run-prompt-03b-m6fany`. Head reviewed: `e22419b`.

VERDICT: CHANGES REQUESTED

One blocking finding, a trivial lockfile fix. Every gate passed on the reviewer's re-run, including the libFuzzer half the implementer could not run.

## Blocking

1. **`Cargo.lock` (lines ~4006-4007, 4215-4216, 4238-4239): three new pins are younger than 14 days.** This violates AGENTS.md rule 15 and S19. Ages were checked on crates.io on 2026-10-11:
   - `prodash 31.0.1`: published 2026-10-01 (10 days). Compiled into the shipped hub (gix-features, gix, hub-platform).
   - `quinn-proto 0.11.19`: published 2026-09-30 (11 days). On reqwest's QUIC path; reported as not compiled on Linux (not re-checked).
   - `quinn-udp 0.5.16`: published 2026-09-30 (11 days). Same.

   `cargo deny` and `cargo audit` do not enforce the age rule. No other new crate is under 14 days (99 new lockfile entries checked; `defmt 0.3.100` dates from 2025-04).

   Fix: older versions exist. `prodash 31.0.0` (2026-01-08; gix-features 0.48.1 requires `^31.0.0`), `quinn-proto 0.11.18` (2026-09-14), `quinn-udp 0.5.15` (2026-07-04). Run `cargo update -p prodash --precise 31.0.0`, `cargo update -p quinn-proto --precise 0.11.18`, `cargo update -p quinn-udp --precise 0.5.15`, then re-run `cargo deny check`, `cargo audit` and `just platform-test`.

## Non-blocking

1. **Evidence and PR text are stale on the fuzz half.** The evidence file leaves acceptance items 1 and 4 unticked for lack of nightly/cargo-fuzz. The reviewer's machine has `nightly-2026-09-14`, cargo-fuzz 0.13.2 and `just`: `just verify-03b` as one recipe exited 0, and `platform-fuzz` fuzzed all 5 targets for 30 s each without a crash. The PR text should use these results and tick both items.
2. **Relay fix (T11, `crates/hub-platform/src/events/relay.rs:266-301`) is correct for the failure it targets.** Residual 1: the gap read happens only on the first loop iteration (`first && !gaps.is_empty()`), so with a backlog over `batch` (200) and a concurrent publisher, order could still invert (no event lost; the next poll delivers the gap). Residual 2: the regression test (`events_bus.rs:111`) is probabilistic; a deterministic test with a stub row reader would be better.
3. **A15 (sqlx on ring) is reasonable but should be recorded.** `_tls-rustls-aws-lc-rs` pulls `webpki-roots` (CDLA-Permissive-2.0, disallowed), so `tls-rustls-ring-native-roots` is the only deny-clean variant. ring is in the hub binary only; `hub/src/db.rs` pins `verify-full` with the CA file. No `ServerConfig::builder()`, `ClientConfig::builder()` or `install_default` in our code; reqwest falls back to aws-lc-rs explicitly, so nothing panics with two providers. No test runs gix-over-HTTPS inside the `hub` binary. Add a note in `docs/decisions.md` for the two-provider exception and the narrowed `deny.toml` targets (not an owned path: needs a human or contract/docs PR). The `Cargo.toml` comment near line 122 says "(no ring ...)", stale for the hub binary.
4. **A19: webhook cap is 1 MiB (`crates/hub-platform/src/git/webhook.rs:42`); the approved plan said 4 MiB.** 1 MiB is the `api/openapi.yaml` contract number and is stricter than S11, so security is not weakened, but it needs an explicit human nod. Also: the limits layer (`crates/hub/src/limits.rs:216-235`) buffers any `application/json` body up to 4 MiB and scans its depth before the webhook's 1 MiB check or the HMAC runs, so an unauthenticated sender can make the hub buffer 4 MiB per request (bounded by the per-IP bucket and the 30 s timeout). Consider exempting `/hooks/github` from the buffering or passing its 1 MiB cap into the layer.
5. **The replay guard keys on the unsigned `X-GitHub-Delivery` header, so the threat model overstates it** (code `webhook.rs:273-286`; claim `docs/threat-model.md:261`). An attacker holding a captured signed body can change the header and bypass the guard: one coalesced fetch for pushes; for `pull_request` it can re-apply an old `closed` or `reopened` and move a link wrongly (no ordering on `updated_at`; `merged` stays terminal). Key the guard on the signature digest or body hash as well, or soften the wording.
6. **OpenAPI mismatch.** `api/openapi.yaml` lists 400, 403 and 413 for the webhook; the code returns a bare 401 for a bad signature (as the plan says), which falls under `default`. Align the contract or the code (contract path: human).
7. **`Notifier` is not implemented.** The prompt's Interfaces row says it implements `Notifier` (no-op plus Teams later); no task or plan item covers it and no no-op impl exists. Record it as deferred.
8. **Cargo.toml and `deny.toml` changes match the human's A16 decision.** The only `deny.toml` change is `[graph].targets` plus its comment, so Windows, macOS, Android and wasm32 graphs are no longer checked for any crate. Recorded in `plans/03b-hub-platform.blockers.md` because the plan is frozen.
9. **`crates/hub-platform/fuzz/` has no `.gitignore`.** `crates/agent/fuzz/.gitignore` ignores `target/`, `artifacts/` and `coverage/`; add the same here.
10. **Pre-existing flaky agent test, not caused by 03b:** `crates/agent/tests/binary.rs:72` (`binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm`) failed once in a `just verify` run (exit status `None`; SIGTERM likely arrived before the handler was installed) and passed on rerun. Needs a prompt-02 follow-up.
11. **Contract route.** The plan proposed a separate `contract:` PR for `AuditAction::PrStateChanged` and the budget entries, but both land on this branch. The plan header says Contract-change: APPROVED, so acceptable; the human should confirm the merge route.
12. **Smaller points.** `sentinel_service.rs:358-360`: a batch the sink rejects as `Invalid` or `TooLarge` gets no ack and no stream end, so a sentinel that resends forever would be stuck on a poison batch (07's side). Not proven here: `docker build` of `crates/hub/Dockerfile`, `osv-scanner`, the three `#[ignore]` real-GCP tests.

## Spec chain (verified by the reviewer)

- Plan approval commit `98b632d` exists; the plan file has only `81fd103` (draft) and `98b632d`.
- Commits T1 to T11 in order, with `03b/blocked: A16` between T6 and T7, then `03b/evidence`.
- Every test name in the plan's traceability table exists except three (`fuzz_seconds` is a Justfile variable; `no_unwrap_or_expect_in_non_test_code` is enforced by workspace clippy lints; `pg_pool_refuses_wrong_ca_and_plaintext` was split into three named tests).
- All 7 registered benchmark rows exist.
- Diff outside `crates/hub*`, `docs/threat-model.md` and `plans/` touches only the approved Extra-paths and contract paths (`Cargo.toml`, `Cargo.lock`, `Justfile`, `crates/xtask/tests/budgets_registry.rs`, `deny.toml`, `perf/budgets.toml`, `crates/ports/...` for the additive `PrStateChanged`).
- CA key never leaves KMS; TLS 1.3 only with one pinned CA; webhook order is body cap, HMAC (bare 401), event type, replay guard, strict parse; leases use the DB clock with fence checks; acks only after `ReportSink::delta` returns Ok; PR state update, audit event and `publish_in_tx` share one transaction.

## Gates re-run by the reviewer

- `just verify-03b` (whole recipe incl. fuzz and bench): PASS. 674 tests passed, 0 failed, Docker up, `LK_REQUIRE_DOCKER=1`.
- `platform-fuzz`: five targets x 30 s on `nightly-2026-09-14`, no crash; deny and audit on the fuzz lockfile pass.
- `just verify`: first run FAIL on the unrelated flaky agent test (non-blocking 10); second run PASS (1621 passed, 0 failed; deny, audit, web verify green).
- `cargo xtask bench-check`: 14 pass, 0 fail, 23 unmet (other prompts' unregistered budgets). This PR's rows: P2 26.8 ms (10000), P7.blob_get_cached 0.000 ms (1), P7.blob_endpoint 0.007 ms (30), P11 40.0 ms (10000), P11.memory_mib 758.2 (1536), P13 5.46 ms (1000), P15.hub_ack_versions_per_s 20968 min (1000 floor).
- Not re-run: `docker build` of `crates/hub/Dockerfile`, `osv-scanner`, `#[ignore]` real-GCP tests.
