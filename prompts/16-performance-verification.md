# 16: Performance verification at design scale

| | |
|---|---|
| **Wave** | 3. Waves 1 and 2 merged, and staging deployed. |
| **Depends on** | Everything in v1; Q27 confirmed |
| **You own** | `perf/**` (load scenarios), `crates/xtask` (load-test commands), `docs/performance-report.md` |
| **Performance** | P1–P13 (verification) |
| **Gate** | `just verify-16` |

## Objective

Show that every budget holds at design scale on staging, under concurrent load, with 3 hub replicas. Leave behind a repeatable gate that runs on every release.

## Tasks

### T1: Environment

- Load target-scale synthetic fixtures into staging: 40 simulated swimlanes using agent simulators, which run the real agent code against fixture directories, plus fixture repos with webhooks.
- Record the hardware and configuration in `docs/performance-report.md`.

### T2: Load scenarios

Write the scenarios in Rust, as xtask load tests driving HTTP, SSE and MCP clients.

**Mixed web load:** 100 users browsing, comparing, searching and editing, in realistic proportions.

**MCP sessions:** 30 sessions running read-tool chains.

**Concurrent with that load:**

- agent churn, with reconnects across replicas;
- Git push bursts;
- an out-of-band edit storm across 40 swimlanes;
- the nightly full recompute;
- an audit checkpoint.

### T3: Measure every budget

For each P#, record p50, p95 and p99 with the measurement method:

- **P1:** the time from NFS change to the UI event.
- **P2:** the time from push to drift.
- **P3/P4:** agent traffic and resource use.
- **P5/P6:** recompute times.
- **P7:** every read endpoint.
- **P8:** writes.
- **P9:** MCP read tools.
- **P10:** Lighthouse and in-browser traces, measured through the VPN path.
- **P11:** hub memory and startup time.
- **P12:** slow-query log, which must be empty above 50 ms.
- **P13:** SSE latency.

### T4: Profile and fix

- For any budget that's missed, capture profiles: CPU flamegraphs, allocation profiles, and Postgres `EXPLAIN ANALYZE`.
- Fix the code, if it's owned by a v1 prompt, through a focused PR.
- If an architecture change is needed, escalate.

### T5: Resilience under load

- Kill a hub replica during load: agents reconnect, SSE resyncs, and no write is lost or duplicated, thanks to idempotency keys.
- Fail over Postgres with CloudNativePG: the system recovers within its documented time window.

### T6: Gate

`just verify-16` runs a reduced version of the scenario set in CI nightly against staging, and fails if any budget regresses by more than 10% from the last baseline.

## Acceptance (all required)

- [ ] `docs/performance-report.md` shows every P# within budget at design scale, under load.
- [ ] The resilience tests pass.
- [ ] The nightly regression gate is active.

## Stop and escalate if

- A budget can't be met without changing a decision or contract. Include the profiles.
