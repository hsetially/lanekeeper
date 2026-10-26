# Review of prompt 09

Rounds are appended in order. Round 1 is first, round 2 below it. Each is the reviewer subagent's output (prompts/REVIEW.md), saved by the orchestrator.

---

# Round 1

Reviewer: independent `reviewer` subagent. Branch `agent/09-deployment-ci` at `772d46a`, base `origin/main`.
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

## Evidence re-run (round 1)

Commit order, scope, `ops-tools-check`, `ci-lint`, `pins-test` (78 of 78), every `verify-09` recipe (failing only for B5), `cargo deny check` (B14), `cargo fmt`, `bench-check` (65 pass, 6 fail, 2 unmet of 73; B15 reproduced), a `readOnlyRootFilesystem` mutation spot-check (caught), the sentinel unit score (1.7, limit 2.0), agent image reproducibility, the SHA-pin and supply-chain checks, the 46 cited Rego test names, the weakening check (nothing loosened) and the threat-model section were re-run or re-checked by the reviewer. The two inaccuracies it found are blocking items 2 and 3.

---

# Round 2

Reviewer: independent `reviewer` subagent. Reviewed at `9eef6b4`, base `origin/main`. The fix round is commits 643a896, f5402a5, d7a8aaa, c88a5c9, 0032cee, df624a6, 4ffaa0d, b1ca35a, plus the merge 9eef6b4 that brings in PR 30.

VERDICT: CHANGES REQUESTED

Round-1 blocking items 1, 3 and 4 are fixed and the reviewer verified each independently. Item 2 is only partly fixed, and that is the one blocking finding below. The worktree is clean: no tracked file changed. The reviewer deleted `target/debug` (ignored, rebuildable) when its cargo runs filled the disk.

## Blocking

1. `.github/workflows/sentinel.yml:179` (job `sign-release`, `id-token: write`, `environment: release`), together with `deploy/sentinel/Dockerfile.build:72`. The claim that `sign-release` "compiles and installs nothing" is false, and so are the docs that repeat it.
   - **What the job really does.** It runs `deploy/sentinel/build.sh --tools`, which runs `docker buildx build --target rpm`. The `rpm` stage does `COPY --from=binary` (Dockerfile.build:72). So BuildKit must build the `binary` stage first: `cargo build --release --locked --target x86_64-unknown-linux-musl -p sentinel`.
   - **Proof it is in the graph.** The reviewer ran the equivalent `--target rpm` build. BuildKit resolved `rust:1.88.0-trixie` as part of it, before Docker Hub returned a 429 on the ubuntu base. Docker only resolves bases for stages that are in the target's graph.
   - **What runs in the job.** On a cold runner it runs every crates.io build script and proc-macro of the sentinel's dependency tree. It also runs `apt-get install rpm gpg gpg-agent` (apt postinst scripts as root). `gpg` and `gpg-agent` are unpinned. The same image is the one `sign.sh` later runs with the release key on stdin.
   - **Environment.** The build gets only `SOURCE_DATE_EPOCH` and `LK_VERSION` as build args. `LK_APT_MIRROR`, proxy and CA are not set in CI. Isolation is therefore only the container boundary plus the fact that nothing forwards `ACTIONS_ID_TOKEN_*`. No check enforces either.
   - **Why W6 cannot see it.** W6 (the new rule in `check-workflows.mjs`) does not look inside `build.sh`.
   - **What the docs say.** All of these state the opposite:
     - `deploy/sentinel/README.md:63`: "That job compiles and installs nothing";
     - `docs/threat-model.md:1033` and `:1069`: "no dependency's code runs next to the token";
     - `sentinel.yml:25-32`: calls the tools build "rpm and gpg from Ubuntu packages";
     - `plans/09-deployment-ci.blockers.md` round-1 section: the same wording.
   - **Failing scenario.** A compromised crates.io dependency of the sentinel runs its build script as root, with network, in a container, in the job that minutes later receives the Workload Identity credential and pipes the RPM signing key. The round-1 invariant ("jobs that hold the release key or can sign run no build step") is not met, and the threat model a human signs off says it is. Practical exploitability is low (it needs a container escape), but this was the explicit round-1 requirement and the docs are false.
   - **Violates:** S19, S20, S7 (least privilege for the signing job).
   - **Fix, either of:**
     - (a) Add a stage in `Dockerfile.build` (for example `rpm-tools`: the Ubuntu base, the apt install and `WORKDIR /build`, with no `COPY --from=binary`). Make `rpm` extend it. Point `build.sh --tools` at that target. Or build the tools image in a tokenless job and hand it over as a `docker save` artifact.
     - (b) Keep the behaviour, correct all four places to say that a cargo build and apt installs run in a build container in that job, and have a human accept it. (a) is cheaper and removes the problem.
   - Whichever is picked, update `dockerfile_artifact*.rego` and its planted cases if stage names matter to them.

