# 09: Deployment, CI and supply chain

**Not mergeable green, and not claimed to be.** Ready for human review with these gates open: B5 (GitHub's hook ranges: the gcp chart cannot be rendered with staging or prod values), acceptance item 3 (`/readyz` on three replicas: not met, and the production image cannot meet it in kind), and the red CI jobs listed below, each explained. Nothing is ticked that a command did not prove, and every statement about GitHub says **as observed on GitHub at `62d23dc`**: it was read from GitHub's pages by the orchestrator and not produced here. **The run for this branch's newest head is pending.** The evidence is `plans/09-deployment-ci.evidence.md` (read its first part; the history below it is superseded); the blockers are `plans/09-deployment-ci.blockers.md`; the chart alignment after prompt 18 is `plans/09-deployment-ci.chart-alignment.md`; the earlier review rounds are `plans/09-deployment-ci.review.md`.

## Prompt

`prompts/09-deployment-ci.md` · plan: `plans/09-deployment-ci.md` (approved by a human; amended once by a human through PR 29, A32: images moved to Debian 13) · wave: 1

Commits: `09/T1:` to `09/T8:`, then `09/blocked:`, `09/fix:`, `09/evidence:` and `09/review:` commits, and merges of `origin/main` (the last brought in prompt 18, PRs 40 to 43, as `cb4367c`). The `09/fix:` commits since then: the hub chart's settings, migration Job and seed Job (`30b53a1`, `66ba049`), the kind job's new assertions (`972b361`), the agent chart's config-server namespace (`77481be`), the gcp chart's keys, secrets and IAM (`b5db76d`), lint findings (`5a3ab7d`), the install guide (`1f8fe02`, `99a00b9`), review round 1 (`b2ca79f` to `c3e322b`), and review round 2 (below: `7b0a0ab`, `d075c09`, `b16492d`, `4f20dab`, `6821663`, `cfde244`, `0064fea`, `8c78389`, `e69212e`, `1a2dc23`, `023c779`).

## What the last review found, and what was done

A second independent review re-checked round 1 at `de30902` and returned **CHANGES REQUESTED**: two blocking findings (B-1, B-2) and six non-blocking (NB-1 to NB-6); the orchestrator declined two more (NB 7: per-resource sync options other than `Prune=false`, which predates the round; NB 8: unbounded old seed Jobs, which is by design and is residual 33). All of round 1 was confirmed fixed except these. Each finding was checked against the code first; **all eight were real and none was rejected**. Every rule, guard and check was written first and seen red (B-2 and NB-1 were reproduced on prod values before the fix), and each new rule was shown able to fail.

| Finding | Fixed in |
|---|---|
| B-1 "the seed record is kept" was false for staging and prod, where the file is `seed.existingConfigMap`, a ConfigMap a person makes: the chart renders none, the Job's name does not cover the file, and the runbook said to replace the ConfigMap in place | `7b0a0ab` (the chart refuses an `existingConfigMap` that does not end with `-<revision>`; `prod.yaml`, `staging.yaml` follow), `b16492d` (the runbook: a new ConfigMap per change, named with the revision, made with `create`, never edited or deleted in place; the five statements qualified), `4f20dab` (the asset, the R row, residuals 33, 34, 37) |
| B-2 `Prune=false` made a revert a silent no-op (revisions 1, 2, 1 give one Job name for 1 and 3; Argo CD shows `Synced` and `hub seed` does not run) | `7b0a0ab` (`seed.revision` is a positive integer; it was a free string), `b16492d` ("never reuse a revision, a revert included: always move forward", in the guide, the chart README, `values.yaml`, `HARDENING.md`, the values comments), `4f20dab` (residual 38), `e69212e` (the collision needs the same image too). The chart cannot refuse a reused revision (a template has no state); the README says so |
| NB-1 `LK_BLOB_CACHE_MIB: "04096"` rendered (Sprig reads base 0, the hub reads 4096) and the pod is OOM-killed | `d075c09` (every number of `config.tuning` is a number in the hub's range, never a string; planted cases for each) |
| NB-2 the migrator-Secret row and the seed row overstated what the policies cover | `4f20dab` |
| NB-3 `kms.rego` held the names of a secret container's labels and annotations, not their values (`instance: hunter2`, the project annotation, `finalizers`, `generateName`, `ownerReferences` passed) | `6821663` (15 Rego tests, 7 planted cases) |
| NB-4 nothing held the guide's round-1 fixes; the webhook secret went into the working directory; the `ca-init` pods' open egress was unsaid; "begin 10.2 again" fails with `AlreadyExists` | `cfde244` (four rules and 11 mutants in `install-guide.test.mjs`; the guide) |
| NB-5 the disk check did not see the fuzz jobs | `0064fea` (six mutants of a copy of the workflows) |
| NB-6 `HARDENING.md` and a chart comment said the hub "binds the operations socket first" and listed four `/readyz` checks (there are seven) | `8c78389` |

The earlier evidence said the round-1 documents and workflows were "held by the checks named in their rows". That overstated NB-5 and NB-11; the evidence file corrects it.

## Gates

Run here (this sandbox has no Docker daemon and about 6.7 GB of disk; `conftest` ran from the pinned binary through a stand-in for `docker`): the render, policy, planted and `ci-lint` runs on the tree at `8c78389`; `conftest verify`, the script tests and the two hub tests again on the final tree (`e69212e`, `1a2dc23` and `023c779` after it change prose and one threat-model table). Plans commits follow.

| Gate | Result |
|---|---|
| `conftest verify --policy deploy/policy` | **1364 of 1364** (1349 at `de30902`) |
| `node --test .github/scripts/*.test.mjs` | **223 of 223** (205 at `de30902`; the install guide's test is 42 of 42, the consistency test 17 of 17) |
| `deploy/tests/render.sh lint`, `contract`, `data`, `argocd` | each exit 0 (`data`: 294 cases ok; `argocd`: 67) |
| `deploy/tests/render.sh render`, `policy` | **exit 1, B5 only**: the gcp chart with staging and prod values (`cloudArmor.githubHookRanges is empty`); every other chart and environment passes |
| `deploy/tests/render.sh planted` | **exit 1, B5 only**: **847 cases ok** (779 at `de30902`), no `PLANTED FAILED` line, and one counted failure, the Cloud Armor group, which is not run because the gcp chart cannot render |
| `just ci-lint` | exit 0 (actionlint, zizmor offline, SHA pins, workflow rules, shellcheck over 35 scripts) |
| `node deploy/schemas/verify.mjs` | exit 0 (38 files equal the lock) |
| `cargo test -p hub --test hardening`, `--test justfile_gate` | 7 of 7 and 8 of 8 |

**`cargo xtask bench-check`: not re-run in this round.** The `09/fix:` commits of this round change `deploy/`, `docs/`, `plans/`, `.github/scripts/` and prose in `crates/hub/HARDENING.md`: no crate source, no benchmark, no `perf/` file. The two earlier misses (B15: `P6.index_settings`, `P12.audit_record`) are, as reported by the orchestrator and not checked here, settled by PR 34 on `main`. P11 (the only P# of this prompt) is asserted per replica by the kind job and reads **NOT MEASURED**. `just verify` and `just verify-09` as single commands were not re-run either: `verify-09` stops at `deploy-render` for B5, and `just verify` takes about 80 minutes.

**As observed on GitHub at `62d23dc`** (the run for the newer head is pending):

| Job | Result |
|---|---|
| "kind, the hub chart with CloudNativePG" (run 40665155670) | **passed**: the migration and seed Jobs completed; all 3 replicas stopped at their first Google step (exit 1) with no setting, database or schema error in their logs; `/readyz` on 3 replicas **NOT MEASURED**, P11 **NOT MEASURED**, acceptance 3 **NOT MET by this job** |
| rust `test` (`cargo test --workspace`), `lint`, `sqlx`, `fuzz-smoke`, `engine-fuzz`, the hub `image` job, `scan-test`, policies and scripts, both Rocky Linux installs, the sentinel's build, signing and SBOM, `e2e` | success |
| `charts` | red: the gcp chart only (B5) |
| `osv`, `supply-chain` | red: RUSTSEC-2026-0414 (pulldown-cmark 0.13.4 through prost-build). The fix, 0.13.5, was released on 2026-10-21 and is 14 days old on 2026-11-04. `Cargo.lock` equals main's, so `main` fails the same way |
| the seven `*-fuzz` jobs that run `cargo deny` | red: rustls-webpki 0.103.15 and 0.103.16 and webpki-root-certs 1.0.10 are under the 14-day rule until 2026-11-02 and 2026-11-03, and the same pulldown-cmark |
| `workflows` | red only for a shellcheck SC2016 finding that `5a3ab7d` fixed |

## Acceptance checklist

The prompt's three items:

- [ ] `just verify-09` passes. **NOT MET (B5).** `render.sh lint`, `contract`, `data` and `argocd`, `just ci-lint`, the 1364 policy tests and the SHA-pin test pass; `render` and `policy` fail for the gcp chart with staging and prod values, because the hook ranges file is the unresolved marker.
- [ ] Images are signed, and their SBOMs and provenance attestations are published. **NOT PROVEN.** SBOMs are made and scanned (the `image` and `scan-test` jobs passed on GitHub at `62d23dc`). Signing, the SBOM attestation, provenance and the Cloud KMS signature happen in the `publish` job, on `main` and tags, and have not been seen; only a key-based flow ran, against a registry on localhost.
- [ ] A kind cluster in CI runs the hub chart with CloudNativePG, and `/readyz` passes on all 3 replicas. **NOT MET.** The production image reaches Secret Manager, Cloud KMS, Cloud Storage and Entra at start (prompt 18, G23: no fakes in a release build), so in kind it stops at its first Google step. The kind job asserts what the image can do and passed at `62d23dc` (above); it reports `/readyz` and P11 as not measured. The reading of this item is a human's decision (below).

Each task's Verify line:

- [x] T1: images build reproducibly (the hub `image` job passed on GitHub at `62d23dc`; `just images-repro` for both images ran at an earlier head and no Dockerfile changed in this round) and have a CycloneDX SBOM and a scan that fails on critical findings (`scan-test` passed).
- [x] T2: `actionlint` passes (`just ci-lint`) and a test fails if an action reference is not a SHA (`check-pins.test.mjs`, in the 223).
- [x] T3: kubeconform passes and the policy tests reject a pod missing any hardening field (`render.sh render` and `policy` for the hub chart in kind, staging and prod; 1364 tests).
- [x] T4: kubeconform passes and the Role has no Secret list (`render.sh contract`).
- [ ] T5: db and gcp charts. **PARTIAL (B5).** The db chart and `gcp-storage` pass `render.sh data`; the gcp chart with Cloud Armor cannot render.
- [ ] T6: every values file through conftest. **PARTIAL (B5).**
- [x] T7: the threat-model section, updated in this round (residuals 37 and 38, the seed and secret-metadata rows, the `ca-init` row, a "run here, round 2" row; every test and planted case it cites exists, checked by script).
- [ ] T8: the RPM installs and starts on Rocky 8.9 and 8.10. **PARTIAL (B13):** both installs passed on GitHub at `62d23dc`; the real `audit` package and the release key do not exist.
- [x] T8: `systemd-analyze security` exposure score 2.0 or lower (1.7, earlier head).
- [ ] T8: the RPM's signature verifies. **PARTIAL (B13):** with a throwaway key only.

## Security requirements implemented (S#)

S5 (two CA keys that cannot be one, `signerVerifier` on each key alone, the agent's audience equal to the hub's, `ca-init` per key), S6 (TLS 1.3, Postgres verify-full, the Jobs' network), S7 (no Kubernetes Secret but the agent's empty one; the migrator's Secret for the migration Job alone; no value in a secret container's spec or metadata, nor in the value of an allowed label or annotation, a finalizer, a `generateName` or an owner reference; the webhook secret never printed or left in the working directory; `envFrom` and `secretKeyRef` refused), S8 (the vault key and its one role), S9 (the audit checkpoint key and bucket; the seed record: kept by the chart for `seed.content`, and for a person-made ConfigMap only while the person keeps it, residuals 37 and 38), S10 (customer-managed keys and encryption of the Secret Manager containers), S16 (hardening of every pod, the Jobs included), S17 (the agent's Role and exact egress, the config-server's one namespace), S18 (default deny, the Jobs' policy, no second policy; the one-off `ca-init` pods' open egress is said and is a residual), S19 and S20 (pinned actions and images, SBOM, signing, Binary Authorization in dry run), S21 (the Jobs' contract, the withheld settings, numbers that the chart and the hub read alike). The tests are named in the rows of the deployment section of `docs/threat-model.md`.

## New dependencies and why

None in this round: no `Cargo.toml`, `Cargo.lock`, `package.json`, `pnpm-lock.yaml`, `deny.toml`, `Justfile`, tool pin or image digest changed after `7ff2d6c`. The tools, images, schemas and model of the whole branch, with versions and ages, are in the history part of the evidence file (no Rust crate and no npm package was added by this prompt).

## Assumptions

- **A32 (human amendment, PR 29):** images are Debian 13. **B10 (human):** the CloudNativePG `system-trixie` image with the built-in barman. **A19 (human, 2026-10-26):** the audit checkpoint bucket in prod keeps three months, locked. **A5:** Binary Authorization stays in `DRYRUN_AUDIT_LOG_ONLY` until Q24.
- **The seed Job is kept (`Prune=false`), so old seed Jobs accumulate by design; the chart keeps the seed ConfigMap only when it made it (`seed.content`).** In staging and prod the file is a ConfigMap a person makes (`seed.existingConfigMap`), named with the revision and never edited or deleted in place; the chart refuses a name that does not end with the revision, and cannot see an edit, a deletion or a revision used twice (residuals 37 and 38). Argo CD's reading of the annotation is **not verified** (its documentation was not reachable): it may show the Application as `OutOfSync` while an old Job exists.
- **`seed.revision` is a positive integer** (it was a free string; the committed values said `"1"` and now say `1`, with ConfigMaps named `...-1`). **The numbers of `config.tuning` are JSON numbers in the hub's own ranges**, copied from the hub's code; nothing compares the copy (residual 34).
- **The gcp chart's release name is `lanekeeper-gcp`** (the Application's name, which `argocd.rego` holds to `helm.releaseName`), and `kms.rego` now requires it as the name and instance label of a secret container.
- **`LK_START_TIMEOUT_SECS` is capped at 8**: the hub serves `/healthz` only when its whole start has finished, and the start is six steps in a row, each bounded by it (`crates/hub/src/process.rs`; the six is a constant in `_helpers.tpl`).
- **The caches may exceed their defaults (512 and 256 MiB) by at most the limit's headroom over the request.** This assumes that the P11 figure (1536 MiB) was measured with the default caches; **not verified**.
- **HSM in prod** is this implementation's reading of S5, enforced by `kms.rego`; no decision records it.
- **The `ca-init` pods' `--overrides`, every `gcloud` command of the install guide, and Argo CD's behaviour with the hooks** are from memory or from source, marked Not verified, and never run.
- **Risk:** `hub-fuzz` builds the whole hub; the fuzz jobs now free disk first, as the test job does, but `hub-fuzz` has not run on the new head.

## Needs a human before merge

1. **B5:** run `node deploy/tools/github-hook-ranges.mjs --write` on a machine with normal GitHub API access and commit `deploy/values/hub/github-hook-ranges.yaml`; the gcp chart also refuses to render without the Binary Authorization attestor's public key (B12).
2. **Acceptance item 3:** accept that the production image cannot be Ready in kind and that the kind job asserts what it can do (the Jobs, the database, and each replica stopping at its first Google step), or change the item. A dev-only image with fakes is what G23 forbids.
3. **HSM in prod:** record the rule in `docs/decisions.md` (a human's file; it was not touched), or relax `kms.rego`. The same for software keys in staging.
4. **The start timeout:** a value above 8 s needs a wider startup probe window, which is a change of the contract in `crates/hub/HARDENING.md` and its test. Confirm the six steps and the 768 MiB assumption against a running pod.
5. **The first sync:** the hooks (`PreSync`, ordering of the Jobs' account and policy, failure) and `Prune=false` on the seed Job are the first real test of Argo CD's behaviour; decide who removes old seed Jobs and when, knowing that removing one removes its record. **Keep every seed ConfigMap** (staging and prod: the chart does not), review each change of `seed.revision` for a number used before, and look at the new seed Job after each sync (residuals 37 and 38).
6. **Red CI that is not this branch's:** `osv` and `supply-chain` (RUSTSEC-2026-0414; the fix is 14 days old on 2026-11-04, or a time-boxed ignore, which is a human's call and not this prompt's files); the fuzz jobs' `cargo deny` (rustls-webpki and webpki-root-certs, until 2026-11-02 and 2026-11-03).
7. **The `ca-init` pods' network:** `kubectl run` pods carry `run=<name>`, which no NetworkPolicy selects, so their egress is open for the minutes they hold a signing role. The guide says so and gives no policy (what it should allow is not verified); decide whether to write one.
8. **Real values** (rule 12): project, hostnames, ranges, key versions, image digests and `targetRevision` are examples or placeholders; the install guide orders the two rounds of pull requests this needs.
9. **Branch protection:** `ci.yml` no longer holds the `rust`, `web` and `pinned-actions` jobs; the required-check names need updating after the merge.
10. **B11 to B13:** the CI signing key, its Workload Identity pool and the Binary Authorization trust root, and the RPM release key and its trust root, are made by no chart; a release tag stops at `sign-release` until they exist.

## Reviewer agent verdict

The second independent review, at `de30902` (it re-checked round 1): **CHANGES REQUESTED** (two blocking, six non-blocking, and two declined by the orchestrator). This round answers it; **re-review is pending**. The first review, at `7ff2d6c`, was answered by round 1. The earlier rounds (`plans/09-deployment-ci.review.md`) ended in APPROVE at `a5f46a4`, before prompt 18 was merged.

## Labels

No `contract-change` label: `Contract-change: none`, and no contract path changed (`perf/budgets.toml`, `proto/`, `db/migrations/`, `api/`, `crates/domain` and `crates/ports` are untouched by this round).

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_011wuVg8Nmkp8WWHzDsARGuX
