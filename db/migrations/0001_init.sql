-- 0001_init.sql: the initial Lanekeeper schema (prompt 01, T6). A contract: never edit it once merged, add 0002_*.sql.
--
-- Run it as `lanekeeper_migrator`. The roles `lanekeeper_migrator` (owns the schema) and `lanekeeper_app` (what the hub
-- connects as) must already exist: this file never creates roles (plan Q6). See db/migrations/README.md.
--
-- Conventions
--   * Enum-like columns are `text` with a CHECK that lists the wire names of crates/domain (adding a value is a
--     compatible change; PostgreSQL enum types are not used).
--   * Hashes are `sha256_hash` (32 raw bytes). Times are `timestamptz`. Paths are the normalised NfsPath/RepoPath text.
--   * A user is keyed by (tid, oid), never by email (D34).
--   * Grants are explicit, per table, at the end of this file (plan Q19). There are no default privileges, so a table
--     added later without a grant decision fails `every_table_has_a_grant_decision`.

-- ---------------------------------------------------------------------------------------------------------------
-- Preconditions

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

-- No-ops when the extensions are pre-installed, which is the normal case (init.sql, CloudNativePG image).
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE DOMAIN sha256_hash AS bytea
  CONSTRAINT sha256_hash_length CHECK (octet_length(VALUE) = 32);

-- ---------------------------------------------------------------------------------------------------------------
-- Identity (S1, S2, S5, S8)

CREATE TABLE users (
  tid               text        NOT NULL CHECK (length(tid) BETWEEN 1 AND 64),
  oid               text        NOT NULL CHECK (length(oid) BETWEEN 1 AND 64),
  email             text        NOT NULL CHECK (length(email) BETWEEN 1 AND 320),
  display_name      text        NOT NULL CHECK (length(display_name) <= 256),
  -- NULL is the "none" role: signed in, nothing granted yet.
  role              text        CHECK (role IN ('viewer', 'editor', 'operator', 'admin')),
  status            text        NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'active', 'disabled')),
  requires_approval boolean     NOT NULL DEFAULT false,
  github_login      text        CHECK (length(github_login) <= 64),
  last_seen_at      timestamptz,
  created_at        timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tid, oid)
);
CREATE INDEX users_role_idx ON users (role) WHERE role IS NOT NULL;

CREATE TABLE access_requests (
  id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_tid     text        NOT NULL,
  user_oid     text        NOT NULL,
  status       text        NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
  role         text        CHECK (role IN ('viewer', 'editor', 'operator', 'admin')),
  decided_by_tid text,
  decided_by_oid text,
  reason       text        CHECK (length(reason) <= 2000),
  created_at   timestamptz NOT NULL DEFAULT now(),
  decided_at   timestamptz,
  FOREIGN KEY (user_tid, user_oid) REFERENCES users (tid, oid) ON DELETE CASCADE,
  FOREIGN KEY (decided_by_tid, decided_by_oid) REFERENCES users (tid, oid),
  CHECK ((decided_by_tid IS NULL) = (decided_by_oid IS NULL))
);
-- One open request per user.
CREATE UNIQUE INDEX access_requests_one_pending ON access_requests (user_tid, user_oid) WHERE status = 'pending';
CREATE INDEX access_requests_status_idx ON access_requests (status, id);

-- Server-side sessions (S2). Only the hash of the cookie value is stored.
CREATE TABLE sessions (
  session_hash  sha256_hash PRIMARY KEY,
  user_tid      text        NOT NULL,
  user_oid      text        NOT NULL,
  csrf_hash     sha256_hash NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  last_seen_at  timestamptz NOT NULL DEFAULT now(),
  -- Absolute expiry (12 h). The idle limit (8 h) is checked against last_seen_at.
  expires_at    timestamptz NOT NULL,
  FOREIGN KEY (user_tid, user_oid) REFERENCES users (tid, oid) ON DELETE CASCADE
);
CREATE INDEX sessions_user_idx ON sessions (user_tid, user_oid);
CREATE INDEX sessions_expires_idx ON sessions (expires_at);

-- The GitHub token vault (S8): one fresh AES-256-GCM data key per token, wrapped by Cloud KMS. The associated data
-- is the user id plus the credential id, so a ciphertext cannot be moved to another owner.
CREATE TABLE github_credentials (
  id                bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_tid          text        NOT NULL,
  user_oid          text        NOT NULL,
  github_login      text        NOT NULL CHECK (length(github_login) BETWEEN 1 AND 64),
  wrapped_dek       bytea       NOT NULL,
  kms_key_version   text        NOT NULL,
  nonce             bytea       NOT NULL CHECK (octet_length(nonce) = 12),
  ciphertext        bytea       NOT NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  last_validated_at timestamptz,
  UNIQUE (user_tid, user_oid),
  FOREIGN KEY (user_tid, user_oid) REFERENCES users (tid, oid) ON DELETE CASCADE
);

-- ---------------------------------------------------------------------------------------------------------------
-- Topology (D7, D22, D62, D84)

CREATE TABLE projects (
  id           text        PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 64),
  display_name text        NOT NULL CHECK (length(display_name) <= 256),
  created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE swimlanes (
  id                      text        PRIMARY KEY CHECK (id ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
  project_id              text        NOT NULL REFERENCES projects (id),
  display_name            text        NOT NULL CHECK (length(display_name) <= 256),
  cluster                 text,
  -- Environment tier for the C6 host check; the tier vocabulary is configurable, so it is not a CHECK list.
  tier                    text        CHECK (length(tier) <= 64),
  nfs_server              text,
  nfs_export              text,
  mount_root              text,
  -- S5: the Workload Identity service account whose Google ID token may join as this swimlane's agent.
  agent_service_account   text,
  -- D2: editing stays locked until an admin confirms the swimlane's baseline in the adoption pass.
  editing_unlocked        boolean     NOT NULL DEFAULT false,
  baseline_confirmed_at   timestamptz,
  baseline_confirmed_by_tid text,
  baseline_confirmed_by_oid text,
  -- D85: off by default.
  auto_notify             boolean     NOT NULL DEFAULT false,
  -- C11: when the config-server pod started.
  config_server_started_at timestamptz,
  created_at              timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (baseline_confirmed_by_tid, baseline_confirmed_by_oid) REFERENCES users (tid, oid)
);
CREATE INDEX swimlanes_project_idx ON swimlanes (project_id);

-- A swimlane has a set of deployed tenants, not exactly one (Q34). The tenant id is the tenant branch name (D84).
CREATE TABLE swimlane_tenants (
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  tenant_id   text        NOT NULL CHECK (length(tenant_id) BETWEEN 1 AND 128),
  added_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, tenant_id)
);
CREATE INDEX swimlane_tenants_tenant_idx ON swimlane_tenants (tenant_id);

