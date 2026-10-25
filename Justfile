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
    @echo "the hub dev server is added by prompt 03b; the web app: 'just web-dev' (Vite with a proxy to a running hub) or 'just web-mock' (the production build behind the hub's headers with mock data)"

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
# crates/agent/fuzz/artifacts/. `tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart.
agent_fuzz_targets := "agent_path agent_hub_message agent_cert_chain agent_pem agent_id_token agent_spool_record"
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

# P1, P3, P4 and P4.stat_walk, P4.cpu_mcores, P4.memory_mib and P15 (the spool's replay rate): the criterion benchmarks and
# the harnesses that write target/perf-results/, then the check against perf/budgets.toml. A registered budget without a
# result fails.
agent-bench:
    cargo bench -p agent
    cargo xtask bench-check
# Prompt 03a, hub identity, sessions, authorization and audit (S1 to S4, S7, S9, S10, S21, S22; P7, P12): lint, the tests
# (unit, property, golden, integration; the integration tests need a Docker daemon from T2 on and fail without one), the
# fuzz targets on the pinned nightly, and the benchmarks checked against perf/budgets.toml. Later tasks add to the
# sub-recipes below and never skip one.
verify-03a: identity-tools-check identity-lint identity-test identity-fuzz identity-bench

# The pinned tools the gate needs, or the command that installs them. Never skips.
identity-tools-check: agent-tools-check

identity-lint:
    cargo fmt --all -- --check
    cargo clippy -p hub-identity --all-targets -- -D warnings

# The mock Entra, the key-set cache with a paused clock, the real HTTPS client against a local TLS 1.3 server, the
# seed replay of the fuzz corpora. LK_REQUIRE_DOCKER makes a missing Docker daemon a failure and not a skip.
identity-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-identity --lib --tests

