-- 0005_tool_write_registrations.sql: the registry's half of the tool-write port (prompts 05 and 06, D73). A contract: never
-- edit it once merged, add 0006_*.sql. Run it as `lanekeeper_migrator`, behind the same guard as 0001 to 0004.
--
-- One new table, one index and one grant. Expand-only: nothing existing is altered, dropped, renamed, re-granted or
-- written, and 0001 to 0004 are untouched. The audit tables, their triggers and their grants are not touched either.
--
-- Why a table of its own (plan 05, T1 follow-up to A10): the port `ports::ToolWrites` needs a row per registration with a
-- unique key over (swimlane, path, hash, request, kind), the user, an age for the expiry and the bound, and a place that no
-- reader of history can mistake for an observation. `file_observations` has none of that: its replay key
-- `(swimlane_id, path, observed_at, hash)` lets a registration swallow the agent's own observation of the same version
-- (`ON CONFLICT DO NOTHING`), nothing indexes a pending row for the bound, and every reader of observations (the history
-- page, `is_version_of`, the last observation, the retention rule) would have to filter the registrations out.
--
-- Both kinds of registration, a write (`register`) and a delete (`register_delete`), are pinned to a content hash: the hash
-- that was written, or the hash of the file that was deleted (the expected hash that every NFS write and delete carries,
-- AGENTS rule 11). A delete is matched only to a deletion of that version of the file, so a registration that commits late
-- cannot be taken by the deletion of some other version.
--
-- The bound (`Full`) is the registry's rule and not a constraint: the table has no row limit, and the rule is soft by at most
-- the number of registering transactions in flight (a count, then an insert). The registry runs its statements inside a
-- savepoint that it opens itself (the port docs require it), so that a failing statement here cannot abort the caller's
-- transaction and the caller can still record its audit event in it (a lost connection excepted: the transaction goes with it).
--
-- Deploy order: this migration, then the 05 code that implements `ports::ToolWrites`. A binary from before it never reads
-- or writes the table, so it runs unchanged against the new schema.
--
-- Grants: one statement at the end, SELECT, INSERT and DELETE (README: write-once). A registration is never updated: it is
-- consumed by the observation it announced, dropped by a newer observation of the path, or dropped by the expiry sweep, and
-- all three are DELETEs. The `ON CONFLICT DO NOTHING` insert needs no UPDATE. No TRUNCATE, REFERENCES or TRIGGER. The
-- identity column needs no sequence grant. Without UPDATE the app also holds no row locks: `SELECT ... FOR UPDATE` (with or
-- without SKIP LOCKED), `FOR NO KEY UPDATE` and `FOR SHARE` fail with 42501. An expiry sweep, or a claim by several replicas,
-- must use `DELETE ... RETURNING` (or a CTE around one), never a row lock.

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

-- A write the tool is about to make (or has made), or a delete it is about to make (or has made), that no observation has
-- explained yet. Written by `ToolWrites::register` and `ToolWrites::register_delete` in the caller's transaction, so a row
-- exists only if that transaction committed. Consumed by the observation that explains it (ingestion, or the matcher when
-- the registration committed after the observation), dropped `ToolWriteEntry::TTL_SECS` after its `registered_at`, and dropped
-- at once by a newer observation of the path that contradicts it. Dropping is a DELETE and is not audited.
--   hash           the content hash the tool wrote (a write), or the hash of the file the tool deleted (a delete). Never NULL:
--                  both kinds are pinned to a content hash.
--   is_delete      false for a write, true for a delete. No default: the registry states which kind it registers, so a
--                  delete can never be taken for a write by omission.
--   registered_at  the registry's clock when `register` or `register_delete` ran: the TTL runs from here, and the stamp is for
--                  the TTL only. Which pending registration is the oldest is decided by `id` (the order of insertion),
--                  because replicas stamp with their own clocks: skew is harmless for the TTL but could invert an order by
--                  stamp. A registry cannot know the commit time and a caller-owned transaction has no commit hook, so a
--                  registration expires at most the transaction's time early, the safe direction. No default and no CHECK:
--                  the registry passes its injected clock (so tests can date a registration) and must clamp it to its own
--                  clock, never a time that a caller supplied; a now()-dependent CHECK is a bad idea, and a time in the
--                  future would never age out of the sweep.
--   user_tid/oid   the user who wrote, the actor of the attribution. No foreign key: the row is a short-lived expectation,
--                  and a user row never decides whether a write was the tool's.
--   request_id     the 06 request. (swimlane, path, hash, request_id, is_delete) is one registration, so a retry registers
--                  nothing twice, while a write and a delete of the same path, hash and request are two registrations, and a
--                  delete of another version of the file is another one. The key excludes the user: the same key from another
--                  user is a silent no-op that keeps the first user. That is an accepted risk that rests on the uniqueness of
--                  hub-made request ids, which nothing here enforces. The key is also the lookup of ingestion and the matcher
--                  (`swimlane_id = $1 AND path = ANY($2)`).
CREATE TABLE tool_write_registrations (
  id            bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path          text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  hash          sha256_hash NOT NULL,
  is_delete     boolean     NOT NULL,
  user_tid      text        NOT NULL CHECK (length(user_tid) BETWEEN 1 AND 64),
  user_oid      text        NOT NULL CHECK (length(user_oid) BETWEEN 1 AND 64),
  request_id    text        NOT NULL CHECK (length(request_id) BETWEEN 1 AND 128),
  registered_at timestamptz NOT NULL,
  CONSTRAINT tool_write_registrations_key UNIQUE (swimlane_id, path, hash, request_id, is_delete)
);

-- The expiry sweep and the bound: the oldest registrations first (`WHERE registered_at < cutoff ORDER BY registered_at, id
-- LIMIT n`, and the count of the registrations newer than the cutoff, up to the bound).
CREATE INDEX tool_write_registrations_age_idx ON tool_write_registrations (registered_at, id);

GRANT SELECT, INSERT, DELETE ON tool_write_registrations TO lanekeeper_app;