CREATE TABLE agents (
  swimlane_id        text        PRIMARY KEY REFERENCES swimlanes (id) ON DELETE CASCADE,
  agent_version      text,
  joined_at          timestamptz,
  last_hello_at      timestamptz,
  last_heartbeat_at  timestamptz,
  scan_seq           bigint      NOT NULL DEFAULT 0,
  merkle_root        sha256_hash,
  file_count         bigint      NOT NULL DEFAULT 0,
  cert_serial        text,
  cert_expires_at    timestamptz
);

-- Which hub replica holds each agent stream (D62). The highest epoch wins.
CREATE TABLE agent_connections (
  swimlane_id     text        PRIMARY KEY REFERENCES swimlanes (id) ON DELETE CASCADE,
  replica_id      text        NOT NULL,
  replica_address text        NOT NULL,
  epoch           bigint      NOT NULL,
  connected_at    timestamptz NOT NULL DEFAULT now(),
  last_seen_at    timestamptz NOT NULL DEFAULT now()
);

-- The last scan sequence the hub durably applied; the agent keeps unacknowledged versions in its spool (D74).
CREATE TABLE agent_acks (
  swimlane_id text        PRIMARY KEY REFERENCES swimlanes (id) ON DELETE CASCADE,
  acked_seq   bigint      NOT NULL DEFAULT 0 CHECK (acked_seq >= 0),
  updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE join_tokens (
  id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  token_hash  sha256_hash NOT NULL UNIQUE,
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  created_by_tid text     NOT NULL,
  created_by_oid text     NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  expires_at  timestamptz NOT NULL,
  used_at     timestamptz,
  FOREIGN KEY (created_by_tid, created_by_oid) REFERENCES users (tid, oid)
);
CREATE INDEX join_tokens_expires_idx ON join_tokens (expires_at);

-- One rule per repo (domain-model, "Path mapping"). Q3: whether the tenant rename also applies to images and XSL.
CREATE TABLE path_mappings (
  repo                  text        PRIMARY KEY CHECK (repo IN ('base', 'tenant')),
  repo_root             text        NOT NULL CHECK (length(repo_root) BETWEEN 1 AND 1024),
  rename_all_file_types boolean     NOT NULL DEFAULT true,
  -- Which tenant branches are deployable (include) and which are not (exclude), as glob patterns.
  branch_include        text[]      NOT NULL DEFAULT '{}',
  branch_exclude        text[]      NOT NULL DEFAULT '{}',
  updated_at            timestamptz NOT NULL DEFAULT now()
);

-- Merge mode per file pattern (D14, D82). The pattern with the lowest position that matches wins.
CREATE TABLE merge_modes (
  id        bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  pattern   text        NOT NULL UNIQUE CHECK (length(pattern) BETWEEN 1 AND 512),
  mode      text        NOT NULL CHECK (mode IN ('whole_file', 'spring_merge')),
  position  integer     NOT NULL DEFAULT 100
);

-- Hostname patterns that classify an external host into an environment tier (C6).
CREATE TABLE host_tiers (
  id        bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  pattern   text        NOT NULL UNIQUE CHECK (length(pattern) BETWEEN 1 AND 512),
  tier      text        NOT NULL CHECK (length(tier) BETWEEN 1 AND 64),
  position  integer     NOT NULL DEFAULT 100
);

-- Folder -> Deployment links. swimlane_id NULL is the global default; a row with a swimlane overrides it.
CREATE TABLE folder_mappings (
  id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id text        REFERENCES swimlanes (id) ON DELETE CASCADE,
  folder_path text        NOT NULL CHECK (length(folder_path) BETWEEN 1 AND 1024),
  namespace   text        NOT NULL,
  deployment  text        NOT NULL,
  -- The tool suggests by name similarity; a person confirms.
  suggested_score real,
  confirmed   boolean     NOT NULL DEFAULT false,
  confirmed_by_tid text,
  confirmed_by_oid text,
  confirmed_at timestamptz,
  FOREIGN KEY (confirmed_by_tid, confirmed_by_oid) REFERENCES users (tid, oid)
);
CREATE UNIQUE INDEX folder_mappings_unique ON folder_mappings (swimlane_id, folder_path, namespace, deployment) NULLS NOT DISTINCT;
CREATE INDEX folder_mappings_folder_idx ON folder_mappings (folder_path);

-- A Kubernetes Deployment in a swimlane cluster (domain-model, "Services").
CREATE TABLE services (
  id            bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  namespace     text        NOT NULL,
  name          text        NOT NULL,
  -- Values of the allowlisted variables only (D88); names of the others in env_names, never their values.
  env_values    jsonb       NOT NULL DEFAULT '{}',
  env_names     text[]      NOT NULL DEFAULT '{}',
  first_seen_at timestamptz NOT NULL DEFAULT now(),
  last_seen_at  timestamptz NOT NULL DEFAULT now(),
  removed_at    timestamptz,
  UNIQUE (swimlane_id, namespace, name)
);

-- Pod start times and the per-service pickup state (D85).
CREATE TABLE service_state (
  service_id            bigint      PRIMARY KEY REFERENCES services (id) ON DELETE CASCADE,
  pods                  jsonb       NOT NULL DEFAULT '[]',
  oldest_pod_started_at timestamptz,
  newest_pod_started_at timestamptz,
  pickup_state          text        NOT NULL DEFAULT 'live'
                        CHECK (pickup_state IN ('live', 'live_within_ttl', 'needs_notify_or_restart', 'needs_config_server_restart')),
  live_by               timestamptz,
  pending_since         timestamptz,
  updated_at            timestamptz NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------------------------------------------
-- Content (D47, D65, D78, D79)

-- File versions keyed by SHA-256. lz4 TOAST compression (docs/performance.md, item 8). The CHECK keeps the table
-- content-addressed even if a writer is wrong.
CREATE TABLE blobs (
  hash       sha256_hash PRIMARY KEY,
  content    bytea       COMPRESSION lz4 NOT NULL,
  size       bigint      NOT NULL CHECK (size >= 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (hash = sha256(content))
);

-- Why a blob must be kept (D78): referenced by a baseline, audit event, proposal, draft or PR link.
CREATE TABLE blob_refs (
  hash       sha256_hash NOT NULL REFERENCES blobs (hash) ON DELETE RESTRICT,
  ref_kind   text        NOT NULL CHECK (ref_kind IN ('baseline', 'audit_event', 'proposal', 'draft', 'pr_link')),
  ref_id     text        NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (hash, ref_kind, ref_id)
);

-- Every observed version of a file, with attribution (D47, D73). hash NULL records a deletion.
CREATE TABLE file_observations (
  id                       bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id              text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path                     text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  hash                     sha256_hash,
  size                     bigint,
  source                   text        NOT NULL CHECK (source IN ('scan', 'tool_write', 'sync')),
  class                    text        CHECK (class IN ('structured', 'text', 'binary', 'denied')),
  eol_style                text        CHECK (eol_style IN ('lf', 'crlf', 'mixed')),
  encoding                 text        CHECK (encoding IN ('utf8', 'utf16le', 'utf16be', 'latin1')),
  bom                      boolean,
  mtime                    timestamptz,
  observed_at              timestamptz NOT NULL,
  -- The sync Job that was running when the change was observed (D75).
  during_job               text,
  severity                 text        CHECK (severity IN ('low', 'medium', 'high', 'critical')),
  -- Attribution (D73): upgraded later, never downgraded.
  attribution_source       text        NOT NULL DEFAULT 'unknown'
                           CHECK (attribution_source IN ('tool_write', 'sentinel_login', 'sync_job', 'nfs_client', 'fs_owner_hint', 'unknown')),
  attribution_confidence   text        NOT NULL DEFAULT 'low' CHECK (attribution_confidence IN ('low', 'medium', 'high', 'certain')),
  attribution_actor        jsonb,
  attribution_evidence     jsonb       NOT NULL DEFAULT '{}',
  recorded_at              timestamptz NOT NULL DEFAULT now()
);
-- History of one file, newest first, keyset-paginated by (observed_at, id).
CREATE INDEX file_observations_history_idx ON file_observations (swimlane_id, path, observed_at DESC, id DESC);
CREATE INDEX file_observations_hash_idx ON file_observations (hash) WHERE hash IS NOT NULL;
CREATE INDEX file_observations_swimlane_time_idx ON file_observations (swimlane_id, observed_at DESC, id DESC);

-- The adopted baseline: hash and Git commit at the last known sync (D19). Absent until adoption.
CREATE TABLE baselines (
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path          text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  hash          sha256_hash NOT NULL,
  repo          text        NOT NULL CHECK (repo IN ('base', 'tenant')),
  branch        text,
  commit_id     text        NOT NULL,
  adopted_at    timestamptz NOT NULL DEFAULT now(),
  adopted_by_tid text,
  adopted_by_oid text,
  PRIMARY KEY (swimlane_id, path),
  FOREIGN KEY (adopted_by_tid, adopted_by_oid) REFERENCES users (tid, oid)
);

-- Current drift state per path (D19, D20): the three hashes and the result.
CREATE TABLE file_state (
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path          text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  logical_file  text        NOT NULL,
  kind          text        NOT NULL CHECK (kind IN ('base', 'tenant', 'untracked')),
  class         text        NOT NULL CHECK (class IN ('structured', 'text', 'binary', 'denied')),
  role          text        CHECK (role IN ('property_source', 'resource')),
  nfs_hash      sha256_hash,
  git_hash      sha256_hash,
  baseline_hash sha256_hash,
  state         text        NOT NULL
                CHECK (state IN ('in_sync', 'git_ahead', 'nfs_ahead', 'conflict', 'unknown', 'untracked', 'intentional_divergence')),
  severity      text        CHECK (severity IN ('low', 'medium', 'high', 'critical')),
  size          bigint,
  eol_style     text        CHECK (eol_style IN ('lf', 'crlf', 'mixed')),
  encoding      text        CHECK (encoding IN ('utf8', 'utf16le', 'utf16be', 'latin1')),
  bom           boolean,
  denied        boolean     NOT NULL DEFAULT false,
  updated_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, path)
);
CREATE INDEX file_state_state_idx ON file_state (swimlane_id, state, path) WHERE state <> 'in_sync';
CREATE INDEX file_state_logical_idx ON file_state (swimlane_id, logical_file);

-- Somebody marked a divergence as intended. It holds only while N and G still equal the recorded hashes (D20).
CREATE TABLE intentional_divergence (
  swimlane_id text        NOT NULL,
  path        text        NOT NULL,
  nfs_hash    sha256_hash,
  git_hash    sha256_hash,
  reason      text        CHECK (length(reason) <= 2000),
  marked_by_tid text      NOT NULL,
  marked_by_oid text      NOT NULL,
  marked_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, path),
  FOREIGN KEY (swimlane_id, path) REFERENCES file_state (swimlane_id, path) ON DELETE CASCADE,
  FOREIGN KEY (marked_by_tid, marked_by_oid) REFERENCES users (tid, oid)
);

-- Settings index (D65): written once per unique structured blob, backing settings search, the grid and feature
-- status. Rows go when the blob is garbage-collected.
CREATE TABLE settings_index (
  blob_hash    sha256_hash NOT NULL REFERENCES blobs (hash) ON DELETE CASCADE,
  setting_path text        NOT NULL CHECK (length(setting_path) BETWEEN 1 AND 2048),
  value_text   text        NOT NULL,
  value_type   text        NOT NULL,
  start_line   integer     NOT NULL CHECK (start_line >= 1),
  end_line     integer     NOT NULL CHECK (end_line >= start_line),
  PRIMARY KEY (blob_hash, setting_path)
);
CREATE INDEX settings_index_path_trgm ON settings_index USING gin (setting_path gin_trgm_ops);

-- Marks a blob as indexed, including blobs that yield no setting rows or do not parse, so each is parsed once.
CREATE TABLE settings_index_blobs (
  blob_hash     sha256_hash PRIMARY KEY REFERENCES blobs (hash) ON DELETE CASCADE,
  indexed_at    timestamptz NOT NULL DEFAULT now(),
  setting_count integer     NOT NULL DEFAULT 0 CHECK (setting_count >= 0),
  parse_ok      boolean     NOT NULL DEFAULT true
);

-- Git tree index (D65): built once per commit, so a moving head diffs trees instead of re-reading files.
CREATE TABLE git_tree_index (
  repo      text        NOT NULL CHECK (repo IN ('base', 'tenant')),
  commit_id text        NOT NULL CHECK (length(commit_id) BETWEEN 7 AND 64),
  path      text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  object_id text        NOT NULL CHECK (length(object_id) BETWEEN 40 AND 64),
  hash      sha256_hash NOT NULL,
  PRIMARY KEY (repo, commit_id, path)
);
CREATE INDEX git_tree_index_hash_idx ON git_tree_index (hash);

-- The agent's Merkle tree (D63): a hash per directory, the root being dir_path ''.
CREATE TABLE swimlane_merkle (
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  dir_path    text        NOT NULL CHECK (length(dir_path) <= 1024),
  hash        sha256_hash NOT NULL,
  entry_count integer     NOT NULL DEFAULT 0 CHECK (entry_count >= 0),
  updated_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, dir_path)
);