# S22. `tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart.
identity_fuzz_targets := "identity_jwt identity_cookie identity_audit_event"
identity-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/hub-identity/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/hub-identity/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/hub-identity/fuzz/Cargo.lock
    for target in {{identity_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/hub-identity/fuzz "target/fuzz-corpus/$target" "crates/hub-identity/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "identity-fuzz: {{identity_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P7 and P12 (from T3 and T5): the criterion benchmarks, then `cargo xtask bench-check` on this prompt's budgets only. The
# full registry needs every other prompt's benchmarks to have run (`just bench`), and `bench-check` has no filter, so the
# recipe cuts a registry with just the budgets below and judges that; the full `cargo xtask bench-check` is run by the FINAL
# step after `just bench`. `tests/justfile_gate.rs` fails if this list and the registered `hub-identity/` budgets drift apart.
identity_budgets := "P7.auth_middleware P7.auth_middleware_cold P12.audit_record"
identity-bench:
    #!/usr/bin/env bash
    set -euo pipefail
    # Only this run's numbers may satisfy the check: a benchmark that dies at teardown must not pass on an earlier run's data.
    rm -rf target/criterion/hub-identity
    cargo bench -p hub-identity
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    awk -v ids="{{identity_budgets}}" 'BEGIN { RS = ""; ORS = "\n\n"; n = split(ids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 } /^\[\[budget\]\]/ { if (match($0, /\nid = "[^"]+"/)) { id = substr($0, RSTART + 7, RLENGTH - 8); if (id in want) print } }' perf/budgets.toml > "$tmp/budgets.toml"
    found="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml" || true)"
    wanted="$(wc -w <<< "{{identity_budgets}}")"
    if [ "$found" -ne "$wanted" ]; then echo "identity-bench: found $found of $wanted budgets in perf/budgets.toml"; exit 1; fi
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# Prompt 03b gate (the hub platform): every sub-recipe, in order, stopping at the first failure. Run `just verify` and
# `cargo xtask bench-check` after it (AGENTS.md, protocol step 4; platform-bench already ends with the check). Not part of the gate,
# and said so rather than skipped silently: `docker build` of crates/hub/Dockerfile (the Dockerfile and HARDENING.md are linted by
# `platform-test`), osv-scanner (not installed here), and the opt-in tests against a real Google Cloud project
# (`cargo test -p hub-platform --test gcp_real -- --ignored`, which need a project to spend on them).
verify-03b: platform-tools-check platform-lint platform-test platform-slow-test platform-fuzz platform-bench

# The pinned tools the gate needs, and a Docker daemon (the database tests start Postgres 16 with pgvector through
# testcontainers), or the command that fixes it. Never skips.
platform-tools-check: docker-check
    @cargo deny --version | grep -qx 'cargo-deny {{cargo_deny_version}}' || { echo "need cargo-deny {{cargo_deny_version}}: cargo install --locked cargo-deny --version {{cargo_deny_version}}"; exit 1; }
    @cargo audit --version | grep -qx 'cargo-audit-audit {{cargo_audit_version}}' || { echo "need cargo-audit {{cargo_audit_version}}: cargo install --locked cargo-audit --version {{cargo_audit_version}}"; exit 1; }

platform-lint:
    cargo fmt --all -- --check
    cargo clippy -p hub-platform -p hub --all-targets --all-features -- -D warnings

# Unit, property, golden and integration tests of the hub platform and the composition crate. LK_REQUIRE_DOCKER=1 turns a
# missing Docker daemon into a failure, never a skip.
platform-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-platform -p hub --all-features

# The tests whose time bounds are the point, in release mode and one at a time, so that a bound (events to every replica within
# 1 s, a push to `GitHeadMoved` within 10 s, the routing, lease and outbox races over three replicas) is checked without the load
# of the rest of the suite. They also run in `platform-test`; this is the run with nothing else going on.
platform-slow-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-platform --release --all-features --test events_bus --test gateway_cluster --test git_sync --test lease_race --test outbox -- --test-threads=1

# S22: the hub platform's own fuzz crate (plan A17; `fuzz-smoke` does not run it). Its lockfile is complete and passes cargo deny
# and cargo audit (S19), then each target runs {{fuzz_seconds}} s on the pinned nightly. A crash leaves its input in
# crates/hub-platform/fuzz/artifacts/. `crates/hub/tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart. The
# committed seeds also replay on stable in `cargo test -p hub-platform --test corpus_replay`.
platform_fuzz_targets := "hub_agent_message hub_forward_request hub_join_csr hub_sentinel_message hub_webhook_payload"
platform-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/hub-platform/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/hub-platform/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/hub-platform/fuzz/Cargo.lock
    for target in {{platform_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/hub-platform/fuzz "target/fuzz-corpus/$target" "crates/hub-platform/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "platform-fuzz: {{platform_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P2, P7 (blob), P11, P13 and P15 (acks): the criterion benchmarks and the harnesses that write target/perf-results/, at target
# scale (`cargo xtask gen-fixtures --scale target` first, as `agent-fixtures` does), then the check against perf/budgets.toml. A
# registered budget without a result fails, and the check covers the whole registry, so the agent's benchmarks (prompt 02) and
# the hub identity's (prompt 03a: P7.auth_middleware, P7.auth_middleware_cold, P12.audit_record) run too: without them their
# registered budgets have no result. The benchmarks need Docker (or LK_TEST_PG_ADMIN_URL) for the Postgres they measure.
platform-bench: agent-fixtures
    cargo bench -p agent
    cargo bench -p hub-identity
    cargo bench -p hub-platform -p hub
    cargo xtask bench-check
# The gate is complete (T10): lint, tests, the engine's fuzz crate, `fuzz-smoke`, and the benchmarks against this prompt's
# budgets. Run `just verify` and `cargo xtask bench-check` after it (AGENTS.md, protocol step 4).
# `engine-real-snapshot` is opt-in, needs the real repositories, and is not part of the gate.
# Prompt 04 gate (the fact engine; S11, S13, S21, S22; P5 to P7): every sub-recipe, in order, stopping at the first failure.
verify-04: engine-tools-check engine-lint engine-test engine-fuzz fuzz-smoke engine-bench

# The pinned tools the gate needs, or the command that installs them. Never skips.
engine-tools-check: agent-tools-check

engine-lint:
    cargo fmt --all -- --check
    cargo clippy -p engine --all-targets -- -D warnings

# Unit, property, golden and integration tests of the engine. It does no I/O, so there is no database and no Docker here. The
# committed seeds of the engine's fuzz corpora replay here too (tests/corpus_replay.rs).
engine-test:
    cargo test -p engine --lib --tests

# Acceptance 4 (opt-in, plan decision A4): the engine against the real repositories, which this repository never holds (rule 12).
# Not part of `verify-04`. Without the two checkouts it says NOT RUN and exits 1, because a run that did not happen is not a
# pass; a human runs it with them and pastes the output into the PR. It checks the locations, the checks of `sit1` and the
# ambiguous names of the base (C9). It prints counts, file names and setting paths, never a value.
#   LK_REAL_BASE_DIR   configuration-base-saas, default branch (the config/ folder or the checkout)
#   LK_REAL_TENANT_DIR csp-tenant-data, branch sit1 (the data/config/ folder or the checkout)
engine-real-snapshot:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${LK_REAL_BASE_DIR:-}" ] || [ -z "${LK_REAL_TENANT_DIR:-}" ]; then
        echo "real-snapshot: NOT RUN (set LK_REAL_BASE_DIR to a configuration-base-saas checkout and LK_REAL_TENANT_DIR to a csp-tenant-data checkout at sit1)"
        exit 1
    fi
    cargo test -p engine --test real_snapshot -- --ignored --nocapture

# S22: the engine's own fuzz crate (plan decision A2; `fuzz-smoke` does not run it). Its lockfile is complete and passes cargo
# deny and cargo audit (S19), then each target runs {{fuzz_seconds}} s on the pinned nightly. A crash leaves its input in
# crates/engine/fuzz/artifacts/. `tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart.
engine_fuzz_targets := "engine_bytes engine_yaml engine_json engine_properties engine_flatten engine_index_settings engine_path_mapping engine_regex engine_secret_scan engine_render"
engine-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/engine/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/engine/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/engine/fuzz/Cargo.lock
    for target in {{engine_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/engine/fuzz "target/fuzz-corpus/$target" "crates/engine/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "engine-fuzz: {{engine_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P5 to P7 (engine budgets, from T2 on): the criterion benchmarks at target scale (the fixtures are generated into
# target/fixtures/target when missing), then `cargo xtask bench-check` on this prompt's budgets only. The full registry needs
# every other prompt's benchmarks to have run (`just bench`), and `bench-check` has no filter, so the recipe cuts a registry with
# just the budgets below and judges that; the full `cargo xtask bench-check` is run by the FINAL step after `just bench`.
# `tests/justfile_gate.rs` fails if this list and the registered `engine/` budgets drift apart.
engine_budgets := "P6 P6.index_settings P6.parse_flatten P7.compare P7.engine_grid P7.engine_text_search P5.engine_base P5.engine_per_file_per_swimlane"
engine-bench:
    #!/usr/bin/env bash
    set -euo pipefail
    # Only this run's numbers may satisfy the check: a benchmark that dies at teardown must not pass on an earlier run's data.
    rm -rf target/criterion/engine
    cargo bench -p engine
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    awk -v ids="{{engine_budgets}}" 'BEGIN { RS = ""; ORS = "\n\n"; n = split(ids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 } /^\[\[budget\]\]/ { if (match($0, /\nid = "[^"]+"/)) { id = substr($0, RSTART + 7, RLENGTH - 8); if (id in want) print } }' perf/budgets.toml > "$tmp/budgets.toml"
    found="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml" || true)"
    wanted="$(wc -w <<< "{{engine_budgets}}")"
    if [ "$found" -ne "$wanted" ]; then echo "engine-bench: found $found of $wanted budgets in perf/budgets.toml"; exit 1; fi
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# Prompt 07 gate (the MCP server; S4, S11, S14, S14b, S15, S21; P9): every sub-recipe, in order, stopping at the first failure. T0
# started it with the tools check, lint, tests and the status of the Cursor results; T1 adds the Docker daemon for the Postgres-backed
# token tests and the fuzz targets; T2 adds the P9 benchmarks (`mcp-bench`, which T7 extends with `P9.get_served_config`); T7 closes
# the list: the gate is complete. Run `just verify` and `cargo xtask bench-check` after it (AGENTS.md, protocol step 4). Not part of
# the gate and said so: the Cursor 3.22.12 checks of T0 need a human (see `mcp-t0-status`).
verify-07: mcp-tools-check mcp-lint mcp-test mcp-fuzz mcp-bench mcp-t0-status

# The pinned tools the gate needs, or the command that installs them, and the Docker daemon the token store tests start Postgres in.
# Never skips.
mcp-tools-check: agent-tools-check docker-check

mcp-lint:
    cargo fmt --all -- --check
    cargo clippy -p hub-mcp --all-targets -- -D warnings
    cargo clippy -p hub --all-targets -- -D warnings

# Unit, property, golden and protocol tests of the MCP crate, including the compatibility stub driven in process (no sockets, a
# paused clock), the rmcp spike of plan A19, the authentication tests, and the token store against a real Postgres 16 (T1).
# LK_REQUIRE_DOCKER makes a missing Docker daemon a failure and not a skip. Then the hub's module test: the MCP routes in the composed hub.
mcp-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-mcp --lib --tests
    cargo test -p hub --test mcp_module

# S22: the MCP crate's own fuzz crate (decision A10 of plan 07; `fuzz-smoke` does not run it). Its lockfile is complete and passes
# cargo deny and cargo audit (S19), then each target runs {{fuzz_seconds}} s on the pinned nightly. A crash leaves its input in
# crates/hub-mcp/fuzz/artifacts/. `tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart.
mcp_fuzz_targets := "mcp_bearer_header mcp_pat_token mcp_tool_args mcp_cursor mcp_http_body mcp_elicitation_result mcp_redactor"
mcp-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/hub-mcp/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/hub-mcp/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/hub-mcp/fuzz/Cargo.lock
    for target in {{mcp_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/hub-mcp/fuzz "target/fuzz-corpus/$target" "crates/hub-mcp/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "mcp-fuzz: {{mcp_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P9 (T2): the read tools of the MCP server at target scale, on fakes, so this layer's share of a call (plan A7). The criterion
# benchmark `crates/hub-mcp/benches/read_tools.rs` runs the real server in process, then `cargo xtask bench-check` judges a registry
# cut to the P9 budgets below (the full registry needs every other prompt's benchmarks; the FINAL step runs it after `just bench`).
# `tests/justfile_gate.rs` fails if this list and the registered `hub-mcp/` budgets drift apart.
mcp_budgets := "P9 P9.list_swimlanes P9.get_swimlane P9.list_files P9.read_file P9.get_effective_config P9.compare P9.compare_tree P9.compare_grid P9.search_settings P9.search_text P9.find_inconsistencies P9.get_drift P9.get_pending_restarts P9.get_history P9.search_docs P9.read_doc P9.grep_docs P9.get_docs_for_file P9.get_feature_status P9.get_served_config"
mcp-bench:
    #!/usr/bin/env bash
    set -euo pipefail
    # Only this run's numbers may satisfy the check: a benchmark that dies at teardown must not pass on an earlier run's data.
    rm -rf target/criterion/hub-mcp
    cargo bench -p hub-mcp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    awk -v ids="{{mcp_budgets}}" 'BEGIN { RS = ""; ORS = "\n\n"; n = split(ids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 } /^\[\[budget\]\]/ { if (match($0, /\nid = "[^"]+"/)) { id = substr($0, RSTART + 7, RLENGTH - 8); if (id in want) print } }' perf/budgets.toml > "$tmp/budgets.toml"
    found="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml" || true)"
    wanted="$(wc -w <<< "{{mcp_budgets}}")"
    if [ "$found" -ne "$wanted" ]; then echo "mcp-bench: found $found of $wanted budgets in perf/budgets.toml"; exit 1; fi
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# `crates/hub-mcp/src/schemas/tools.json` is generated from docs/mcp-tools.md and never edited by hand. After a change to the
# document (a contract PR), this regenerates it; `mcp-test` fails when the file and the document differ.
mcp-schemas:
    UPDATE_SCHEMAS=1 cargo test -p hub-mcp --test mcp_auth contract_schemas::embedded_schemas_equal_the_contract_document

# T0 (A4): what is recorded of the Cursor 3.22.12 compatibility test (D91 in docs/decisions.md). The checks need a human with
# Cursor on Windows and on macOS (`crates/hub-mcp/t0/README.md`), so an agent run says NOT RUN per operating system, and NOT RUN is
# never counted as a pass. This prints the table and exits 0; a table it cannot read is an error. A human runs
# `just mcp-t0-status --strict` before go-live: it exits 1 until every cell is filled and the decisions follow the outcome rules.
mcp-t0-status *args:
    cargo run --quiet -p hub-mcp --example t0_status -- {{args}}

# T4 (A9): the Cursor plugin package for ONE authentication mode, built into target/plugin/ (it is removed first). `mode` is oauth (the
# default) or personal_token, never both (D52). The hub's origin and, for oauth, the Cursor client id and the hub API's audience come
# from the environment, so no tenant value is committed: LK_PLUGIN_HUB_ORIGIN, LK_PLUGIN_CLIENT_ID, LK_PLUGIN_API_AUDIENCE (see
# crates/hub-mcp/plugin/README.md). The Cursor plugin format is UNVERIFIED (D91, T0 is NOT RUN): check the package on the first install.
mcp-plugin mode="oauth":
    rm -rf target/plugin
    bash crates/hub-mcp/plugin/package.sh "{{mode}}" target/plugin

# T4: rewrites the two generated blocks of docs/cursor-admin.md (the tool allowlist and the write tools, with their counts) from the
# contract. `mcp-test` fails when they differ from `crates/hub-mcp/src/schemas/tools.json`, which `mcp-schemas` keeps equal to
# docs/mcp-tools.md.
mcp-admin-doc:
    UPDATE_ADMIN_DOC=1 cargo test -p hub-mcp --test cursor_admin_doc generated_blocks_equal_the_contract

# Prompt 05 gate (the registry: ingestion, indexes, the incremental pipeline, adoption, the read API; S4, S10, S11, S13, S21, S22; P1, P2,
# P5 to P7, P12 to P15): every sub-recipe, in order, stopping at the first failure. Later tasks add to the sub-recipes below and never
# skip one. Run `just verify` and `cargo xtask bench-check` after it (AGENTS.md, protocol step 4). Not part of the gate, and said so
# rather than skipped silently: osv-scanner (not installed here).
verify-05: registry-tools-check registry-lint registry-test registry-slow-test registry-fuzz registry-bench

# The pinned tools the gate needs, and a Docker daemon (the database tests start Postgres 16 with pgvector through testcontainers),
# or the command that fixes it. Never skips.
registry-tools-check: platform-tools-check

registry-lint:
    cargo fmt --all -- --check
    cargo clippy -p hub-registry --all-targets --all-features -- -D warnings

# Unit, property, golden and integration tests of the registry, over a real Postgres 16 with pgvector. LK_REQUIRE_DOCKER=1 turns a
# missing Docker daemon into a failure, never a skip. The committed seeds of the fuzz corpora replay here too
# (tests/corpus_replay.rs).
registry-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-registry --lib --tests

# The tests whose time bounds are the point, in release mode and one at a time (marked `#[ignore]`), so that a bound is checked
# without the load of the rest of the suite. Nothing is ignored in the other recipe: these only run here.
registry-slow-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-registry --release --tests -- --ignored --test-threads=1

# S22: the registry's own fuzz crate (plan A40; `fuzz-smoke` does not run it). Its lockfile is complete and passes cargo deny and
# cargo audit (S19), then each target runs {{fuzz_seconds}} s on the pinned nightly. A crash leaves its input in
# crates/hub-registry/fuzz/artifacts/. `tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart. The committed seeds
# also replay on stable in `cargo test -p hub-registry --test corpus_replay`.
registry_fuzz_targets := "registry_delta registry_cursor registry_query registry_sentinel_path"
registry-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/hub-registry/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/hub-registry/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/hub-registry/fuzz/Cargo.lock
    for target in {{registry_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/hub-registry/fuzz "target/fuzz-corpus/$target" "crates/hub-registry/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "registry-fuzz: {{registry_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P15 (T1), P1, P2, P5 and P6 (T3), P6.adoption_rank (T5), P7 and P12 (T6; from the read bed, through the router), and P14 (T7; the sentinel's attribution, with the leased job running at its interval): the benchmarks at target scale (the fixtures are generated into
# target/fixtures/target when missing), then `cargo xtask bench-check` on the budgets whose benchmark is in crates/hub-registry/benches/
# only. The full registry needs every other prompt's benchmarks to have run (`just bench`), and `bench-check` has no filter, so the
# recipe cuts a registry with just the budgets below and judges that; the full `cargo xtask bench-check` is run by the FINAL step
# after `just bench`. A budget that is not registered yet is judged too: it is listed as UNMET with the number the benchmark
# measured and does not fail the recipe. The benchmarks need a database (Docker, or LK_TEST_PG_ADMIN_URL) for the Postgres they
# measure.
# `tests/justfile_gate.rs` fails if this list and the benchmark files of the crate drift apart.
registry_budgets := "P15.registry_ingest_versions_per_s P5 P5.per_file_per_swimlane P6.hub_full_recompute P1.hub_delta_to_drift P2.hub_head_moved_to_drift P6.adoption_rank P7.swimlane_list P7.file_tree P7.file_content_cached P7.file_content_cold P7.compare_api P7.grid P7.whole_swimlane_compare P7.settings_search P7.text_search P12.steady_state_query_ms P14"
registry-bench:
    #!/usr/bin/env bash
    set -euo pipefail
    # Only this run's numbers may satisfy the check: a benchmark that dies at teardown must not pass on an earlier run's data.
    rm -rf target/criterion/hub-registry
    rm -f target/perf-results/hub-registry_*.json
    cargo bench -p hub-registry
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    awk -v ids="{{registry_budgets}}" 'BEGIN { RS = ""; ORS = "\n\n"; n = split(ids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 } /(^|\n)\[\[budget\]\]/ { if (match($0, /\nid = "[^"]+"/)) { id = substr($0, RSTART + 7, RLENGTH - 8); if (id in want) print } }' perf/budgets.toml > "$tmp/budgets.toml"
    found="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml" || true)"
    wanted="$(wc -w <<< "{{registry_budgets}}")"
    if [ "$found" -ne "$wanted" ]; then echo "registry-bench: found $found of $wanted budgets in perf/budgets.toml"; exit 1; fi
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# Prompt 06 gate (the write paths; S4, S8 to S11, S13, S21, S22; P8, P12): lint, the tests (unit, property, golden and integration; the
# integration tests need Postgres 16 with pgvector from a Docker daemon or from `LK_TEST_PG_ADMIN_URL`, and fail without one), and
# the benchmarks checked against perf/budgets.toml. Later tasks add to the sub-recipes below and never skip one. Run `just verify` and `cargo xtask bench-check` after it (AGENTS.md, protocol step 4).
# Not part of the gate, and said so rather than skipped silently: a real GitHub (CRLF through the Git Data API, fine-grained token
# behaviour), a real Cloud KMS, a real Teams Workflows webhook and a real agent on NFS; none is reachable from the sandbox.
verify-06: writes-tools-check writes-lint writes-test writes-fuzz writes-bench

# The pinned tools the gate needs, and a database for the tests (a Docker daemon, or `LK_TEST_PG_ADMIN_URL` pointing at a throwaway
# Postgres 16 server), or the command that fixes it. Never skips. The fuzz step's tools (the pinned nightly, cargo-fuzz, a C++
# compiler) are looked for here too, so a missing one fails before the database tests and not after them.
writes-tools-check: fuzz-tools-check
    @cargo deny --version | grep -qx 'cargo-deny {{cargo_deny_version}}' || { echo "need cargo-deny {{cargo_deny_version}}: cargo install --locked cargo-deny --version {{cargo_deny_version}}"; exit 1; }
    @cargo audit --version | grep -qx 'cargo-audit-audit {{cargo_audit_version}}' || { echo "need cargo-audit {{cargo_audit_version}}: cargo install --locked cargo-audit --version {{cargo_audit_version}}"; exit 1; }
    @docker info >/dev/null 2>&1 || [ -n "${LK_TEST_PG_ADMIN_URL:-}" ] || { echo "verify-06 needs a Docker daemon (Postgres 16 with pgvector through testcontainers) or LK_TEST_PG_ADMIN_URL=postgres://postgres:...@host:5432/postgres"; exit 1; }

writes-lint:
    cargo fmt --all -- --check
    cargo clippy -p hub-writes --all-targets -- -D warnings

# Unit, property, golden and integration tests of the write path. LK_REQUIRE_DOCKER=1 turns a missing database into a failure,
# never a skip.
writes-test:
    LK_REQUIRE_DOCKER=1 cargo test -p hub-writes --lib --tests

# S22 (from T3): the write path's own fuzz crate (`fuzz-smoke` does not run it). Its lockfile is complete and passes cargo deny and
# cargo audit (S19), then each target runs {{fuzz_seconds}} s on the pinned nightly. A crash leaves its input in
# crates/hub-writes/fuzz/artifacts/. `tests/justfile_gate.rs` fails if this list and fuzz_targets/ drift apart; the same seeds also
# replay on stable in `cargo test -p hub-writes --test corpus_replay`.
writes_fuzz_targets := "writes_request_json writes_upload_multipart writes_preview_id writes_github_token writes_github_responses writes_git_data_responses"
writes-fuzz: fuzz-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    cargo +{{fuzz_nightly}} metadata --locked --format-version 1 --manifest-path crates/hub-writes/fuzz/Cargo.toml > /dev/null
    cargo deny --manifest-path crates/hub-writes/fuzz/Cargo.toml check --config deny.toml
    cargo audit --file crates/hub-writes/fuzz/Cargo.lock
    for target in {{writes_fuzz_targets}}; do
        mkdir -p "target/fuzz-corpus/$target"
        cargo +{{fuzz_nightly}} fuzz run "$target" --fuzz-dir crates/hub-writes/fuzz "target/fuzz-corpus/$target" "crates/hub-writes/fuzz/corpus/$target" -- -max_total_time={{fuzz_seconds}}
    done
    echo "writes-fuzz: {{writes_fuzz_targets}} ran {{fuzz_seconds}} s each without a crash"

# P8 and P12.hub_writes_queries (from T1): the criterion benchmarks, then `cargo xtask bench-check` on this prompt's budgets only. The
# full registry needs every other prompt's benchmarks to have run (`just bench`), and `bench-check` has no filter, so the recipe cuts
# a registry with just the budgets below and judges that; the full `cargo xtask bench-check` is run by the FINAL step after
# `just bench`. `tests/justfile_gate.rs` fails if this list and the registered `hub-writes/` budgets drift apart.
writes_budgets := "P8 P12.hub_writes_queries"
writes-bench:
    #!/usr/bin/env bash
    set -euo pipefail
    # Only this run's numbers may satisfy the check: a benchmark that dies at teardown must not pass on an earlier run's data.
    rm -rf target/criterion/hub-writes
    cargo bench -p hub-writes
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    awk -v ids="{{writes_budgets}}" 'BEGIN { RS = ""; ORS = "\n\n"; n = split(ids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 } /^\[\[budget\]\]/ { if (match($0, /\nid = "[^"]+"/)) { id = substr($0, RSTART + 7, RLENGTH - 8); if (id in want) print } }' perf/budgets.toml > "$tmp/budgets.toml"
    found="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml" || true)"
    wanted="$(wc -w <<< "{{writes_budgets}}")"
    if [ "$found" -ne "$wanted" ]; then echo "writes-bench: found $found of $wanted budgets in perf/budgets.toml"; exit 1; fi
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# Prompt 08 (web). The web app is served by the hub itself, so its tests run behind the hub's own header code: `web-fixture`
# builds the Rust example (crates/hub/examples/web_fixture.rs) that serves web/dist with the production CSP, and the Playwright
# harness (web/e2e/harness) puts the mock API in front of it. No browser is needed for lint, typecheck or the unit tests.
web-install:
    cd web && pnpm install --frozen-lockfile

# The generated API client is committed (the image build copies only web/, so it cannot read ../api). It must be what
# `pnpm gen:api` writes from api/openapi.yaml now: regenerate and fail if anything under src/api changed or is new.
web-gen-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd web
    pnpm gen:api
    if [ -n "$(git status --porcelain -- src/api)" ]; then
        echo "web-gen-check: src/api is stale; run 'pnpm gen:api' in web/ and commit the result"
        git status --porcelain -- src/api | head -20
        exit 1
    fi

web-fixture:
    cargo build --locked -p hub --example web_fixture

# Vite's dev server with the proxy to a running hub (LK_HUB_URL, default http://127.0.0.1:8080). It has no production CSP:
# CSP checks run in `web-e2e`, behind the hub's header code.
web-dev:
    cd web && pnpm dev

# The production build behind the hub's headers with a mock scenario (admin by default): `just web-mock`, then open the URL it prints.
web-mock scenario="admin": web-fixture
    cd web && pnpm build && node e2e/harness/serve.ts --dist dist --scenario {{scenario}}

# End to end under the production CSP (S12): the app, every overlay and the editor, in a real Chromium. `build:perf` is the
# test-only build with the extra routes (the overlay and editor pages); it is never the production dist. The whole project is
# about 75 minutes of two workers, so CI runs it as a matrix of Playwright shards (.github/workflows/web-e2e.yml): a shard
# runs when LK_E2E_SHARD is set to `i/N` (`LK_E2E_SHARD=2/5 just web-e2e`), and every test when it is not.
web-e2e: web-fixture
    cd web && pnpm build && pnpm build:perf && pnpm exec playwright test --project=chromium ${LK_E2E_SHARD:+--shard=$LK_E2E_SHARD}

# P10 performance gates, measured here and judged by `web-bench-check`: the bundle (the end of `pnpm build`), then, in the
# `perf` Playwright project with one worker so that nothing else shares the machine, the route change, the 6,000-line diff and
# the 5,000-row grid, then Lighthouse (web/scripts/perf/lighthouse.mjs, web/lighthouserc.json). Every web result is removed
# first, so only this run's numbers can pass. The functional specs of the same pages are in `web-e2e` (the `chromium` project).
# The grid number is the grid's CAPACITY (frames a second with the frame-rate limit off, the same scroll in every frame, the
# minimum of five runs, floor 60), and the spec ALSO fails when the grid drops more frames than a control list on the same
# machine in the same run, plus a margin (W20, decided 2026-10-17; web/e2e/perf/grid-5000.spec.ts). It is not the display's rate.
web-perf: web-fixture
    #!/usr/bin/env bash
    set -euo pipefail
    rm -f "${CARGO_TARGET_DIR:-target}"/perf-results/web-*.json
    cd web
    pnpm build
    pnpm build:perf
    pnpm exec playwright test --project=perf --workers=1
    node scripts/perf/lighthouse.mjs

# The five P10 budgets, judged on their own (the full registry needs every other prompt's benchmarks, see writes-bench).
# `web/scripts/verify08.test.mjs` fails if this list and the registered `web/` budgets of perf/budgets.toml drift apart.
web_budgets := "P10.initial_js_gzip_kb P10.route_change_ms P10.diff_6000_lines_ms P10.grid_5000_rows_fps P10.lighthouse_perf"
web-bench-check:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    awk -v ids="{{web_budgets}}" 'BEGIN { RS = ""; ORS = "\n\n"; n = split(ids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 } /^\[\[budget\]\]/ { if (match($0, /\nid = "[^"]+"/)) { id = substr($0, RSTART + 7, RLENGTH - 8); if (id in want) print } }' perf/budgets.toml > "$tmp/budgets.toml"
    found="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml" || true)"
    wanted="$(wc -w <<< "{{web_budgets}}")"
    if [ "$found" -ne "$wanted" ]; then echo "web-bench-check: found $found of $wanted budgets in perf/budgets.toml"; exit 1; fi
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# Prompt 08 gate: install, the generated client is fresh, lint, typecheck, unit tests, the production build (bundle report and
# the dist scan), the end-to-end CSP run, the performance gates, then the P10 budgets. Stops at the first failure.
verify-08: web-install web-gen-check
    cd web && pnpm lint
    cd web && pnpm typecheck
    cd web && pnpm exec vitest run
    cd web && pnpm build
    just web-e2e
    just web-perf
    just web-bench-check

# Prompt 09, T1 (S16, S19, S20): the images. Every tool runs from a digest-pinned image (deploy/images/tools.env), so the
# recipes need Docker, git, jq, node and python3, and fail at once without them, never skip.
images-tools-check:
    @docker info >/dev/null 2>&1 || { echo "the image recipes need a running Docker daemon with buildx"; exit 1; }
    @docker buildx version >/dev/null 2>&1 || { echo "the image recipes need docker buildx"; exit 1; }
    @for t in git jq node python3; do command -v "$t" >/dev/null 2>&1 || { echo "the image recipes need $t"; exit 1; }; done

# One image as an OCI archive in target/images, from `git archive HEAD` (see deploy/images/build.sh for LK_BUILD_CA and
# the other settings).
images-build image: images-tools-check
    deploy/images/build.sh {{image}}

# The T1 Verify: each image built twice from the same commit without the cache gives one digest.
images-repro: images-tools-check
    deploy/images/reproducible-check.sh hub agent

# The CycloneDX SBOM (image plus Rust crates) and the scan that fails on any critical finding, for an image already built.
images-sbom image: images-tools-check
    deploy/images/sbom.sh {{image}}

images-scan image: images-tools-check
    deploy/images/scan.sh {{image}}

# The lock and Dockerfile policies: the model lock's checker and its planted cases, the Rego unit tests, the planted
# Dockerfile through conftest's real parser (every expected rule must fire), both Dockerfiles, the crates' own Dockerfile
# lints, the hub's model-stage script against planted locks, and last the committed lock (refused while UNRESOLVED).
images-policy-test: images-tools-check
    #!/usr/bin/env bash
    set -euo pipefail
    . deploy/images/tools.env
    conftest() { docker run --rm -v "$PWD:/repo:ro" -w /repo "$LK_CONFTEST_IMAGE" "$@"; }
    node --test deploy/images/tests/*.test.mjs
    conftest verify --policy deploy/policy --namespace dockerfile --no-color
    planted="$(conftest test --parser dockerfile --policy deploy/policy --namespace dockerfile --no-color deploy/policy/testdata/dockerfile/planted.Dockerfile || true)"
    while IFS= read -r rule; do
        grep -qF -- "$rule" <<<"$planted" || { echo "images-policy-test: the planted Dockerfile did not trigger: $rule"; echo "$planted"; exit 1; }
    done < deploy/policy/testdata/dockerfile/planted.expected
    conftest test --parser dockerfile --policy deploy/policy --namespace dockerfile --no-color crates/hub/Dockerfile crates/agent/Dockerfile
    cargo test -p hub --test hardening
    cargo test -p agent --test hardening
    deploy/images/tests/model-stage-test.sh
    deploy/images/tests/build-args-test.sh
    node deploy/images/model-lock.mjs check deploy/images/hub-model.lock

# The scan blocks a critical finding: a planted SBOM with CVE-2021-44228 must fail it (needs the grype database).
images-scan-test: images-tools-check
    deploy/images/tests/scan-test.sh

# Prompt 09, T2 (S19, S20): the workflow and CI gates. The tools that run on the host are release files, pinned by version and by
# the sha256 of the file (plan 09 A22). Each sha256 was computed from the bytes downloaded for the pinned version; for actionlint,
# kubeconform, kind, osv-scanner, cosign and crane it equals the one in the checksum file that the release publishes (zizmor and
# shellcheck publish none, so theirs is the hash of the download alone). To move one: change the version, download the file
# again, compare its sha256 with the release's checksum file if there is one, and put the value here. The one tool that runs as a
# container (BuildKit) is pinned by digest in deploy/images/tools.env. Every version was released at least 14 days before it was
# chosen (2026-10-25): actionlint 1.7.12 on 2026-03-31, kubeconform 0.8.0 on 2026-06-04, kind 0.33.0 on 2026-08-26, osv-scanner
# 2.6.0 on 2026-09-14, zizmor 1.30.1 on 2026-09-09, cosign 3.1.3 on 2026-08-05, crane 0.22.1 on 2026-09-03, shellcheck 0.11.0 on
# 2025-08-03. helm is not here: get.helm.sh, where its release files live, is not reachable from the sandbox that wrote this
# (plans/09-deployment-ci.blockers.md, B4), so its pin comes with T3, the first task that needs it.
ops_tools_bin := "target/ops-tools/bin"
actionlint_version := "1.7.12"
actionlint_sha256 := "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
kubeconform_version := "0.8.0"
kubeconform_sha256 := "9bc2bffbf71f261128533edaf912153948b7ff238f9a531ae6d34466ec287883"
kind_version := "0.33.0"
kind_sha256 := "aee6151561422756b764a4ae28e7f44cda5af5a9eead3cc9985112b1de8d8e0d"
osv_scanner_version := "2.6.0"
osv_scanner_sha256 := "ca69b3d3cd08f889a49dc0a383122f71cc528b83803671df5fd874d97485b108"
zizmor_version := "1.30.1"
zizmor_sha256 := "e65324f4430c2717591937edcec90ccbefaf14c174f8ec9415e03ca875b46e1a"
cosign_version := "3.1.3"
cosign_sha256 := "4629c757b7618056f8ddd7e2625ae9fdd94c0372a65049520bc7d9df9efc7f71"
crane_version := "0.22.1"
crane_sha256 := "0ab7a1d6932a213aed964ce97666c3077fe691c8606413674a8b3e0b9ec4cda0"
shellcheck_version := "0.11.0"
shellcheck_sha256 := "8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198"
# Installed in CI by taiki-e/install-action (just) and `cargo install --locked` (sqlx-cli, which must match the sqlx in Cargo.toml).
just_version := "1.40.0"
sqlx_cli_version := "0.8.6"

# Downloads the pinned release files into target/ops-tools/bin and refuses any file whose sha256 differs from the pin. Linux amd64
# only: on another machine install the versions that ops-tools-check names. `just ops-tools-install actionlint zizmor` installs
# only those (CI jobs install what they use); with no argument it installs every tool.
ops-tools-install *tools:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)/$(uname -m)" = "Linux/x86_64" ] || { echo "ops-tools-install: the pinned release files are linux/amd64 builds; on this machine install the versions that 'just ops-tools-check' names"; exit 1; }
    bin="$PWD/{{ops_tools_bin}}"
    mkdir -p "$bin"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    wanted=({{tools}})
    # fetch NAME URL SHA256 [MEMBER]: MEMBER is the file to take out of an archive; without it the download is the binary.
    fetch() {
        local name="$1" url="$2" sha="$3" member="${4:-}"
        if [ ${#wanted[@]} -gt 0 ] && [[ ! " ${wanted[*]} " == *" $name "* ]]; then return 0; fi
        curl --fail --silent --show-error --location --retry 3 --max-time 300 --output "$work/$name.download" "$url"
        local got
        got="$(sha256sum "$work/$name.download" | cut -d' ' -f1)"
        [ "$got" = "$sha" ] || { echo "ops-tools-install: $name from $url has sha256 $got, the pin is $sha"; exit 1; }
        if [ -n "$member" ]; then
            mkdir "$work/$name.x"
            tar -xf "$work/$name.download" -C "$work/$name.x" "$member"
            install -m 0755 "$work/$name.x/$member" "$bin/$name"
        else
            install -m 0755 "$work/$name.download" "$bin/$name"
        fi
        echo "ops-tools-install: $name installed"
    }
    fetch actionlint "https://github.com/rhysd/actionlint/releases/download/v{{actionlint_version}}/actionlint_{{actionlint_version}}_linux_amd64.tar.gz" "{{actionlint_sha256}}" actionlint
    fetch kubeconform "https://github.com/yannh/kubeconform/releases/download/v{{kubeconform_version}}/kubeconform-linux-amd64.tar.gz" "{{kubeconform_sha256}}" kubeconform
    fetch kind "https://github.com/kubernetes-sigs/kind/releases/download/v{{kind_version}}/kind-linux-amd64" "{{kind_sha256}}"
    fetch osv-scanner "https://github.com/google/osv-scanner/releases/download/v{{osv_scanner_version}}/osv-scanner_linux_amd64" "{{osv_scanner_sha256}}"
    fetch zizmor "https://github.com/zizmorcore/zizmor/releases/download/v{{zizmor_version}}/zizmor-x86_64-unknown-linux-gnu.tar.gz" "{{zizmor_sha256}}" zizmor
    fetch cosign "https://github.com/sigstore/cosign/releases/download/v{{cosign_version}}/cosign-linux-amd64" "{{cosign_sha256}}"
    fetch crane "https://github.com/google/go-containerregistry/releases/download/v{{crane_version}}/go-containerregistry_Linux_x86_64.tar.gz" "{{crane_sha256}}" crane
    fetch shellcheck "https://github.com/koalaman/shellcheck/releases/download/v{{shellcheck_version}}/shellcheck-v{{shellcheck_version}}.linux.x86_64.tar.xz" "{{shellcheck_sha256}}" shellcheck-v{{shellcheck_version}}/shellcheck

# Fails at once, with the fix, when a pinned tool is missing or has another version. Never skips. `just ops-tools-check actionlint
# zizmor` checks only those (with none, every tool). A tool found on PATH counts when it reports the pinned version, so a developer's
# own install is fine.
ops-tools-check *tools:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$PWD/{{ops_tools_bin}}:$PATH"
    fail=0
    wanted=({{tools}})
    need() { # NAME WANTED FOUND
        if [ "$3" != "$2" ]; then echo "ops-tools-check: need $1 $2, found '${3:-nothing}'; run: just ops-tools-install $1"; fail=1; fi
    }
    want() { [ ${#wanted[@]} -eq 0 ] || [[ " ${wanted[*]} " == *" $1 "* ]]; }
    want actionlint && need actionlint "{{actionlint_version}}" "$(actionlint -version 2>/dev/null | head -n1 || true)"
    want kubeconform && need kubeconform "v{{kubeconform_version}}" "$(kubeconform -v 2>/dev/null || true)"
    want kind && need kind "v{{kind_version}}" "$(kind version 2>/dev/null | awk '{print $2}' || true)"
    want osv-scanner && need osv-scanner "{{osv_scanner_version}}" "$(osv-scanner --version 2>/dev/null | sed -n 's/^osv-scanner version: //p' || true)"
    want zizmor && need zizmor "zizmor {{zizmor_version}}" "$(zizmor --version 2>/dev/null || true)"
    want cosign && need cosign "{{cosign_version}}" "$(cosign version 2>/dev/null | sed -n 's/^GitVersion: *v\{0,1\}//p' || true)"
    want crane && need crane "{{crane_version}}" "$(crane version 2>/dev/null | sed 's/^v//' || true)"
    want shellcheck && need shellcheck "{{shellcheck_version}}" "$(shellcheck --version 2>/dev/null | sed -n 's/^version: //p' || true)"
    for t in "${wanted[@]}"; do
        case "$t" in actionlint | kubeconform | kind | osv-scanner | zizmor | cosign | crane | shellcheck) ;; *) echo "ops-tools-check: unknown tool '$t'"; fail=1 ;; esac
    done
    node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 22 ? 0 : 1)' || { echo "ops-tools-check: the .github scripts need Node 22 or later"; fail=1; }
    # Every image a script runs is pinned by digest (S19): a tag alone can be moved.
    while IFS= read -r line; do
        case "$line" in LK_*_IMAGE=*@sha256:????????????????????????????????????????????????????????????????) ;; LK_*_IMAGE=*) echo "ops-tools-check: not pinned by digest in deploy/images/tools.env: $line"; fail=1 ;; esac
    done < deploy/images/tools.env
    exit "$fail"

# The workflow checks: actionlint (which runs the pinned shellcheck on every run: script), zizmor, the SHA-pin check, the structural
# rules of check-workflows.mjs, and shellcheck on the shell scripts under deploy/ and .github/. zizmor's online audits (does a
# pinned SHA belong to the action's repository? does the version comment match the SHA?) run with LK_ZIZMOR_ONLINE=1 and a GH_TOKEN,
# as in CI; otherwise zizmor runs offline and says so.
ci-lint: (ops-tools-check "actionlint zizmor shellcheck")
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$PWD/{{ops_tools_bin}}:$PATH"
    actionlint
    node .github/scripts/check-pins.mjs
    node .github/scripts/check-workflows.mjs
    zizmor_args=(--config .github/zizmor.yml --no-progress)
    if [ "${LK_ZIZMOR_ONLINE:-}" != "1" ]; then zizmor_args+=(--offline); echo "ci-lint: zizmor runs offline and skips its online audits (CI sets LK_ZIZMOR_ONLINE=1 and GH_TOKEN)"; fi
    zizmor "${zizmor_args[@]}" .github/workflows .github/actions
    mapfile -t scripts < <(git ls-files --cached --others --exclude-standard -- 'deploy/*.sh' '.github/*.sh')
    shellcheck -x "${scripts[@]}"
    echo "ci-lint: ${#scripts[@]} shell scripts checked"

# osv-scanner over every lockfile of the repository (S19): the workspace's Cargo.lock, the lockfiles of the fuzz crates, and the pnpm
# lockfiles of the root and of web/. It asks api.osv.dev, so it needs the network; with the API unreachable osv-scanner fails
# (exit 127), it never reports a clean run. The list comes from git, so a new fuzz crate is scanned without a change here.
osv-scan: (ops-tools-check "osv-scanner")
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$PWD/{{ops_tools_bin}}:$PATH"
    mapfile -t locks < <(git ls-files -- ':(glob)**/Cargo.lock' ':(glob)**/pnpm-lock.yaml')
    for must in Cargo.lock pnpm-lock.yaml web/pnpm-lock.yaml; do
        printf '%s\n' "${locks[@]}" | grep -qxF "$must" || { echo "osv-scan: $must is not among the tracked lockfiles"; exit 1; }
    done
    args=()
    for f in "${locks[@]}"; do args+=(-L "$f"); done
    echo "osv-scan: ${#locks[@]} lockfiles"
    osv-scanner scan source "${args[@]}"

# Every registered budget the Rust benchmarks produce, judged by `cargo xtask bench-check`. perf/budgets.toml also holds the P10 web
# budgets, whose numbers come from the web-e2e workflow's `perf` job (web-bench-check), so a registry without them is cut here, the
# way the per-prompt bench recipes cut theirs. A registered budget without a result still fails.
bench-check-rust:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    # One entry runs from its [[budget]] line to the next one (a few are preceded by comment lines without a blank line).
    awk 'function flush() { if (buf != "" && buf !~ /\nbench = "web\//) printf "%s\n\n", buf; buf = "" } /^\[\[budget\]\]/ { flush(); buf = $0; next } buf != "" { buf = buf "\n" $0 } END { flush() }' perf/budgets.toml > "$tmp/budgets.toml"
    total="$(grep -c '^\[\[budget\]\]' perf/budgets.toml)"
    kept="$(grep -c '^\[\[budget\]\]' "$tmp/budgets.toml")"
    web="$(grep -c '^bench = "web/' perf/budgets.toml)"
    if [ "$((kept + web))" -ne "$total" ]; then echo "bench-check-rust: kept $kept and cut $web of $total budgets"; exit 1; fi
    echo "bench-check-rust: $kept of $total budgets ($web web budgets are judged by web-bench-check)"
    cargo xtask bench-check --budgets "$tmp/budgets.toml"

# S19: the SHA-pin check, the workflow rules and the Renovate settings, each against planted violations as well as the real files.
pins-test:
    node --test .github/scripts/*.test.mjs

# The pinned BuildKit (deploy/images/builder.sh) starts, reports the pinned version, and builds the same OCI archive twice.
images-builder-test: images-tools-check
    deploy/images/tests/builder-test.sh

# deploy/images/publish.sh against a registry on localhost: the digest that is pushed is the digest that was built, and a signature,
# an SBOM attestation and their verification work with a throwaway key. Keyless signing needs GitHub's OIDC and runs in CI only.
images-publish-test: images-tools-check (ops-tools-check "cosign crane")
    deploy/images/tests/publish-test.sh

# The T2 gate so far (T3 to T8 add to it): the pinned tools, the workflow lint, the pin tests, and the image checks.
verify-09: ops-tools-check ci-lint pins-test images-policy-test images-builder-test images-publish-test
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
