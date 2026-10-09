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

---

# Review 01, round 2

VERDICT: CHANGES REQUESTED

The round 1 finding is fixed: the 90-day floor holds against every bypass tried on the function (NULL, -5, 0, 89, 2147483647, named and string arguments, timezone and datestyle changes; DROP, TRUNCATE, DELETE, UPDATE, DETACH PARTITION, CREATE TABLE and CREATE SCHEMA all refused). Two new problems remain.

## Blocking

1. `db/migrations/0001_init.sql:748` (with the grant at about `:985`, `GRANT ... DELETE ... sentinels ... TO lanekeeper_app`). `sentinel_records.sentinel_id REFERENCES sentinels (id) ON DELETE CASCADE`, and the app has DELETE on `sentinels`. This breaks S9/S21, the "History nobody may rewrite or prune" comment, and the "append-only" decision for `sentinel_records` in `db/migrations/README.md`. It lets `lanekeeper_app` erase sentinel records inside the 90-day life, which is the round 1 threat by another path.
   - Reproduced on PG16 as `lanekeeper_app`: after inserting one sentinel record, `DELETE FROM sentinels` printed `DELETE 1`; the record count as migrator went from 1 to 0. Direct DELETE, UPDATE and TRUNCATE on `sentinel_records` were all refused.
   - `docs/threat-model.md` (E row and the "definer function" residual-risk bullet) says a compromised hub cannot prune records inside the life. That is still false through this path.
   - Fix: change the FK to `ON DELETE RESTRICT`, so removing a sentinel that has records is refused. Alternatives: drop DELETE on `sentinels` for the app and add a `decommissioned_at` column, or add a trigger. Add a test that, as `lanekeeper_app`, `DELETE FROM sentinels` with records present is refused and `sentinel_records` is unchanged. Update the threat-model text to match. Also check every other table with an ON DELETE CASCADE into an append-only table.

2. `crates/xtask/tests/db_migrations.rs` (`run_init`, about lines 386-397), plus the claims in `db/migrations/README.md` ("parallel test threads are safe") and the new section of `plans/01-contracts.evidence.md` ("--test-threads=4 and 8 ... 23 passed"). The advisory lock does not serialise anything: `pg_advisory_lock` is scoped to the current database, and each test takes it in its own `lk_t_*` database. The role-creation race (round 1 non-blocking 6) is therefore unfixed.
   - Failing scenario: on a fresh external server with no roles, `LK_TEST_PG_ADMIN_URL=... cargo test -p xtask --test db_migrations` failed in 3 of 5 runs (1, 2 and 3 failures), each `duplicate key value violates unique constraint "pg_authid_rolname_index"` (`CREATE ROLE lanekeeper_migrator`). The 8 later runs on a cluster that already had the roles passed.
   - The README and evidence statements are therefore false. Evidence that does not match reality is blocking.
   - Fix: take the lock on a connection to one shared database (the base `postgres` database from `admin_options`), or create the roles once under a process-wide `tokio::sync::OnceCell` or mutex. Then re-prove it on a FRESH cluster (drop the roles and `lk_t_*` databases first), or remove the claim from the README and evidence.

## Non-blocking

1. The pgvector part of db-verify is still unproven (no Docker daemon, no pgvector). Run `LK_REQUIRE_DOCKER=1 cargo test -p xtask --test db_migrations` or `just verify-01` on a Docker host or in CI before merge.
2. CI runs `cargo test --workspace` without `LK_REQUIRE_DOCKER=1`; belongs to prompt 09.
3. `deny.toml` NCSA licence exception for `libfuzzer-sys` needs explicit human acceptance.
4. The plan changed after approval only in `ca6c822` (human-approved Extra-path).
5. `osv-scanner` not run. `hyper-util` 0.1.21 is about 15 days old (dev-only).
6. The app keeps PUBLIC's TEMP privilege; harmless here (`search_path` pinned). Consider `REVOKE TEMP`.
7. All 33 P# are `registered = false`; prompt 16 and the release gate must use `--strict`.
8. `6380730` touched only the migration, DB test file, migration README, threat model and evidence file.

## Evidence re-run

- Throwaway PG16.15 (no pgvector), floor probes as `lanekeeper_app`: all refused as listed above; the cascade bypass via `DELETE FROM sentinels` succeeded (blocking 1).
- `db_migrations` against the external server: fresh cluster failed on the first run and in 3 of 5 runs of the loop (blocking 2); with roles present, 23 passed in 8 of 8 runs.
- `just verify`: exit 0. `cargo xtask bench-check`: exit 0, 0 pass, 0 fail, 33 unmet of 33.
- `just verify-01`: exit 1 at `docker-check` (known sandbox limit, not weakened).
- Spec chain: plan approval commit `2153623` exists; commit order T1 to T9, `01/fix`, `01/evidence`, `01/review`, `01/fix`. Traceability table not re-verified row by row.