-- A hash per swimlane per directory over (logical file, effective hash) pairs (D65, D67): whole-swimlane compare
-- skips subtrees whose hashes are equal.
CREATE TABLE effective_tree_hashes (
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  dir_path    text        NOT NULL CHECK (length(dir_path) <= 1024),
  hash        sha256_hash NOT NULL,
  computed_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, dir_path)
);
CREATE INDEX effective_tree_hashes_hash_idx ON effective_tree_hashes (hash);

-- Admin-configurable severity rules (D76). `matcher` holds the rule's conditions as JSON.
CREATE TABLE severity_rules (
  id          text        PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
  description text        NOT NULL DEFAULT '',
  scope       text        NOT NULL CHECK (scope IN ('change', 'finding')),
  matcher     jsonb       NOT NULL,
  severity    text        NOT NULL CHECK (severity IN ('low', 'medium', 'high', 'critical')),
  position    integer     NOT NULL DEFAULT 100,
  enabled     boolean     NOT NULL DEFAULT true,
  is_default  boolean     NOT NULL DEFAULT false,
  updated_at  timestamptz NOT NULL DEFAULT now()
);

-- Results of the consistency checks C1-C11. `fingerprint` identifies the same finding across runs.
CREATE TABLE findings (
  id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  kind        text        NOT NULL CHECK (kind IN (
                'missed_base_changes', 'redundant_tenant_file', 'uneven_tenant_files', 'orphan_file', 'drift',
                'environment_host_mismatch', 'overwritten_nfs_change', 'duplicate_keys', 'ambiguous_file_name',
                'unresolved_placeholder', 'config_server_restart_required')),
  path        text        CHECK (length(path) <= 1024),
  severity    text        NOT NULL CHECK (severity IN ('low', 'medium', 'high', 'critical')),
  rule_ids    text[]      NOT NULL DEFAULT '{}',
  message     text        NOT NULL CHECK (length(message) <= 2000),
  locations   jsonb       NOT NULL DEFAULT '[]',
  details     jsonb       NOT NULL DEFAULT '{}',
  fingerprint text        NOT NULL,
  detected_at timestamptz NOT NULL DEFAULT now(),
  resolved_at timestamptz
);
CREATE UNIQUE INDEX findings_open_fingerprint ON findings (swimlane_id, kind, fingerprint) WHERE resolved_at IS NULL;
CREATE INDEX findings_list_idx ON findings (swimlane_id, detected_at DESC, id DESC) WHERE resolved_at IS NULL;
CREATE INDEX findings_kind_idx ON findings (kind, id DESC) WHERE resolved_at IS NULL;

