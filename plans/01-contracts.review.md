# Review 01, round 1

VERDICT: CHANGES REQUESTED

## Blocking

1. `db/migrations/0001_init.sql:785` and `:829-993` (S9, S21). `maintain_sentinel_partitions(keep_days)` lets `lanekeeper_app` prune sentinel records that are still inside the 90-day retention window.
   - The function is SECURITY DEFINER and runs `DROP TABLE` on partitions. It accepts any `keep_days` from 1 to 3650, and `lanekeeper_app` holds EXECUTE.
   - The migration comment says `sentinel_records` is "History nobody may rewrite or prune". The grant block gives the app only SELECT and INSERT on that table, which implies the same.
   - Reproduced on PG16 as `lanekeeper_app`: `SELECT * FROM maintain_sentinel_partitions(1)` returned `created=0, dropped=1`. It dropped last month's partition, far inside the 90-day life.
   - A compromised hub, or an injected query, can therefore erase NFS attribution evidence (D72, D73) older than one day. This voids the append-only intent for that table.
   - `docs/threat-model.md` (Elevation of privilege row, and the residual-risk bullet "The definer function is a privileged surface") claims the function can only drop "data already past its 90-day life". That claim is false as the code stands.
   - `sentinel_partition_function_is_the_only_ddl_path` only tests `keep_days` of 0 and 100000. It has no test that a short retention is refused.
   - Fix, either of:
     - raise the lower bound to a hard minimum (for example `keep_days < 90` raises `22023`, or the floor is a constant in the function, not a value from the app-writable `retention_settings`);
     - drop the parameter and have the function use a migrator-owned value.
   - Then add a test that the app calling it with a short `keep_days` is refused and drops nothing, and correct the threat-model text to match the real behaviour.

## Non-blocking

1. The pgvector part of db-verify was not proven by anyone (no pgvector locally; no Docker). Run `LK_REQUIRE_DOCKER=1 cargo test -p xtask --test db_migrations` (or `just verify-01`) on a Docker host or in CI before merge.
2. CI runs `cargo test --workspace` without `LK_REQUIRE_DOCKER=1`, so a runner without Docker passes the DB tests by printing SKIPPED. `.github/workflows/ci.yml` belongs to prompt 09; it must not be forgotten.
3. `deny.toml` adds a scoped NCSA licence exception for `libfuzzer-sys`. It needs explicit human acceptance.
4. `plans/01-contracts.md` changed after approval in `ca6c822`, explicitly "approved by a human" (adds `.cargo/audit.toml` to Extra-paths). Accepted on the user's stated approval.
5. `osv-scanner` (AGENTS.md rule 15) was not run. `hyper-util` 0.1.21 is 15 days old (dev-only).
6. In the DB harness, the `LK_TEST_PG_ADMIN_URL` external-server mode races on role creation under parallel test threads (3 failures with 4 threads; all pass with `--test-threads=1`). Container mode is unaffected. Document the single-thread requirement or serialise `run_init`.
7. The app keeps PUBLIC's TEMP privilege. Harmless with the pinned `search_path`; consider `REVOKE TEMP` if the hub never needs it.
8. Every P# in `perf/budgets.toml` is `registered = false`, so `bench-check` prints "0 pass, 0 fail, 33 unmet" and exits 0 (approved Q5(c) design). Prompt 16 and the release gate must use `--strict`.

## Evidence re-run

- `just verify`: PASS, exit 0 (fmt, clippy, 254 tests, deny, audit, web checks).
- `just verify-01`: FAIL at `docker-check` (no Docker daemon). Expected; recipe not weakened.
- verify-01 sub-recipes without db-verify: all pass (buf lint, redocly lint, bench-check selftest, fixtures determinism and target scale, fuzz-smoke 3 x 30 s with no crash).
- `cargo xtask bench-check`: exit 0, 0 pass, 0 fail, 33 unmet of 33.
- DB tests on a throwaway local PG16 (no pgvector), 1 thread: 22 passed.
- Manual probe as `lanekeeper_app`: `maintain_sentinel_partitions(1)` dropped a partition (blocking finding 1). `SET session_replication_role` was refused.
- `cargo tree -i rsa -e all --target all`: nothing, so the RUSTSEC-2023-0071 ignore is justified.
- Spec chain: plan approval commit exists; all commits in order T1 to T9, `01/fix`, `01/evidence`; every test name in the traceability table exists; no path outside ownership, Extra-paths, shared files or tests/fixtures changed; Contract-change "none" is correct.
