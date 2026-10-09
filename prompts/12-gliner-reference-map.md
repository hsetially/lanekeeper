# 12: GLiNER reference map (v2)

| | |
|---|---|
| **Wave** | v2 |
| **Depends on** | v1 |
| **Security** | S11, S16, S20, S22. The model is pinned and its code reviewed. The service has no egress. |
| **Performance** | Latency budget to be added to `perf/budgets.toml` before the service is enabled |
| **Gate** | `just verify-12` |

## Goal

Build a nightly, server-side map of which settings reference which hosts, databases, queues and services. It upgrades check C6 and answers "who references X?" in Cursor.

## Build

1. **Deterministic extraction first, in the engine.** This pass extracts:
   - URLs;
   - host:port pairs;
   - connection strings;
   - cluster-local service names, such as `core-adapter-symxchange`.

   In the real data, most references are cluster-local service names. Parsing catches these.
2. **GLiNER service** on CPU.
   - It receives only the values the deterministic pass couldn't parse.
   - Pin the model and its revision. Review any remote code before enabling it.
   - Measure latency on real samples first.
3. **Reference table** in Postgres, rebuilt nightly and updated incrementally.
4. **Access:** a REST endpoint and an MCP tool, `who_references(entity)`. Adding these is a contract change and needs its own PR.

## Acceptance criteria

- [ ] Fixture tests cover the deterministic extractor.
- [ ] Latency and accuracy are measured on 200 real values before the service is enabled.
