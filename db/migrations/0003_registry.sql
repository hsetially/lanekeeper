-- 0003_registry.sql: the registry's tables and columns (prompt 05: ingestion, indexes, the incremental pipeline, adoption,
-- sync windows, pickup state and the read API). A contract: never edit it once merged, add 0004_*.sql.
--
-- Expand-only: new columns, new indexes, eight new tables, one grant, and one CHECK that is replaced by a wider one in the
-- same transaction (`findings.kind` gains `served_mismatch`). Nothing existing is dropped, renamed, re-typed, deleted or
-- re-granted, and 0001 and 0002 are untouched. The audit tables, their triggers and their grants are not touched, and the
-- `sentinel_records.operation` CHECK is NOT relaxed: `Attribute` records are acknowledged and counted, not stored (plan
-- 05, A16). Run it as `lanekeeper_migrator`, like 0001 and 0002.
--
-- Why (plan 05, A1): the 0001 schema has no home for adoption progress that resumes, release hints, effective hashes per
-- file (for `compare_tree`), durable staging of multi-part deltas (P15: no acknowledged version is lost), folder first-seen
-- times (C11), the C7 context, normalised drift hashes, or the pipeline's version and counters.
--
-- Deploy order: this migration first, then the 05 code. An older binary ignores every new column and table (all new
-- columns are nullable or have a default). Rows written before this migration stay valid: the table has no writer before
-- prompt 05, so the unique replay key on `file_observations` cannot meet a duplicate; if one existed, the migration would
-- fail as a whole (sqlx runs the file in one transaction) rather than leave a half-applied schema.
--
-- Grants: one statement at the end. The new tables are all `mutable` (SELECT, INSERT, UPDATE, DELETE): the pipeline
-- upserts and prunes them, and every one hangs off `swimlanes` with ON DELETE CASCADE, which
-- `no_cascading_foreign_key_touches_a_protected_table` allows only for tables the app may delete from. The columns added to
-- existing tables are covered by those tables' existing table-level grants.

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

-- ---------------------------------------------------------------------------------------------------------------
-- Columns and indexes on existing tables

-- T1, T4. `initial`: the first full inventory of a swimlane is not a change (A11), so it writes no out-of-band audit event
-- and flags no pickup change. `previous_hash` and `previous_state` are the C7 context: what the file was, and its drift state,
-- when a sync overwrote it (plan 05, T4: the overwrite context is stored on the observation).
ALTER TABLE file_observations
  ADD COLUMN initial        boolean NOT NULL DEFAULT false,
  ADD COLUMN previous_hash  sha256_hash,
  ADD COLUMN previous_state text CHECK (previous_state IN
    ('in_sync', 'git_ahead', 'nfs_ahead', 'conflict', 'unknown', 'untracked', 'intentional_divergence'));

-- Replaying a delta is idempotent (T1): the same version of a file at the same instant is one observation. NULLS NOT DISTINCT
-- because a deletion has no hash, and two deletions at one instant are one. Ingestion inserts with ON CONFLICT DO NOTHING.
CREATE UNIQUE INDEX file_observations_replay_key
  ON file_observations (swimlane_id, path, observed_at, hash) NULLS NOT DISTINCT;

-- T1, T3, T6.
--   seq                  a short keyset surrogate (A31): `Cursor` holds at most 512 characters, a path up to 1,024 bytes.
--                        Assigned by the database, never by the app, and kept when a row is updated.
--   nfs_drift_hash       the normalised N of the drift table (line endings normalised). `nfs_hash` stays the raw content
--                        hash that `FileEntry.hash` and the blob URLs need.
--   observed_at          when the agent saw the current version.
--   last_observation_id  the observation behind the current row. No foreign key on purpose: retention GC deletes old
--                        observations, and a foreign key would need an index on this column for every such delete.
ALTER TABLE file_state
  ADD COLUMN seq                 bigint GENERATED ALWAYS AS IDENTITY,
  ADD COLUMN nfs_drift_hash      sha256_hash,
  ADD COLUMN observed_at         timestamptz,
  ADD COLUMN last_observation_id bigint;

