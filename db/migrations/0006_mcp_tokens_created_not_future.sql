-- 0006_mcp_tokens_created_not_future.sql: holds `mcp_tokens.created_at` to the moment of the insert (follow-up to 0005, prompt 07).
-- A contract: never edit it once merged. Run it as `lanekeeper_migrator`.
--
-- One statement: a CHECK on the table 0005 made. Nothing is created, dropped, renamed or granted, and no row is written.
--
-- Why. The header of 0005 says "A token lives at most 31 days in the schema ... so no row can be a bearer credential without an
-- end". That is only true from this migration on. `mcp_tokens_lifetime` bounds `expires_at` relative to `created_at`
-- (`expires_at <= created_at + interval '31 days'`), and `lanekeeper_app` has INSERT on the table, so it may supply `created_at`.
-- With 0005 alone a `created_at` ten years ahead and an `expires_at` 30 days after it satisfy every constraint, and the token
-- lives ten years. (Shown on Postgres 16: 3683 days.) 0005 is merged and is not edited; this migration closes the gap.
--
--   `mcp_tokens_created_not_future  CHECK (created_at <= now() + interval '1 minute')`
--
-- With it, `expires_at <= created_at + 31 days <= insert time + 31 days + 1 minute`: no row lives longer than 31 days and one
-- minute from the moment it was written, whatever the app supplies. A back-dated `created_at` only shortens the life
-- `mcp_tokens_lifetime` allows. The minute is slack for clock skew between the hub's clock and the database's; the hub issues 7 days.
--
-- `now()` in a CHECK. It is STABLE, not IMMUTABLE, which is acceptable for a CHECK: the expression is evaluated when a row is
-- inserted or updated, and a row only ages, so it stays true for every row that ever satisfied it (an UPDATE of `revoked_at` or
-- `last_used_at` re-checks it later and passes; a dump and restore re-checks it later than the insert and passes). No index,
-- generated column or partition bound may use `now()`, and none does.
--
-- Compatibility. It tightens what the table accepts, so it is not purely additive: a row whose `created_at` is more than a minute
-- ahead of the moment this runs makes the migration fail (SQLSTATE 23514) instead of leaving it in the table, and a human looks. No such
-- row exists unless a client of the table put it there: 0005 shipped with the 07 code not yet written, and the hub inserts with the default
-- `created_at`. The ALTER takes a short ACCESS EXCLUSIVE lock and reads a table of a handful of rows. Deploy it before the 07 code;
-- binaries from before it never touch the table. The grants, the trigger and the other constraints of 0005 are untouched.

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

ALTER TABLE mcp_tokens
  ADD CONSTRAINT mcp_tokens_created_not_future CHECK (created_at <= now() + interval '1 minute');
