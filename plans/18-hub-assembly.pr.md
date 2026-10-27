<!-- PR body for prompt 18. Title: "18: Hub assembly". Branch: agent/18-hub-assembly. Not pushed; no PR is open. Label: contract-change (the plan's CC1 was approved by a human). -->

## Prompt

`prompts/18-hub-assembly.md` · plan: `plans/18-hub-assembly.md` (approved by a human, commit `53cb876`, with its `Extra-paths:` and CC1) · wave: 3

`hub` was a binary that read its settings and exited 1: nothing linked the identity module, the registry, the write paths or the MCP server in, and no listener, migration, signal handler or job was started. It is now a server: `hub` (serve), `hub migrate`, `hub seed <file> [--dry-run]` and `hub ca-init`. The human's answers at the plan gate are in `plans/18-hub-assembly.answers.md` (G2 per-session stream end, G8 the 12 MiB edit routes, G20 no merge order with prompt 09).

Eight task commits `18/T1` to `18/T8`, in order; then the FINAL step, three `18/fix:` commits found by the first real fuzz run, two review rounds, a review-fix round, and three merges of `origin/main` (the last one brings in #39). **The branch also carries `3e68646`** (`prompts/18-hub-assembly.md`, `prompts/README.md`, the base of `docs/hub-assembly-survey.md`) from `docs/18-hub-assembly-prompt`, which has no PR yet: settle which goes first.

## Gates

Details, with output: `plans/18-hub-assembly.evidence.md`.

- **Not run as single commands: `just verify-18` and `just verify`.** About 24 GB of debug test binaries do not fit the sandbox disk. Each sub-recipe and each step ran separately and passed, except the one B1 failure that is now resolved (below). **CI must run both commands green before this merges.**
- `just hub-lint`: pass (fmt, clippy `-D warnings` on all targets, release graph free of the 7 test features, `cargo deny`, `cargo audit`: 10 allowed zstd-safe warnings, D135).
- `hub-test`: `cargo test -p hub --all-features` one target at a time: **522 passed, 0 failed** (lib 236 + 34 test files, 286); `events_sse` 33 passed; `ca_key_use` 6 passed; `budgets_registry` 8 passed.
- `just hub-fuzz` (`nightly-2026-09-14`, `cargo-fuzz 0.13.2`): `hub_cli_args`, `hub_pem`, `hub_seed_file`, `hub_settings` ran 30 s each, no crash. **The first real run of these targets found two product bugs in `decode_ca_certificate` (a glued second certificate marker was skipped; a CA certificate that ends before it begins was accepted) and an unsound check in the fuzz harness itself**; all fixed test first (`13a974a`, `3532d31`, `31588fc`). Both product changes only add refusals; no valid input changed.
- `just hub-bench`: pass. `cargo clippy --workspace --all-targets -- -D warnings`: pass. `web-verify` (FINAL step): 391 files, 9,683 tests passed.
- `cargo test --workspace` (FINAL step, in slices): 5,826 passed, **1 failed (B1)**; that test passes now, and the whole command was **not** re-run after the merge. The reviewer re-ran all of `hub`, `hub-platform`, `hub-mcp` and workspace clippy in round 2.
- **NOT RUN:** `osv-scanner`, the kind test, helm / kubeconform / conftest and prompt 09's chart checks (not installed; prompt 09 has not merged), `docker build`, anything that needs a real Google project, Entra tenant or GitHub (every test has software at the edges), `just dev` as a whole (its pieces ran by hand), and the 20 existing `#[ignore]` tests.

### B1 (resolved)

`just verify` was red on one test of prompt 03b: its S5 source scan forbids any private key built from bytes in the hub, and the web listener's TLS key (plan G3, read from Secret Manager) tripped it. The human chose to allowlist exactly `web_tls.rs::decode` (option A). The allowlist line is **#39, merged at `5ae2ec0`**; this branch has main merged in (`f972c6b`) and the scan passes (6 of 6, also checked against the real `web_tls.rs`; a second private-key constructor added next to it still fails the scan). Decision D145 records the choice and the rejected alternatives. The residual (rustls keeps its own copy of that one key in memory) is in the threat model.

## Acceptance checklist

- [ ] `just verify-18` passes, and `just verify` passes. **Not ticked: not proved in the sandbox** (see Gates). CI must prove it.
- [x] The binary starts against a real Postgres, serves `/readyz` 200, and exits cleanly on SIGTERM within the grace period (proved with the `hub_dev` example, the same `hub::process::entry`, because the release binary cannot start without Google, G23).
- [x] It refuses to start, with a typed message and no secret, when the database, the CA, the identity settings or a required module is missing; no fake is reachable from a release build.
- [x] One test proves every operation of `api/openapi.yaml` is mounted with a guard or named in the allowance list, and every state-changing route is behind CSRF and `Origin` checks (also over the real `Running::routes()`).
- [x] An agent whose swimlane was seeded joins through the real agent listener and its report reaches a registry read.
- [x] `hub migrate` and `hub seed` are idempotent, audited where they change state, and refuse the wrong role. **`hub migrate` writes no audit event, by the human's decision (D138): the one stated exception to rule 8.**
- [x] Start-up is under 10 s with the real composition (p95 143 to 153 ms across runs, software edges), and memory is recorded against P11.
- [x] `docs/threat-model.md` has the section "Hub assembly (18)"; D137 to D145 are in `docs/decisions.md`.

## Security requirements implemented (S#)

S1, S2, S3, S4, S5, S6, S7, S9, S10, S12, S16, S18, S21 (the prompt's list), each traced to code and to a test in the evidence file. Also touched: **S11** (D137, below) and S22 (four fuzz targets).

- **D137 relaxes S11 for exactly two routes, at the human's decision (plan gate, G8).** `PUT /files` and `POST /drafts` take a body of up to 12 MiB + 64 KiB, only for a live session of an Active user with at least the Editor role; every other route and every request without such a session keeps 4 MiB; the GitHub webhook is 1 MiB. Narrower than the answer in one way (Editor or above, not any session) and bounded in another (at most 8 large bodies in flight per replica, about 100 MiB). Tests: `body_limits::*`, including `at_most_eight_large_bodies_are_read_at_once` (watched red with the control removed). **S11's wording in `docs/security.md` still says 4 MiB and needs a follow-up by whoever owns that file.**
- **G2 per-session stream end** uses the three approved `hub-platform` files; `SessionAuth::binding_valid` refuses by default so an implementation that names a session and cannot answer for it fails closed.
- **Residuals stated in the threat model, not fixed:** an unaudited `hub migrate`; the `hub seed` operator is not recorded (only the file's SHA-256); up to 12 MiB buffered per edit request (8 at once); MCP session affinity across replicas is unverified on GCP; identity's own router keeps an inner CSRF check on about ten routes (G1); the two CA references are compared as written (a project id versus a project number for one key is not caught); 33 admin tables have no route and no owner.

## Benchmarks against budgets (P#)

Software edges, release profile, target-scale fixtures; real Google latency is not in these numbers. Registered in `perf/budgets.toml` (CC1, two additive entries, nothing loosened).

| P# | Benchmark | Result (p95) | Budget |
|---|---|---|---|
| P11 | `hub/startup` through `hub::process::run` | 143 to 153 ms | 10,000 ms |
| P11 memory | `hub-platform/memory_at_design_scale` | 733 MiB | 1,536 MiB |
| P7 (new) | `hub/auth_composed_warm` (`P7.auth_composed`) | 0.027 to 0.040 ms | 2 ms |
| P7 (new) | `hub/auth_composed_cold` (`P7.auth_composed_cold`) | 1.18 to 1.42 ms | 2 ms |
| P2 | `hub-platform/push_to_drift_webhook` | 23.5 to 32.4 ms | 10,000 ms |
| P13 | `hub-platform/sse_delivery` | 4.6 to 5.4 ms | 1,000 ms |
| P12 | `audit_record` / `write_queries` / `steady_state_queries` | 2.3 / 9.7 / 16.6 ms | 4 / 50 / 50 ms |

The cold cookie path has about 2x margin, and prompt 03a's own cold bench once hit 2.29 ms on a contended VM: prompt 16 should watch it. P11 is measured with an empty registry, so "a database of the fixture size" is not literally met; the whole hub at design scale is prompt 16's, the real edges prompt 09's kind test. `cargo xtask bench-check` on the whole registry gives 11 pass, 62 "registered but no result" (other prompts' benchmarks, not run) and 2 unmet (prompt 14).

## New dependencies and why

The root `Cargo.lock` gains **no package**, only edges. All pinned versions are at least 14 days old (checked against the registry); `cargo deny` passes on the root and on the new hub fuzz lockfile.

- `serde-saphyr` 1.1.0 (73 days; **now a production dependency of `hub`**, a workspace pin): `hub seed` reads an operator-written file with hard budgets (events, nodes, depth, aliases) and no unsafe code.
- `x509-parser` 0.18.1, `zeroize` 1.9.0, `fastrand` 2.5.0, `pem` 3.0.6: the web certificate's `notAfter` for `/readyz`; wiping the PEM and DER copy of the web key (S7); jitter for the certificate reload; the PEM `hub ca-init` prints.
- `sqlx` feature `migrate` (no new package): `hub migrate`.
- dev only: `criterion` 0.8.2, `url` 2.5.8, `base64` 0.22.1, `hyper`, `tokio-stream`, `tonic` (`codegen`), `proto`, `hub-writes/test-doubles`.
- `crates/hub/fuzz` has its own lockfile; 499 of its 501 external packages match the root lockfile by version and checksum, the other two are `arbitrary` 1.4.2 and `libfuzzer-sys` 0.4.13, as the other fuzz crates pin them.
- `osv-scanner` was **not run** here.

## Assumptions

- The human's answers changed three questions (G2, G8, G20); every other question the plan marked "needs a human" took the plan's default, and so did the unmarked ones (D137 to D145 record them).
- `hub seed`: create and update only, never delete (rows absent from the file are reported); unchanged rows write nothing; a carried field that differs is updated, **including a swimlane's `project`, which moves it** (`--dry-run` shows it first). For swimlanes that already run, the real seed file should **omit** `cluster`, `nfs_server`, `nfs_export` and `mount_root` (the agent's first hello fills them when empty). Swimlanes and sentinels must carry their service-account address or Join is refused. It runs as `lanekeeper_app` only.
- Example-only development helpers (`examples/support/dev_login.rs`, `examples/dev_pki.rs`, `examples/dev/`) exist for `just dev`; they are never built into the image and the dev login refuses a database that is not on this machine.
- **What prompt 09's Helm chart and install guide must match** (all in `crates/hub/HARDENING.md`): the settings and secrets of T1 to T7 (web certificate and key, the two CAs' KMS keys and certificates, `hub ca-init` once per CA before the first rollout, Entra and public origin, the audit checkpoint bucket and key, the write vault KMS key, the Git tokens and webhook secret, the optional MCP settings), the migration Job (before the hub) and the seed Job (after it), `LK_REPLICA_NAME` from the downward API, `terminationGracePeriodSeconds: 45`, the `storage.googleapis.com` egress host, 12 MiB bodies and `Mcp-Session-Id` affinity on the load balancer.

## Reviewer agent verdict

Round 1 (`plans/18-hub-assembly.review.md`): CHANGES REQUESTED. Blocking: B1 (known, escalated) and an untested bound on large bodies in flight; both fixed. Round 2 (`plans/18-hub-assembly.review-round2.md`):

**VERDICT: APPROVE.** No blocking findings. The reviewer re-ran the gates itself, proved every new test red by removing the control it covers, and showed that the flaky `process::a_second_signal_exits_at_once` was a real race, now fixed (54 of 54 passes; with the fix removed, 1 failure in 20 under load). Its non-blocking notes are in the evidence ("Open items for the human").

## Needs a human

- **Confirm D145 on this PR:** it says the human chose option A for B1 in chat, which the repository cannot show.
- Update S11 in `docs/security.md` to match D137, and add `*.pending-snap` to `.gitignore` (neither file is prompt 18's).
- Settle the merge order with the docs branch (`3e68646`), and with prompt 09 (no order is required, G20; the hunks are additive).
- Optional follow-up: refuse the two CA certificates when their public keys are equal (catches the spellings the as-written comparison misses).

## Labels

`contract-change` (the plan's CC1, two additive budget entries, was approved by a human).

---

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_011wuVg8Nmkp8WWHzDsARGuX
