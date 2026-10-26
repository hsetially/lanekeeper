# 09: Deployment, CI and supply chain

**This branch is NOT ready to merge green.** It is ready for human review with four human-owned gates open: B5, B15, B17 and A1 (details below). Nothing is ticked that command output does not prove. The full evidence is `plans/09-deployment-ci.evidence.md`; the blockers are `plans/09-deployment-ci.blockers.md` (B1 to B17); the three review rounds are `plans/09-deployment-ci.review.md`.

## Prompt

`prompts/09-deployment-ci.md` · plan: `plans/09-deployment-ci.md` (approved by a human; amended once by a human through PR 29, A32: images moved to Debian 13) · wave: 1

Commits: `09/T1:` to `09/T8:` (T1 took two commits and T8 three, as restart checkpoints), then `09/blocked:`, `09/fix:`, `09/evidence:` and `09/review:` commits, and two merges of `origin/main`. Head at the time of writing: see the branch.

## Gates

Real results at head `5931895`, each recipe run on its own (one-command `just verify-09` exits 1 at `deploy-render`):

| Gate | Result |
|---|---|
| `just ci-lint` | exit 0: actionlint, zizmor offline (online audits not run), SHA-pin check, workflow rules W1 to W6, shellcheck |
| `just pins-test` | 115 of 115 |
| `just policy-test` | 1021 of 1021 |
| `just contract-check` | 34 of 34 |
| `just deploy-lint` | 11 charts, 0 failed |
| `just images-policy-test`, `images-builder-test`, `images-publish-test`, `sentinel-check`, `schemas-verify`, `env-names-test`, `tools-test`, `ops-tools-check` | exit 0 |
| `cargo test -p hub --test hardening`, `-p agent --test hardening` | 5 of 5 each |
| `threat_model` tests of the seven crates that parse `docs/threat-model.md` | all pass |
| **`just deploy-render`, `just policy-check`, `just deploy-planted`** | **exit 1, for B5 only** (`deploy/charts/gcp/templates/cloudarmor.yaml:7`). Hub, db, `gcp-storage`, agent and Argo CD all valid in kubeconform; 417 planted cases ok, 1 failed: the Cloud Armor group |
| **`just verify-09` (one command)** | **exit 1 at `deploy-render`, B5 only** |
| **`cargo deny check`, `cargo audit`** | **exit 1 today: B17** (four zstd advisories; see below) |
| **`cargo xtask bench-check`** | **exit 1: 65 pass, 6 fail, 2 unmet of 73** (B15; see Benchmarks) |
| `just verify` (one command) | **Passed once** on 2026-10-26 (exit 0, 80 min, 13:47 UTC, on `3a67982`) against the advisory database of that moment (1355 advisories). It **does not reproduce today**: with 1378 advisories it stops at `cargo deny check` (B17). It was not re-run as one command after that. That run covered fmt, clippy, `cargo test --workspace` (5,434 passed, 0 failed, 20 ignored) and web-verify (9,679 tests) |

Not run: `just fuzz-smoke`, `images-repro` and `sentinel-repro` after the final Dockerfile change, `sentinel-test` after the `rpm-tools` change, `kind-e2e` past its cgroup v1 preflight, and every step that needs GitHub, GCP or a cluster. **No workflow in `.github/` has run on GitHub.**

## Acceptance checklist

The prompt's three acceptance items:

- [ ] `just verify-09` passes: lint, kubeconform, conftest and the SHA-pin test. **NOT MET (B5).** Lint and the SHA-pin test pass, every chart passes kubeconform and conftest except the gcp chart with Cloud Armor.
- [ ] Images are signed, and their SBOMs and provenance attestations are published. **NOT PROVEN.** SBOMs are made and scanned (hub 1,723, agent 1,593 components, no critical finding). Keyless signing, the SBOM attestation, provenance, the KMS signature and `gh attestation verify` were never run against GitHub; only a key-based push, sign, attest and verify ran against a localhost registry.
- [ ] A kind cluster in CI runs the hub chart with CloudNativePG, and `/readyz` passes on all 3 replicas. **NOT MET and NOT PROVEN (A1).** The hub binary exits 1, so `/readyz` cannot pass. The kind script and job are written and are red by design on A1 (they never fake readiness); nothing past `kind create cluster` has run (cgroup v1 here, and the CloudNativePG images are on a refused host, B9).

Each task's Verify line:

- [x] T1: images build reproducibly (`images-repro`: hub and agent built twice, same digests).
- [x] T1: per image a CycloneDX SBOM and a scan that fails on critical findings (the planted CVE-2021-44228 fails the scan).
- [x] T2: `actionlint` passes.
- [x] T2: a test fails if any action reference is not a SHA (`pins-test`, including folded block scalars).
- [x] T3 (hub chart): kubeconform passes and the policy tests reject a pod missing any hardening field. The Cloud Armor part of the gcp chart is unproven (B5).
- [x] T4: kubeconform passes and the Role has no Secret list.
- [ ] T5: db and gcp charts. **PARTIAL.** The db chart and `gcp-storage` pass. The full gcp chart is blocked by B5.
- [ ] T6: every values file through conftest. **PARTIAL.** Everything passes except the gcp chart with Cloud Armor (B5).
- [x] T7: the threat-model section (34 STRIDE rows, each with a status, proof and S#; every residual risk carries its blocker id).
- [ ] T8: the RPM installs and starts on Rocky 8.9 and 8.10. **PARTIAL (B13).** It does, with a labelled stand-in for the `audit` package (the Rocky and Debian hosts are refused here); the real `--audit repo` path has never run.
- [x] T8: `systemd-analyze security` exposure score 2.0 or lower (1.7, scored by systemd 252 offline).
- [ ] T8: the RPM's signature verifies. **PARTIAL (B13).** `rpm -K` verifies with a throwaway key and refuses a changed byte; the release key does not exist.

## Security requirements implemented (S#)

S6 (TLS 1.3 and Postgres verify-full), S7 (no Kubernetes Secrets but two; secrets from Secret Manager; the signing job builds nothing from crates.io), S9 (locked audit checkpoint bucket; the hub may reach `storage.googleapis.com`), S10 (CMEK on buckets and disks), S16 (container hardening, distroless non-root, one policy test per field), S17 (agent RBAC with no Secret list, exact egress), S18 (default deny, FQDN egress allowlist, load-balancer ranges), S19 (pinned actions and images, `--locked`, Renovate with a 14-day age), S20 (SBOM, signing, provenance, Binary Authorization in dry run, scans that block critical findings), S25 (sentinel packaging and systemd unit); S5 and S22 in support. The table with the code location, the named tests and what is **not** proven for each is in the evidence file. S24 sign-off of the threat model is a human step.

## Benchmarks against budgets (P#)

`cargo xtask bench-check`: **exit 1, 65 pass, 6 fail, 2 unmet of 73.** All 44 bench targets ran. The prompt changed no crate and no budget, and nothing was changed to pass.

- **B15, two real misses in crates 09 does not own** (five runs each, quiet machine included): `P6.index_settings` (prompt 04) p95 62.4 to 72.1 ms against 60; `P12.audit_record` (prompt 03a) p95 3.14 to 3.59 ms against 3. A human chooses: rerun on the CI bench runner, send them back to 04 and 03a, or change the thresholds through a contract-change PR.
- **No result for four web budgets** (`route_change`, `diff_6000_lines`, `grid_5000_rows_fps`, `lighthouse_perf`): no browser is installed here and the Playwright download hosts do not answer. `P10.initial_js_gzip_kb` measured 195.9 of 250 KB.
- **Unmet:** `P7.docs_search` and `P7.feature_status` are not registered.
- 27 of the 44 targets ran while another session shared the 4 vCPUs; the evidence flags each, and the five near-budget results were rerun quiet.
- The only P# tied to this prompt, P11 (hub start-up), is asserted by the kind test per replica and reports `NOT MEASURED` while A1 keeps the replicas from starting. `P11` in `bench-check` passes (41.2 ms against 10000, memory 746.5 MiB against 1536).

## New dependencies and why

No Rust crate and no npm package is added by this prompt (`Cargo.toml`, `Cargo.lock`, `package.json`, `pnpm-lock.yaml` are untouched by it; the two merges of `origin/main` bring in PR 30's lockfile change, which is not this prompt's). Build and CI tools, container images, vendored schemas and the model are listed with versions, ages and the gate that covers each in the evidence file's "New dependencies" table. The ones worth a reviewer's attention:

- crates.io tools run by `cargo install --locked` and not part of any lockfile: cargo-chef 0.1.78, cargo-cyclonedx 0.5.9, sqlx-cli 0.8.6, just 1.40.0.
- Release files pinned by version and sha256 (rule 15 names crates.io and npm only, A22): helm 4.3.0, kubeconform, actionlint, zizmor, osv-scanner, cosign, crane, shellcheck, kind 0.33.0.
- Container images pinned by digest read from the registry: BuildKit, syft, grype, conftest, kindest/node v1.37.0, CloudNativePG operator 1.30.1, the Debian 13 builder and distroless runtime, and the CloudNativePG PostgreSQL image (`16.15-202610050823-system-trixie`, **not scanned here, B9**).
- The embedding model `BAAI/bge-small-en-v1.5`, from Hugging Face by commit and sha256 (A3).

## Assumptions

The plan's A-table with what was decided is in the evidence file. The ones that matter at review:

- **A32 (human amendment, PR 29):** images moved to Debian 13 (`cc-debian13` runtime, trixie builder) because the Debian 12 base had two critical findings that will not be fixed.
- **B10 (human decision):** keep the CloudNativePG `system-trixie` image with the built-in barman, both deprecated; migration to the Barman Cloud plugin is a later question. A nightly `db-image` job scans it by digest and has not run yet.
- **A19 (decided by a human on 2026-10-26):** `deploy/values/gcp/prod.yaml` commits `isLocked: true` on the audit checkpoint bucket with a retention of **three months** (`retentionPeriodSeconds: 7776000`, 90 days). The lock cannot be undone; the file's project and names are still examples, so nothing is applied or locked yet.
- **A1:** composing the hub binary is not in this prompt (it owns no application code). A human assigns it.
- **A5:** Binary Authorization runs in `DRYRUN_AUDIT_LOG_ONLY` until Q24 is answered.
- The reproducibility claim holds for the pinned BuildKit v0.34.0; the pinned CI builder's own reproducibility has not been tested.
- Signing job: `sign-release` builds only the `rpm-tools` stage (proved by `tools-graph.sh`, which reads BuildKit's own plan). It still runs an `apt-get install` of Ubuntu's signed rpm and gpg packages as root in a build container before it authenticates; gpg and gpg-agent are pinned by a glob and the libraries float. This is documented in the workflow header, the README and the threat model (residuals 27 and 28).

## Needs a human before merge

1. **B5:** run `node deploy/tools/github-hook-ranges.mjs --write` on a machine with normal GitHub API access and commit `deploy/values/hub/github-hook-ranges.yaml`. This sandbox's proxy answers 403 on `api.github.com/meta`. Until then `just verify-09` fails and Cloud Armor's GitHub-only allowlist for `/hooks/github` is not in place. The same chart also refuses to render without the Binary Authorization attestor public key (B12).
2. **B17 (new, same class as B14):** `cargo deny check` and `cargo audit` exit 1 on RUSTSEC-2026-0391, -0398, -0403 and -0410 (zstd 0.13.3, zstd-safe 7.3.0, zstd-sys 2.0.16, dated 2026-10-16). The fix, zstd 0.14.3, is 10 days old (rule 15 is met from 2026-10-30), **and `tonic` 0.14.6, the newest, requires `zstd = "0.13.0"`**, so a lockfile bump cannot reach it. Waiting until 2026-10-30 or an override like D134 may not be enough: a `tonic` release or a decision on the `zstd` feature (P4) is also needed. `Cargo.lock`, `Cargo.toml` and `deny.toml` are identical to `main`, so `main` fails the same way.
3. **B15:** the two benchmark misses above.
4. **A1:** the hub binary composition, which also decides whether the kind job can ever go green.
5. **B8 (a contract disagreement between HARDENING.md files):** the config-server egress rule has no `namespaceSelector`, so it matches nothing if the config-server runs outside the agent's namespace. Open. (B7, the agent's port, is resolved: a human decided on 2026-10-26 that the hub's agent load balancer listens on 443 and forwards to the pods' 8444.)
6. **Branch protection:** `ci.yml` no longer holds the `rust`, `web` and `pinned-actions` jobs (moved to path-filtered workflows). Required-check names need updating.
7. **B11 and B12 and B13:** nothing creates a swimlane; no chart creates the CI signing key, the Workload Identity pool or the Binary Authorization trust root; the RPM release key and its trust root do not exist. A release tag stops at `sign-release` until they do.

## Reviewer agent verdict

Round 3 (`plans/09-deployment-ci.review.md`), reviewed at `a5f46a4`:

```
VERDICT: APPROVE
No blocking finding in the work of round 2. The branch is not mergeable green today
because of human-owned gates (B17 and the ones above).
```

Rounds 1 and 2 returned CHANGES REQUESTED (round 1: the lost disk-space steps, a build in the signing job, the missing `storage.googleapis.com` egress, the missing kind test; round 2: the signing job still built the sentinel through the `rpm` stage). All blocking findings were fixed. The small cleanup after round 3 (B17 in the evidence, a stale threat-model phrase, W6 for tools called by path, a kind README note) was made by the implementer and is **not** itself reviewed.

## Labels

No `contract-change` label: `Contract-change: none`, and no contract path changed.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_011wuVg8Nmkp8WWHzDsARGuX
