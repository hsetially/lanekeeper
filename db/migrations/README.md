# Migrations

These files are a contract. Prompt 01 (T6) writes `0001_init.sql`; `0002_audit_verify_state.sql` is a contract-change that adds one table for prompt 03a; `0003_registry.sql` is a contract-change that adds the registry's tables and columns for prompt 05; `0004_write_additions.sql` is a contract-change for prompt 06 (notify proposals, the token expiry, and the approvals hardening); `0005_tool_write_registrations.sql` is a contract-change for prompts 05 and 06 (the table behind `ports::ToolWrites`).

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
| append-only | SELECT, INSERT | `audit_events`, `audit_checkpoints`, `sentinel_records`, `approvals` (from `0004`) |
| write-once | SELECT, INSERT, DELETE | content-addressed or immutable rows that retention or garbage collection may remove (`blobs`, `blob_refs`, `settings_index`, `settings_index_blobs`, `git_tree_index`, `doc_versions`, `webhook_deliveries`, `tool_write_registrations` from `0005`) |
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

Expand-only: new columns and indexes on existing tables, eight new tables, one grant, and one CHECK widened in the same transaction. Apart from replacing the `findings.kind` CHECK with its wider version (dropped and re-added in the same transaction), no existing object is dropped, renamed, re-typed or deleted, `0001` and `0002` are untouched, and every new column is nullable or has a default, so an older binary ignores them. Deploy the migration before the 05 code. `migration_0003_changes_existing_objects_only_by_adding` compares the catalog before and after and fails if any existing object other than the `findings.kind` CHECK changed.

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
- **Who did it.** `adoptions.started_by_*`, `adoptions.confirmed_by_*` and `adoption_resolutions.resolved_by_*` are a user's `(tid, oid)`: both set (and then a real user, by the foreign key) or both `NULL`. A foreign key on two columns is not checked when one is `NULL`, so a CHECK on each pair (`adoptions_started_by_pair`, `adoptions_confirmed_by_pair`, `adoption_resolutions_resolved_by_pair`) stops half an identity from being stored as attribution.
- **Not relaxed (plan 05, A16).** The `sentinel_records.operation` CHECK still allows only `create`, `write`, `delete` and `rename`: an `Attribute` record (chmod, chown) is acknowledged and counted, not stored. Storing them would be a later contract change.
- **Cursors.** A `Cursor` is at most 512 characters and a path up to 1,024 bytes, so keyset lists over files page on `file_state.seq` (or a time and an id), never on a path (A31).

## The write additions (`0004_write_additions.sql`, prompt 06)

- **`proposals.action` accepts `'notify'`** (G8): a notify of the config-server is a write like any other, so a user who requires approval proposes it. The CHECK is replaced by a wider one in a single `ALTER TABLE`.
- **`github_credentials.expires_at timestamptz NULL`** (G9): when GitHub says the stored fine-grained token expires, read from the `GET /user` response header when the token is stored and again when it is validated. `NULL` means GitHub did not say. The UI warns a week before; nothing is called per page load.
- **Approvals hardening** (G10): see the `approvals` bullet under "Other database-side guards". This part of the migration is separable: if a human strikes it, delete the `REVOKE`, the key replacement and the trigger from the file and their tests (`the_app_cannot_change_or_remove_an_approval`, `a_proposals_author_cannot_change`, the `approvals` grant decision) together.
- **Rollout order.** The migration deploys before the 06 code. Binaries from before it never write `'notify'` or `expires_at`, never update an approval and never change a proposal's author, so they run unchanged.

## The tool-write registrations (`0005_tool_write_registrations.sql`, prompts 05 and 06)

One table, one index and one grant, and nothing existing is touched (`migration_0005_changes_existing_objects_only_by_adding` compares the catalog before and after; `migration_0005_is_expand_only` pins the statements). It is the storage behind `ports::ToolWrites` (D73): prompt 06 registers what a tool write is about to put on NFS, in the transaction that carries the write's audit event, and prompt 05 explains the next observation with it. It is a table of its own, and not rows of `file_observations`, because that table's replay key `(swimlane_id, path, observed_at, hash)` (ingestion inserts `ON CONFLICT DO NOTHING`) would let a registration swallow the agent's own observation of the same version, nothing there indexes a pending row for the bound or holds a request id, and every reader of observations (the history page, `is_version_of`, the retention rule) would have to filter the registrations out.

