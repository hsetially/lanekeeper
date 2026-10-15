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
obvious reading of the decision and which the measurement refused: the engine's search time on the target fixtures (the 78,414 structured and
text files, 23,265 distinct blobs, 2 threads, 20 queries, 5 rounds, engine only, no database; every configuration's answers are asserted equal to the single
corpus's):

| Shape | mean | p50 | p95 |
|---|---|---|---|
| one corpus (what it was) | 142 ms | 132 ms | 193 ms |
| 40 corpora, one per swimlane (a shared blob is searched in each) | 781 ms | 732 ms | 1,210 ms |
| 8 shards, one engine call after another | 170 ms | 158 ms | 273 ms |
| **8 shards, side by side on the 2-thread pool** | **140 ms** | **131 ms** | **194 ms** |
| 16 shards, side by side | 148 ms | 139 ms | 221 ms |
| 4 shards, side by side | 134 ms | 125 ms | 182 ms |

So per-swimlane corpora cost 5.5 times the search (they hold 3.3 times the blobs, 76,519 against 23,265), and sharding by content keeps a blob
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

## T1 (A10 follow-up): `ports::ToolWrites` cannot be implemented soundly on the 0001 to 0004 schema; it needs one new table (proposed `0005`)

Status: BLOCKED. Nothing of the port is implemented. The old seam (`src/ingest/tool_writes.rs`, `ToolWrites::record`) is
untouched and still compiles and passes. `db/migrations` is untouched. The rest of this section is what a contract-change PR for
`0005` would carry.

### What the port needs

From `crates/ports/src/tool_writes.rs`, `docs/interfaces.md`, D73, the fake and the conformance suite:

1. `register(tx, swimlane, path, new_hash, user, request_id)` writes in the CALLER's transaction: the registration is visible
   only if that transaction commits, and a rollback leaves nothing.
2. The same (swimlane, path, hash, **request**) again is a no-op; the same hash from another request, at another path or in
   another swimlane is another registration. `ToolWritesProbe::registered()` reads back swimlane, path, hash, **user** and
   **request id** of every committed registration, oldest first.
3. Bounded: `Full` comes only from `register`, never from the commit, and a repeat of a registration already held is a no-op, not `Full`.
4. Expiry: an unmatched registration goes `TTL_SECS` (15 min) after it became visible; a newer observation of the same path with
   another hash drops it at once; dropping is not audited.
5. Late commit: an observation that arrives before the registration is visible must be kept as unattributed for at least `TTL_SECS`
   and upgraded (never downgraded, one `AttributionUpgraded` audit event and one `AttributionUpdated` outbox event in the same
   transaction) when a matching registration commits.

### Why the existing schema cannot hold it

The only table that can hold a row without a migration is `file_observations` (source `tool_write`), which is what the old seam
used. I looked for every other candidate (`idempotency_keys`, `outbox`, `leases`, `delta_staging`, a key in
`swimlane_registry.state_counts`, in-process state, `pg_notify`). None has a home for (swimlane, path, hash, user, request) with a
unique key and an expiry; the outbox is a fan-out queue with no user or request id in its events; leases carry no payload and
`registered()` could not read a hashed name back; `pg_notify` and process memory are not durable and cannot be read back by the
conformance probe right after the commit, and they are not shared by replicas. So `file_observations` is the only option, and it fails
on four counts. Two I measured, one I reproduced, one is a reader audit.

1. **The registration collides with the real observation (reproduced).** `file_observations_replay_key` is unique on
   `(swimlane_id, path, observed_at, hash)`, and ingestion inserts with `ON CONFLICT DO NOTHING`. A registration row and the
   agent's observation of the same version at the same millisecond share that key, and the observation is silently not recorded:
   ```
   INSERT ... VALUES ('sit1','svc/a.yml', sha256('x: 2'), 'tool_write', '2026-10-15 10:00:00+00', 'unknown', 'low');   INSERT 0 1
   INSERT ... VALUES ('sit1','svc/a.yml', sha256('x: 2'), 'scan',       '2026-10-15 10:00:00+00') ON CONFLICT DO NOTHING; INSERT 0 0
   ```
   Tests date an observation by hand against a `ManualClock`, so they can land on the hub's instant exactly; in production it is a rare
   and silent loss (the pipeline's `last_observation_id` would then point at the registration, and the registration's later deletion
   leaves it dangling).
