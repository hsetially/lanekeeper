# Migrations

These files are a contract. Prompt 01 (T6) writes `0001_init.sql`; `0002_audit_verify_state.sql` is a contract-change that adds one table for prompt 03a; `0003_registry.sql` is a contract-change that adds the registry's tables and columns for prompt 05.

- Add new migrations as new files (`0002_<what>.sql`). Never edit a merged one.
- Follow the lanekeeper-contract-change skill: expand first, contract later.
- Run them as `lanekeeper_migrator` with sqlx's `Migrator` (runtime, `sqlx::migrate::Migrator::new(path)`); the tests use the same path.

## Required server features

| Feature | Why |
|---|---|
| PostgreSQL 16 | `NULLS NOT DISTINCT` unique indexes, `sha256()`, partitioning, `COMPRESSION lz4` |
| built with `--with-lz4` | `blobs.content` uses lz4 TOAST compression (the official and CloudNativePG images have it) |
| `pgvector` >= 0.5 | `doc_chunks.embedding vector(384)` and its HNSW index |
| `pg_trgm` | trigram GIN index on `settings_index.setting_path` |
| `pg_stat_statements` | created by `deploy/dev/init.sql` for dev; optional in production |

`0001_init.sql` runs `CREATE EXTENSION IF NOT EXISTS` for `pg_trgm` and `vector`. That is a no-op when they are already installed (the normal case), so the migrator does not need superuser rights. If one is missing and the migrator cannot create it, the migration fails at that statement.

Everything that needs pgvector sits between the `-- lk:pgvector:begin` and `-- lk:pgvector:end` markers. Do not move pgvector statements out of that block: the tests rely on it to run the rest of the file on a server without pgvector.

## Roles

Two roles exist before any migration runs. `0001_init.sql` never creates roles (plan Q6); it stops with a clear error if either is missing, or if it is not run as `lanekeeper_migrator`.

- `lanekeeper_migrator` owns every object and runs the DDL.
- `lanekeeper_app` is what the hub connects as. It has no DDL right, owns nothing, and holds only the table privileges granted in the migration.

They come from `deploy/dev/init.sql` (dev and tests) and from CloudNativePG managed roles (prompt 09).

## Grant convention (plan Q19)

- Grants are explicit, per table, at the end of each migration that adds tables. There are no `ALTER DEFAULT PRIVILEGES`.
- Every table has exactly one of these decisions for `lanekeeper_app`:

| Decision | Privileges | Used for |
|---|---|---|
| append-only | SELECT, INSERT | `audit_events`, `audit_checkpoints`, `sentinel_records` |
| write-once | SELECT, INSERT, DELETE | content-addressed or immutable rows that retention or garbage collection may remove (`blobs`, `blob_refs`, `settings_index`, `settings_index_blobs`, `git_tree_index`, `doc_versions`, `webhook_deliveries`) |
| mutable | SELECT, INSERT, UPDATE, DELETE | everything else |
| state row | SELECT, UPDATE | a singleton row the migration inserts and the app only reads and updates (`audit_verify_state`) |
| none | nothing | `_sqlx_migrations` |

- Never grant TRUNCATE, REFERENCES or TRIGGER. Sequences: `USAGE` only where a `serial` column needs it (`audit_events.seq`); identity columns need nothing.
- The decision for every table is repeated in `crates/xtask/tests/db_migrations.rs` (`grant_decisions`). A migration that adds a table without adding its decision there, and the matching `GRANT`, fails `every_table_has_a_grant_decision`.

## The audit log (S9, D56)

- `audit_events` and `audit_checkpoints` are append-only. `lanekeeper_app` can only INSERT and SELECT. A trigger on both tables refuses UPDATE, DELETE and TRUNCATE for every role, the owner and superusers included, in every `session_replication_role` (`ENABLE ALWAYS`). The trigger raises SQLSTATE `23001`, so a trigger refusal differs from a missing privilege (`42501`).
- `event_json` holds the exact canonical JSON that was hashed. The table checks `hash = sha256(prev_hash || event_json)` (the definition in `ports::audit::chain_hash`) and has `UNIQUE(prev_hash)`, so a chain cannot fork and a wrong hash is refused. The first event's `prev_hash` is 32 zero bytes.
- Appends must still be serialised by the writer (`AuditLog::record`), so that the chain order is the commit order.

## The audit verifier's state (`0002_audit_verify_state.sql`, prompt 03a)

`audit_verify_state` is one row: how many nights in a row the nightly audit verification ended without a verdict (`unfinished_runs`, which a night that ran to the end resets to 0), the class of the last such error (`last_error`, at most 200 characters; an error class or a SQLSTATE, never a value, an error text or an event: S10) and when the row was written. The verifier reads it under its lease and updates it after each run, so that the count survives a restart of the pod and a move of the lease to another replica, and the alert on the second night in a row can fire. The `Leases` port carries no payload, so the count had no other home.

