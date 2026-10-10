<!-- PR title: "02: Agent" · branch agent/02-agent · label: contract-change (the plan header says Contract-change: APPROVED) -->

## Prompt

`prompts/02-agent.md` · plan: `plans/02-agent.md` (approved by a human) · wave: 1

The in-cluster agent (`crates/agent`): joins the hub and gets a certificate, keeps an mTLS gRPC stream, scans the NFS export with a Merkle tree, serves hash-checked file operations through cap-std, watches Kubernetes, spools to disk through hub outages, tags changes made during a sync Job, applies deny globs, and talks to the config-server only on a hub command. Includes the agent threat model (`docs/threat-model.md`, `## Agent (02)`).

**Depends on a merged contract PR.** The wire additions (`SpoolGap` on `ScanDelta`, `NotifyResult`, S5 SAN wording) are already on `main` as `67acb04`, `c2a2717`, `4d6aca4` (`contract(02):`). This branch merged `origin/main` in (`8f850c7`); its diff against `main` contains no contract file except `perf/budgets.toml`.

## Gates

All run fresh by the implementer's FINAL step, and re-run by the reviewer agent. All exit 0.

| Command | Result |
|---|---|
| `just verify-02` | PASS, 35 min 47 s. `agent-test`: 742 passed, 0 failed, 1 ignored. `agent-slow-test`: `p1_change_visible_real_timings` ok (123.86 s). `agent-fuzz`: deny and audit on the fuzz lock clean, 6 targets x 30 s, no crash. `agent-bench` ok. |
| `just verify` | PASS, 3 min 42 s. fmt, clippy `-D warnings`, `cargo test --workspace` (1003 passed, 0 failed, 2 ignored), `cargo deny` ok, `cargo audit` (500 crates) no findings, web lint, typecheck, vitest and build clean. |
| `cargo xtask bench-check` | PASS: `7 pass, 0 fail, 27 unmet of 34 budgets`. The 27 unmet rows are other prompts' budgets. |

Full output summaries: `plans/02-agent.evidence.md`.

**Not run / not provable in this environment** (no Docker daemon, no real cluster, no NFS): `docker build` of `crates/agent/Dockerfile` (never built; the reviewer resolved both base-image digests and they match); `db-verify` and `verify-01`'s `docker-check`; `osv-scanner` (not installed); `just fuzz-smoke`; `fixtures-verify`; a real NFS mount test (rename atomicity, attribute caching); the GKE metadata server on a real node (Q26). CI or a human must cover these.

## Acceptance checklist

- [x] `just verify-02` passes, and P1, P3 and P4 are within budget according to `bench-check` (local disk, in-process harness; see A15).
- [x] Idle traffic is under 1 KB per minute, measured with the fake hub over 10 minutes (672 B/min; `p3_idle_traffic_under_1kb_per_min`).
- [x] Every file operation goes through cap-std, and the traversal and fuzz tests pass (`all_file_access_via_cap_std`, `traversal_corpus_never_escapes_root`, `symlinked_*_refused`, fuzz `agent_path`).
- [x] The RBAC manifest grants no Secret list (`rbac_manifest_has_no_secret_list` and three more; Secrets are `get`/`update` on one named Secret and `get` on one named join-token Secret).

## Security requirements implemented (S#)

S5 (join, identity, certificate; SAN `spiffe://lanekeeper/swimlane/<id>` in one constant), S6 (TLS 1.3 only, pinned CA, zstd, bounded queues), S10 (logs carry paths, hashes and ids only), S11 (typed paths, bounded input before any I/O), S16 (distroless non-root image, `HARDENING.md` contract), S17 (no shell or exec, cap-std, no symlinks followed, minimal RBAC, deny globs, env allowlist, config-server limits), S21 (no unsafe, no panics, no leaked internals), S22 (six fuzz targets). Each S# is traced to code and named tests in `plans/02-agent.evidence.md`; the reviewer traced them independently (`plans/02-agent.review.md`).

## Benchmarks against budgets (P#)

`cargo xtask bench-check` after `cargo bench -p agent`, fixtures `cargo xtask gen-fixtures --scale target`, swimlane 1 (2,003 files).

| P# | Result | Budget |
|---|---|---|
| P1 | PASS 9,590 ms p95 | <= 15,000 ms |
| P3 | PASS 672 B/min | <= 1,024 B/min |
| P4 (full rehash, 2,000 files) | PASS 6.3 ms | <= 20,000 ms |
| P4.stat_walk | PASS 3.0 ms | <= 1,000 ms |
| P4.cpu_mcores | PASS 4 mCPU | <= 50 mCPU |
| P4.memory_mib | PASS 22.8 MiB | <= 64 MiB |
| P15 | PASS 382,516 versions/s | >= 1,000 versions/s |