2. **The bound and the expiry cannot be checked cheaply (measured).** Nothing indexes `source` or "pending". `register` has to count
   the waiting registrations in the caller's transaction. With one swimlane's 15 minutes of ingestion at the P15 rate (900,000
   observations, 20 registrations), on this machine (Postgres 16, loopback, `shared_buffers` default):

   | Query | Plan | Time |
   |---|---|---|
   | count `source = 'tool_write' AND attribution_source = 'unknown' AND observed_at > now() - 15 min` | parallel seq scan, 16,840 buffers | 94 ms |
   | the same limited to the bound + 1 (stops early only when many are pending) | parallel seq scan | 116 ms |
   | the same forced through `file_observations_swimlane_time_idx` | bitmap scan, 859,064 index entries, 859,043 rows removed by the filter | 407 ms |

   That is 0.1 to 0.4 s inside 06's transaction for every write, growing with the ingestion rate and the window. A partial index
   on the pending rows would fix it and is exactly the migration this section asks for.
3. **No place for the request id, and no unique key over it.** Idempotency is by (swimlane, path, hash, request). The table has no
   such column or index. It could be done with the request id in `attribution_evidence` and an advisory lock around a
   `NOT EXISTS`, which is a protocol that every writer must follow instead of a constraint.
4. **A registration is not an observation, and every reader would have to know (audit of the readers).** The rows would be read as
   history by: `read/history.rs` (the history page and **`is_version_of`**), `read/tree.rs` and `pipeline/lane_loader.rs` (last
   observation, attribution), `ingest/apply.rs::existing_observations`, `sync/overwrite.rs::claim`, and the retention rule of D78
   (the last 50 versions per file), when it is written. One of them is a security check: `is_version_of` decides whether
   `ContentSource::Blob { hash }` may serve the blob of `hash` for a path ("a version this path has had"). A registered hash is
   the content someone is about to write, often a proposal's or a draft's blob that the blob store already holds. With the
   registration in `file_observations` that blob is readable by any Viewer of the swimlane through the file-content read (`ContentSource::Blob`) before the
   write happens, or if it never happens. **The old seam on this branch has that exposure today** (it has never been merged); any design that keeps
   registrations in `file_observations` must add the filter to each of these readers and to every future one.

Each of the four has a workaround; together they make the table do two jobs, with a marker that no constraint enforces. A table of
its own makes all four disappear (a unique key, a small table for the bound, no reader to audit).

### Proposed `0005_tool_write_registrations.sql` (expand-only; validated, see below)

```sql
-- 0005_tool_write_registrations.sql: the registry's half of the tool-write port (prompts 05 and 06, D73). A contract: never
-- edit it once merged. Run it as `lanekeeper_migrator`, behind the same guard as 0001 to 0004.
--
-- One new table, one index and one grant. Nothing existing is touched. Deploy order: this migration, then the 05 code that
-- implements `ports::ToolWrites`; a binary from before it never reads or writes the table.

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

-- A write the tool is about to make (or has made) and no observation has explained yet. Written by `ToolWrites::register` in
-- the caller's transaction, so a row exists only if that transaction committed. Consumed by the first observation of the hash
-- at the path (ingestion, or the matcher when the registration committed late), dropped `ToolWriteEntry::TTL_SECS` after it was
-- made, and dropped at once by a newer observation of the path with another hash. Dropping is a DELETE and is not audited.
--   registered_at  the registry's clock when `register` ran, which is at most the commit time: the TTL runs from here.
--   user_tid/oid   the user who wrote, the actor of the attribution. No foreign key: the row is a short-lived expectation, and a
--                  user row never decides whether a write was the tool's.
--   request_id     the 06 request: (swimlane, path, hash, request_id) is one registration, so a retry registers nothing twice.
CREATE TABLE tool_write_registrations (
  id            bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  swimlane_id   text        NOT NULL REFERENCES swimlanes (id) ON DELETE CASCADE,
  path          text        NOT NULL CHECK (length(path) BETWEEN 1 AND 1024),
  hash          sha256_hash NOT NULL,
  user_tid      text        NOT NULL CHECK (length(user_tid) BETWEEN 1 AND 64),
  user_oid      text        NOT NULL CHECK (length(user_oid) BETWEEN 1 AND 64),
  request_id    text        NOT NULL CHECK (length(request_id) BETWEEN 1 AND 128),
  registered_at timestamptz NOT NULL,
  -- Idempotency, and the lookup of ingestion (`swimlane_id = $1 AND path = ANY($2)`) and of the matcher.
  UNIQUE (swimlane_id, path, hash, request_id)
);
-- The expiry sweep and the bound: the oldest registrations first.
CREATE INDEX tool_write_registrations_age_idx ON tool_write_registrations (registered_at, id);

-- write-once: SELECT, INSERT, DELETE (README grant convention). A registration is never updated.
GRANT SELECT, INSERT, DELETE ON tool_write_registrations TO lanekeeper_app;
```

