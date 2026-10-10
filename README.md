# Lanekeeper

Lanekeeper is an internal tool for seeing, comparing, changing and auditing the config files behind every swimlane. AI coding agents build it, using Cursor and Claude Code, and humans review and merge their work.

## Start here

1. **Read `AGENTS.md`.** It's the contract every agent follows. Claude Code loads it through `CLAUDE.md`, and Cursor reads it directly.
2. **Read `prompts/README.md`** for the waves, the human gates, and how to start an agent.
3. **Put the design screens** (`*.dc.html`) in `design/handoff/screens/`, following the README there, and commit them as `design: handoff v1`.
4. **Run prompt 01 first:** `/run-prompt 01` in Claude Code. It plans, waits for your approval, implements task by task with subagents, runs the gates and has a reviewer agent check it. Then run wave 1 in parallel with `/run-prompt wave:1`. See "Running prompts (automated)" in `AGENTS.md`.

## What's here

| Path | Contents |
|---|---|
| `docs/` | Product brief, decisions (D1–D88), security (S1–S25), performance budgets (P1–P15), interfaces, domain model, open questions, threat model |
| `prompts/` | 18 work orders for agents, `README.md` (waves) and `REVIEW.md` (reviewer agent) |
| `design/` | Claude Design prompts 00–14, `handoff/screens/` for the `.dc.html` references, and `DEVIATIONS.md` |
| `.claude/skills/` | Project skills for the coding agents, loaded automatically by Claude Code. `.cursor/skills` links to the same folder. |
| `skills/lanekeeper/` | The end-user Cursor skill (a draft; prompt 11 finalises it) |
| `crates/` | Rust workspace stubs. Each crate names its owning prompt. |
| `web/` | React + TypeScript app skeleton (Vite, strict TS, CSP-safe lint, 14-day package-age rule) |
| `proto/`, `api/`, `db/migrations/`, `perf/budgets.toml` | Contract stubs that prompt 01 fills in |
| `deploy/` | Local dev Postgres (pgvector + pg_trgm), plus placeholders for the charts and sentinel packaging |
| `.github/workflows/ci.yml` | CI, with actions pinned by commit SHA |

## Project skills (for agents)

| Skill | Use it for |
|---|---|
| `lanekeeper-config-semantics` | Anything about effective config, rendering, drift, tenants, channels, pickup states, or checks C1–C11 |
| `lanekeeper-rust-conventions` | Any Rust change: ports, errors, async rules, sqlx, audit plus outbox, tests |
| `lanekeeper-security` | Auth, secrets, paths, input, HTML, MCP output, charts, CI, dependencies, threat model |
| `lanekeeper-performance` | Any P#: benchmarks at design scale, `budgets.toml`, profiling, escalation |
| `lanekeeper-contract-change` | Changing proto, OpenAPI, migrations, ports, domain, MCP tools or budgets |
| `lanekeeper-ui-from-design` | Building screens from `design/handoff/screens/*.dc.html` |
| `lanekeeper-pr-review` | The reviewer agent's procedure and verdict format |

## Local commands

```
just --list
docker compose -f deploy/dev/compose.yaml up -d
cd web && pnpm install && pnpm dev
cargo build --workspace     # the Rust toolchain is pinned in rust-toolchain.toml
```

## Tools the gates need

`just verify` and `just verify-NN` use these tools. `just tools-check` fails with the install command when one is missing or has the wrong version.

| Tool | Install | Used for |
|---|---|---|
| Rust 1.88.0 | `rustup` reads `rust-toolchain.toml` | build, fmt, clippy, tests |
| `just` | `cargo install --locked just` | task runner |
| `cargo-deny`, `cargo-audit` | `cargo install --locked cargo-deny --version 0.19.9`, `cargo install --locked cargo-audit --version 0.22.2` | S19 licences, bans, sources and advisories |
| `buf`, `redocly` | `pnpm install --frozen-lockfile` in the repo root (versions are pinned in `package.json` and `pnpm-lock.yaml`) | lint `proto/` and `api/openapi.yaml` |
| pnpm 10 and Node 22 | see `web/.nvmrc` | the root tools and `web/` |
| Docker | Docker Desktop or Docker Engine | the dev database, and the database tests (set `LK_REQUIRE_DOCKER=1` to fail instead of skip when it is missing) |
| `git` | any recent version | the fixture generator builds Git repos with `git fast-import` |
| `cargo-fuzz` and a pinned nightly | see `fuzz/README.md` | `just fuzz-smoke` |

On a Windows machine, test through WSL2: the agent crate cannot compile natively (Unix-only file metadata through cap-std), and the gates are proven on Linux. See [`docs/windows-testing.md`](docs/windows-testing.md) for the setup that was proven end to end.

## Before the pilot goes live

Answer the seven pilot-blocking questions in `docs/open-questions.md`: Q1, Q2, Q11, Q17, Q21, Q24 and Q26.
