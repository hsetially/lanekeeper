# 05: Registry, indexes, incremental pipeline and read API

| | |
|---|---|
| **Wave** | 2. Can start against fakes as soon as 01 merges. |
| **Depends on** | 01. Integrates with 03a, 03b and 04. |
| **You own** | `crates/hub-registry/**` |
| **Interfaces** | Implements `ReportSink`, `SentinelSink` and `RegistryRead`, plus `record_sync`. Consumes `BlobStore`, `GitReader`, `EventBus`, `Leases`, `AuditLog`, `Users`, `AgentGateway` and the engine. |
| **Security** | S4, S10, S11, S13, S21 |
| **Performance** | P1, P2, P5, P6, P7, P12, P13, P14, P15 |
| **Gate** | `just verify-05` |

## Objective

Turn raw agent deltas and Git movements into indexed, incrementally maintained facts, and serve every read within its P7 budget. This crate holds:

- the registry;
- the baselines and the adoption pass;
- drift and findings;
- every index;
- the read API used by both REST and MCP.

## Tasks

### T1: Ingestion (`ReportSink`; P1)

**`hello`:** the agent must match a swimlane an admin created. Return `AgentConfig`.

**`heartbeat`:** compare the root with `swimlane_merkle`. If it differs, return `RequestDelta(since_root)`.

**`delta`:**

1. Store each blob with `put`, which is idempotent.
2. Insert observations.
3. Classify each file with the engine.
4. Update `nfs_files` and `swimlane_merkle`.
5. Publish `FileObserved`.

A hash explained by neither a registered tool write nor a sync is an out-of-band change. Write an audit event with via=agent_detected and author unknown.

**`cluster`:** update services, pods and release hints, and publish `PendingRestartChanged` when the state changes.

**Verify:** a fake agent changes one file, and `DriftChanged` arrives within 15 seconds end to end (P1).

### T2: Indexes (D65)

- **Settings index.** On every new structured blob, write `index_settings` rows exactly once, keyed by blob hash.
- **Git tree index.** On `GitHeadMoved`, the leaseholder writes the tree index for the new commit and diffs it against the old one.
- **Effective tree hashes.** Maintained incrementally for every directory whose effective files changed.
- **Index work** runs on rayon inside `spawn_blocking`, and is idempotent if replayed.

**Verify:** replaying the same deltas twice gives identical tables. A test asserts the settings-index row count.

### T3: Incremental pipeline (P5, P6)

**Consumers.** Subscribe to `FileObserved`, `GitHeadMoved` and changes to baselines or intentional marks.

**For each event:**

1. Compute the affected (swimlane, path) pairs from the tree diff or the delta.
2. Recompute drift, effective config and findings only for those pairs.
3. Publish `DriftChanged` and `FindingsChanged`.

**Coalescing.** Bursts are coalesced per swimlane in 200 ms windows.

**Nightly.** A leased job runs a full recompute with the engine's batch functions and repairs any divergence.

**Verify:**

- a property test: after random event sequences, the incremental state equals a full recompute;
- benchmarks for P5 (incremental) and P6 (full, target scale).

### T4: Branches, path mapping and sync

**Branches.** Classify tenant branches as deployable (`^sit\d+$` plus admin additions), template (`^templates/`) or docs (`templates/common-docs`).

**`record_sync(swimlane, tenant_branch, tenant_commit, base_commit, actor, source)`:**

1. Request a full scan.
2. Check C7 first.
3. Set baselines for files that match Git at the reported commits.
4. Update the deployment record.

**Fallback.** When the tenant suffix changes on NFS, re-run branch matching and raise an admin confirmation.

**Verify:** the sequence test from the previous bundle: adoption, then an out-of-band edit, then a sync that overwrites it, which produces C7 and a notifier call.

### T5: Adoption pass

1. **Rank deployable branches** by the share of logical files that match. The ranking runs on rayon over tree indexes, without re-reading Git.
2. **Show hints:** env hints and release hints.
3. **Find the best-matching base commit.**
4. **Scan for secrets** in both repos (Q20).
5. **Collect per-file resolutions,** with bulk actions.
6. **Confirm,** which sets baselines and enables editing.

**Verify:** on target-scale fixtures, the correct branch ranks first, and ranking takes 5 seconds or less per swimlane.

