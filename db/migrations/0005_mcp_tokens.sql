-- 0005_mcp_tokens.sql: personal tokens for the MCP server (prompt 07). A contract: never edit it once merged.
--
-- One new table, `mcp_tokens`, with one index, two grants, one function and one trigger. Run it as `lanekeeper_migrator`.
--
-- Personal-token mode is the second way a client authenticates to `/mcp` (D52). It is never combined with OAuth. A token is
-- `lkpat_` plus 256 random bits, shown once when it is created and never again. The hub stores only
-- `HMAC-SHA-256(key, token)` (32 bytes; the key is a runtime secret, never in this database), so a copy of this table lets
-- nobody use a token, and finds a token by an indexed lookup of that hash (S7). A token lives at most 31 days in the schema (the
-- hub issues 7), so no row can be a bearer credential without an end. `revoked_at` ends a token at once and for good;
-- `last_used_at` is an operational note, not an audited event.
--
-- What `lanekeeper_app` may do (the decision of db/migrations/README.md, "append-only with two updatable columns"):
--   - SELECT and INSERT on the table;
--   - UPDATE on `revoked_at` and `last_used_at` and on no other column, so the owner, the hash, the label and the end of a
--     token are fixed when it is written. This is a column-level grant: `has_table_privilege(.., 'UPDATE')` is false for the
--     table, and the migration tests name the grant (`COLUMN_UPDATES`);
--   - no DELETE and no TRUNCATE: a token, revoked or expired, is evidence and stays. Nothing prunes the table in v1 (a row is
--     about 200 bytes and the hub creates at most a handful per user); a pruning function would be a later migration, like
--     `maintain_sentinel_partitions`.
--
-- Two guards beyond the grants:
--   - the key to `users` is ON DELETE RESTRICT, so deleting a user who has a token is refused (SQLSTATE 23503) instead of
--     erasing the token through a cascade, which changes the child rows without a privilege check. Decommissioning such a user
--     means disabling them (`users.status`), which is what the product does anyway;
--   - a BEFORE UPDATE trigger refuses any change of `revoked_at` once it is set (SQLSTATE 23514), for the app and the owner
--     alike, so a revoked token cannot be revived by clearing or moving the column. Setting it to the value it has is allowed,
--     so revoking twice is a no-op. Like `proposals_author_immutable` (0004) it is an ordinary trigger: the owner can disable
--     it and a superuser can bypass it with `session_replication_role = replica`; neither is the role the hub connects as.
--
-- Not enforced here: "at most five live tokens per user". A count cannot be a constraint, and the hub checks it when it
-- creates a token (a race can briefly allow a sixth). Revoking and expiring are enforced by the hub on every call, before the
-- user's status is looked at.
--
-- Audit: creating and revoking a token each write one audit event in the same transaction (`mcp_token_created` and
-- `mcp_token_revoked`, the wire names of `ports::AuditAction::McpTokenCreated` and `McpTokenRevoked`). `audit_events.action` is
-- free text, so the audit tables, their triggers and their grants are not touched.
--
-- Compatibility: expand-only. Nothing existing is altered, dropped, renamed or rewritten, and no row is written. Deploy order:
-- this migration, then the 07 code. Binaries from before it never touch the table.

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

CREATE TABLE mcp_tokens (
  id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_tid     text        NOT NULL,
  user_oid     text        NOT NULL,
  -- HMAC-SHA-256 of the token under a key the database never sees (S7). Unique: it is the lookup key of every call.
  token_hmac   bytea       NOT NULL UNIQUE CONSTRAINT mcp_tokens_hmac_length CHECK (octet_length(token_hmac) = 32),
  -- What the user calls it ("laptop"), shown in the list of tokens. Never the token.
  label        text        NOT NULL CONSTRAINT mcp_tokens_label_length CHECK (length(label) BETWEEN 1 AND 64),
  created_at   timestamptz NOT NULL DEFAULT now(),
  expires_at   timestamptz NOT NULL,
  revoked_at   timestamptz,
  last_used_at timestamptz,
  -- A token ends, and not later than 31 days after it was made (the hub issues 7). `infinity` is refused by the upper bound.
  CONSTRAINT mcp_tokens_lifetime CHECK (expires_at > created_at AND expires_at <= created_at + interval '31 days'),
  FOREIGN KEY (user_tid, user_oid) REFERENCES users (tid, oid) ON DELETE RESTRICT
);
-- A user's own tokens, newest first, by keyset on `id`.
CREATE INDEX mcp_tokens_user_idx ON mcp_tokens (user_tid, user_oid, id DESC);

GRANT SELECT, INSERT ON mcp_tokens TO lanekeeper_app;
GRANT UPDATE (revoked_at, last_used_at) ON mcp_tokens TO lanekeeper_app;

CREATE FUNCTION mcp_tokens_revocation_is_final() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
  IF OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at THEN
    RAISE EXCEPTION 'a revoked token stays revoked' USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END
$$;
CREATE TRIGGER mcp_tokens_revocation_is_final BEFORE UPDATE OF revoked_at ON mcp_tokens
  FOR EACH ROW EXECUTE FUNCTION mcp_tokens_revocation_is_final();
