# 07 blockers

## T2: the cold `read_file` of the largest file is over the 50 ms screen (p95 52 to 63 ms), and `get_effective_config` has two gaps on the ports

Everything else in T2 is done and green (see the T2 commit). Nothing was loosened: no threshold of `perf/budgets.toml` was changed, all 21 P9 entries are registered and pass their 300 ms budgets. The two items below are the ones that do not fit the plan; a human decides.

### 1. Cold `read_file` p95 is above 50 ms (plan A7, orchestrator rule)

Plan A7 and the T2 instruction say: if a read tool's p95 on fakes at target scale is above 50 ms, escalate with p50, p95, p99 and a profile, and do not raise a threshold. Registered budget: `P9.read_file`, p95 <= 300 ms. It passes (52.0 ms in the last `just mcp-bench`).

Benchmark: `crates/hub-mcp/benches/read_tools.rs`, `hub-mcp/read_file`. The real server in process (axum, gate, rmcp, role check, admission, typed arguments, tool, redaction, cap, SSE), over a fake registry that returns the 6,000-line CRLF YAML of swimlane 1 (a fake key planted near line 5,000). The caches are off (`cache_bytes = 0`), so every call is the first for that file version. Default view: the outline of the top-level settings plus the first 200 lines. 4 vCPU, shared (load average 1.5 to 1.8 from other sessions), release build.

| Run | p50 | p95 | p99 |
|---|---|---|---|
| first measurement, before the fix below | 53.1 ms | 70.8 ms | 72.7 ms |
| after the fix (full suite, run 1) | 39.5 ms | 63.1 ms | 64.4 ms |
| after the fix (`just mcp-bench`, final) | 34.9 ms | 52.1 ms | 58.7 ms |

The same call with the caches on (`read_file_warm`, the normal case: one scan and one index per file version): p50 1.57 ms, p95 1.87 ms, p99 1.89 ms. All other 19 read tools: p95 between 0.36 ms and 6.14 ms (`compare_grid` is the largest), p99 at most 6.92 ms. The rotation over all 20 tools, `hub-mcp/read_tools`: p50 3.5 ms, p95 4.65 ms, p99 5.16 ms.

Where the time goes (stage timings in a hot loop of the same bytes, then a callgrind profile; `perf` and `cargo flamegraph` are not installed in this sandbox, so the inclusive-cost table below stands in for the flamegraph):

| Stage of a cold `read_file` | Time |
|---|---|
| `ports::content_hash` (SHA-256 of 146 KB) | 0.10 ms |
| `engine::normalise_eol` | 0.32 ms |
| `engine::decode_text` | 0.14 ms |
| `engine::secret_scan` of the whole text | 0.9 to 1.0 ms |
| `engine::index_settings` | 31.8 to 37.3 ms (varies between runs of the same loop) |
| this crate's own work on the index (`IndexFacts`) | about 9 ms before the fix, about 0 after |
| the rest of the pipeline (window, redaction, cap, serialise, HTTP, SSE) | about 1.5 ms (the warm call) |

