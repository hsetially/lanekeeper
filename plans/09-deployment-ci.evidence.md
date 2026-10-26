# Evidence 09: Deployment, CI and supply chain

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proved by output shown here. Where an item is not ticked, the blocker id is next to it.

**Status of this file: PARTIAL (second commit).** `PENDING` marks the benchmark run (`cargo bench --workspace`, then `cargo xtask bench-check`) and the one-command `just verify`, which were still running when this was committed. A `PENDING` that is still in the last commit means that step did not finish; the text says why.

Tree under test: branch `agent/09-deployment-ci`, code head `d164637` (the commits after it only add `plans/09-deployment-ci.*`; `git status` was clean before every run and no tracked file was changed by any recipe). Run on 2026-10-26 in the sandbox: Docker 29.9.2 daemon running, helm 4.3.0 and the other pinned tools in `target/ops-tools/bin`, 4 vCPUs. Raw logs are in the session scratchpad, not in the repository.

## Read this first: what is open, and why

| Id | State | What it stops in this evidence |
|---|---|---|
| **B5** | **open, by the human's decision** | `deploy/values/hub/github-hook-ranges.yaml` is the unresolved marker (`githubHookRanges: []`) and the gcp chart refuses to render. `just verify-09` therefore fails at `deploy-render`, and `policy-check` and `deploy-planted` fail for the same reason and no other. No range was typed in, copied or swapped in, even for one run, and no marker, refusal or test was changed. |
| **A1** | open (needs a human to assign) | The hub binary exits 1 (`crates/hub/src/main.rs`) until 03a and 05 are composed into it, and prompt 09 owns no application code. `/readyz` cannot pass on any replica. |
| kind job | **not written** (deviation) | `deploy/tests/kind/run.sh`, the `kind-e2e` job and the `kind-e2e` recipe do not exist (T5's row lists no file for them; `grep kind-e2e Justfile` and `ls deploy/tests` show it). Acceptance 3 is not proven. |
| **B14** | **open, new in this step, needs a human** | `cargo deny check` and `cargo audit` fail on three RustSec advisories published 2026-10-13 to 2026-10-20 (`rustls-webpki` 0.103.15, `webpki-root-certs` 1.0.9, `tokio` 1.53.1). Every fixed release is younger than rule 15's 14 days (6, 7 and 13 days), so the fix and the dependency rule conflict. Nothing was bumped or ignored. This branch changed none of `Cargo.lock`, `Cargo.toml`, `deny.toml`, `.cargo/audit.toml`. `just verify` cannot be green until a human decides. Recorded in `plans/09-deployment-ci.blockers.md`, commit `ee694a2`. |
| **B9** | open, informational | `pkg-containers.githubusercontent.com` is refused, so the database image (`ghcr.io/cloudnative-pg/postgresql:16.15-202610050823-system-trixie`) was pinned by a digest read from the registry, but its layers were never read or scanned. |
| unrun workflows | open | No workflow under `.github/` has run on GitHub. Nothing that needs GitHub's OIDC, Sigstore, Workload Identity Federation or `api.github.com` was exercised. |
| **B13** | open, informational | The RPM release key and its Workload Identity trust root do not exist (`RPM-GPG-KEY-lanekeeper.pub` is an unresolved marker). The Rocky and Debian hosts are refused, so the install test ran with a labelled stand-in for the `audit` package. |
| **B11** | open, informational | Nothing creates a swimlane in the hub; step 1 of `deploy/ADDING-A-SWIMLANE.md` describes a contract, not something that can be followed. |
| **B12** | open, informational | The CI signing key, the Workload Identity Federation pool and the IAM around them are made by no chart; whether a classic Binary Authorization attestor accepts a cosign signature is unconfirmed; the policy is per project and in dry run. |
| B7, B8 | open, informational | The agent dials the hub on 443 while the hub's agent load balancer listens on 8444; the config-server egress rule selects pods in the agent's own namespace. |
| B10 | **decided by a human** | Keep the interim database image (`system` variant, built-in barman). Migration to the Barman Cloud plugin is a later open question. |
| B1 to B4 | resolved | Model lock filled from a download made here (B1); the scan runs (B2); the images moved to Debian 13 by plan amendment A32 (B3); helm 4.3.0 pinned from a download made here (B4). |

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-09` | **FAIL, exit 1, 40 s. B5 only.** It stops at `deploy-render`; every recipe before it passed. The recipes after it were run on their own (next section). | `error: Recipe 'deploy-render' failed on line 914 with exit code 1`, after kubeconform reported hub 6, 13, 13; db 2, 4, 4; `gcp-storage` 19, 19; agent 11; `deploy/argocd` 7 resources valid, and for `gcp` with staging and with prod: `Error: execution error at (lanekeeper-gcp/templates/cloudarmor.yaml:7:4): cloudArmor.githubHookRanges is empty: GitHub's webhooks would be denied (P2, Q28). Run 'node deploy/tools/github-hook-ranges.mjs --write' and commit deploy/values/hub/github-hook-ranges.yaml` |
| `just verify` | PENDING as one command (see "`just verify`, step by step" for each of its steps, run on its own). **It cannot pass: `cargo deny check` and `cargo audit` fail (B14).** | steps: fmt pass, clippy pass, `cargo test --workspace` pass (5,434 passed, 0 failed, 20 ignored), deny **fail**, audit **fail**, web lint pass, typecheck pass, test pass on the second run (one failure in the first, see below), build pass |
| `cargo xtask bench-check` | PENDING | |

## `verify-09`, each recipe run on its own (so the evidence shows which pass and which fail only because of B5)

`verify-09` is `ops-tools-check ci-lint pins-test images-policy-test images-builder-test images-publish-test deploy-check sentinel-check`, and `deploy-check` is `schemas-verify deploy-lint deploy-render policy-test policy-check contract-check deploy-planted env-names-test tools-test`. `just` stops at the first failure, so each recipe was run alone, in that order, from the committed tree. The time is the recipe's own, with its prerequisites.

| Recipe | Exit | Time | Result |
|---|---|---|---|
| `ops-tools-check` | 0 | 0 s | every pinned tool at its version; every image in `deploy/images/tools.env` pinned by digest |
| `ci-lint` | 0 | 4 s | actionlint clean; `check-pins.mjs`; `check-workflows.mjs`; zizmor 1.30.1 **offline** (its online audits cannot run here) `No findings to report. Good job! (20 suppressed)`; `ci-lint: 26 shell scripts checked` (shellcheck 0.11.0) |
| `pins-test` | 0 | 2 s | `node --test .github/scripts/*.test.mjs`: `# pass 78`, `# fail 0` |
| `images-policy-test` | 0 | 18 s | model-lock tests 25 of 25; Rego unit tests 22 of 22; the planted Dockerfile triggers every expected rule; both real Dockerfiles pass the policy (30 of 30); `cargo test -p hub --test hardening` 5 of 5; `cargo test -p agent --test hardening` 5 of 5; the model-stage test accepts the matching lock and refuses every planted mismatch; `scan-db-test`; the committed lock passes (`5 files of BAAI/bge-small-en-v1.5 at 5c38ec7c405ec4b44b94cc5a9bb96e735b38267a`) |
| `images-builder-test` | 0 | 5 s | pinned BuildKit v0.34.0 starts and writes the same OCI archive twice with `rewrite-timestamp=true` (`sha256:e990e35d...` twice) and different ones without it |
| `images-publish-test` | 0 | 6 s | `publish.sh` against a registry on localhost with a throwaway key: push by digest, sign, SBOM attestation, verify; another key, an unsigned image and a missing attestation do not verify |
| `schemas-verify` | 0 | 0 s | `schemas: 37 files equal the lock` |
| `deploy-lint` | 0 | 1 s | `helm lint --strict` of 11 chart and values pairs, each `1 chart(s) linted, 0 chart(s) failed` |
| `deploy-render` | **1** | 2 s | **B5 only.** kubeconform `-strict`, offline: hub (kind, staging, prod) 6, 13, 13 resources valid; db 2, 4, 4; `gcp-storage` (the gcp chart without Cloud Armor) 19, 19; agent (sit1) 11; `deploy/argocd` 7 in 5 files; all `Invalid: 0, Errors: 0, Skipped: 0`. Failed: `gcp` with staging and with prod, with the B5 message (`render.sh: kubeconform_one FAILED for the gcp chart with the staging values`, and the prod values). |
| `policy-test` | 0 | 8 s | `conftest verify`: `1002 tests, 1002 passed, 0 warnings, 0 failures, 0 exceptions, 0 skipped` |
| `policy-check` | **1** | 20 s | **B5 only.** conftest passes (0 failures) on hub (kind `266 passed, 4 warnings`, the fallbacks kind is allowed; staging and prod `270 passed`, with the placeholder-digest warning of A29 in the separate images run), db (kind `267 passed, 3 warnings`, its fallbacks; staging and prod `270 passed`), `gcp-storage` (staging and prod `270 passed`, with Binary Authorization's dry-run warning), agent (`270 passed`, placeholder-digest warning), the Argo CD policy on the platform's files (`43 passed, 3 warnings`: the placeholder `targetRevision`) and on the swimlane's (`45 passed, 1 warning`), the files each Application names, and the scope (`3 tests, 3 passed`) of hub, db, `gcp-storage` and agent against their projects. Failed: `gcp` with staging and prod (`render.sh: policy_one FAILED`), and the scope of `gcp` with prod (`render.sh: argocd scope FAILED for the gcp chart with the prod values`), each with the B5 message. |
| `contract-check` | 0 | 2 s | `contract-extract.sh --self-test`; `contract agent (sit1)`: `34 tests, 34 passed`; the db, gcp and hub charts agree (staging and prod) |
| `deploy-planted` | **1** | 431 s | **B5 only.** 417 planted cases report `ok`. The single failure is the Cloud Armor group, which prints `(the gcp chart does not render with the committed hook ranges, so the Cloud Armor cases are not run: ...)` and ends with `render.sh: 1 planted case(s) failed`. |
| `env-names-test` | 0 | 0 s | every `LK_*` variable of the hub chart is read by `crates/hub/src/config.rs`, and of the agent chart by `crates/agent/src/config.rs`; the self-test refuses a misspelt and a secret-looking name |
| `tools-test` | 0 | 6 s | `node --test deploy/tools/*.test.mjs`: `# pass 30`, `# fail 0` |
| `sentinel-check` | 0 | 11 s | 20 of 20 tests of the unit-file parser; `conftest verify` of the `systemd` (S25) and `dockerfile_artifact` namespaces `97 tests, 97 passed`; the policies on the real unit file `23 tests, 23 passed` and on the real `Dockerfile.build` `14 tests, 14 passed`; the planted Dockerfile checked rule by rule through the real parser; a unit with `CapabilityBoundingSet=CAP_SYS_ADMIN` refused through the real parser. Last line: `sentinel-check: the unit passes the S25 policy, Dockerfile.build passes its policy, and the planted cases are refused` |

Recipes that pass: 13 of 16. Recipes that fail: `deploy-render`, `policy-check`, `deploy-planted`, **each only on the gcp chart with Cloud Armor, that is, B5**. Every other chart, values file, policy, planted case and script test passes.

What B5 leaves unproven: kubeconform, conftest and the Argo CD project scope on **one render that holds Cloud Armor and the buckets and the admission resources together**, and the three Cloud Armor planted cases. No rule reads both, so the gap is small, but it is a gap.

## The other recipes of this prompt's tasks (not in `verify-09`; they need Docker and are never skipped)

All run in this step on `ee694a2`, with the sandbox's build settings (`LK_BUILD_NETWORK=host`, `LK_BUILD_CA=/root/.ccr/ca-bundle.crt`; the egress proxy is the sandbox's, CI sets none).

| Recipe | Exit | Time | Result |
|---|---|---|---|
| `just images-repro` (T1 Verify) | 0 | 24 min 50 s | each image built twice with no cache from two `git archive` extractions; both builds agree: `reproducible: hub sha256:cc4f00a89a61983d68630f65a0127914ee8196b3403c6c9f29acb35af9392322` and `reproducible: agent sha256:f49282a08036bfb33cac677a9ac54b8b56d599c2318bfedf0982aa25f48fb28a` (commit `ee694a2`, `SOURCE_DATE_EPOCH=1792982471`). A later `just images-build` of both images from the cache gave the same two digests. The digests depend on the commit, so they differ from the ones in `REPRODUCIBILITY.md` (measured at `e2c199d`). |
| `just images-sbom hub`, `agent` | 0, 0 | 5 s, 3 s | CycloneDX: `hub.cdx.json` 1,723 components, `agent.cdx.json` 1,593 (the same counts as at T1) |
| `just images-scan hub`, `agent` | 0, 0 | 7 s, 5 s | grype v0.120.1 (database downloaded in this run) over the image archive and over its merged SBOM: `scan.sh: hub has no critical findings`, `scan.sh: agent has no critical findings`; per image 0 critical and 11 high in each pass, all `(won't fix)` or without a fix upstream, the Debian 13 base's; `deploy/images/grype.yaml` has `ignore: []` |
| `just images-scan-test` | 0 | 3 s | `scan-test: the planted critical finding (CVE-2021-44228) fails the scan` |
| `just sentinel-test` with `LK_SENTINEL_AUDIT=stand-in` (T8 Verify) | 0 | 2 min 31 s | 124 `ok` lines, no failure. Rocky Linux 8.9 (systemd 239-78.el8) and 8.10 (239-82.el8): `rpm -K refuses the package while the key is unknown`, `rpm -K verifies the package with the key`, `rpm -K refuses a package with a changed byte`; dnf refuses the unsigned and the changed package, installs the signed one, `systemd-analyze verify prints nothing`, `systemctl start runs the packaged binary under the restrictions`, `CapInh/CapPrm/CapEff/CapBnd/CapAmb is empty`, `NoNewPrivs is 1`, `Seccomp: 2`; removal and reinstall; SBOM 2 components, grype `No vulnerabilities found`; **exposure score 1.7 OK** (limit 2.0, scored offline by systemd 252 of AlmaLinux 9.8, the self-test refuses every worse unit); cosign `sign-blob` with a key. **The first banner of the install step is `install.sh: AUDIT STAND-IN IN USE. The package 'audit' below is tests/audit-stand-in.spec, not the real one, because the Rocky repositories cannot be reached from here (plan 09 blockers, B13). The real audit package is tested ...` (the line goes on)**. The signing key is a throwaway made for the run, not the release key. |
| `just sentinel-test` with the default `LK_SENTINEL_AUDIT=repo` and none of the sandbox settings | 1 | 5 s | failed at `sentinel-build`: the build container could not reach `static.rust-lang.org` through the sandbox's proxy (`curl: (7) Failed to connect to 127.0.0.1 port 34627`). That was the missing sandbox settings, not the repository; the run above has them. The `repo` mode (what CI does: `dnf install audit` from the Rocky repositories) has never run, because those hosts are refused here (B13). |

## `just verify`, step by step

`verify` is `rust-verify web-verify`; `rust-verify` is `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo audit`; `web-verify` is `cd web && pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test --run && pnpm build`. Run one step at a time because of the disk (the sandbox had 2.4 to 11 GB free). **For the same reason the Rust build used `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` for the test step** (no debug info in the test executables: same tests, same assertions, same `debug-assertions`); clippy ran with the default profile.

| Step | Exit | Time | Result |
|---|---|---|---|
| `cargo fmt --all -- --check` | 0 | seconds | no output |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 6 min 18 s (379 s) | `Finished dev profile [unoptimized + debuginfo] target(s) in 6m 18s`; no warning |
| `cargo test --workspace` | 0 | 50 min 33 s, compile included | 352 test binaries and doc-tests: **5,434 passed, 0 failed, 20 ignored, 0 filtered out**; no line says `SKIPPED`; the database tests ran against `pgvector/pgvector:pg16` containers started by testcontainers. The 20 ignored carry their own reasons (the target-scale and `registry-slow-test` runs, five that need `LK_REAL_BASE_DIR`, three that need a real Google Cloud project, four measurements run by hand with `--ignored`, one run by another test in a child process, `agent-slow-test`). |
| `cargo deny check` | **1** | 6 s | **B14.** `advisories FAILED, bans ok, licenses ok, sources ok`; `error[vulnerability]: Certificate verification may accept distrusted intermediates` (RUSTSEC-2026-0382, `rustls-webpki` 0.103.15) and `error[unsound]: AsyncFd::new and related constructors violate I/O safety` (RUSTSEC-2026-0386, `tokio` 1.53.1); warnings only for duplicate versions and the unused `libfuzzer-sys` licence exception |
| `cargo audit` | **1** | 5 s | **B14.** `Loaded 1352 security advisories`, `Scanning Cargo.lock for vulnerabilities (641 crate dependencies)`, RUSTSEC-2026-0382 (`rustls-webpki`), RUSTSEC-2026-0383 (`webpki-root-certs` 1.0.9), `error: 2 vulnerabilities found!` and `warning: 1 allowed warning found` (RUSTSEC-2026-0386, tokio, unsound) |
| `pnpm install --frozen-lockfile` | 0 | 13 s | `Done in 11.7s using pnpm v10.34.6` |
| `pnpm lint` | 0 | 135 s | eslint, no output |
| `pnpm typecheck` | 0 | 50 s | `tsc -b`, no output |
| `pnpm test --run` | **1, then 0** | 514 s, then 384 s | **first run: `Test Files 1 failed | 389 passed (390)`, `Tests 1 failed | 9675 passed (9676)`**: `scripts/perf/chromium-process.test.mjs > a Chromium that starts > ends the whole process group on stop(): ...` failed at `expect(alive(helper)).toBe(true)`. This is 08's test, not 09's. Not reproduced: the file passed alone 3 of 3 times, 12 of 12 times with eight busy loops on the 4 vCPUs, and the **second complete run passed: `Test Files 390 passed (390)`, `Tests 9676 passed (9676)`**. The root cause was not found. A hypothesis, not proved: the test reads `helper.pid` once the file exists, which is also true while the shell has created it and not yet written it (`Number("")` is 0, and `alive(0)` is false). Under the load of the first run (a `cargo install` was compiling at the same time) that window may have opened. It is recorded here, not fixed, because the file is not 09's. |
| `pnpm build` | 0 | 72 s | `built in 24.00s`; `initial JavaScript: 195.9 KB gzip (1 files, 622 KB raw), budget 250 KB`; `dist-check: 158 files scanned, nothing the CSP would refuse` |

**What `just verify` as one command does:** PENDING (a one-command run, with the same reduced-debug-info environment, to show the real stop). By the order of its recipe lines it stops at `cargo deny check` and never reaches `web-verify`.

## The threat-model tests of the seven crates, one crate at a time

`cargo test -p <crate> --test threat_model` for every crate that has one (they parse `docs/threat-model.md`, so T7's new section had to leave them passing; T7 could not run the five other crates' tests for lack of disk). All exit 0, with `CARGO_INCREMENTAL=0` and no debug info. They also ran inside `cargo test --workspace`, with the same counts.

| Crate | Tests | Result | Time |
|---|---|---|---|
| `agent` | 6 | `test result: ok. 6 passed; 0 failed` | 60 s (compile included) |
| `engine` | 5 | `ok. 5 passed; 0 failed` | 17 s |
| `hub-identity` | 7 | `ok. 7 passed; 0 failed` | 41 s |
| `hub-platform` | 8 | `ok. 8 passed; 0 failed` | 48 s |
| `hub-registry` | 9 | `ok. 9 passed; 0 failed` | 72 s |
| `hub-writes` | 11 | `ok. 11 passed; 0 failed` | 63 s |
| `hub-mcp` | 11 | `ok. 11 passed; 0 failed` | 46 s |

PENDING: the benchmark run and `cargo xtask bench-check` (below), and the one-command `just verify`.

## Acceptance checklist

The prompt's three acceptance items, then each task's Verify line. **Nothing is ticked without the output above.**

### The prompt's acceptance

- [ ] **`just verify-09` passes: lint, kubeconform, conftest and the SHA-pin test.** NOT MET, **B5**. Exit 1 at `deploy-render`. What passes, from the table above: lint (`ci-lint` exit 0), the SHA-pin test (`pins-test` 78 of 78), conftest (`policy-test` 1002 of 1002, `policy-check` on every chart but the gcp chart with Cloud Armor), kubeconform on every chart but that one.
- [ ] **Images are signed, and their SBOMs and provenance attestations are published.** NOT PROVEN. The SBOMs are made and scanned (`images-sbom`, `images-scan`: 1,723 and 1,593 components, no critical finding), and `publish.sh` signs, attaches an SBOM attestation and verifies, against a registry on localhost with a throwaway key (`images-publish-test` exit 0). Nothing was published: no workflow has run on GitHub, so keyless signing, `actions/attest-build-provenance`, the Cloud KMS signature through Workload Identity Federation and `gh attestation verify` never ran. B12 (the CI signing key and its trust root are made by no chart; the Binary Authorization attestor may not accept a cosign signature).
- [ ] **A kind cluster in CI runs the hub chart with CloudNativePG, and `/readyz` passes on all 3 replicas.** NOT MET and NOT PROVEN. **A1:** the hub binary exits 1, so `/readyz` cannot pass. The kind job and its script were never written (deviation, above), and the CloudNativePG operator image is on a host that is refused here (B9). What exists is the kind values files and the charts' kind variants, which render and pass kubeconform and the policies (`deploy-render`, `policy-check`).

### Each task's Verify

- [x] **T1: images build reproducibly.** `just images-repro` exit 0 on `ee694a2`: hub `sha256:cc4f00a8...`, agent `sha256:f49282a0...`, each built twice with no cache. Proof: the output above. Not proven: the reproducibility of the pinned CI builder (`builder.sh` on a GitHub runner) and of a build with another BuildKit.
- [x] **T1: per image a CycloneDX SBOM and a scan that fails on critical findings.** `images-sbom` (1,723 and 1,593 components), `images-scan` (no critical finding in either), `images-scan-test` (the planted CVE-2021-44228 fails the scan). The database image is not scanned (**B9**); the nightly `db-image` job is the first scan of it and has not run.
- [x] **T2: `actionlint` passes.** `ci-lint` exit 0 (actionlint, zizmor offline, pin check, workflow rules, shellcheck on 26 scripts).
- [x] **T2: a test fails if any action reference is not a SHA.** `pins-test` 78 of 78, including `bad-tag.yml is refused`, `bad-branch.yml is refused`, `bad-short-sha.yml is refused`, `bad-docker-tag.yml is refused`, `bad-docker-sha-tag-only.yml is refused` and `unpinning any single reference of a real workflow fails the check`. Not proven: that the workflows run on GitHub (none has), zizmor's online audits, `osv-scanner` (`api.osv.dev` is refused here; it has never completed a scan).
- [x] **T3 (hub chart): kubeconform passes and the policy tests reject a pod missing any hardening field.** kubeconform hub 6, 13, 13 valid; `policy-test` 1002 of 1002 (`hardening_test.rego` 101 tests, one planted case per field: `test_deny_run_as_non_root_missing`, `test_deny_seccomp_missing`, `test_deny_privilege_escalation_missing`, `test_deny_writable_root_missing`, `test_deny_added_capability`, `test_deny_limits_memory_missing`, `test_deny_token_automount_missing`, ...). **The Cloud Armor part of T3 (the gcp chart, the hook ranges) is not proven: B5.**
- [x] **T4: kubeconform passes and the Role has no Secret list.** agent kubeconform 11 of 11; `rbac_test.rego::test_deny_secret_list`, `test_deny_secret_watch`, `test_deny_unnamed_secret`, `test_deny_clusterrole` inside the 1002; `contract-check` 34 of 34 (the rendered Roles equal `crates/agent/RBAC.md`). B7 and B8 (two contract files that disagree) are open and no check sees them.
- [ ] **T5: `helm template` plus kubeconform plus conftest on the db and gcp charts; the policy tests reject a public, non-CMEK or unlocked-in-prod bucket and a cluster without TLS.** PARTIAL, not ticked. Proven: the db chart (2, 4, 4 valid; policies pass), the buckets, keys and IAM of the gcp chart rendered without Cloud Armor (`gcp-storage` 19 and 19 valid; policies pass), and the policy tests (`storage_test.rego::test_deny_bucket_without_cmek`, `test_deny_unlocked_audit_bucket_in_prod`, `test_deny_missing_retention`; `database_test.rego::test_deny_tls12_min`, `test_deny_hostnossl_allowed`). Not proven: **the gcp chart as a whole (B5)**; the database image's contents (**B9**); WAL archiving (**B10**, built-in barman, deprecated); a restore.
- [ ] **T6: `helm template` with every values file, then conftest for hardening and network rules.** PARTIAL, not ticked. Every chart and values file passes except the gcp chart with Cloud Armor (staging and prod): **B5**. Argo CD files: kubeconform 7 of 7, policy and scope pass. Binary Authorization is in dry run and its cosign path is unconfirmed (**B12**); `targetRevision` and the image digests are placeholders (A29).
- [x] **T7: the threat-model section.** Mechanical checks I ran: the `## Deployment and supply chain (09)` section has a STRIDE table of 34 rows, each with a Proof cell and an S# cell; 219 backticked test, file and policy names in the Proof cells were each searched for in the repository (`deploy/`, `.github/`, `crates/`, `Justfile`, `renovate.json`): 216 were found as text, 2 are the file `deploy/policy/systemd_test.rego`, which exists (my search looked for its name inside files), and 1 is a runtime directory (`/run/lanekeeper-audisp`), so none is missing; the seven crates' `threat_model` tests pass (above). The check by a reviewer ("every STRIDE row names an S# and a test or policy") is still the reviewer's.
- [ ] **T8: the RPM installs and starts on Rocky Linux 8.9 and 8.10 containers.** PARTIAL, not ticked, **B13**: it installs and starts on both (`install.sh: the package installs, verifies and starts on Rocky Linux 8.9 8.10`), but with a stand-in for the `audit` package it depends on, because the Rocky repositories are refused here. The real `audit` package, the audisp plugin creating the socket, and SELinux were never run (A17).
- [x] **T8: `systemd-analyze security` gives an exposure score of 2.0 or lower.** `exposure.sh`: `Overall exposure level for lanekeeper-sentinel.service: 1.7 OK`, scored offline by systemd 252, and every directive of the unit is read by the systemd 239 of Rocky 8.9 and 8.10 (`systemd-analyze verify prints nothing`). Caveat in A15: Rocky 8 cannot score it itself.
- [ ] **T8: the RPM's signature verifies.** PARTIAL, not ticked, **B13**: `rpm -K` verifies the package with the throwaway key of the run and refuses it while the key is unknown and when a byte changed. The release key does not exist (`RPM-GPG-KEY-lanekeeper.pub` is an unresolved marker), so the signature of a release has never been made or verified; keyless `cosign sign-blob` of the RPM has not run.

## Security requirements implemented

Where in code, and the named tests that fail if the protection is removed (all these tests exist and pass in `policy-test`, `pins-test`, `images-policy-test`, `sentinel-check` and `sentinel-test` above). Rego test files are in `deploy/policy/`.

| S# | Where | Tests | Not proven |
|---|---|---|---|
| S6 TLS 1.3, mTLS, Postgres verify-full | `deploy/charts/db` (`ssl_min_protocol_version`, `pg_hba`), the hub chart's `items: [ca.crt]` db-ca mount | `database_test.rego::test_deny_tls12_min`, `test_deny_hostnossl_allowed`; `hardening_test.rego::test_db_ca_only_ca_crt` | the kind check that `psql sslmode=disable` is rejected (no kind job) |
| S7 no Kubernetes Secrets but the agent cert Secret and CNPG's; secrets from Secret Manager | `deploy/policy/secrets.rego`, the charts | `secrets_test.rego::test_deny_secret_object`, `test_deny_env_secretkeyref`, `test_deny_secret_in_values` | the Secret Manager containers and bindings are the hub's configuration (A1) |
| S9 locked audit checkpoint bucket | `deploy/charts/gcp/templates/buckets.yaml`, `storage.rego` | `storage_test.rego::test_deny_unlocked_audit_bucket_in_prod`, `test_deny_missing_retention` | prod's lock and a one-year period are an EXAMPLE a person must confirm (A19, deviation below) |
| S10 CMEK on buckets and disks, encrypted backups | the gcp chart, the hub, db and agent `StorageClass`es | `storage_test.rego::test_deny_bucket_without_cmek`, `test_deny_pvc_without_cmek_class`, `test_deny_backup_without_cmek_bucket` | the keys are referenced, not made (T5 decision 5); the fallback to Google-managed keys warns |
| S16 container hardening, distroless non-root images | the hub and agent charts, `crates/{hub,agent}/Dockerfile`, `dockerfile.rego` | `hardening_test.rego` (one test per field, listed in T3 above); `dockerfile_test.rego::test_deny_tag_only_base`, `test_deny_non_distroless_final`, `test_deny_missing_user`, `test_deny_root_user`; `cargo test -p hub -p agent --test hardening` 5 and 5 | the cluster's admission of the pods (no apply ever ran) |
| S17 agent: minimal RBAC, no Secret list, egress limited | `deploy/charts/agent`, `rbac.rego`, `contract.rego` | `rbac_test.rego::test_deny_secret_list`, `test_deny_secret_watch`, `test_deny_unnamed_secret`, `test_deny_clusterrole`; `contract_test.rego` (Roles equal RBAC.md); `network_test.rego::test_agent_egress_exact` | B7, B8 |
| S18 default deny, egress allowlist, LB source ranges, webhook ranges | `deploy/charts/hub` (NetworkPolicy, FQDN policy, Service ranges), `network.rego` | `network_test.rego::test_deny_without_default_deny`, `test_hub_egress_exact`, `test_deny_empty_source_ranges`, `test_deny_world_source_range`, `test_hooks_ranges_only_on_hooks_path`, `test_hooks_route_exists` | **Cloud Armor with GitHub's hook ranges (B5)**; enforcement on a real cluster; a second `NetworkPolicy` selecting the hub's pods is not looked at (`network.rego` gap, T5 note) |
| S19 pinned actions and images, `--locked`, deny/audit/osv, Renovate 14 days | `.github/scripts/check-pins.mjs`, `renovate.json`, `dockerfile.rego`, `images.rego` | `pins-test` (78); `images_test.rego::test_deny_tag_only_image`; `dockerfile_test.rego::test_deny_unlocked_build`, `test_deny_unlocked_install`, `test_deny_unlocked_cook` | `cargo deny` and `cargo audit` **fail (B14)**; `osv-scanner` has never completed a scan; no workflow has run |
| S20 SBOM, signing, provenance, Binary Authorization, scan blocks critical | `deploy/images/{sbom,scan,publish}.sh`, `.github/workflows/images.yml`, `charts/gcp/templates/binauthz.yaml` | `images-scan-test` (CVE-2021-44228 fails the scan); `images-publish-test`; `binauthz_test.rego::test_deny_allow_all_default` | keyless signing and provenance (never ran), Binary Authorization enforcement (dry run, B12), the database image (B9) |
| S25 sentinel packaging and systemd (support) | `deploy/sentinel/*`, `systemd.rego`, `dockerfile_artifact.rego` | `systemd_test.rego` (72), `dockerfile_artifact_test.rego` (25), `sentinel-test` (exposure 1.7, `CapBnd` empty) | the real `audit` package, the plugin and SELinux (B13, A17) |
| S5, S22 (support) | Workload Identity annotations on the KSAs; fuzz matrix in `rust.yml` and `nightly.yml` | the render and the policies | the fuzz jobs and the keyless join have not run |

## Benchmarks against budgets

Prompt 09 adds no benchmark and no budget (`perf/budgets.toml` is unchanged). Its performance items are: **P11** (resources): `resources_test.rego::test_hub_memory_request_matches_p11` reads `perf/budgets.toml` and ties the hub's memory request to the budget (it passes in the 1002); the budgets themselves are the existing hub benches `hub/startup` and `hub/memory_at_design_scale_mib`; **P2** (webhook route): `network_test.rego::test_hooks_route_exists` and `test_hooks_ranges_only_on_hooks_path`; the timing is 05's `hub-registry/head_moved_to_drift` and 03b's `hub-platform/push_to_drift_webhook`.

### How the benchmark run was made

The first attempt, `cargo bench --workspace --no-fail-fast` as one command, was killed by a container restart. The restart also stopped the Docker daemon. The result files it had written (31 files in `target/perf-results` and 6 groups in `target/criterion`, written 04:06 to 04:34 UTC on 2026-10-26, the last ones from `hub-registry`) have no log (none survived in the scratchpad) and were **not used**. They were moved to `target/bench-logs/killed-run-results/` so that `cargo xtask bench-check` can only read numbers from the run below.

The run below is the same benchmarks in 44 pieces that can be resumed. The workspace has 44 `[[bench]]` targets in 8 packages (`cargo metadata`); no `#[bench]` function exists (`grep -rn '#\[bench\]' crates` finds none, and the libtest `#[bench]` attribute is unavailable on stable Rust 1.88), so the lib and bin harnesses that `cargo bench --workspace` also builds run no benchmark, and the 44 targets are all of them. Each target is one command, `cargo bench -p <package> --bench <target> --no-fail-fast`, run **one after the other** (never in parallel, so that no timing disturbs another), by `target/bench-logs/run-benches.sh <package>`. The script writes `target/bench-logs/<package>__<target>.log` and, only when the command has ended, `<package>__<target>.exit` holding `<exit code> <seconds>`. A target with an `.exit` file is skipped, so a restart resumes at the first target that did not finish. Before the first target, `target/criterion` and `target/perf-results` were moved aside (above), `dockerd` was started with its default settings (the registry and writes benches start Postgres 16 with pgvector through testcontainers), and `git status` was clean. 4 vCPUs, 16 GB, Rust 1.88.0, the `bench` profile (`release`: thin LTO, one codegen unit). The seconds of the first target of a package include the compile. Raw logs are under `target/bench-logs/` (ignored by git), not in the repository.

<!-- bench-pieces:start -->
| Package | Bench target | `cargo bench -p ... --bench ... --no-fail-fast` exit | Seconds |
|---|---|---|---|
| agent | full_rehash | 0 | 290 |
| agent | idle_traffic | 0 | 36 |
| agent | oob_change | 0 | 162 |
| agent | spool_replay | 0 | 19 |
| agent | stat_walk | 0 | 28 |
| agent | steady_state | 0 | 221 |
| engine | compare | 0 | 75 |
| engine | full_recompute | 0 | 67 |
| engine | grid | 0 | 26 |
| engine | incremental | 0 | 44 |
| engine | index_settings | 0 | 31 |
| engine | parse_flatten | 0 | 25 |
| engine | search_text | 0 | 30 |
| hub | startup | 0 | 355 |
| hub-identity | audit_record | 0 | 193 |
| hub-identity | auth_middleware | 0 | 129 |
| hub-mcp | read_tools | 0 | 462 |
| hub-platform | ack_throughput | not run yet | |
| hub-platform | blob_endpoint | not run yet | |
| hub-platform | blob_get_cached | not run yet | |
| hub-platform | diff_trees | not run yet | |
| hub-platform | memory_at_design_scale | not run yet | |
| hub-platform | push_to_drift_webhook | not run yet | |
| hub-platform | sse_delivery | not run yet | |
| hub-registry | adoption_rank_per_swimlane | not run yet | |
| hub-registry | compare_6000_lines | not run yet | |
| hub-registry | delta_to_drift_changed | not run yet | |
| hub-registry | file_content_cached | not run yet | |
| hub-registry | file_content_cold | not run yet | |
| hub-registry | file_tree | not run yet | |
| hub-registry | full_recompute_target_scale | not run yet | |
| hub-registry | grid_40_swimlanes | not run yet | |
| hub-registry | head_moved_to_drift | not run yet | |
| hub-registry | incremental_recompute_base | not run yet | |
| hub-registry | incremental_recompute_per_file_per_swimlane | not run yet | |
| hub-registry | ingest_versions_per_s | not run yet | |
| hub-registry | search_settings_all_swimlanes | not run yet | |
| hub-registry | search_text_all_swimlanes | not run yet | |
| hub-registry | sentinel_attribution | not run yet | |
| hub-registry | steady_state_queries | not run yet | |
| hub-registry | swimlane_list | not run yet | |
| hub-registry | whole_swimlane_compare | not run yet | |
| hub-writes | edit_applied | not run yet | |
| hub-writes | write_queries | not run yet | |

17 of 44 bench targets have run; 0 of them exited non-zero.
<!-- bench-pieces:end -->

PENDING: the budget table, from `cargo xtask bench-check` after every piece has run.

| P# | Benchmark | Result | Budget |
|---|---|---|---|
| PENDING | | | |

## New dependencies

**No Rust crate and no npm package is added**: `Cargo.toml`, `Cargo.lock`, `package.json` and `pnpm-lock.yaml` are unchanged by this prompt (`git diff 5d67817^..HEAD` on them is empty). The rows below are build and CI tools, container images, vendored schemas and the model; none of them is in a lockfile that `cargo deny` or `osv-scanner` reads, so the last column says so. Age is in days on 2026-10-26.

| What | Version | Age | Why | Gate |
|---|---|---|---|---|
| cargo-chef (crates.io, `cargo install --locked` in the Dockerfiles) | 0.1.78 | 75 | caches the dependency layer of the image builds | not in `Cargo.lock`; installed from its own lockfile |
| cargo-cyclonedx (crates.io) | 0.5.9 | 221 | lists the Rust crates compiled into each binary for the image SBOMs | same |
| sqlx-cli (crates.io, CI only) | 0.8.6 | 525 | `cargo sqlx prepare --check` (must match the sqlx in `Cargo.toml`) | same |
| just (crates.io, CI only, via install-action) | 1.40.0 | 596 | the task runner CI uses | same |
| zizmor (release file, sha256-pinned) | 1.30.1 | 47 | workflow security audit | n/a |
| actionlint | 1.7.12 | 209 | workflow lint | n/a |
| kubeconform | 0.8.0 | 144 | manifest validation against vendored schemas | n/a |
| kind | 0.33.0 | 61 | the local cluster (the kind job is not written) | n/a |
| osv-scanner | 2.6.0 | 42 | lockfile scan (has never completed a scan here: `api.osv.dev` is refused) | n/a |
| cosign | 3.1.3 | 82 | image and RPM signing | n/a |
| crane | 0.22.1 | 53 | digest reads and pushes in the publish script | n/a |
| shellcheck | 0.11.0 | 449 | shell lint | n/a |
| helm (from `get.helm.sh`, sha256 of the download made here) | 4.3.0 | 47 | chart rendering; 4.3.1 is younger than 14 days | n/a |
| conftest image (by digest) | v0.71.1 | 20 | the Rego policies | not scanned |
| syft image | v1.54.1 | 20 | image SBOM | not scanned |
| grype image | v0.120.1 | 20 | image scan | not scanned |
| BuildKit image | v0.34.0 | 19 | reproducible builds | not scanned |
| distribution `registry` image (tests only) | 3.0.0 | 271 | a registry on localhost for the publish test | scanned by `scan.sh --image` in T6: it has critical findings, and is never deployed |
| `rust` builder, `node`, distroless `cc-debian13` | 1.88.0-trixie, 22.20.0-trixie-slim, `:nonroot` | 480, 397, 25 | build and runtime bases, Debian 13 (A32) | `images-scan`: no critical finding |
| Rocky 8.9, 8.10; AlmaLinux 9.8; Ubuntu noble | by digest | years; 24; 26 | the sentinel's install and exposure tests; the RPM build host | not scanned |
| CloudNativePG PostgreSQL image | 16.15-202610050823-system-trixie | 21 | the database (B10, a human confirmed) | **not scanned here (B9)** |
| `BAAI/bge-small-en-v1.5` ONNX files, from Hugging Face by commit and sha256 (not crates.io or npm: A3) | rev `5c38ec7c405e...` | 977 | the embedding model baked into the hub image | the lock checks each file's size and sha256 |
| Vendored CRD schemas (Kubernetes 1.34.0, CloudNativePG v1.30.1, Gateway API v1.6.3, Argo CD v3.5.4, Config Connector v1.159.0, datreeio catalog), at pinned commits | commits dated 2026-09-23 to 2026-10-09 | 17 to 33 | offline kubeconform | `deploy/schemas/verify.mjs`: 37 files equal the lock |
| GitHub Actions new in this prompt: `actions/download-artifact` v4.3.0, `google-github-actions/auth` v3.0.0, `actions/attest-build-provenance` v4.2.2, `actions/cache` v4.3.0 | pinned by full SHA | 550, 424, 81, 397 | artifact hand-over, Workload Identity auth, provenance, caches | `pins-test`, zizmor offline |

## Assumptions

The plan's A-table, with what was decided or done (the plan is frozen; this is the record).

| # | Decision or outcome |
|---|---|
| A1 | Not in 09. The hub does not serve; a human assigns composition. The kind job was not written (deviation). |
| A2 | The Dockerfiles and the hub and agent hardening tests and `HARDENING.md` files were changed as Extra-paths (approved). `crates/agent/Dockerfile.dockerignore` was listed and did not need a change. |
| A3 | `BAAI/bge-small-en-v1.5` ONNX at the commit in `hub-model.lock`, filled from a download made here; prompt 14 confirms or changes it. |
| A4 | Config Connector manifests in `deploy/charts/gcp`. The keys are referenced, not made; the hub's own signing keys, its Secret Manager and KMS bindings, the CI key, and the GitHub Workload Identity pool are made by no chart (T5 decision 5, B12). |
| A5 | Binary Authorization classic attestor with a CI KMS key, in `DRYRUN_AUDIT_LOG_ONLY`; whether a classic attestor accepts a cosign signature is unconfirmed (B12). |
| A6 | An `imagePullSecrets` name in the hub and agent values; the secret is created out of band. |
| A7 | The database credentials are created out of band; not exercised (no kind job). |
| A8 | Out of 09. Nothing runs migrations in production. |
| A9, A10 | `items: [ca.crt]` on the db-ca mount; ingress on 9090 from Google's two health-check ranges; `HARDENING.md` and `hardening.rs` amended with planted cases. |
| A11 | `FQDNNetworkPolicy` where available; kind falls back to a weaker 443 rule with a warning; CMEK falls back to Google-managed keys with a warning. Both are warnings in kind and failures in staging and prod. |
| A12 | The agent's egress has five destinations, as `HARDENING.md` lists. |
| A13 | `required` zone anti-affinity in staging and prod; kind uses host spread with a warning. |
| A14 | The agent's certificate Secret is chart-created, empty, with `helm.sh/resource-policy: keep`, and ignored by Argo CD (`/data`). |
| A15 | The exposure score is 1.7, measured offline by systemd 252; the unit sets only directives that Rocky's 239 reads (checked by that systemd, not by a list). No escalation was needed. |
| A16 | The RPM release key is a marker; PR runs sign with a throwaway key. B13. |
| A17 | The socket is in a `0750 root:lanekeeper-sentinel` directory; prompt 17 confirms against the daemon. Not run against the real audit package. |
| A18 | The sentinel binary is a stub; "starts" proves the unit and not the daemon. |
| A19 | **Not as planned**: the committed prod values carry an EXAMPLE one-year period with `isLocked: true`, because the policy requires the lock in prod. A comment says a person confirms the period before the first apply. Staging is never locked. |
| A20 | `bench.yml` runs on `vars.LK_BENCH_RUNNER`; the comparison with `main` is reported, not gating. It has not run. |
| A21 | Lighthouse stays in 08's `web-e2e.yml` `perf` job. |
| A22 | Tools are pinned by version and sha256 in the Justfile or by digest in `tools.env`; the table above. |
| A23, A24, A25 | `sqlx prepare --check` passes trivially while there are no query macros; a pgvector service container plus Docker for testcontainers; `check-pins.mjs` is wired in and the old `scripts/check-actions-pinned.sh` is left alone. |
| A26 | **Changed by B10:** the database image is the `system` variant (`16.15-202610050823-system-trixie`), not `standard-bookworm`, because only `system` has barman; **a human confirmed** keeping it with the built-in `barmanObjectStore`. Moving to the Barman Cloud plugin is a later open question. |
| A27, A28, A29, A30, A31 | amd64 only; the runbook adds the NFS VM's egress IP to the agent load balancer's ranges; image digests and `targetRevision` are placeholders until the first publish; no collector is deployed; every `*-fuzz` recipe runs at 30 s in `rust.yml`. |
| A32 | **Amendment (a human decided option 1 of B3, PR #29):** the runtime moved to `gcr.io/distroless/cc-debian13:nonroot` and the builders to Debian 13, with no exception in `grype.yaml`, because the Debian 12 base had two critical findings that will not be fixed. Re-scanned in this step: no critical finding in either image. |
| B10 | **Decided by a human:** keep the interim image and backup method. |
| B5 | **Decided by a human:** stays open. |

Commit structure (git shows it; `git log --reverse 5d67817^..HEAD`): the plan (`09/plan: draft`, `approved by a human`, and the amendment `7385ce2` merged as PR #29), then `09/T1:` twice (`e2c199d`, `af7366b`; before them two `09/blocked: T1` commits, `f02523a` and `19b4aa2`), `09/T2:` once, `09/T3:` twice (`f9992b0` the helm pin, `d0c6a72` the chart) and one `09/blocked: T3` (`5af7f75`), `09/T4:`, `09/T5:` and one `09/blocked: T5` (`6ed348f`), `09/T6:`, `09/T7:`, and `09/T8:` three times (`2d937de`, `fa4b4bc`, `d164637`, restart checkpoints). T4, T6 and T8 have no separate `09/blocked:` commit; their notes are in the `plans/09-deployment-ci.blockers.md` changes inside their own commits. This step adds `09/evidence:` commits and one `09/blocked:` commit for B14.

## Deviations from the plan

Each is also in `plans/09-deployment-ci.blockers.md` unless it says otherwise.

1. **The kind job, its script and the `kind-e2e` recipe were not written** (T5's note, "Not done in T5"; T6 and T8 repeat it). Acceptance 3 is unproven (A1 as well).
2. **The database image and backup method** differ from the plan's literal (B10, a human confirmed).
3. **Prod's audit bucket is locked in the committed values with an example period** (A19, T5 decision 6).
4. **No `values/gcp/kind.yaml`** (T6 decision 3): kind has no Google Cloud.
5. **`contract.rego` compares the agent only**, not the hub (T4 decision 1).
6. **Helm 4.3.0** is pinned, which the plan did not name (B4).
7. **`cargo deny` and `cargo audit` fail on the committed lockfile** (B14, new in this step; recorded in the blockers file).
8. **A test of 08 failed once** in the web suite (`web/scripts/perf/chromium-process.test.mjs`) and passed in the rerun; not 09's, not fixed, not in the blockers file (recorded here).
9. **`cargo test --workspace` ran with no debug info** (`CARGO_PROFILE_*_DEBUG=0`) because of the disk; recorded here only.
10. Not changed, and open for a human: B5, B7, B8, B11, B12, B13, the hub chart's `network.rego` gap, `images.yml` (KMS signature path), `CODEOWNERS` entries for `deploy/**`.
