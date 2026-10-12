-- 0002_audit_verify_state.sql: the nightly audit verifier's consecutive-failure count (prompt 03a). A contract: never
-- edit it once merged.
--
-- Expand-only: one new table, one seeded row, one grant. Nothing existing is dropped, altered or re-granted, and the audit
-- tables, their triggers and their grants are not touched. Run it as `lanekeeper_migrator`, like 0001.
--
-- Why: the nightly verifier alerts when verification ends without a verdict two nights in a row. The `Leases` port has no
-- payload, so a count held in memory is lost when the pod restarts or the lease moves to another replica, and a pod that
-- restarts daily would never reach two. The verifier reads this row under its lease and updates it after each run.
--
-- What it is not: it is operational state, like `last_seen_at`, and not part of the audit chain. It has no trigger, is not
-- tamper-evident, and an update of it writes no audit event. A compromised `lanekeeper_app` can reset the count, which is
-- why the Prometheus alerts that read history stay the recommended ones (db/migrations/README.md).
--
-- Deploy order: this migration first, then the 03a code that reads it. An older binary ignores the table. The new
-- verifier treats a missing table as a changed schema (an alert).

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

-- A single row. `id` can only be true and is the primary key, so a second row cannot exist.
--   unfinished_runs: nights in a row that ended without a verdict; a night that ran to the end sets it back to 0.
--   last_error:      the class of the last such error, or a SQLSTATE. Never a value, an error text or an event (S10); the
--                    cap keeps the column from holding more.
--   updated_at:      when the verifier last wrote the row. The writer sets it on every update.
CREATE TABLE audit_verify_state (
  id              boolean     PRIMARY KEY DEFAULT true CHECK (id),
  unfinished_runs integer     NOT NULL CHECK (unfinished_runs >= 0),
  last_error      text        CHECK (last_error IS NULL OR char_length(last_error) <= 200),
  updated_at      timestamptz NOT NULL DEFAULT now()
);
INSERT INTO audit_verify_state (id, unfinished_runs, last_error) VALUES (true, 0, NULL);

-- SELECT and UPDATE only. The migration seeds the row, so the app needs no INSERT; without DELETE and TRUNCATE the app
-- cannot remove it either. No ON CONFLICT upsert is possible for the app, by design.
GRANT SELECT, UPDATE ON audit_verify_state TO lanekeeper_app;
