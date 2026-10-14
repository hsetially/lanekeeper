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
registry_fuzz_targets := "registry_delta"
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

# P15 (T1), and from the later tasks P1, P2, P5, P6, P7, P12 and P14: the benchmarks at target scale (the fixtures are generated into
# target/fixtures/target when missing), then `cargo xtask bench-check` on the budgets whose benchmark is in crates/hub-registry/benches/
# only. The full registry needs every other prompt's benchmarks to have run (`just bench`), and `bench-check` has no filter, so the
# recipe cuts a registry with just the budgets below and judges that; the full `cargo xtask bench-check` is run by the FINAL step
# after `just bench`. A budget that is not registered yet is judged too: it is listed as UNMET with the number the benchmark
# measured and does not fail the recipe (plans/05-registry-read-api.blockers.md says why P15 is one). The benchmarks need a database
# (Docker, or LK_TEST_PG_ADMIN_URL) for the Postgres they measure.
# `tests/justfile_gate.rs` fails if this list and the benchmark files of the crate drift apart.
registry_budgets := "P15.registry_ingest_versions_per_s"
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
