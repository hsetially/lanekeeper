# Performance budgets

Budgets are p95 unless stated otherwise. Each one is enforced by a benchmark or load test registered in `perf/budgets.toml`, and checked by `cargo xtask bench-check` or the load-test gate in prompt 16. If a budget can't be met, the agent stops and escalates with measurements.

## Design scale

These are the targets the system is built and tested for. Confirm them in Q27.

- 40 swimlanes, each with 2,000 files on NFS. That's about 80,000 files, many of them identical.
- 60 tenant branches.
- Files up to 2 MiB. The largest real file is over 6,000 lines.
- 100 concurrent web users and 30 concurrent MCP sessions.
- 3 hub replicas, each with 2 vCPU and 4 GiB of memory.

## Budgets

**Detection**

- **P1** An out-of-band change on NFS appears in the UI within 15 seconds.
- **P2** A push to Git shows up as updated drift within 10 seconds through the webhook, or within 75 seconds through polling.

**Agent**

- **P3** Idle traffic from each agent stays under 1 KB per minute: one heartbeat carrying a Merkle root.
- **P4** Each agent uses at most 50 mCPU and 64 MiB of memory at steady state. A full rehash of 2,000 files takes under 20 seconds.

**Recompute**

- **P5** Incremental recompute after a change to N files finishes within 50 ms + 2 ms × N × (number of affected swimlanes).
- **P6** The nightly full recompute at design scale finishes within 3 minutes.

**Reads**

- **P7** API reads, measured on the server:

| Request | Budget |
|---|---|
| Swimlane list | 50 ms |
| File tree | 100 ms |
| File content | 30 ms cached, 80 ms cold |
| Compare two versions of a 6,000-line file | 100 ms |
| Grid for one logical file across 40 swimlanes | 150 ms |
| Whole-swimlane compare | 300 ms |
| Settings search across all swimlanes | 200 ms |
| Text search across all swimlanes | 400 ms |
| Docs search | 150 ms |
| Feature status | 200 ms |

**Writes**

- **P8** An edit is applied on NFS and acknowledged within 1.5 seconds, not counting any wait for approval.

**MCP**

- **P9** Read tools respond within 300 ms on the server. Default output is at most 16 KB.

**Web**

- **P10** Web performance:
  - The initial JavaScript bundle is under 250 KB gzipped, with Monaco loaded lazily.
  - Route changes complete within 100 ms when the data is cached.
  - The diff view of a 6,000-line file is interactive within 500 ms.
  - A 5,000-row grid scrolls at 60 frames per second.
  - Lighthouse performance score is at least 90.

**Hub and database**

- **P11** Hub memory stays under 1.5 GiB per replica at design scale. Startup takes under 10 seconds.
- **P12** No steady-state query takes longer than 50 ms. List endpoints use keyset pagination. Hot queries have an `EXPLAIN` checked into their tests.

**Attribution and spool**

- **P14** A sentinel-based attribution is attached within 30 seconds of the change being observed.
- **P15** After a 1-hour hub outage, the spool replays at 1,000 or more versions per second, and no acknowledged version is lost.

**Live updates**

- **P13** A live update reaches SSE subscribers within 1 second.

## Mechanisms

Prompts implement these. They are the reasons the budgets are achievable.

1. **Content addressing.** Blobs are stored once, keyed by SHA-256. Blob URLs are served with `Cache-Control: immutable`. ETags are used on everything else.
2. **Agent-side Merkle trees.**
   - Heartbeats carry only the root hash.
   - The hub asks for a delta only when the root changes.
   - Stat-walks every 10 seconds, with a full rehash every 15 minutes.
3. **Parse once per blob.** `settings_index` rows (setting path, value and line range) are written once per unique structured blob. Settings search, grid and feature status become indexed SQL using pg_trgm.
4. **Git tree index per commit.** Each commit's paths and blob hashes are stored in Postgres. Head moves are handled by diffing trees, so recompute only touches the paths that changed.
5. **Effective tree hashes.** A hash per directory per swimlane makes whole-swimlane compare skip identical subtrees.
6. **Event-driven incremental pipeline.** Each domain event recomputes only the affected (swimlane, path) pairs. Full recomputes happen only in the nightly run.
7. **Caching.** moka caches, weighted by size, for parsed documents, effective configs and diffs, keyed by content hashes so they never go stale.
8. **Compression.** Agent streams use zstd. Blob storage uses lz4 TOAST compression.
9. **Parallel CPU work.** rayon, over deduplicated blobs, for text search, findings and adoption ranking.
10. **Server-side diffs.** The server computes diff hunks and the web view renders them virtualised.
11. **Live updates** through SSE, instead of polling.
12. **Multiple hub replicas.** 3 replicas. Agent commands are routed to the replica that holds the agent's connection, over internal gRPC with mutual TLS. Singleton jobs use Postgres leases.

## Measurement

- **Benchmarks:** criterion benchmarks for the engine and the hot paths in the hub.
- **Fixtures:** `cargo xtask gen-fixtures --scale target` generates synthetic data at design scale, matching the real layout: channel subfolders, CRLF line endings, anchors, duplicate keys, images and 6,000-line files.
- **Load tests:** in prompt 16, Rust load tests run against staging at design scale.
- **Web:** Lighthouse CI and a bundle-size check.
- **Tracing:** traces go to Cloud Trace. Every request logs its handler latency, which feeds per-route RED metrics (rate, errors and duration).