-- ---------------------------------------------------------------------------------------------------------------
-- Writes (D37, D44, D68, D77)

CREATE TABLE proposals (
  id             bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  author_tid     text        NOT NULL,
  author_oid     text        NOT NULL,
  action         text        NOT NULL CHECK (action IN ('edit', 'upload', 'delete', 'revert', 'restart', 'pr')),
  swimlane_id    text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  paths          text[]      NOT NULL CHECK (cardinality(paths) BETWEEN 1 AND 100),
  -- path -> expected hash (hex) or "absent".
  base_hashes    jsonb       NOT NULL DEFAULT '{}',
  new_hash       sha256_hash REFERENCES blobs (hash) ON DELETE RESTRICT,
  params         jsonb       NOT NULL DEFAULT '{}',
  via            text        NOT NULL CHECK (via IN ('ui', 'mcp')),
  status         text        NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('draft', 'pending', 'approved', 'rejected', 'stale', 'expired', 'applied')),
  request_id     text,
  created_at     timestamptz NOT NULL DEFAULT now(),
  -- 72 hours (D37).
  expires_at     timestamptz NOT NULL,
  applied_at     timestamptz,
  FOREIGN KEY (author_tid, author_oid) REFERENCES users (tid, oid)
);
CREATE INDEX proposals_status_idx ON proposals (status, id DESC);
CREATE INDEX proposals_swimlane_idx ON proposals (swimlane_id, id DESC);
CREATE INDEX proposals_expiry_idx ON proposals (expires_at) WHERE status = 'pending';

-- The decision on a proposal (D37). Nobody can approve their own proposal: the table refuses it.
CREATE TABLE approvals (
  id             bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  proposal_id    bigint      NOT NULL UNIQUE REFERENCES proposals (id) ON DELETE CASCADE,
  decision       text        NOT NULL CHECK (decision IN ('approved', 'rejected')),
  approver_tid   text        NOT NULL,
  approver_oid   text        NOT NULL,
  reason         text        CHECK (length(reason) <= 2000),
  -- The file hash found when the approval re-checked it; a mismatch makes the proposal stale.
  rechecked_hash sha256_hash,
  decided_at     timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (approver_tid, approver_oid) REFERENCES users (tid, oid)
);

CREATE FUNCTION approvals_no_self_approval() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM public.proposals p
    WHERE p.id = NEW.proposal_id AND p.author_tid = NEW.approver_tid AND p.author_oid = NEW.approver_oid
  ) THEN
    RAISE EXCEPTION 'a proposal cannot be decided by its author' USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END