## Non-blocking

1. **W6 is bypassable, and the docs do not say so for these cases.** The rule is correct for direct commands: `cargo install`, `npm ci`, chained, `sudo`, folded and flow forms are caught, and all 6 W6 tests pass. The reviewer probed it with scratch workflows (`node .github/scripts/check-workflows.mjs`):
   - Not caught: `id-token: "write"` (quoted value); `run: 'cargo install x'` (quoted run); `bash -c 'cargo …'`; `docker run … cargo`; `python3 -m pip install`; `curl | sh`; `rustup run stable cargo build`; `./script.sh`; `just build`; and a local composite action under `uses: ./.github/actions/…`.
   - Only the script and `just` cases are disclosed in the header.
   - The pre-existing W1 also passes a top-level `id-token: "write"` (quoted) in the probe.
   - Fix: strip quotes in `keyValue` for W1, W2 and W6, add fixtures, and add `sh -c` and `bash -c` handling. Or document the gaps.
2. **kind `/readyz` step is likely to give a false failure once A1 is fixed.**
   - `deploy/tests/kind/run.sh:407` reads `/readyz` through the API server's pod proxy. The hub chart's default-deny ingress on 9090 (`networkpolicy.yaml`) admits only the monitoring namespace and Google ranges.
   - kind v0.33's kindnet enforces NetworkPolicy: its RBAC lists `networkpolicies` in the `default-cni.yaml` the reviewer extracted from the node image. The API server's source address would be the control-plane node.
   - The pod's `Ready` condition already comes from a readinessProbe on `/readyz`. Drop the proxy check, or run it from a pod in the monitoring namespace. The evidence file itself lists "kindnet enforces the hub's NetworkPolicy" as unproven.
   - P11 is recorded as INFO only. Once A1 is fixed it should assert the 10 s budget per replica.
3. **`deploy/tests/kind/pins.env` is not covered by Renovate.** The `renovate.json` custom manager matches only `tools.env`. The node image, CNPG manifest URL and sha256, and operator digest will go stale and need a manual bump. The Justfile tool versions are also not covered (pre-existing). `consistency.test.mjs` checks only that the pins agree with each other and with the db chart README.
4. **Evidence file is stale for the next refresh.**
   - It still describes B14 as open. After merge 9eef6b4, `cargo deny check` and `cargo audit` both exit 0.
   - Its "Tree under test" and `Review round 1` sections cite `df624a6`, and its recorded counts predate the later commits (it quotes `pins-test` as 78 and then 92 of 92, and `policy-test` as 1002 and then 1003 of 1003).
   - It correctly leaves acceptance 1, 2 and 3 and T5, T6 and T8 partly unticked.
5. **Commit names.** Fix commits are `09/fix:`, `09/evidence:` and `09/blocked:` plus merges, not `NN/Tk:`. That matches the earlier convention. T1 to T8 order is intact.
6. **Not verifiable here.**
   - The SHA-to-tag mapping of the pinned actions. The `gh` API is refused in this session. Only zizmor's online audit in CI will check for impostor commits.
   - Any GitHub run: no workflow has run.
   - kind past cluster creation. The host is cgroup v1, and `run.sh` exits 2 with the reason.
7. **Still reported honestly; nothing here is a defect of the fixes.** B5 (`github-hook-ranges.yaml` still `githubHookRanges: []`, `cloudarmor.yaml:7` still refuses to render; `verify-09`, `policy-check` and `deploy-planted` fail only on that), A1 (the hub binary exits 1), B15 (both budgets still over), A19 (prod `isLocked: true` with the example one-year period), B7, B8, B9, B10, B11, B12, B13.

## Round-1 items 1 to 4

