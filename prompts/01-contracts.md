# 01: Contracts, skeleton, ports, gates and fixtures

| | |
|---|---|
| **Wave** | 0. One agent; a human reviews closely before merge. |
| **Depends on** | Nothing |
| **You own** | Workspace root files, `crates/domain`, `crates/ports`, `crates/proto`, `crates/xtask`, `proto/`, `api/openapi.yaml`, `db/migrations/`, `docs/mcp-tools.md`, `perf/budgets.toml`, `fuzz/` (skeleton), `deploy/dev/`, `CODEOWNERS` |
| **Interfaces** | Creates every trait in `docs/interfaces.md`, with fakes |
| **Security** | S4 (route-guard test harness), S11 (typed validators), S19 (dependency policy), S21, S22 (fuzz skeleton) |
| **Performance** | Creates the budget registry for P1–P13 |
| **Gate** | `just verify-01` |

## Objective

Every other agent builds against what this prompt creates:

- typed contracts;
- port traits with in-memory fakes;
- the verification gates;
- the benchmark budget registry;
- a synthetic data generator that mirrors the real config layout.

Make all of it precise enough that wave-1 agents never have to guess.

## Tasks

### T1: Workspace and gates

**The scaffold already provides** the workspace, crate stubs, lints, `deny.toml`, `Justfile`, CI with SHA-pinned actions, Renovate, the web skeleton (with its lockfile) and local Postgres. Verify and extend these rather than recreating them. The `toolchain` channel in `rust-toolchain.toml` is pinned; bump it only if CI's runners need to. The bullets below remain the acceptance target.


- A Cargo workspace with every crate in the AGENTS.md layout, compiling as stubs.
- `rust-toolchain.toml` pins the toolchain.
- Workspace lints:
  - forbid `unsafe_code`;
  - deny `clippy::unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented` and `dbg_macro`, with these allowed in tests;
  - `clippy::pedantic` as warnings.
- `deny.toml` covers advisories, licences, bans and sources (crates.io only).
- A `Justfile` with:
  - `verify`: fmt, clippy, tests, `cargo deny`, `cargo audit`, and web lint, typecheck and test;
  - `verify-NN` for every prompt. Each starts as a stub that fails with "not implemented", and the owning prompt replaces it;
  - `bench`, `fuzz-smoke` and `dev`.
- `deploy/dev/compose.yaml` runs Postgres 16 with pgvector and pg_trgm.

**Verify:** `just verify` passes on the skeleton.

### T2: `crates/domain`

Write all the types named in `docs/interfaces.md` and `docs/domain-model.md`.

- **Validated newtypes:**
  - `NfsPath`: relative, no `..`, no NUL, at most 1,024 bytes, normalised separators.
  - `RepoPath`, `LogicalFile`, `SettingPath`, `ContentHash`, `CommitId`, `SwimlaneId` and `UserId` (tid plus oid).
- **Enums:**
  - `Role`, ordered so that Viewer < Editor < Operator < Admin.
  - `UserStatus`, `FileKind`, `FileClass`, `EolStyle`, `MergeMode`, `DriftState`, `Via`, `ProposalStatus`, `FindingKind` (C1–C8) and `DomainEvent`.
- **Rules and parsers:**
  - `PathMappingRule`, with forward and reverse mapping, including the branch-suffix rename;
  - `CompareRef`, parsed from a string;
  - `SettingLocation`.
  - `FileRole {PropertySource, Resource}`.
  - `PickupState {Live, LiveWithinTtl, NeedsNotifyOrRestart, NeedsConfigServerRestart}`.
  - `TenantId`.
  - `ServeRequest {application, tenant, channel?, file}`.
- **Secrets:** `Secret<T>`, which wraps secrecy and zeroizes. It has no `Serialize`, and its `Debug` prints `[redacted]`.

**Verify:** unit and property tests (proptest):

- `NfsPath` rejects every traversal form;
- path mapping round-trips:
  - `data/config/tx-infinity-api/tx-infinity-core.yml` on branch `sit1` maps to `tx-infinity-api/tx-infinity-core-sit1.yml`;
  - hyphenated and extension-less names work.

### T3: `crates/ports`

- Every trait in `docs/interfaces.md`, plus the request and response types each needs.
- An in-memory fake for each port, behind the `fakes` feature, with realistic behaviour:
  - the gateway fake supports timeouts;
  - the blob store fake is content-addressed;
  - the event bus fake has bounded subscribers and emits `Resync` when one lags;
  - the leases fake expires leases.

**Verify:** contract tests that every implementation will later run too. Expose them as `ports::conformance::<port>(impl)`, run them against the fakes now, and have every real implementation run them later.

### T4: `proto/agent.proto` and `crates/proto`

Generate code with tonic and prost, with zstd compression enabled.

**RPCs:**

