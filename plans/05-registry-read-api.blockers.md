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

## Resolved: T1, `P15.registry_ingest_versions_per_s` is met (FIX after the batched audit and outbox contract)

A human chose option 1. Its contract (`AuditLog::record_many`, `EventBus::publish_many_in_tx`, the 200-event and 1 MiB limits,
`canonical_size` and `split_batches`, real one-statement overrides in `PgAuditLog` and `PgEventBus`) was merged into this branch
(5a9065c), and the registry now uses it. Nothing was loosened: the threshold is 1,000, the workload is the one above (18,000 versions in
360 messages of 50, distinct paths, every one audited), and the entry is registered (`df82516`).

### What changed (all in `crates/hub-registry`, plus the flag, its pin and a Justfile comment)

| Commit | What |
|---|---|
| `e2bec86` | The crate did not compile against the merged ports (`BusError::BatchTooLarge` in an exhaustive match). A port refusing a batch for size is a bug of the registry, which cuts its batches itself: `RegistryError::Internal`, `SinkError::Unavailable`, logged by stage (`audit_batch`, `event_batch`) with nothing of the batch. |
| `b4abbbb` | The test stand-ins `PgTestAudit` and `PgTestBus` were implementations of their own, so they could never take the real batch path (a stand-in that forwards only `record` and `publish_in_tx` runs the ports' default loops, silently). They are now decorators over the real `PgAuditLog` (hub-identity, a dev-dependency; Cargo.toml and Cargo.lock) and `PgEventBus` that forward all four calls and keep the test hooks; `per_event()` is the old one-at-a-time behaviour on purpose, the reference of the equivalence test. The P15 benchmark hands the registry the real `PgAuditLog` and `PgEventBus` with no decorator. `IngestDeps` and `PipelineDeps` take `Arc<dyn AuditLog>` and `Arc<dyn EventBus>` and store them unwrapped (there is no `Registry::build` yet). |
| `d5b3230` | `crate::batch::{record_all, publish_all}`; the apply path builds the events of a chunk and writes them with one `record_many` (cut by `ports::split_batches` on the blocking pool, so both limits hold) and one `publish_many_in_tx`; one event is the single call. The outbox is written before the audit events, so the chain lock is held for the audit insert and the commit only. |
| `9b45161` | The last chunk's transaction also moves the root, the acknowledgement and the staged parts (one transaction and one flush of the write-ahead log for a message, where it was two and a third that only read the rules); the rules are read in the first chunk's transaction instead of a throwaway one. |
| `449166b`, `2674c7b` | The other writers of several events in one transaction use the same calls (below). |
| `0509cd7` | Not part of P15: the EXPLAIN snapshot test (T2) failed on about one run in three because of when autovacuum ran (below). |
| `df82516` | `registered = true`, the pin in `crates/xtask/tests/budgets_registry.rs`. |

### Same data as before (rule 8, D92/D93)

`tests/batch_writes.rs`, over a real Postgres: the same script (inventory, an increment with a path changed three times and a removal, a
sync job, a tool write, a replay of a delta, a full scan with two files gone and one changed; in chunks of 3, so batches of 3, 3, 2, 2 and
single calls for the chunks of one) runs through the batch ports and through the per-event reference, and the two are equal in every column
of `audit_events` but `recorded_at` and in `outbox` but `created_at` and `published_at` (a test fails if a column is added and neither
compared nor known to be time-derived); both pass the nightly chain verifier (S9: 13 events checked, clean); and the rows equal a snapshot
(`tests/snapshots/batch_writes__the_rows_of_the_script.snap`) recorded by the code of `e2bec86`, which called `record` and `publish_in_tx`
once for each path, hashes of the chain included. Reversing the audit order in the registry makes it fail (checked). One audit event per
path per delta, one outbox event per `FileObserved`.

No silent fallback: the statements of a chunk of 50 paths are counted from sqlx's own statement log (`tests/support/statements.rs`): one
`INSERT INTO audit_events` of 50 rows, one `INSERT INTO outbox` of 50, one read of the chain head, one chain lock, `record_many` called once
and `record` never; on the per-event reference the same count reads 50 inserts of one row, so a decorator that forwarded only the single
calls fails it (this was seen on the code before `d5b3230`: 50 inserts of 1 row). A chunk of 450 paths is three inserts of 200, 200, 50 and
one valid chain.

### Measurements

Machine: 4 vCPU, 16 GB, Postgres 16.15 over loopback, fsync on, `shared_buffers` 128 MB. Before every series `uptime` and
`pgrep -af 'cargo|rustc|criterion'` were checked and the series waited until the 1-minute load average was under 0.3 to 0.5 and no
other build or test ran: the series started at a load average of 0.45 (the 587 series), 0.21 (batch calls only), 0.29 (its two
repeats), 0.23 (with the last-chunk change), 0.29 (the official three) and 0.29 (the recipe). Every number is versions per second of
one run on a fresh database; the benchmark takes the minimum of its three runs.

```
before, as in the blocked section (a test stand-in for the log, one event at a time):      724  843  805
before, with the real PgAuditLog and PgEventBus, one event at a time (same code as above): 590  588  587    (min 587)

batch calls only (d5b3230), three invocations:                   939 1196 1149 | 1079 1184 1202 | 1143 1163 1082   (min 939)
batch calls and the last chunk carrying the root (9b45161):     1190 1150 1286 | 1110 1103 1171 | 1174 1297 1368   (min 1103)

final code, three invocations, the official ones:               1232 1342 1274 | 1327 1197 1319 | 1284 1297 1247   (min 1197)
final code, inside the registry-bench recipe:                   1313 1182 1134                                   (min 1134)
```

The batch calls alone met the budget in two of three invocations and missed it in one (939; the first run of an invocation was the
slowest in four of the ten invocations, by up to 21 %), so two of the trims the blockers file had estimated were done as well, for a
margin: with them the lowest of the 21 runs is 1103, and of the 12 runs on the final code 1134. Not done, and not needed: replacing the root's `count(*)` by a running count (a count that is wrong
after a crash between two chunks stays wrong; the statement is 1.1 ms), one statement with data-modifying CTEs for the end of a delta
(about 1.8 ms), and a partial index for the tool-write read (a migration). The join-on-`unnest` form of the two read statements was tried
and is no better (the planner without statistics picks the same bitmap scan).

