# Lanekeeper task runner. Run `just --list` to see every task.
set shell := ["bash", "-euo", "pipefail", "-c"]

# Pinned versions of the tools that are not in a lockfile (S19). Install with:
#   cargo install --locked cargo-deny --version {{cargo_deny_version}}
#   cargo install --locked cargo-audit --version {{cargo_audit_version}}
# buf and redocly come from the root package.json and pnpm-lock.yaml (run `pnpm install --frozen-lockfile` in the repo root).
cargo_deny_version := "0.19.9"
cargo_audit_version := "0.22.2"

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

# 30 seconds per fuzz target. Replaced by T9 of prompt 01; until then it fails, so verify-01 stays red.
fuzz-smoke:
    @echo "fuzz targets are added in prompt 01, T9" && exit 1

# Local dev: Postgres (pgvector + pg_trgm), hub, web
dev:
    docker compose -f deploy/dev/compose.yaml up -d --wait
    @echo "hub and web dev servers are added by prompts 03b and 08"

dev-down:
    docker compose -f deploy/dev/compose.yaml down

# Fail early, with the install command, when a pinned gate tool is missing or has the wrong version.
tools-check:
    @cargo deny --version | grep -qx 'cargo-deny {{cargo_deny_version}}' || { echo "need cargo-deny {{cargo_deny_version}}: cargo install --locked cargo-deny --version {{cargo_deny_version}}"; exit 1; }
    @cargo audit --version | grep -qx 'cargo-audit-audit {{cargo_audit_version}}' || { echo "need cargo-audit {{cargo_audit_version}}: cargo install --locked cargo-audit --version {{cargo_audit_version}}"; exit 1; }
    pnpm install --frozen-lockfile
    pnpm exec buf --version
    pnpm exec redocly --version

# Prompt 01 gate. Each task of prompt 01 adds its own recipe to this list; it is green only after T9.
verify-01: tools-check domain-verify ports-verify proto-verify openapi-verify db-verify fuzz-smoke

# T2: domain types, validators and secrets (S11, S21)
domain-verify:
    cargo test -p domain

# T3: ports, fakes and conformance suites (S4, S21). All features, so the Postgres `Tx` is compiled too.
ports-verify:
    cargo test -p ports --all-features
    cargo clippy -p ports --all-features --all-targets -- -D warnings

# T4: agent.proto lints with buf, the generated code builds without warnings, round trips and zstd (S5, S11)
proto-verify:
    pnpm exec buf lint proto
    cargo test -p proto
    cargo clippy -p proto --all-targets -- -D warnings

# T5: api/openapi.yaml passes redocly (every operation has x-required-role, problem+json errors, operationIds) and the
# conventions test (CSRF, Idempotency-Key, keyset lists, ETag, no agent join-token endpoint; S3, S4, S7, S21)
openapi-verify:
    pnpm exec redocly lint api/openapi.yaml --config .redocly.yaml
    cargo test -p ports --all-features --test openapi_conventions
    cargo clippy -p ports --all-features --all-targets -- -D warnings

# T6: db/migrations/0001_init.sql on Postgres 16 with pgvector (testcontainers, image pinned by digest) and the
# database-role tests (S9, S21). LK_REQUIRE_DOCKER=1 turns a missing Docker daemon into a failure instead of a skip.
db-verify:
    LK_REQUIRE_DOCKER=1 cargo test -p xtask --test db_migrations
    cargo clippy -p xtask --all-targets -- -D warnings

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