| Column | Meaning |
|---|---|
| `swimlane_id`, `path` | what was written or deleted; the rows go with the swimlane (`ON DELETE CASCADE`, allowed because the app may delete from the table) |
| `hash` | the content hash the tool wrote (`ToolWrites::register`), **or the hash of the file the tool deleted** (`ToolWrites::register_delete`, the expected hash that every NFS write and delete carries). Never `NULL`: both kinds are pinned to a content hash, so a delete is matched only to a deletion of that version of the file |
| `is_delete` | `false` for a write, `true` for a delete. No default: the registry states which kind it registers |
| `user_tid`, `user_oid` | the user who wrote, the actor of the attribution; **no foreign key to `users`**: a registration is a short-lived expectation, and a `users` row never decides whether a write was the tool's |
| `request_id` | the 06 request |
| `registered_at` | the registry's clock when `register` or `register_delete` ran; the 15-minute TTL (`ToolWriteEntry::TTL_SECS`) runs from here, and the stamp is for the TTL only: which pending registration is the oldest is decided by `id` (the order of insertion), since replicas stamp with their own clocks and skew is harmless for the TTL but could invert an order by stamp. A registry cannot know the commit time and a caller-owned transaction has no commit hook, so a registration expires at most the transaction's time (milliseconds) early, the safe direction. **No default, and no CHECK**: the registry passes its injected clock, and must clamp it to its own clock, never a time that a caller supplied (a time in the future would never age out of the sweep and would count towards the bound for good). A `now()`-dependent CHECK was refused on purpose |

- **Idempotency is a constraint.** `tool_write_registrations_key` is `UNIQUE (swimlane_id, path, hash, request_id, is_delete)`: a repeat is `INSERT ... ON CONFLICT DO NOTHING` and `INSERT 0 0`, whichever writer sends it, for either kind. A delete registration and a write registration of the same path, hash and request are two rows, and a delete of another version of the file (another `hash`) is another registration. **The key excludes the user**: the same key from another user is a silent no-op that keeps the first user. That is an accepted risk that rests on the uniqueness of request ids, which the hub makes and never takes from a client (`docs/threat-model.md`, request ids) and which nothing in the schema enforces, so two users should never share a key; the conformance suite pins the behaviour. The key doubles as the lookup of ingestion and the matcher (`swimlane_id = $1 AND path = ANY($2)`), and `ON CONFLICT (swimlane_id, path, hash, request_id, is_delete)`, `ON CONFLICT ON CONSTRAINT tool_write_registrations_key` and a bare `ON CONFLICT DO NOTHING` all match it.
- **The age index** `tool_write_registrations_age_idx (registered_at, id)` serves the expiry sweep (the oldest first, `LIMIT`) and the bound (the registrations newer than the cutoff, counted up to the limit). The bound itself (`Full`) is the registry's rule, not a constraint: the table has no row limit, and the rule is **soft** by at most the number of registering transactions in flight (the registry counts, then inserts).
- **Grants: `write-once` (SELECT, INSERT, DELETE).** A registration is never updated. Consumption by the observation it announced, invalidation by a newer observation of the path, and expiry are all DELETEs, and none is audited (the write's own audit event is). The `grant_decisions` entry in `db_migrations.rs` is `Grant::Immutable`.
- **The app holds no row locks.** `SELECT ... FOR UPDATE` (with or without `SKIP LOCKED`), `FOR NO KEY UPDATE` and `FOR SHARE` need `UPDATE`, which the app does not have, and fail with SQLSTATE `42501`. An expiry sweep, or a claim by several replicas, therefore uses `DELETE ... RETURNING`, or a CTE around one (`WITH gone AS (DELETE ... RETURNING ...) SELECT ...`), never a row lock; the database decides which replica deleted a row. `the_app_holds_no_row_locks_so_a_sweep_deletes_returning` pins both.
- **Checks.** The lengths of `path` (1 to 1,024, as everywhere), `user_tid` and `user_oid` (1 to 64, as `users`) and `request_id` (1 to 128, as `idempotency_keys.key`); `hash` is the 32-byte `sha256_hash` domain, never `NULL`, and `is_delete` is never `NULL` either.
- **A savepoint for every registration.** A statement that fails inside the caller's transaction aborts the whole transaction in Postgres (`25P02` for every later statement, the caller's audit insert included), so the registry runs its statements here inside a savepoint that it opens itself (the port docs require it of the registry, because the suites cannot test it and the fakes have no failed state; callers SHOULD ALSO wrap the call in a savepoint of their own, as defence in depth). A failing registration then rolls back only its own work and returns `Unavailable`, and the caller records its audit event in the same transaction. A lost connection is the exception: the transaction goes with it. Prompt 05 tests the savepoint against a real Postgres.
- **Rollout order.** The migration deploys before the 05 code that implements the port. A binary from before it never touches the table.

