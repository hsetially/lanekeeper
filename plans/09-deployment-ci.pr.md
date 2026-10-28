# 09: Deployment, CI and supply chain

**Not mergeable green, and not claimed to be.** Ready for human review with these gates open: B5 (GitHub's hook ranges: the gcp chart cannot be rendered with staging or prod values), acceptance item 3 (`/readyz` on three replicas: not met, and the production image cannot meet it in kind), and the red CI jobs listed below, each explained. Nothing is ticked that a command did not prove, and every statement about GitHub says **as observed on GitHub at `62d23dc`**: it was read from GitHub's pages by the orchestrator and not produced here. **The run for this branch's newest head is pending.** The evidence is `plans/09-deployment-ci.evidence.md` (read its first part; the history below it is superseded); the blockers are `plans/09-deployment-ci.blockers.md`; the chart alignment after prompt 18 is `plans/09-deployment-ci.chart-alignment.md`; the earlier review rounds are `plans/09-deployment-ci.review.md`.

## Prompt

`prompts/09-deployment-ci.md` · plan: `plans/09-deployment-ci.md` (approved by a human; amended once by a human through PR 29, A32: images moved to Debian 13) · wave: 1

Commits: `09/T1:` to `09/T8:`, then `09/blocked:`, `09/fix:`, `09/evidence:` and `09/review:` commits, and merges of `origin/main` (the last brought in prompt 18, PRs 40 to 43, as `cb4367c`). The `09/fix:` commits since then: the hub chart's settings, migration Job and seed Job (`30b53a1`, `66ba049`), the kind job's new assertions (`972b361`), the agent chart's config-server namespace (`77481be`), the gcp chart's keys, secrets and IAM (`b5db76d`), lint findings (`5a3ab7d`), the install guide (`1f8fe02`, `99a00b9`), and review round 1 (below).

## What the last review found, and what was done

An independent reviewer (it re-ran the gates and mutation-tested 99 rules) returned **CHANGES REQUESTED** at `7ff2d6c`: one blocking finding and eleven non-blocking. All twelve were checked against the code and were real; none was rejected. Rules were fixed with tests that failed first, and each new rule was removed from a copy of the policies to show the tests fail without it.

| Finding | Fixed in |
|---|---|
| B-1 the deployment section of the threat model stated things as fact that the earlier rounds made false, and had no rows for the Jobs, the keys and secrets, the `ca-init` identity, the config-server rule or the agent's audience | `b03e76a` (rows added, residuals 29 to 36 added, 3, 5, 7, 10, 11, 13, 20 and 23 corrected; nothing else in the file) |
| NB-1 `envFrom` bypassed the env rules | `b2ca79f` |
| NB-2 a Job of another name could mount the migrator's Secret | `d56b3fc` |
| NB-3 a second NetworkPolicy defeated default deny | `41c654c` |
| NB-4 a secret container's metadata could hold a value | `0fa1524` |
| NB-5 the install guide (per-key `ca-init`, a hardened one-off pod, no `fqdnPolicy: false` for staging and prod, start-to-Ready is not P11, no printed secret, Argo CD marked Not verified) | `c00930e`, `0aa1153` |
| NB-6 `HARDENING.md` contradicted the chart (the Service name, and what `/healthz` waits for) | `9dbf41c` |
| NB-7 Argo CD would prune the seed Job, the record the contract relies on | `0aa1153`, `2b7852f` |
| NB-8 tunables that get the pod killed (start timeout, caches) | `bc393e3`, `f9471a7` |
| NB-9 three settings kept off by the schema alone | `5feab3e` |
| NB-10 this file and the evidence file | this commit |
| NB-11 `hub-fuzz` without a free-disk step | `5eea693` |

## Gates

Run here on the tree at `2b7852f` (this sandbox has no Docker daemon and about 6.7 GB of disk; `conftest` ran from the pinned binary through a stand-in for `docker`). Docs and plans commits follow it.

| Gate | Result |
|---|---|
| `conftest verify --policy deploy/policy` | **1349 of 1349** (1309 at `7ff2d6c`) |
| `node --test .github/scripts/*.test.mjs` | **205 of 205** (the install guide's test, 31 of 31, is part of it) |
| `deploy/tests/render.sh lint`, `contract`, `data`, `argocd` | each exit 0 |
| `deploy/tests/render.sh render`, `policy` | **exit 1, B5 only**: the gcp chart with staging and prod values (`cloudArmor.githubHookRanges is empty`); every other chart and environment passes |
| `deploy/tests/render.sh planted` | **exit 1, B5 only**: 779 cases ok, no `PLANTED FAILED` line, and one counted failure, the Cloud Armor group, which is not run because the gcp chart cannot render. (A first run on `5eea693` had 778 ok and a second failure: an older planted case that the new `Prune=false` annotation had broken, repaired in `2b7852f`.) |
| `just ci-lint` | exit 0 (actionlint, zizmor offline, SHA pins, workflow rules, shellcheck over 35 scripts) |
| `node deploy/schemas/verify.mjs` | exit 0 (38 files equal the lock) |
| `cargo test -p hub --test hardening`, `--test justfile_gate` | 7 of 7 and 8 of 8 |

**`cargo xtask bench-check`: not re-run in this round.** The `09/fix:` commits of this round change `deploy/`, `docs/`, `plans/`, `.github/workflows/` and one comment in `crates/hub/tests/hardening.rs`: no crate source, no benchmark, no `perf/` file. The two earlier misses (B15: `P6.index_settings`, `P12.audit_record`) are, as reported by the orchestrator and not checked here, settled by PR 34 on `main`. P11 (the only P# of this prompt) is asserted per replica by the kind job and reads **NOT MEASURED**. `just verify` and `just verify-09` as single commands were not re-run either: `verify-09` stops at `deploy-render` for B5, and `just verify` takes about 80 minutes.

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

- [ ] `just verify-09` passes. **NOT MET (B5).** `render.sh lint`, `contract`, `data` and `argocd`, `just ci-lint`, the 1349 policy tests and the SHA-pin test pass; `render` and `policy` fail for the gcp chart with staging and prod values, because the hook ranges file is the unresolved marker.
- [ ] Images are signed, and their SBOMs and provenance attestations are published. **NOT PROVEN.** SBOMs are made and scanned (the `image` and `scan-test` jobs passed on GitHub at `62d23dc`). Signing, the SBOM attestation, provenance and the Cloud KMS signature happen in the `publish` job, on `main` and tags, and have not been seen; only a key-based flow ran, against a registry on localhost.
- [ ] A kind cluster in CI runs the hub chart with CloudNativePG, and `/readyz` passes on all 3 replicas. **NOT MET.** The production image reaches Secret Manager, Cloud KMS, Cloud Storage and Entra at start (prompt 18, G23: no fakes in a release build), so in kind it stops at its first Google step. The kind job asserts what the image can do and passed at `62d23dc` (above); it reports `/readyz` and P11 as not measured. The reading of this item is a human's decision (below).

Each task's Verify line:

- [x] T1: images build reproducibly (the hub `image` job passed on GitHub at `62d23dc`; `just images-repro` for both images ran at an earlier head and no Dockerfile changed in this round) and have a CycloneDX SBOM and a scan that fails on critical findings (`scan-test` passed).
- [x] T2: `actionlint` passes (`just ci-lint`) and a test fails if an action reference is not a SHA (`check-pins.test.mjs`, in the 205).
- [x] T3: kubeconform passes and the policy tests reject a pod missing any hardening field (`render.sh render` and `policy` for the hub chart in kind, staging and prod; 1349 tests).
- [x] T4: kubeconform passes and the Role has no Secret list (`render.sh contract`).
- [ ] T5: db and gcp charts. **PARTIAL (B5).** The db chart and `gcp-storage` pass `render.sh data`; the gcp chart with Cloud Armor cannot render.
- [ ] T6: every values file through conftest. **PARTIAL (B5).**
- [x] T7: the threat-model section, updated in this round (new rows and residuals; the tests it names exist, checked by script).
- [ ] T8: the RPM installs and starts on Rocky 8.9 and 8.10. **PARTIAL (B13):** both installs passed on GitHub at `62d23dc`; the real `audit` package and the release key do not exist.
- [x] T8: `systemd-analyze security` exposure score 2.0 or lower (1.7, earlier head).
- [ ] T8: the RPM's signature verifies. **PARTIAL (B13):** with a throwaway key only.

## Security requirements implemented (S#)

S5 (two CA keys that cannot be one, `signerVerifier` on each key alone, the agent's audience equal to the hub's, `ca-init` per key), S6 (TLS 1.3, Postgres verify-full, the Jobs' network), S7 (no Kubernetes Secret but the agent's empty one; the migrator's Secret for the migration Job alone; no value in a secret container's spec or metadata; `envFrom` and `secretKeyRef` refused), S8 (the vault key and its one role), S9 (the audit checkpoint key and bucket; the seed record kept), S10 (customer-managed keys and encryption of the Secret Manager containers), S16 (hardening of every pod, the Jobs included), S17 (the agent's Role and exact egress, the config-server's one namespace), S18 (default deny, the Jobs' policy, no second policy), S19 and S20 (pinned actions and images, SBOM, signing, Binary Authorization in dry run), S21 (the Jobs' contract, the withheld settings). The tests are named in the rows of the deployment section of `docs/threat-model.md`.

## New dependencies and why

None in this round: no `Cargo.toml`, `Cargo.lock`, `package.json`, `pnpm-lock.yaml`, `deny.toml`, `Justfile`, tool pin or image digest changed after `7ff2d6c`. The tools, images, schemas and model of the whole branch, with versions and ages, are in the history part of the evidence file (no Rust crate and no npm package was added by this prompt).

## Assumptions

- **A32 (human amendment, PR 29):** images are Debian 13. **B10 (human):** the CloudNativePG `system-trixie` image with the built-in barman. **A19 (human, 2026-10-26):** the audit checkpoint bucket in prod keeps three months, locked. **A5:** Binary Authorization stays in `DRYRUN_AUDIT_LOG_ONLY` until Q24.
- **The seed Job and its ConfigMap are kept (`Prune=false`), so old seed Jobs accumulate by design.** Argo CD's reading of the annotation is **not verified** (its documentation was not reachable): it may show the Application as `OutOfSync` while an old Job exists.
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
5. **The first sync:** the hooks (`PreSync`, ordering of the Jobs' account and policy, failure) and `Prune=false` on the seed Job are the first real test of Argo CD's behaviour; decide who removes old seed Jobs and when, knowing that removing one removes its record.
6. **Red CI that is not this branch's:** `osv` and `supply-chain` (RUSTSEC-2026-0414; the fix is 14 days old on 2026-11-04, or a time-boxed ignore, which is a human's call and not this prompt's files); the fuzz jobs' `cargo deny` (rustls-webpki and webpki-root-certs, until 2026-11-02 and 2026-11-03).
7. **Real values** (rule 12): project, hostnames, ranges, key versions, image digests and `targetRevision` are examples or placeholders; the install guide orders the two rounds of pull requests this needs.
8. **Branch protection:** `ci.yml` no longer holds the `rust`, `web` and `pinned-actions` jobs; the required-check names need updating after the merge.
9. **B11 to B13:** the CI signing key, its Workload Identity pool and the Binary Authorization trust root, and the RPM release key and its trust root, are made by no chart; a release tag stops at `sign-release` until they exist.

## Reviewer agent verdict

An independent review at `7ff2d6c`: **CHANGES REQUESTED** (one blocking, eleven non-blocking). This round answers it; **re-review is pending**. The earlier rounds (`plans/09-deployment-ci.review.md`) ended in APPROVE at `a5f46a4`, before prompt 18 was merged.

## Labels

No `contract-change` label: `Contract-change: none`, and no contract path changed (`perf/budgets.toml`, `proto/`, `db/migrations/`, `api/`, `crates/domain` and `crates/ports` are untouched by this round).

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_011wuVg8Nmkp8WWHzDsARGuX