- **Grants:** SELECT, INSERT, DELETE (the README's "write-once" class: removed by expiry, consumption and invalidation, never
  updated). No UPDATE: the `ON CONFLICT DO NOTHING` insert needs none. No TRUNCATE.
- **CHECKs:** the lengths of `path` (as everywhere), `user_*` (as `users`) and `request_id` (as `idempotency_keys.key`); `hash` is
  the `sha256_hash` domain (32 bytes).
- **No foreign key to `users`:** the conformance suite registers for users that have no row, and a registration is a short-lived
  expectation, not a record of a user. `ON DELETE CASCADE` from `swimlanes` is allowed because the app may delete from the table.
- **The key doubles as the lookup** ingestion and the matcher need (`swimlane_id = $1 AND path = ANY($2)`); the age index serves
  the expiry sweep and the bound (`WHERE registered_at > cutoff ... LIMIT bound`).
- **Deploy order:** the migration, then the 05 code. A binary from before it never touches the table.
- The contract-change PR also needs, outside this crate: the `grant_decisions` entry and the catalog-diff test for 0005 in
  `crates/xtask/tests/db_migrations.rs`, a README section, and a new D-entry that supersedes D91 (D91 is 05's, from plan A10).

What I ran against a scratch database (`0001` to `0004` applied as `lanekeeper_migrator`, `0005` applied as `lanekeeper_migrator`,
then used as `lanekeeper_app`): the insert; the repeat of a key (`INSERT 0 0`); another request id for the same hash (`INSERT 0 1`);
a rollback leaving the count unchanged; `UPDATE` and `TRUNCATE` refused (42501); `DELETE` allowed. The scratch database is dropped.

### The registry side once 0005 exists (so that one approval covers the whole change)

- **`register`** (the port, in the caller's transaction): `WrongTx` for a foreign transaction; a repeat of a held key returns `Ok`
  without a bound check; otherwise count the registrations newer than `now - TTL` up to the bound (`RegistryConfig::tool_write_max_pending`,
  range-checked, default 10,000 like the fake) and answer `Full`, else `INSERT ... ON CONFLICT DO NOTHING`. The bound is soft by at
  most the number of registering transactions in flight: a hard one needs a lock held until the caller's commit, whose order against
  the audit chain's lock I cannot control from here (06 may record its audit event first), and a deadlock with an ingestion apply
  is worse than a short overshoot. The commit cannot fail for lack of room because nothing is reserved at commit.
- **Ingestion** (`decide` in `apply_chunk`, which already holds the swimlane lock): reads the registrations of the chunk's paths
  from the new table instead of the `tool_write` rows, ignores those older than `TTL_SECS` by the injected `Clock`, explains an
  observation of the registered hash with the **earliest** valid registration (source `ToolWrite`, attribution `tool_write` /
  `certain`, the registration's user), and in the same transaction deletes the registrations it used and those of the same path with
  another hash (invalidation). No audit event for a drop (the port says so); `FileChangeDetected` is not written for an explained change, as today.
- **Late commit:** a leased job `ToolWriteMatcher` (an interval from `RegistryConfig`, `run_once()` for the tests) takes the
  swimlane lock, finds unattributed observations (confidence below `certain`, newer than `TTL_SECS`) whose (swimlane, path, hash)
  match a registration that committed after them, upgrades them, writes the `AttributionUpgraded` batch and the `AttributionUpdated`
  outbox batch through `crate::batch` as `claim_chunk` does, and consumes the registration. It also deletes expired registrations in
  bounded batches. The lost-wakeup window (an observation committed between `register` and the caller's commit) is closed by this
  job and not by a lock, for the same lock-order reason. Latency is one interval.
- **The old seam goes:** `ToolWrites::record`, `ToolWriteRecord` and `src/ingest/tool_writes.rs` are removed from the public API
  (06 calls the port, nothing else); the `tool_write` observation rows and `AgentIngest::tool_writes()` go with them. Tests that
  encoded the old model are rewritten with the reason in the commit message.
- **Tests:** `ports::conformance::tool_writes` through a `ToolWritesProbe` over real Postgres; registry tests for the late commit
  (both orders), the TTL with `ManualClock`, invalidation by a newer observation with another hash, `Full`, idempotency inside and
  across transactions, rollback, the same hash at another path or swimlane not matching, the lost-wakeup window, S10 in the
  log-capture test (no path, hash or user in a log line or an error), an `EXPLAIN` snapshot of the bound and the lookup (no seq scan).

### What the human must decide

1. **Approve `0005`** as a `contract-change` PR (the plan's approved contract change covered `0003` only). Without it the port stays unimplemented and 06 has nothing real to call.
2. **Deletes cannot be registered.** The port's `register` takes `new_hash: &ContentHash`. The old seam registered a delete with
   no hash, so the scan that saw a tool's delete was not out of band; D91 says so. With the port as written a tool delete is an
   out-of-band change (`FileChangeDetected`, author unknown) on top of 06's own audit event. Options: widen the port to
   `Option<&ContentHash>` (a contract-change of `crates/ports` and the fake), or accept the double audit for deletes.
3. **The TTL starts at `register`, not at the commit.** Postgres gives a caller-owned transaction no commit hook or timestamp
   (`track_commit_timestamp` is a server setting I cannot assume). A registration therefore expires up to the caller's
   transaction time (milliseconds, bounded by 06's transaction timeout) earlier than "TTL after the commit". That is the safe
   direction. The alternative is a deferred constraint trigger that stamps the commit time, which is more schema for a few milliseconds.
4. **Soft bound** (above) and **earliest registration wins** when two requests registered the same hash for the path.
5. **The observation source of an explained change** becomes `ToolWrite` (the port says so). The old code kept `Scan` with a
   `tool_write` attribution, and `publish_moves` only ever publishes `Scan` or `Sync` in `FileObserved`. The registry's consumers
   (the pipeline) do not branch on it today; `docs/mcp-tools.md` allows `tool_write` in `ObservationSource`.
6. **D91 must be superseded** (it describes `record`, an observation row, an event on registration, delete registration).

### What is done and committed

Step 0, the regression check after merging `main` (`3328e88`, the 06 write contract: migration `0004`, ports and domain additions, the
`ToolWrites` port with its fake and conformance suite). Run with `LK_TEST_PG_ADMIN_URL` set (Postgres 16 with pgvector on loopback;
no test printed `SKIPPED`), `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`:

| Command | Result |
|---|---|
| `cargo check --workspace --all-targets --locked` | exit 0 (2 m 16 s); `Cargo.lock` is consistent with the merge |
| `cargo test -p hub-registry --lib --tests` | 49 test binaries, **623 passed, 0 failed, 3 ignored** (the migrations of the tests include `0004`) |
| `cargo test -p xtask` | 11 test binaries, **123 passed, 0 failed, 1 ignored** |

**The merge broke nothing**: no compile error, no test failure, so there is no `05/T1:` fix commit. The things the merge could
have touched (an `AuditEvent` author field, the audit size constants, the new ports and domain items, `0004` on `proposals`,
`approvals` and `github_credentials`, `Cargo.lock`) are not used by the registry in a way that changed.

Committed: this record only (`05/blocked:`). `db/migrations`, `crates/ports` and `crates/domain` are untouched; no source file of
`crates/hub-registry` changed. The old seam is still the one T1 built, and it still passes its tests; it must not be called by 06
(06 will call `ports::ToolWrites`), and it carries the `is_version_of` exposure described above until it is replaced.

Not run, because the work stopped before it: the conformance suite over Postgres, the registry's bench (no code changed, so there
are no new numbers; the numbers recorded in the sections above stand).


## T9 (second half, retention): STOPPED before any code. The registry cannot delete a blob safely on the contracts as they are; three questions for the human

Stop rule of the brief, met: `ports::BlobStore` has no delete and no way to check references atomically. Checking what the registry
would have to do instead found two more questions that a conditional delete does not answer. Nothing was built for the second
half (no `src/retention/`, no config fields, no tests); `src/severity.rs` and `src/denied.rs` (`6e2c02d`) are untouched; no tool-write
code was touched. `crates/ports`, `crates/hub-platform`, `crates/domain`, `db/` and `perf/budgets.toml` are untouched.

### 1. The port has no delete, and no port could do the check (decision: may the registry delete from `blobs` by SQL?)

- `ports::BlobStore` (`crates/ports/src/blob.rs`) is `put`, `get`, `get_many`. The fake (`ports::fakes::blob_store`) and `PgBlobStore`
  (`crates/hub-platform/src/blobs/store.rs`) are the same. Nothing else in `ports` deletes a blob.
- "Delete this blob unless something references it" has to name the tables that reference blobs (`file_state`, `baselines`,
  `delta_staging`, `adoptions`, ...). Those are the registry's. A port method would either carry the registry's schema into
  `hub-platform` or take a list of "protected" hashes and lose the atomicity the brief asks for.
- The database contract already expects the delete to be SQL: `lanekeeper_app` has `SELECT, INSERT, DELETE` on `blobs` and `blob_refs`
  ("content-addressed or immutable rows that retention or garbage collection may remove", `db/migrations/README.md`),
  `settings_index` and `settings_index_blobs` cascade on a blob delete ("Rows go when the blob is garbage-collected", 0001), and the
  registry already reads `blobs` with SQL (`baselines.rs:38`, `sync/record.rs:296`, `adoption/confirm.rs:555`; it writes only through
  `put`).
- Side effect of deleting behind the port: `PgBlobStore` keeps a per-process cache by hash. A collected blob stays readable on a replica
  whose cache holds it until it is evicted and is a 404 on the others. Content is immutable, so this is harmless to correctness, but
  the blob endpoint answers differently per replica for a while.

Recommendation: yes. The registry's job deletes with one SQL statement under its grant, in `crates/hub-registry`, and records this as
a D-entry. No contract file changes for this part.

### 2. What does "referenced by an audit event" mean? (decision; it decides whether retention collects anything)

- **Nothing writes `blob_refs` rows of kind `audit_event`.** The CHECK allows the kind (and `proposal`, `draft`, `pr_link`), but
  `hub-identity`'s audit log, `hub-platform` and `ports` never insert one. The registry writes only `baseline` rows (`baselines.rs`).
- **Every out-of-band or sync-sourced version is named by an audit event.** D93 / `ingest/apply.rs:596-597`: one `FileChangeDetected`
  per path and chunk with `hash_before` = the first version's predecessor and `hash_after` = the last version's hash; `sync/record.rs`,
  `sync/overwrite.rs` and `adoption/marks.rs` do the same, and 06 audits its in-tool writes (plan A12). If "referenced by an audit event" means "its hash is in
  `audit_events.hash_before` or `hash_after`", then nearly every version the hub ever recorded is kept for ever (only versions
  that were coalesced inside one chunk, superseded before apply, or staged and never applied are left), so **the keep rules of 50
  versions and 180 days would never decide anything** and the store would grow without bound. That reading makes D78's own last
  bullet pointless ("Audit events keep their own diff text, so evidence survives garbage collection").
- **If that reading is wanted, the check has no index.** `audit_events` has no index on `hash_before` or `hash_after`, and the table
  is append-only and kept for ever. Measured in a scratch database (migrations 0001 to 0004, `lanekeeper_app`, 500,000 synthetic rows
  of 600 bytes, heap 434 MB), for one batch of 500 candidates, page cache warm:

  | Form of the check | Plan | Time |
  |---|---|---|
  | `WHERE EXISTS (... a.hash_before = c.hash OR a.hash_after = c.hash)` per candidate | nested loop semi join, 250 M comparisons | **51 s and 53 s** (two runs) |
  | `WHERE hash_before = ANY($1) OR hash_after = ANY($1)`, no index | parallel seq scan, 54,613 pages read | **141 ms and 146 ms**, linear in the table (about 9 GB per batch at 10 M rows) |
  | the same with two partial indexes (`hash_before IS NOT NULL`, `hash_after IS NOT NULL`) | two bitmap index scans | **11.6 ms**; the indexes are 56 MB at 500,000 rows and cost two more btree inserts per audit event |

  The indexes are a migration on a table 03a owns (additive, `lanekeeper_migrator`); the registry cannot add them.

Options:
- (a) Audit events pin the blobs they name. Needs the two indexes (contract change, `db/migrations`), costs two btree inserts per audit event,
  and retention then only collects what no audit event ever named. Honest consequence: almost nothing; say so in D78.
- (b) Audit events do not pin through their hashes, because they carry their own diff text (D78, last bullet). An audit event pins a
  blob only through a `blob_refs('audit_event', ...)` row that its writer chose to add (none today; 06 may for the versions an
  approval or a PR names). Retention works as designed: 50 versions or 180 days, baselines, proposals, drafts and PR links always.
  No migration. The FK on `blob_refs` makes a mistaken delete of a pinned blob fail instead of succeed.
- (c) The registry's audit writes also insert `blob_refs('audit_event', seq)` for each named hash that is a stored blob (guarded by
  `EXISTS`, as `baselines.rs` does). Same outcome as (a) with the FK as a safety net; costs two guarded inserts per event on the
  P15 path, which was just brought to its budget.

Recommendation: (b), recorded in `docs/decisions.md` as the reading of D78 with the human's confirmation, because it is the only
reading under which "keep the last 50 versions or 180 days" means anything. If the human wants (a) or (c), the migration (a) or the P15
re-measurement (c) comes first.

### 3. A conditional delete does not close the race the brief names (decision: how are writers kept from losing a blob they just stored?)

The brief asks for the delete to be conditional on the absence of references inside the same transaction. That is necessary and is
what I would build (below), but it cannot see a reference that has not committed. Ingestion stores a blob **before** the transaction
that references it: `ingest/delta.rs` `store_blobs` calls `BlobStore::put` (its own connection, committed at once, `ON CONFLICT DO
NOTHING`), then the apply transaction (`ingest/apply.rs`) writes `file_observations` and `file_state`, and a non-final part of a delta waits in
`delta_staging` for up to `staging_ttl` (24 h) with only its hashes in `metadata`. Between the put and the commit of the reference:

1. a blob that already exists, is unreferenced and is beyond the keep rules (a file reverting to content from over 180 days ago whose
   blob has not been collected yet) is a no-op `put` and keeps its old `created_at`, so an age grace on `blobs.created_at` does not
   protect it;
2. GC's conditional delete finds no reference (correct: none has committed) and deletes it;
3. the apply transaction commits a `file_state.nfs_hash` for a blob that no longer exists. Nothing stops it: `file_state`,
   `file_observations`, `baselines`, `delta_staging`, `adoptions` and `audit_events` have **no foreign key to `blobs`**. The only
   tables that do are `blob_refs`, `proposals.new_hash` and `drafts.new_hash` (RESTRICT, so a mistaken delete fails loudly and a late
   writer gets a clean FK error) and `settings_index`, `settings_index_blobs` (CASCADE).

The result is a current version of a file with no bytes, silently, and the agent will not send the content again because its tree
has not changed. Reads degrade to 404 (see below), but the data is gone. The window is milliseconds for a single-part delta and
bounded by a `put` to its staging commit for a multi-part one (staged hashes can be read from `delta_staging.metadata`), and it needs a
night-time revert of old content; rare, but the consequence is a loss and the brief called this race out.

Options:
- (W1) Cooperation inside `crates/hub-registry`: the apply transaction takes `SELECT hash FROM blobs WHERE hash = ANY($1) FOR SHARE`
  on the blobs it is about to reference, before it writes anything, and treats a missing row as "collected between put and commit" (a bounded
  re-put when the bytes are in hand, which is the single-part case; otherwise fail the message so that the agent sends it again and
  flag a full scan, as `complete` already does). GC takes the rows `FOR UPDATE SKIP LOCKED` first, so a blob a writer holds is skipped (the
  bounded skip policy), then runs the reference check as a **second statement** (READ COMMITTED gives it a new snapshot that sees
  everything committed so far, which a single `DELETE ... WHERE NOT EXISTS` would not after waiting on a lock), then deletes, all in one
  transaction. Covers the registry's own writers; 06's drafts and proposals are covered by their FKs; anything else that references a hash
  without an FK (06's PR links, 07's docs) must do the same lock.
- (W2) One contract change instead: `PgBlobStore::put` refreshes `created_at` when the blob exists and is older than a day
  (`ON CONFLICT (hash) DO UPDATE SET created_at = now() WHERE blobs.created_at < now() - interval '1 day'`), which needs `UPDATE` on `blobs`
  for `lanekeeper_app` (a new migration and its grant decision, 03b's `store.rs`), and GC refuses any blob younger than `gc_min_age`
  (48 h, above `staging_ttl`). Then no writer has to cooperate, and the age grace also covers staged parts. Costs a write only for a
  re-put of a blob that is over a day old.

Recommendation: W2 if the contract branch that is already open for `ports::ToolWrites` (`0005`) can take a second small migration; W1 if
not. Either way the conditional delete below is built.

### What I would build once the three answers are in (so that approval covers the whole of it)

- `src/retention/plan.rs`: pure selection of candidates from the keep rules (Q33 defaults read from `retention_settings`: last
  `keep_versions` versions per file by `file_observations` order, or newer than `keep_days`), the reference classes as SQL over indexed
  columns, no hash or path ever formatted into a message; `src/retention/job.rs`: the leased `registry-retention` job (`JobSpec`, `run_once()`),
  at most `gc_batch` (500) blobs and 64 MiB per run, `dry_run` reports and changes nothing, counts and bytes in logs and metrics only,
  GC deletes not audited (A12, human-approved); `src/retention/mod.rs`: the settings (`RegistryConfig` fields, range-checked).
- The delete: one transaction, `SELECT ... FOR UPDATE SKIP LOCKED`, then `DELETE FROM blobs WHERE hash = ANY(locked) AND NOT EXISTS (...)`
  for every class (`blob_refs`, `file_state.nfs_hash`, `baselines.hash` and `blob_hash`, the hashes in `delta_staging.metadata`,
  `adoptions` and `adoption_resolutions`, the registry's pending pipeline state, and per answer 2 the audit hashes), counting what it deleted.
- The nightly prune the orchestrator delegated: processed `sync_windows` older than `sync_window_retention` (7 days, range-checked) and expired
  `delta_staging` in bounded batches; the budget-unchanged test over `sync_windows` before and after pruning.
- Tests: `tests/retention_gc.rs::{gc_never_deletes_a_referenced_blob (proptest), keeps_the_last_50_versions_and_180_days, dry_run_deletes_nothing}`, the
  interleavings over real Postgres (a reference committed between plan and delete; a writer holding the row lock; a collected blob
  met by a writer), S10 in the log-capture test, one lease holder, `run_once` idempotent; and the mutation checks the brief lists.

### Found while looking, independent of the answers

- **`delta_staging` already has a sweep, with no caller.** `IngestInner::sweep_staging` (`ingest/hello.rs:129`, reachable as
  `AgentIngest::sweep_staging`) is one unbounded `DELETE ... WHERE staged_at < $1`; the comment says "a job may call it too", and none does.
  Only a `hello` of the same swimlane discards that swimlane's old parts. The nightly job would call it (made batched) and not duplicate it.
- **A vanished blob does not make a read or a search fail.** Read by code, not yet exercised with a real GC: `read/content.rs:222-230`
  answers `NotFound` (404) and logs one warning that carries the hash (existing T6 behaviour, S10 allows hashes in logs); the text-search
  shard asks `get_many`, which leaves unknown hashes out, and `assemble_shard` builds from what came back (T6's remake-the-shard rule).
  The settings index cascades with the blob. A current file's blob is never a GC candidate, so none of this is reached except through the
  race in question 3.
- **A blob of a denied file** is collected like any other once beyond the keep rules; D104 point 2 (the blob endpoint refuses every version
  a denied file has had) holds until then and is independent of GC.
- **Scratch state.** The scratch database of the measurements (`lk_t_<pid>_gcprobe`) was dropped. Other `lk_t_*` databases that were
  already there belong to other pids and were not touched. Nothing was run in release mode; no benchmark was run.