- **Singleton.** `id boolean PRIMARY KEY DEFAULT true CHECK (id)` allows exactly one row, the one `0002` inserts (`unfinished_runs = 0`, `last_error` NULL). `unfinished_runs >= 0` and the length cap on `last_error` are CHECKs, so the table stays harmless as data even if a caller writes to it.
- **Grants: SELECT and UPDATE only.** No INSERT, because the migration seeds the row and the app never has to create it; no DELETE or TRUNCATE, so the app cannot remove it. Without INSERT, `INSERT ... ON CONFLICT DO UPDATE` is refused too; the verifier uses a plain `UPDATE`. If the row is missing (only the owner or a superuser can remove it), the `UPDATE` touches zero rows and the verifier treats that as a broken schema (`SchemaChanged`, an alert), not as a reason to recreate the row.
- **Rollout order.** The migration deploys before the 03a code that reads it. An older 03a binary ignores the table. The new verifier treats a missing table (`42P01`) as `SchemaChanged`, so deploying the binary before the migration alerts instead of silently counting nothing.
- **Not tamper-evident, not audited.** The table is not part of the audit chain and has no trigger. A compromised `lanekeeper_app` can set the count to 0 or to a large number, which can silence or force the in-process alert. The alerts that read history (`increase(audit_verify_errors_total[3d]) >= 2`, `increase(audit_verify_runs_total[36h]) == 0 or absent(audit_verify_runs_total)`) stay the recommended ones, because they do not depend on this row. Writes to it are operational state, like `last_seen_at` and the session sweep, not a change of state of the product (B19; "Not everything is audited" in the 03a section of the threat model), so they write no audit event. What a verification found is reported through the verifier's findings, metrics and logs, not stored here.

## The registry's tables and columns (`0003_registry.sql`, prompt 05)

Expand-only: new columns and indexes on existing tables, eight new tables, one grant, and one CHECK widened in the same transaction. Nothing is dropped, renamed, re-typed or deleted, `0001` and `0002` are untouched, and every new column is nullable or has a default, so an older binary ignores them. Deploy the migration before the 05 code. `migration_0003_changes_existing_objects_only_by_adding` compares the catalog before and after and fails if anything that existed changed.

| Where | What | Why |
|---|---|---|
| `file_observations` | `initial`, `previous_hash`, `previous_state`; unique `file_observations_replay_key` on `(swimlane_id, path, observed_at, hash)` with `NULLS NOT DISTINCT` | the first inventory is not a change (A11); the C7 context; replaying a delta is idempotent |
| `file_state` | `seq` (identity, `GENERATED ALWAYS`), `nfs_drift_hash`, `observed_at`, `last_observation_id` (no foreign key on purpose); indexes `file_state_seq_idx` (unique, `(swimlane_id, seq)`), `file_state_prefix_idx` (`text_pattern_ops`), `file_state_denied_hash_idx` (partial), `file_state_current_hash_idx` (covering, partial) | a short keyset surrogate (A31); the normalised N of the drift table; folder listings; the blob policy; the text-search file list |
| `baselines` | `blob_hash` | the raw version behind a baseline (`hash` is the drift hash) |
| `sync_windows` | `warned_at`, `processed_at` | a stuck window warns once; a closed window is processed once |
| `service_state` | `pending_files jsonb` | the files whose change a service has not picked up (D85) |
| `settings_index` | `secret_rule` | the rule that flagged a value (the value is stored redacted, S13) |
| `swimlanes` | `last_notified_at` | written by prompt 06's notify action, read by the pickup state (A20) |
| `findings` | the `kind` CHECK also allows `served_mismatch` | D86: "as served" differs from the engine's rendering |
| new: `swimlane_registry` | first inventory, `needs_full_scan`, rules fingerprint, `version` (the list ETag), `state_counts`, `finding_count` | the pipeline's state per swimlane |
| new: `swimlane_deployments` | what the last sync deployed, per `(swimlane, tenant)` | `record_sync` (T4) |
| new: `swimlane_folders` | when each folder was first seen, and whether it was in the first inventory | C11 |
| new: `release_hints` | Helm release hints per service | adoption hints (Q19) |
| new: `effective_files` | the effective hash of each logical file | `compare_tree` descends only into directories whose hashes differ (D67) |
| new: `adoptions`, `adoption_resolutions` | the wizard's state, and the admin's decision per differing file | adoption progress persists and resumes (D22) |
| new: `delta_staging` | the acknowledged parts of a multi-part delta that are not yet applied | P15: no acknowledged version is lost (A14) |

