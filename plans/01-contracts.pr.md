# 01: Contracts, skeleton, ports, gates and fixtures

## Prompt

`prompts/01-contracts.md` · plan: `plans/01-contracts.md` (approved by a human, commit `2153623`; one approved addendum, `ca6c822`: Extra-path `.cargo/audit.toml`) · wave 0 · evidence: `plans/01-contracts.evidence.md` · reviews: `plans/01-contracts.review.md`

Nine task commits (`01/T1` to `01/T9`), then evidence, review and fix commits. Contract-change: none (prompt 01 owns every contract path). Extra-paths approved: `docs/threat-model.md`, `.cargo/audit.toml`.

## NOT PROVEN: database tests (read this first)

`just verify-01` stops at `docker-check` in the sandbox this was built in (no Docker daemon). **Every test that needs Postgres is unproven by a Docker run**: roles, grants, the append-only triggers, TRUNCATE guard, outbox NOTIFY, sentinel partitions, pgvector, the HNSW index and the container image path. In a plain `cargo test` those tests print `SKIPPED` and report `ok`, so a green test count is not proof.

Before merge, run on a Docker host or in CI:

```
LK_REQUIRE_DOCKER=1 cargo test -p xtask --test db_migrations     # or: just verify-01
```

The DB tests were run, with `--test-threads` 1, 4 and 8, against throwaway local PostgreSQL 16.15 clusters **without pgvector**: 25 passed, 0 failed. The pgvector statements sit between `-- lk:pgvector:begin/end` markers and have never been executed.

## Gates

| Command | Result |
|---|---|
| `just verify-01` | **FAIL at `docker-check`** (no Docker daemon). Recipe not weakened. |
| every other `verify-01` sub-recipe, one by one | pass: `tools-check`, `fuzz-tools-check`, `domain-verify`, `ports-verify`, `proto-verify`, `openapi-verify`, `mcp-doc-verify`, `perf-verify`, `fixtures-verify`, `fuzz-smoke` (3 x 30 s, no crash) |
| `just verify` | pass, exit 0 (fmt, clippy `-D warnings`, 257 tests passed, 1 ignored, `cargo deny`, `cargo audit`, web lint, typecheck, test, build) |
| `cargo xtask bench-check` | exit 0, `0 pass, 0 fail, 33 unmet of 33 budgets` (see below) |

## Acceptance checklist

- [ ] `just verify-01` and `just verify` pass. `just verify` passes; `verify-01` is not proven as a whole (Docker).
- [x] Every trait in `docs/interfaces.md` exists, with a fake and conformance tests (17 ports).
- [x] The proto passes its linter and tests (`buf lint`, 21 round-trip and transport tests).
- [x] The OpenAPI spec passes its linter and tests (`redocly lint`, 10 convention tests, 87 operations).
- [ ] The migrations pass their tests, and the database-role permission tests pass. **Not proven by a Docker run** (above).
- [x] Fixture generation is deterministic (same seed gives identical manifest, Git ref ids and NFS trees).
- [x] Target scale matches `docs/performance.md` (40 swimlanes, 60 tenant branches, about 80,000 files).
- [x] `bench-check` fails a planted over-budget value.

Test names for each item are in the evidence file.

## Security requirements implemented (S#)

S4 (route-guard harness), S11 (typed validators), S19 (dependency policy), S21 (`Secret<T>`, no leakage), S22 (fuzz skeleton). Written but only proven by the unrun DB tests: S9 (append-only audit log, role split). Shaped for later prompts: S5/S25, S7/S8, S14/S14b. Details and test names: evidence file.

## Benchmarks against budgets (P#)

Prompt 01 owns the registry, not the benchmarks. `perf/budgets.toml` has 33 entries, all `registered = false`, so **no P# is measured or proven yet**. The default `bench-check` passes because unregistered budgets are warnings; prompt 16 and the release gate must use `--strict`.

## New dependencies and why

All exact pins at least 14 days old (transitive crates younger than that were lowered with `cargo update --precise`). Full table with ages: evidence file. Notable:

