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

## T6: `P7.text_search` (400 ms, p95) is not met when the corpus has to be built: cold 577 ms, rebuilt 481 ms (warm: 191 ms)

Everything else in T6 is done and green, and nine of its ten budgets are registered and met (table below). This one is not registered
(`registered = false` in `perf/budgets.toml`); `bench-check` lists it as UNMET with the cold number. Nothing was loosened and the
benchmark's workload was not changed to flatter the number. The approved plan expected this: risk 3 says that 400 ms "is reachable only
warm" and that "if cold misses the budget this is an escalation, not a loosening".

### The budget and the benchmark

`docs/performance.md` P7: "Text search across all swimlanes | 400 ms", measured on the server. `perf/budgets.toml`: `P7.text_search`, bench
`hub-registry/search_text_all_swimlanes`, p95, 400 ms, scale target. The bench (`crates/hub-registry/benches/search_text_all_swimlanes.rs`)
runs the engine's twenty queries (frequent, rare and absent literals, case-insensitive ones, regexes) over the stored blobs of the 40
swimlanes of the read bed (80,120 files, 23,533 distinct blobs, 53.5 MB stored) through the axum router and the real blob store, and
checks every answer against the first one. It measures three states, all logged; **the budget takes the cold one**, because the
budget entry says the database read of the blobs is inside the number and because it is the slowest the user meets:

- **cold**: the hub has just started (every cache empty). File lists of 40 swimlanes, the blobs read from `blobs`, the corpus made, the
  search, the JSON. 40 samples.
- **rebuilt**: the corpus is made again with the blobs in the blob cache and the file lists cached. This is what the next search is after
  *any* applied change to *any* swimlane: the corpus is keyed by the (swimlane, version) pairs of every swimlane it covers
  (`swimlane_registry.version` moves on every applied change). On a busy fleet this, not warm, is the usual state. 40 samples.
- **warm**: the corpus is cached; the search itself. 60 samples.

### Measurement (final series, quiet 4-core machine with Postgres on it, release build)

| State | p50 | p95 | max |
|---|---|---|---|
| cold | 500.3 ms | **576.6 ms** | 705.8 ms |
| rebuilt | 396.3 ms | **480.6 ms** | 488.2 ms |
| warm | 126.4 ms | 191.1 ms | 223.3 ms |

The same measurement three times on earlier states of the code (twice before the corpus was assembled off the async threads, once
after it and before the last commit, which does not touch text search): cold p95 763, 757, 716 ms; rebuilt 635, 628, 570 ms; warm 218, 210, 186 ms. The spread between runs on this
machine is about 20 %, so no single change below can be told from it; all of them are above 400 ms.

### Where the time goes

From an instrumented run (timers around each step of the corpus build; removed again, not in the commits). Means over the 81 builds of
that run, in ms, and the first cold request of it:

| Step | mean | first cold request |
|---|---|---|
| 40 file lists (`FILES_SQL`, 8 at once, 80,120 rows in all; cold only) | 151.7 | 173.0 |
| places: swimlane and path of each of the 80,120 places, grouped by blob (before it ran in a hash map off the async threads) | 51.3 | 61.2 |
| blobs read through `BlobStore::get_many` (1,000 a batch, 4 batches at once; cold only; 23.5 k blobs) | 107.8 | 231.6 |
| assembling the blobs | 6.1 | 6.0 |
| `engine::SearchCorpus::new` (sorts the 80,120 places by swimlane and path, through two levels of indirection and string compares) | 103.8 (max 173.6) | about 86 |
| the search itself, corpus cached | 126 to 160 | 150 |

Added up for the first cold request: 3 + 558 (corpus) + 150 = 711 ms, of which the database (file lists and blobs) is 405 ms and the
engine's sort 86 ms. Even with the places and the assembly at nothing, cold is file lists + blobs + sort + search, about 640 ms in that
run. For rebuilt it is places + sort + search.

### What was tried

- The corpus is built once for a set of swimlane versions, concurrent first searches share one build (singleflight), the blobs are read
  in batches of 1,000, four at once, the file lists eight at once (all in T6 from the start).
