# Evidence 09: Deployment, CI and supply chain

Written by the implementer in the FINAL step. Real output only; nothing is ticked that was not proved by output shown here.

**Status of this file: PARTIAL (first commit, written while the Rust gates are still running).** Sections marked `PENDING` are filled in by the next `09/evidence:` commit. A section still marked `PENDING` in the last commit means the step did not finish; the text says why.

Tree under test: branch `agent/09-deployment-ci`, head `d164637` (`git status` clean before every run; the verify-09 recipes changed no tracked file). Run on 2026-10-26 in the sandbox (Docker 29.9.2 daemon running, helm 4.3.0 and the other pinned tools in `target/ops-tools/bin`). Raw logs are in the session scratchpad, not in the repository.

## Read this first: what is open, and why

| Id | State | What it stops in this evidence |
|---|---|---|
| **B5** | **open, by the human's decision** | `deploy/values/hub/github-hook-ranges.yaml` is the unresolved marker (`githubHookRanges: []`); the gcp chart refuses to render. `just verify-09` therefore fails at `deploy-render` and nothing else. No range was typed in, copied or swapped in, even for one run, and no marker, refusal or test was changed. |
| **A1** | open (needs a human to assign) | The hub binary exits 1 (`crates/hub/src/main.rs`) until 03a and 05 are composed into it, and prompt 09 owns no application code. `/readyz` cannot pass on any replica. |
| not written | deviation | The kind job (`deploy/tests/kind/run.sh`, the `kind-e2e` job and recipe) was never written (T5's row lists no file for it; see "Deviations"). Acceptance 3 is not proven. |
| **B9** | open, informational | `pkg-containers.githubusercontent.com` is refused, so the database image (`ghcr.io/cloudnative-pg/postgresql:16.15-202610050823-system-trixie`) was pinned by a digest read from the registry but its layers were never read or scanned. |
| unrun workflows | open | No workflow under `.github/` has run on GitHub. |
| **B13** | open, informational | The RPM release key and its Workload Identity trust root do not exist (`RPM-GPG-KEY-lanekeeper.pub` is an unresolved marker). The Rocky, Debian and several other hosts are refused, so the install test ran with a labelled stand-in for the `audit` package. |
| **B11** | open, informational | Nothing creates a swimlane in the hub; step 1 of `deploy/ADDING-A-SWIMLANE.md` describes a contract, not something that can be followed. |
| **B12** | open, informational | The CI signing key, the Workload Identity Federation pool and the IAM around them are made by no chart; whether a classic Binary Authorization attestor accepts a cosign signature is unconfirmed; the policy is per project and in dry run. |
| B7, B8 | open, informational | The agent dials the hub on 443 while the hub's agent load balancer listens on 8444; the config-server egress rule selects pods in the agent's own namespace. |
| B10 | **decided by a human** | Keep the interim database image (`system` variant, built-in barman). Migration to the Barman Cloud plugin is a later open question. |
| B1 to B4 | resolved | Model lock filled from a download made here (B1); scan runs (B2); images moved to Debian 13 by plan amendment A32 (B3); helm 4.3.0 pinned from a download made here (B4). |

## Gates

| Command | Result | Output (tail) |
|---|---|---|
| `just verify-09` | **FAIL, exit 1, 40 s. B5 only.** It stops at `deploy-render` (the first recipe after `deploy-lint` that renders the gcp chart); every recipe before it passed. | `error: Recipe 'deploy-render' failed on line 914 with exit code 1`, after `kubeconform` had reported hub 6, 13, 13; db 2, 4, 4; gcp-storage 19, 19; agent 11; deploy/argocd 7 resources valid, and the two failures `Error: execution error at (lanekeeper-gcp/templates/cloudarmor.yaml:7:4): cloudArmor.githubHookRanges is empty: GitHub's webhooks would be denied (P2, Q28). Run 'node deploy/tools/github-hook-ranges.mjs --write' and commit deploy/values/hub/github-hook-ranges.yaml` for gcp with staging and prod. |
| `just verify` | PENDING | |
| `cargo xtask bench-check` | PENDING | |

## `verify-09`, each recipe run on its own (so the evidence shows which pass and which fail only because of B5)

`verify-09` is `ops-tools-check ci-lint pins-test images-policy-test images-builder-test images-publish-test deploy-check sentinel-check`, and `deploy-check` is `schemas-verify deploy-lint deploy-render policy-test policy-check contract-check deploy-planted env-names-test tools-test`. `just` stops at the first failure, so each recipe was run alone, in that order, from the committed tree. Wall time is the recipe's own (`just <recipe>`, with its prerequisites).

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
| `deploy-render` | **1** | 2 s | **B5 only.** kubeconform `-strict`, offline: hub (kind, staging, prod) 6, 13, 13 resources valid; db 2, 4, 4; `gcp-storage` (the gcp chart without Cloud Armor) 19, 19; agent (sit1) 11; `deploy/argocd` 7 in 5 files; all `Invalid: 0, Errors: 0, Skipped: 0`. Failed: `gcp` with staging and with prod, with the B5 message (`render.sh: kubeconform_one FAILED for the gcp chart with the staging values` and `...prod values`). |
| `policy-test` | 0 | 8 s | `conftest verify`: `1002 tests, 1002 passed, 0 warnings, 0 failures, 0 exceptions, 0 skipped` |
| `policy-check` | **1** | 20 s | **B5 only.** conftest passes (0 failures) on hub (kind `266 passed, 4 warnings`, the fallbacks kind is allowed; staging and prod `270 passed`, with the placeholder-digest warning of A29 in the separate images run), db (kind `267 passed, 3 warnings`, its fallbacks; staging and prod `270 passed`), `gcp-storage` (staging and prod `270 passed`, with Binary Authorization's dry-run warning), agent (`270 passed`, placeholder-digest warning), the Argo CD policy on the platform's files (`43 passed, 3 warnings`: the placeholder `targetRevision`) and on the swimlane's (`45 passed, 1 warning`), the files each Application names, and the scope (`3 tests, 3 passed`) of hub, db, `gcp-storage` and agent against their projects. Failed: `gcp` with staging and prod (`render.sh: policy_one FAILED`), and the scope of `gcp` with prod (`render.sh: argocd scope FAILED for the gcp chart with the prod values`), each with the B5 message. |
| `contract-check` | 0 | 2 s | `contract-extract.sh --self-test`; `contract agent (sit1)`: `34 tests, 34 passed`; the db, gcp and hub charts agree (staging and prod) |
| `deploy-planted` | **1** | 431 s | **B5 only.** 417 planted cases report `ok`. The single failure is the Cloud Armor group, which prints `(the gcp chart does not render with the committed hook ranges, so the Cloud Armor cases are not run: ...)` and ends with `render.sh: 1 planted case(s) failed`. |
| `env-names-test` | 0 | 0 s | every `LK_*` variable of the hub chart is read by `crates/hub/src/config.rs`; of the agent chart by `crates/agent/src/config.rs`; self-test refuses a misspelt and a secret-looking name |
| `tools-test` | 0 | 6 s | `node --test deploy/tools/*.test.mjs`: `# pass 30`, `# fail 0` |
| `sentinel-check` | 0 | 11 s | 20 of 20 tests of the unit-file parser (`# tests 20`); `conftest verify` of the `systemd` (S25) and `dockerfile_artifact` namespaces `97 tests, 97 passed`; the policies on the real unit file `23 tests, 23 passed` and on the real `Dockerfile.build` `14 tests, 14 passed`; the planted Dockerfile checked rule by rule through the real parser; a unit with `CapabilityBoundingSet=CAP_SYS_ADMIN` refused through the real parser. Last line: `sentinel-check: the unit passes the S25 policy, Dockerfile.build passes its policy, and the planted cases are refused` |

Recipes that pass: 13 of 16. Recipes that fail: `deploy-render`, `policy-check`, `deploy-planted`, **each only on the gcp chart with Cloud Armor**, i.e. B5. Every other chart, values file, policy, planted case and script test passes.

What B5 leaves unproven: kubeconform, conftest and the Argo CD project scope on **one render that holds Cloud Armor and the buckets and the admission resources together**, and the three Cloud Armor planted cases. No rule reads both, so the gap is small, but it is a gap.

PENDING: the rest of this file (the `just verify` steps, `cargo xtask bench-check`, the threat-model tests per crate, the acceptance checklist, security requirements, dependencies, assumptions, deviations).
