# 03b: Hub platform

<!-- Filled from plans/03b-hub-platform.evidence.md by /run-prompt. Title format: "03b: Hub platform" -->

## Prompt

`prompts/03b-hub-platform.md` · plan: `plans/03b-hub-platform.md` (approved by a human, with the listed contract change and Extra-paths) · wave: 1

The branch is up to date with `main` (`8c3a27c`: prompt 03a identity, plus contract PRs #6 and #8). It was brought in with a merge commit (`03b/merge: ...`), not a rebase and not a force push: the repo guard blocks force pushes, and 03a itself merged `main` into its branch. The 11 task commits `03b/T1` to `03b/T11` keep their history, followed by one `03b/blocked: A16` commit, the evidence, review-fix commits, the merge and the post-merge fixes.

### What the merge with main changed

- **Security fix, found by the merge.** PR #6 made `DomainEvent::UserSessionsRevoked` an internal event (`is_internal()`) and requires the SSE edge to drop internal events before the per-user filter. This branch's `RoleVisibility` ended in `_ => user.allows(Role::Viewer)`, so after a plain merge every active Viewer would have received it, live and on replay. Fixed (`03b/fix: drop internal events at the SSE edge`): one `deliverable` check on both the live and replay paths drops internal events before the per-user filter, `RoleVisibility` refuses them for every role, and `kinds=user_sessions_revoked` is a 400 (refused whole, so an emptied kind list can never mean "every kind"). In-process subscribers still receive the event, as the ports conformance suite requires. Tests were written first and failed (6 in `events_sse`, 2 in `events_sse_cluster`); there is a three-replica test that the event crosses the bus and reaches no browser.
- **Conflict resolutions** (Cargo.toml, Cargo.lock, Justfile, `perf/budgets.toml`, `budgets_registry.rs`, `docs/threat-model.md`): one pin per crate with the union of features (axum `http1 http2 json matched-path query tokio`, moka `future sync`, aws-lc-rs `aws-lc-sys alloc ring-sig-verify`); `prodash 31.0.0`, `quinn-proto 0.11.18` and `quinn-udp 0.5.15` re-pinned after cargo undid them; 17 budgets registered (the union, no threshold changed on either side); both threat-model sections kept.
- **`crates/hub-identity/fuzz/Cargo.lock` (03a's path) was regenerated.** After the axum feature union it no longer satisfied `cargo metadata --locked`, which `identity-fuzz` runs first. It pins `h2 0.4.19` and `tokio-util 0.7.19` to stay within the 14-day rule. This is a lockfile only; the reviewer judged it acceptable, but it is outside 03b's owned paths and outside the plan's Extra-paths, so it needs a human's nod.
- **`platform-bench` now also runs `cargo bench -p hub-identity`**, because `bench-check` covers the whole registry and 03a registered three budgets. Side effect: `just verify-03b` now depends on 03a's marginal P12 budget (see Gates).

## Gates

Real output is in `plans/03b-hub-platform.evidence.md`. All of the following were run on the merged tree, by the implementer and again independently by the round 3 reviewer.

| Command | Result |
|---|---|
| `just verify-03b` (tools check, fmt, clippy `-D warnings`, `platform-test`, `platform-slow-test`, `platform-fuzz`, `platform-bench`) | **exit 0.** `platform-test` 66 binaries, 626 passed, 0 failed, 4 ignored; `platform-slow-test` 57 passed (release, `--test-threads=1`); `platform-fuzz` 5 targets x 30 s on `nightly-2026-09-14` with `cargo-fuzz 0.13.2`, no crash; `platform-bench` green. |
| `just verify` | **exit 0**: `cargo test --workspace` 2157 passed, 0 failed, 6 ignored; `cargo deny check` ok; `cargo audit` clean (600 crates); web lint, typecheck, test and build pass. |
| `cargo xtask bench-check` | **exit 0: 17 pass, 0 fail, 23 unmet.** The 23 unmet are other prompts' budgets (`registered = false`, no benchmark yet). |
| `cargo tree -p hub-platform -i ring -e normal` | prints nothing. `ring` is only in the `hub` binary (see A15 below). |
| fuzz lockfiles (`fuzz`, `agent`, `hub-identity`, `hub-platform`) | `cargo audit --file` and `cargo deny --manifest-path` clean on all four. |

Honest record of the `just verify-03b` runs on the merged tree: run 1 failed in `platform-test` on a flake in my own round-1 test `lease_race::the_local_guard_never_outlives_the_database_lease` (it compared two readings taken at different moments; 1 failure in 15 runs under load with the code unchanged; the assertion was fixed and passed 40 of 40 under the same load). Run 2 passed everything up to `platform-bench`, which failed on **03a's** `P12.audit_record 3.190 ms p95 <= 3 ms`; five isolated runs on the merged tree gave 2.78 to 3.01 ms and five on a clean `origin/main` gave 2.53 to 3.03 ms, and 03a's own evidence calls the budget marginal ("Known gap 15"). Run 3 passed; the reviewer's run saw 2.530 ms. **The P12 budget is 03a's and marginal, so `just verify-03b` can fail at its last step on a noisy machine; that is for a human to settle.**

Two agent flakes, prompt 02's, hit CI on this PR (see "Agent fixes"): `crates/agent/tests/binary.rs:72` (the reported failure) and `crates/agent/tests/kube_watch.rs:310` (once on main). Both are fixed on this branch, in `2ad6d24`.

Not run: `docker build` of `crates/hub/Dockerfile` (the Dockerfile and `HARDENING.md` are linted by tests), `osv-scanner` (not installed; `cargo deny` and `cargo audit` stand in), the three `#[ignore]` real-Google-project tests in `gcp_real.rs`, and `just fuzz-smoke` / `just verify-03a` as such (03a's tests and benches ran inside `just verify` and `platform-bench`).

## Acceptance checklist

- [x] `just verify-03b` passes
- [x] P2, P13 and the blob-endpoint budget are within limits in `bench-check`
- [x] The three-instance test for routing passes (`three_hubs_agent_moves_during_traffic_no_request_lost`)
- [x] The three-instance test for events passes (`three_hubs_event_reaches_every_subscriber_within_1s`, `three_hubs_committed_event_delivered_once_per_subscriber`, `slow_consumer_gets_resync_instead_of_blocking_the_bus`)
- [x] The three-instance test for leases passes (`three_hubs_race_for_a_lease_exactly_one_holds_it`, `lease_taken_over_after_holder_dies`)
- [x] The CA key is only ever used through `KmsSigner`; a test fails if any code path loads a private CA key (`ca_key_use` tests, with the planted negative case and spy signer)
- [x] Webhook verification passes (including `webhook_replay_with_a_changed_delivery_header_is_still_a_replay`)
- [x] The fuzz targets pass (5 targets x 30 s in `platform-fuzz`; corpus replay on stable in `cargo test`)
- [x] Internal events never reach a browser (`an_internal_event_crosses_the_replicas_on_the_bus_and_reaches_no_browser` and the `events_sse` internal-event tests; see the caveat in item 1 of the open items)
- [x] T1 to T11 `Verify` items (each named test, bench or fuzz target is listed in the evidence file)

## Security requirements implemented (S#)

S5 (agent and sentinel identity, join tokens, 24 h certificates, SAN built by the hub), S6 (TLS 1.3 only, one pinned CA, Postgres `verify-full`), S7 (Secret Manager, KMS signer and envelope, bounded 5-minute cache), S10, S11 (body, depth, query and proto size limits), S12 (headers, CSP nonce), S16 (container hardening), S18 (host allowlist, HMAC-verified webhook with a replay guard), S21 (`forbid(unsafe_code)`, no panics), S22 (five fuzz targets with corpora). Shaping the work: S4, S9 (audit in the same transaction), S19 (`cargo deny`, `cargo audit`, and a manual 14-day age check of every lockfile entry the branch adds). The proofs are named in the evidence file, and `docs/threat-model.md` has a STRIDE section for the platform whose Proof cells are checked by `tests/threat_model.rs`.

## Benchmarks against budgets (P#)

From `cargo xtask bench-check` on the merged tree, round 3 reviewer's run (one machine, loopback Postgres, so GitHub, Cloud KMS and real network latency are not in these figures):

| P# | Benchmark | Result | Budget | Status |
|---|---|---|---|---|
| P2 | `hub-platform/push_to_drift_webhook` | 26.9 ms p95 | 10000 ms | PASS (webhook in to `GitHeadMoved` on another replica over the file transport; plan A3) |
| P7.blob_get_cached | `hub-platform/blob_get_cached` | 0.000 ms p95 | 1 ms | PASS (new entry) |
| P7.blob_endpoint | `hub-platform/blob_endpoint` | 0.005 ms p95 | 30 ms | PASS (new entry) |
| P11 | `hub/startup` | 35.9 ms p95 | 10000 ms | PASS |
| P11.memory_mib | `hub/memory_at_design_scale_mib` | 760.7 MiB max | 1536 MiB | PASS (platform share only, includes the 512 MiB blob cache; excludes 03a's session cache of up to 50,000 sessions, which prompt 16 must measure with the whole hub) |
| P13 | `hub-platform/sse_delivery` | 5.3 ms p95 | 1000 ms | PASS (3 replicas, 300 SSE clients, real sockets and Postgres) |
| P15.hub_ack_versions_per_s | `hub-platform/ack_throughput` | 20942 min | >= 1000 | PASS (new entry) |
| P12.audit_record (03a's) | `hub-identity` bench | 2.530 ms p95 | 3 ms | PASS here, marginal: 2.78 to 3.01 ms and one failing run at 3.190 ms were seen on the implementer's box |

The agent's budgets (P1, P3, P4.*, P15.spool_replay) also have results and pass; P1 is the closest margin (about 9.6 s of 15 s).

## New dependencies and why

Pins are exact and every one is at least 14 days old: 424 (name, version) pairs added or changed versus `origin/main` across `Cargo.lock`, `crates/hub-identity/fuzz/Cargo.lock` and `crates/hub-platform/fuzz/Cargo.lock` were looked up on crates.io by the round 3 reviewer; none is younger than 18 days and none is yanked. Review round 1 had found `prodash 31.0.1`, `quinn-proto 0.11.19` and `quinn-udp 0.5.16` too young; they are pinned to `31.0.0`, `0.11.18` and `0.5.15`.

| Crate | Version | Why |
|---|---|---|
| axum | 0.8.9 | the web edge; already in the graph through tonic (shared pin with 03a, feature union) |
| moka | 0.12.16 | bounded caches: rate-limit buckets, blob cache, tree-index cache (shared pin with 03a) |
| aws-lc-rs | 1.18.1 | request ids, CSP nonces and webhook HMAC from the provider already used by rustls and rcgen (shared pin with 03a) |
| opentelemetry, opentelemetry_sdk, opentelemetry-otlp | 0.33.0 | OTLP trace export for Cloud Trace |
| tracing-opentelemetry | 0.34.0 | bridge from `tracing` spans |
| base64 | 0.22.1 | Secret Manager and KMS speak base64 in JSON (shared pin with 03a) |
| rustls-native-certs | 0.8.4 | roots for `*.googleapis.com` (not `webpki-roots`, whose licence is not allowed) (shared pin with 03a) |
| gix | 0.87.1 | Git mirrors; feature `blocking-http-transport-reqwest-rust-tls` brings `reqwest 0.13.5` as a transitive dependency only |
| sqlx (hub only, feature `tls-rustls-ring-native-roots`) | 0.8.6 (unchanged) | Postgres TLS with full verification |

`cargo deny check` and `cargo audit` pass on the main lockfile and on all four fuzz lockfiles.

## Assumptions

Items 1 to 3 and 11 to 12 need an explicit human nod.

1. **A15, sqlx and ring.** Every sqlx variant on aws-lc-rs failed `cargo deny` or pulled `webpki-roots` (CDLA-Permissive-2.0), so the hub binary uses `tls-rustls-ring-native-roots` for the database leg only. `ring 0.17.14` is therefore compiled into the `hub` binary and rustls has two providers there; the hub's own listeners, the CA and rcgen use aws-lc-rs, and `hub-platform` alone has no ring. This departs from the Stack line "rustls with the aws-lc-rs provider".
2. **A16, gix (human decision at the plan gate, option 1).** `deny.toml` `[graph].targets` is narrowed to `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, and gix's reqwest-rust-tls feature is enabled. reqwest comes in transitively only. Side effect: Windows, macOS, Android and wasm32 dependency graphs are no longer checked by `cargo deny` for any crate. Recorded in `plans/03b-hub-platform.blockers.md`.
3. **A19, webhook body cap is 1 MiB** (the `api/openapi.yaml` number), not the plan's 4 MiB. It is stricter than S11; an oversized push gets 413 and the 60 s poll picks the change up.
4. `BlobPolicy::allowed` is `async` (05's real policy reads the registry).
5. A sentinel join is audited as `AuditAction::AgentJoined` (no sentinel-specific action exists).
6. The replay guard keys on the HMAC of the verified body (`hmac-sha256:<hex>`), not on the unsigned `X-GitHub-Delivery` header as the plan wording said (review round 1 finding). No migration was needed; `webhook_deliveries.delivery_id` holds these keys. Two distinct deliveries with byte-identical bodies count as one.
7. `ports::conformance::event_bus` assumes `publish` is visible synchronously; with the outbox relay (D80) delivery is async, so `tests/outbox.rs` runs the suite through a `Settled` wrapper. The suite should grow an async-settle hook in a `contract-change` PR.
8. The hub binary still refuses to start without a real `SessionAuth` and the 05 registry, and does not yet wire the replica listener, agent listener, job runner or the webhook `PrLinks` implementation. Those pieces are built and tested in `hub-platform`; `crates/hub/src/compose.rs` is the plug-in point (plan A5). Prompt 09 composes the binary.
9. `web/index.html` has no `__CSP_NONCE__` placeholder, so `StaticFiles::open` refuses the current web build; prompt 08 must add it.
10. The T11 relay fix (`events/relay.rs`: read missing ids after new rows) fixed a real ordering bug found in T5's relay.
11. The merge touched `crates/hub-identity/fuzz/Cargo.lock` (03a's path, outside the plan's Extra-paths); see "What the merge with main changed".
12. `platform-bench` runs 03a's bench too, so `just verify-03b` depends on 03a's marginal P12 budget.

Open items for a human, left unfixed on purpose (the round 3 reviewer found no blocking defect in any of them):

1. **SSE edge test hardening (non-blocking).** Every SSE test rig uses `RoleVisibility`, which also refuses internal events, so deleting the edge check at `events/sse.rs:493-495` would leave the tests green. `EventVisibility` is meant to be swapped by later modules, so a swapped-in rule that says yes to unknown variants could leak with CI green. Fix: re-run the live, replay and replayed-and-live internal-event cases with a test-local allow-all `EventVisibility`.
2. **`SessionAuth` is not what 03a ships.** 03a does not implement `hub_platform::SessionAuth` and does not depend on `hub-platform`; it ships its own `GuardedRouter` with CSRF. `still_valid(&UserId)` is per user but 03a's sessions are per cookie, so after a plain logout that browser's SSE stream stays open until the 30-minute maximum age, while `docs/threat-model.md:392` says it ends "within one heartbeat (15 s)" when the session ends. A disabled user or a role change is fine (03a deletes all of that user's sessions). Not exposed today (the hub refuses to start without a real `SessionAuth`), but the threat-model claim should be corrected, and the adapter needs a session handle or a changed `still_valid`.
3. **Duplication to settle in prompt 09:** two `GuardedRouter`s (03b's has no CSRF check and its `route()` accepts any method; fine today because only GET routes and the HMAC webhook use it; suggested hardening: refuse non-safe methods), two request-id layers, two outbound HTTPS clients and two metadata-server token sources (`hub-identity/src/http.rs` and `hub-platform/src/gcp/http.rs`), and 03a's `CounterSource::gauges()` metrics hand-off, which nothing in `crates/hub` reads yet.
4. **`git/pr_webhook.rs` `apply_link` (~line 240)** records an audit event and publishes per link (up to 500) in one transaction. With 03a's real audit log the chain lock is held from the first `record` to commit, stalling every audit writer and working against P12. Not an S9 violation. Fix before 09 wires 03a's `PgAuditLog`: do the updates and publishes first and `record` just before commit.
5. `PgTestAuditLog` (tests/support/pg.rs) is not chain-equivalent to 03a's `PgAuditLog` (test-only).
6. The lease test assertion fixed after the merge could be tightened further (take the local reading after the query returns).
7. `api/openapi.yaml` lists 400, 403 and 413 for the webhook, while the code answers a bare 401 for a bad signature; and its "delivery-id" wording (and `prompts/03b-hub-platform.md:120`) no longer matches the HMAC-keyed guard. Contract path.
8. `docs/decisions.md` has no note for A15 and A16 (not an owned path).
9. No no-op `Notifier` exists although the prompt's Interfaces row names it.
10. `crates/hub/src/limits.rs` buffers up to 4 MiB of any `application/json` body and scans its depth before the webhook's own 1 MiB check or HMAC. Bounded by the per-IP bucket and the 30 s timeout; worth a follow-up.
11. Relay residuals: the gap read happens only on the first loop iteration, so with a backlog above one batch and a concurrent publisher an order inversion is still possible (no event is lost); the regression test `events_bus.rs:111` is probabilistic.
12. Stale wording left by the replay-guard change: `docs/threat-model.md:269`, `crates/hub-platform/src/git/webhook.rs:100` and `crates/hub-platform/tests/pr_webhook.rs` (lines 315, 647) still say "delivery id".
13. A sentinel batch the sink rejects as `Invalid` or `TooLarge` gets no ack and no stream end (07's side).
14. `git_tree_index` has no retention; the gix transport trusts system roots and has a 30 s total timeout per request.
15. The plan proposed a separate `contract:` PR for `AuditAction::PrStateChanged` and the budget entries; both landed on this branch under the approved `Contract-change`. Confirm the merge route.
16. ~~The two flaky agent tests above need a prompt-02 follow-up.~~ Fixed on this branch in `2ad6d24`, with the human's Extra-paths approval; see "Agent fixes".

## Agent fixes

Two pre-existing races in `crates/agent` — prompt 02's ownership, not this prompt's — made CI red on this PR (`binary.rs:72`, the reported failure) and once on main (`kube_watch.rs:310`, run 38278983056). The human approved fixing both here (conversation, 2026-10-13) by adding `crates/agent/**` to the plan's Extra-paths for this. Commit `2ad6d24`, cross-ownership, test-support and startup-order only; no hub-platform behaviour touched. The full analysis is in `plans/03b-hub-platform.blockers.md`.

1. **SIGTERM race** (`binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm`). The health port opened before the shutdown future's first poll installed its signal handlers, so a SIGTERM in that window was the default kill (`status.code() == None`). `process::run` now polls the future once before the port binds. The reproduction script went 10/10 killed before, 10/10 orderly exit 0 after; the binary test file passes 5/5 batches. Known limit, unchanged and now stated in the module doc: a SIGTERM before `run()`'s first poll (between `exec` and the runtime hand-off) is still the default kill.
2. **`kube_watch` log assertion.** Not a timing race: `tracing` fixes each callsite's interest once per process, at the callsite's first hit, against whatever subscriber the hitting thread has then; with no global subscriber in a test binary, a callsite first hit by a capture-less test is cached "never" and stays silent in every later capture. `a_failing_list_is_retried_and_the_report_follows` (lists fail 500/503, no capture) could disable the agent's watch-failure `warn!` for the whole process, and then the log assertion read `[]` — deterministically at `--test-threads=1`, ~15% of runs under four threads (6/40 pinned to 2 vCPUs). `tests/support/log_capture.rs`'s `install` now installs a once-per-process keep-alive global (answers "always", drops events) and rebuilds the interest cache per install, so a capture always sees its own callsites; capture-less threads still drop their events, and no binary mixes `install` with `install_global`. After: `--test-threads=1` passes, 40 pinned four-thread batches 0 failures, `cargo test -p agent` green across all 55 binaries (so no test was passing on a silenced line).

## Reviewer agent verdict

Round 1: CHANGES REQUESTED (three lockfile pins younger than 14 days; fixed). Round 2 (head `3d7ec2c`): APPROVE. Round 3, after merging `main` (head `2434614`, covering the merge, the SSE fix and the other post-merge changes): no blocking findings.

```
VERDICT: APPROVE
```

Full text of all three rounds: `plans/03b-hub-platform.review.md`.

## Labels

`contract-change`: the plan's Contract-change was approved by a human. It covers `perf/budgets.toml` (P2, P11, P11.memory_mib and P13 registered; new P7.blob_get_cached, P7.blob_endpoint, P15.hub_ack_versions_per_s) and `crates/ports` (additive `AuditAction::PrStateChanged`). Approved Extra-paths: `crates/xtask/tests/budgets_registry.rs`, `deny.toml` and — added for `2ad6d24`, the agent fixes above — `crates/agent/**`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01Ha8WFk1edKYY8xg8Bm6vQo