Where the time goes, per message of 50 versions (release build, sqlx's statement log, 30 messages after 10 of warm-up; the elapsed time
of each statement, the blobs overlap 16 at a time):

```
                                       blocked section     batch calls only (44.0 ms, 79 statements)
audit events                           29 ms (50 x 0.58)    4.4 ms: lock 0.40, head 0.67, insert of 50 rows 3.28
outbox events                           8 ms (50 x 0.155)   1.2 ms: one insert of 50 rows
blob puts (16 at a time)                8 ms                about 8 ms (50 inserts, 1.7 ms each)
the registry's own statements         about 20 ms           about 20 ms; two commits 2.5 ms
```

and after the last-chunk change one `COMMIT`, one `set_config`, one lock and one read of the rules for the message (75 statements; the
transactions are counted by `tests/batch_writes.rs`). Audit and outbox events went from 37 ms of a 67 ms message to about 5.6 ms.

The other benchmarks, rerun on the final code (`cargo bench -p hub-registry`, then twice more for P1, P2 and P6; their budgets are
p95 of the harness's samples):

```
P5   incremental_recompute_base                 32.4 ms of 50      (T3 reported 36.4)
P5   per_file_per_swimlane                      0.48 ms of 2       (0.48)
P1   hub_delta_to_drift                         273, 275, 259 ms of 2,000   (251)
P2   hub_head_moved_to_drift                    736, 706, 735, 644 ms of 1,000   (644)
P6   hub_full_recompute (cold run, 80,120 paths) 15.4, 12.4, 13.4 s of 180 s   (12.9)
```

P1, P2 and P6 differ from T3's single numbers by what they differ from run to run (P2 ranged 644 to 736 here); none of them is on the
audit path of the batch apart from the commit's events.

### Other registry places that write several events in one transaction

- T1 apply (`apply_chunk`): done above.
- T1 cluster report (`cluster.rs`): the sync-window and `PendingRestartChanged` events of one report now go through `publish_all`
  (three events are one insert; order tested).
- T3 pipeline commit (`persist.rs`): `DriftChanged` (one for each 500 paths, at most 20) and `FindingsChanged` through `publish_all`;
  `BaselineAdopted` and one `DriftMarkCleared` for each mark that no longer holds through `record_all` (two cleared marks are one insert;
  order tested in `pipeline_core.rs`). The typical commit has two outbox events and no audit event, so this saves one round trip there;
  it saves the most when many marks clear at once.
- T1 `ToolWrites::record`: one observation and one event for each call, in the caller's transaction; nothing to batch.
- T2 (the settings index, the Git side of drift, the tree hashes): no audit or outbox write at all.
- Nothing wraps the ports: `IngestDeps`, `PipelineDeps` and `Inner` hold the `Arc<dyn AuditLog>` and `Arc<dyn EventBus>` they are given.

### For the human

- `hub-identity` is a **dev-dependency** of `hub-registry` (and a workspace dependency in the root `Cargo.toml`), for the real `PgAuditLog` in
  tests and in the P15 benchmark. Plan A9 says the registry depends on hub-platform "and for nothing else"; that is true of the library,
  and a test or benchmark of P15 over a stand-in cannot say whether `record_many` is the real batch. If it is not wanted, the alternative
  is a copy of the batch insert in the stand-in, which proves nothing about the real log.
- The outbox is now written before the audit events in a chunk (it was after), and a full scan reads the files it lost before the first
  chunk instead of after the last (the same list: none of them is a file that a chunk of the scan touches). Both are for the lock and for
  the last chunk to be known; neither changes what is written (the snapshot holds).
- A crash in the last chunk now leaves nothing of the delta (it used to leave the chunk without its root, and asked for a full scan); a
  crash between earlier chunks is repaired by the agent sending the delta again, as before.
- `tests/explain.rs` (T2) flaked, one run in three, before and independently of this change: the plan of `markers_of_a_batch` was an
  index-only scan in the failing runs because the insert-triggered autovacuum had run before the EXPLAIN. The seeded tables now have
  autovacuum off (`0509cd7`); eight runs in a row passed; the snapshots are unchanged.
- The measurement is on one quiet machine; the first run of an invocation is sometimes the slowest. If CI is noisier than this machine,
  the margin is 13 % at the lowest of the 12 runs on the final code (1134) and 10 % at the lowest of the 21 runs since the trims (1103);
  with the batch calls alone it was one invocation in three below the budget (939).
