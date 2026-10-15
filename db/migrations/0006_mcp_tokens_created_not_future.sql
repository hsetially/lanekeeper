-- 0006_mcp_tokens_created_not_future.sql: holds `mcp_tokens.created_at` to the moment of the insert (follow-up to 0005, prompt 07).
-- A contract: never edit it once merged. Run it as `lanekeeper_migrator`.
--
-- One function and one BEFORE INSERT trigger on the table 0005 made. No table, column, index or grant changes and no row is written.
--
-- Why. The header of 0005 says "A token lives at most 31 days in the schema ... so no row can be a bearer credential without an
-- end". That is only true from this migration on. `mcp_tokens_lifetime` bounds `expires_at` relative to `created_at`
-- (`expires_at <= created_at + interval '31 days'`), and `lanekeeper_app` has INSERT on the table, so it may supply `created_at`.
-- With 0005 alone a `created_at` ten years ahead and an `expires_at` 30 days after it satisfy every constraint, and the token
-- lives ten years. (Shown on Postgres 16: 3683 days.) 0005 is merged and is not edited; this migration closes the gap.
--
-- The rule: an INSERT whose `created_at` is later than `now() + interval '1 minute'` is refused with SQLSTATE 23514
-- (check_violation), naming `mcp_tokens_created_not_future` as the constraint of the error. With it,
-- `expires_at <= created_at + 31 days <= insert time + 31 days + 1 minute`: no row lives longer than 31 days and one minute from
-- the moment it was written, whatever the app supplies. A back-dated `created_at` only shortens the life `mcp_tokens_lifetime`
-- allows. The minute is slack for clock skew between the hub's clock and the database's; the hub issues 7 days.
--
-- Why a trigger on INSERT and not a CHECK. A CHECK is re-evaluated on every UPDATE of the row, and this one reads the clock: if the
-- database clock stepped back by more than the slack, an UPDATE of `revoked_at` or `last_used_at` on a recently created token would
-- fail, and a token could not be revoked. A BEFORE INSERT trigger runs once, when the row is written, and never on an UPDATE, so
-- a clock step can never block a revocation or a use. A normal dump and restore creates triggers after it loads the data (they
-- are "post-data" in pg_dump), so it does not re-fire it either; a data-only restore fires it like any insert, and a row only ages.
-- Like the trigger of 0005 it is an ordinary trigger: no SECURITY DEFINER, a pinned search_path, owned by the migrator. The owner can
-- disable it and a superuser can bypass it with `session_replication_role = replica`; neither is the role the hub connects as.
-- It is not a catalog constraint, so `pg_constraint` does not list it; the trigger is the guard.
--
-- Compatibility. It tightens what the table accepts, so it is not purely additive: an INSERT that 0005 took is now refused when its
-- `created_at` is more than a minute ahead. A trigger does not look at rows that are already there, so the migration does: a row
-- whose `created_at` is more than a minute ahead of the moment it runs makes it fail (23514) and a person looks. None exists unless a
-- client of the table put it there; the hub inserts with the default `created_at`, and the 07 code that inserts is not written, so
-- nothing in use is affected. Deploy it before the 07 code; binaries from before it never touch the table. The grants, the trigger
-- and the constraints of 0005 are untouched.

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
  -- The trigger below guards inserts; a row that is already there with a created_at ahead of the slack would keep its long life.
  IF EXISTS (SELECT 1 FROM mcp_tokens WHERE created_at > now() + interval '1 minute') THEN
    RAISE EXCEPTION 'mcp_tokens has a row whose created_at is in the future; look at it before migrating'
      USING ERRCODE = '23514';
  END IF;
END
$$;

CREATE FUNCTION mcp_tokens_created_not_future() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
  IF NEW.created_at > now() + interval '1 minute' THEN
    RAISE EXCEPTION 'a token cannot be created in the future'
      USING ERRCODE = '23514', CONSTRAINT = 'mcp_tokens_created_not_future';
  END IF;
  RETURN NEW;
END
$$;
CREATE TRIGGER mcp_tokens_created_not_future BEFORE INSERT ON mcp_tokens
  FOR EACH ROW EXECUTE FUNCTION mcp_tokens_created_not_future();
