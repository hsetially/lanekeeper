# Lanekeeper task runner. Run `just --list` to see every task.
set shell := ["bash", "-euo", "pipefail", "-c"]

# Pinned versions of the tools that are not in a lockfile (S19). Install with:
#   cargo install --locked cargo-deny --version {{cargo_deny_version}}
#   cargo install --locked cargo-audit --version {{cargo_audit_version}}
# buf and redocly come from the root package.json and pnpm-lock.yaml (run `pnpm install --frozen-lockfile` in the repo root).
cargo_deny_version := "0.19.9"
cargo_audit_version := "0.22.2"
# Fuzzing (S22, plan 01 Q24). cargo-fuzz needs a nightly compiler for the sanitizer flags; the date is pinned and was
# at least 14 days old when chosen. The fuzz crate is outside the workspace, so the main toolchain stays on stable 1.88.
#   rustup toolchain install {{fuzz_nightly}} --profile minimal
#   cargo +{{fuzz_nightly}} install --locked cargo-fuzz --version {{cargo_fuzz_version}}   (the pinned 1.88 is too old to build it)
fuzz_nightly := "nightly-2026-09-14"
cargo_fuzz_version := "0.13.2"
# Override for a longer run, for example: just fuzz_seconds=600 fuzz-smoke
fuzz_seconds := "30"
fuzz_targets := "nfs_path compare_ref path_mapping_reverse"

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

# Fail early, with the install command, when the pinned nightly toolchain or cargo-fuzz is missing. Never skips.
fuzz-tools-check:
    @cargo +{{fuzz_nightly}} --version >/dev/null 2>&1 || { echo "need the {{fuzz_nightly}} toolchain for cargo-fuzz: rustup toolchain install {{fuzz_nightly}} --profile minimal"; exit 1; }
    @cargo fuzz --version 2>/dev/null | grep -qx 'cargo-fuzz {{cargo_fuzz_version}}' || { echo "need cargo-fuzz {{cargo_fuzz_version}}: cargo +{{fuzz_nightly}} install --locked cargo-fuzz --version {{cargo_fuzz_version}}"; exit 1; }
    @command -v clang >/dev/null 2>&1 || command -v cc >/dev/null 2>&1 || { echo "cargo-fuzz compiles libFuzzer, which needs a C++ compiler (clang or gcc/g++)"; exit 1; }

# Fail early when no Docker daemon is reachable. db-verify (Postgres 16 with pgvector in testcontainers) needs one.
docker-check:
    @docker info >/dev/null 2>&1 || { echo "verify-01 needs a running Docker daemon: db-verify starts Postgres 16 with pgvector through testcontainers"; exit 1; }