- The place map was a `BTreeMap` built on an async thread (60 ms of CPU, against rule 4); it is a `HashMap` built, with the blobs
  assembled and handed to the engine, inside `spawn_blocking`. Rebuilt moved from 520 to 544 ms p50 before that to 396 to 468 ms after,
  inside the 20 % spread.
- Not done, because it cannot reach 400 ms alone and adds a second query: reading the distinct blob hashes with their own statement so
  that the blob reads start while the file lists are still loading. It would hide most of the file lists (up to about 150 ms) behind the
  blob reads; cold would still be about 430 to 480 ms at p95.

### Options (a human picks; none is taken here)

1. **Define the budget as the warm search** (191 ms p95 against 400, with room to spare) and register the warm result file
   (`hub-registry_search_text_all_swimlanes_warm.json`) instead. That changes what the budget measures; it is not mine to change.
2. **Make the corpus survive a change to one swimlane** (per-swimlane corpora, searched one after the other and merged with the caps; or
   an incremental corpus). On a busy fleet the corpus is invalidated by every applied change anywhere, so "rebuilt" (p95 481 ms) is the
   usual state, and this is the option that changes what users meet. It is a registry change (and a design change to the caps and the
   order of results).
3. **A faster `SearchCorpus::new`** (prompt 04, `crates/engine`): sort the places by integers computed once (a swimlane's ordinal and a
   path's rank, which the registry already has from its sorted file lists) instead of comparing strings through two indirections. About
   100 ms of the rebuilt and the cold search; I estimate under 15 ms. Not mine to edit.
4. **Warm the corpus in the background** after start-up and after changes (debounced), so a user's search is a warm one. It spends CPU
   and holds the corpus (about 60 MB at the target scale: 53.5 MB of blobs and the place lists; the plan's 230 MB was a larger guess) for as long as the hub runs; a product decision within P11's memory budget.
5. **Read the blobs faster**: 107.8 ms mean and 231.6 ms for the first request, for 23.5 k blobs, are in `hub-platform`'s blob store (not
   mine); a larger batch or more batches at once might help (the settings are `search_blob_batch` 1,000 and `search_blob_parallel` 4 in
   `RegistryConfig`, both tunable now, within the port's limit of 1,000 a call).
6. **Raise the budget for the cold search.** A human decision; the cold search is at 577 ms p95 and 500 ms p50.

## T6: the read budgets that were measured and registered

All through the axum router over real Postgres at the target scale (40 swimlanes, 80,120 files, 23,533 blobs, 867,285 settings), release
build, one quiet machine, one series run after the last code change (`cargo bench -p hub-registry`, every bench once, in the order of
`Cargo.toml`), checked with `cargo xtask bench-check` on the registry's budgets (the recipe's filter).

| Budget | Result (p95) | Threshold | What a sample is |
|---|---|---|---|
| P7.swimlane_list | 2.6 ms | 50 ms | the whole list of 40 |
| P7.file_tree | 6.8 ms | 100 ms | root, folder, filtered and all views |
| P7.file_content_cached | 1.7 ms | 30 ms | 80 files, the largest 146 kB |
| P7.file_content_cold | 1.7 ms | 80 ms | 628 distinct blobs, blob cache cold |
| P7.compare_api | 32.5 ms | 100 ms | two versions of a 6,173-line file, 40 pairs, cold (309 hunks, 103 setting changes) |
| P7.grid | 129.3 ms | 150 ms | one 6,000-line file across 40 swimlanes, a page of 500 rows of 40 cells (2.7 MB), **cold**: the service has nothing cached (warm: 27.6 ms) |
| P7.whole_swimlane_compare | 27.5 ms | 300 ms | 2,500 differing entries; 24 subtrees skipped |
| P7.settings_search | 63.2 ms | 200 ms | 20 needles, five rounds, a page of 100 (see the note on short needles below) |
| P12.steady_state_query_ms | 18.2 ms (the slowest statement's p95: 27.9 ms) | 50 ms | every statement of the read side, 39 of them, 20 runs each, as the service issues them |

The grid is the one with the least room (14 %). It was 389 to 439 ms before this task's optimisations. What moved it: the setting rows
are read in short statements that run side by side (`ROWS_PER_STATEMENT` 2, `STATEMENTS_AT_ONCE` 4); the arrays of a row are read in
place from the server's binary format (`array_bin`, a statement of four 6,000-setting blobs converts in about 5 ms instead of 10 to
30); and the group query asks for the application names that a file's name and folders can ask about instead of every service's
(12,000 rows for 40 swimlanes; 28 ms to 9 ms, for the warm grid too). The read pool is two threads, the figure of the plan (the hub pod has
2 vCPU); the merge of 40 effective configs (about 45 ms cold) and the rows are bound by it and by Postgres sharing the four cores here.

## T6: for the human

- **Short needles in the settings search are slow, and the 200 ms is met by the statistic, not by every request.** `q` may be one
  character (`minLength: 1` in `api/openapi.yaml`). The trigram index needs three, so a needle of one or two characters that matches few
  settings reads the whole index: a parallel sequential scan of 867,285 rows, 220 to 290 ms here. The bench's twenty needles include one
  of two characters (`id`, which matches nothing in the fixtures), five rounds, so exactly 5 of the 100 samples are that scan (220 to
  237 ms in the final series) and they sit above the p95 line; p95 is 63.2 ms. If one more request were that slow, the p95 would be
  about 235 ms. That workload was written before the first measurement and was not changed. The same scan is measured as a series of its
  own (`hub-registry/search_settings_short_needles`, 8 needles of one and two characters, no budget): p50 7.0 ms (dense ones are fast),
  p95 253.5 ms, max 290.2 ms. Options: a minimum needle length of three in the contract (`minLength: 3`, a contract change); a table of the
  distinct setting paths (a migration); or accept it. It is also a cheap request for a viewer to repeat, which the threat model should say.
- **A prepared statement that has run five times gets one plan for all values, and for a rare needle that plan walked the whole primary
  key** (1.0 to 1.4 s, against 0.3 ms to 9 ms for the plan made for the needle). The settings-search statements therefore run unnamed
  (`persistent(false)`), and `tests/explain.rs` now also asks for the generic plan of every read statement and refuses a sequential scan
  of a large table in it. It does not catch an ordered scan that is slow only for some values (that was this case), so the benches are
  the check for that. Other statements of the read side were not found to have it (the steady-state bench runs each 20 times, past the
  switch), but nothing proves that they cannot.
- **`SwimlaneSummary.tenants` has `minItems: 1`, but a swimlane that is not adopted has no tenants yet.** The service answers `[]`, which is
  what the domain says; `tests/http_contract.rs` pins exactly that one deviation from the schema walker. Either the schema or the data
  has to give.
- **An index on `file_state (nfs_hash)` (a migration, a contract path) would make the places of a setting one probe per blob, not one per
  blob and swimlane asked for.** The covering index is `(swimlane_id, nfs_hash)`, so a page of settings over 40 swimlanes probes it 40 times for every blob.
  Not needed for the budgets (the statement is 10 ms p95 in chunks of 25 blobs); noted for a later change.
- **Residual risks for the threat model (T10):** cursors are bound to their query by a tag and are not MACed (a client can forge one for
  the same query; it cannot reach another query's data, which a test holds); raw file text, blobs, diff hunks and text-search lines are not
  masked (A28); `only_differing` on the grid and the effective view hides a change between two *different* secrets of the same length and
  rule, because the index stores one marker for both (the compare reads the bytes and is not affected); text search keeps a corpus of up
  to `corpus_cache_bytes` (256 MiB by default; the target scale's corpus is about 60 MB) per distinct set of swimlane versions; the short-needle scan above; the
  served-file route is limited to 16 concurrent calls per replica and one agent round trip each (A36).

## Resolved: T6, `P7.text_search` is met in the steady state (the human's decision of 2026-10-15: per-swimlane corpora that survive a change, plus background warm-up)

The section "T6: `P7.text_search` ... is not met" above stands as the record of what was measured before. The human decided on
2026-10-15 that the registered case of the budget becomes the steady state (the first search after an applied change to ONE swimlane,
on a warmed hub, not waiting for the warm-up), with threshold 400, p95, ms, target scale and the bench name unchanged, and that the
corpus must survive a change to one swimlane. It is decision D97 in `docs/decisions.md`. `perf/budgets.toml` changed in the
`P7.text_search` entry only: its description and `registered = true` (pin in `crates/xtask/tests/budgets_registry.rs`).

### Design chosen and why

A snapshot of every swimlane's file list and of **8 shards of the distinct blobs, split by the blob's hash, each shard with all the
places (swimlane, path) of its blobs** (`src/read/corpora.rs`, `src/read/search_text.rs`). Not one corpus per swimlane, which was the
obvious reading of the decision and which the measurement refused: the engine's search time on the target fixtures (80,120 files, 23,265
distinct blobs, 2 threads, 20 queries, 5 rounds, engine only, no database; every configuration's answers are asserted equal to the single
corpus's):

| Shape | mean | p50 | p95 |
|---|---|---|---|
| one corpus (what it was) | 142 ms | 132 ms | 193 ms |
| 40 corpora, one per swimlane (a shared blob is searched in each) | 781 ms | 732 ms | 1,210 ms |
| 8 shards, one engine call after another | 170 ms | 158 ms | 273 ms |
| **8 shards, side by side on the 2-thread pool** | **140 ms** | **131 ms** | **194 ms** |
| 16 shards, side by side | 148 ms | 139 ms | 221 ms |
| 4 shards, side by side | 134 ms | 125 ms | 182 ms |

So per-swimlane corpora cost 5.5 times the search (the 3.4 times of the places per blob, and more), and sharding by content keeps a blob
searched once while a change to one swimlane makes again only the shards whose places changed: making one shard (the places of its
~3,000 blobs, `SearchCorpus::new`) takes about 12 ms at 8 shards (33 ms at 4, 7 ms at 16); compiling the query for each shard costs
0.5 ms. Eight is the default (`search_shards`, 1 to 64). How a search runs: the engine on every shard side by side and
`corpora::merge`. How a change is taken in: the changed swimlane's file list is read (the shards' keys are made of each swimlane's digest of
the paths and blobs it has in the shard, so a new version that changes nothing for a shard keeps the shard), the changed shards are made
again from the bytes they already hold plus the few blobs they lack, and the rest of the snapshot is shared with the one before.

The merge is exact because of what the engine's answer over one corpus is: the places that have hits in (swimlane, path) order, each
with at most `per_file` hits, at most `total` hits, and `truncated` when the total cut something or a place that was reached overflowed.
A place is in one shard and each shard keeps that order, so the merged hits cut at `total` are the whole's first `total`, and
`truncated` is "the shards hold more than `total` hits between them, or any shard says it was truncated".

### Measurements, before and after (final series on the final code, one quiet 4-core machine with Postgres on it, release build)

Before is the single corpus (the series of the section above; "rebuilt" is what the next search was after any change to any
swimlane, so it is that design's steady state). After is the same bench on the new code. p50 / p95 ms.

| State | Before | After |
|---|---|---|
| **steady: one swimlane changed, the search right after it (the registered case)** | 396 / 481 (rebuilt) | **156 / 201** |
| steady, with the warm-up running and settled | not a state | 140 / 184 |
| warm (nothing changed) | 126 / 191 | 134 / 195 |
| rebuilt-all (every swimlane's version moved, the files not: 40 file lists read) | 396 / 481 | 301 / 351 |
| rebuilt from nothing (blob cache warm; the old "rebuilt") | 396 / 481 | 372 / 426 |
| cold (just started, no warm-up) | 500 / 577 | 512 / 568 |

`bench-check`: `P7.text_search` 200.989 p95 against 400, PASS. The 40 steady samples are two for each of the twenty queries, each on
another swimlane, each after a real edit through ingestion (a new blob, a new version of the swimlane; the bench asserts that the
corpora were behind it), and each answer is held to the answer of a service that reuses nothing (the bench fails otherwise).
The registered file is `hub-registry_search_text_all_swimlanes.json`; the other states are in `..._steady_warmed`, `..._warm`,
`..._rebuilt_all`, `..._rebuilt` and `..._cold`. Cold is not better because it makes everything (every list, 23.5 k blobs from the database,
all shards): that is the warm-up's job at start-up, and a search before the warm-up has finished builds the same thing itself.

### The warm-up: cost and bounds (`src/read/warmup.rs`)

- **Memory:** the snapshot (the blobs, 100 bytes of allowance per place and the file lists) is 67.9 MiB at the target scale, inside
  `corpus_cache_bytes` (256 MiB by default). The bound is held: a snapshot over it is made for the request and not kept (a test). During a
  build the shards being replaced and their replacements coexist, so the peak is the snapshot plus the shards that changed (a change to one
  swimlane: the 2 shards it touches, about 17 MiB at most).
- **CPU:** the first build (start-up, or the first search) is about 0.4 s of work (the cold figures above less the search itself); a
  change costs about 25 ms (the steady figures less the warm ones: one file list, one or two shards); the poll is two small statements
  every `warmup_poll` (2 s by default) and builds happen at most once per `warmup_poll + warmup_debounce` (2.5 s by default).
- **Staleness window:** with a 100 ms poll and no debounce the corpora took a change in at a p50 of 63 ms and a p95 of 126 ms after the
  edit; at the defaults it is that plus up to 2.5 s. Inside the window a search does not wait for the warm-up: it takes the change in itself
  (the steady state above).
- **A search never returns a version older than the swimlane's current one:** it reads the versions first, uses the snapshot only if it
  covers them, and builds otherwise; the lists are read after the versions. `tests/read_corpora.rs` changes a file, searches at once and
  finds the new content and not the old one, in four rounds with the warm-up not running and with it running; the test fails if the check
  of the versions is removed.
- **Bounded, and not for a viewer to drive:** one build at a time on a replica (a lock; the searches that waited take the result), each
  build bounded by the swimlanes that moved, the shards that changed and the blobs they lack (every call has a timeout), at most one
  build per poll period from the warm-up. Nothing a viewer sends reaches the warm-up. A viewer's search builds only when the data moved
  past the snapshot, so the builds are the writers' doing, and at most one runs at a time. A failing store is logged by kind and tried
  again at the next poll; the task never ends on its own and stops when asked (also in the middle of a build) or dropped.

### Evidence of equivalence

- `tests/corpora_equivalence.rs`: 600 random corpora (swimlanes that share contents, CRLF files, NUL and non-UTF-8 files, files with more
  matching lines than the per-file cap, 1 to 8 shards) with random queries (literals and regexes, both cases, a swimlane list, a glob, caps of
  1 to 59 hits and 1 to 7 per file, ceilings of 5, 25, 1,000 and 2, 6, 50): the engine on one corpus against the engine on the shards merged,
  `TextResults` equal. Four wrong merges (no cut, no sort by path, a cut with `>=`, a flag taken from the shards only) each fail it.
- The same over the target-scale fixtures (the slow test, release): the twenty benchmark queries at three cap and swimlane-list settings and
  1, 8 and 16 shards, 180 comparisons, equal; more than ten of the answers are cut by a cap.
- Every steady and warmed sample of the bench is checked against a service that reuses nothing, on the real data.
- `tests/read_corpora.rs` also holds the incremental snapshot to a fresh service's answer after a change.

### Residual risks for the threat model (T10)

- **Memory held per replica:** 67.9 MiB at the target scale and up to `corpus_cache_bytes` (256 MiB) by configuration, plus the transient
  shards of a build; every replica holds its own copy (3 replicas, so 3 times that).
- **Staleness:** a search is never older than the swimlane's version as the registry read it; the warm-up's own window is 2.5 s at the
  defaults and is invisible to a search. The assumption is the old one, that the registry's version of a swimlane moves in the same commit as
  its files (or after it): a writer that moved the version before its files were visible would make a search read the old files at the new
  version.
- **What a viewer can cause:** a search costs the same as before (the regex over every blob: the rayon pool, 5 s deadline) and a build
  only when data moved; the short-needle scan of the settings search above is the one request that is expensive and repeatable.
- **The warm-up reads every blob:** the replica holds the content of every readable structured or text file in memory (as the search
  corpus did, when it was built); a process dump of a replica holds config text (A28 says no secrets are in it, D18).
- **A blob the store lacks** is left out of the corpus with its places (as before) and its shard is made again by every build until it
  turns up.
