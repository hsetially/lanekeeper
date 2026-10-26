# Review of prompt 09, round 1

Reviewer: independent `reviewer` subagent (prompts/REVIEW.md). Branch `agent/09-deployment-ci` at `772d46a`, base `origin/main`.
The branch is 4 commits behind `origin/main`, and `git merge-tree` reports a conflict in `.github/workflows/ci.yml`.

VERDICT: CHANGES REQUESTED

Four blocking items need work in this branch (1 to 4). Items 5 to 7 are the known gate failures that need a human decision.

## Blocking

1. `.github/workflows/rust.yml:57-89` (test job) and `.github/workflows/ci.yml`. The branch deletes the `rust` job from `ci.yml` and moves it to `rust.yml`. It drops the "Free disk space" and "Disk left" steps that `origin/main` added in `1dcc360` (PR 25). That commit exists because main run 40166747415 hit Postgres 53100 "No space left on device" in `cargo test --workspace`.
   - Failing scenario: the branch merges (`ci.yml` conflicts, and taking the branch side loses the fix). `cargo test --workspace --locked` then runs on `ubuntu-latest` with the same disk pressure and fails the same way.
   - Fix: bring `origin/main` in, resolve `ci.yml`, and carry both steps into the `rust.yml` `test` job. Consider the same for the `image` job in `images.yml` (two `--no-cache` builds of each image plus a grype database) and for `bench.yml`.
   - Violates: S19/S22 gate reliability, and the T2 task.

2. `.github/workflows/sentinel.yml:148-212` (job `sign-release`: `id-token: write` at line 158, `environment: release` at line 155). The job runs `cargo install --locked cargo-cyclonedx` at line 180. That compiles crates.io build scripts and proc-macros inside the job that can mint the Workload Identity token for the release-key service account. After the auth step, `sbom.sh` runs that third-party binary with the Google credentials in the environment.
   - Failing scenario: a compromised dependency of cargo-cyclonedx requests an OIDC token for `release` and reads the RPM signing key.
   - This contradicts the header (lines 22-23, "jobs that hold the release key or can sign run no build step") and the threat-model STRIDE row at `docs/threat-model.md:1033` ("the jobs that sign hold no build step"). `images.yml` `publish` does it correctly: pinned, sha256-checked binaries only.
   - Fix: take the SBOM and the `cargo install` out of `sign-release`. Generate the SBOM in a later job with no `id-token`, `needs: sign-release`, on the signed file. Or use a sha256-pinned prebuilt binary. Then correct the header and the threat-model row.
   - Violates: S19, S20, S7.

3. `deploy/charts/hub/templates/networkpolicy.yaml:135` (FQDN list), `deploy/policy/network.rego:154-156` (`required_hosts`), `crates/hub/HARDENING.md:194,293-295` and `crates/hub/tests/hardening.rs:582-590` (`EXPECTED_HOSTS`). None allows `storage.googleapis.com`.
   - The hub's audit-checkpoint writer needs it (`crates/hub-identity/src/audit/bucket.rs:178`, `GcsCheckpointBucket::HOST`, S9).
   - The 09 gcp chart provisions the locked audit bucket and grants the hub `objectCreator` and `objectViewer`, and plan T3 lists Google APIs and Workload Identity for GCS.
   - The reviewer rendered `deploy/values/hub/staging.yaml`: the FQDNNetworkPolicy has only 6 hosts and no storage host.
   - Failing scenario: staging or prod with the FQDN policy. Default-deny blocks the hub's checkpoint `put` to the locked bucket, so S9's second copy never lands.
   - No blockers or evidence file mentions this.
   - Fix: add `storage.googleapis.com`. Change the chart, `network.rego` and its tests, `HARDENING.md` and `hardening.rs`, with a planted case for each. The last two are in the plan's Extra-paths.
   - Violates: S18/S9 coherence and the T3 task.

4. Traceability. The approved plan's Acceptance 3 row names `deploy/tests/kind/run.sh` and job `kind-e2e` in `images.yml`. Plan summary item 1 says "The kind test gets built and runs, and acceptance 3 stays unticked."
   - `deploy/tests/kind/`, a `kind-e2e` recipe and a `kind-e2e` job do not exist (checked in Justfile, `.github/` and `deploy/tests`). Two other rows depend on the missing kind run: S6 "kind: `psql sslmode=disable` is rejected" and P11 "kind measures time to Ready".
   - The evidence reports this honestly, but it is an unmet row of the approved plan, not an escalated decision.
   - Fix: write the kind script and job (it may fail honestly until A1 is composed), or have a human amend the plan's traceability.

