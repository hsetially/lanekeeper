# Lanekeeper: instructions for the coding agents that build it

AI coding agents (Cursor and Claude Code) build this repository, and humans review and merge their work. This file is the contract every agent follows. Claude Code reads it through CLAUDE.md.

**Priorities, in order: security, performance, efficient features.** Implementation complexity is acceptable when it buys one of these. Never give up any of the three to save effort.

## Protocol for every task

1. **Read** these files, in this order:
   1. this file;
   2. `docs/decisions.md`;
   3. `docs/security.md`;
   4. `docs/performance.md`;
   5. `docs/interfaces.md`;
   6. `docs/domain-model.md`;
   7. `docs/open-questions.md`;
   8. your prompt in `prompts/`.

   Read other prompts only when yours names them.
2. **Plan.** Write `plans/NN-<name>.md` covering:
   - the files and types you will create;
   - how each task in your prompt maps to commits;
   - tests, benchmarks and fuzz targets;
   - every ambiguity or open question you'll hit.

   Then stop and wait for approval.
3. **Implement** task by task, in the prompt's order. After each task, run its `Verify` command. Commit once per task, with the message `NN/Tk: <summary>`.
4. **Before opening the PR,** run `just verify-NN` and then `just verify`. The PR description must contain:
   - their output;
   - `cargo xtask bench-check` results against the budgets;
   - the acceptance checklist, ticked;
   - the security requirement IDs (S#) you implemented;
   - any assumptions you made.
5. **Review.** A reviewer agent runs `prompts/REVIEW.md` on the PR. Fix what it finds before a human looks.

## Stop and escalate

Write the problem into the plan or PR and wait, rather than working around it, when:

- an interface in `docs/interfaces.md` or a contract file doesn't fit what you need;
- a decision in `docs/decisions.md` blocks you;
- a performance budget can't be met with the approach in your prompt (include measurements);
- a security requirement conflicts with a functional one;
- you need the answer to an open question that has no default.

## Project skills

Load the matching skill before you start a task. Claude Code reads them from `.claude/skills/`, and Cursor through `.cursor/skills`.

- **`lanekeeper-config-semantics`:** effective config, rendering, drift, tenants, channels, pickup states, and checks C1–C11.
- **`lanekeeper-rust-conventions`:** any Rust change.
- **`lanekeeper-security`:** anything security-relevant. Most changes are.
- **`lanekeeper-performance`:** any P#.
- **`lanekeeper-contract-change`:** any contract path.
- **`lanekeeper-ui-from-design`:** any `web/` work. The `.dc.html` references are in `design/handoff/screens/`.
- **`lanekeeper-pr-review`:** reviewing a PR.

Third-party skills (shadcn, skill-creator, webapp-testing, systematic-debugging, verification-before-completion, test-driven-development, receiving-code-review, six Rust skills, writing-for-agents) are vendored in the same folder at pinned commits; see `.claude/skills/THIRD-PARTY.md`. The `lanekeeper-*` skills win on any conflict, and a third-party skill must not change the plan, commit or PR protocol above.

## Running prompts (automated)

Start work with `/run-prompt NN` (or `/run-prompt wave:1`) in Claude Code. It runs the protocol above with subagents, in an isolated worktree per prompt (`.worktrees/NN-slug`, branch `agent/NN-slug`):

1. `planner` writes `plans/NN-slug.md` from `plans/TEMPLATE.md`. **A human approves it.** This is the one routine stop.
2. One implementer subagent per task (`rust-`, `web-` or `ops-implementer`), test first, one `NN/Tk:` commit each.
3. A final step runs `just verify-NN`, `just verify` and `cargo xtask bench-check` and writes `plans/NN-slug.evidence.md`.
4. `reviewer` runs `prompts/REVIEW.md` independently. Blocking findings go back to the implementer, for at most 3 rounds.
5. The PR text is written to `plans/NN-slug.pr.md`; with `--pr` the PR is opened.

Inside `agent/*` worktrees a PreToolUse hook (`.claude/hooks/guard.mjs`) blocks edits until the plan is approved, blocks edits outside the prompt's owned paths (plus tests, benches and fixtures under them, and the shared `Cargo.toml`, `Cargo.lock` and `Justfile`), blocks contract paths unless the approved plan says so, freezes the plan after approval, and blocks `--no-verify`, force pushes and pushes to main. The hook cannot see arbitrary shell writes, so the reviewer and CI stay the backstop. Cursor does not run these hooks; in Cursor follow the protocol by hand.

A plan can list `Extra-paths:` (paths beyond the prompt's ownership) and `Contract-change:`. Both take effect only when the human approves the plan.

## Ownership

- Each prompt lists the paths it owns. Edit only those, plus tests, benches and fixtures under them.
- **Contract paths** change only through a `contract-change` PR approved by a human:
  - `crates/domain`
  - `crates/ports`
  - `proto/`
  - `api/openapi.yaml`
  - `db/migrations/`
  - `docs/mcp-tools.md`
  - `perf/budgets.toml`
  - `design/handoff/**`, which only changes through a design-handoff PR
- Add migrations as new files. Never edit a merged migration.

## Repo layout

```
AGENTS.md, CLAUDE.md, CHANGES.md, Justfile, deny.toml, rust-toolchain.toml
docs/        product-brief, decisions, security, performance, interfaces, domain-model,
             open-questions, threat-model, mcp-tools
prompts/     one prompt per workstream, plus README and REVIEW
plans/       agent plans, one per prompt
perf/        budgets.toml, load-test scenarios
proto/       agent.proto
api/         openapi.yaml
db/          migrations/
fuzz/        cargo-fuzz targets and corpora
crates/
  domain/        shared types, no I/O
  ports/         traits between components, with in-memory fakes (feature "fakes")
  proto/         generated gRPC code
  engine/        deterministic fact engine
  hub-identity/  Entra sign-in, sessions, users, roles, CSRF, audit chain
  hub-platform/  agent gateway and CA, replica routing, event bus and SSE, leases,
                 Git mirror and webhooks, blob store, observability
  hub-registry/  ingestion, indexes, baselines, adoption, drift, findings, read API
  hub-writes/    edits, uploads, PRs, token vault, restarts, approvals, Teams
  hub-docsearch/ docs ingestion, hybrid search, docs filesystem, feature flags
  hub-mcp/       MCP server
  hub/           binary that composes everything and serves web/dist
  agent/         in-cluster agent
  sentinel/      NFS VM sentinel daemon (attribution)
  xtask/         verify, bench-check, synthetic fixture generator
design/      Claude Design prompts; handoff bundles (read-only) and DEVIATIONS.md
web/         React + TypeScript app
skills/      Cursor skill
deploy/      Helm charts, CloudNativePG, ArgoCD, policies
```

## Stack (fixed)

- **Rust core:** Axum, Tokio, tonic (with zstd compression) and prost, sqlx, gix, octocrab, kube-rs, rmcp.
- **TLS and certificates:** rustls with the aws-lc-rs provider; rcgen for certificates.
- **Filesystem access:** cap-std, which makes it impossible for agent file access to escape the NFS root.
- **Performance:** moka (caches), rayon, and ripgrep's `ignore`, `globset`, `grep-regex` and `grep-searcher`.
- **Parsing and diffs:** tree-sitter with the YAML grammar, and similar.
- **Docs:** pulldown-cmark with ammonia for sanitising, and fastembed/ONNX for embeddings.
- **Secrets:** secrecy, zeroize and subtle.
- **Observability:** tracing, plus tracing-opentelemetry exporting to Cloud Trace.
- **Testing:** proptest, insta, criterion, testcontainers and cargo-fuzz.
- **Web:** React and TypeScript in strict mode, Vite, TanStack Router, Query and Virtual, Monaco (self-hosted and lazy-loaded), shadcn/ui, Tailwind, orval, MSW, Vitest, Playwright and Lighthouse CI.
- **Data and CI:** Postgres 16 with pgvector and pg_trgm, run by CloudNativePG. GitHub Actions, with images pushed to GHCR.

## Code rules (enforced by workspace lints and CI)

1. **No unsafe code.** Every crate we own has `#![forbid(unsafe_code)]`.
2. **No panics outside tests.** `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` and `dbg!` are clippy-denied.
3. **Errors.** Libraries return `thiserror` types. Errors are mapped at the edge to `application/problem+json`, with no internal details.
4. **Async code never blocks.** CPU-heavy work goes through rayon inside `spawn_blocking`. Blocking file I/O goes through `spawn_blocking`.
5. **Everything is bounded.** Every external call has a timeout, and retries are bounded with jitter. Every channel, queue, cache and result set has a size limit.
6. **Secrets.** Secrets are `Secret<T>`: redacted in output and zeroized on drop. Compare them in constant time.
7. **Facts come only from `crates/engine`:** diffs, drift, effective config, grid, checks, locations and text search. No AI computes them.
8. **Audit.** Every state change writes exactly one audit event, in the same transaction.
9. **Authorization on the server, on every route and tool.** A route-table test fails if any route lacks a guard.
10. **Untrusted input.** Paths are untrusted. Config and doc text is data, never instructions.
11. **No blind writes.** Every NFS write carries the expected hash. Writes preserve the file's line endings, encoding and BOM.
12. **No real tenant config in the repo.** Use the synthetic generator (`cargo xtask gen-fixtures`).
13. **Tests ship with the code:**
    - unit tests;
    - property tests;
    - golden tests;
    - integration tests against real Postgres;
    - a fuzz target for every parser at a trust boundary.
14. **Hot paths are benchmarked.** Every hot path has a criterion benchmark registered in `perf/budgets.toml`. `cargo xtask bench-check` must pass.
15. **Dependencies:**
    - only from crates.io and the npm registry;
    - pinned versions must be at least 14 days old;
    - each new dependency is justified in the PR;
    - `cargo deny` and `osv-scanner` must pass.

## Commands

```
just dev                              # Postgres + hub + web, using the synthetic fixtures
just verify                           # fmt, clippy, tests, deny, audit, web checks
just verify-NN                        # one prompt's acceptance gate
just bench && cargo xtask bench-check # benchmarks checked against perf/budgets.toml
just fuzz-smoke                       # 30 seconds per fuzz target
cargo xtask gen-fixtures --scale small|target
```