$$;
CREATE TRIGGER approvals_no_self_approval BEFORE INSERT ON approvals
  FOR EACH ROW EXECUTE FUNCTION approvals_no_self_approval();

-- MCP `propose_change` drafts. The id is the DraftId token handed to the client.
CREATE TABLE drafts (
  id              text        PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
  author_tid      text        NOT NULL,
  author_oid      text        NOT NULL,
  swimlane_id     text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path            text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  expected        jsonb       NOT NULL,
  new_hash        sha256_hash NOT NULL REFERENCES blobs (hash) ON DELETE RESTRICT,
  hunks           jsonb       NOT NULL DEFAULT '[]',
  setting_changes jsonb       NOT NULL DEFAULT '[]',
  status          text        NOT NULL DEFAULT 'draft'
                  CHECK (status IN ('draft', 'pending', 'approved', 'rejected', 'stale', 'expired', 'applied')),
  created_at      timestamptz NOT NULL DEFAULT now(),
  expires_at      timestamptz NOT NULL,
  FOREIGN KEY (author_tid, author_oid) REFERENCES users (tid, oid)
);
CREATE INDEX drafts_author_idx ON drafts (author_tid, author_oid, created_at DESC);
CREATE INDEX drafts_expiry_idx ON drafts (expires_at) WHERE status IN ('draft', 'pending');

-- PR creation runs as a persisted, resumable, idempotent job (D77).
CREATE TABLE pr_jobs (
  id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  author_tid  text        NOT NULL,
  author_oid  text        NOT NULL,
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  paths       text[]      NOT NULL CHECK (cardinality(paths) BETWEEN 1 AND 100),
  title       text        NOT NULL CHECK (length(title) <= 256),
  body        text        CHECK (length(body) <= 65536),
  state       text        NOT NULL DEFAULT 'pending'
              CHECK (state IN ('pending', 'preparing', 'committing', 'opening', 'completed', 'failed')),
  -- A short, safe failure reason; never SQL, paths or tokens.
  failure     text        CHECK (length(failure) <= 500),
  attempts    integer     NOT NULL DEFAULT 0,
  step_data   jsonb       NOT NULL DEFAULT '{}',
  request_id  text,
  created_at  timestamptz NOT NULL DEFAULT now(),
  updated_at  timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (author_tid, author_oid) REFERENCES users (tid, oid)
);
CREATE INDEX pr_jobs_state_idx ON pr_jobs (state, id) WHERE state NOT IN ('completed', 'failed');

-- Every PR is linked to the swimlane, path and NFS hash it came from (D77).
CREATE TABLE pr_links (
  id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id  text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path         text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  nfs_hash     sha256_hash NOT NULL,
  repo         text        NOT NULL CHECK (repo IN ('base', 'tenant')),
  branch       text        NOT NULL,
  pr_number    integer     NOT NULL CHECK (pr_number > 0),
  state        text        NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'merged', 'closed')),
  job_id       bigint      REFERENCES pr_jobs (id) ON DELETE SET NULL,
  author_tid   text,
  author_oid   text,
  github_login text,
  url          text,
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (author_tid, author_oid) REFERENCES users (tid, oid)
);
-- At most one open PR per (swimlane, path, nfs_hash): a second PR for the same change is blocked (D77).
CREATE UNIQUE INDEX pr_links_one_open ON pr_links (swimlane_id, path, nfs_hash) WHERE state = 'open';
CREATE INDEX pr_links_lookup_idx ON pr_links (repo, pr_number);
CREATE INDEX pr_links_swimlane_idx ON pr_links (swimlane_id, path, id DESC);

-- Stored per user and key for 24 hours (D68).
CREATE TABLE idempotency_keys (
  user_tid        text        NOT NULL,
  user_oid        text        NOT NULL,
  key             text        NOT NULL CHECK (length(key) BETWEEN 1 AND 128),
  request_hash    sha256_hash NOT NULL,
  -- NULL while the first request is still running.
  response_status integer,
  response_body   bytea,
  created_at      timestamptz NOT NULL DEFAULT now(),
  expires_at      timestamptz NOT NULL,
  PRIMARY KEY (user_tid, user_oid, key)
);
CREATE INDEX idempotency_keys_expires_idx ON idempotency_keys (expires_at);

