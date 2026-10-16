# 07: MCP server for Cursor 3.22.12

> **Two gates are still open and nothing in this branch can close them.** Please do not merge until both are answered.
>
> 1. **A Docker-capable run.** The sandbox that built this branch has no Docker daemon, so `just verify-07` stops at `docker-check` and `just verify` was never run as one command. CI or your machine has to run `just verify-07`, `just verify` and `just bench && cargo xtask bench-check` and attach the output. Pgvector-dependent checks, the libFuzzer campaigns and `osv-scanner` belong to the same run.
> 2. **T0 on Cursor 3.22.12 (Windows and macOS) is NOT RUN.** Entra OAuth with the fixed client ID, elicitation accept/decline/cancel and the tool allowlist have not been tried against a real Cursor (`just mcp-t0-status` prints `overall: NOT RUN`, 0 of 25 cells per OS). Either record the results in D91 (`crates/hub-mcp/t0/README.md` is the runbook) or waive T0 for the merge in a PR comment. **`just mcp-t0-status --strict` is a go-live gate either way**, because `verify-07` stays green with T0 unrun.

The library is complete and reviewed, but **the hub does not mount `/mcp` yet** (`McpModule` is built nowhere but `crates/hub/src/mcp.rs`; a test pins that). Merging this exposes nothing.

## Prompt

`prompts/07-mcp-server.md` · plan: `plans/07-mcp-server.md` (approved by a human, Contract-change approved) · wave: 2

The contract this depends on was merged earlier by two separate PRs: hsetially/lanekeeper#13 (tools, personal tokens, port gaps, P9 budgets as unregistered entries) and hsetially/lanekeeper#14 (review fixes, migration 0006). This branch has `main` merged in four times (no rebase); the last merge brought in prompt 06 (hsetially/lanekeeper#15, `crates/hub-writes`) and touched `Cargo.lock`, `Justfile` and `docs/threat-model.md` (both modules' sections kept whole, 06 first). The only contract path touched here is `perf/budgets.toml`: the 21 `P9` entries flip from `registered = false` to `registered = true`, no threshold changed (proof: `git diff --name-only origin/main..HEAD` against the contract paths of AGENTS.md prints only that file). `skills/` and the plan file were not edited.

An MCP server inside the hub (rmcp, Streamable HTTP at `/mcp`): 27 tools with the web UI's permissions, server-enforced human confirmation for every side effect, compact data-labelled output within P9, personal-token and OAuth modes, a Cursor plugin package and admin guide, and a threat model.

## Gates

Run one by one in a sandbox without Docker (details and raw tails: `plans/07-mcp-server.evidence.md`). A test that prints SKIPPED is never counted as passed.

| Gate | Result |
|---|---|
| `just verify-07` as one command | **FAILED at `docker-check`** (no Docker daemon). Sub-recipes run by hand below. |
| `just verify` as one command | **NOT RUN.** Its steps: fmt, clippy `--workspace`, `cargo deny`, `cargo audit`, `just web-verify` passed; `cargo test` ran per crate (below). |
| `cargo fmt --all -- --check`; clippy `-D warnings` for `hub-mcp`, `hub` (and `--workspace` in the FINAL step) | passed |
| `cargo test -p hub-mcp --lib --tests`, against a real PostgreSQL 16 (no pgvector) | **505 passed, 0 failed, 0 SKIPPED** (reviewer re-ran it, round 3). The 11 token-store tests ran on the real server. |
| `cargo test -p hub --test mcp_module` | 5 passed |
| `cargo test -p xtask --test mcp_tools_doc --test budgets_registry` | 23 and 8 passed |
| After the last merge of `main` (06): `cargo check --workspace --all-targets --locked`; `cargo test -p hub-mcp --test threat_model`, `-p hub-writes --test threat_model`, `-p xtask --test budgets_registry --test mcp_tools_doc` | compiled; 11, 11, 8 and 23 passed. **The 505-test run, clippy, deny and audit above are from before this merge and were not repeated** (the merge changed no file of `crates/hub-mcp`). |
| `cargo deny check`, `cargo audit` (workspace and the hub-mcp fuzz crate) | passed |
| `just mcp-fuzz` (7 targets, 30 s each, FINAL step) | no crash; `mcp_elicitation_result` now runs through the recipe. Not repeated after the review rounds, where no fuzz target changed except the `mcp_pat_token` invariants file (seed replay passes). |
| `just mcp-bench` / P9 | **21 of 21 pass**, measured on `f67bf80`; the last production commit (`dc299b0`) was not re-measured (it touches a summary sentence, a question signature and test code). Re-run in the Docker-capable run. |
| `cargo xtask bench-check` (full registry) | 21 pass, 25 fail, 26 unmet of 72 at the FINAL step. All 51 other rows belong to other prompts (benchmarks never run here, same on the base commit). |
| Other crates' `cargo test` (one at a time) | all passed except `xtask` `db_migrations::folder_listings_use_the_prefix_indexes`, a planner/collation difference of the sandbox's Postgres that **also fails on `origin/main`**. `hub-platform`: 161 of 512 tests printed SKIPPED (need Docker or pgvector), `hub`: 1 SKIPPED. 53 `db_migrations` tests print "pgvector checks NOT PROVEN". |
| `osv-scanner` | **NOT RUN** (not installed) |
| Flaky-test proof for the log tests (fixed in review round 2) | filter `the_` (133 tests) 0 of 30 failed, whole `mcp_auth` binary 0 of 6; the reviewer: 0 of 20 and 0 of 5 |

