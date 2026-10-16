# 05: Registry, indexes, incremental pipeline and read API

> **Please do not merge until these are answered. None of them can be closed from the sandbox that built this branch.**
>
> 1. **The contract PR is merged (hsetially/lanekeeper#18, `a058592`).** The five contract commits (migrations `0007_tool_write_registrations` and `0008_blob_touch`, `ports::ToolWrites::register_delete`, the blob-retention contract, the contract docs) are on `main`, and this branch has `main` merged in (`bd8da2c`, no change to the tree), so this PR's diff is 05's own changes. Still to do with it: prompt 06's delete path must call `register_delete` (see #18).
> 2. **A run of the gates that need tools the sandbox does not have.** `just verify-05`, `just verify`, `cargo deny`, `cargo audit` (including the new `crates/hub-registry/fuzz/Cargo.lock`), `osv-scanner`, the libFuzzer runs of the four registry fuzz targets (stable `corpus_replay` stands in), and Docker/testcontainers could not run here. Their underlying commands were run by hand where possible (below). The repository's CI job runs `cargo fmt`, `clippy --workspace`, `cargo test --workspace`, `cargo deny check` and `cargo audit`, so it covers most of that (it passed on #18); **it does not run `just verify-05`, the benchmarks and `bench-check`, `osv-scanner` or the libFuzzer runs**, so those still need a run by you: `just verify-05`, `just verify` and `just bench && cargo xtask bench-check`.
> 3. **Decisions that are yours** (list under "Assumptions"), above all: **retention deletes by default** (D118) and **an audit event does not pin a blob** (D116). Do not wire or enable `registry-retention` before you have confirmed both.
>
> **`hub-registry` is a library: it is not wired into `crates/hub`** (A8, D112). Merging this changes nothing at run time until the follow-ups below are done.

## Prompt

`prompts/05-registry-read-api.md` · plan: `plans/05-registry-read-api.md` (approved by a human, Contract-change approved, Extra-paths `docs/threat-model.md`, `docs/decisions.md`, `crates/xtask/tests/budgets_registry.rs`) · wave: 2

The hub's brain: ingestion of agent and sentinel reports, the settings and tree indexes, the incremental pipeline that turns an observation into drift and findings, adoption, severity, denied files, tenants and pickup state, the structure checks C9 to C12, blob retention, and the read API (the 15 read operations and the port methods prompt 07 added). Prompt 06's write paths and prompt 07's MCP server (PR #17) are merged into this branch; the registry implements the six `RegistryRead` methods that 07 added to the port (D108).

**What is in the diff, so the review can be sized:** 63 `05/Tk:` task commits, the `05/fix:`, `05/review:`, `05/evidence:` and `05/pr:` commits, and merge commits from `main` (the contract commits arrive through #18, which is already merged, so they are not part of this diff). Everything of 05's own is under `crates/hub-registry/**`, plus the budget flips in `perf/budgets.toml`, the `verify-05` recipes in the `Justfile`, `crates/xtask/tests/budgets_registry.rs`, the D-entries D93 to D106, D108, D110 to D123 in `docs/decisions.md`, and the "Registry and read API (05)" section of `docs/threat-model.md`. No `crates/domain`, `proto/`, `api/openapi.yaml`, `docs/mcp-tools.md` or `design/handoff` path changed. No budget threshold was changed.

## Gates

Run by hand in the recipes' order, because the sandbox has no `just`, no Docker daemon, no `cargo-deny`, `cargo-audit`, `osv-scanner`, nightly or `cargo-fuzz`. Raw tails, exit codes and what each row ran on are in `plans/05-registry-read-api.evidence.md`. A test that prints SKIPPED is never counted as passed: the proof of "no skips" is a run with `LK_REQUIRE_DOCKER=1`, where a skip is a failure.

| Gate | Result |
|---|---|
| `just verify-05` as one recipe | **NOT RUN** (needs Docker, cargo-deny, cargo-audit, nightly, cargo-fuzz). Sub-recipes below. |
| `registry-lint`: `cargo fmt --all -- --check`; `cargo clippy -p hub-registry --all-targets --all-features -- -D warnings` | exit 0, exit 0 (re-run after each review round) |
| `registry-test`: `LK_REQUIRE_DOCKER=1 cargo test -p hub-registry --lib --tests --no-fail-fast`, real Postgres 16 with pgvector | **79 binaries, 1,184 passed, 0 failed, 3 ignored** at head `6be7d9a` (the three ignored are the target-scale tests below) |
| `registry-slow-test`: the 3 ignored tests, release, one at a time | **3 passed** (ranking at target scale x2, sharded-search equivalence) |
| `registry-fuzz` (libFuzzer, 4 targets) | **NOT RUN** (no nightly, no cargo-fuzz). Stand-in: `corpus_replay` (17 tests, in the 1,184) replays the committed seeds through the same invariants. |
| `registry-bench`: `cargo bench -p hub-registry`, then `bench-check` over the 18 registry budgets | **18 pass, 0 fail, 0 unmet** (official run; P12 re-measured after the last fix: 12.3 ms) |
| `just verify` as one recipe | **NOT RUN.** Its steps: `cargo fmt` exit 0; `cargo clippy --workspace --all-targets -- -D warnings` exit 0; `cargo check --workspace --all-targets --locked` exit 0 (re-run at the last head); `cargo test` crate by crate (disk): **5,234 passed, 0 failed** in the first pass; web checks (`pnpm install --frozen-lockfile`, lint, typecheck, test, build) all exit 0 |
| `cargo test -p xtask` | exit 0, 162 passed, 1 ignored |
| `cargo xtask bench-check` (whole registry, after every crate's benchmarks) | **66 pass, 0 fail, 7 unmet of 73.** The 7 unmet are other prompts' budgets with no benchmark yet (`P7.docs_search`, `P7.feature_status`, five `P10.*`). |
| `cargo deny check`, `cargo audit`, `osv-scanner` | **NOT RUN** (not installed). This branch adds no `name version` pair to `Cargo.lock` or to the fuzz lockfiles (compared against `origin/main`). |
| Docker-only test | `hub` `pg_tls::pg_pool_connects_to_a_real_tls_postgres_and_refuses_the_wrong_ca` needs a Docker daemon (`crates/hub` is untouched by this branch). |

**The final gate found two real defects, both of the benchmark harness, both fixed:** `steady_state_queries` bound five parameters where the drift statement has taken six since T9 (the bench panicked and two benchmarks after it never ran), and the read bed measured on a database whose tables had never been analysed (dense settings searches took 110 to 160 ms instead of 3 to 30 ms until autovacuum ran). See "Production note" below.

**Flaky tests fixed on the way:** `served_verification` two-swimlane test (failed about 1 run in 6; now a deterministic rendezvous, 0 of 50) and `retention_gc` lease-holder test (a fixed 900 ms sleep; now waits for events). `hub-platform` `lease_race::pg_leases_conformance` (prompt 03b's, a 100 ms wall-clock test) failed once when the whole workspace ran in parallel and passed alone and in both later passes; it is byte-identical to `main`.

## Acceptance checklist

- [ ] `just verify-05` passes. **Not ticked: it cannot run as a recipe here.** What ran by hand, in its order, passed (rows above).
- [x] Every P1, P5, P6, P7 and P12 item owned here is within budget (and P2, P14, P15, which the plan also gives this prompt): 18 of 18 registry budgets met; table below.
- [x] The incremental-equals-full property test passes (`pipeline_prop::incremental_state_equals_full_recompute_after_random_events`, `::incremental_equals_full_after_a_takeover`; the model is built independently on `engine::drift_state`).
- [x] Every endpoint matches the OpenAPI spec and passes the route-guard test, **with one pinned deviation**: `SwimlaneSummary.tenants` has `minItems: 1`, an unadopted swimlane answers `[]` (`http_contract.rs:701`, `known_deviation_an_unadopted_swimlane_lists_no_tenants_where_the_spec_wants_one`). You choose: `minItems: 0` in the spec, or a placeholder tenant.
- [x] The pending-user 403 and Viewer requirements are tested (`http_authz::*`: every route, every role, no session is 401, the service itself refuses a non-viewer).
- [x] Plan rows T1 to T10, the A10 follow-up (`ToolWrites`), S22 and the review fixes: each ticked with the test names in the evidence file (`## Acceptance checklist`). Fourteen of the plan's test names differ from the built ones; the evidence maps each.

## Security requirements implemented (S#)

S4 (default-deny authorization: routes only through `GuardedRouter`, every service method checks the caller; adoption Admin, marks Operator), S5 (agent and sentinel identity at `hello`), S9 (one audit event per state change, in the transaction, batched through the contract's `record_many`), S10 (no content, secret, hash or path in logs, errors or metrics; log-capture tests with planted words for ingest, index, reads, blob endpoint, retention), S11 (typed parsers, keyset pagination capped at 500, linear-time regex with a deadline), S13 (secret scan of every indexed blob and of both repositories at adoption; flagged values stored redacted), S14b/S15 (served and config text are data), S17 (denied files: **one rule** `crate::denied::is_denied`, used on tree, content, compare, effective, grid, served, settings search, text-search corpora, pickup, C10 and the blob endpoint, with the hub's own globs and for deleted files), S21 (`#![forbid(unsafe_code)]`, no panics, errors leak nothing), S22 (fuzz target for every parser at a trust boundary: `registry_delta`, `registry_cursor`, `registry_query`, `registry_sentinel_path`), S24 (threat-model section: 36 STRIDE rows, 35 residuals, pinned by `tests/threat_model.rs`), S25 (sentinel attribution confined to the sentinel's own swimlane; an upgrade never downgrades). The evidence file maps each S# to code and to the tests that prove it.

## Benchmarks against budgets (P#)

All 18 `hub-registry/` budgets are registered and met (official run, release, target-scale fixtures: 40 swimlanes, 80,120 files, 867,285 settings, 4 shared vCPU). p95 against budget:

| Budget | Measured | Budget |
|---|---|---|
| P1.hub_delta_to_drift | 244 ms | 2,000 |
| P2.hub_head_moved_to_drift (tight) | 615 ms | 1,000 |
| P5 / P5.per_file_per_swimlane | 27.4 ms / 0.43 ms | 50 / 2 |
| P6.hub_full_recompute / P6.adoption_rank | 11.7 s / 39 ms | 180 s / 5,000 ms |
| P7.swimlane_list, file_tree, file_content_cached, file_content_cold | 1.8, 5.8, 1.6, 1.6 ms | 50, 100, 30, 80 |
| P7.compare_api / whole_swimlane_compare | 28.1 / 24.9 ms | 100 / 300 |
| P7.grid (tight) | 102 ms | 150 |
| P7.settings_search (**brittle, see below**) | 108 ms (other runs: 43, 107, 197) | 200 |
| P7.text_search (steady state, D97) | 189 ms | 400 |
| P12.steady_state_query_ms (45 statements, none over 50 ms) | 12.3 ms | 50 |
| P14 | 4.7 s | 30 s |
| P15.registry_ingest_versions_per_s (min; the blob double now runs the real `PUT_SQL` with `touched_at`) | 1,547 | >= 1,000 |

Numbers on this shared box moved by up to 30% between runs. Noisier CI hardware can fail the tight ones (P2, P7.grid, P15, P7.settings_search) without a code change.

**`P7.settings_search` is met by the statistic, not by every request.** The index needs three characters of a needle. A one- or two-character needle that matches few settings reads all 867,285 rows (220 to 290 ms). The registered workload has exactly one 2-character needle in twenty, so 5 of its 100 samples are that scan and sit exactly at the 5% boundary of p95. It would breach if more than 5% of real queries were short needles (type-ahead). It needs a contract decision: `minLength: 3` in the OpenAPI for this parameter, or a separate budget for short needles. The needle set also changes per run (`TABLESAMPLE SYSTEM REPEATABLE` depends on page layout).

**`P7.text_search` is the steady state** (D97, your decision of 2026-10-15): the first search after an applied change to one swimlane on a warmed hub. A cold search (corpora not yet built) is about 570 ms, outside the budget; the per-replica warm-up (`spawn_warmup`) exists to avoid it.

**Production note (planner statistics).** After a bulk load into `settings_index` (the first inventory of a swimlane, 40 of them at adoption) and before autovacuum's first auto-analyze, dense settings searches cost 100 to 160 ms a statement, and `lanekeeper_app` cannot run `ANALYZE`. The bench's read bed now analyses as the table owner, as autovacuum would. This is in the threat model as a residual risk. Closing it needs a contract change: per-table `autovacuum_analyze_*` settings in a migration, or an owner-run `ANALYZE` after adoption.

## New dependencies and why

**None.** No `name version` pair is new in `Cargo.lock` or in the new `crates/hub-registry/fuzz/Cargo.lock` compared with `origin/main`; every dependency of `crates/hub-registry` is `x.workspace = true` of an already-pinned crate (the youngest, `prometheus-client 0.25.1`, is 45 days old; the fuzz crate's `libfuzzer-sys 0.4.13` is 134 days old). The one root `Cargo.toml` change is a workspace path entry. `cargo deny`/`cargo audit` could not run here (the evidence file has the age table).

## Assumptions

The plan's ambiguity rows A1 to A46 were approved with the plan; what each became is in the evidence file. **Decisions recorded as "to be confirmed at review" (yours):**

- *By the orchestrator under your delegation of 2026-10-15 ("make decisions for me"):* D98 to D102 (drift alerts held while a sync window is open: only for nfs_ahead, conflict and untracked; a 4 h hold budget in 24 h), D110 to D112 (ingestion staging, the read shapes the contract leaves open, what is not built), D116 to D118 (**retention**: an audit event does not pin a blob, only an explicit `blob_refs` row does; the sync_windows prune; **GC deletes by default**, in bounds of 500 blobs / 64 MiB a run, with a `dry_run` setting, deletes not audited).
- *By the implementer:* D103 (severity rules are **raise-only**, so an admin cannot lower or disable a default although D76 says admin-configurable), D104 (what a denied file is), D105/D106 (pickup state; tenant hints and suffixes are evidence, not members), D108 (the six port methods; flags a contract question: the port says drift by path, REST says by severity), D113 to D115, D119, D120 (`ports::ToolWrites` as an authority with a 15 minute TTL), D123 (sentinel attribution), D121/D122.
- **Numbering:** our old D91, D92 and D109 are now **D121, D122, D123**, because prompt 07 took D91, D92 and D109 on `main`. `plans/05-*.md` is frozen and still uses the old numbers; each renumbered entry says so.
- Product behaviours to check: D111 leaves the A28 masking gap against S14b (the threat model lists it); D116 means the audit chain can name a hash whose blob has been collected (residual risk, in the threat model).

## Reviewer agent verdict

Two rounds (`plans/05-registry-read-api.review.md`). Round 1 reviewed `a3b29b4`:

```
VERDICT: CHANGES REQUESTED
No code defect found. Blocking: (1) the threat-model section described the retention job as not built; (2) the gates that cannot run in the sandbox.
```

All five fixable findings were fixed in five `05/fix:` commits: the threat model rewritten with 8 STRIDE rows and 8 residuals and pinned by tests; the blob endpoint now applies the one denial rule with the hub's own globs and for deleted files (bounded lookup on existing indexes, fails closed); the P12 benchmark times all 45 read statements and a test ties its list and binds to the code; a log line that carried a hash became a counter; stale `audit_event` wording corrected. Round 2 reviewed `372cace`:

```
VERDICT: APPROVE
No blocking finding other than the environmental one (round-1 item 2: the gates that cannot run here).
```

The five non-blocking round-2 findings (an unbounded second `EXISTS` in the blob policy, a per-request warning, P12's scan missing four inline statements, two evidence inconsistencies, a wrong test citation and a missing residual in the threat model) were then fixed in five more `05/fix:` commits (`ee6e9e9` to `6be7d9a`), with mutation checks. **Those five commits were checked by the orchestrator, not re-reviewed by the reviewer.** The reviewer ran fmt, clippy, the full registry suite, `xtask` and the registry budget cut itself, and recomputed every p95 from the raw samples.

## Known weaknesses and follow-ups

- **Seven registry tests assert "nothing happened" after a fixed sleep** (`tool_write_matching.rs:993`, `sentinel_sink.rs:683`, `attribution_job.rs:123`, `pipeline_events.rs:316`, `severity.rs:542`, `read_corpora.rs:278` and `:298`). They cannot fail a correct job, but under heavy load could pass a broken one. Not changed.
- **Not wired into `crates/hub`** (A8, D112): the hub must give the gateway `AgentIngest` and `SentinelIngest`; give prompt 06 `PgToolWrites`; mount the 15 read routes and **install `RegistryBlobPolicy` on the blob endpoint (otherwise denied hashes are served; `RegistryBlobPolicy::new` is fallible)**; start eleven leased jobs (`registry-pipeline`, `registry-nightly`, `registry-pickup-sweeper`, `registry-structure-sweeper`, `registry-index-sweeper`, `registry-tool-write-matcher`, `registry-served-verify`, `registry-attribution-grace`, `registry-sentinel-partitions`, `registry-retention`, `registry-sync-watch`) and the per-replica text-search warm-up (`RegistryService::spawn_warmup`); run `0007` and `0008` before the code that needs them. A forgotten job fails silently, so the wiring prompt should test that all are registered. The registry trusts the `User` it is handed: the hub's active-user guard must load it per request.
- **Not built, no owner yet:** the adoption and "mark intentional" routes (A6) and the admin operations of the spec (A7). The contract has no operation for them.
- **Sentinel heartbeats do not reach the registry** (`hub-platform/src/sentinel_service.rs`); prompt 17's "heartbeat gap raises a finding" needs a contract decision.
- **T7 (attribution) landed after T8, T9 (first half) and part of T10** in commit order; no behavioural effect.
- **Not run here:** `registry-fuzz` (libFuzzer), `cargo deny`, `cargo audit`, `osv-scanner`, Docker/testcontainers, a real config-server behind a real agent (the as-served check, D86, is exercised against a fake).

## Labels

`contract-change`: the plan's Contract-change was approved by a human (it covers the budget flips in `perf/budgets.toml`). The contract commits `0007`, `0008` and `register_delete` went in their own contract PR, hsetially/lanekeeper#18, which is merged.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01E8yUvEBixTNGvgtpLUCi4y