-- Keyset pages over a swimlane's files ordered by `seq` (drift list, settings search); unique within a swimlane.
CREATE UNIQUE INDEX file_state_seq_idx ON file_state (swimlane_id, seq);
-- Listing the children of a folder is a range over `path`; the default operator class cannot serve a LIKE prefix under a
-- non-C collation, text_pattern_ops can (T6).
CREATE INDEX file_state_prefix_idx ON file_state (swimlane_id, path text_pattern_ops);
-- The blob policy asks "is this hash the content of a denied file?" before it serves a blob (T6, T9).
CREATE INDEX file_state_denied_hash_idx ON file_state (nfs_hash) WHERE denied AND nfs_hash IS NOT NULL;
-- The text-search file list: the current content hash, path and class of every readable file of a swimlane, from the index
-- alone (T6). Denied files and files that are gone are not in it.
CREATE INDEX file_state_current_hash_idx ON file_state (swimlane_id, nfs_hash) INCLUDE (path, class)
  WHERE nfs_hash IS NOT NULL AND NOT denied;

-- T4, T5. `hash` is the drift hash (normalised); `blob_hash` is the raw version behind the baseline, which "restore lost
-- version" needs. Retention keeps it through `blob_refs` (D78), so there is no foreign key.
ALTER TABLE baselines
  ADD COLUMN blob_hash sha256_hash;

-- T8: a window open for more than 2 hours warns once (`warned_at`); a closed window is processed once (`processed_at`).
ALTER TABLE sync_windows
  ADD COLUMN warned_at    timestamptz,
  ADD COLUMN processed_at timestamptz;

-- T10 (D85): the files whose change a service has not picked up yet.
ALTER TABLE service_state
  ADD COLUMN pending_files jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(pending_files) = 'array');

-- T2 (S13): the rule that flagged a value as a secret. The value itself is stored redacted.
ALTER TABLE settings_index
  ADD COLUMN secret_rule text CHECK (secret_rule IS NULL OR length(secret_rule) BETWEEN 1 AND 128);

-- T10 (A20): when the services were last told about a change. Written by prompt 06's notify action, read by the pickup state.
ALTER TABLE swimlanes
  ADD COLUMN last_notified_at timestamptz;

-- T10 (A3, D86): "as served" differs from what the engine renders. Widened in the same transaction, so there is never a
-- moment without the CHECK. Every old value stays.
ALTER TABLE findings DROP CONSTRAINT findings_kind_check;
ALTER TABLE findings ADD CONSTRAINT findings_kind_check CHECK (kind IN (
  'missed_base_changes', 'redundant_tenant_file', 'uneven_tenant_files', 'orphan_file', 'drift',
  'environment_host_mismatch', 'overwritten_nfs_change', 'duplicate_keys', 'ambiguous_file_name',
  'unresolved_placeholder', 'config_server_restart_required', 'served_mismatch'));

-- ---------------------------------------------------------------------------------------------------------------
-- New tables

-- The pipeline's state of a swimlane (T1, T3, T6). One row per swimlane, created by ingestion on the first hello. A
-- swimlane without a row has no inventory yet.
--   first_inventory_at  NULL until the first full inventory has been applied (A11).
--   needs_full_scan     set by a spool gap or a failed apply (A13); the next heartbeat answers RequestFullScan.
--   rules_fingerprint   a hash of the tenants, host rules, merge modes, severity rules and application names; a change makes
--                       the pipeline recompute the whole swimlane.
--   version             bumped by every pipeline commit that changes what a read returns; the ETag of the list reads.
--   state_counts        {"in_sync": n, "git_ahead": n, ...}, for the swimlane list.
CREATE TABLE swimlane_registry (
  swimlane_id        text        PRIMARY KEY REFERENCES swimlanes (id) ON DELETE CASCADE,
  first_inventory_at timestamptz,
  needs_full_scan    boolean     NOT NULL DEFAULT false,
  rules_fingerprint  sha256_hash,
  recomputed_at      timestamptz,
  version            bigint      NOT NULL DEFAULT 0 CHECK (version >= 0),
  state_counts       jsonb       NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(state_counts) = 'object'),
  finding_count      integer     NOT NULL DEFAULT 0 CHECK (finding_count >= 0),
  updated_at         timestamptz NOT NULL DEFAULT now()
);

