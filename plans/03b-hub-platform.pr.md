# 03b: Hub platform

<!-- Filled from plans/03b-hub-platform.evidence.md by /run-prompt. Title format: "03b: Hub platform" -->

## Prompt

`prompts/03b-hub-platform.md` · plan: `plans/03b-hub-platform.md` (approved by a human, with the listed contract change and Extra-paths) · wave: 1

Head: `6b0b768` on `agent/03b-hub-platform`. 11 task commits (`03b/T1` to `03b/T11`, the plan's count), one `03b/blocked: A16` commit, `03b/evidence`, five `03b/fix:` commits from review round 1, and the review and plan-bookkeeping commits.

## Gates

Real output is in `plans/03b-hub-platform.evidence.md`; the tail of each is below. `just` was run by hand where it was not installed at first; the final run used the recipes.

| Command | Result |
|---|---|
| `just verify-03b` (tools check, fmt, clippy `-D warnings`, `platform-test`, `platform-slow-test`, `platform-fuzz`, `platform-bench`) | **exit 0** on `3d7ec2c`, with Docker up and `LK_REQUIRE_DOCKER=1`. 618 tests in `platform-test`, 57 in `platform-slow-test` (release, `--test-threads=1`), 5 fuzz targets x 30 s on `nightly-2026-09-14` with `cargo-fuzz 0.13.2`, no crash. The round 2 reviewer re-ran the whole recipe independently: 675 tests, 0 failed, same fuzz result. |
| `just verify` | **exit 0**: 166 test binaries, 1622 passed, 0 failed, 6 ignored; `cargo deny check` ok (advisories, bans, licenses, sources); `cargo audit` clean (599 crates); web lint, typecheck, test, build pass. |
| `cargo xtask bench-check` | **exit 0: 14 pass, 0 fail, 23 unmet.** The 23 unmet are other prompts' budgets (`registered = false`, no benchmark yet). |
| `cargo tree -p hub-platform -i ring -e normal` | prints nothing. `ring` is only in the `hub` binary (see A15 below). |

Known flake, not 03b's: `crates/agent/tests/binary.rs:72` (`binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm`) failed once in each of two reviewer `just verify` runs and passed on re-run; `crates/agent/tests/kube_watch.rs` had a similar intermittent failure earlier. Both belong to prompt 02.

Not run: `docker build` of `crates/hub/Dockerfile` (the Dockerfile and `HARDENING.md` are linted by tests), `osv-scanner` (not installed; `cargo deny` and `cargo audit` stand in), and the three `#[ignore]` real-Google-project tests in `gcp_real.rs`.

## Acceptance checklist

- [x] `just verify-03b` passes
- [x] P2, P13 and the blob-endpoint budget are within limits in `bench-check`
- [x] The three-instance test for routing passes (`three_hubs_agent_moves_during_traffic_no_request_lost`)
- [x] The three-instance test for events passes (`three_hubs_event_reaches_every_subscriber_within_1s`, `three_hubs_committed_event_delivered_once_per_subscriber`, `slow_consumer_gets_resync_instead_of_blocking_the_bus`)
- [x] The three-instance test for leases passes (`three_hubs_race_for_a_lease_exactly_one_holds_it`, `lease_taken_over_after_holder_dies`)
- [x] The CA key is only ever used through `KmsSigner`; a test fails if any code path loads a private CA key (`ca_key_use` tests, with the planted negative case and spy signer)
- [x] Webhook verification passes (including `webhook_replay_with_a_changed_delivery_header_is_still_a_replay`)
- [x] The fuzz targets pass (5 targets x 30 s in `platform-fuzz`; corpus replay on stable in `cargo test`)
- [x] T1 to T11 `Verify` items (each named test, bench or fuzz target is listed in the evidence file)

## Security requirements implemented (S#)

S5 (agent and sentinel identity, join tokens, 24 h certificates, SAN built by the hub), S6 (TLS 1.3 only, one pinned CA, Postgres `verify-full`), S7 (Secret Manager, KMS signer and envelope, bounded 5-minute cache), S10, S11 (body, depth, query and proto size limits), S12 (headers, CSP nonce), S16 (container hardening), S18 (host allowlist, HMAC-verified webhook with a replay guard), S21 (`forbid(unsafe_code)`, no panics), S22 (five fuzz targets with corpora). Shaping the work: S4, S9 (audit in the same transaction), S19 (`cargo deny`, `cargo audit`, and a manual 14-day age check of every lockfile entry the branch adds). The proofs are named in the evidence file, and `docs/threat-model.md` has a STRIDE section for the platform whose Proof cells are checked by `tests/threat_model.rs`.

## Benchmarks against budgets (P#)

From `cargo xtask bench-check` on the fixed tree (one machine, loopback Postgres, so GitHub, Cloud KMS and real network latency are not in these figures):

| P# | Benchmark | Result | Budget | Status |
|---|---|---|---|---|
| P2 | `hub-platform/push_to_drift_webhook` | 32.6 ms p95 | 10000 ms | PASS (webhook in to `GitHeadMoved` on another replica over the file transport; plan A3) |
| P7.blob_get_cached | `hub-platform/blob_get_cached` | 0.001 ms p95 | 1 ms | PASS (new entry) |
| P7.blob_endpoint | `hub-platform/blob_endpoint` | 0.007 ms p95 | 30 ms | PASS (new entry) |
| P11 | `hub/startup` | 40.2 ms p95 | 10000 ms | PASS |
| P11.memory_mib | `hub/memory_at_design_scale_mib` | 746 MiB max | 1536 MiB | PASS (platform share only, includes the 512 MiB blob cache) |
| P13 | `hub-platform/sse_delivery` | 6.2 ms p95 | 1000 ms | PASS (3 replicas, 300 SSE clients, real sockets and Postgres) |
| P15.hub_ack_versions_per_s | `hub-platform/ack_throughput` | 20851 min | >= 1000 | PASS (new entry) |

The agent's budgets (P1, P3, P4.*, P15.spool_replay) also have results now and pass; P1 is the closest margin (about 9.6 s of 15 s).

## New dependencies and why

Pins are exact and every one is at least 14 days old (checked against crates.io for the 101 lockfile entries the branch adds; the youngest is 17 days). Review round 1 found `prodash 31.0.1`, `quinn-proto 0.11.19` and `quinn-udp 0.5.16` too young; they were downgraded to `31.0.0`, `0.11.18` and `0.5.15` in both lockfiles.

| Crate | Version | Why |
|---|---|---|
| axum | 0.8.9 | the web edge; already in the graph through tonic |
| moka | 0.12.16 | bounded caches: rate-limit buckets, blob cache, tree-index cache |
| aws-lc-rs | 1.18.1 | request ids, CSP nonces and webhook HMAC from the provider already used by rustls and rcgen |
| opentelemetry, opentelemetry_sdk, opentelemetry-otlp | 0.33.0 | OTLP trace export for Cloud Trace |
| tracing-opentelemetry | 0.34.0 | bridge from `tracing` spans |
| base64 | 0.22.1 | Secret Manager and KMS speak base64 in JSON |
| rustls-native-certs | 0.8.4 | roots for `*.googleapis.com` (not `webpki-roots`, whose licence is not allowed) |
| gix | 0.87.1 | Git mirrors; feature `blocking-http-transport-reqwest-rust-tls` brings `reqwest 0.13.5` as a transitive dependency only |
| sqlx (hub only, feature `tls-rustls-ring-native-roots`) | 0.8.6 (unchanged) | Postgres TLS with full verification |

`cargo deny check` and `cargo audit` pass on the main lockfile and the hub-platform fuzz lockfile.

## Assumptions

Items 1 to 3 and the open items below need an explicit human nod.

1. **A15, sqlx and ring.** Every sqlx variant on aws-lc-rs failed `cargo deny` or pulled `webpki-roots` (CDLA-Permissive-2.0), so the hub binary uses `tls-rustls-ring-native-roots` for the database leg only. `ring 0.17.14` is therefore compiled into the `hub` binary and rustls has two providers there; the hub's own listeners, the CA and rcgen use aws-lc-rs, and `hub-platform` alone has no ring. This departs from the Stack line "rustls with the aws-lc-rs provider".
2. **A16, gix (human decision at the plan gate, option 1).** `deny.toml` `[graph].targets` is narrowed to `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, and gix's reqwest-rust-tls feature is enabled. reqwest comes in transitively only. Side effect: Windows, macOS, Android and wasm32 dependency graphs are no longer checked by `cargo deny` for any crate. Recorded in `plans/03b-hub-platform.blockers.md`.
3. **A19, webhook body cap is 1 MiB** (the `api/openapi.yaml` number), not the plan's 4 MiB. It is stricter than S11; an oversized push gets 413 and the 60 s poll picks the change up.
4. `BlobPolicy::allowed` is `async` (05's real policy reads the registry).
5. A sentinel join is audited as `AuditAction::AgentJoined` (no sentinel-specific action exists).
6. The replay guard keys on the HMAC of the verified body (`hmac-sha256:<hex>`), not on the unsigned `X-GitHub-Delivery` header as the plan wording said (review round 1 finding). No migration was needed; `webhook_deliveries.delivery_id` holds these keys. Two distinct deliveries with byte-identical bodies count as one.
7. `ports::conformance::event_bus` assumes `publish` is visible synchronously; with the outbox relay (D80) delivery is async, so `tests/outbox.rs` runs the suite through a `Settled` wrapper. The suite should grow an async-settle hook in a `contract-change` PR.
8. The hub binary still refuses to start without the real 03a (identity) and 05 (registry) modules, and does not yet wire the replica listener, agent listener, job runner or the webhook `PrLinks` implementation. Those pieces are built and tested in `hub-platform`; `crates/hub/src/compose.rs` is the plug-in point (plan A5).
9. `web/index.html` has no `__CSP_NONCE__` placeholder, so `StaticFiles::open` refuses the current web build; prompt 08 must add it.
10. The T11 relay fix (`events/relay.rs`: read missing ids after new rows) fixed a real ordering bug found in T5's relay.

Open items for a human, left unfixed on purpose (the round 2 reviewer found none that violates a security requirement):

- `api/openapi.yaml` lists 400, 403 and 413 for the webhook, while the code answers a bare 401 for a bad signature; and its "delivery-id" wording (and `prompts/03b-hub-platform.md:120`) no longer matches the HMAC-keyed guard. Contract path.
- `docs/decisions.md` has no note for A15 and A16 (not an owned path).
- No no-op `Notifier` exists although the prompt's Interfaces row names it.
- `crates/hub/src/limits.rs` buffers up to 4 MiB of any `application/json` body and scans its depth before the webhook's own 1 MiB check or HMAC. Bounded by the per-IP bucket and the 30 s timeout; worth a follow-up (exempt `/hooks/github` or pass the 1 MiB cap).
- Relay residuals: the gap read happens only on the first loop iteration, so with a backlog above one batch and a concurrent publisher an order inversion is still possible (no event is lost); the regression test `events_bus.rs:111` is probabilistic.
- Stale wording left by the replay-guard change: `docs/threat-model.md:269`, `crates/hub-platform/src/git/webhook.rs:100` and `crates/hub-platform/tests/pr_webhook.rs` (lines 315, 647) still say "delivery id".
- A sentinel batch the sink rejects as `Invalid` or `TooLarge` gets no ack and no stream end (07's side).
- `git_tree_index` has no retention; the gix transport trusts system roots and has a 30 s total timeout per request.
- The plan proposed a separate `contract:` PR for `AuditAction::PrStateChanged` and the budget entries; both landed on this branch under the approved `Contract-change`. Confirm the merge route.
- The two flaky agent tests above need a prompt-02 follow-up.

## Reviewer agent verdict

Round 1: CHANGES REQUESTED. One blocking finding (three lockfile pins younger than 14 days); fixed in `254ce27`. Round 2 (head `3d7ec2c`), no blocking findings:

```
VERDICT: APPROVE
```

Full text of both rounds: `plans/03b-hub-platform.review.md`.

## Labels

`contract-change`: the plan's Contract-change was approved by a human. It covers `perf/budgets.toml` (P2, P11, P11.memory_mib and P13 registered; new P7.blob_get_cached, P7.blob_endpoint, P15.hub_ack_versions_per_s) and `crates/ports` (additive `AuditAction::PrStateChanged`). Approved Extra-paths: `crates/xtask/tests/budgets_registry.rs` and `deny.toml`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01Ha8WFk1edKYY8xg8Bm6vQo
