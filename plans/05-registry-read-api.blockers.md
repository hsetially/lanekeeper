# 05 blockers

## T1: `P15.registry_ingest_versions_per_s` (1,000 versions per second, minimum of three runs) is not met: 724

Everything else in T1 is done and green (tests, lint, the fuzz skeleton and its seed replay, the gate recipes). The one thing that
is not is the registry's half of P15, so it is not registered (`registered = false` in `perf/budgets.toml`) and `bench-check`
lists it as UNMET with the number. Nothing was loosened and the benchmark's workload was not changed to flatter the number. A
human decides which way to go (options at the end); the entry is then registered by flipping one flag in `perf/budgets.toml` and
adding the id to the pinned list in `crates/xtask/tests/budgets_registry.rs` (a path the plan's approval already covers).

### The budget

`docs/performance.md` P15: after a one-hour hub outage the spool replays at 1,000 or more versions per second, and no
acknowledged version is lost. `perf/budgets.toml` (contract PR 05): `P15.registry_ingest_versions_per_s`, bench
`hub-registry/ingest_versions_per_s`, statistic `min`, threshold 1,000, unit `versions_per_s`, direction `lower`. The agent's half
(`agent/spool_replay_versions_per_s`) and the platform's acknowledgement (`hub-platform/ack_throughput`) are registered; this is
what the sink behind that acknowledgement does.

### The benchmark

`crates/hub-registry/benches/ingest_versions_per_s.rs`, run as `cargo bench -p hub-registry` (the `registry-bench` recipe). It
inventories one swimlane of the target-scale fixtures through the real ingestion path (2,003 files), then applies the agent's
one-hour spool the way the other two P15 benchmarks size it: 360 messages of 50 versions each (18,000 versions), over a real
Postgres 16 as `lanekeeper_app`, with the registry's real transactions, a blob store, an outbox bus and an audit log over the real
tables. Every version is a new version of a different YAML file, so every one is an out-of-band change: it writes an observation,
a `file_state` row, one audit event (in a transaction that shares the audit chain's lock) and one outbox event. After the run it
asserts that 18,000 observations and 18,000 audit events exist (none lost). Three runs on fresh databases; the budget takes the
slowest. This is the heaviest spool the design allows: no version repeats a path inside a message, so the "one audit event per
path per delta" rule (D93) saves nothing.

### Measurement

Machine: 4 vCPU, 16 GB, Postgres 16.15 on the same machine over loopback (fsync on, `shared_buffers` 128 MB). Raw round trip
(`pgbench`, `select 1`): 0.09 to 0.10 ms. A single autocommit insert: 0.41 ms; 16 clients: 14,800 per second.

```
final code, three runs:   724   843   805   versions/s   (min 724, p50 805, mean 791)
draft code, three runs:   778   784   728   versions/s
```

`cargo xtask bench-check` on the registry's budgets: `UNMET P15.registry_ingest_versions_per_s 724.102 min >= 1000
versions_per_s  not registered; measured outside budget`. Six runs in two sessions ranged from 724 to 843; the best single run is
below 1,000.

### Where the time goes

A temporary probe (removed again) timed every registry statement and every port call over 120 messages of the same workload
(66.9 ms per message, 748 versions per second in that run). Per message of 50 versions:

| Part | Per message | Share |
|---|---|---|
| `AuditLog::record`, 50 calls at 0.58 ms (lock, chain head, insert: three round trips each) | 29 ms | 43 % |
| `EventBus::publish_in_tx`, 50 calls at 0.155 ms | 8 ms | 12 % |
| `BlobStore::put`, 50 calls at 2.06 ms, 16 at a time | 8 ms | 12 % |
| the registry's own statements (see below) | about 20 ms | 30 % |
| validation (SHA-256, classification, drift hash) | 0.9 ms | 1 % |

The registry's own statements: `upsert_state` 3.2 ms, `insert_observations` 2.8, `tool_writes` 2.4, `stored_state` 1.9,
`merkle_root` 1.7 (a `count(*)` over `file_state`), two commits 2.1, six `begin` with their `set_config` 1.3, ten statements of
0.2 to 0.6 ms each about 3.5.

Run again with the audit write switched off (the event is still built): 38.0 ms per message, **1,316 versions per second**. So the
audit chain alone costs the 29 ms that separate 748 from 1,316. 1,000 versions per second means at most 50 ms per message; the
audit and the outbox events take 37 ms of them before the registry does anything else.

### What was tried

- Measured the draft as it stood, three runs (above), then the final code, three runs. The refactoring done in this commit
  (splitting `apply_chunk` for clippy, a timeout on blob puts) does not change the number.
- Instrumented the probe described above; ran it with and without the audit write; measured the database's round trip and commit
  cost with `pgbench`.
- Did not change the workload (distinct paths, 50 per message) or the threshold.
- Registry-side trimming only, **estimated, not implemented**: merge the last small transaction into the last chunk's, replace
  the `count(*)` of the root with a running count, skip the throwaway transaction that reads the path-mapping rules, and overlap
  what can be overlapped saves at most about 8 ms of the 20. That is about 925 versions per second at best, so it cannot make the
  minimum of three runs reach 1,000, and each of the four changes touches the core of the apply path. They are worth doing next to
  option 1, not instead of it.
- The plan's own fallback for this risk (risk 2: one audit event per path per delta, intermediate versions in history only) is
  already how the registry works (D93), and does not help this workload, whose 50 versions per message are on 50 paths.

### Options (a human picks; none is taken here)

1. **Batch the audit and outbox writes in the ports (contract change; recommended).** Add `AuditLog::record_many(&mut Tx, Vec<AuditEvent>)
   -> Result<Vec<AuditId>, AuditError>` (one lock, one read of the chain head, the hashes chained in Rust, one multi-row insert) and
   `EventBus::publish_many_in_tx(&mut Tx, Vec<DomainEvent>)` (one insert). Every audited path still gets its own audit row and its own
   link in the chain, so rule 8 and D38 are untouched. The registry would call each once per chunk of 50 paths. Needs a
   `contract-change` PR for `crates/ports` (traits, fakes, conformance) and changes in 03a's `PgAuditLog` and 03b's bus. Effect:
   the 37 ms of audit and outbox per message become a few milliseconds. The measured bound is the audit-off run, 1,316 versions per
   second with the outbox still one insert per event; with both batched and the trimming of the next paragraph, about 2,000 or
   more (an estimate).
2. **Change what an audit event covers during a replay (decision on D38, A12 and D93).** One `FileChangeDetected` per chunk with
   the path list in its `diff`, or one per path only below some number of paths. The effect is the audit-off run, 1,316 versions per
   second. It weakens the per-path audit trail that the history views and the S9 verification read, so it is not recommended.
3. **Re-define the workload or the number of `P15.registry_ingest_versions_per_s`.** The benchmark is the heaviest case: one swimlane,
   every version on a different path, every one audited. If a replay is mostly repeated versions of a few files, one audit event
   covers all of a path's versions in a message and the rate is far higher. A threshold of 700 would pass today; that is a
   loosening of a contract number and is not mine to make. A workload with five paths per message is a smaller change but also a
   decision about what P15 means for the registry.
4. **Trim the registry's own statements** (the four changes above, plus a partial index on `file_observations (swimlane_id, path)
   where `source = 'tool_write'` for the 2.4 ms `tool_writes` read, which is a migration and so a contract change). Useful with
   option 1; alone it ends near 925 versions per second.

Recommendation: option 1, with the trimming of option 4 that needs no migration done in the same task. The 1,000 versions per second
of P15 was set for the acknowledgement of a replay, and the registry meets it only if the cost of an audit event stops being three
round trips.

### Other things found while doing T1 (for the record, all fixed in the T1 commit)

- The draft's `registry-bench` recipe cut the registry with an `awk` program that only found a `[[budget]]` at the start of a
  paragraph; the contract PR put a comment line before `P15.registry_ingest_versions_per_s`, so the recipe would have stopped with
  "found 0 of 1 budgets". The program now finds the block anywhere in its paragraph, and `tests/justfile_gate.rs` runs it against
  `perf/budgets.toml`.
- The draft did not pass `cargo fmt` and had clippy findings (11 in the library alone: long functions, complex types, a struct with
  four bools). `apply_chunk`
  and `apply_services` are split, `Candidate` has an `Origin` instead of two flags, and the crate is clean under
  `-D warnings` for the library, the tests and the benchmark.
- A refused `hello` (no room for another stream) had already written the agent's rows; admission now comes first and a failed
  `hello` gives its place back. Blob puts had no timeout. An increment that reached an empty swimlane ended its inventory, so the
  full scan after it raised an audit event for every file. Each has a test that fails without the fix.
- Unused dependencies (`sha2`, `insta`, `criterion`, `tracing-subscriber`) were removed from `crates/hub-registry/Cargo.toml`; later
  tasks add them when they use them. The registry's fuzz lockfile is refreshed to match and gets the `.gitignore` and
  `.gitattributes` that the other fuzz crates have.