-- What the last sync deployed (T4): one row per (swimlane, tenant), replaced by each `record_sync`. The history is the
-- audit chain. `tenant_id` is the tenant branch name (D84). `source` says how the sync was reported (the reporting option
-- is still open, D21, prompt 10), so it is bounded text and not a closed list.
CREATE TABLE swimlane_deployments (
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  tenant_id     text        NOT NULL CHECK (length(tenant_id) BETWEEN 1 AND 128),
  tenant_commit text        NOT NULL CHECK (length(tenant_commit) BETWEEN 7 AND 64),
  base_commit   text        NOT NULL CHECK (length(base_commit) BETWEEN 7 AND 64),
  source        text        NOT NULL CHECK (length(source) BETWEEN 1 AND 64),
  actor         jsonb,
  recorded_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, tenant_id)
);

-- When the hub first saw each folder of a swimlane's config tree (T1, C11): a folder created after the config-server
-- started is not a search location until it restarts. `initial`: it was in the first inventory (A11), so C11 takes it as
-- seen at the epoch. The root is not a folder here.
CREATE TABLE swimlane_folders (
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  dir_path      text        NOT NULL CHECK (length(dir_path) BETWEEN 1 AND 1024),
  first_seen_at timestamptz NOT NULL DEFAULT now(),
  initial       boolean     NOT NULL DEFAULT false,
  PRIMARY KEY (swimlane_id, dir_path)
);

-- Hints from the cluster that name the deployed tenant (T1, T5, Q19): Helm release names that match a configurable pattern,
-- as the agent reports them. Hints only, never an authority. The latest report replaces the earlier one.
CREATE TABLE release_hints (
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  namespace   text        NOT NULL CHECK (length(namespace) BETWEEN 1 AND 253),
  service     text        NOT NULL CHECK (length(service) BETWEEN 1 AND 253),
  key         text        NOT NULL CHECK (length(key) BETWEEN 1 AND 256),
  value       text        NOT NULL CHECK (length(value) <= 256),
  reported_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, namespace, service, key)
);

-- The effective hash of each logical file of a swimlane (T2, T6): the leaves that `effective_tree_hashes` (one hash per
-- directory) lacks, so that a compare of two swimlanes can descend only into directories whose hashes differ. The tenant
-- is the swimlane's first one (A30).
CREATE TABLE effective_files (
  swimlane_id  text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  logical_file text        NOT NULL CHECK (length(logical_file) BETWEEN 1 AND 1024),
  hash         sha256_hash NOT NULL,
  computed_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, logical_file)
);
-- The files below one directory, for the descent of a compare.
CREATE INDEX effective_files_prefix_idx ON effective_files (swimlane_id, logical_file text_pattern_ops);