## Acceptance checklist

- [ ] **T0 results are recorded and the modes are set in config.** NOT RUN (human part), see the top of this description. What is proved and does not tick it: the modes exist and default as D91 says; the server side of the T0 stub runs in process (`stub_protocol::*`, 24 tests).
- [ ] **`just verify-07` passes.** Not proved: stops at `docker-check` here.
- [x] **P9 is within budget for every read tool.** `just mcp-bench`: 21 of 21, every p95 far below 300 ms (table below). These are this layer's share on fakes at target scale; prompt 16 proves the end-to-end path.
- [x] **All authentication, confirmation and role tests pass.** 505 passed (above), personal-token store on real Postgres.

## Security requirements implemented (S#)

S4, S7, S9, S11, S14, S14b, S15, S21, and S3 **only partly** (below). The trace from each requirement to code and named tests is in `plans/07-mcp-server.evidence.md` and in the new `## MCP server (07)` section of `docs/threat-model.md`, which a test (`threat_model.rs`) checks against the code. In short:

- **S4:** every route and tool has a declared guard or an explicit allowance; default deny under `/mcp`; role per call; pending and disabled users refused (`route_table::*`, `tool_matrix::*`, `auth_oauth::*`, hub `mcp_module::*`).
- **S7, S9:** personal tokens `lkpat_` plus 256 random bits, stored only as HMAC-SHA-256, 7 days, revocable, one audit event in the same transaction (`auth_pat::*`, `auth_pat_pg::*`).
- **S11:** typed inputs through the domain parsers, bounded sizes, fuzz targets `mcp_tool_args`, `mcp_cursor`, `mcp_bearer_header`, `mcp_http_body`.
- **S14:** token and user checks on every request, server-enforced elicitation with an exact answer, the confirmation gate as the only caller of mutating write methods (a whole-file source scan with positive controls), per-user rate limits, session slots that survive a dropped request, every port wait bounded in time.
- **S14b, S15:** every outgoing text goes through one redaction type and a final sweep that fails closed; denied files are withheld in every tool; file and doc text is labelled `untrusted` (delimited blocks exist only in `data_block` mode, see below); the `raise_pr` question shows the title and a scanned, escaped, quoted start of the description.
- **S21:** `#![forbid(unsafe_code)]`, no panics outside tests, fixed-text errors, logs carry the tool, outcome, timing, stable user id and request id and nothing else (`logging::*`, with a positive control).

## Benchmarks against budgets (P#)

P9, budget p95 <= 300 ms each, 4 shared vCPU, release build, caches off for `read_file`, the real server in process over fakes at target scale. p95 in ms, run on `f67bf80`:

| Tool | p95 | Tool | p95 |
|---|---|---|---|
| rotation over 20 tools (`P9`) | 4.4 | `search_settings` | 0.8 |
| `list_swimlanes` | 0.5 | `search_text` | 1.5 |
| `get_swimlane` | 0.6 | `find_inconsistencies` | 0.9 |
| `list_files` | 0.8 | `get_drift` | 0.9 |
| **`read_file` (cold, largest file)** | **47.6** | `get_pending_restarts` | 0.8 |
| `get_effective_config` | 1.1 | `get_history` | 0.6 |
| `compare` | 1.2 | `search_docs` | 0.7 |
| `compare_tree` | 0.5 | `read_doc` | 2.4 |
| `compare_grid` | 5.1 | `grep_docs` | 1.3 |
| `get_docs_for_file` | 0.8 | `get_feature_status` | 1.6 |
| `get_served_config` | 2.4 | | |

Cold `read_file` varied between 46 and 63 ms across runs on the shared machine. The plan's internal 50 ms screen does not apply to it (D92): its cost is `engine::index_settings`, whose own accepted budget is 60 ms, paid once per file version per replica; the registered 300 ms budget passes and warm calls take about 2 ms. No threshold in `perf/budgets.toml` changed.

## New dependencies and why

`Cargo.lock` gains 23 packages, none changed. Runtime: `rmcp` =3.5.0 (the MCP SDK of the fixed stack; features `server`, `transport-streamable-http-server`, `elicitation` only; 17 days old, 3.5.1 refused for its age) with `pastey`, `schemars_derive`, `serde_derive_internals`, `sse-stream`. Dev-only: `jsonschema` =0.58.3 (validates tool inputs and results against the contract's schemas with a validator that is not ours) and its transitive crates. A new lockfile for the hub-mcp fuzz crate. All at least 15 days old at the time of the check; two lockfile entries that had resolved to 9-day-old versions were pinned back (`07/fix: pin ...`). `cargo deny` and `cargo audit` pass; `osv-scanner` NOT RUN. Full table with ages and licences in the evidence file.

## Assumptions, decisions and follow-ups

**Accepted by a human, recorded, to be revisited** (D109 and D92 in `docs/decisions.md`; the D109 text names the human, hsetially, and the date, but git shows the agent as the commit author, so **your review of that diff is the record**):

- **D-1:** `/mcp` is a self-authenticating `public` route instead of `Role(Viewer)` as the plan says, because `GuardedRouter` cannot add the `WWW-Authenticate` challenge to its own 401 (Cursor's OAuth discovery needs it). AGENTS.md rule 9 is met by an explicit allowance plus named route-table tests. Reversal: a three-line `GuardedRouter::layer` in `hub-platform`; only `server/guard.rs` changes.
- **D-2:** S3 (CSRF) for the personal-token routes is met by the exact `Origin` check only, not the double-submit token, because the hub composes no CSRF layer for any module yet. Condition: the hub composition wraps every non-GET route, `/api/v1/me/mcp-tokens` included, in 03a's `CsrfLayer` and `crates/hub/tests/mcp_module.rs` then asserts it. The reviewer agreed it does not block merge, since nothing is mounted.
- **D92a:** cold `read_file` p95 about 52 to 63 ms against the plan's 50 ms internal screen (above). The clean fix is an additive `RegistryRead` outline served from 05's settings index.
- **D92b:** `get_effective_config` returns `text` null and typed values `other` until a later contract PR adds the rendered resource-file text and a typed value (the summary points to `get_served_config`, which does return the rendered body). The same PR can settle where the hash of a denied served file comes from (prompt 05; `ServedFile.hash` is required, the agent answers a denied fetch with a bare `DENIED`).

**Choices the plan did not fix:** draft mode and a client without the form capability do not check that the draft exists; after the answer the gate re-reads the user and refuses if role, status or `requires_approval` changed; tools that ask get a 185 s deadline; the replay map stores only successful writes; denied-file outcomes (`unprocessable` with reason `denied` for `get_effective_config` and `compare_grid`, `class: denied` for `compare`, search hits in denied files dropped, write tools refuse a denied path before drafting, `notify_services` does not refuse one because only a name is sent); the cached scan lives in `tools/cache.rs`, not `output/scan_cache.rs`; the proposal summary says "Nothing was done yet."; `get_served_config` shows at most 200 lines as the contract says; `tools/list` answers "unavailable" (was "not allowed") when the user directory cannot answer; the `raise_pr` question shows the title and at most 240 characters of the description (scanned, escaped, quoted), the rest is posted unread under the user's GitHub identity; `quote()` escapes `< > & [ ] \`` for every quoted field.

**For other prompts and humans:**

- **Decision numbers collide with `origin/agent/05-registry-read-api`**, which uses its own D91, D92, D107 and D108 for other decisions (and main has a different D107). Whoever merges second renumbers; `examples/t0_status.rs` parses the literal `- **D91**` heading and about 25 files name D91, D92 and D109 here.
- Prompt 09: `Mcp-Session-Id` affinity (sessions live in one replica's memory and fail closed), or deployers set `confirm_mode = draft`.
- Prompt 06 (merged): it caps open drafts at 100 per author (`MAX_OPEN_DRAFTS` in `crates/hub-writes/src/drafts.rs`), which bounds `propose_change`; a draft is still up to 2 MiB and created at the write rate limit, so the byte total per author is 200 MiB. Nobody has checked that `crates/hub-writes` and this crate's gate agree on every error and result shape; the hub integration must run the two together.
- Hub integration: mount `/mcp`, compose 03a's `CsrfLayer` over the token routes (go-live checklist item next to `mcp-t0-status --strict`), give `tools.notify_services.enabled` an environment key (it ships off and cannot be switched on today), fix the rmcp allowed-origin-without-port warning (`:*` or an explicit port) before any rmcp bump.
- Decide with the T0 results: the content mode (delimited blocks only exist in `data_block` mode; the default `summary` labels text `untrusted` without delimiting it) and the `advertise_output_schemas` default (`tools/list` is 112 KB with result schemas, 31 KB without).
- Q20 (six real files for scan tuning) blocks go-live. The Cursor plugin format is UNVERIFIED (14 marked items in `plugin/README.md` and `docs/cursor-admin.md`). The package copies the dev-only `evals/evals.json` of prompt 11's skill, which is still a draft.
- Plan traceability: `docs_tree::read_doc_takes_only_doc_paths` and `docs_tree::dotdot_and_backslash_are_refused` do not exist under those names (the cases are in `args::every_path_argument_uses_a_domain_parser`), and the goldens of the 401 challenge, `tools/list` per role and the cursor are inline assertions. The evidence file's per-crate rows still say 482 tests in places (round 1); 505 is current.
- Follow-ups from the final review (non-blocking): a type-level narrow write handle for the gate (`ToolHost.writes` is still public); `RedactedText::window` doc overclaims for a lone `\r`; a source rule against a future direct `Users::require_active` call; per-user limits cover `tools/call` only, the threat-model wording should say so; two residual session leaks bounded by the 30-minute idle expiry; `crates/hub/tests/telemetry.rs` still uses a scoped `set_default` subscriber (outside 07's paths).
- Out of scope, unchanged: `docs/interfaces.md` does not mention the new port methods; the stale "only Operator" wording of the notify tool in `.claude/skills/lanekeeper-security/SKILL.md`, D85 and `skills/lanekeeper/SKILL.md` (see hsetially/lanekeeper#14).

## Reviewer agent verdict

Three rounds (`plans/07-mcp-server.review.md`):

- Round 1: CHANGES REQUESTED, no code defect; blocking items were the missing Docker run, T0 NOT RUN, and D-1/D-2 missing from `docs/decisions.md` (recorded as D109).
- Round 2: CHANGES REQUESTED, two reproduced defects: a session slot leaked when a request was dropped (and the rmcp session it had made stayed open), and the log proofs were flaky and could pass vacuously. Both fixed with tests that failed first, plus port-call timeouts, a bounded `get_served_config`, and small fixes.
- **Round 3: `VERDICT: APPROVE`, no blocking finding.** The reviewer re-ran fmt, clippy, 505 tests, the flaky filter 20 times (0 failures), `cargo deny` and `cargo audit`; mutation-checked the session-slot guard (five mutations, each caught); and judged D-1, D-2, D92a and D92b on their merits as non-blocking. The two open gates above were not counted against the branch by instruction; it found no place where the docs, evidence or threat model misstate them.

## Labels

`contract-change`: the plan's Contract-change was approved by a human (the 21 P9 entries in `perf/budgets.toml`).

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01D11j28mLCqHPFdCgZSSdf6