- **Grants.** All eight new tables are `mutable` (SELECT, INSERT, UPDATE, DELETE). The pipeline upserts and prunes them, and each hangs off `swimlanes` with `ON DELETE CASCADE`, which `no_cascading_foreign_key_touches_a_protected_table` allows only for tables the app may delete from. Their decision is in `grant_decisions` in `db_migrations.rs` (`REGISTRY_TABLES`).
- **Vocabularies.** `adoptions.reason` is `initial` or `tenant_suffix_changed`; `status` is `in_progress`, `review`, `confirmed` or `cancelled`; `step` is `branch`, `base_version`, `files`, `secrets` or `confirm`; `adoption_resolutions.resolution` is `nfs_is_right`, `git_is_right` or `intentional`. At most one adoption per `(swimlane, reason)` is open (`in_progress` or `review`) at a time. A new value is a new migration that widens the CHECK.
- **Not relaxed (plan 05, A16).** The `sentinel_records.operation` CHECK still allows only `create`, `write`, `delete` and `rename`: an `Attribute` record (chmod, chown) is acknowledged and counted, not stored. Storing them would be a later contract change.
- **Cursors.** A `Cursor` is at most 512 characters and a path up to 1,024 bytes, so keyset lists over files page on `file_state.seq` (or a time and an id), never on a path (A31).

## Other database-side guards

- Outbox (D80): the `outbox_notify` trigger calls `pg_notify('lanekeeper_outbox', <row id>)`. PostgreSQL delivers it when the inserting transaction commits and drops it on rollback.
- `pr_links`: a partial unique index allows one `open` PR per (swimlane, path, nfs_hash).
- `approvals`: a `BEFORE INSERT` trigger refuses an approval by the proposal's author. It guards the insert path against a hub bug. It does not protect against a caller that holds UPDATE or DELETE on `approvals` or `proposals` (`lanekeeper_app` does): that caller can change `approver_oid` or `author_oid` after the insert, or delete a proposal, which cascades to its approvals. Approval enforcement (S4/D37) must also be done in application code (prompt 06). Closing the gap (UPDATE and DELETE restrictions plus an author-immutability trigger) is a possible hardening for prompt 06 or a later contract-change.
- Append-only tables and cascades: a foreign key whose action is `CASCADE`, `SET NULL` or `SET DEFAULT` changes child rows without a privilege check on them, so it is a way around the grants. `sentinel_records.sentinel_id` is `ON DELETE RESTRICT`: `lanekeeper_app` has DELETE on `sentinels`, and deleting a sentinel that still has records is refused (`23503`) instead of erasing them. A sentinel with records cannot be removed before its records expire (`maintain_sentinel_partitions`); decommissioning one means keeping the row. `no_cascading_foreign_key_touches_a_protected_table` walks the catalog and fails if any foreign key into or out of a protected table (no DELETE for the app, or guarded by the append-only trigger) has an action other than `NO ACTION` or `RESTRICT`. A new migration that adds such a key must pass it.
- Sentinel records are partitioned by month. Dropping a partition is DDL, so the hub calls `SELECT * FROM maintain_sentinel_partitions(90)` from a leased job (prompts 05 and 17). It is a `SECURITY DEFINER` function owned by the migrator, with a pinned `search_path` and schema-qualified names. `EXECUTE` is granted to `lanekeeper_app` only (plan Q18). It refuses any `keep_days` below 90 (`22023`), a constant floor in the function body, so the app cannot prune records still inside their 90-day life. It also creates last month, this month and the next two, so the job must run at least monthly.

## Running the tests

`just db-verify` runs `crates/xtask/tests/db_migrations.rs` against a container of the image pinned in `deploy/dev/compose.yaml` (the test `image_matches_compose` keeps the two equal). It sets `LK_REQUIRE_DOCKER=1`: without a Docker daemon the tests fail. A plain `cargo test` skips them with a `SKIPPED` line instead.

To use an existing throwaway server instead of Docker, set `LK_TEST_PG_ADMIN_URL` to a superuser URL, for example `postgres://postgres@127.0.0.1:5432/postgres`. Each test creates its own database on it and leaves it behind. Role creation in the init SQL is serialised with a session advisory lock taken on the base database of the URL (roles are cluster-wide, and an advisory lock is per database, so a lock taken in each test's own database would serialise nothing), so parallel test threads are safe; this was run on freshly initialised clusters at `--test-threads=4` and `--test-threads=8`. If that server has no pgvector, the pgvector block is left out and the checks that need it print `NOT PROVEN`; `LK_REQUIRE_DOCKER=1` refuses such a server.