-- One adoption pass of a swimlane (T4, T5, D22): the wizard's state, so that progress persists and resumes.
--   reason          'initial' for the first pass of a swimlane; 'tenant_suffix_changed' for the re-match that a changed
--                   tenant suffix set on NFS raises (status 'review': an admin confirms it).
--   status          'in_progress' (the wizard is open), 'review' (waits for an admin), 'confirmed', 'cancelled'.
--   step            the wizard step to resume at.
--   ranking         the ranked branches: [{branch, tenant, shared, union}, ...], best first.
--   hints           env hints and release hints shown beside the ranking.
--   tenants         the branches the admin chose; a branch name is the tenant id (D84), and a swimlane has a set (Q34).
--   tenant_commits  {tenant id: commit id}: the heads the baselines are taken from, fixed when the branch is chosen.
--   base_commit     the best-matching base commit, with its `VersionLabel` in base_label.
--   secret_scan     [{file, line, length, rule}, ...]: where a secret may be, never the value (S13).
-- At most one open adoption per swimlane and reason; a confirmed or cancelled one does not block a new one.
CREATE TABLE adoptions (
  id               bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id      text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  reason           text        NOT NULL CHECK (reason IN ('initial', 'tenant_suffix_changed')),
  status           text        NOT NULL DEFAULT 'in_progress' CHECK (status IN ('in_progress', 'review', 'confirmed', 'cancelled')),
  step             text        NOT NULL DEFAULT 'branch' CHECK (step IN ('branch', 'base_version', 'files', 'secrets', 'confirm')),
  ranking          jsonb       NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(ranking) = 'array'),
  hints            jsonb       NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(hints) = 'object'),
  tenants          text[]      NOT NULL DEFAULT '{}' CHECK (cardinality(tenants) <= 64),
  tenant_commits   jsonb       NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(tenant_commits) = 'object'),
  base_commit      text        CHECK (base_commit IS NULL OR length(base_commit) BETWEEN 7 AND 64),
  base_label       jsonb,
  secret_scan      jsonb       NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(secret_scan) = 'array'),
  started_by_tid   text,
  started_by_oid   text,
  started_at       timestamptz NOT NULL DEFAULT now(),
  updated_at       timestamptz NOT NULL DEFAULT now(),
  confirmed_by_tid text,
  confirmed_by_oid text,
  confirmed_at     timestamptz,
  FOREIGN KEY (started_by_tid, started_by_oid) REFERENCES users (tid, oid),
  FOREIGN KEY (confirmed_by_tid, confirmed_by_oid) REFERENCES users (tid, oid),
  -- A foreign key on two columns is not checked when either is NULL, so half an identity would pass it: both or neither.
  CONSTRAINT adoptions_started_by_pair CHECK ((started_by_tid IS NULL) = (started_by_oid IS NULL)),
  CONSTRAINT adoptions_confirmed_by_pair CHECK ((confirmed_by_tid IS NULL) = (confirmed_by_oid IS NULL))
);
CREATE UNIQUE INDEX adoptions_one_open ON adoptions (swimlane_id, reason) WHERE status IN ('in_progress', 'review');
CREATE INDEX adoptions_swimlane_idx ON adoptions (swimlane_id, id DESC);

-- The admin's decision for each file that differs between NFS and Git in an adoption pass (T5): "NFS is right", "Git is right"
-- or "Intentional (reason)". The hashes are what the admin looked at; a resolution whose file has changed since is stale.
CREATE TABLE adoption_resolutions (
  adoption_id     bigint      NOT NULL REFERENCES adoptions (id) ON DELETE CASCADE,
  path            text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  resolution      text        NOT NULL CHECK (resolution IN ('nfs_is_right', 'git_is_right', 'intentional')),
  reason          text        CHECK (length(reason) <= 2000),
  nfs_hash        sha256_hash,
  git_hash        sha256_hash,
  resolved_by_tid text,
  resolved_by_oid text,
  resolved_at     timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (adoption_id, path),
  FOREIGN KEY (resolved_by_tid, resolved_by_oid) REFERENCES users (tid, oid),
  CONSTRAINT adoption_resolutions_resolved_by_pair CHECK ((resolved_by_tid IS NULL) = (resolved_by_oid IS NULL))
);

-- The parts of a multi-part scan delta that arrived and were acknowledged but not yet applied (T1, A14, P15). A part is
-- staged, with its blobs already in `blobs`, and acknowledged; the last part applies the whole logical delta and clears the
-- staging. A stream that ends before the last part applies nothing, and stale staging is dropped on the next hello and by a
-- 24-hour sweep (`delta_staging_staged_idx`). `seq` is the message's sequence number (the key of the agent's ack); `part`
-- is its index within the logical delta; the roots are those every part of one logical delta must agree on.
CREATE TABLE delta_staging (
  swimlane_id text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  seq         bigint      NOT NULL CHECK (seq >= 0),
  part        integer     NOT NULL CHECK (part >= 0),
  base_root   sha256_hash,
  new_root    sha256_hash NOT NULL,
  metadata    jsonb       NOT NULL,
  staged_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (swimlane_id, seq)
);
CREATE INDEX delta_staging_staged_idx ON delta_staging (staged_at);

-- ---------------------------------------------------------------------------------------------------------------
-- Grants for lanekeeper_app (plan Q19): explicit, per table. No TRUNCATE, REFERENCES or TRIGGER, no DDL, no ownership.
-- Identity columns need no sequence grant.

GRANT SELECT, INSERT, UPDATE, DELETE ON
  swimlane_registry, swimlane_deployments, swimlane_folders, release_hints, effective_files,
  adoptions, adoption_resolutions, delta_staging
  TO lanekeeper_app;