- `Join(JoinRequest{swimlane_id, csr_der, oneof credential {google_id_token, join_token}}) returns (JoinResponse{cert_chain_der, not_after})`.
- `Connect(stream AgentMessage) returns (stream HubMessage)`.

**`AgentMessage`** is a oneof of:

- `Hello` (agent version, swimlane, cluster, project, NFS server, export, mount root);
- `Heartbeat` (scan sequence, Merkle root, file count);
- `ScanDelta` (base root, new root, changed entries {path, hash, size, mtime, bytes}, removed paths, skipped {path, reason}), batched to at most 3 MiB per message;
- `OpResult` (request id, ok, error code, current hash);
- `ClusterReport` (deployments, pods, env hints, release hints; full or delta);
- `CertRenewalRequest`.

**`HubMessage`** is a oneof of:

- `AgentConfig`;
- `RequestDelta { since_root }`;
- `RequestFullScan`;
- `ReadFile`;
- `WriteFile { request_id, path, expected_hash, bytes }`;
- `DeleteFile`;
- `RestartDeployment`;
- `RequestClusterReport`;
- `CertRenewalResponse`.

**Additions (D72–D75):**

- `ScanDelta` carries:
  - a `seq` number, which the hub acknowledges with `HubMessage::Ack{seq}`;
  - an `observed_at` timestamp per entry;
  - an optional `during_job` field (the Job's name and uid);
  - a `denied` flag per entry. Denied entries have no bytes.
- `ClusterReport` carries `SyncWindow` start and end events.
- A separate `Sentinel` service:
  - `Join`, reused with the VM's ID token;
  - `Report(stream SentinelMessage) returns (stream SentinelAck)`;
  - `SentinelMessage` is a oneof of `Hello` (VM, export root, version), `AuditRecordBatch` (seq, plus records each holding time, path, operation, success, login user, effective user, exe, comm) and `Heartbeat`.

**Config-server additions (D85, D86, D88):**

- `HubMessage::NotifyConfigServer { request_id, paths[] }`: the agent POSTs the paths to `/update-resources`.
- `HubMessage::FetchServed { request_id, application, tenant, channel?, file }`: the agent GETs the config-server's response for that request.
- `ClusterReport` additionally carries:
  - for each Deployment, the values of allowlisted environment variables, plus the names (never the values) of every other environment variable;
  - the config-server pod's start time.

**Verify:** `buf lint`, and the generated code compiles with no warnings.

### T5: `api/openapi.yaml` (OpenAPI 3.1)

**Conventions:**

- Errors use `application/problem+json`.
- Cookie session auth, with `X-CSRF-Token` required on every non-GET.
- Every list endpoint uses keyset pagination (`cursor`, `limit` ≤ 500).
- Content endpoints return an `ETag` and honour `If-None-Match`.
- Every write accepts an `Idempotency-Key` header.

**Endpoints.** Start from the previous bundle's list, and add these:

- `GET /api/v1/events`: SSE, with `swimlane` and `kinds` filters.
- `GET /api/v1/blobs/{hash}`: immutable, with `Cache-Control: public, max-age=31536000, immutable` restricted to authenticated users through `private`.
- `GET /api/v1/compare/tree?left=&right=&prefix=`.
- `GET /api/v1/search/text`.
- `POST /hooks/github`: no session; verified by HMAC.
- `GET /api/v1/docs/fs/read` and `GET /api/v1/docs/fs/grep`.
- Admin endpoints for path mappings, merge modes, host tiers, folder mappings, notifications, users, access requests and agents.

Don't include an agent join-token endpoint. Join tokens are issued by an admin endpoint and used only as a fallback (S5).

**Additions:**

- **Attribution:** in history, audit and file entries.
- **Severity:** on findings and changes, plus admin endpoints for severity rules.
- **PR links and PR jobs:**
  - `GET /api/v1/prs`;
  - `GET /api/v1/pr-jobs/{id}`;
  - `POST /hooks/github`, which now also handles `pull_request` events.
- **Sentinels:**
  - `GET /api/v1/admin/sentinels`;
  - admin endpoints for the OS Login user mapping.
- **Retention:** settings, admin only.
- **Reveal:** `POST /api/v1/swimlanes/{id}/files/reveal`, which reveals a masked value. It's audited, and requires Editor or above.
- **Denied files:** "content withheld" responses for denied files.

**Config-server additions:**

- `POST /api/v1/swimlanes/{id}/notify`: notify services of changed paths. Operator.
- `GET /api/v1/swimlanes/{id}/served?application=&tenant=&channel=&file=`: the "as served" view. Viewer.
- `GET` and `PUT /api/v1/admin/swimlanes/{id}/auto-notify`: the per-swimlane auto-notify setting. Admin.
- Pickup states on service and file entries.
- A tenant set per swimlane.

**Verify:** `redocly lint` passes, and every operation documents its required role in an `x-required-role` extension.

### T6: `db/migrations/0001_init.sql`

Start from all the tables in the previous bundle, and add:

- **Indexes and state:**
  - `settings_index`, with a pg_trgm GIN index on `setting_path`;
  - `git_tree_index`, keyed by (repo, commit, path);
  - `swimlane_merkle`;
  - `effective_tree_hashes`.
- **Replica coordination:**
  - `agent_connections` (swimlane, replica id, replica address, epoch);
  - `leases` (name, holder, expires_at).
- **Audit:**
  - `audit_events`, with `seq bigserial`, `prev_hash` and `hash`;
  - `audit_checkpoints` (seq, head hash, KMS signature, GCS object).
- **Request handling:**
  - `idempotency_keys` (user, key, request hash, response, expires_at);
  - `webhook_deliveries` (delivery id primary key, received_at), for replay protection.

Storage and access rules:

- `blobs.content` uses lz4 TOAST compression.
- There are two database roles:
  - `lanekeeper_migrator`, which owns the DDL;
  - `lanekeeper_app`, which has no DDL rights and only INSERT and SELECT on `audit_events`.
- A trigger blocks UPDATE and DELETE on `audit_events`.
- `doc_chunks.embedding` is `vector(384)`, with an HNSW index.

**Additions:**

- **Attribution:** columns on `file_observations` and `audit_events` (source, confidence, actor, evidence JSON).
- **Agent spool:** `agent_acks` (swimlane, last acknowledged seq).
- **Sentinels:**
  - `sentinels`;
  - `sentinel_records`, partitioned by month and pruned after 90 days;
  - `os_login_user_map`.
- **Sync windows:** `sync_windows`.
- **Severity:** `severity_rules`.
- **PRs:** `pr_links`, with a unique index on (swimlane, path, nfs_hash) where state = open, and `pr_jobs`.
- **Retention:** `blob_refs`, a reference-count view or table, and `retention_settings`.
- **Outbox:** `outbox` (id, event JSON, created_at), with a commit-time trigger that issues `NOTIFY`.

**Verify:** the migration applies on Postgres 16, and tests prove that `lanekeeper_app` can't UPDATE, DELETE or run DDL.

### T7: `docs/mcp-tools.md`

Specify every tool: name, input and output JSON schema, minimum role, whether it needs confirmation, output cap, and pagination.

- **Read tools:**
  - `list_swimlanes`, `get_swimlane`, `list_files`;
  - `read_file` (with `lines` or `setting_path`);
  - `get_effective_config`, `compare`, `compare_tree`, `compare_grid`;
  - `search_settings`, `search_text`, `find_inconsistencies`, `get_drift`, `get_pending_restarts`, `get_history`;
  - `search_docs`, `read_doc`, `grep_docs`, `get_docs_for_file`, `get_feature_status`.
- **Write tools:** `propose_change`, `apply_change`, `upload_file`, `delete_file`, `raise_pr`, `restart_service`.

### T8: Budgets and fixtures

- **`perf/budgets.toml`:** one entry per P#, each with the benchmark or load test that measures it, a threshold, a unit and the scale it's measured at.
- **`cargo xtask bench-check`:** reads criterion output and the load-test JSON, and fails when any value is over budget.
- **`cargo xtask gen-fixtures --scale small|target`:** deterministic, seeded synthetic data:
  - a base repo and tenant branches as real Git repos;
  - NFS snapshots per swimlane, using the `-<branch>` naming;
  - realistic shapes: channel subfolders, nested service folders, CRLF line endings, anchors, duplicate keys, XSL, images, extension-less files, and 6,000-line YAML;
  - tenant forks with missing entries;
  - redundant copies.

  At target scale it produces the full design scale.

**Verify:** two runs with the same seed produce byte-identical output, and `bench-check` fails a planted over-budget value.

### T9: Fuzz skeleton

- `fuzz/` with targets for `NfsPath`, `CompareRef` and `PathMappingRule` reverse mapping.
- Seed corpora.
- `just fuzz-smoke` runs each target for 30 seconds.

**Verify:** `just fuzz-smoke` runs clean.

## Acceptance (all required)

- [ ] `just verify-01` and `just verify` pass.
- [ ] Every trait in `docs/interfaces.md` exists, with a fake and conformance tests.
- [ ] The proto, OpenAPI spec and migrations pass their linters and tests. The database-role permission tests pass.
- [ ] Fixture generation is deterministic, and target scale matches `docs/performance.md`.
- [ ] `bench-check` fails a planted over-budget value.

## Stop and escalate if

- A type or signature in `docs/interfaces.md` is ambiguous, or can't be expressed object-safely.
- A Postgres feature you need (pgvector or pg_trgm) isn't available in the CloudNativePG image you plan to use.

## Out of scope

Any behaviour behind the ports.