5. Human-owned gate failure, B5 (verified, reported honestly). `just verify-09` exits 1 at `deploy-render`. `deploy/values/hub/github-hook-ranges.yaml` is still the unresolved marker (`githubHookRanges: []`), and `deploy/charts/gcp/templates/cloudarmor.yaml:7` refuses to render. Cloud Armor, and so the GitHub hook-range allowlist on `/hooks/github` (S18, P2 support), is not in place.
   - It cannot be approved while red unless a human records a waiver.
   - Acceptance 1 and the T5/T6 verify rows stay partial.
   - Acceptance 2 (signed images, SBOM and provenance published) has no run on GitHub behind it.
   - Acceptance 3 is unmet; this is also A1 (the hub binary exits 1, `crates/hub/src/main.rs:36`).

6. Human-owned gate failures, B14 and B15 (verified, reported honestly). `cargo deny check` exits 1 on RUSTSEC-2026-0382 (rustls-webpki) and RUSTSEC-2026-0386 (tokio). The fix is PR 30, open, touching `Cargo.lock`, `Cargo.toml` and `docs/decisions.md` only. `cargo xtask bench-check` exits 1 with 65 pass, 6 fail and 2 unmet of 73, so `just verify` and bench-check cannot be green. B15 covers two benches in crates 09 does not own:
   - `P6.index_settings` is 71.6 ms p95 against 60.
   - `P12.audit_record` is 3.29 ms p95 against 3.

7. Human confirmation needed at merge: A19 deviation. `deploy/values/gcp/prod.yaml:26` commits `isLocked: true` on the audit-checkpoint bucket with an EXAMPLE one-year retention. Plan A19 said "isLocked only after a human confirms the period"; a lock cannot be undone. It is labelled as an example, but the human must consciously confirm or change it before any apply.

## Non-blocking

1. B7: the agent chart's egress and `crates/agent/HARDENING.md:173` use 443. The hub's agent Service listens on 8444 (`deploy/charts/hub/templates/services.yaml:70`). B8: the config-server egress rule has no `namespaceSelector`. As built, an agent cannot reach the hub; this needs a human decision before the first swimlane. It is escalated rather than hidden. The blockers file proposes making the Service listen on 443 (`port: 443, targetPort: agent`), which is inside 09's own chart.
2. `.github/scripts/check-pins.mjs` misses a reference written as a folded block scalar. The reviewer probed `uses: >-` followed by `actions/setup-node@v4` on the next line and it passed. It does flag tags, branches, short SHAs, the quoted form, flow style, and `container`, `services` and `docker://` without a digest. Nothing in the repo is affected, but the "test fails if any action reference isn't a SHA" claim has this hole.
3. `deploy/policy/network.rego` does not look at a second NetworkPolicy that selects the hub's pods (self-reported). `contract.rego` compares the agent only, not the hub.
4. B9/B10: the database image (`16.15-202610050823-system-trixie`) was never read or scanned here. It is deprecated, but the human accepted it. The nightly `db-image` job is the first scan and has not run.
5. B11/B12/B13, all disclosed:
   - Nothing creates a swimlane.
   - The CI signing key, the Workload Identity pool and the Binary Authorization trust root are made by no chart. Binary Authorization is dry run only, and whether a classic attestor accepts a cosign signature is unconfirmed.
   - `RPM-GPG-KEY-lanekeeper.pub` is a marker.
   - The install test used the labelled audit stand-in. The default `LK_SENTINEL_AUDIT=repo` is the real package, and the reviewer checked that CI uses it.
   - The Rocky containers have never run on a cgroup v2 GitHub runner, and no workflow has run on GitHub.
6. The sandbox-only `build_ca` secret mounts remain in the production Dockerfiles (`crates/hub/Dockerfile`, `crates/agent/Dockerfile`, `deploy/sentinel/Dockerfile.build`). They are harmless when the secret is unset, but they are sandbox accommodation in release files.
7. `bench.yml` uses `just bench-check-rust`, which cuts the web budgets from a temp copy. The consistency count is kept and `perf/budgets.toml` is untouched. Web budgets are judged by web-e2e `perf`, and there is precedent in the per-prompt recipes.
8. `ci.yml` no longer holds the `rust`, `web` and `pinned-actions` jobs. Branch protection's required-check names need updating, and path-filtered workflows leave required checks "expected".
9. hub `HARDENING.md` and `hardening.rs` also gained `items: [uri]` on `db-credentials` and a startupProbe, beyond the A9/A10 text. This is a strengthening and inside the Extra-paths.
10. The evidence ticks T3 (hub chart) while its Cloud Armor part is unproven; the evidence text says so. The T3 verify as written is met for the hub chart.
11. Not re-run by the reviewer: `just verify` end to end (about 70 minutes; the evidence shows it fails at deny, which the reviewer reproduced), `images-scan` and `images-sbom` (3.3 GB grype database), `sentinel-test` (the full RPM install on Rocky), and the full `just images-repro` for the hub image. The `hub-registry` benches were not re-run either.

## Evidence re-run