- **Item 1 (disk space): fixed and bound.**
  - No conflict markers or duplicated content in `ci.yml`.
  - `rust.yml` `test`, `images.yml` `image` and `kind-e2e`, and `bench.yml` and `nightly.yml` `bench` all carry "Free disk space" (the same paths as main's PR 25 step, placed before the first heavy step) and "Disk left" with `if: always()`.
  - `consistency.test.mjs` is a genuine gate: with the step removed from the `rust.yml` test job in a scratch copy, it fails ("rust.yml job test has no "Free disk space" step").
- **Item 2: partly fixed.**
  - The `cargo install`, SBOM and scan left `sign-release`.
  - The new `sbom` job holds `contents: read` only and no token. `publish` needs it and fetches its artifact. The artifact paths line up.
  - The residual docker build is blocking finding 1.
- **Item 3 (storage host): fixed.**
  - `storage.googleapis.com` is in `networkpolicy.yaml`, `network.rego` (`required_hosts`), `network_test.rego`, `fixtures_test.rego`, `HARDENING.md` and `hardening.rs`. Planted cases exist in rego and in the Rust test.
  - Removing the host from `required_hosts` in a scratch copy: `conftest verify` gives 8 failures.
  - Independent host audit of the hub crates: login.microsoftonline.com, github.com, api.github.com, secretmanager, cloudkms, www.googleapis.com (`GOOGLE_CERTS_URL`), storage.googleapis.com (`GcsCheckpointBucket::HOST`), the metadata server and the configured Teams host. Nothing else is missing.
- **Item 4 (kind e2e): fixed, red by design on A1.**
  - Real pins: `crane digest kindest/node:v1.37.0` equals `sha256:a1ed56cf…`, which is also the default node image embedded in the pinned kind v0.33.0 binary. `crane digest ghcr.io/cloudnative-pg/cloudnative-pg:1.30.1` equals `sha256:923c267e…`. The CNPG manifest downloads to sha256 `37237f14…` and 1,293,413 bytes, and names the operator image in exactly 2 places. The db image digest `sha256:c2a18065…` and `registry:3.0.0` digest `sha256:6c5666b8…` are also real.
  - The reviewer extracted the node image's `/etc/containerd/config.toml`: `version = 2` with the `io.containerd.grpc.v1.cri` key and no `config_path`, so the `containerdConfigPatches` entry has the right shape for containerd 2.3.4.
  - The script fails honestly on A1, but only when all 3 replicas log "refusing to start without them". The StatefulSet is `podManagementPolicy: Parallel`. Nothing fakes readiness, retries or skips.
  - S6 is measured: positive TLS 1.3 control, `sslmode=disable` rejected by `pg_hba`, and a TLS 1.2-only client refused. P11 time to Ready is measured (INFO).
  - The job needs only `contents: read`, and `publish` does not wait for it.

## Evidence re-run (round 2)

- `just ci-lint`: exit 0 (`check-pins` 122 references in 11 files; 10 workflows follow W1 to W6; zizmor offline, no findings, 20 suppressed from the one rule disabled in `.github/zizmor.yml`; shellcheck 27 scripts).
- `pins-test` 92 of 92; `ops-tools-check`, `images-policy-test`, `images-builder-test`, `images-publish-test`, `sentinel-check`, `env-names-test`, `tools-test`: exit 0.
- `policy-test` 1003 of 1003; `contract-check` 34 of 34 (the db, gcp and hub charts agree for staging and prod); `deploy-lint` 11 charts, 0 failed; `schemas-verify` 37 files equal the lock.
- `just verify-09`: exit 1 at `deploy-render`, for B5 only (`cloudarmor.yaml:7`). Hub, db, `gcp-storage`, agent and Argo CD all valid in kubeconform. `policy-check` exit 1 only for the gcp chart with staging and prod; `deploy-planted` 417 ok, 1 failed (the Cloud Armor group), all B5.
- `cargo test --locked -p hub --test hardening` 5 of 5; `-p agent --test hardening` 5 of 5; `cargo clippy --locked -p hub --tests -- -D warnings` exit 0; the `threat_model` tests of engine, agent, hub-identity, hub-platform, hub-registry, hub-writes and hub-mcp pass; `cargo fmt --all -- --check` exit 0.
- `cargo deny check`: exit 0 (one unmatched-license-exception warning for `libfuzzer-sys`). `cargo audit`: exit 0.
- `cargo xtask bench-check`: exit 1, 65 pass, 6 fail, 2 unmet of 73, run against the existing criterion results and not re-measured; identical to round 1 (`P6.index_settings` 71.639 ms p95 against 60 and `P12.audit_record` 3.285 ms against 3, both B15; four P10 web budgets with no result; `P7.docs_search` and `P7.feature_status` unmet; `P11` 41.168 ms against 10000, `P11.memory_mib` 746.5 against 1536).
- Mutation checks in scratch copies: `storage.rego` lock rule inverted (5 failures); `topology.rego` replicas changed (2); `required_hosts` without the storage host (8); `rust.yml` test job without the disk step (`consistency.test.mjs` red).
- Scope and weakening: `git diff origin/main...HEAD` touches only `deploy/**`, `.github/**`, `renovate.json`, `Justfile`, the plan files, the 7 approved Extra-paths files, and 140 added lines in `docs/threat-model.md` (no deletions). `Cargo.*`, `deny.toml`, `.cargo`, `perf/`, `docs/decisions.md`, the contract paths and `web/` are identical to `origin/main`. Since f6a0469 no marker, refusal, test, budget or ignore was removed.
- Spec chain: plan approved at `0e87849`; the only later plan change is the human-merged PR 29 amendment `7385ce2`. All 32 test names in the traceability table exist in `deploy/policy/*_test.rego`.
- Not run: `just verify` end to end (by instruction), `just fuzz-smoke`, `images-repro`, `images-scan` and `images-sbom` (the Dockerfiles are unchanged since round 1), `sentinel-test`, `kind-e2e` past its cgroup v1 preflight, and any GitHub-side step.
