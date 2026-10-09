# Lanekeeper task runner. Run `just --list` to see every task.
set shell := ["bash", "-euo", "pipefail", "-c"]

# Everything a PR must pass (AGENTS.md, rule 13)
verify: rust-verify web-verify

rust-verify:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo deny check
    cargo audit

web-verify:
    cd web && pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test --run && pnpm build

# Tests for the agent automation (scripts/lk.mjs and the guard hook)
lk-test:
    node --test scripts/lk.test.mjs

bench:
    cargo bench --workspace

fuzz-smoke:
    @echo "fuzz targets are added in prompt 01, T9" && exit 1

# Local dev: Postgres (pgvector + pg_trgm), hub, web
dev:
    docker compose -f deploy/dev/compose.yaml up -d
    @echo "hub and web dev servers are added by prompts 03b and 08"

# Per-prompt gates. Each owning prompt replaces its stub.
verify-01:
    @echo "verify-01 not implemented yet (prompts/01-contracts.md)" && exit 1
verify-02:
    @echo "verify-02 not implemented yet" && exit 1
verify-03a:
    @echo "verify-03a not implemented yet" && exit 1
verify-03b:
    @echo "verify-03b not implemented yet" && exit 1
verify-04:
    @echo "verify-04 not implemented yet" && exit 1
verify-05:
    @echo "verify-05 not implemented yet" && exit 1
verify-06:
    @echo "verify-06 not implemented yet" && exit 1
verify-07:
    @echo "verify-07 not implemented yet" && exit 1
verify-08:
    @echo "verify-08 not implemented yet" && exit 1
verify-09:
    @echo "verify-09 not implemented yet" && exit 1
verify-11:
    @echo "verify-11 not implemented yet" && exit 1
verify-14:
    @echo "verify-14 not implemented yet" && exit 1
verify-15:
    @echo "verify-15 not implemented yet" && exit 1
verify-16:
    @echo "verify-16 not implemented yet" && exit 1
verify-17:
    @echo "verify-17 not implemented yet" && exit 1