callgrind over three `index_settings` calls (inclusive): `index_settings` 97.4% of all instructions; inside it `ts_parser_parse` (tree-sitter YAML) 55.1%, `engine::parse::parse_within` (the Rust parser) 16.2%, `engine::parse::flatten` 8.3%, `Translator::run` (tree-sitter tree to the parser's shape) 9.8%. So the cold path is the engine's `index_settings`, which this crate calls and does not compute.

What was fixed in this crate: `IndexFacts::from_rows` took apart every one of the 6,000 rows (decode and encode of the path, a `HashMap` with a `String` key each) and cost about 9 ms. It now compares each path with the previous row's top-level key and takes a path apart only when the top-level key changes, and keeps the engine's `SettingPath`s without a second table. `tests/mcp_auth/index_facts.rs` checks that the outline and the lookup equal the old, obvious computation on the hard cases (a key that starts another, a dotted key, a later document, a sequence at the top, a `#` key). That took p50 from 53 to 35 to 40 ms. The remainder is the engine.

Why this cannot be closed in T2 without a decision: the engine's own accepted budget for the same function is `P6.index_settings`, p95 <= 60 ms (`plans/04-fact-engine.blockers.md`: 45.8 ms p95 measured on this kind of shared machine, threshold set by a human to 60 ms). The default `read_file` view of a large file needs the outline, which the plan builds from `engine::index_settings` (rule 7: locations come only from the engine). A p95 under 50 ms for the cold call is therefore not reachable by this crate whenever the engine's own p95 is above about 48 ms.

Context for the decision: it is paid once per file version per replica (the cache is keyed by the SHA-256 of the bytes and never goes stale), only for files over 16 KB or 200 lines, and the end-to-end budget is 300 ms. The 50 ms line in A7 was meant to leave the ports their share of 300 ms.

Options (each is a human decision; none is implemented):

- A. Accept it. Record in plan A7 / `docs/decisions.md` that the 50 ms screen does not apply to the cold default view of the largest file, because the engine's accepted budget for the index is 60 ms. No code change. Prompt 16 measures the real path.
- B. Plan change in T2/T6: return the first lines at once and the outline only when the index is already cached (or compute it after the answer for the next call). Cold p95 would be about 3 ms. The default view of a large file would lose its outline on the first call, which D58 and the prompt (T2: "an outline plus the first lines") ask for.
- C. Contract change (C4, `crates/ports` and `crates/domain`, additive): `RegistryRead` gains a method that returns the top-level locations of a file from the settings index that 05 already stores per unique blob (P6 indexes each blob once at ingestion). The MCP crate would then never parse at call time. This is the clean answer, and it touches 05.
- D. Make `engine::index_settings` faster (prompt 04): the tree-sitter pass is 55% of it.

### 2. `get_effective_config`: the ports cannot supply two things the contract describes

`docs/mcp-tools.md` says the result holds, for a resource file, the rendered body in `text`, and each setting's `value` is a `SettingValue` with `type` and `flag` (`yaml_boolean_like`). `domain::EffectiveConfig` has no rendered text (its comment says `settings` is "empty for resource files" and nothing replaces it), and `domain::EffectiveSetting.value` is a `String` with no type or flag. Both are in contract paths (`crates/domain`, `crates/ports`), so they are not mine to change.

What T2 does, isolated in `crates/hub-mcp/src/tools/read/effective.rs` (the one function, marked in a comment): `text` is always `null`; each setting is `type: "other"`, `flag: null`, `redacted` as the secret scan of this crate finds it (so S14b holds: a flagged value is redacted whatever the port says). The result validates against the contract's schema. It is honest (nothing is invented) but incomplete: a resource file's body is not returned, and `yes`/`on` are not flagged. `get_served_config` gives a resource file's served body.

Options: (a) an additive contract change that adds the rendered text and a typed value to `EffectiveConfig` (05 and 04 fill it); (b) accept `null`, `other` and `null` for now and say so in `docs/mcp-tools.md` (a contract wording change).

### 3. For information: found by the new fuzz target, fixed in T2

`mcp_http_body` found, in 45 seconds, that a second `initialize` on a live session (with a protocol version such as `4050-06-18`) rewrote what the session had negotiated (rmcp's own handler does the same) and left the session where `ping` is "method not found". Only the caller's own session was ever hurt. T2 refuses a repeated `initialize` (`invalid_request`, once per session, in `McpHandler::initialize`) and keeps the input as the seed `initialize_again_with_a_future_version`; test `sessions::a_second_initialize_is_refused_and_the_session_keeps_its_terms`.

## Resolution (decided by the human)

Both blockers of T2 are decided. The plan stays frozen; the decision is recorded as D92 in `docs/decisions.md`. No threshold in `perf/budgets.toml` changed, and nothing in `crates/domain` or `crates/ports` changed.

1. **Cold `read_file` of the largest file (option A, accept).** It measured p95 52 ms (runs of 52 to 63 ms). The plan's internal 50 ms screen (A7) does not apply to the cold default view of the largest file, because its cost is `engine::index_settings`, whose own accepted budget is `P6.index_settings` (60 ms). It is paid once per file version per replica, and only for files over 16 KB or 200 lines. The registered `P9.read_file` budget of 300 ms passes, and warm calls take about 1.9 ms. A later option, outside 07, is an additive `RegistryRead` outline method served from the index that 05 stores (option C).
2. **`get_effective_config` (option b, accept the gap for now).** It returns `text` null and settings with type `other` and flag null until a later contract PR adds the rendered resource-file text and a typed value to `EffectiveConfig`. That PR is to be done together with the open question of where the hash of a denied served file comes from (prompts 04 and 05). `get_served_config` gives the rendered body meanwhile. Secrets stay redacted either way (S14b).
