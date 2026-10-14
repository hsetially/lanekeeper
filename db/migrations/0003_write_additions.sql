-- 0003_write_additions.sql: what the write paths need from the schema (prompt 06). A contract: never edit it once merged.
--
-- Three changes, all behind the same guard as 0001 and 0002. Run it as `lanekeeper_migrator`.
--
--   1. `proposals.action` accepts 'notify' (a notify of the config-server, which an approval-bound user proposes like any
--      other write). The CHECK is replaced by a wider one in a single statement, so no row is ever unchecked.
--   2. `github_credentials.expires_at`: when GitHub says the stored fine-grained token expires. NULL means unknown. It is
--      read from the `GET /user` response header when the token is stored and checked again, so the UI can warn a week
--      before, without a GitHub call on each page load.
--   3. The approvals hardening that 0001's own comment left to prompt 06. `lanekeeper_app` could change an approval's
--      approver or its decision after the insert, delete an approval, or change a proposal's author, and so walk round the
--      rule that nobody approves their own proposal (S4, D37). From here on:
--        - the app has SELECT and INSERT on `approvals`, nothing else. A decision is written once;
--        - `approvals.proposal_id` is ON DELETE RESTRICT instead of CASCADE. A cascade changes the child rows without a
--          privilege check, so with the old key the app could still delete an approval by deleting its proposal. With
--          RESTRICT a proposal that was decided cannot be deleted; a proposal nobody decided still can;
--        - `proposals.author_tid` and `author_oid` cannot change after the insert, for any role (BEFORE UPDATE trigger).
--      The insert-time trigger from 0001 (`approvals_no_self_approval`) is unchanged and still the first line of defence.
--
-- Compatibility: expand-only apart from the two constraints it replaces. Nothing is dropped from a table, no row is
-- rewritten, and the audit tables, their triggers and their grants are not touched. Deploy order: this migration, then the
-- 06 code. Binaries from before it never write 'notify' or `expires_at`, never update an approval and never change a
-- proposal's author, so they run unchanged against the new schema.
--
-- What changes for other code: deleting a swimlane that has a decided proposal is now refused (SQLSTATE 23503), because
-- the swimlane's proposals cascade and a decided proposal cannot go. Decommissioning such a swimlane means keeping its row,
-- like a sentinel that still has records (db/migrations/README.md).

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

-- 1. Notify proposals.
ALTER TABLE proposals
  DROP CONSTRAINT proposals_action_check,
  ADD CONSTRAINT proposals_action_check
    CHECK (action IN ('edit', 'upload', 'delete', 'revert', 'restart', 'pr', 'notify'));

-- 2. The expiry of a stored GitHub token. The table-level grant of 0001 covers the new column.
ALTER TABLE github_credentials ADD COLUMN expires_at timestamptz;

-- 3. Approvals are written once.
REVOKE UPDATE, DELETE ON approvals FROM lanekeeper_app;

ALTER TABLE approvals
  DROP CONSTRAINT approvals_proposal_id_fkey,
  ADD CONSTRAINT approvals_proposal_id_fkey
    FOREIGN KEY (proposal_id) REFERENCES proposals (id) ON DELETE RESTRICT;

CREATE FUNCTION proposals_author_immutable() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
  IF NEW.author_tid IS DISTINCT FROM OLD.author_tid OR NEW.author_oid IS DISTINCT FROM OLD.author_oid THEN
    RAISE EXCEPTION 'the author of a proposal cannot change' USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END
$$;
CREATE TRIGGER proposals_author_immutable BEFORE UPDATE OF author_tid, author_oid ON proposals
  FOR EACH ROW EXECUTE FUNCTION proposals_author_immutable();