- Commit order: `09/plan: draft`, then `0e87849 approved by a human`; the only later change to `plans/09-deployment-ci.md` is the human-merged PR 29 amendment (`7385ce2`, merge `d00df91`). T1 to T8 commits are in order, with `blocked` and `evidence` commits as on prior prompts. Pass.
- Scope: every changed path is under `deploy/**`, `.github/**`, `renovate.json`, `Justfile`, `plans/`, the threat-model section (+140 lines, 0 deleted), or the 7 approved Extra-paths files. `perf/budgets.toml`, `Cargo.*`, `deny.toml`, `crates/domain|ports|proto`, `api/`, `db/migrations/` and `docs/mcp-tools.md` are untouched, and Contract-change is none. Pass.
- `just ops-tools-check`: exit 0.
- `just ci-lint`: exit 0 (actionlint; `check-pins` 112 references in 11 files; 10 workflows follow the workflow rules; zizmor offline "No findings (20 suppressed)", which come from the one rule disabled in `.github/zizmor.yml`; shellcheck on 26 scripts).
- `just pins-test`: 78 of 78 pass.
- `just verify-09`: exit 1 at `deploy-render`. The cause is the B5 message in `cloudarmor.yaml:7`. Hub 6, 13 and 13 resources, db 2, 4 and 4, `gcp-storage` 19 and 19, agent 11 and Argo CD 7 are all valid in kubeconform.
- The recipes after it, run alone:
  - `schemas-verify`: 37 files equal the lock.
  - `deploy-lint`: 11 charts, 0 failed.
  - `policy-test`: 1002 of 1002.
  - `contract-check`: 34 of 34.
  - `env-names-test`: pass.
  - `tools-test`: pass.
  - `sentinel-check`: pass.
  - `policy-check`: exit 1, only for `gcp` with staging and prod (B5).
  - `deploy-planted`: 417 `ok`, 1 failure (the Cloud Armor group, B5).
- `cargo deny check`: exit 1, advisories FAILED (RUSTSEC-2026-0382 and RUSTSEC-2026-0386); bans, licenses and sources ok. Same as B14.
- `cargo fmt --all -- --check`: exit 0.
- `cargo xtask bench-check`, after re-running the two B15 targets: exit 1, `65 pass, 6 fail, 2 unmet of 73`. B15 rows: `P6.index_settings` 71.639 p95 against 60 ms (fail), `P12.audit_record` 3.285 p95 against 3 ms (fail). The other four failures are the P10 web budgets with no result, and the two unmet are `P7.docs_search` and `P7.feature_status`. B15 is reproduced.
- Mutation spot-check: the reviewer removed the `readOnlyRootFilesystem` rule from a scratch copy of `hardening.rego`. `conftest verify` reported 5 failures (1002 tests, 997 passed), so the tests do bind the rule.
- Sentinel unit: `systemd-analyze security --offline` (AlmaLinux 9.8 systemd 252) scores 1.7 OK, limit 2.0. `systemd-analyze verify` on Rocky 8.9 (239-78) and 8.10 (239-82) prints nothing. The unit carries all S25 directives: empty `CapabilityBoundingSet=`, `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `MemoryDenyWriteExecute`, `RestrictAddressFamilies`, dedicated user.
- Image reproducibility: `deploy/images/reproducible-check.sh agent` (two `--no-cache` builds from two `git archive` extractions, pinned BuildKit) gave `reproducible: agent sha256:bc92dc41acae73d3aca418cfd81666f37f2c58376d523f14662308d488f794af`, exit 0.
- SHA-pin and supply chain (rule 15, S19, S20):
  - Every real `FROM` has a digest, and the three base digests exist in their registries.
  - `grype.yaml` has `ignore: []` and `deny.toml` is unchanged.
  - Every workflow has a top-level `permissions` block with no write scope.
  - `id-token`, `packages: write` and `attestations: write` appear only in `images.yml` `publish`, `sentinel.yml` `publish` and `sentinel.yml` `sign-release` (the last is blocking item 2).
  - No private key or token appears in the diff apart from planted test strings.
- Spec chain: all 46 Rego test names cited in the traceability table exist in `deploy/policy/*_test.rego`. `publish.sh` runs `cosign verify` and `verify-attestation --type cyclonedx`, and `images.yml` runs `gh attestation verify`. The gaps are the kind rows (blocking item 4) and anything that needs GitHub, GCP or a cluster.
- Weakening check: no marker, refusal, test, budget or `ignore` was loosened. The B5 marker and refusal are intact and `planted_gcp` still counts its failure. The `crates/*/tests/hardening.rs` edits only add checks and planted cases. Renovate got stricter.
- Threat model: the `## Deployment and supply chain (09)` section is present, with 34 STRIDE rows, each with a Status, a Proof and an S#. It is honest about B5 (Cloud Armor "not in place") and about what never ran. Two inaccuracies: the sign-release statement (blocking item 2), and the hub egress list, which is missing the GCS host (blocking item 3). `docs/threat-model.md` has no other changes.
- Tree state: `git status` clean throughout. The reviewer deleted only `target/debug`; `target/release` and `target/images` are ignored build output it created.