- **sqlx 0.8.6**, no TLS feature (`webpki-roots` is CDLA-Permissive-2.0, rejected by `deny.toml`). Production TLS to Postgres is open.
- **serde-saphyr 1.1.0** (tests and conformance harness only; `serde_yaml` is denied).
- **testcontainers 0.28.0** (dev only), **libfuzzer-sys 0.4.13** (`fuzz/` only, own lockfile).
- `hyper-util` 0.1.21 is 15 days old (dev only).
- `osv-scanner` (AGENTS.md rule 15) was **not run**.

## Decisions a human must make

1. **NCSA licence exception for `libfuzzer-sys` in `deny.toml`** (scoped to that crate, which is never shipped). A licence-policy change; please accept or reject.
2. **`.cargo/audit.toml` ignore of RUSTSEC-2023-0071** (`rsa`, via sqlx's lockfile entry for `sqlx-mysql`; `cargo tree -i rsa -e all --target all` prints nothing, so it is never compiled). Already approved during T3; confirm.
3. **Contract points** called out in the plan: `PeerKind` on `JoinRequest` (Q11), `more`/`part` on `ScanDelta` (Q12), the budget `direction` field (Q5), `propose_change` has `confirmation: none` (the plan said `required`; it has no side effect, the confirmation is at `apply_change`), the extra table `settings_index_blobs`, and the `SECURITY DEFINER` function `maintain_sentinel_partitions` (Q18).
4. **Accepted residual risk: self-approval.** The trigger that stops a proposal's author approving their own change fires on INSERT only. `lanekeeper_app` can also UPDATE or DELETE `approvals` and `proposals`, so a compromised hub could make a proposal look self-approved or erase an approval. The claim was reworded in the threat model, the migration comment and the migration README (text only; no schema change). Approval enforcement must also live in application code in prompt 06. Possible hardening (SELECT/INSERT only, `BEFORE UPDATE` trigger, author-immutability trigger) is left to prompt 06 or a later contract-change.
5. Production Postgres image (pgvector >= 0.5, lz4) and TLS: owned by prompt 09.

## Handed to prompt 09

- CI runs `cargo test --workspace` without `LK_REQUIRE_DOCKER=1`, so a runner without Docker silently skips the DB tests. Set it for the test step.
- `lanekeeper_app` must never be the database owner (it would then belong to `pg_database_owner` and could drop `audit_events` and `sentinel_records`). A catalog test for this is suggested.
- CI wiring for `buf`, `redocly`, Docker tests and the nightly fuzz job.

## Assumptions

The plan's Q1 to Q25 were approved as written, so every default was used, except `propose_change` and the partial Q7 (`LK_REQUIRE_DOCKER=1` is set inside `db-verify` only). No previous endpoint/table bundle existed (Q1), so the endpoint and table inventories were derived from the docs. Full table: evidence file.

## Reviewer agent verdict

Three rounds, the maximum.

- **Round 1: CHANGES REQUESTED.** `maintain_sentinel_partitions` let the app drop partitions inside the 90-day life. Fixed with a hard floor of 90 days and a test.
- **Round 2: CHANGES REQUESTED.** `ON DELETE CASCADE` from `sentinels` into `sentinel_records` bypassed the floor, and the test harness lock serialised nothing. Fixed with `ON DELETE RESTRICT`, a catalog test for cascades into protected tables, and a lock on the shared base database proven on fresh clusters.
- **Round 3: CHANGES REQUESTED, one blocking finding.** The self-approval claim was stronger than the trigger. The human chose "reword only" (point 4 above). **That final reword was not re-reviewed**: the diff was checked to be comments and docs only (no SQL statement changed), and `just verify` passes.

Both round 1 and round 2 fixes were re-verified by the reviewer in the following round, including attempts to erase or modify protected data as `lanekeeper_app` by cascades, TRUNCATE, DROP, DETACH, ALTER, SET ROLE, replication-role changes, COPY and more (all refused).

## Labels

None. This is not a `contract-change` PR (prompt 01 owns the contract paths).