### T6: Read API (`RegistryRead`, REST handlers; P7, P12)

Implement every method in `docs/interfaces.md`.

**General rules:**

- All lists use keyset pagination.
- ETags use content hashes, and requests with `If-None-Match` get 304.
- moka caches, keyed by content hashes, hold effective configs, diffs and grids, so entries never go stale.
- Every handler is guarded (S4).

**Specific operations:**

- **`compare_tree`:** walk both effective trees and skip subtrees whose effective tree hashes are equal (D67).
- **`search_settings`:** SQL on `settings_index`, using pg_trgm, joined with the current effective hashes.
- **`search_text`:**
  - collect the deduplicated set of current blob hashes for the chosen swimlanes;
  - run the engine's `search_text` on rayon;
  - caps of 1,000 matches total and 50 per file, with a 5-second timeout and truncation reported.
- **Locations:** findings, grid rows and search hits carry line ranges.

**Verify:**

- a criterion benchmark or load script for each P7 line at target scale;
- `EXPLAIN` snapshots checked into tests for every hot query, with no sequential scans on large tables (P12).

### T7: Attribution (D72, D73, P14)

- Store sentinel records through `SentinelSink`, deduplicated by sequence number.
- Correlate attribution in the order given in `docs/domain-model.md` → Attribution. Attributions are upgraded as records arrive, never downgraded, and every upgrade is audited.
- Map OS Login users to app users automatically by email, with admin overrides (Q32).
- Publish `AttributionUpdated`.

**Verify:**

- end-to-end with fakes: a local VM edit produces `sentinel_login`, high confidence, naming the user, within 30 seconds (P14);
- a sync-window change produces `sync_job`;
- an NFS write outside any Job produces `nfs_client`;
- a late sentinel record upgrades an earlier `unknown`.

### T8: Sync windows (D75)

- While a sync window is open for a swimlane, record drift but hold alerts.
- When the window closes, run `record_sync` processing, including C7, and evaluate alerts once.
- A window left open for more than 2 hours raises a warning.

**Verify:** a 600-file sync produces one alert evaluation, and C7 still fires for an overwritten `nfs_ahead` file.

### T9: Severity, retention and denied files

- **Severity:** attach it to every change and finding. The notifier's priority and the list ordering use it.
- **Retention GC (D78):** a leased nightly job, run in batches.
  - It never deletes a blob referenced by a baseline, audit event, proposal, draft or PR link, or within the per-file keep window.
  - It produces metrics and a dry-run report.
- **Denied files:** they appear in trees with class `denied`. Content endpoints return "content withheld".
- Every domain event goes out through `publish_in_tx` (D80).

**Verify:**

- a GC property test: referenced blobs are never deleted;
- the denied-file read API returns no content;
- the outbox is used for every event.

### T10: Tenants, pickup and "as served" (D84–D86)

**Tenants.** Each swimlane has a set of tenants, from the adoption pass, release hints and suffixes seen on NFS (Q34). Checks C3 and C4 evaluate against that whole set.

**Pickup state.** Computed per change and consuming service from:

- the file role;
- each Deployment's client settings (`CONFIG_CLIENT_CACHE_TTL`, `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED`);
- the config-server's start time.

`PendingRestartChanged` becomes `PickupChanged`, which includes "live by HH:MM" for cache-lifetime cases.

**"As served":** `RegistryRead::served` calls `AgentGateway` with `FetchServed`. Results are cached by (request, current effective hashes).

**Verification job:** each night, a leased job samples 200 (application, tenant, channel, file) combinations per swimlane. It compares the engine's rendering with the "as served" response. A mismatch is a high-severity finding that names the file.

**Verify:**

- a fake config-server drives a mismatch, and the finding fires;
- a pickup-state test for each of the four states.

## Acceptance (all required)

- [ ] `just verify-05` passes, and every P1, P5, P6, P7 and P12 item owned here is within budget.
- [ ] The incremental-equals-full property test passes.
- [ ] Every endpoint matches the OpenAPI spec (contract test) and passes the route-guard test.
- [ ] The pending-user 403 and Viewer requirements are tested.

## Stop and escalate if

- A P7 budget fails at target scale despite caching and indexes. Include profiles.
- Merkle deltas and observation semantics disagree in a way that would need a proto change.

## Out of scope

Writes (06), docs (14) and MCP formatting (07).