# T9: the fuzz crate's own lockfile is committed and complete, passes cargo deny and cargo audit (S19), then each target
# runs {{fuzz_seconds}} s on the pinned nightly. The committed seeds in fuzz/corpus/ are only read; new inputs go to
# target/fuzz-corpus/. A crash fails the recipe and leaves the input in fuzz/artifacts/. The same seeds also replay on
# stable in `cargo test -p domain` (crates/domain/tests/corpus_replay.rs).
fuzz-smoke: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path fuzz/Cargo.toml check --config deny.toml
    cargo audit --file fuzz/Cargo.lock
    for target in {{fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" "target/fuzz-corpus/$target" "fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "fuzz-smoke: {{fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

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

# Prompt 01 gate: every sub-recipe, in order, stopping at the first failure. The prerequisite checks come first so a
# missing tool or Docker daemon fails at once with the fix, never a silent skip. perf-verify includes the bench-check
# self-test and `cargo xtask bench-check`; run `just verify` after this gate (AGENTS.md, protocol step 4).
verify-01: tools-check fuzz-tools-check docker-check domain-verify ports-verify proto-verify openapi-verify db-verify mcp-doc-verify perf-verify fixtures-verify fuzz-smoke

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

# T7: docs/mcp-tools.md lint: the 25 tools, schemas that parse and resolve, bounded inputs, role floor against
# api/openapi.yaml, confirmation on the write tools, output caps (S4, S14, S14b, S15, D52, P9)
mcp-doc-verify:
    cargo test -p xtask --test mcp_tools_doc
    cargo clippy -p xtask --all-targets -- -D warnings

# T8: the budget registry (P1-P15) and bench-check (AC5). The planted over-budget value must be rejected, and the
# real registry is checked without --strict (an unregistered budget is a warning until its owning prompt registers it).
perf-verify: bench-check-selftest
    cargo test -p xtask --test budgets_registry --test bench_check
    cargo clippy -p xtask --all-targets -- -D warnings
    cargo xtask bench-check

# T8: bench-check must exit 1 on a planted over-budget value (budget 10 ms, committed samples 50 ms), not 0 and not a
# usage error.
bench-check-selftest:
    #!/usr/bin/env bash
    set -euo pipefail
    dir=crates/xtask/testdata/bench-planted
    code=0
    cargo xtask bench-check --budgets "$dir/budgets.toml" --criterion-dir "$dir/criterion" --results-dir "$dir/no-results" || code=$?
    if [ "$code" -ne 1 ]; then echo "bench-check-selftest: expected exit 1 on the planted over-budget value, got $code"; exit 1; fi
    echo "bench-check-selftest: the planted over-budget value was rejected"

# T8: deterministic fixtures (AC4). Two small-scale runs with the same seed must give the same manifest, NFS files and
# Git object ids; the target-scale check (40 swimlanes, about 80,000 files, 60 tenant branches) is #[ignore] and runs here.
fixtures-verify:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test -p xtask --test fixtures
    cargo clippy -p xtask --all-targets -- -D warnings
    a=target/fixtures/determinism-a
    b=target/fixtures/determinism-b
    cargo xtask gen-fixtures --scale small --out "$a"
    cargo xtask gen-fixtures --scale small --out "$b"
    cmp "$a/manifest.json" "$b/manifest.json"
    diff -r "$a/nfs" "$b/nfs"
    echo "fixtures-verify: two same-seed runs give an identical manifest (including every Git ref id) and identical NFS trees"
    cargo test -p xtask --test fixtures -- --ignored

# Prompt 02 gate (the agent): every sub-recipe, in order, stopping at the first failure. T9 to T12 append their own
# sub-recipes to this line. Run `just verify` and `cargo xtask bench-check` after it (AGENTS.md, protocol step 4).
# Not part of the gate, and said so rather than skipped silently: `docker build` of crates/agent/Dockerfile (needs a Docker
# daemon; the Dockerfile and HARDENING.md are linted by `agent-test`), and osv-scanner (not installed here).
verify-02: agent-tools-check agent-fixtures agent-lint agent-test agent-slow-test agent-fuzz agent-bench

# The pinned tools the gate needs, or the command that installs them. Never skips.
agent-tools-check: fuzz-tools-check
    @cargo deny --version | grep -qx 'cargo-deny {{cargo_deny_version}}' || { echo "need cargo-deny {{cargo_deny_version}}: cargo install --locked cargo-deny --version {{cargo_deny_version}}"; exit 1; }
    @cargo audit --version | grep -qx 'cargo-audit-audit {{cargo_audit_version}}' || { echo "need cargo-audit {{cargo_audit_version}}: cargo install --locked cargo-audit --version {{cargo_audit_version}}"; exit 1; }

# The benchmarks measure at design scale, never on toy data: target-scale fixtures (40 swimlanes, about 80,000 files).
agent-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f target/fixtures/target/manifest.json ]; then
        cargo xtask gen-fixtures --scale target --out target/fixtures/target
    fi

agent-lint:
    cargo fmt --all -- --check
    cargo clippy -p agent --all-targets -- -D warnings

# Unit, property, golden and integration tests of the agent, in virtual time where they can be: the fake hub (real TLS 1.3
# and gRPC), the fake Kubernetes API, the fake metadata server, the corpus replay, the Dockerfile and HARDENING.md lints.
agent-test:
    cargo test -p agent --lib --tests

# The tests that run in real time: P1 with the 10 s walk, the 3 s quiet period and the 30 s maximum deferral.
agent-slow-test:
    cargo test -p agent --release --tests -- --ignored

# S22: the agent's own fuzz crate (decision A16; `fuzz-smoke` does not run it). Its lockfile is complete and passes
# cargo deny and cargo audit (S19), then each target runs {{fuzz_seconds}} s on the pinned nightly. A crash leaves its input in
# crates/agent/fuzz/artifacts/. T9 adds agent_spool_record to the list.
agent_fuzz_targets := "agent_path agent_hub_message agent_cert_chain agent_pem agent_id_token"
agent-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/agent/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/agent/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/agent/fuzz/Cargo.lock
    for target in {{agent_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/agent/fuzz "target/fuzz-corpus/$target" "crates/agent/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "agent-fuzz: {{agent_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P1, P3, P4 and P4.stat_walk, P4.cpu_mcores and P4.memory_mib: the criterion benchmarks and the harnesses that write
# target/perf-results/, then the check against perf/budgets.toml. A registered budget without a result fails.
agent-bench:
    cargo bench -p agent
    cargo xtask bench-check
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
