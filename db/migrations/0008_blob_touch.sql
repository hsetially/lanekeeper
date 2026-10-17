-- 0008_blob_touch.sql: a blob that is stored again has its age refreshed, so that retention can use a grace period (prompts 03b
-- and 05, D47, D78; human-approved 2026-10-15, "W2"). A contract: never edit it once merged, add 0009_*.sql. Run it as
-- `lanekeeper_migrator`, behind the same guard as 0001 to 0007.
--
-- One new column and one column-level grant. Expand-only: nothing existing is altered, dropped, renamed, re-granted or
-- written, no row is updated, and 0001 to 0007 are untouched. The audit tables, their triggers and their grants are not touched.
--
-- Why (plan 05, T9 retention). The registry's garbage collection must delete blobs that nothing references. Ingestion stores a
-- blob with `BlobStore::put` on its own connection, committed at once, BEFORE the transaction that references it (and a
-- non-final part of a delta waits in `delta_staging` for up to 24 hours). `put` of a blob that already exists was a no-op that
-- kept its old age, so an old, unreferenced blob whose content reappears (a file reverted to a version from months ago) could be
-- deleted in the gap, and the transaction would then commit `file_state.nfs_hash` for a blob that is gone. `file_state`,
-- `file_observations` and `baselines` have no foreign key to `blobs`, and `blob_refs.ref_kind` has no "pending" kind, so a delete
-- that is conditional on the absence of references cannot see a reference that has not committed. The fix is in the age: `put`
-- of an existing blob refreshes `touched_at` (at most once an hour, `hub_platform::blobs::store::TOUCH_INTERVAL`), and retention
-- deletes only a blob whose `touched_at` is older than a grace period of 48 hours, longer than the 24-hour staging lifetime
-- plus that hour. The grace period is the registry's setting, not a column or a constraint here.
--
-- A column of its own, and `created_at` keeps its meaning. `blobs.created_at` is "when the version was first stored". The whole
-- repository was searched for it: no code reads it (the blob store, the registry, the docs search, the tests and the README all
-- insert and select blobs without naming it), so nothing depends on it today, and the grace period of retention is its first
-- intended reader. Granting UPDATE on `created_at` instead would have saved eight bytes a row, but then the one timestamp that
-- records when a version first appeared could be rewritten by the role that runs the hub, and "first stored" would be lost for
-- every blob that is ever stored again. `touched_at` is "when this content was last stored or stored again": a new column, so the
-- old one stays write-once.
--   touched_at  NOT NULL DEFAULT now(). A new blob gets now() from the default, so a writer that does not know the column (an older
--               binary, a test's own helper, the registry's fixtures) keeps working and its blobs count as touched when stored.
--               It is the time of the last `put` to within the touch interval (a put refreshes it only when it is older than the
--               interval), so it can lag the true last put by up to an hour, plus the wait for a row lock (a put that meets a
--               refresh or a collector in progress waits, bounded by its own timeout of 5 seconds, and writes the time it started),
--               and never leads it. A read (`get`, `get_many`) never touches it: reading is not a reference.
--
-- The migration touches no row and rewrites nothing. `DEFAULT now()` is STABLE, so Postgres 11 and later evaluates it once, keeps
-- it in the catalog and adds the column without rewriting the table or its TOAST table; the lock is the brief ACCESS EXCLUSIVE
-- of a catalog change (`existing_blobs_count_as_touched_when_the_migration_ran_and_the_table_is_not_rewritten` compares the files).
-- Every existing blob therefore reads
-- as touched at the moment the migration ran. That is intended and it is the conservative direction: the first grace period of
-- retention starts at the migration, so no existing blob, however old and however unreferenced, can be collected earlier than
-- 48 hours after it. It delays the first collection by at most the grace period; it never makes one earlier.
--
-- Grants: one statement at the end, `GRANT UPDATE (touched_at) ON blobs TO lanekeeper_app`. A COLUMN-level privilege, so the app
-- can update that one column and nothing else of a blob: not `content`, `hash`, `size` or `created_at`, by UPDATE or by the update
-- half of an upsert (all refused with SQLSTATE 42501), and no table-level UPDATE. `blobs` stays write-once at table level
-- (SELECT, INSERT, DELETE: the bytes of a stored version never change); the README names the column grant beside the class and
-- `COLUMN_GRANTS` in `crates/xtask/tests/db_migrations.rs` decides it, so that a column privilege cannot be added unseen. One
-- consequence: UPDATE on a column is enough for the row locks of `SELECT ... FOR UPDATE [SKIP LOCKED]` (a retention job can lock
-- the candidates it is about to delete, as the blockers file of prompt 05 describes), which the write-once class did not allow.
-- No TRUNCATE, REFERENCES or TRIGGER, and no sequence grant.
--
-- No index. The retention query reads `touched_at` for candidate hashes it already has (`hash = ANY($1) AND touched_at < now() -
-- grace`, the primary key first), and a scan for orphans has to look at nearly every blob whatever the cutoff, since at steady
-- state nearly every blob is older than it. An index on `touched_at` would be read by neither, and it would cost every write: an
-- index entry for every new blob, and an UPDATE of an indexed column cannot be a HOT update, so every refresh would write a new
-- index entry as well as a new row version. If a later query needs one it is an expand-only migration of its own.
--
-- Deploy order: this migration, then the 03b code that refreshes `touched_at` in `PgBlobStore::put`. A binary from before it
-- never names the column and runs unchanged against the new schema. The new binary against the old schema fails every `put`
-- (42703, `BlobError::Unavailable`), so the order matters. Any other writer of `blobs` (the registry's test support has its own
-- `put`) must refresh `touched_at` the same way, or its blobs are collectable as soon as they are old.
--
-- Rolling deploy: while the replicas are replaced, the old ones still run the old `ON CONFLICT DO NOTHING` put, which refreshes
-- nothing. The migration covers that overlap on its own: it makes every existing blob count as touched when it ran, and a blob that
-- an old replica stores gets now() from the column's default, so nothing is collectable for the first 48 hours after the migration,
-- far longer than the overlap (minutes). The retention job must not be enabled before the last old replica is gone.

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'lanekeeper_migrator')
     OR NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'lanekeeper_app') THEN
    RAISE EXCEPTION 'roles lanekeeper_migrator and lanekeeper_app must exist before migrating (deploy/dev/init.sql, CloudNativePG managed roles)'
      USING ERRCODE = '55000';
  END IF;
  IF current_user <> 'lanekeeper_migrator' THEN
    RAISE EXCEPTION 'run migrations as lanekeeper_migrator (current user: %); objects must be owned by it', current_user
      USING ERRCODE = '55000';
  END IF;
END
$$;

ALTER TABLE blobs ADD COLUMN touched_at timestamptz NOT NULL DEFAULT now();

GRANT UPDATE (touched_at) ON blobs TO lanekeeper_app;