All numbers are local disk, in-process, no NFS latency (assumption A15); the stat walk over a real export may exceed 1 s and needs pilot numbers. P1 has about 5 s of headroom, with the 10 s walk interval as the dominant term.

**Fresh-checkout note (A26, accepted).** Seven budgets are now `registered = true`, so `cargo xtask bench-check` (and `just verify-01`'s `perf-verify`) fails on a fresh checkout until `cargo bench -p agent` has run (the benches need `target/fixtures/target`). CI (`ci.yml`) does not run benches, so CI stays green.

## New dependencies and why

All exact-pinned (`=x.y.z`); every one of the 99 package versions that `Cargo.lock` gains is at least 18 days old (youngest: `pest*` 2.9.2; 2.9.3 was 1 day old and is held back). `cargo deny` and `cargo audit` are clean on the root lock (500 crates) and the fuzz crate's own lock (301 crates). `cargo tree -i ring -p agent` prints nothing.

`cap-std` 4.0.3 (all file access, S17), `globset` 0.4.20 (ignore and deny globs; the `ignore` crate is not used, D89), `rcgen` 0.14.10 and `x509-parser` 0.18.1 and `pem` 3.0.6 (key, CSR, certificate check, S5), `kube` 4.0.0 with `k8s-openapi` 0.28.0 (watchers, restart), `crc32fast` 1.5.0 (spool checksums), `prometheus-client` 0.25.1 (`/metrics`), `tracing-subscriber` 0.3.23 `json` feature (logs), `rayon` 1.12.0 (hashing pool), dev: `criterion` 0.8.2, `serde-saphyr` 0.0.27. Full table with ages: evidence file.

Lockfile notes:
- **`kube` is 4.0.0, not 4.2.0.** 4.2.0 needs rustc 1.89 and `rust-toolchain.toml` pins 1.88.
- **`smallvec` goes from 1.16.1 to 1.15.2** (commit `170c4a1`; the T2 commit message omits it). Not the age rule: `kube-client 4.0.0` pulls `serde-saphyr 0.0.27` and `granit-parser 0.0.3`, both declaring `smallvec < 1.16.0`. Forced and justified; `cargo audit` is clean on 1.15.2.
- `deny.toml` is unchanged: the `aws-lc-sys` licence exception listed as an Extra-path was not needed.

## Assumptions

Human decisions recorded in the plan: A1 own cap-std walker (**D89**, appended to `docs/decisions.md`), A2 `get` on exactly two named Secrets, A4 new `NotifyResult` message, A5 SAN `spiffe://lanekeeper/swimlane/<id>`, A12 `WriteFile` never creates parent directories, A14 P3 counts TLS ciphertext both ways, A16 own fuzz crate `crates/agent/fuzz/`, A17 the Dockerfile lives in `crates/agent`, A25 contract edits in their own PR, A26 budget registration in this PR. Defaults: see the plan's table (A3, A6 to A11, A13, A15, A18 to A24).

Implementer assumptions, for the reviewer and the hub-side owners:
- **`k8s-openapi` feature `v1_35` is a guess at the GKE minor version.** Confirm it on the pilot cluster.
- Own hyper HTTP/2 client over rustls instead of tonic's `Channel`, so TLS 1.3 is the only version and the pinned hub CA is the only root.
- T9 rewrote `scan_session.rs::what_changed_while_disconnected_*` because it asserted pre-spool behaviour; the reviewer judged it legitimate (it asserts more, not less).
- Non-2xx `FetchServed` bodies are dropped (status only), so config-server error pages cannot carry denied names. `Accept` text/binary follows `docs/domain-model.md` by extension (the contract has no binary flag).
- Spool: bytes spooled before a path became denied stay in the segment file until acked, are stripped on replay, and so are never sent (residual risk in the threat model).
- Deny matches names in the request and paths only. A secret inside `app.yml`, or a file under a denied folder that the request does not name, is not caught.
- T6: terminating pods and pods in Succeeded or Failed are not reported; init-container env is not read; pods match a Deployment by label selector (no ReplicaSet watch).
- Three more fuzz targets than the plan named (`agent_cert_chain`, `agent_pem`, `agent_id_token`) beyond `agent_path`, `agent_hub_message` and `agent_spool_record`.
- The plan promised three insta goldens; only `merkle_encoding_golden` exists. The notify request and the cluster report are pinned by exact-byte and field assertions instead.

**Contract and Extra-path usage.** `docs/decisions.md`: one appended line (D89). `perf/budgets.toml`: six `registered` flips and one new `P4.stat_walk` block, no threshold changed. `docs/performance.md`: one sentence added to P4 ("A stat walk of 2,000 files takes under 1 second."), `crates/xtask/tests/budgets_registry.rs`: the "nothing registered" test became an explicit list of the seven registered ids. The last two were added to the plan's Extra-paths by the human (commit `f0b55a0`) to resolve blocker B1 (`plans/02-agent.blockers.md`). That edit is the only change to the plan after its approval; **the human should re-confirm it at merge.** `ScanDelta` has five struct literals, not the four the plan counted; the fifth (`crates/domain/tests/dto.rs`) was added to the contract PR by the human.

**For other prompts.**
- **05:** the Merkle byte encoding is pinned by `merkle_encoding_golden` and needs a line in `docs/domain-model.md` (not owned by 02). `ReportSink::delta` should treat `ScanDelta.gap` as "history lost between from and to; compare roots and request a full scan" and use checked subtraction on `to - from`. Apply denial retroactively (a glob change does not alter the Merkle root). A Closed sync window only arrives after every tagged delta of that connection; window events are unacknowledged and repeated on reconnect for one hour. Anyone who can create a Job matching `*dataload*` can open a sync window, so the hub must bound how long it holds alerts.
- **03b:** hand `AgentReply::Notify` to the waiting caller by request id; pass `ScanDelta.gap` through; issue the SAN `spiffe://lanekeeper/swimlane/<id>` and refuse the old `agent/` form; delivery is at-least-once, so tolerate a repeated sequence number; after a certificate renewal the agent reconnects once (A22). Deploy the hub before agents (an older hub ignores `notify_result`).
- **06:** read `AgentReply::Notify { status }`, treat 2xx as success and record the status in the audit event.
- **09 / 15:** add `just agent-fuzz` to CI (`just fuzz-smoke` does not run this crate, A16); build `crates/agent/Dockerfile` in CI (it has never been built; watch `aws-lc-sys` compiling in the builder and the `rust-toolchain.toml` components); the pod spec, NetworkPolicy and PVC follow `crates/agent/HARDENING.md`.

**Known residual risks (recorded in the threat model):** the stale cluster-delta window at reconnect (a delta blocked in the 8-slot report channel); a FIFO swapped in between lstat and open ties up a pool thread and its lock stripe; NFS has no compare-and-swap between the hash check and the replace; deny globs match by name only; RBAC `patch` on Deployments cannot be limited to the annotation; the service-account token is mounted. **Do not pilot against real tenant data before the image is built and the NFS behaviour is proved.**

## Reviewer agent verdict

Round 1 of at most 3. Full text: `plans/02-agent.review.md`.

```
VERDICT: APPROVE

Blocking: none.

Non-blocking:
1. smallvec 1.16.1 -> 1.15.2 is forced by kube-client 4.0.0 (serde-saphyr/granit-parser need smallvec < 1.16.0), not by the age rule. Justified; state it in the PR. (done above)
2. plans/02-agent.md was edited after approval by f0b55a0 (human-directed Extra-paths); the human should re-confirm at merge.
3. bench-check fails on a fresh checkout until `cargo bench -p agent` has run (A26 accepted; CI does not run benches). (done above)
4. Only 1 of the 3 promised insta goldens exists; the others are exact-byte assertions.
5. docs/threat-model.md: "hub ... never speed it past the budgets" is imprecise (scan_interval clamps to 5-300 s); the budgets still hold. Wording to fix.
6. Nightly warns `unreachable_code` at src/scan.rs:351 and src/app.rs:357, :370, :379; a toolchain bump under -D warnings would fail.
7. Dockerfile never built; digests re-resolved and match. CI must show aws-lc-sys builds and the rust-toolchain components fetch.
8. Merkle byte encoding must be relayed to 05 and docs/domain-model.md. (done above)
9. P-numbers are local-disk, in-process (A15); agent-fuzz is not in CI (A16).

Re-run by the reviewer: fmt, clippy -D warnings, cargo test -p agent (742 passed), just verify (1003 passed), just verify-02 (exit 0, 2196 s), cargo deny, cargo audit, cargo xtask bench-check (7 pass, 0 fail, 27 unmet). Not run: docker build, osv-scanner, verify-01, fuzz-smoke, real NFS test.
```

## Labels

`contract-change`: the plan header says `Contract-change: APPROVED` (limited by the plan to `perf/budgets.toml`). A human must approve it.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01SZ3cPVtRsS5p9PN6JFGW9M