-- Replay protection for GitHub webhooks (D64): a delivery id is accepted once.
CREATE TABLE webhook_deliveries (
  delivery_id text        PRIMARY KEY CHECK (length(delivery_id) BETWEEN 1 AND 128),
  event       text        NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX webhook_deliveries_received_idx ON webhook_deliveries (received_at);

-- ---------------------------------------------------------------------------------------------------------------
-- Audit and events (S9, D56, D80)

-- The tamper-evident, hash-chained audit log. `event_json` is the exact canonical JSON that was hashed
-- (ports::audit::canonical_json), so the nightly verifier recomputes the chain from it. The columns beside it are
-- copies for querying. The table refuses a row whose `hash` is not SHA-256(prev_hash || event_json), and a second row
-- with the same `prev_hash`, which would fork the chain.
CREATE TABLE audit_events (
  seq                    bigserial   PRIMARY KEY,
  prev_hash              sha256_hash NOT NULL UNIQUE,
  hash                   sha256_hash NOT NULL UNIQUE,
  event_json             text        NOT NULL CHECK (octet_length(event_json) <= 1048576),
  at                     timestamptz NOT NULL,
  actor_kind             text        NOT NULL CHECK (actor_kind IN ('user', 'agent', 'sentinel', 'system')),
  actor_ref              text,
  action                 text        NOT NULL CHECK (length(action) BETWEEN 1 AND 64),
  via                    text        NOT NULL CHECK (via IN ('ui', 'mcp', 'sync', 'agent_detected', 'system')),
  swimlane_id            text,
  path                   text,
  hash_before            sha256_hash,
  hash_after             sha256_hash,
  github_login           text,
  approval_id            bigint,
  request_id             text,
  -- Attribution (D73).
  attribution_source     text        CHECK (attribution_source IN ('tool_write', 'sentinel_login', 'sync_job', 'nfs_client', 'fs_owner_hint', 'unknown')),
  attribution_confidence text        CHECK (attribution_confidence IN ('low', 'medium', 'high', 'certain')),
  attribution_actor      jsonb,
  attribution_evidence   jsonb,
  recorded_at            timestamptz NOT NULL DEFAULT now(),
  UNIQUE (seq, hash),
  CONSTRAINT audit_events_hash_chain CHECK (hash = sha256(prev_hash || convert_to(event_json, 'UTF8')))
);
CREATE INDEX audit_events_swimlane_idx ON audit_events (swimlane_id, seq DESC) WHERE swimlane_id IS NOT NULL;
CREATE INDEX audit_events_path_idx ON audit_events (swimlane_id, path, seq DESC) WHERE path IS NOT NULL;
CREATE INDEX audit_events_actor_idx ON audit_events (actor_ref, seq DESC) WHERE actor_ref IS NOT NULL;
CREATE INDEX audit_events_at_idx ON audit_events (at);

-- Hourly KMS-signed checkpoints of the chain head, also written to a GCS bucket with a locked retention policy.
CREATE TABLE audit_checkpoints (
  seq             bigint      PRIMARY KEY,
  head_hash       sha256_hash NOT NULL,
  kms_key_version text        NOT NULL,
  signature       bytea       NOT NULL,
  gcs_object      text        NOT NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (seq, head_hash) REFERENCES audit_events (seq, hash)
);

-- Nobody rewrites history: not the app (no privilege), and not the owner or a superuser, because the trigger fires
-- for them too, in every session_replication_role (ENABLE ALWAYS). Changing this takes a migration that drops it.
CREATE FUNCTION audit_append_only() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
  RAISE EXCEPTION '% on % is refused: the audit log is append-only', TG_OP, TG_TABLE_NAME
    USING ERRCODE = '23001';
END
$$;

CREATE TRIGGER audit_events_no_row_change BEFORE UPDATE OR DELETE ON audit_events
  FOR EACH ROW EXECUTE FUNCTION audit_append_only();
CREATE TRIGGER audit_events_no_truncate BEFORE TRUNCATE ON audit_events
  FOR EACH STATEMENT EXECUTE FUNCTION audit_append_only();
CREATE TRIGGER audit_checkpoints_no_row_change BEFORE UPDATE OR DELETE ON audit_checkpoints
  FOR EACH ROW EXECUTE FUNCTION audit_append_only();
CREATE TRIGGER audit_checkpoints_no_truncate BEFORE TRUNCATE ON audit_checkpoints
  FOR EACH STATEMENT EXECUTE FUNCTION audit_append_only();
ALTER TABLE audit_events ENABLE ALWAYS TRIGGER audit_events_no_row_change;
ALTER TABLE audit_events ENABLE ALWAYS TRIGGER audit_events_no_truncate;
ALTER TABLE audit_checkpoints ENABLE ALWAYS TRIGGER audit_checkpoints_no_row_change;
ALTER TABLE audit_checkpoints ENABLE ALWAYS TRIGGER audit_checkpoints_no_truncate;

-- The transactional outbox (D80). A row is written in the same transaction as its change. The trigger's NOTIFY is
-- delivered by PostgreSQL only when that transaction commits, so nothing is fanned out for a rolled-back change.
-- The payload is the row id; a poller picks up rows still unpublished after a missed NOTIFY.
CREATE TABLE outbox (
  id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  event        jsonb       NOT NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  published_at timestamptz
);
CREATE INDEX outbox_unpublished_idx ON outbox (id) WHERE published_at IS NULL;

CREATE FUNCTION outbox_notify() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
  PERFORM pg_notify('lanekeeper_outbox', NEW.id::text);
  RETURN NULL;
END
$$;
CREATE TRIGGER outbox_notify AFTER INSERT ON outbox
  FOR EACH ROW EXECUTE FUNCTION outbox_notify();

-- Singleton jobs hold a lease (D62). `fence` increases on every takeover.
CREATE TABLE leases (
  name        text        PRIMARY KEY CHECK (length(name) BETWEEN 1 AND 128),
  holder      text        NOT NULL,
  expires_at  timestamptz NOT NULL,
  fence       bigint      NOT NULL DEFAULT 1,
  acquired_at timestamptz NOT NULL DEFAULT now()
);

-- A sync Job is running in a swimlane (D75). The hub holds drift alerts until the window closes.
CREATE TABLE sync_windows (
  id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  job_name    text        NOT NULL,
  job_uid     text        NOT NULL,
  opened_at   timestamptz NOT NULL,
  closed_at   timestamptz,
  UNIQUE (swimlane_id, job_uid)
);
CREATE INDEX sync_windows_open_idx ON sync_windows (swimlane_id) WHERE closed_at IS NULL;

-- ---------------------------------------------------------------------------------------------------------------
-- Sentinel (D72, Q18, Q32)

CREATE TABLE sentinels (
  id                  bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  vm                  text        NOT NULL UNIQUE CHECK (length(vm) BETWEEN 1 AND 253),
  swimlane_id         text        REFERENCES swimlanes (id) ON DELETE SET NULL,
  export_root         text        NOT NULL,
  version             text,
  service_account     text,
  enrolled_at         timestamptz NOT NULL DEFAULT now(),
  last_seen_at        timestamptz,
  -- The last record batch applied, acknowledged to the sentinel (D74).
  last_acked_seq      bigint      NOT NULL DEFAULT 0 CHECK (last_acked_seq >= 0)
);

-- Local audit records from the NFS VM, by month. Retention (90 days) is by dropping whole partitions, which is DDL,
-- so it is done only by maintain_sentinel_partitions(), which the migrator owns (below). A default partition means an
-- insert never fails for lack of a partition.
CREATE TABLE sentinel_records (
  sentinel_id    bigint      NOT NULL REFERENCES sentinels (id) ON DELETE CASCADE,
  batch_seq      bigint      NOT NULL,
  record_index   integer     NOT NULL,
  observed_at    timestamptz NOT NULL,
  path           text        NOT NULL CHECK (length(path) BETWEEN 1 AND 4096),
  operation      text        NOT NULL CHECK (operation IN ('create', 'write', 'delete', 'rename')),
  success        boolean     NOT NULL,
  login_user     text,
  effective_user text,
  exe            text,
  comm           text,
  received_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (sentinel_id, batch_seq, record_index, observed_at)
) PARTITION BY RANGE (observed_at);
CREATE TABLE sentinel_records_default PARTITION OF sentinel_records DEFAULT;
-- Correlation with a file change: same path within +-60 seconds (D73).
CREATE INDEX sentinel_records_path_idx ON sentinel_records (sentinel_id, path, observed_at);

-- Creates last month, this month and the next two, moves default-partition rows into a new partition, and drops whole
-- months that ended before now() - keep_days, plus default-partition rows older than that. Months are UTC.
-- Retention floor (S9, S21): keep_days below 90 is refused (22023). lanekeeper_app holds EXECUTE and sentinel_records is
-- otherwise insert-only, so a caller that could pass a short keep_days could erase attribution evidence still inside its
-- 90-day life. The floor is a constant in the body below: it is not a parameter, a setting or a row an app can write.
-- Raising it is a new migration; lowering retention below 90 days needs a human decision in docs/decisions.md.
-- SECURITY DEFINER: runs with the migrator's rights, so lanekeeper_app needs no DDL right of its own (Q18). Every name
-- is schema-qualified and search_path is pinned, so a temp table or a search_path change cannot redirect it.
-- Concurrent calls are serialised with an advisory lock. Dropping a partition briefly locks the parent table.
CREATE FUNCTION maintain_sentinel_partitions(keep_days integer DEFAULT 90, OUT created integer, OUT dropped integer)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
  min_keep_days CONSTANT integer := 90;
  cutoff    timestamptz;
  month_ts  timestamp;
  lo        timestamptz;
  hi        timestamptz;
  part      text;
  r         record;
  ym        text[];
BEGIN
  IF keep_days IS NULL OR keep_days < min_keep_days OR keep_days > 3650 THEN
    RAISE EXCEPTION 'keep_days must be between % and 3650', min_keep_days USING ERRCODE = '22023';
  END IF;
  created := 0;
  dropped := 0;
  PERFORM pg_advisory_xact_lock(hashtextextended('lanekeeper.maintain_sentinel_partitions', 0));
  cutoff := now() - make_interval(days => keep_days);

  FOR i IN -1..2 LOOP
    month_ts := date_trunc('month', now() AT TIME ZONE 'UTC') + make_interval(months => i);
    lo := month_ts AT TIME ZONE 'UTC';
    hi := (month_ts + interval '1 month') AT TIME ZONE 'UTC';
    part := format('sentinel_records_y%sm%s', to_char(month_ts, 'YYYY'), to_char(month_ts, 'MM'));
    IF to_regclass(format('public.%I', part)) IS NULL THEN
      EXECUTE format('CREATE TABLE public.%I (LIKE public.sentinel_records INCLUDING DEFAULTS INCLUDING CONSTRAINTS)', part);
      -- ATTACH refuses while the default partition holds rows of this range, so move them first.
      EXECUTE format(
        'WITH moved AS (DELETE FROM ONLY public.sentinel_records_default WHERE observed_at >= %L AND observed_at < %L RETURNING *) '
        'INSERT INTO public.%I SELECT * FROM moved', lo, hi, part);
      EXECUTE format('ALTER TABLE public.sentinel_records ATTACH PARTITION public.%I FOR VALUES FROM (%L) TO (%L)', part, lo, hi);
      created := created + 1;
    END IF;
  END LOOP;

  FOR r IN
    SELECT c.relname::text AS name
    FROM pg_catalog.pg_inherits i
    JOIN pg_catalog.pg_class c ON c.oid = i.inhrelid
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE i.inhparent = 'public.sentinel_records'::regclass AND n.nspname = 'public'
  LOOP
    ym := regexp_match(r.name, '^sentinel_records_y([0-9]{4})m([0-9]{2})$');
    CONTINUE WHEN ym IS NULL;
    hi := (make_timestamp(ym[1]::int, ym[2]::int, 1, 0, 0, 0) + interval '1 month') AT TIME ZONE 'UTC';
    IF hi <= cutoff THEN
      EXECUTE format('DROP TABLE public.%I', r.name);
      dropped := dropped + 1;
    END IF;
  END LOOP;

  DELETE FROM ONLY public.sentinel_records_default WHERE observed_at < cutoff;
END
$$;

REVOKE ALL ON FUNCTION maintain_sentinel_partitions(integer) FROM PUBLIC;

-- The partitions the current months need exist from the start.
SELECT created FROM maintain_sentinel_partitions(90);

-- Maps an OS Login user to an application user (Q32).
CREATE TABLE os_login_user_map (
  os_login       text        PRIMARY KEY CHECK (length(os_login) BETWEEN 1 AND 128),
  user_tid       text        NOT NULL,
  user_oid       text        NOT NULL,
  created_by_tid text,
  created_by_oid text,
  created_at     timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (user_tid, user_oid) REFERENCES users (tid, oid) ON DELETE CASCADE,
  FOREIGN KEY (created_by_tid, created_by_oid) REFERENCES users (tid, oid)
);

-- ---------------------------------------------------------------------------------------------------------------
-- Docs (D53, D57)

CREATE TABLE docs (
  id                 bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  title              text        NOT NULL CHECK (length(title) BETWEEN 1 AND 512),
  source             text        NOT NULL CHECK (source IN ('upload', 'git')),
  -- source = 'git': the repo, branch and path (for example the tenant repo's templates/common-docs). 'upload': the name.
  repo               text        CHECK (repo IN ('base', 'tenant')),
  branch             text,
  path               text        CHECK (length(path) <= 1024),
  upload_name        text        CHECK (length(upload_name) <= 256),
  -- The doc's current version; the foreign key is added below, once doc_versions exists.
  current_version_id bigint,
  created_by_tid     text,
  created_by_oid     text,
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (created_by_tid, created_by_oid) REFERENCES users (tid, oid),
  CHECK (
    (source = 'git' AND repo IS NOT NULL AND branch IS NOT NULL AND path IS NOT NULL AND upload_name IS NULL)
    OR (source = 'upload' AND upload_name IS NOT NULL AND repo IS NULL AND branch IS NULL AND path IS NULL)
  )
);
CREATE UNIQUE INDEX docs_git_unique ON docs (repo, branch, path) WHERE source = 'git';
CREATE UNIQUE INDEX docs_upload_unique ON docs (upload_name) WHERE source = 'upload';

CREATE TABLE doc_versions (
  id             bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  doc_id         bigint      NOT NULL REFERENCES docs (id) ON DELETE CASCADE,
  -- The original in GCS (uploads), and the extracted text for the docs filesystem and grep.
  gcs_object     text,
  content_hash   sha256_hash NOT NULL,
  body           text        NOT NULL CHECK (octet_length(body) <= 2097152),
  source_commit  text,
  created_by_tid text,
  created_by_oid text,
  created_at     timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (created_by_tid, created_by_oid) REFERENCES users (tid, oid)
);
CREATE INDEX doc_versions_doc_idx ON doc_versions (doc_id, id DESC);
ALTER TABLE docs ADD CONSTRAINT docs_current_version_fk
  FOREIGN KEY (current_version_id) REFERENCES doc_versions (id) DEFERRABLE INITIALLY DEFERRED;

-- One section of a doc version, split at H2 headings. The embedding column and its HNSW index are added below,
-- between the pgvector markers.
CREATE TABLE doc_chunks (
  id             bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  doc_version_id bigint      NOT NULL REFERENCES doc_versions (id) ON DELETE CASCADE,
  ordinal        integer     NOT NULL CHECK (ordinal >= 0),
  title          text        NOT NULL,
  heading_path   text[]      NOT NULL DEFAULT '{}',
  body           text        NOT NULL,
  start_line     integer,
  end_line       integer,
  -- Backticked config paths and setting paths the section mentions.
  related_files  text[]      NOT NULL DEFAULT '{}',
  setting_paths  text[]      NOT NULL DEFAULT '{}',
  tsv            tsvector    GENERATED ALWAYS AS (to_tsvector('english'::regconfig, title || ' ' || body)) STORED,
  UNIQUE (doc_version_id, ordinal)
);
CREATE INDEX doc_chunks_tsv_idx ON doc_chunks USING gin (tsv);
CREATE INDEX doc_chunks_related_files_idx ON doc_chunks USING gin (related_files);
CREATE INDEX doc_chunks_setting_paths_idx ON doc_chunks USING gin (setting_paths);

-- lk:pgvector:begin
-- Everything that needs the pgvector extension (>= 0.5, for HNSW) sits between these markers, so that a server without
-- it can still run the rest of the file in tests. Production always has it (the CloudNativePG image).
CREATE EXTENSION IF NOT EXISTS vector;
ALTER TABLE doc_chunks ADD COLUMN embedding vector(384);
CREATE INDEX doc_chunks_embedding_hnsw ON doc_chunks USING hnsw (embedding vector_cosine_ops);
-- lk:pgvector:end

-- A flag parsed from a doc's Feature Flags table (domain-model, "Docs"). setting_pattern is the resolved setting path.
CREATE TABLE documented_flags (
  id               bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  doc_id           bigint      NOT NULL REFERENCES docs (id) ON DELETE CASCADE,
  flag             text        NOT NULL,
  file             text        NOT NULL,
  channel          text,
  template_default text,
  description      text,
  setting_pattern  text        NOT NULL,
  updated_at       timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX documented_flags_unique ON documented_flags (doc_id, flag, file, channel) NULLS NOT DISTINCT;
CREATE INDEX documented_flags_pattern_idx ON documented_flags (setting_pattern);

-- ---------------------------------------------------------------------------------------------------------------
-- Settings

-- A single row. D78: keep the last N versions of a file and every version newer than M days.
CREATE TABLE retention_settings (
  singleton            boolean     PRIMARY KEY DEFAULT true CHECK (singleton),
  keep_versions        integer     NOT NULL DEFAULT 50 CHECK (keep_versions >= 1),
  keep_days            integer     NOT NULL DEFAULT 180 CHECK (keep_days >= 1),
  sentinel_keep_days   integer     NOT NULL DEFAULT 90 CHECK (sentinel_keep_days BETWEEN 1 AND 3650),
  updated_at           timestamptz NOT NULL DEFAULT now()
);
INSERT INTO retention_settings DEFAULT VALUES;

-- Teams notification routing (D49). `destination_ref` names a Secret Manager secret: the webhook URL is a secret and
-- is never stored in the database (S7).
CREATE TABLE notification_settings (
  event_kind      text        PRIMARY KEY CHECK (length(event_kind) BETWEEN 1 AND 64),
  enabled         boolean     NOT NULL DEFAULT true,
  min_severity    text        NOT NULL DEFAULT 'low' CHECK (min_severity IN ('low', 'medium', 'high', 'critical')),
  destination_ref text,
  updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE feature_flags (
  name        text        PRIMARY KEY CHECK (name ~ '^[a-z0-9_]{1,64}$'),
  enabled     boolean     NOT NULL DEFAULT false,
  description text,
  updated_at  timestamptz NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------------------------------------------
-- Grants for lanekeeper_app (plan Q19): explicit, per table, least privilege. No TRUNCATE, REFERENCES or TRIGGER
-- anywhere, no DDL, no ownership. Tables not listed here (_sqlx_migrations) are invisible to the app.

-- History nobody may rewrite or prune. Sentinel retention is the definer function above, not DELETE.
GRANT SELECT, INSERT ON audit_events, audit_checkpoints, sentinel_records TO lanekeeper_app;

-- Content-addressed or write-once rows, removed only by retention or garbage collection.
GRANT SELECT, INSERT, DELETE ON
  blobs, blob_refs, settings_index, settings_index_blobs, git_tree_index, doc_versions, webhook_deliveries
  TO lanekeeper_app;

GRANT SELECT, INSERT, UPDATE, DELETE ON
  users, access_requests, sessions, github_credentials, join_tokens,
  projects, swimlanes, swimlane_tenants, agents, agent_connections, agent_acks, path_mappings, merge_modes, host_tiers,
  folder_mappings, services, service_state,
  file_observations, baselines, file_state, intentional_divergence, swimlane_merkle, effective_tree_hashes, findings,
  severity_rules,
  proposals, drafts, approvals, pr_links, pr_jobs, idempotency_keys,
  outbox, leases, sync_windows,
  sentinels, os_login_user_map,
  docs, doc_chunks, documented_flags,
  retention_settings, notification_settings, feature_flags
  TO lanekeeper_app;

-- audit_events.seq is a bigserial. The identity columns elsewhere need no grant.
GRANT USAGE ON SEQUENCE audit_events_seq_seq TO lanekeeper_app;

-- The one DDL path the app has (Q18).
GRANT EXECUTE ON FUNCTION maintain_sentinel_partitions(integer) TO lanekeeper_app;