## Other database-side guards

- Outbox (D80): the `outbox_notify` trigger calls `pg_notify('lanekeeper_outbox', <row id>)`. PostgreSQL delivers it when the inserting transaction commits and drops it on rollback.
- `pr_links`: a partial unique index allows one `open` PR per (swimlane, path, nfs_hash).
- `approvals` (`0004`): a decision is written once. The app has SELECT and INSERT only, so it cannot re-point `approver_oid` or change a decision after the insert, and `approvals.proposal_id` is `ON DELETE RESTRICT`, so it cannot delete an approval by deleting its proposal either (a cascade would change the child rows without a privilege check). A `BEFORE INSERT` trigger (`approvals_no_self_approval`, `0001`) refuses an approval by the proposal's author, and a `BEFORE UPDATE` trigger on `proposals` (`proposals_author_immutable`, `0004`) refuses any change of `author_tid` or `author_oid` for `lanekeeper_app`, which is the role that runs the hub. Unlike the audit trigger it is an ordinary trigger, not `ENABLE ALWAYS`: the owner (`lanekeeper_migrator`) can disable it with `ALTER TABLE ... DISABLE TRIGGER`, and a superuser can bypass it with `SET session_replication_role = replica`. Neither role is the one the hub connects as, so the protection is against a compromised or buggy app, not against someone who holds the owner or a superuser. Approval enforcement (S4/D37) is still also done in application code (prompt 06), as defence in depth. A consequence: deleting a swimlane that has a decided proposal is refused with `23503`, because the swimlane's proposals cascade and a decided proposal cannot go.
- Append-only tables and cascades: a foreign key whose action is `CASCADE`, `SET NULL` or `SET DEFAULT` changes child rows without a privilege check on them, so it is a way around the grants. `sentinel_records.sentinel_id` is `ON DELETE RESTRICT`: `lanekeeper_app` has DELETE on `sentinels`, and deleting a sentinel that still has records is refused (`23503`) instead of erasing them. A sentinel with records cannot be removed before its records expire (`maintain_sentinel_partitions`); decommissioning one means keeping the row. `no_cascading_foreign_key_touches_a_protected_table` walks the catalog and fails if any foreign key into or out of a protected table (no DELETE for the app, or guarded by the append-only trigger) has an action other than `NO ACTION` or `RESTRICT`. A new migration that adds such a key must pass it.
- Sentinel records are partitioned by month. Dropping a partition is DDL, so the hub calls `SELECT * FROM maintain_sentinel_partitions(90)` from a leased job (prompts 05 and 17). It is a `SECURITY DEFINER` function owned by the migrator, with a pinned `search_path` and schema-qualified names. `EXECUTE` is granted to `lanekeeper_app` only (plan Q18). It refuses any `keep_days` below 90 (`22023`), a constant floor in the function body, so the app cannot prune records still inside their 90-day life. It also creates last month, this month and the next two, so the job must run at least monthly.

## Running the tests

`just db-verify` runs `crates/xtask/tests/db_migrations.rs` against a container of the image pinned in `deploy/dev/compose.yaml` (the test `image_matches_compose` keeps the two equal). It sets `LK_REQUIRE_DOCKER=1`: without a Docker daemon the tests fail. A plain `cargo test` skips them with a `SKIPPED` line instead.

To use an existing throwaway server instead of Docker, set `LK_TEST_PG_ADMIN_URL` to a superuser URL, for example `postgres://postgres@127.0.0.1:5432/postgres`. Each test creates its own database on it and leaves it behind. Role creation in the init SQL is serialised with a session advisory lock taken on the base database of the URL (roles are cluster-wide, and an advisory lock is per database, so a lock taken in each test's own database would serialise nothing), so parallel test threads are safe; this was run on freshly initialised clusters at `--test-threads=4` and `--test-threads=8`. If that server has no pgvector, the pgvector block is left out and the checks that need it print `NOT PROVEN`; `LK_REQUIRE_DOCKER=1` refuses such a server.
