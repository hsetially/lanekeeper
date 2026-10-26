# Plan 09 blockers

Status on 2026-10-26, T8 run: **B1 to B4 are resolved; B5 is open by a human's decision and stops `just verify-09` (and nothing else of T3 to T8); B7 is resolved on 2026-10-26 (the agent load balancer listens on 443) and B8 is informational and needs a human before a swimlane is deployed; B9 is informational; B10 is decided (a human confirmed the interim image, below); B11 and B12 are informational findings of T6; B13 is an informational finding of T8: the RPM release key and its trust root do not exist, and the Rocky and Debian repositories are refused, so the real `audit` package and a musl C toolchain could not be installed.**
**Review round 1 (FIX step, 2026-10-26, commits `643a896` to `df624a6`): four blocking findings fixed in the repository (disk steps lost in the merge with `origin/main`, a build step in the `sign-release` job, `storage.googleapis.com` missing from the hub's egress, the kind test missing) and one non-blocking one; one new blocker, B16 (no kind cluster can start in this sandbox); B5, B14, B15 and A19 untouched, as instructed. Status stays PARTIAL. See the last section.**
**Review round 2 (FIX step, 2026-10-26, commits `077e32e` to `3a67982`): one blocking finding fixed (the `sign-release` job compiled the sentinel through `build.sh --tools`; the tools image is now a stage of its own that builds nothing of ours) and three non-blocking ones (the workflow rule's quoting and shell blind spots, the kind test's `/readyz` proxy read and its P11 assertion, Renovate's coverage of the kind pins). B14 is resolved by PR 30 (merge `7e53bf5`, brought in by `9eef6b4`): `just verify` passes. B5, B15, A1 and A19 were left alone, as instructed. Status stays PARTIAL. See the last section.**
The T1 sections below are from the third T1 run (after a human allowed `us.aws.cdn.hf.co` and merged the plan amendment, PR #29):
T1 is complete; the commit `e2c199d` holds the code and the measurements were taken at it.

| | State | Resolved by |
|---|---|---|
| B1 | resolved | the model lock was filled in by `update-model-lock.sh` from my own download (below) |
| B2 | resolved (second run) | `scan.sh` reaches the grype database and names findings by CVE id (below, unchanged) |
| B3 | resolved | plan amendment A32 (PR #29): the runtime and builders moved to Debian 13; no exception in `grype.yaml` |
| B4 | resolved (T3 run) | a human allowed `get.helm.sh`; helm 4.3.0 is pinned by the sha256 of a download made here (section B4 below) |
| B5 | **open** (T3) | `api.github.com/meta` is refused, so GitHub's webhook ranges cannot be written; the gcp chart refuses to render until a person runs one command (section B5, T3 notes below) |
| B6 | informational (T3) | `docs.cloud.google.com` and `registry.k8s.io` are refused; Google-specific shapes in the hub chart are unverified (T3 notes below) |
| B7 | **resolved (human decision of 2026-10-26)** | the agent contract dials the hub on 443; the hub chart's agent load balancer listened on 8444. `Service/lanekeeper-hub-agent` now listens on 443 and forwards to the container port `agent` (8444); neither contract file changed (section B7 below) |
| B8 | **open, informational (T4)** | the agent contract lets the agent reach the config-server only in the agent's own namespace (T4 notes below) |
| B9 | **open, informational (T5)** | `pkg-containers.githubusercontent.com` (where `ghcr.io` keeps image layers) is refused: the database image's layers and config cannot be read here (T5 notes below) |
| B10 | **decided (T6 run)** | the plan names CloudNativePG's `standard` image for the database and built-in barman for the backups; the `standard` image has no barman. **A human confirmed the interim image** (`16.15-202610050823-system-trixie`, built-in barman). Moving to the Barman Cloud plugin is a later open question (B10 section below) |
| B11 | **open, informational (T6)** | no operation creates a swimlane in the hub, so step 1 of `deploy/ADDING-A-SWIMLANE.md` cannot be completed by following it (T6 notes below) |
| B12 | **open, informational (T6)** | what Binary Authorization needs that no chart makes, and what could not be confirmed without Google's documentation (T6 notes below) |
| B13 | **open, informational (T8)** | the RPM release key and its Workload Identity trust root are made by no one (`RPM-GPG-KEY-lanekeeper.pub` is a marker); `dl.rockylinux.org`, `mirrors.rockylinux.org` and `deb.debian.org` are refused, so the install test ran with a labelled stand-in for `audit` and the musl build has no C toolchain (T8 notes below) |
| B14 | **resolved (review round 2)** | PR 30 (merge `7e53bf5`, the lockfile bump with the rule 15 override in `docs/decisions.md`) reached this branch in the merge `9eef6b4`. `cargo deny check` and `cargo audit` exit 0 and `just verify` passes as one command on `3a67982` (80 min). The branch changed none of `Cargo.lock`, `Cargo.toml`, `deny.toml`, `.cargo/audit.toml`. The section below is kept as it was written |
| B16 | **open, informational (review round 1)** | the kind end-to-end test is written but cannot be run here: the host is cgroup v1 and runc in a kind node fails (`failed to update /proc/self/oom_score_adj: Permission denied`), and the CloudNativePG operator and database images are on a refused host (B9). It is also red by design until A1 (section "Review round 1") |
| B15 | **open, needs a human (FINAL, benchmarks)** | two registered budgets are missed on every run, including five runs on a quiet machine: `P6.index_settings` (prompt 04's bench, p95 62.4 to 72.1 ms against 60 ms) and `P12.audit_record` (prompt 03a's bench, p95 3.14 to 3.59 ms against 3 ms). Not caused by this branch (it changed neither crate nor `perf/budgets.toml`). Nothing was changed to make them pass (section B15 at the end) |

## B1 (T1): resolved. The model lock is filled in, from a download made here

**The problem (kept for the record).** huggingface.co serves LFS files through its CDN: `resolve/<rev>/onnx/model.onnx`
answers `302` to `https://us.aws.cdn.hf.co/xet-bridge-us/...`, which the egress proxy refused, so the ONNX graph could not
be downloaded and hashed and `hub-model.lock` stayed `UNRESOLVED`. A human then allowed `us.aws.cdn.hf.co`.

**What I ran.** `deploy/images/update-model-lock.sh 5c38ec7c405ec4b44b94cc5a9bb96e735b38267a` (4.4 s, exit 0). It resolved
the revision, downloaded and hashed all five files here, and compared each LFS file's hash with the one Hugging Face
reports; it stops on a difference. No hash was copied from anywhere else.

**The result (`git diff deploy/images/hub-model.lock`).**

| Item | Value |
|---|---|
| repo, revision | `BAAI/bge-small-en-v1.5` at `5c38ec7c405ec4b44b94cc5a9bb96e735b38267a` ("onnx support (#9)"), the revision `main` resolves to today |
| commit date | `2024-02-22T03:36:23Z`, 976 days before 2026-10-25 (rule 15 needs 14) |
| `onnx/model.onnx` | sha256 `828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35`, 133093490 bytes; equal to the LFS sha256 Hugging Face reports |
| `tokenizer.json` | sha256 `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66`, 711396 bytes |
| `config.json` | sha256 `094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750`, 743 bytes |
| `special_tokens_map.json` | sha256 `b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3`, 125 bytes |
| `tokenizer_config.json` | sha256 `9261e7d79b44c8195c1cada2b453e55b00aeb81e907a6664974b4d7776172ab3`, 366 bytes |

The four small files equal the hashes that the previous run recorded from its own download of them. The checker passes
(`node deploy/images/model-lock.mjs check deploy/images/hub-model.lock`: "5 files of BAAI/bge-small-en-v1.5 at
5c38ec7c..."), and so does `just images-policy-test`, whose last step is that check on the committed lock.

**Proof in the image.** The hub image built from the committed tree contains `/app/models/bge-small-en-v1.5/` with these five
files; I re-extracted them from the image's OCI layers and their sha256 and sizes equal the lock's (mode 0644, owner root,
so the hub's uid 65532 can read and not write).

**Found while closing it (fixed in `build.sh`).** The first hub build with a real lock failed in the `model` stage with
curl exit 6, "could not resolve host": behind the egress proxy the build containers have no DNS for huggingface.co. A
container on the host network with `HTTPS_PROXY` and the CA bundle reached it (HTTP 200), and one without them did not.
`deploy/images/build.sh` now passes `HTTPS_PROXY` and `NO_PROXY`, when set, as BuildKit's predefined build args, which BuildKit
keeps out of the image history (checked: no proxy text in the hub image's config or history). CI on GitHub-hosted runners
sets none of them, and the Dockerfile is unchanged by this.

## B2 (T1): resolved

The scan now runs. Two defects in `scan.sh` were found and fixed while doing it:

1. **The grype container could not reach the database.** It ran on Docker's bridge network with no proxy, so the lookup of
   `grype.anchore.io` went to 8.8.8.8 and failed (`no such host`), even with the host allowed. `scan.sh` now takes the
   same settings as `build.sh`: `LK_BUILD_NETWORK` goes to `--network`, `LK_BUILD_CA` is mounted read-only as the
   container's CA bundle (`SSL_CERT_FILE`), and `HTTPS_PROXY` and `NO_PROXY` are passed by name when set. CI sets none
   of them.
2. **The scan reported the planted log4j finding by its GHSA id** (`GHSA-jfh8-c2jp-5v3q`), so `scan-test.sh` correctly
   failed: it requires `CVE-2021-44228` in the output. `scan.sh` now passes `--by-cve`, so findings are named by CVE id
   where they have one. That also matches `grype.yaml`, whose exceptions are written with CVE ids. The test was not changed.

Evidence (grype v0.120.1, digest-pinned in `tools.env`; database v6.1.10 built 2026-10-25T06:27:34Z, checksum
`sha256:4bdb4292…a713ea`, cached in `target/grype-db`):

- `just images-scan-test`: `scan-test: the planted critical finding (CVE-2021-44228) fails the scan` (exit 0). The scan
  itself exits 2: `log4j-core 2.14.1 ... CVE-2021-44228 Critical`, `discovered vulnerabilities at or above the severity
  threshold`.
- Mutation: with `--fail-on critical` removed from `scan.sh`, the test fails (`the planted critical SBOM passed the
  scan`, exit 1). Restored, it passes again.
- `shellcheck -x` (koalaman/shellcheck v0.11.0) is clean on `scan.sh`, `update-model-lock.sh` and `scan-test.sh`.

## B3 (T1): resolved. The images moved to Debian 13, and both pass the scan with no exception

**The problem (kept for the record).** `just images-scan agent` failed on two critical findings in the deprecated
`gcr.io/distroless/cc-debian12:nonroot` (libc6 2.36 CVE-2026-5450, won't fix in Debian 12; libssl3 3.0.20 CVE-2026-75803,
fixed only in 3.0.22). Nothing newer than that base existed, and the crates added no finding of their own.

**The decision.** A human chose option 1 and merged it: plan 09 A32 (PR #29). The runtime is
`gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2`, the builder
is `rust:1.88.0-trixie` pinned by digest, no exception goes in `grype.yaml`, and `--fail-on critical` stays. The plan's
Extra-paths gained `crates/agent/tests/hardening.rs` and `crates/agent/HARDENING.md`.

**What changed (commit `e2c199d`).**

- Both Dockerfiles: every Rust stage (`planner`, `build`, and the hub's `model`) is now
  `docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db`, and the
  hub's web stage is `node:22.20.0-trixie-slim@sha256:f3bca16c4b87ad0c7abfad97f6803898ee9b08a1f153a269140f0a08f50231d2`
  (it only makes static files, but no Debian 12 image is named any more). The rust builder, checked by running it, is
  Debian 13 (trixie) with rustc 1.88.0, glibc 2.41, `cc` 14.2.0, `curl` and `sha256sum`; the runtime's packages are
  Debian 13.7 (from the SBOM).
- Digests, read from the registries (`docker buildx imagetools inspect`, index digests, not guessed): the rust one from
  registry-1.docker.io and again from mirror.gcr.io (same digest); the node one from mirror.gcr.io and then confirmed
  against docker.io; the distroless one is the plan's, an index of linux/amd64 and linux/arm64 manifests, read from gcr.io;
  the Dockerfile frontend's digest was re-read from docker.io. The mirror was used because Docker Hub's anonymous pull
  limit for this egress IP (100 an hour) was exhausted and answered 429 for a while; the limit reset
  (`ratelimit-remaining: 100`) before the final build.
- Tests first, in `crates/agent/tests/hardening.rs` and `crates/hub/tests/hardening.rs`: the expected images became trixie
  and cc-debian13 and ran red against the old Dockerfiles (2 failures in each crate); the "builder and runtime are the same
  Debian release" check, which was only implied by the literal strings, is now a function (`debian_release`) with two
  planted violations (the runtime on Debian 12, the builder on Debian 12), and no non-comment line may name `bookworm` or
  `cc-debian12`. With the new check disabled, the planted case is accepted and the test fails (mutation, both crates).
- Both `HARDENING.md` image sections, `deploy/policy/dockerfile_test.rego` and the planted Dockerfile moved too.
  `dockerfile.rego` itself names no release and needed no change. `bookworm` and `cc-debian12` now appear only in the
  tests (the release map, the planted violations, the check that none is left), in the sentences that say why the images
  moved (Dockerfile comments, `HARDENING.md`), and, in the plan, in T5's CloudNativePG image, which is not one of these
  images.

**The scan (grype v0.120.1, database v6.1.10 built 2026-10-25T06:27:34Z, checksum `sha256:4bdb4292…a713ea`, the same file
as in B2).**
`just images-scan agent` and `just images-scan hub`, each over the image archive and over its merged SBOM, both exit 0:
"agent has no critical findings", "hub has no critical findings". In each of the four passes (archive and SBOM, per image):
0 critical, 11 high, 10 medium, 3 low, 7 negligible; the counts are the same for both images, so the 431 and 301 Rust
crates add nothing. The 11 highs are the base's (libc6, gcc-14-base, libgcc-s1, libgomp1, libstdc++6, zlib1g), each "won't fix" or without a fix upstream, as the
amendment predicted. CVE-2026-5450 and CVE-2026-75803 no longer appear in either output. `deploy/images/grype.yaml`
(`ignore: []`) and `scan.sh` (`--fail-on critical`) are unchanged since the amendment.

## What this run proved

Proved on 2026-10-25 (Docker 29.9.2, its built-in BuildKit, 4 vCPUs, `LK_BUILD_CA=/root/.ccr/ca-bundle.crt`,
`LK_BUILD_NETWORK=host`), on the committed tree at `e2c199d`:

- `just images-repro`, with the build cache emptied first, builds each image twice and the two digests match (the archives
  are byte for byte equal): hub `sha256:0117df0be05cd1a3fb6d49b318000dd81544a7a2bbee7a89f92da7c24a5c0e85`, agent
  `sha256:35d95ba845bd7398cb427553ecd6a0590c49dbde0cd0cf71d3d0d62ee3c4750c`. Recorded in `REPRODUCIBILITY.md`.
- `just images-sbom hub` and `agent`: `hub.cdx.json` with 1723 components (431 crates, 14 Debian 13.7 packages) and
  `agent.cdx.json` with 1593 (301 crates, 14 packages); CycloneDX, named by the image digest.
- `just images-scan hub` and `agent`: no critical finding (B3 above).
- `just images-scan-test`: "the planted critical finding (CVE-2021-44228) fails the scan" (exit 0).
- `just images-policy-test` (exit 0): lock tests 25/25, Rego unit tests 22/22, the planted Dockerfile triggers every expected
  rule, both Dockerfiles pass the policy (30/30), `cargo test -p hub --test hardening` 5/5, `cargo test -p agent --test
  hardening` 5/5, the model-stage test accepts the matching lock and refuses all seven planted mismatches, and the committed
  lock passes the checker.
- `cargo fmt --check` and `cargo clippy --test hardening -D warnings` are clean for both crates; `shellcheck -x` (v0.11.0)
  is clean on all the scripts under `deploy/images`.

Nothing is left open in T1.

# T2 notes (2026-10-25)

T2 is complete except for two items that the sandbox cannot do. Neither stops the T2 Verify (`actionlint` passes and `pins-test`
refuses a tag, a branch, a short SHA and an undigested `docker://` reference), so this is not a stop; the first needed a human
before T3, who allowed the host, and B4 is resolved (below).

## B4 (T2, needed by T3): resolved. helm is pinned, from a download made here

**The problem (kept for the record).** The plan's T2 row pins helm, kubeconform, conftest, actionlint and kind with a version
and a sha256 (A22). helm's release files are served from `get.helm.sh`, which the proxy refused during T2 (the CONNECT was
answered 403, `curl -I https://get.helm.sh/helm-v3.18.0-linux-amd64.tar.gz` gave HTTP code 000, the proxy status listed
`connect_rejected get.helm.sh:443`). The helm GitHub release has no binary assets. T2 therefore pinned the other four tools and
left helm out; T3 is the first task that runs it. A human then allowed `get.helm.sh`.

**What was done (T3 run, 2026-10-25).**

| Step | Result |
|---|---|
| Which version | `git ls-remote --tags https://github.com/helm/helm` lists 3.22.1 and 4.3.1 as the newest of each line. Tag dates, read by fetching each tag: v4.3.1 on 2026-10-14 (11 days old, under rule 15's 14), v3.22.1 on 2026-10-15 (10 days), v4.3.0 on 2026-09-09 (46 days), v4.2.4 on 2026-08-13. The newest release that is at least 14 days old in the 4 line is **4.3.0**. The 4 line is chosen over 3 because Helm 3 is in its security-only window, which ends within weeks of today. Renovate, with `minimumReleaseAge` of 14 days, proposes 4.3.1 once it is old enough. |
| Download | `curl https://get.helm.sh/helm-v4.3.0-linux-amd64.tar.gz` into a new empty directory: HTTP 200, 20321063 bytes |
| Hash | `sha256sum` of those bytes: `86584a54def73570558f66f5111cc53dfed56689637ae32c1201205d494f54fb`. This is the pin. It was computed here, not copied. |
| Cross-check | the release's own `helm-v4.3.0-linux-amd64.tar.gz.sha256sum`, fetched afterwards, holds the same value. It was read only to compare, after the hash was computed. |
| The binary | `linux-amd64/helm` from that archive reports `v4.3.0+gbec5b06`; tag v4.3.0 is commit `bec5b06ed841fe5269972d864d5177944fd5970f`, so the binary and the tag agree |
| Justfile | `helm_version := "4.3.0"` and `helm_sha256` next to the other pins; a `fetch helm` line (member `linux-amd64/helm`) in `ops-tools-install`; `want helm` and the `helm` case in `ops-tools-check` (it compares `helm version --short` without the `+g<commit>` suffix) |

**Proof.** Before the edit `just ops-tools-check helm` failed with `unknown tool 'helm'` (red). After it:

- `just ops-tools-check helm` with no helm installed fails with `need helm v4.3.0, found 'nothing'; run: just ops-tools-install helm`.
- `just ops-tools-install helm` downloads the file again and installs it ("ops-tools-install: helm installed"); the Justfile's pin matched
  the bytes of a second, independent download.
- `just ops-tools-check helm` then exits 0, and so does `just ops-tools-check` for every tool.
- Mutation, pin: `just --set helm_sha256 000...0 ops-tools-install helm` refuses the file ("has sha256 86584a54..., the pin is 0000...").
- Mutation, version: `just --set helm_version 4.2.4 ops-tools-check helm` fails with `need helm v4.2.4, found 'v4.3.0'`.
- `node --test .github/scripts/*.test.mjs`: 77 of 77 pass.

## Not provable in the sandbox (recorded so the first CI run is read as the test)

| What | Why it could not be run here | What was run instead |
|---|---|---|
| The workflows (`rust`, `web`, `contracts`, `bench`, `nightly`, `images`, `ci`) on GitHub | no route to GitHub's runners | `actionlint` 1.7.12 with the pinned shellcheck on every `run:` script, zizmor 1.30.1 (offline audits), `check-pins.mjs`, `check-workflows.mjs`; every pinned action SHA was compared with its version tag in the action's own repository (`git ls-remote`) |
| zizmor's online audits (impostor commits, version-comment mismatch, known-vulnerable actions) | `api.github.com` is closed for repositories other than this one | CI sets `LK_ZIZMOR_ONLINE=1` and a `GH_TOKEN`; the SHA-to-tag comparison above is the part of those audits that could be done by hand |
| Keyless signing, the SBOM attestation, `actions/attest-build-provenance`, the KMS signature through Workload Identity Federation, `gh attestation verify` | they need GitHub's OIDC token and the Sigstore services | `deploy/images/tests/publish-test.sh` runs `publish.sh` with the pinned cosign 3.1.3 and crane 0.22.1 against a registry on localhost with a throwaway key: push by digest, sign, attach the SBOM, verify, and every refusal (wrong archive, a tag, a wrong key, an unsigned image, a missing attestation) |
| `osv-scanner` on the lockfiles | `api.osv.dev` is refused by the proxy (osv-scanner then exits 127, it does not report a clean run) | `just osv-scan` was run as far as the API call: it found 11 lockfiles (the workspace's, the 8 of the fuzz crates, the 2 of pnpm) and read 3,885 packages from them, and exited 127 at the refused call |
| The `docker-hub-mirror` action (SIGHUP reload of the daemon, restart as the fallback) | changing the sandbox's Docker daemon settings was not appropriate | `docker-hub-mirror.mjs`, which writes the settings, is unit-tested (13 cases); `builder.sh` with `LK_DOCKER_HUB_MIRROR=mirror.gcr.io` starts a BuildKit that accepts the mirror configuration |
| A full hub or agent build with the pinned BuildKit | not attempted: T1 found that containers on Docker's bridge network have no DNS here, so a container-driver builder would need proxy and CA settings that only this sandbox needs, and the disk had 3 GB free | `builder-test.sh`: the pinned BuildKit v0.34.0 starts, reports its version, and writes one OCI archive for the same files twice with `rewrite-timestamp=true` and two without it (the control); the T1 measurements used the built-in BuildKit of Docker 29.9.2, which is also v0.34.0 |
| The time of the benchmark and fuzz jobs | they do not run here | each job has a `timeout-minutes` (the structural rule `check-workflows.mjs` enforces); the values are estimates from the number of targets (10 minutes each in `nightly`), not measurements |


# T3 notes (2026-10-25)

T3 is built and its checks pass, except those that need GitHub's webhook ranges: **B5 is open.** Everything else the plan's T3
row lists exists, was run, and is shown below.

## B5 (T3): GitHub's webhook ranges cannot be fetched here, so the gcp chart refuses to render

**The host.** `https://api.github.com/meta`. The egress proxy answers 403 with `This GitHub API path is not available: sessions are
bound to their configured repositories. Use repository-scoped endpoints (repos/{owner}/{repo}/...)`. That is a rule of this
sandbox, not a missing allowlist entry for one host: `/meta` is not a repository endpoint. (`docs.github.com` is refused too, but
the ranges would not be copied from a web page in any case.)

**What it blocks.** `deploy/values/hub/github-hook-ranges.yaml` holds the `hooks` list of `/meta` for the Cloud Armor rule that
allows GitHub on `/hooks/github` (plan T3, Q28). A range typed in by hand would let a server that is not GitHub's call the webhook
path, or drop real deliveries, so none was typed in or copied. The file is committed as the **unresolved marker** (an empty
list and a header that says so), the same way `hub-model.lock` was before B1.

**How it shows.** The gcp chart's template stops with `cloudArmor.githubHookRanges is empty ... Run node
deploy/tools/github-hook-ranges.mjs --write`, so `deploy/tests/render.sh` fails for the gcp chart with the prod and staging values
(lint, which Helm runs with `fail` turned off, does not notice), and `just deploy-render`, `policy-check` and `deploy-planted`
exit non-zero. The hub chart, the policies, the schemas, the tools and every other check are unaffected, and they pass.

**To resolve it** (a person with network access, no code change): `node deploy/tools/github-hook-ranges.mjs --write`, then
commit `deploy/values/hub/github-hook-ranges.yaml`. The tool validates what it reads (a `hooks` list of at most 64 CIDRs, none
wider than /16 or /32, so a garbled answer cannot open Cloud Armor) and writes only when all of it passes.
`nightly.yml` job `hook-ranges` runs `--check` every night and fails when the file differs from GitHub's list.

**Proved without the host.** The tool and its tests use a synthetic `/meta` document (documentation addresses, labelled as such
in `deploy/tools/testdata/meta.json`): `node --test deploy/tools/*.test.mjs` 30 of 30. The gcp chart, kubeconform and the policies
were run on a render with six synthetic ranges (swapped in for the run, then restored; `git diff` shows the committed marker): 1 resource
valid, 112 of 112 policy checks pass, and the three Cloud Armor planted cases (`the default rule allows`, `a hook range for every
path`, `no users' ranges`) are each refused for their own reason. With the synthetic file in place `just deploy-check` ran to the
end and only `tools-test` failed, on its check that the committed file is generated output or the unresolved marker, which is
the check doing its job.

## B6 (T3, informational): documentation hosts and what could not be checked against them

`docs.cloud.google.com` (where `cloud.google.com` redirects) and `registry.k8s.io` are refused. Neither is needed for a pin or a
check of T3, so this is not a stop. It does mean that Google's own description of these GKE shapes could not be read:

- the Gateway's `tls.options` keys `networking.gke.io/pre-shared-certs` and `/cert-manager-certs`, and `addresses[].type: NamedAddress`
  (the vendored Gateway API schema allows the type; the options are free-form in the schema);
- the `cloud.google.com/l4-rbs: "enabled"` annotation of the agent Service;
- Cloud Armor's limit of five sub-expressions in a custom expression (the chart splits at four ranges plus the path; a larger real
  limit would only make rules fewer) and the ten ranges of a `SRC_IPS_V1` rule;
- that the proxies of `gke-l7-global-external-managed` and its health checks come from `35.191.0.0/16` and `130.211.0.0/22`;
- the schemas of `GCPBackendPolicy`, `HealthCheckPolicy` and `FQDNNetworkPolicy` are the community catalog's (Google publishes no CRD
  file); they reject unknown fields (checked with planted fields) but may lag the cluster.

They are listed in `deploy/charts/hub/README.md` as "Not verified here". The first apply to a cluster is their test. `registry.k8s.io`
matters only to a local kind run (T5 runs kind in CI).

## Hosts T3 needed, probed from this sandbox

| Host | Result | Used for |
|---|---|---|
| `get.helm.sh` | reachable (a human allowed it) | the helm release file and its checksum (B4) |
| `raw.githubusercontent.com` | reachable | the schema sources, by commit (`deploy/schemas/fetch.py`) |
| `github.com` (git over HTTPS) | reachable | `git ls-remote` and shallow fetches, to read tags, commit ids and dates |
| `docker.io` (registry-1) | reachable | the pinned conftest image |
| `api.github.com/meta` | **refused** | GitHub's hook ranges: **B5** |
| `docs.cloud.google.com`, `registry.k8s.io` | refused | documentation, kind: **B6** |

No kind image was needed by T3 (the kind job is T5).

## What T3 proved (committed tree, 2026-10-25)

| Check | Result |
|---|---|
| `just ops-tools-check` | exit 0, helm v4.3.0 among the tools |
| `python3 -B -I -m unittest` (converter) and `node --test deploy/schemas/verify.test.mjs`, `node deploy/schemas/verify.mjs` | 9 of 9, 10 of 10, `schemas: 36 files equal the lock`; `fetch.py --check` fetched all 36 again and they equal the lock |
| `just policy-test` (`conftest verify`, all namespaces) | 237 of 237: hardening 45, network 95, secrets 22, topology 21, resources 15, images 9, storage 8, dockerfile 22 |
| Mutation of the policies | every `deny` and `warn` rule of the seven manifest policies (114 rules) was disabled one at a time in a scratch copy and `conftest verify` run on each. Three survived (the empty external host, an external rule without ports, and an empty list of GitHub ranges); each got a planted test, and the 59 `network` mutants were run again: none survived. Earlier hand-picked mutations had also shown `test_deny_gateway_http_listener` passing through other rules; it was rewritten to change only the protocol |
| `deploy/tests/render.sh` lint, render, policy, hub chart, kind/staging/prod | helm lint clean; kubeconform `-strict`: 6, 13 and 13 resources valid; conftest 112 of 112 (kind: 4 warnings that are its fallbacks) |
| `deploy/tests/render.sh planted` | 15 template refusals (no digest, short digest, a tag, empty agent ranges, the internet as a range, ...), kubeconform refusing a planted field in each of six kinds and a kind without a schema, and 40 planted changes to the real hub output (one per S16 field, P11, S19, S6/S7, D62, S18, S10) each refused for its own message, plus 3 for Cloud Armor that run only once the ranges exist (B5) |
| `deploy/tests/env-names.sh` and its self-test | every `LK_*` of the chart and its values is read by `crates/hub/src/config.rs`; a misspelt and a secret-looking name are refused |
| `node --test deploy/tools/*.test.mjs` | 30 of 30; seven mutations of the tool each fail a test |
| `cargo test -p hub --test hardening` | 5 of 5, with new planted cases for the CA and credentials `items`, the startup probe and the operations port; two mutations of the new checks each fail the planted test; `cargo fmt --check` and `cargo clippy -p hub --tests -D warnings` clean |
| `just ci-lint`, `just pins-test` | actionlint, zizmor (offline), the pin check and the workflow rules clean for 9 workflows (the new `deploy.yml` and the nightly `hook-ranges` job included), shellcheck on 14 scripts; 77 of 77 |
| Helm 4.3.0 against Helm 3.21.4 | the same manifests apart from blank lines, for the hub chart with all three values files, and byte for byte for the gcp chart (synthetic ranges) |

Found while building (fixed): the first Gateway rendered by kubeconform was refused by the vendored Gateway API schema
(`addresses[0]`): `crd2schema.py` had closed the branches of a `oneOf`, which kubeconform's own converter also does. The converter
now closes only objects that describe the resource, has a test for it, and only the two Gateway schemas changed.

## Decisions T3 made that a reviewer should look at

1. **Helm 4.3.0, not 3.x.** It is the newest 4.x release at least 14 days old (4.3.1 is 11 days old), and Helm 3 is in its security-only
   window. Cross-checked against Helm 3.21.4 (above), because ArgoCD may render with Helm 3.
2. **Kubernetes 1.34.0 schemas.** The GKE channel is not known; `sources.json` has one line to change it.
3. **Schemas from the projects' own CRD files** (CloudNativePG v1.30.1, Gateway API v1.6.3, Argo CD v3.5.4, Config Connector v1.159.0) at
   commits at least 14 days old, converted by `crd2schema.py`; only the three GKE policy kinds come from a third party
   (`datreeio/CRDs-catalog` at a pinned commit). `pod`, `job` and `cronjob` schemas (2 MB) are left out until a chart renders them.
4. **The hub's mirror volume is a `volumeClaimTemplate`**, as the plan says; HARDENING.md still shows the Pod with a claim, and says so.
5. **HARDENING.md and `hardening.rs` changed** (Extra-paths of the approved plan): `items` on the two Secret volumes (A9 and the
   credentials), a startup probe, and ingress on 9090 from Google's two ranges (A10); the web port is admitted from the same two ranges, since
   Google's proxies are the callers there. Each has planted cases.
6. **Optional egress, not in HARDENING.md's table:** the OpenTelemetry rule exists only when `config.otlpEndpoint` is set (A30), and the Teams host
   only when it is named, in an annotation and a pattern (Q9).
7. **Placeholder digest.** `prod.yaml` and `staging.yaml` carry 64 zeros as the image digest until the first publish (A29). It cannot be
   pulled, and the images policy warns; it is the only warning that prod and staging tolerate.
8. **Not done in T3, by the plan:** the hub binary does not serve yet (A1), so `/readyz` cannot pass; `contract.rego` (rendered output
   against HARDENING.md) is T4's; no threat-model text (T7).


# T4 notes (2026-10-25)

T4 is built and every check it owns passes. `just verify-09` still stops, for **B5 and nothing else**: the gcp chart refuses to render
until a person runs `node deploy/tools/github-hook-ranges.mjs --write`. Nothing was typed in, copied or faked to get around that:
`deploy/values/hub/github-hook-ranges.yaml` is still the unresolved marker, the gcp chart still refuses, and no test was touched
to make a recipe pass. The two findings below (B7, B8) are not blockers of T4's checks; they are disagreements between two contract
files that no check can see, and a human has to settle them before the first swimlane is deployed.

## What T4 proved (committed tree, 2026-10-25)

Every recipe of `verify-09` was run on its own (`just` stops at the first failure), twice, the second time after the last edit:

| Recipe | Result |
|---|---|
| `ops-tools-check`, `ci-lint`, `pins-test`, `images-policy-test`, `images-builder-test`, `images-publish-test` | exit 0 |
| `schemas-verify`, `deploy-lint`, `policy-test`, `contract-check`, `env-names-test`, `tools-test` | exit 0 |
| `deploy-render` | exit 1: kubeconform passes for hub (6, 13, 13 resources valid) and **agent (9 of 9 valid)**; it fails only for the gcp chart with staging and prod values, with the B5 message |
| `policy-check` | exit 1: conftest passes for hub (kind, staging, prod) and **agent (155 of 155)**; it fails only for the gcp chart, with the B5 message |
| `deploy-planted` | exit 1: **151 cases report `ok`** (kubeconform, template refusals, planted changes to the real output of the hub and agent charts, each refused for its own message, and the accepted variants of the agent chart); the one failure is the Cloud Armor group that cannot run without the ranges (`the gcp chart does not render with the committed hook ranges ...`) |

T4's own checks, from those runs:

| Check | Result |
|---|---|
| agent chart render, `helm lint --strict` | clean; `helm template` of `deploy/values/agent/sit1.yaml` gives 9 resources |
| kubeconform `-strict`, offline | Deployment, ServiceAccount, Secret, ConfigMap, Role x2, RoleBinding x2 and NetworkPolicy: 9 valid, 0 skipped. Five more kinds got planted invalid fields (`deploy/tests/planted/kubeconform.yaml`): each refused, for the field only |
| conftest, `rbac.rego` and the rest, on the agent's output | 155 of 155 (the manifest policies: hardening, secrets, topology, resources, storage, network, rbac) |
| `contract.rego` with the real `crates/agent/RBAC.md` and `HARDENING.md` extracted by `contract-extract.sh` | 25 of 25: the rendered Roles equal RBAC.md, the egress equals the contract |
| the Secret-list rejection | `rbac_test.rego::test_deny_secret_list`, `test_deny_secret_list_alone`, `test_deny_secret_watch`, `test_deny_unnamed_secret` (and 40 other cases in that file, 44 in all) refuse a planted Role; on the real output, `plant "list on Secrets"`, `"watch on Secrets"`, `"create on Secrets"`, `"an unnamed Secret grant"`, `"another Secret"` are each refused for the verb or the name |
| `conftest verify` over `deploy/policy` | 433 of 433 (237 before T4) |
| accepted variants | the join token with and without Workload Identity, two watched namespaces, other ids for the export's owner, several hub addresses, tunables, an image pull secret and another audience each pass kubeconform, the policies and the contract |
| `node --test .github/scripts/*.test.mjs` | 77 of 77; `actionlint` clean on `deploy.yml` (whose paths now include `crates/agent/RBAC.md`, `HARDENING.md` and `src/config.rs`) |
| `shellcheck -x` (0.11.0) | clean on `render.sh`, `contract-extract.sh`, `env-names.sh` |

**Mutation.** Every `deny` rule T4 wrote or changed (89: rbac 16, contract 25, network 12, hardening 31, secrets 5) was disabled one at
a time in a scratch copy, and `conftest verify` was run on each. Eight survived the first time, all in `rbac.rego` and `contract.rego`:
four had no test that needed them (a binding with no subject; a NetworkPolicy of the wrong name alone; the Secrets Role in a watched
namespace; a second container), and four shared their words with another rule that satisfied the test (the two missing contract files
and the missing Role and RoleBinding in RBAC.md). Each got a test with a fragment only that rule says, or an input only that rule
refuses, and the 41 mutants of those two files were run again: none survived.
Eleven mutations of the chart's templates (a Secret `list`, no `keep`, a writable root, no token, the config-server in every
namespace, a `secretKeyRef`, root as the user, ...) were each refused by the policy that guards the field. Five mutations of the
schema and helpers were tried: three were refused by a planted template case, one is an equivalent mutant (the schema refuses the same
digest first), and one was not caught: with the private-key check removed, the planted case (a lone key) was still refused by the
"starts with a certificate" check, although a certificate followed by a key would have rendered. That case is planted now. The
self-tests of `contract-extract.sh` and `env-names.sh` were mutated the same way (nine mutations): seven were refused, one is
equivalent (a missing file is refused by awk anyway), and one was masked: an unclosed block was refused as "no yaml block" in a file
with no other block, so the case now follows a good block and only the unclosed check can refuse it.

## B7 (T4): the agent dials the hub on 443, and the hub's agent load balancer listens on 8444

**RESOLVED on 2026-10-26, by the `09/fix:` commit that changed `deploy/charts/hub/templates/services.yaml` (find it with `git log --grep='B7' -- deploy/charts/hub/templates/services.yaml`).**
The human asked for the fix proposed below. `Service/lanekeeper-hub-agent` now has `port: 443, targetPort: agent`; the container port stays 8444
and the hub pods' NetworkPolicy ingress on 8444 stays, so `crates/agent/HARDENING.md`, `crates/hub/HARDENING.md` and every crate test are unchanged.
`network.rego` now requires exactly {443} with `targetPort: agent` (a Service on 8444, on another port, or forwarding to a number or another
name is refused: six policy tests and three planted cases in `render.sh`), and the README, `deploy/ADDING-A-SWIMLANE.md`, the agent chart's README
and the threat model (residual 9) say 443. **The sentinel:** nothing in `crates/sentinel` (a stub `main.rs`) or `deploy/sentinel` names a hub
address or port (checked with grep for `844x`, `endpoint` and `443`; the unit file says only "reaches the hub outbound over mutual TLS"), so there
was nothing to edit; whoever writes the sentinel's configuration (prompt 17) should use the load balancer on 443. **Left open:** no check compares
the agent's port with the hub's Service (`contract.rego` covers the agent only), and no cluster has run the two charts together. B8 is not part
of this decision and stays open. The text below is the finding as it was written.

Two committed contracts disagree, and no check sees it because each is right by its own file:

- `crates/agent/HARDENING.md` lists "The hub's address, 443/TCP" as the agent's one egress to the hub, and its test
  (`hardening_contract_egress_is_hub_kube_configserver_metadata_dns_only`) requires `https endpoint TCP/443`.
- `crates/hub/HARDENING.md` puts the agents on the container port `agent` 8444, and the T3 chart's `Service/lanekeeper-hub-agent`
  exposes **port 8444** (`network.rego` requires exactly `{8444}`, citing HARDENING.md, which in fact only names the container port).

The T4 chart follows the agent's contract (443), so an agent would dial `LB:443` where the load balancer listens on 8444. Neither
contract file names the load balancer's own port, so the least change that keeps both files as they are is in the T3 chart: the
Service listens on 443 and forwards to the `agent` container port (`port: 443, targetPort: agent`), with the one policy rule, its
planted case and the README changed to match. The alternative is the agent dialling 8444 (HARDENING.md and
`crates/agent/tests/hardening.rs` change, which are not 09's to change without the plan's Extra-paths). A28 also says the sentinel
"reaches the agent port (8444)", so the sentinel's port needs the same decision. **Not changed here**: it is a design choice about a
T3 deliverable and a possible contract change, so it is left for a human. Until then T4 is correct against its contract, and the two
charts do not talk to each other.

## B8 (T4): the config-server egress selects pods in the agent's own namespace

HARDENING.md's rule for `NotifyConfigServer` and `FetchServed` is `podSelector: {matchLabels: {app: csp-configuration-server}}` with no
`namespaceSelector`, which selects pods **in the policy's namespace**, the agent's own (`lanekeeper` in the examples). The config-server
runs in a namespace the agent watches (`LK_CONFIG_SERVER_DEPLOYMENT` is `namespace/name` with that namespace in `LK_NAMESPACES`,
`sit1-core` in the agent's test), and the RBAC.md examples have the agent in `lanekeeper` and the workloads in `sit1`. If the
config-server is not in the agent's namespace, the rule matches nothing and every notify and fetch is denied by default. The chart
renders the contract as written, `contract.rego` requires it, and no test can tell which namespace the real config-server is in.
The fix, if the config-server is elsewhere, is a `configServer.namespace` value and a `namespaceSelector` in the rule, with
HARDENING.md and its test changed first (the agent's owner's files). Not changed here. Related: the hub requires the Operator role
and audits the call, and the endpoint has no authentication of its own (Q37), so a rule that is too wide here is worse than one that
is too narrow.

## Decisions T4 made that a reviewer should look at

1. **`contract.rego` covers the agent only, and not the hub.** The hub's `HARDENING.md` Pod block deliberately differs from the
   chart (a claim, where the chart has a `volumeClaimTemplate`), so a comparison would need exceptions for the point of the chart.
   The hub's contract stays in `hardening.rego` and `network.rego`, as T3 wrote it. The final gate's `contract-check` is the agent's.
2. **What the contract check compares, and what it leaves to others.** Roles (as sets, with the join-token grant present exactly when
   `LK_JOIN_TOKEN_SECRET` is set, and the names in the Role equal to the names in the environment), RoleBindings, the
   NetworkPolicy (the hub's and the API server's addresses are values, so the contract's blocks stand for "an address block" and the
   metadata server's address is literal) and the pod's token, host fields, seccomp, the container's security context, ports and
   probes. **Not** the environment, volumes and mounts: the spool arrives with T8. `hardening.rego` holds the paths (the export,
   `/tmp` and the hub CA are the paths of their mounts) and `env-names.sh` the variable names until then. The ids and the
   resources are values.
3. **The hub's CA is not in the committed values.** The chart refuses to render without `hubCa.pem` (a certificate, never a key),
   and the values file carries none, because the real one is the public certificate of an agent CA that does not exist yet.
   `render.sh` passes a synthetic one (`deploy/tests/testdata/synthetic-hub-ca.pem`; a throwaway EC certificate whose key was
   deleted at once, subject `SYNTHETIC test data`). T6's swimlane files and the runbook need to say where the real one comes from.
4. **One values file, `deploy/values/agent/sit1.yaml`,** a synthetic swimlane that matches the examples of RBAC.md and HARDENING.md.
   T6 adds the per-swimlane files and may rename it; `render.sh` takes every file under `deploy/values/agent`.
5. **The names of the agent's two Secrets are fixed in the chart** (`lanekeeper-agent-cert`, `lanekeeper-agent-join-token`), not
   values: the Roles grant exactly those, and a value that differed would be an agent that cannot read its certificate.
   `secrets.rego` exempts the two variables that hold them (`LK_CERT_SECRET`, `LK_JOIN_TOKEN_SECRET` look like secrets and are
   names) and requires their values to be those names; `env-names.sh` exempts the same two and nothing else.
6. **`hardening.rego` changed for the agent's token.** The rule "every pod has `automountServiceAccountToken: false`" is now "every
   pod but `Deployment/lanekeeper-agent`, whose token must be mounted and says so": the agent is a Kubernetes client (HARDENING.md).
   The hub's cases still pass unchanged.
7. **`LK_JOIN_MODE` is always set** (`workload-identity`, `token` or `auto`, by what the values enable), so the mode is never implicit.
   It is not in HARDENING.md's list; `crates/agent/src/config.rs` reads it.
8. **`strategy: Recreate` and the spool are T8's, as planned.** The agent stops at start without its spool directory, so the chart is
   checked and not deployable yet (the chart README and NOTES say so). `hardening.rego` already allows the volume name `spool`.
9. **Kubernetes namespaces `kube-system`, `kube-public` and `kube-node-lease` cannot be watched** (the schema refuses them): the
   agent patches Deployments in every namespace it watches, so these are not its to be given.
10. **Two edits to T3's tests, both strengthening.** The planted kubeconform file used `namespace: n`, which YAML 1.1 reads as the
    boolean false, so every document there failed for that as well as for its planted field; it is `planted-ns` now, and the Gateway
    and FQDN documents lost the empty lists that were a second reason. Each of the 12 documents now fails for one reason, checked.
    And `secrets_test.rego::test_agent_cert_secret_is_allowed_empty` now plants the Secret the way the chart renders it (`Opaque`,
    with the `keep` annotation) instead of `kubernetes.io/tls`, which the API server refuses empty.

## Environment

The Docker daemon of the sandbox was not running at the start of this run (`Cannot connect to the Docker daemon`), and conftest runs
from the pinned image. It was started with `dockerd` and its default settings, nothing changed; the images were still there. No host
was refused in this run: every pin and check T4 needed (helm 4.3.0, kubeconform, conftest, the vendored schemas) was already in place.


# T5 notes (2026-10-25)

T5 is built and every check it owns passes. `just verify-09` still stops, for **B5 and nothing else**: the gcp chart refuses to render
until a person runs `node deploy/tools/github-hook-ranges.mjs --write`. Nothing was typed in, copied or faked to get around that, and
no range was swapped in even for one run (T3 did that with synthetic ones; this run did not): `deploy/values/hub/github-hook-ranges.yaml`
is still the unresolved marker, `cloudarmor.yaml` and its refusal are unchanged, `planted_gcp` is unchanged and still counts its
failure, and no test was touched to make a recipe pass. B10 needs a human but does not stop a check.

## How T5 is verified without weakening B5

The new gcp resources (buckets, service accounts, IAM) are in their own templates (`buckets.yaml`, `iam.yaml`), and `deploy/tests/render.sh`
renders the gcp chart a second time as `gcp-storage`: the same chart and values with `--set cloudArmor.enabled=false`. Every stage (lint,
kubeconform, the policies, the planted cases) runs for the gcp chart exactly as before, and fails for it exactly as before, **and also**
for `gcp-storage`. The isolation cannot stand in for the chart: `render.sh planted` fails if the isolated render has a
`ComputeSecurityPolicy`, and fails if any committed `deploy/values/gcp/*.yaml` says `enabled: false`. `just deploy-data` runs the data layer
alone (the db chart, `gcp-storage`, their planted cases and the cross-chart check); it is a subset of `deploy-check` and never the gate.

## Which commands pass and which fail only because of B5 (committed tree, each run on its own, `just` stops at the first failure)

| Recipe | Result |
|---|---|
| `ops-tools-check`, `ci-lint`, `pins-test`, `images-policy-test`, `images-builder-test`, `images-publish-test` | exit 0 |
| `schemas-verify`, `deploy-lint`, `policy-test`, `contract-check`, `env-names-test`, `tools-test` | exit 0 (`policy-test`: 630 of 630; `contract-check` includes the new cross-chart check) |
| `deploy-render` | exit 1, **B5 only**: hub 6, 13, 13 resources valid; **db 2, 4, 4 valid; `gcp-storage` 16 and 16 valid**; agent 9. The two failures are `gcp` with staging and prod, with the B5 message |
| `policy-check` | exit 1, **B5 only**: conftest passes for hub, **db (kind with 3 warnings that are its fallbacks, staging and prod clean), `gcp-storage` (staging and prod clean)** and agent; it fails for `gcp` with staging and prod, with the B5 message |
| `deploy-planted` | exit 1, **B5 only**: 288 cases report `ok` (151 before T5); the one failure is the Cloud Armor group, `the gcp chart does not render with the committed hook ranges` |
| `deploy-data` (new) | **exit 0**: lint, kubeconform and the policies on the db chart (kind, staging, prod) and `gcp-storage` (staging, prod); 137 planted cases (22 template refusals of the db chart, 17 of the gcp chart, 61 and 28 on the real output of the two, 7 cross-chart, 2 that the isolation is what it says) each refused for its own reason |

The T5 Verify of the plan, "`helm template` plus kubeconform plus conftest on the db and gcp charts; the policy tests reject a public,
non-CMEK or unlocked-in-prod bucket and a cluster without TLS", therefore splits like this: the **db chart**, the **buckets, keys and
IAM of the gcp chart** and **all policy tests** pass on their own; the **gcp chart as a whole** (Cloud Armor and buckets in one render)
fails only because of B5. What is not proven until B5 is resolved: kubeconform and conftest on one render that holds both Cloud Armor
and the buckets. No rule reads both (storage, iam and database do not look at a `ComputeSecurityPolicy`; network does not look at a
bucket), so this is a small gap, but it is a gap.

## What T5 proved

| Check | Result |
|---|---|
| `conftest verify` | 630 of 630 (433 before T5, so 197 new: `database` 119, `iam` 31 and 47 more in `storage`), with the fixtures in `fixtures_data_test.rego` |
| Mutation of the new rules | all 105 `deny` and `warn` rules of `database.rego`, `iam.rego` and `storage.rego` were disabled one at a time in a scratch copy and `conftest verify` run on each. **Seven survived** (another cluster; kind's spread and its warning; the owner's Secret named like the hub's; the scheduled backup's method; the pod's `runAsNonRoot`; the "everyone" member). Each got a test whose fragment only that rule says, and the seven were run again: **all killed** |
| kubeconform `-strict`, offline, vendored schemas | db 2, 4, 4; `gcp-storage` 16, 16 (3 `StorageBucket`, 2 `IAMServiceAccount`, 11 `IAMPolicyMember`); five new kinds got planted invalid fields (`deploy/tests/planted/kubeconform.yaml`): each refused for the field only |
| the real output, changed one field at a time | 61 cases on the db chart (TLS 1.2, a first `pg_hba` rule that lets non-TLS in, trust and md5, a parameter that is not ours, two instances, spread by host in prod, image by tag, collation C, no pgvector, a grant at bootstrap, one Secret for both roles, each flag of the app role, no backup, 7 days, a key file, no Workload Identity, no scheduled backup, each S16 field, an unencrypted class, a class that deletes the disk, every caller of the network policy, a second policy that admits everyone, a second cluster, kind with a key file) and 28 on the gcp chart's buckets and IAM (each bucket rule, the unlocked audit bucket in prod, no retention, a lock on the docs, no versioning, a key without a grant, a person, `allUsers`, owner, the audit bucket that can be overwritten, the hub in the backups, a role on the project, Workload Identity for `default`, another pool, a key granted to the hub, the fallback failing in prod) |
| templates and schemas | 22 refusals of the db chart and 17 of the gcp chart (no digest, a tag, another repository, no environment, no backups outside kind, no bucket, no service account, a lock in staging, a key in another location, no retention period, a bucket name that is not one, a password in the values, ...) |
| the cross-chart check | the backup bucket, the Google service accounts, the Workload Identity members and the disk keys agree between the db, gcp and hub charts; seven planted disagreements are each refused for their own reason |
| `shellcheck -x` (0.11.0) | clean on `render.sh` |

Found while building (fixed): the first run of the planted cases showed that a second `NetworkPolicy` that selects the database's pods
and admits everyone was accepted, because the rules looked at the policy named `lanekeeper-db` only and policies add up. `database.rego`
now refuses any other NetworkPolicy in the db chart's output. **The same gap exists in `network.rego` for the hub (T3)** and was not
changed here: a second policy that selects the hub's pods is not looked at. It is T3's file and was not asked for; a reviewer should
decide.

## B9 (T5, informational): `pkg-containers.githubusercontent.com` is refused, so the database image's contents cannot be read

**The host.** `pkg-containers.githubusercontent.com` (the CONNECT is refused by the egress proxy). `ghcr.io` itself answers: the tag list
and the manifests were read with the pinned `crane` 0.22.1. The layers and the config of an image are served from the other host.

**What it blocks.** Reading the image config (so the creation date is not read from the image) and the layers (so that pgvector, pg_trgm,
the `en_US.utf8` locale and barman are not seen in the image, and the image is not scanned with grype, and the signature that CloudNativePG
says it makes is not verified). It does not block the pin: **the digest was read from the registry, not guessed or copied**
(`crane digest ghcr.io/cloudnative-pg/postgresql:16.15-202610050823-system-trixie`).

| Item | Value |
|---|---|
| tag | `16.15-202610050823-system-trixie`; the timestamp is the publisher's (2026-10-05 08:23, 20 days before 2026-10-25, rule 15 needs 14). The newer tags, `202610120828` (13 days) and `202610190818`, `202610190958` (6), are too young |
| index digest | `sha256:c2a1806568615fbd1a675d772fc10641dbc33c10fef00a7b7ee2f2b6b8730af4`; linux/amd64 `sha256:29044d6e...` and linux/arm64 `sha256:81ce0d12...`, and two attestation manifests |
| what is known about its contents | from the build recipe, not from the image: `github.com/cloudnative-pg/postgres-containers` at `4c2710a` (2026-10-22), `Dockerfile` and `docker-bake.hcl`: `standard` = `minimal` + `locales-all` + pgaudit + pgvector + pg-failover-slots, `system` = `standard` + `barman[cloud,google,...]` 3.20.1 |

**Why the work went on.** The instruction was to record the host and stop if a refusal means a pin cannot be had. The pin could be had; the
refusal only stops the check of what is inside. If that reading is wrong, the image is one `digest` line in each of
`deploy/values/db/{kind,staging,prod}.yaml`, and the README lists what is unverified. In CI nothing is refused: the kind job (not written, see
below) will run `CREATE EXTENSION vector` against the real image.

**What the sandbox did verify of the database design** (it could pull `pgvector/pgvector:pg16`, the CI image, by its digest from compose.yaml,
and the container was removed afterwards): on PostgreSQL 16, with no `GRANT` at all, a role created like `lanekeeper_app` has `CONNECT`
and `USAGE` on `public` and not `CREATE`, the owner of the database has `CREATE` on `public` (`public` is owned by `pg_database_owner`), and
`CREATE EXTENSION vector` and `pg_trgm` succeed; `datcollate` is `en_US.utf8`. That is why `postInitApplicationSQL` creates the two extensions
and carries none of the grants of `deploy/dev/init.sql` (the plan's row says it should): a managed role does not exist yet when the
bootstrap runs, so such a grant would fail the bootstrap, and none is needed.

## B10 (T5): the plan's image has no barman, and the built-in barman it relies on is deprecated

**Decided by a human (told to the T6 run, 2026-10-25): keep the interim image.** The database stays on
`ghcr.io/cloudnative-pg/postgresql:16.15-202610050823-system-trixie` (digest `sha256:c2a18065...30af4`, the `system` variant for the barman
binaries, Debian 13) with the built-in `barmanObjectStore`, exactly as built in T5; option 1 below. Nothing in the db chart changes for this.
**Migrating to the Barman Cloud plugin (option 2) is a later open question**, not a task of this plan: the built-in barman and
`spec.backup.retentionPolicy` are deprecated in CloudNativePG (since 1.26) and "remain functional for now", and nobody knows when they go; when a
CloudNativePG release removes them, or a person decides to move first, option 2 below lists what changes. Because B9 meant the image could not be
scanned here, T6 added a nightly scan of it by digest (`just images-scan-db`, the `db-image` job of `nightly.yml`; "Decisions T6 made", 8).

**The plan.** T5 and A26: "`ghcr.io/cloudnative-pg/postgresql:16-standard-bookworm`, with pgvector", "barman to GCS through Workload
Identity; `retentionPolicy: "30d"`; a daily `ScheduledBackup`".

**What was found.**
- **The `standard` image has no barman.** `postgres-containers` README: "Both `minimal` and `standard` images are designed to work with
  backup plugins such as Barman Cloud. The `system` images, built on top of the `standard` ones, also include the Barman Cloud binaries."
  The `Dockerfile` installs barman only in the `system` stage. With the built-in `barmanObjectStore` on a `standard` image the WAL
  archiving cannot run (`barman-cloud-wal-archive` is not there), and an archive that fails keeps its WAL on the volume.
- **The `system` images are deprecated**, and so is what they exist for. README: "will be removed once in-core support for Barman Cloud
  in CloudNativePG is phased out". CloudNativePG 1.30.1 (`docs/src/backup.md`, `appendixes/backup_barmanobjectstore.md`): "native Barman
  Cloud support is deprecated" since 1.26, "remains functional for now"; `spec.backup.retentionPolicy` "is deprecated and will be removed in
  a future release". The vendored 1.30.1 CRD still has all of it, and kubeconform accepts it.
- **The plan's Debian release.** The plan says `bookworm`. T1 (B3, amendment A32) moved the images 09 builds to Debian 13 because Debian 12's
  libc6 has a critical finding that will not be fixed (CVE-2026-5450). The same recipe is published for trixie
  (`16.15-202610050823-{standard,system}-{bookworm,trixie}`). The database image could not be scanned here (B9), so whether the
  bookworm one has the same finding is not measured; the reasoning of A32 says it would.

**What was built meanwhile (interim, and the least that works):** the image `16.15-202610050823-system-trixie` (the `system` variant, for the
barman binaries; trixie, as A32 did for the others) with the built-in barman exactly as the plan describes it. It is flagged in the values
files, in `deploy/charts/db/README.md` and here. It is a substitution of the plan's literal, so a human should confirm it.

**Options for the human.**
1. **Keep the interim** (`system` image, built-in barman): works today; both are deprecated, the horizon of the removal is unknown, and
   `retentionPolicy` is a deprecated field. Nothing to change. Plan a migration.
2. **`standard` image and the Barman Cloud plugin** (CloudNativePG's supported path): `charts/db` changes (an `ObjectStore` object of the
   plugin, `plugins:` in the Cluster, `method: plugin` in the ScheduledBackup, the retention in the `ObjectStore`), the plugin has to be
   installed (a new component, with its own image and pins, and, I believe, cert-manager, which I could not check here), the schema of its
   `ObjectStore` has to be vendored (`deploy/schemas/sources.json`), and `database.rego`'s backup section changes with its tests. The gcp chart does
   not change.
3. **A custom image** under `deploy/images/postgres/` (A26's fallback): `standard` plus barman, pinned and scanned by us, patched by us.

Where each choice lands: `deploy/values/db/*.yaml` (the image), `deploy/charts/db/templates/{cluster,scheduledbackup}.yaml` (the backup
stanza), `deploy/policy/database.rego` (the `# backups` section) and `database_test.rego`.

## Decisions T5 made that a reviewer should look at

1. **The interim image and backup method** (B10).
2. **Only the extensions in `postInitApplicationSQL`, none of `init.sql`'s grants** (B9: verified on PostgreSQL 16). A policy rule refuses any other
   statement there.
3. **The owner's Secret is `lanekeeper-db-migrator`, set explicitly** (`bootstrap.initdb.secret.name`). Left to the operator it would be made in
   `lanekeeper-db-app`, the name the hub reads (T3, A7): the hub would connect as the owner of the database. A policy rule refuses the clash.
4. **Ingress only for the database's network policy**, as the plan's row says. The egress of the database pods is not restricted: it needs the API
   server's addresses per cluster and Google Cloud Storage by name, and the FQDN rules of `network.rego` are written for the hub alone. Proposed
   as a follow-up; until then the database pods can reach anything the node can.
5. **The keys are referenced, not made.** A bucket and a disk take the name of an existing Cloud KMS key (Q29 has not chosen the ring), and
   the chart grants Google's service agents their use. The signing keys of the hub (agent CA, replica CA, audit checkpoint, token KEK, CI cosign) and
   the hub's Secret Manager and KMS bindings are **not** in T5: their names are the hub's configuration (A1). Chart.yaml's old sentence "T5 adds
   buckets and keys" is corrected.
6. **A19: prod's audit bucket is locked in the committed prod values. RESOLVED on 2026-10-26: the period is three months.** The policy requires
   the lock in prod, so the values must have it to pass. This was written with an EXAMPLE period of one year; a human decided on 2026-10-26 that
   the period is **three months (90 days = 7,776,000 s)**, and `deploy/values/gcp/prod.yaml` now has `retentionPeriodSeconds: 7776000` with
   `isLocked: true` (the lock stays; no policy was weakened). The chart still has no default period and refuses to render without one. The project
   and the names in the file are still examples, so it cannot be applied as it is, and nothing is locked until it is. Staging is never locked
   (the template and the policy refuse it), and its one-week period is still an example (the decision named prod). See "Human decisions of
   2026-10-26" at the end.
7. **The buckets cannot be deleted by their Kubernetes resource** (`deletion-policy: abandon`, `Prune=false`): the backups and the checkpoints
   outlive a mistake in Git. Removing one is a person's act in Google Cloud.
8. **The data disk class has `reclaimPolicy: Retain`** (the hub's mirror class is `Delete`, since a mirror is a cache).
9. **Sizing is a placeholder** (2 CPU, 2 GiB, 20 GiB, `shared_buffers` 512 MB): the load test of prompt 16 measures. No planner parameter is
   changed (D124), and a policy refuses any parameter outside a short list.
10. **The hub chart's `storageclass.yaml` comment** now says where the grant is (`charts/gcp` `cmek.diskKeys`), and `render.sh` checks that it exists.

## Not done in T5

- **The kind job** (`deploy/tests/kind/run.sh`, the job `kind-e2e` of `images.yml`, `just kind-e2e`). The plan's traceability gives acceptance 3 to T3 and T5, T3's
  notes say "the kind job is T5", but T5's row lists no file for it, nothing in this sandbox can run it (the CloudNativePG operator image is on
  `ghcr.io`, whose layers are B9, and the disk has 2.4 GB free), and it cannot pass until the hub serves (A1). A script
  that was never run would be a guess, so it is left for a task of its own; `deploy/values/{hub,db}/kind.yaml` exist for it. **[Review round 1 wrote it anyway, as the reviewer required (blocking item 4): `deploy/tests/kind/run.sh`. It has still never run past the creation of a cluster (B16 below).]**
- **Renovate does not see the database image** (`deploy/values/db/*.yaml` holds the tag and digest in a shape its managers do not match). It is
  updated by hand, at least 14 days after publication (rule 15). A custom manager could not be tested here.
- **Egress of the database pods** (decision 4) and **monitoring on 9187**.
- **The signing keys and the hub's own bindings** (decision 5).
- **The hub's `network.rego` gap** (a second NetworkPolicy), above.
- **B7 and B8 were not touched**: no contract file was changed and no T3 or T4 chart was changed to settle them.

## Hosts probed in this run

| Host | Result | Used for |
|---|---|---|
| `ghcr.io` | reachable (tags, manifests) | the database image's tag list and digest |
| `pkg-containers.githubusercontent.com` | **refused** | the image's layers and config: **B9** |
| `raw.githubusercontent.com` | reachable | CloudNativePG's source and documentation at the commit of the schemas (`2a35abb4`) |
| `github.com` (git over HTTPS) | reachable | `cloudnative-pg/postgres-containers`, shallow |
| `docker.io` (registry-1) | reachable | `pgvector/pgvector:pg16` by its digest, to check the PostgreSQL 16 defaults (removed afterwards) |


# T6 notes (2026-10-25)

T6 is built and every check it owns passes. `just verify-09` still stops, for **B5 and nothing else**: the gcp chart refuses to render until a
person runs `node deploy/tools/github-hook-ranges.mjs --write`. Nothing was typed in, copied or faked to get around that, and no range was swapped
in, even for one run: `deploy/values/hub/github-hook-ranges.yaml` is still the unresolved marker, `cloudarmor.yaml` and its refusal are unchanged,
`planted_gcp` is unchanged and still counts its failure, and no test was touched to make a recipe pass. A human decided that B5 stays open, and it does.

## How T6 is verified without weakening B5

- **Admission** (`charts/gcp/templates/binauthz.yaml`) is in the gcp chart, as the plan says, and `render.sh` checks it through the render T5 made for this
  (`gcp-storage`: the same chart and values with `--set cloudArmor.enabled=false`). Every stage runs for the gcp chart exactly as before and fails for it
  exactly as before, **and also** for `gcp-storage`, which now holds the admission resources too. The T5 safeguards stand: `render.sh planted` fails if
  the isolated render has a `ComputeSecurityPolicy`, and fails if a committed gcp values file says `enabled: false`; there is no `enabled` switch for
  admission, so none could be added to dodge the key (below). `just deploy-data` runs the data layer plus admission; it is a subset of `deploy-check`.
- **The Argo CD files** do not depend on B5 except in one place: the scope of the gcp chart *with* Cloud Armor against its project. `render.sh argocd`
  (`just deploy-argocd`, a subset) leaves that one out and says so; `render.sh policy` (so `policy-check` and `verify-09`) runs it, and it fails with the
  B5 message for the gcp chart, as every other check of that render does.
- **The public key of the CI signing key** is the second thing that is *not typed in*, for the same reason as the ranges: it does not exist until a
  person makes the key in Cloud KMS. The committed gcp values carry no key, and the chart refuses to render without one (`binaryAuthorization.publicKeys
  is empty`). `render.sh` stands in a throwaway public key (`deploy/tests/testdata/synthetic-attestor-key.yaml`, labelled SYNTHETIC, its private half
  never written to disk), **only while the environment's values have none**: with a key in the values, the output has that key and not the synthetic
  one (a planted case shows it). So the committed `values/gcp/{prod,staging}.yaml` do not render by themselves with `helm template`: they fail for the
  ranges (B5) and for the key, each with its own message, and a person fixes each by running one command. That is a second "unresolved marker" in the same chart.

## B11 (T6, informational): nothing creates a swimlane in the hub

Step 1 of the runbook (`deploy/ADDING-A-SWIMLANE.md`, "Create the swimlane") has two halves. The infrastructure half is the platform team's. The half in
the hub cannot be written as steps, because there is nothing to follow:

- A swimlane is a row of `swimlanes` (`db/migrations/0001_init.sql`: id, project, display name, cluster, tier, NFS server, export and mount root).
- `api/openapi.yaml` has `GET /api/v1/swimlanes` and `GET /api/v1/swimlanes/{id}`, `PUT /api/v1/admin/agents/{swimlane}` (the agent's service account,
  for a swimlane that exists), and the tenant set and auto-notify of a swimlane. **It has no operation that creates a swimlane.**
- In the code, `crates/hub-platform/src/join/google.rs` reads `swimlanes.agent_service_account` when an agent joins; nothing writes it, and
  `INSERT INTO swimlanes` appears only in tests.

The hub does not serve yet (plan 09 A1), so steps 1, 2 (the registration) and 6 of the runbook describe the contract and not a running system; this
one is a hole in the contract itself. The runbook says so in step 1. Who fills the swimlane registry (an endpoint, a seed, a migration) is for a
human to assign, and it is not 09's: it is application code, which this prompt does not own.

## B12 (T6, informational): what Binary Authorization needs that no chart makes, and what could not be confirmed

The plan's A5 says "to my knowledge" and "(to confirm)", and Google's documentation (`docs.cloud.google.com`, B6) is still refused, so these were taken
from the Config Connector CRDs and samples at the vendored commit (v1.159.0) and from the code, and are listed here so that the first apply is read as
their test. The policy is in `DRYRUN_AUDIT_LOG_ONLY` (A5, Q24) precisely so that finding out blocks nothing.

1. **Does a classic attestor accept a cosign signature?** The chart makes the classic pieces the plan names (a Container Analysis note, an attestor with the
   CI KMS key's public key, the project's admission policy). `images.yml` signs the image with that key by cosign, which writes a signature to the
   registry. To my knowledge a classic attestor reads *attestations* in Container Analysis, not signatures in the registry, so the CI job may also need a
   step that writes an attestation (`gcloud container binauthz attestations ...`). Not confirmed, not changed (`images.yml` is T2's). The same CRD set has
   `BinaryAuthorizationPlatformPolicy` (v1alpha1) with `sigstoreSignatureCheck`, which takes a cosign public key directly; its spec has no
   `enforcementMode` field, so it is not a drop-in for the plan's dry-run default, and it is what to ask about if the answer to the question above is no.
2. **The trust root is not made by any chart.** Plan A4 lists the CI cosign key, the Workload Identity Federation pool and provider for GitHub, and the IAM
   that lets only this repository's `main` and tags sign with the key, among the resources of the gcp chart. T3, T5 and T6 do not make them (T5's notes say
   so for the keys; T6's row lists only the attestor and the policy). `images.yml` reads their names from repository variables
   (`LK_COSIGN_KMS_KEY`, `LK_GCP_WIF_PROVIDER`, `LK_GCP_SERVICE_ACCOUNT`). The plan's own risk section calls this "the trust root of signing and admission";
   a human should assign it, and the chart's README ("Admission") lists it as not done.
3. **The policy is per Google project.** The gcp chart's policy protects the platform project's clusters. A swimlane cluster in another project has no policy
   until one is made there that requires the same attestor. Nothing in this repository makes it (the gcp chart's checks forbid the switch that would let
   it be rendered without Cloud Armor, on purpose), and Q24 still asks whether Binary Authorization is available on the swimlane clusters.
4. **Before enforcing**, the third-party images in a cluster (the CloudNativePG operator and instances, Argo CD, Config Connector) would be blocked: they need
   an attestation of ours or an entry in `binaryAuthorization.admissionWhitelistPatterns` (the policy refuses a pattern that covers an image of ours). The
   dry run's audit log lists them.
5. **Smaller:** the key `id` is written as `//cloudkms.googleapis.com/v1/<key version>`; that deleting the policy resets the project to a policy that admits every
   image (the reason for `abandon` and `Prune=false`) is from memory; the project's two Google APIs must be enabled; and whether Binary
   Authorization's service agent needs an IAM binding on the note, which the chart does not make, is unconfirmed.

## Decisions T6 made that a reviewer should look at

1. **Admission is the classic project policy and attestor of plan A5, in dry run.** `enforcementMode` is a value, `DRYRUN_AUDIT_LOG_ONLY` in the chart's
   defaults and written out in `values/gcp/{prod,staging}.yaml`, until Q24 is answered. `binauthz.rego` *warns* about it on every run and the warning is
   allowed to render in every environment, the way the placeholder image digest is (`render.sh` runs the `images` and `binauthz` policies apart, without
   `--fail-on-warn`). A warning that failed prod and staging would make the committed values fail until Q24 is answered, which is not what A5 says.
2. **The public key is not in the committed values** (the section above). The alternative, an EXAMPLE key in `values/gcp/*.yaml`, would have rendered
   without a stand-in and put key material that looks real into a prod file.
3. **No `values/gcp/kind.yaml`.** The plan's row says `values/{hub,db,gcp}/{prod,staging,kind}.yaml`; kind has no Google Cloud, no Config Connector, and
   the gcp chart's checks forbid the only switch (`enabled: false`) that could make a kind render of it meaningful. hub and db have all three.
4. **The Helm release name is the Application's name** (`lanekeeper-hub`, `-db`, `-gcp`, `-agent`), required by `argocd.rego`, and `render.sh` renders
   each chart under that name (it was `lanekeeper` for all). Only labels carry it, and Argo CD's documentation warns that a release name that differs
   from the Application name can break selectors that use `app.kubernetes.io/instance`. Nothing else changed in any render.
5. **Three Argo CD projects with approved lists** (`argocd.rego`), not the `default` project, which allows anything: `lanekeeper-workloads` (hub, db),
   `lanekeeper-cloud` (gcp) and `lanekeeper-agent` (one per swimlane, in the swimlane's file, destinations compared with its values file). Each allows
   the Namespace that `CreateNamespace=true` makes, by name: read in Argo CD's source at the vendored commit (gitops-engine `sync_context.go`: the
   auto-created namespace task passes the permission validator), so without it the first sync would stop. `argocd_scope.rego` renders every chart and
   shows its project allows everything the chart makes. **The agent project does not allow a PersistentVolumeClaim yet**: T8's spool adds one, and
   T8 must add the kind to the approved list, with its test (`test_deny_a_persistent_volume_claim_until_the_project_knows_it` marks the place).
6. **`targetRevision` is a placeholder (40 zeros) with a warning**, like the image digests (A29): a commit cannot name the commit that contains it.
   `deploy/argocd/README.md`, "Moving a revision", is the procedure; Argo CD cannot resolve the placeholder, so nothing syncs until it is replaced.
7. **No staging Applications.** The plan lists one file per component and nothing says which cluster staging runs on. The policy accepts the staging
   pair (`staging` label with `staging.yaml`) and a planted case shows it.
8. **The nightly `db-image` job, and `scan.sh --image`, `scan-db.sh` and `scan-db-test.sh`, are an addition that a human asked for inside this
   task's commit** (B9 meant the database image could not be scanned here). `scan.sh --image REF` takes `name@sha256:<64 hex>` only (a tag, a short digest,
   an upper-case host and anything with a space or a leading dash are refused before a container runs); `scan-db.sh` reads the repository from the db chart
   and the digest from every `values/db/*.yaml` and fails closed on a missing, placeholder or malformed digest or no file at all; the test runs in
   `images-policy-test` and needs no Docker. Nothing else of the nightly workflow changed.
9. **Prune is off for the database and the Google Cloud resources; no Application has `resources-finalizer`.** Deleting an Application leaves what runs.
10. **A flaky gate was fixed: `deploy/images/tests/builder-test.sh` (T2's) failed in 2 of the 4 runs made while other checks were running** in this
    run's `images-builder-test` ("the builder does not report BuildKit v0.34.0") although `builder.sh` had just checked the version. Root cause, measured: the test ran
    `docker buildx inspect NAME | grep -q ...` under `set -o pipefail`; `grep -q` exits at its match, `docker` gets SIGPIPE while it is still
    writing, and the pipeline fails with the line in the output. Over 300 runs of that pipeline on an idle machine 18 failed (6%); with the output
    captured first, 0 of 100 did. The test now captures the output and then greps it (12 runs of `just images-builder-test` in a row pass). It checks the same
    thing as before. The other `| grep -q` pipelines under `deploy/` and the Justfile read the one line of a version command, which is written
    in one piece, and were not changed.
11. **`scan.sh --image` was proved end to end on a real image**, since the database image's layers are refused here (B9): on the test registry image of
    `tools.env` (`docker.io/library/registry@sha256:6c5666b8...`, run only by the publish test and never deployed), grype pulled it by digest and failed on its
    critical findings (CVE-2026-33186 in `google.golang.org/grpc`, CVE-2026-63073 in `libcrypto3` and `libssl3`) with exit 2, using the same
    `--fail-on critical` function as the images we build. Renovate proposes a newer digest of that test image; it is not a deployed one. The scan of the database image itself
    could not run here, and its first run is the nightly job.
12. **`crates/*` is untouched**, and no contract path changed. Not changed either: B7 and B8, the hub chart's `network.rego` gap (a second NetworkPolicy, T5's
    note), `images.yml`.

## Not done in T6

- **The CI signing key, the Workload Identity Federation pool and the IAM around them** (B12, 2), **Binary Authorization for the swimlane projects** (B12, 3).
- **The hub-side swimlane registry** (B11).
- **The kind job** (still T5's omission: nothing in this sandbox can run it, and the hub does not serve, A1). **Written in review round 1; see "Review round 1" at the end.**
- **Renovate does not move `targetRevision`**; a person does, by the procedure. **No `CODEOWNERS` entry** for `deploy/argocd/**` or `deploy/policy/**`: a human
  may want the platform team to review them.
- **Pod Security labels on the namespaces** (`deploy/argocd/README.md`, "Not verified here").

## Which commands pass and which fail only because of B5 (committed tree, each recipe run on its own, `just` stops at the first failure)

| Recipe | Result |
|---|---|
| `ops-tools-check`, `ci-lint`, `pins-test`, `images-policy-test`, `images-publish-test`, `schemas-verify`, `deploy-lint`, `policy-test`, `contract-check`, `env-names-test`, `tools-test` | exit 0 (`pins-test` 77 of 77; `schemas-verify` 37 files equal the lock; `deploy-lint` 11 charts and values linted, 0 failed; `policy-test` **851 of 851**, 630 before T6; `tools-test` 30 of 30; `images-policy-test` includes the new `scan-db-test.sh`) |
| `images-builder-test` | exit 1 in the first full run (the flake of "Decisions", 10), exit 0 in 12 runs in a row after the fix |
| `deploy-render` | exit 1, **B5 only**: hub 6, 13, 13 resources valid; db 2, 4, 4; **`gcp-storage` 19 and 19 (it was 16: the note, the attestor and the policy)**; agent 9; **the Argo CD files 7 of 7**. The two failures are `gcp` with staging and prod, with the B5 message |
| `policy-check` | exit 1, **B5 only**: conftest passes for hub, db, `gcp-storage` (with Binary Authorization's dry-run warning printed), agent, the Argo CD policy on the platform's files and on the swimlane's, the paths, and the scope of hub, db, `gcp-storage` and agent against their projects. It fails for `gcp` with staging and prod, and for the scope of `gcp` with Cloud Armor, each with the B5 message |
| `deploy-planted` | exit 1, **B5 only**: **388 cases report `ok`** (288 before T6); the one failure is the Cloud Armor group, `the gcp chart does not render with the committed hook ranges` |
| `deploy-data` | **exit 0** (178 planted cases `ok`; 137 before T6): the db chart and `gcp-storage`, which now includes admission |
| `deploy-argocd` (new) | **exit 0**: kubeconform, the argocd policy on both groups, the paths, the scope of each project against its charts, and 59 planted cases |
| `just verify-09` | stops at `deploy-render`, for B5 |

The T6 Verify of the plan, "`helm template` with every values file, then conftest for hardening and network rules", therefore passes for every chart and
values file but one: the **gcp chart with Cloud Armor** (staging and prod), which fails only because of B5. What is not proven until B5 is resolved:
kubeconform, conftest and the project scope on one render that holds Cloud Armor *and* the admission resources and the buckets. As in T5 no rule reads
both, so this is a small gap, but it is a gap.

## What T6 proved

| Check | Result |
|---|---|
| `conftest verify` | 851 of 851: `binauthz_test.rego` 62 tests, `argocd_test.rego` 140, `argocd_scope_test.rego` 19, each rule with a test whose fragment only that rule says |
| Mutation of the new rules | all 72 `deny` and `warn` rules of `binauthz.rego` (23), `argocd.rego` (46) and `argocd_scope.rego` (3) were disabled one at a time in a scratch copy and `conftest verify` run on each. **Six survived the first run** (the end of a PEM, a project missing from the input, `ignoreDifferences` on a non-agent Application, a namespace pattern in a destination, the agent project's swimlane label, its own namespace): in each, another rule's message contained the fragment the test looked for. Each test now looks for a fragment that only its rule says, and the 72 were run again: **72 killed, 0 survived** |
| Mutation of the shell checks | `scan-db.sh` and `scan.sh --image`: 12 mutants (each check removed, the dedupe, the key it reads, the pattern loosened to a tag or to upper case), all killed by `scan-db-test.sh`. `render.sh`'s new checks, 9 mutants (each check of `argocd_paths`, the binauthz policy not run, the synthetic key always layered, and the calls that run the argocd policy and the project scope): **three survived the first run**, the deleted calls (the planted cases test the policies, not that the real run calls them). `argocd_policy_all` now counts its runs against the files in the directory (a tripwire), and the three are killed, each with the tripwire's message |
| Fixture bug found by a planted case | `object.union` in a fixture helper merged a patched file into the original, so a `remove` in six tests removed nothing; the helper replaces the file now, and the six tests ran red before they ran green |
| kubeconform `-strict`, offline | 5 new kinds got planted invalid fields (`BinaryAuthorizationPolicy`, `BinaryAuthorizationAttestor`, `ContainerAnalysisNote`, `Application`, `AppProject`), each refused for the field only; `ContainerAnalysisNote` is a new vendored schema, from the CRD at the pinned Config Connector commit (`deploy/schemas/fetch.py`, the lock holds its hash) |
| the real files, changed one thing at a time | 25 admission cases on the gcp chart's output and 14 refusals of its templates (a private key inside the public key, a certificate, a weak algorithm, a key version that is not one, no key, ...), and 59 on the Argo CD files (a branch, a tag, a short commit, another repository, inline values, `default` as the project, another namespace, no self-heal, prune on the database, `Replace`, a finalizer, the certificate Secret not ignored or ignored too much, a project with every namespace, Secrets or a ClusterRole, project tokens, a namespace the project does not allow, ...), each refused for its own reason |
| the paths between the files | an Application whose values file is gone, a swimlane's values without an Application and the other way round, the hook ranges file gone, a chart path that cannot be read, a gone Application file: each refused; and a Secret in the hub's output and a ClusterRole in the agent's are refused by their projects |
| a real key is never replaced | with a public key in the environment's values, the render has that key and not the synthetic one (a planted case) |
| `ADDING-A-SWIMLANE.md` | not run: the hub does not serve (A1). Its commands that need nothing of the hub were written from the chart and the contracts and not executed |

## Hosts probed in this run

No new host was refused, and nothing was pinned from a guess.

| Host | Result | Used for |
|---|---|---|
| `github.com` (git over HTTPS) | reachable | the Config Connector samples and CRD list at the vendored commit `53e89210`, and Argo CD's source and documentation at `d6d5b248` (v3.5.4), by shallow sparse fetches, read as data and never run |
| `raw.githubusercontent.com` | reachable | the `ContainerAnalysisNote` CRD, by `deploy/schemas/fetch.py` at the vendored commit; the lock holds its hash |
| `api.github.com` | **403 for another repository's contents** (`repos/GoogleCloudPlatform/k8s-config-connector/contents/...`) | nothing: the same rule as B5 (the sandbox is bound to its own repository), so git over HTTPS was used instead; no pin depended on it |
| `docker.io` (registry-1) | reachable | the grype image, and `docker.io/library/registry@sha256:6c5666b8...` by digest, for the end-to-end run of `scan.sh --image` |
| `grype.anchore.io` | reachable | grype's vulnerability database (downloaded once, then cached in `target/grype-db`) |
| `ghcr.io` and `pkg-containers.githubusercontent.com` | not probed again | the database image's layers are still B9; the nightly job is the first scan of it |
| `docs.cloud.google.com`, `registry.k8s.io` | not probed again | still B6 |


# T8 notes (2026-10-26)

T8 is built and every check it owns passes. `just verify-09` still stops, for **B5 and nothing else**: the gcp chart refuses to render until a
person runs `node deploy/tools/github-hook-ranges.mjs --write`. Nothing was typed in, copied or faked to get around that, and no range was swapped in,
even for one run: `deploy/values/hub/github-hook-ranges.yaml` is still the unresolved marker, `cloudarmor.yaml` and its refusal are unchanged,
`planted_gcp` is unchanged and still counts its failure, and no test was touched to make a recipe pass. T8 added no `enabled` switch for anything.

## Which commands of `verify-09` pass, and which fail only because of B5 (committed tree, each recipe run on its own, `just` stops at the first failure)

| Recipe | Result |
|---|---|
| `ops-tools-check`, `ci-lint`, `pins-test`, `images-policy-test`, `images-builder-test`, `images-publish-test`, `schemas-verify`, `deploy-lint`, `contract-check`, `env-names-test`, `tools-test` | exit 0 (`ci-lint`: actionlint, the pin check over 112 references, the workflow rules over 10 workflows, zizmor offline with no finding, shellcheck on 26 scripts; `pins-test` 78 of 78; `images-policy-test` includes `cargo test -p hub --test hardening` and `-p agent --test hardening`, 5 of 5 each; `contract-check` 34 of 34 on the real RBAC.md and HARDENING.md) |
| `policy-test` | exit 0: **1,002 of 1,002** (851 before T8; 905 after the agent's spool, 1,002 with the sentinel's two policies) |
| `sentinel-check` (new, now part of `verify-09`) | exit 0: 20 unit-parser tests, the S25 policy (72) and the Dockerfile.build policy (25) tests, the policies on the real unit (23) and the real `Dockerfile.build` (14), the 13 planted Dockerfile rules through the real parser, and a unit with a capability refused through the real parser |
| `deploy-render` | exit 1, **B5 only**: hub 6, 13, 13 resources valid; db 2, 4, 4; `gcp-storage` 19 and 19; **agent 11 of 11 (it was 9: the StorageClass and the claim)**; the Argo CD files 7 of 7. The two failures are `gcp` with staging and prod, with the B5 message |
| `policy-check` | exit 1, **B5 only**: conftest passes for hub, db, `gcp-storage` and **agent (270 of 270)**, the Argo CD policy on both clusters' files, and the scope of hub, db, `gcp-storage` and **agent** against their projects; it fails for `gcp` with staging and prod, and for the scope of `gcp` with Cloud Armor, each with the B5 message |
| `deploy-planted` | exit 1, **B5 only**: 417 cases report `ok` (388 before T8); the one failure is the Cloud Armor group, `the gcp chart does not render with the committed hook ranges` |
| `just verify-09` | stops at `deploy-render`, for B5 |

The T8 Verify of the plan is `just sentinel-test`, which is not part of `verify-09` (it needs Docker and a privileged container, as `images-repro` does):
**exit 0 in 3 min 13 s**, from the committed files (`git archive HEAD`), with the audit stand-in of B13 below. Its checks, in order: the signing script (every case), a
throwaway key signs the RPM, the CycloneDX SBOM (2 components) is written and scanned (grype v0.120.1: "No vulnerabilities found"), then on **Rocky Linux 8.9 (systemd
239-78.el8) and 8.10 (239-82.el8)**: `rpm -K` refuses the package until the key is imported, passes after, and refuses a changed byte; dnf refuses the unsigned and the
changed package; dnf refuses the package until `audit` is there; the files, owners and modes; `rpm -V` clean; `systemd-analyze verify` silent; `systemctl start`; the probe;
removal and reinstall. Then the exposure score: **1.7** (limit 2.0), scored by systemd 252 (252-67.el9_8.6.alma.1), and the self-test (10 worse units refused). Then the
cosign script. 124 `ok` lines, no failure.

## A15: the exposure score, measured, and no escalation was needed

`systemd-analyze security --offline=true --threshold=20` on the unit packaged in the RPM, by the pinned AlmaLinux 9.8 image: **1.7 OK**. Counted against it (11 items): `PrivateNetwork` 0.5, the two
address-family items 0.3 and 0.1, `ProtectClock`, `ProtectKernelLogs`, `ProtectProc`, `PrivateUsers` and `IPAddressDeny` 0.2 each, `ProtectHostname`, `ProcSubset` and
`RootDirectory` 0.1 each (the table of the analyzer is in the README of deploy/sentinel). An early draft of the unit that also set `ProtectClock`, `ProtectHostname`,
`ProtectKernelLogs`, `ProtectProc` and `ProcSubset` scored 1.2 against 1.9 without them; **the unit does not set them**, and a mutant that does is refused by
`exposure.sh` for exactly that reason ("Unknown lvalue" on 8.9 and 8.10) although its score is better. What each directive of the unit is worth was measured by
removing it (3.7 without `CapabilityBoundingSet=`, 3.3 without `SystemCallFilter`, 2.4 without `RestrictNamespaces`, 2.1 without `User=` or without
`RestrictAddressFamilies`, and 1.7 to 1.9 for the rest, so the limit of 2.0 is 0.3 above the unit).

Two things found while doing it. (1) `systemd-analyze verify` on 239 prints `Unknown lvalue 'ProtectClock' in section 'Service'` and **exits 0**, so a check on its exit status
would accept a unit that Rocky 8 half ignores: `install.sh` and `exposure.sh` check that its output is empty. (2) **Rocky's 239-78.el8 and 239-82.el8 are not upstream 239**:
they read `RestrictSUIDSGID` (upstream 242), `PrivateMounts` and the rest of the 239 list, and not `ProtectClock`, `ProtectHostname`, `ProtectKernelLogs`, `ProtectProc`
or `ProcSubset` (each tested on both). The unit sets `RestrictSUIDSGID=yes` (0.2 of score, and a real restriction) on the strength of that measurement; on an EL8 whose systemd is
older it would be ignored with a line in the journal.

## B13 (T8, informational): the RPM release key does not exist, and three hosts were refused

**The key.** Plan A16: the RPM release key is a GPG key in Secret Manager, read through Workload Identity Federation by the protected `release` job, and "no stored keys".
Nothing makes the key, the secret, the pool and provider that admit only this repository's `sentinel-v*` tags in the `release` environment, the service account, the
environment or the three repository variables (`LK_RPM_WIF_PROVIDER`, `LK_RPM_SERVICE_ACCOUNT`, `LK_RPM_SIGNING_SECRET`, now known to actionlint). It is the same
situation as B12 (2) for the CI signing key, and it is handled like B5 and B1: `deploy/sentinel/RPM-GPG-KEY-lanekeeper.pub` is committed as an **unresolved marker** (the
plan lists the file; a key made here would be a trust root with a private half that nobody holds), `sign.sh release` refuses a marker ("is not a public key") and a secret
key whose fingerprint is not the committed one, and a release tag stops at `sign-release`. One command makes the key (`sign.sh new-release-key --out ... | gcloud secrets
versions add ... --data-file=-`: the secret key goes to standard output once). Pull requests are signed with a throwaway key made inside a container for the run, and
verified with it, so every verification step is tested. The invariant "the committed file is the marker or a public key, never a private key" is a test.

**The hosts** (probed from this sandbox on 2026-10-26; the proxy answers the CONNECT with 403):

| Host | Result | Used for |
|---|---|---|
| `dl.rockylinux.org`, `mirrors.rockylinux.org`, `download.rockylinux.org`, `rockylinux.org` | **refused** (the proxy's status lists these four as `connect_rejected`) | Rocky Linux 8 repositories: the real `audit` package for the install test. **That piece stopped**: the test ran with `deploy/sentinel/tests/audit-stand-in.spec`, a package that only provides `audit` and the directories it owns (labelled in its own name and release, installed only with `--audit stand-in`, and announced in capitals by the test). CI uses `--audit repo` (`dnf -y install audit`), which has never run |
| `deb.debian.org`, `security.debian.org` | **refused** (`connect_rejected` in the proxy's status) | Debian packages: `rpm` for the builder and `musl-tools` for a musl C compiler. The RPM is built with Ubuntu 24.04's rpm instead (see decisions); a musl C toolchain is not in `Dockerfile.build`, so the static build works for a crate with no C code (the stub) and a dependency with C code (prompt 17's mutual TLS) needs one added and tested |
| `repo.almalinux.org`, `mirrors.almalinux.org`, `vault.almalinux.org`, `vault.centos.org`, `mirror.stream.centos.org`, `dl.fedoraproject.org`, `mirrors.fedoraproject.org`, `cdn-ubi.redhat.com`, `registry.access.redhat.com`, `quay.io`, `snapshot.debian.org`, `ftp.debian.org`, `mirrors.kernel.org`, `repo.rockylinux.org`, `sources.debian.org`, `packages.debian.org`, `cdn-fastly.deb.debian.org` | no answer to curl (code 000) | looked at as other sources for rpmbuild and for `audit`; none was needed once Ubuntu's archive worked |
| `archive.ubuntu.com` | reachable | `rpm` 4.18.2+dfsg-2.1build2 and `gpg`, checked by apt against the archive's signatures |
| `static.rust-lang.org` | reachable | the musl standard library of Rust 1.88.0 (hashed here) |
| `registry-1.docker.io` | reachable; **429 once** | Docker Hub's anonymous limit was hit again by the rust base image after the builder cache was pruned. The same digest was pulled from `mirror.gcr.io` (`docker pull mirror.gcr.io/library/rust@sha256:f2a17efb...`) and tagged locally; no daemon setting was changed |
| `mirror.gcr.io`, `ghcr.io`, `gcr.io`, `index.crates.io`, `static.crates.io`, `grype.anchore.io`, `pypi.org`, `proxy.golang.org`, `github.com` (git) | reachable | pins read from the registries (`crane digest`), the crates index, the vulnerability database |

Every pin was read, not guessed: the Rocky tags `8.9` and `8.10` and the Ubuntu tag `noble-20260930.1` with `crane digest` on docker.io **and** mirror.gcr.io (same
digests), the AlmaLinux tag `9.8-20261002` (which is also `9.8` and `latest` today), the musl standard library's sha256 from a download hashed here and compared with the
`.sha256` file Rust publishes afterwards.

## What T8 proved

| Check | Result |
|---|---|
| The agent's spool, test first | 54 new Rego tests (905), then 97 for the sentinel (1,002). Every new rule was disabled one at a time in a scratch copy and `conftest verify` run on each: hardening 4, storage 7, topology 1, contract 9, systemd 22, dockerfile_artifact 14 (+ the one rule whose message is a literal, covered by its test) and three edits of the approved lists. **One survived the first run** (the pin rule for the final stage of `Dockerfile.build`: another rule refused the same input); it got a test and was killed. Three existing tests used list positions that the new env, mount and volume moved (`volumes/2`, `volumeMounts/2`, `env/12`); they now use the new positions, and one of them (`test_agent_deny_the_hub_ca_as_a_secret`) would have kept passing for the wrong reason |
| The agent chart on the real output | kubeconform 11 of 11; policies 270 of 270; contract 34 of 34; 12 planted changes to the real output (the claim not larger than the bound, a bound the claim cannot hold, a bound that is not a number, the default class, read-write-many, prunable, deleted with the release, a rolling update, the spool as an emptyDir, a wrong path, read-only, inside the export) each refused for its own message, 5 more against the contract (another claim, the mount path, the directory, a rolling update, no claim rendered); 3 variants accepted (a larger spool and a raised bound, faster disks, no key: the fallback) and 9 template refusals (the bound or the path as a tunable, `1Ti`, `big`, a bound under 4 MiB or over 64 GiB, a key that is not one, a disk type, an unknown value) |
| The install checks on a worse unit | the unit with a directive removed or added, built, signed and installed on 8.9: `ProtectClock` added (the 239 check), no `CapabilityBoundingSet` (`CapBnd` not empty), no `PrivateTmp`, no `PrivateDevices`, no `ProtectHome`, no `ProtectSystem`, no `SystemCallFilter`, no `NoNewPrivileges`. **Two were not caught by the process probe the first time** (without `ProtectSystem` the user cannot write `/etc` anyway; without `SystemCallFilter` the other directives still install a seccomp filter), and got probes: a directory the user owns outside the spool, and `nice(2)`, which is in `@resources`. `NoNewPrivileges` is caught by `systemctl show` and not by the process (a seccomp filter implies it), which is stated here and not hidden |
| The signing script on a worse script | fingerprint comparison removed, the unsigned check removed, the secret-key count removed: each killed. One mutant survived (the grep after signing in `sign_and_check`): the verify subcommand, run next, is a second barrier. It is an equivalent mutant in practice, not a proof |
| Reproducibility | `reproducible.sh` (two `--no-cache` builds): the same binary (`56cf2581...`) and the same RPM; a third build earlier gave the same RPM again |
| What the VM's systemd reads | `PrivateMounts=yes` and `RestrictSUIDSGID=yes` are read by Rocky's 239; the user the package makes is a system account (uid 998 in the container) with `/sbin/nologin` and the home `/var/lib/lanekeeper-sentinel` |

## Decisions T8 made that a reviewer should look at

1. **The RPM is built with Ubuntu 24.04's rpm, pinned by digest, in a Dockerfile.** Rocky's images have no `rpmbuild`, their repositories are refused here, and Debian's are
   too. The spec fixes the payload (gzip), the file digests (SHA-256), the build host and the file times so that the rpm 4.14 of Rocky 8 reads it and two builds agree.
   Only `rpm` is pinned to a version (the release pocket's, which does not move); `gpg` takes the archive's security updates. If a reviewer prefers every package
   pinned, the price is a build that breaks when the archive moves on.
2. **A15 is read as "the unit sets only directives that the VM's systemd reads", and checked by that systemd**, not as "directives from a list of 239's": a list would go stale
   and would have missed Rocky's backports. The score is of that unit, so what 239 cannot do is counted against it.
3. **`RestrictSUIDSGID=yes` is set** although upstream 239 does not have it (above).
4. **The socket's mode is `0666` inside a `0750 root:lanekeeper-sentinel` directory** (A17), because the plugin runs as root and cannot set a group, and a unix socket is
   connected to with write permission on the file. Prompt 17 confirms it against the daemon. A reviewer who dislikes a world-writable socket mode should look at the directory.
5. **The unit has an `[Install]` section (`WantedBy=multi-user.target`)** so that configuration management can `enable` it; the policy allows that and nothing else there. The
   package neither enables nor starts it.
6. **`ReadWritePaths=` and `StateDirectory=` both name the spool**, so the one writable path is stated in a line that the policy reads, and systemd makes the directory.
7. **A separate policy for `Dockerfile.build`** (`dockerfile_artifact.rego`) instead of loosening `dockerfile.rego`: the runtime rules (distroless, a numeric user, an entrypoint)
   do not apply to an export, and a switch in `dockerfile.rego` that skips them for a final `FROM scratch` would let a runtime image skip them too.
8. **Separate repository variables for the RPM key** (`LK_RPM_*`), not the image signing account's: the account that signs images must not read the RPM key, and the reverse.
   The tag pattern is `sentinel-v*`, not `v*`, so a product tag does not create two releases.
9. **The agent's StorageClass is rendered by the agent chart** (cluster-scoped), because `storage.rego` requires the class of every claim to be rendered with it and
   carry a key. So the swimlane's Argo CD project grows one cluster-scoped kind, by name (`lanekeeper-agent-cmek`), and the policy compares it. `reclaimPolicy: Delete`:
   the hub recovers lost spool contents by root comparison (HARDENING.md).
10. **`spool.maxBytes` is a value of the chart, not a tunable**, so that `storage.rego` can compare the claim with the bound; `config.tuning` refuses `LK_SPOOL_MAX_BYTES`
    and `LK_SPOOL_DIR`. The claim cannot be smaller than the agent's range allows (4 MiB) by the schema, and the policy keeps it larger than the bound.
11. **`contract.rego` now compares volumes (kind and one detail), the mounts as a set, and four path variables** with HARDENING.md, and ties `Recreate` and the rendered
    claim to the contract's own claim. It still does not compare the other variables (their values are the swimlane's, and `env-names.sh` holds the names).
12. **Renovate**: the custom manager's pattern now accepts digits in the variable name (`LK_ROCKY_8_9_IMAGE` was invisible to it, and so to its own test, whose filter had the
    same flaw), and the Rocky images are not proposed for another release.
13. **`crates/*` is untouched, and no contract path changed.** Not changed either: B5, B7, B8, the hub chart's `network.rego` gap, `images.yml`.

## Not done in T8

- **The release key and its trust root** (B13); the real `audit` package in the install test; a cgroup v2 host; the audisp plugin and SELinux against the real daemon (A17); a musl C
  toolchain; the sentinel's configuration and arguments (prompt 17); the sentinel workflow on GitHub; keyless signing and the release.
- **`CODEOWNERS`** has no entry for `deploy/sentinel/**`, `.github/workflows/sentinel.yml` or `RPM-GPG-KEY-lanekeeper.pub`; once the key exists, that file is a trust root
  and a change to it should need a platform reviewer.
- **The kind job** (still T5's omission, and A1). **Written in review round 1; see "Review round 1" at the end.**

## Environment

The Docker daemon was not running at the start of the run and was started with `dockerd` and its default settings. Disk was the constraint again (2.4 to 3.8 GB free for most of
the run): `target/grype-db` (3.3 GB of vulnerability database, downloaded each time the scan ran) was removed after each scan, the Docker builder cache was pruned (934 MB, then
11 GB once the rust image was removed), the unused `fedora` image was removed, and the rust base image was pulled again from `mirror.gcr.io` by digest. `target/debug` (7.8 GB)
was kept for the hardening tests. Nothing tracked was deleted.


# FINAL notes (2026-10-26)

## B14 (FINAL): `cargo deny check` and `cargo audit` fail on advisories whose fixed releases are younger than 14 days

**What failed (the committed tree at `d164637`, run 2026-10-26; the toolchain pins are the Justfile's: cargo-deny 0.19.9 and cargo-audit 0.22.2, built here with
`cargo +stable install --locked`).**

| Advisory | Crate in `Cargo.lock` | Found by | Fixed in | Fixed release published | Age on 2026-10-26 |
|---|---|---|---|---|---|
| RUSTSEC-2026-0382 (2026-10-20), certificate verification may accept distrusted emSign intermediates | `rustls-webpki` 0.103.15 | `cargo deny` (vulnerability, error) and `cargo audit` (vulnerability) | `>=0.103.16, <0.104.0-alpha.1` | `0.103.16` on 2026-10-20 | 6 days |
| RUSTSEC-2026-0383 (2026-10-19), bundled roots allow trust in distrusted emSign intermediates | `webpki-root-certs` 1.0.9 | `cargo audit` only (`deny.toml` limits the graph to Linux targets, where this crate is not reached) | `>=1.0.10` | `1.0.10` on 2026-10-19 | 7 days |
| RUSTSEC-2026-0386 (2026-10-13), `tokio::io::unix::AsyncFd::new` and the related constructors violate I/O safety | `tokio` 1.53.1 | `cargo deny` (unsound, error) and `cargo audit` (warning: unsound) | `>=1.53.3` (or `>=1.51.6, <1.52.0`); `1.54.0` is yanked | `1.53.3` on 2026-10-13 | 13 days |

`cargo deny check`: `advisories FAILED, bans ok, licenses ok, sources ok` (exit 1; the duplicate-version and one unused-licence-exception messages are warnings).
`cargo audit`: `error: 2 vulnerabilities found!` and `warning: 1 allowed warning found` (exit 1). Publication dates are from the crates.io API.

**Why this is a conflict and not something to fix.** AGENTS.md rule 15 says pinned versions must be at least 14 days old. The three fixed releases are 6, 7 and 13 days old, so
`cargo update -p rustls-webpki -p webpki-root-certs -p tokio` would pin releases the rule forbids, and Renovate (`minimumReleaseAge` 14 days) will not propose them yet. The other
way to make the gates pass is an `ignore` entry in `deny.toml` or `.cargo/audit.toml`, which weakens a check. Neither was done. Only a human can decide between a security fix and the
dependency age rule (AGENTS.md "Stop and escalate": a security requirement conflicts with another requirement).

**Not caused by prompt 09.** This branch did not touch `Cargo.lock`, `Cargo.toml`, `deny.toml` or `.cargo/audit.toml` (`git diff 5d67817^..HEAD` on them is empty). The two gates read only those files and the advisory database, so they give the same verdict on the lockfile this
branch started with: the advisories are newer than that lockfile. (The current main was not checked; if its lockfile has moved, it may differ.) `just verify` therefore cannot be green on this
branch until the lockfile moves, whatever happens to B5.

**What the human can choose.**
1. **Wait.** `tokio` 1.53.3 is 14 days old on 2026-10-27, `webpki-root-certs` 1.0.10 on 2026-11-02 and `rustls-webpki` 0.103.16 on 2026-11-03. A lockfile-only PR (`cargo update -p ... --precise ...`; no contract path
   changes) then clears both gates and the full `cargo test --workspace` is run again on it.
2. **Take the fix now**, recording in `docs/decisions.md` that a published security fix overrides the 14-day rule for these three crates. The same lockfile-only PR, today.
3. **A time-boxed ignore** for the advisories, which is the weakest and is not recommended.

Where the crates matter: `rustls-webpki` is reached through `rustls` by `tonic`, `reqwest`, `kube-client` and `octocrab` (the hub's and the agent's outbound TLS) as well as by the test dependency
`testcontainers`; `tokio` is in every crate. Whether the hub's trust anchors contain an emSign root (the condition in RUSTSEC-2026-0382) was not checked here.

**RESOLVED (review round 2, 2026-10-26).** A human decided the conflict between the fix and rule 15 in PR 30 (merged to `origin/main` as `7e53bf5`; the override is recorded in `docs/decisions.md`). `origin/main` was merged into this branch (`9eef6b4`), which brought the new `Cargo.lock` and `Cargo.toml` in; this branch did not touch them. On `3a67982`: `cargo deny check` prints `advisories ok, bans ok, licenses ok, sources ok`, `cargo audit` exits 0 (1355 advisories, 641 crate dependencies), and `just verify` as one command exits 0 after 4800 s. The text above is what was true before the merge.

## B15 (FINAL, benchmarks): `P6.index_settings` and `P12.audit_record` are over budget on every run

**What was run (2026-10-26, this branch's tree, Rust 1.88.0, `bench` profile, 4 vCPUs, 16 GB).** All 44 `[[bench]]` targets of the workspace, one after the other, then `cargo xtask bench-check` (run 1), then the three over-budget and two narrow-margin targets again once the coordinator reported that the machine was quiet. The p95 in ms is the value `bench-check` prints for each run (the same statistic the budget uses); a run is the target's own `cargo bench -p <package> --bench <target>`.

| Budget | Bench | Threshold | Run 1 | Run 2 | Extra 1 | Extra 2 | Extra 3 | Verdict |
|---|---|---|---|---|---|---|---|---|
| `P6.index_settings` | `engine/index_settings_6000_lines` | p95 <= 60 ms | 68.862 | 72.135 | 67.623 | 67.054 | 62.439 | **over on all 5 runs** |
| `P12.audit_record` | `hub-identity/audit_record` | p95 <= 3 ms | 3.472 | 3.590 | 3.571 | 3.137 | 3.342 | **over on all 5 runs** |
| `P7.grid` | `hub-registry/grid_40_swimlanes` | p95 <= 150 ms | 200.968 | 105.241 | | | | run 1 was a load artifact; passes |
| `P2.hub_head_moved_to_drift` | `hub-registry/head_moved_to_drift` | p95 <= 1000 ms | 956.532 | 741.565 | | | | passes (run 1 was close because of load) |
| `P15.registry_ingest_versions_per_s` | `hub-registry/ingest_versions_per_s` | min >= 1000 /s | 1065.008 | 1162.003 | 1190.026 | 1158.237 | | passes, 15 to 19% above the floor when quiet |

Run 1 ran while another agent's session used the same 4 vCPUs and Docker daemon (a `cargo check --workspace --all-targets` of about 9.5 minutes, whose time I was not given, and, from 05:22:04 to about 07:34 UTC, a `nice -n 19` per-crate `cargo test` run of the `lockfile-advisories` worktree). I did not record the load during run 1. Run 2 and the extra runs were made after that work was over, with a sampler logging the load and the three busiest processes every 10 s (`target/bench-logs/contention-run2.log`, `contention-extra.log`, `contention-extra2.log`): load average 0.7 to 1.3 for the five extra runs of `index_settings` and `audit_record`, with no process in the sampler that was not the bench, Docker, Postgres or this run's cargo; the run 2 log shows two single samples of other processes (`rm -rf target` at 07:37:56, during the grid rerun, and a `cargo metadata` at 07:41:37, during the `audit_record` rerun), owner unknown. Load can only slow a benchmark, so the runs that passed under load stand, and the two misses cannot be explained by it: `index_settings` first ran at 04:57 UTC (before the other session's test run) and missed, and every quiet run missed again.

**What the two misses look like.**

- `P6.index_settings`: the median of run 1's 100 samples is 40.4 ms (min 35.5, max 80.7); the budget is on p95, which is 5% of the samples at 62 to 80 ms. Prompt 04 measured **46.3 ms p95** on its machine (`plans/04-fact-engine.evidence.md`; a human then set the budget to 60 ms, `perf/budgets.toml` comment), so this machine gives 1.35 to 1.56 times that. The other engine benches against 04's figures: `P6.parse_flatten` 12.678 against 9.151 ms (1.39), `P6` 1650 against 1308 ms (1.26), `P7.engine_text_search` 193.7 against 151.4 ms (1.28), `P7.compare` 29.1 against 24.5 ms (1.19), but `P7.engine_grid` 39.7 against 47.5 ms (0.84). The crate has changed since 04's measurement (`29992b8 contract(08)` touched file entries), so a slower machine and a regression cannot be told apart from here.
- `P12.audit_record`: run 1's 500 samples have p50 1.57 ms, p95 3.47 ms and max 9.19 ms. Prompt 03a's own evidence (`plans/03a-hub-identity-audit.evidence.md`, rows R3 and R4) records the same bench passing and failing on repeats: 3.521 (fail), 2.802 (pass), 3.183 (fail), then 2.346, 2.324 and 2.054 on a quiet machine, and it accepted a band up to about 3.1. It is a database bench (Postgres 16 in a container, a second replica contending for the chain lock), so it also depends on the disk; this run's Docker data directory is on the sandbox's single disk.

**Why this is a stop for 09 and not something to fix.** Prompt 09 owns no application code and changed neither benched crate: `git diff --stat 5d67817^..HEAD -- crates/engine crates/hub-identity perf/budgets.toml Cargo.lock Cargo.toml` is empty. AGENTS.md says to stop and write the problem down when "a performance budget can't be met" (with measurements, above). The budgets are a contract path (`perf/budgets.toml`), and a miss is not made to pass by changing the budget, the bench, its sample count or the machine's settings. None was changed.

**What a human can choose.** (1) Measure on the bench runner CI uses (`vars.LK_BENCH_RUNNER`, "a quiet 4-core machine" per `bench.yml` A20; this sandbox is a 4-vCPU VM of unknown speed) and treat these two as noise or as real by that result. (2) Send them back to prompts 04 and 03a for a fix or a measured explanation. (3) Change the two thresholds through a `contract-change` PR, which only a human can approve. The same run also leaves the four web budgets below without a result; they are not a regression (next paragraph).

**Not measured in this run.** `P10.route_change_ms`, `P10.diff_6000_lines_ms`, `P10.grid_5000_rows_fps` and `P10.lighthouse_perf` come from `just web-perf` (Playwright's Chromium and Lighthouse), which prompt 08 owns. No browser is installed here and the download hosts `cdn.playwright.dev` and `playwright.download.prss.microsoft.com` answer HTTP 000 from this sandbox, so they have no result and `bench-check` fails on them ("registered but no result"). `P10.initial_js_gzip_kb` was measured: running `pnpm build` writes it, 195.908 KB against 250. `P7.docs_search` and `P7.feature_status` are `registered = false` in the registry ("no benchmark yet") and are UNMET, as they were before this branch.

## Review round 1 (FIX step): what was fixed, what is new, what is still unproven

Commits `643a896` (merge of `origin/main`), `f5402a5`, `d7a8aaa`, `c88a5c9`, `0032cee`, `df624a6`. Gates re-run recipe by recipe on `df624a6` (`plans/09-deployment-ci.evidence.md`, "Review round 1"): `just verify-09` still fails only at `deploy-render`, `policy-check` and `deploy-planted` for B5; `pins-test` 92 of 92, `policy-test` 1003 of 1003, `contract-check` 34 of 34, `cargo test -p hub --test hardening` 5 of 5.

### What changed, and what each fix leaves unproven

1. **Disk (resolved in the repository).** The PR 25 steps ("Free disk space", "Disk left") were carried into `rust.yml` `test` (merge commit `643a896`) and added to `images.yml` `image`, `bench.yml` `bench`, `nightly.yml` `bench` and the new `kind-e2e` job; `consistency.test.mjs` keeps them. Unproven: that the freed space is enough on a real runner; no workflow has run on GitHub.
2. **`sign-release` (resolved in the repository).** It compiles nothing now; the SBOM moved to a job without a token (`sbom`), and rule W6 of `check-workflows.mjs` refuses a build command in any job with `id-token: write`. **What W6 does not cover, and a person reads:** `sign-release` still runs `build.sh --tools` (a `docker build` of rpm and gpg from Ubuntu packages; the build container is given neither the job's token variables nor the key) and `sign.sh`; scripts and `just` recipes that a step calls are not looked into. Moving the tools-image build out of the signing job, as an artifact, would close that, at the price of trusting the artifact where the RPM is already trusted. Not done; not asked. Unproven: the artifact hand-over of `sentinel-sbom` into `target/sentinel/signed`, which relies on `upload-artifact`'s root-directory rule. **CORRECTION (review round 2, fixed in `077e32e`; the text above is kept as it was written): "It compiles nothing now" was false.** `build.sh --tools` ran `docker buildx build --target rpm`, and the `rpm` stage copies the binary from the `binary` stage, so BuildKit built the whole `binary` stage first: `cargo build --release --locked` for the sentinel, with the build script and proc-macro of every crate in its tree, plus the apt install, in the job that holds `id-token: write` and `environment: release` and that later receives the key. Reproduced on the old Dockerfile: BuildKit's plan of `--target rpm` lists seven `[binary n/7]` steps. The tools image is now the `rpm-tools` stage, which depends on no other stage (section "Review round 2" at the end).
3. **`storage.googleapis.com` (resolved in the repository).** Added to the hub chart's FQDN policy, `network.rego`, `HARDENING.md`, `hardening.rs`, with a planted case at each layer. Every other outside host of the hub crates was compared with the list; it was the only one missing. Unproven: enforcement by a cluster.
4. **The kind test (written, never run past cluster creation: B16).**
5. **`check-pins.mjs` (non-blocking 2, resolved).** A reference written as a block scalar is read.

### B16 (review round 1, informational): the kind test cannot be run here

**What was tried (2026-10-26).** `deploy/tests/kind/run.sh` with the pinned node image `kindest/node:v1.37.0@sha256:a1ed56cf...` (pulled in 17 s from Docker Hub).
- Without a configuration of my own, the script stops in its preflight: `this host uses cgroup v1, and the kubelet of the pinned node image ... refuses to run on it` (exit 2). The kubelet's own message: `kubelet is configured to not run on a host using cgroup v1`.
- With a scratch configuration that sets `failCgroupV1: false` (outside the repository, passed in `LK_KIND_CONFIG_FILE`), the kubelet starts and the control plane never comes up, because no pod sandbox can be created: `OCI runtime create failed: runc create failed: unable to start container process: can't get final child's PID from pipe: EOF; runc init error(s): nsexec[...]: failed to update /proc/self/oom_score_adj: Permission denied`. This is the sandbox's nested-container limit, not the script, the configuration or Kubernetes. The script's output for that run: `FAILED in the stage 'cluster': kind could not create the cluster`, its summary table, exit 1, the cluster and the registry removed.
- Even a working cluster could not pull the CloudNativePG operator image or the database image: **`pkg-containers.githubusercontent.com` is refused** (`crane config` of `ghcr.io/cloudnative-pg/cloudnative-pg:1.30.1`: `Forbidden`), which is B9 again. `registry.k8s.io` answers HTTP 000, which a kind node does not need (its images are in the node image).

**Hosts probed from this sandbox:** `registry-1.docker.io` 401 (reachable, the node image pulled), `auth.docker.io` 200, `mirror.gcr.io` 401, `ghcr.io` 401 (manifests are readable; `crane digest` worked), `gcr.io` 401, `github.com` 200 (the kind binary and the CloudNativePG manifest downloaded), `hub.docker.com` 200, `objects.githubusercontent.com` 404, **`registry.k8s.io` 000**, **`pkg-containers.githubusercontent.com` 403**.

**What this means.** Everything in `run.sh` past `kind create cluster` is unproven: the registry push and a node's pull by digest, containerd's `config_path` for `kind-registry.test` (the node image's own `config.toml` has none, so `run.sh` patches it through `containerdConfigPatches`), the operator, the database, every S6 check against CloudNativePG, the hub stage, `/readyz`, the time to Ready, and the A1 report on real pods. What was shown instead is in the evidence file: the S6 patterns against a stand-in PostgreSQL 16 with pgvector and the chart's `pg_hba` order, the manifest pin and rewrite, the `jq` expressions on canned data, the cgroup refusal. **The first CI run on a cgroup v2 runner is the test of the rest.** A person should expect it to find at least a small mistake.

**It fails by design until A1 is resolved.** When the three replicas log `refusing to start without them` and exit 1 (`crates/hub/src/main.rs`), `run.sh` prints that this is plan 09 A1, that the replicas started and read their settings, and that the chart, the database and the image are not at fault; it exits 1. Nothing retries, fakes readiness or skips the stage. **For a human:** (a) assign the composition of the hub binary (A1); (b) decide whether a red `kind-e2e` blocks a merge (the `publish` job does not wait for it, so images are still published); (c) a nightly run of the kind test was not added (it would re-run the whole `images.yml`), say if you want one.

**Update after the second CI run (2026-10-26, run 40366344156).** The prediction above held: the first run on a cgroup v2 runner found a mistake, in the registry stage (B21). What the runner proved: the cluster is created (1 control plane and 3 workers from the pinned node image, 50 s), so the preflight, the kind configuration and `containerdConfigPatches` parse. What is still unproven is everything after it, as listed in B21.

### B14 after this step

PR 30 (`7e53bf5`, "lockfile advisories") was merged into `origin/main` while this step was running. The instruction was not to merge it, so this branch was merged only up to `2d7d1cc`, the commit before it. B14 therefore still stands on this branch: `Cargo.lock`, `Cargo.toml`, `deny.toml` and `.cargo/audit.toml` are untouched. A plain `git merge origin/main` brings the fix in (`git merge-tree HEAD origin/main` is clean; the difference is `Cargo.lock`, `Cargo.toml` and `docs/decisions.md` only); whoever does it should then re-run `cargo deny check`, `cargo audit` and `just verify`.

### Non-blocking items of the review that were left as they are

1 (B7 and B8), 3 (`network.rego` and a second NetworkPolicy; `contract.rego` and the hub), 4 (B9 and B10), 5 (B11, B12, B13 and what never ran), 6 (`build_ca` secret mounts in the production Dockerfiles), 7 (`bench.yml` and `bench-check-rust`), 8 (branch protection and required-check names), 9 (`items: [uri]`, startup probe), 10 (T3 ticked while Cloud Armor is unproven), 11 (what the reviewer did not re-run).

## Review round 2 (FIX step): what was fixed, what is new, what is still unproven

Reviewed at `9eef6b4` (`plans/09-deployment-ci.review.md`, commit `33d6cf3`): CHANGES REQUESTED, one blocking finding. Commits: `077e32e`, `6011140` (blocking 1), `8211d2a`, `e4920db`, `6ebdbc0`, `3a67982`, then the `09/evidence:` commits. Human-owned items left untouched, as instructed: B5 (no range typed, no marker, refusal or test changed), B15 (the benchmarks in crates 09 does not own), A1, A19, and `Cargo.lock`, `Cargo.toml`, `deny.toml`, `.cargo/audit.toml`, `perf/budgets.toml`, the frozen plan.

### What changed, and what each fix leaves unproven

1. **The `sign-release` job compiled the sentinel (blocking 1; fixed in the repository).** Reproduced first: the plan that BuildKit printed for `--target rpm` of the old `Dockerfile.build` held `[binary 1/7]` to `[binary 7/7]`, `cargo build --release --locked ... -p sentinel` among them, because the `rpm` stage copies from `binary` and `build.sh --tools` built `rpm`. That job holds `id-token: write` and the `release` environment and later receives the Workload Identity credential and the key, so a crates.io dependency's build script would have run as root in a container next to them. The round-1 claim (above, item 2) was wrong. **Fix:** a stage `rpm-tools` (pinned Ubuntu base, the apt install, `WORKDIR`) with no dependency on another stage; `rpm` extends it; `build.sh --tools` builds `rpm-tools`. Held by `dockerfile_artifact.rego` (the stage exists, starts from a pinned image, uses no other stage, runs no build tool; gpg and gpg-agent pinned), by `build-tools.test.mjs` (the real `build.sh` with a recording `docker`) and by `tools-graph.sh`, which reads BuildKit's own plan (`just sentinel-tools-graph`, in the `policy` job of `sentinel.yml`): `rpm-tools` plans 3 steps, all its own; `rpm` plans 10 (7 of `binary`). The real `--tools` build ran here (19.8 s; its log has no `binary` and no `cargo`), `sign-test.sh` passes against its image, and the RPM and the binary are byte-identical to the ones the old Dockerfile makes from the same context and date. The `sentinel.yml` header, the step's comment, `deploy/sentinel/README.md` and `docs/threat-model.md` now say what is true. **What the job still runs:** the apt install of Ubuntu's signed rpm, gpg and gpg-agent packages (and their libraries) as root in a build container, before it authenticates; the maintainer scripts of those packages run there. Doing the build in a tokenless job and handing the image over as a `docker save` artifact would remove it; not done (a person's decision). **Pins:** rpm exact; gpg and gpg-agent `2.4.4-2ubuntu17*` (a glob over the archive revision, because a superseded revision leaves the archive's index); the libraries are not pinned; the build prints and the image keeps the installed versions. **Unproven:** a run on GitHub with the real key (B13); `tools-graph.sh` on a GitHub runner's default builder; `sentinel-repro` and `sentinel-test` were not re-run (Docker Hub 429 for the Ubuntu base: pulled from `mirror.gcr.io` by digest and tagged locally, no daemon setting changed).
2. **W1, W2 and W6 (non-blocking 1; fixed).** Quoted values, `sh -c`/`bash -c`, `docker run IMAGE cargo`, `docker build`, `python -m pip`, `uv`, `rustup`, `curl | sh` and the like are read; six planted workflows were accepted by the round-1 checker (five outright) and are refused now. **What no text rule can see is written in the rule's header and as residual 28 of the threat model:** scripts and `just` or `make` targets that a step calls, local composite actions (`docker-hub-mirror` runs in `sign-release`) and reusable workflows, programs under a name the list does not hold, commands built in a variable, multi-line quoted `run:` values, workflows outside `.github/workflows`. A reviewer reads the three jobs with `id-token: write`.
3. **The kind test (non-blocking 2; fixed, still never run).** The `/readyz` read through the API server's pod proxy is gone (the hub chart's default-deny ingress on 9090 would refuse it once A1 is fixed); the `Ready` condition is the check. P11 is asserted at 10 s per replica (`lib.sh` `p11_verdict`, tested to agree with `perf/budgets.toml`) and the summary says `NOT MEASURED` while the replicas cannot start; the A1 path is unchanged. Unproven: everything past cluster creation (B16), and whether kindnet enforces the hub's NetworkPolicy and leaves the kubelet's probes alone.
4. **Renovate and the kind pins (non-blocking 3; fixed).** `pins.env` is covered: the node image and the operator image by the docker manager, the CloudNativePG version and manifest URL by two `github-releases` managers. **Not expressible:** `LK_CNPG_MANIFEST_SHA256` (Renovate cannot hash a release asset) and the CRDs in `deploy/schemas` and the db chart README, which name the version too; so the CloudNativePG group waits for approval on the dependency dashboard and carries a note of what a person finishes. Said in the new `deploy/tests/kind/README.md`. Unproven: that Renovate accepts the configuration (not run).
5. **The evidence (non-blocking 4; refreshed).** `plans/09-deployment-ci.evidence.md`, section "Review round 2": every recipe alone on `3a67982`, `verify-09` fails only for B5, and `just verify` passes as one command.

### Items that need a human, after round 2

- **B5** (the hook ranges), **A1** (the hub binary exits 1), **B15** (the two benchmark budgets; `cargo xtask bench-check` was not re-run), **A19** (prod `isLocked: true` with the example period): unchanged and open.
- B7 to B13, B16 and everything that needs GitHub or Google (no workflow has run): unchanged.
- A new decision for a person, not a blocker: whether to move the tools image out of the signing job (a tokenless job and a `docker save` artifact), which removes the apt install from it.

### Non-blocking items of the review left as they are

5 (commit names: a note), 6 (what cannot be verified here: the SHA-to-tag mapping of the pinned actions, any GitHub run, kind past cluster creation) and 7 (B5, A1, B15, A19, B7 to B13: still reported honestly).

## B17 (review round 3, reproduced in the cleanup step): `cargo deny check` and `cargo audit` fail on four zstd advisories, and a lockfile bump cannot reach the fix

**State: open. A human decides. The same class as B14, but not the same way out.** Not caused by this branch, and not fixable inside prompt 09's paths. Review round 3 found it (`plans/09-deployment-ci.review.md`, round 3, non-blocking 1); this section is what I reproduced myself.

**Reproduced on 2026-10-26 at 14:27 UTC, on branch head `86bdc40`, each command alone, with the advisory database fetched by the tools themselves.**
- `cargo deny check`: **exit 1**, `advisories FAILED, bans ok, licenses ok, sources ok`, four `error[vulnerability]`.
- `cargo audit`: **exit 1**, `Loaded 1378 security advisories`, `error: 4 vulnerabilities found!`, `warning: 10 allowed warnings found` (ten `unsound` warnings on zstd-safe 7.3.0: RUSTSEC-2026-0395, -0396, -0397, -0399, -0400, -0401, -0402, -0404, -0405, -0406; warnings, not failures).

| Advisory | Locked crate | Title | Fixed in | Fix published (crates.io API, read 2026-10-26) |
|---|---|---|---|---|
| RUSTSEC-2026-0391 | zstd 0.13.3 | Data from an abandoned stream leaks into the next stream on a reused context | >= 0.14.3 | zstd 0.14.3, 2026-10-16 16:59:51 UTC |
| RUSTSEC-2026-0398 | zstd-safe 7.3.0 | `Seekable::decompress` sets a `Vec` length beyond its capacity for offsets past the end | >= 8.2.0 | zstd-safe 8.2.0, 2026-10-16 16:59:12 UTC |
| RUSTSEC-2026-0403 | zstd-safe 7.3.0 | Uninitialized memory exposed when streaming decompression of a legacy frame fails | >= 8.2.0 | same |
| RUSTSEC-2026-0410 | zstd-sys 2.0.16+zstd.1.5.7 | WebAssembly allocator shim corrupts memory on allocation failure | >= 2.1.2 | zstd-sys 2.1.2+zstd.1.5.7, 2026-10-16 16:59:08 UTC |

All four advisories are dated 2026-10-16. None of the fixed releases is yanked. Rule 15's 14-day minimum is met for all three from **2026-10-30 at 16:59 UTC**.

**Why `just verify` passed on 2026-10-26 and does not reproduce now.** The one-command run (12:27:55 to 13:47:55 UTC, on `3a67982`) printed `advisories ok` and `Loaded 1355 security advisories` (lines 12287 and 12290 of its log). The advisory database now holds 1378 entries (newest commit `a78282a`, dated 2026-10-26 13:45:01 UTC) and contains these four. The lockfile did not change; the database did. I did not record which database commit the run used, so I cannot say to the minute when the entries arrived. `just verify` was not re-run as one command in this step (80 minutes): it stops at `cargo deny check`, the step before `cargo audit`, and both steps were run alone.

**This branch is not the cause, and main fails the same way.** `Cargo.lock`, `Cargo.toml`, `deny.toml` and `.cargo/audit.toml` on this branch are identical to `origin/main` (`7e53bf5`; `git diff --quiet` on each file, also against the tip fetched at 14:28 UTC). No dependency file was edited in this step either, and no ignore was added.

**Who pulls zstd in.** Only `tonic` 0.14.6 (`cargo tree -i zstd`): its `zstd` feature, which `hub-platform` and `proto` enable for compressed gRPC streams (P4, and the stack names "tonic with zstd compression"). `zstd` brings in `zstd-safe` and `zstd-sys`.

**Why waiting for 2026-10-30 may not be enough.**
- `tonic` 0.14.6 is the newest tonic on crates.io (published 2026-05-07), and it declares `zstd = "0.13.0"` (optional), which excludes 0.14.x. `zstd` 0.13.3 needs `zstd-safe` 7.x. So `cargo update -p zstd` cannot reach the fix while tonic 0.14.6 is the newest tonic: the lockfile cannot leave the 0.13 line.
- So the way B14 was closed (D134: a recorded override of the age rule, then a lockfile bump) does not carry over by itself. After 2026-10-30 the age rule is no longer in the way, but the dependency range still is.
- What would unblock it, none of which I tried (no dependency file may be touched here) and none of which I checked beyond the registry: a tonic release that accepts zstd 0.14; a recorded decision to accept the advisories for now (an `ignore` in `deny.toml` and `.cargo/audit.toml` with the reason and an expiry, as `RUSTSEC-2023-0071` already has); or a decision about the tonic `zstd` feature itself (P4 and the stack name it).
- Whether the four advisories can be reached by how the hub and the agent use zstd (tonic's gRPC message compression between mTLS-authenticated peers) was **not analysed**. That belongs in the human's decision.

**For a human:** decide between (a) a recorded override or ignore with an expiry, (b) waiting for a tonic release that takes zstd 0.14 (and for 2026-10-30 for the age rule), (c) something else. Until then `just verify` is not green on this branch or on `main`, and the PR text must not say it is.

# CI first run (2026-10-26, after the first GitHub run of PR 31 at `671fe4e`)

The full account, with the commands and the output, is in `plans/09-deployment-ci.evidence.md`, section "CI first run". This is the part that a person has to read.

## What failed on GitHub, the root cause, and the fix (all local commits, not pushed, **nothing proven on GitHub yet**)

| Job | Root cause | Fix |
|---|---|---|
| `images` / `scan-test` | the grype image's `/tmp` is root's (0755); `scan.sh` ran it as the runner's uid | `60f1693`: `deploy/images/container.sh`, a `/tmp` and `HOME` that the caller owns |
| `sentinel` / `sign with a throwaway key` | `--cap-drop ALL` and no `--user`: root without `CAP_DAC_OVERRIDE` cannot write a directory the runner owns | `bc57355`: the container runs as the caller's uid, tmpfs owned by it; isolation unchanged and checked |
| `images` / `image hub`, `image agent` ("Write the SBOM", exit 1, no output) | syft hit the same `/tmp`, and `--quiet` hid the message | `60f1693`, `8ecbe8b`, `7b45bf7`: not quiet, every failure says which command, line and exit code |
| `bench` (33 min, then `Unrecognized option: 'baseline-lenient'`) | `--benches` also runs the lib and bin unit-test harnesses, which reject criterion's options | `a3093de`: `--bench '*'`, rule W7, timeout 240 to 300 minutes |
| review bot, 5 findings | 1 did not reproduce as stated (its variants did), 2, 3 (`./gradlew` only), 4, 5 reproduced | `5fbed87` (2), `a3093de` (1, 3, 4, 5); gaps that remain are named in the checker's header and in threat-model residual 28 |

## New, for a human (none of them is in prompt 09's paths, and none is touched)

**B18. The six fuzz jobs of `rust.yml` fail on the first run: the fuzz crates are out of step with the workspace.** `fuzz agent-fuzz` and `fuzz mcp-fuzz`: `failed to select a version for tokio`, `=1.53.3` required by the crate against `=1.53.1` pinned by the fuzz crate's own manifest; `fuzz registry-fuzz`, `writes-fuzz`, `identity-fuzz`, `platform-fuzz`: `cannot update the lock file crates/<crate>/fuzz/Cargo.lock because --locked was passed`. The fuzz crates belong to their own prompts and carry their own lockfiles. The nightly workflow runs the same recipes, so it will fail the same way. Probably the workspace pins moved (PR 30) and the fuzz manifests and locks were not moved with them: not checked beyond the log. **For a human:** whoever owns those crates regenerates the fuzz manifests and lockfiles; 09 can add a CI check that a fuzz crate's pins equal the workspace's, once they agree.

**B19. `osv-scan` (the `osv` job of `web.yml`) fails: advisories in lockfiles.** Besides the zstd crates (B17): `dompurify` 3.4.16 (GHSA-cg2g-xh64-v4q7, CVSS 9.2, fixed in 3.4.17; GHSA-mrv2-3pqr-j5jr 7.6; GHSA-c6gr-qvv4-5v3f), `undici` 7.29.1 and 7.30.0 (7.30.1), `fast-uri` 3.1.8 (3.1.9), `@vitest/mocker` 4.1.11 (4.1.12), `zod` 3.25.76 and 4.6.5 (4.6.6), all in `web/pnpm-lock.yaml`, and `RUSTSEC-2026-0310` against `domain 0.1.0` in `fuzz/Cargo.lock` and the other fuzz lockfiles: that is almost certainly osv-scanner matching this workspace's own path crate `domain` to an unrelated crates.io crate of the same name, a false positive. `dompurify` is not the docs sanitiser (that is ammonia, on the server, S12): it is Monaco's Markdown renderer's dependency, pinned to 3.4.16 by an override in the lockfile, and `web/src/editor/entry.ts` says the hub's CSP does not let it create its Trusted Types policy. Whether that makes the 9.2 advisory unreachable was **not analysed**; it belongs in the human's decision. Dependency files and exceptions are not 09's. **For a human:** bump the npm packages (each fixed release must be at least 14 days old, rule 15, or the age rule needs a recorded override as in B14), and decide how the `domain` collision is to be excluded (an osv-scanner ignore entry with a reason, or a rename of the crate).

**B20 (informational). The first cold-cache compile of the bench job took 33 minutes, 28 of them in `jsonschema 0.58.3`.** The workspace `[profile.release]` has `codegen-units = 1` and `lto = "thin"`, which the bench profile inherits, so one generic-heavy dev-dependency of `hub-mcp` compiles on a single core. Not changed (the workspace `Cargo.toml` is not 09's). A candidate for whoever owns it: `[profile.bench.package.jsonschema] codegen-units = 16` or `opt-level = 1`, measured on a cold cache. `bench.yml`'s timeout is now 300 minutes because of it.
`bench.yml`'s timeout is now 300 minutes because of it.

**Relaxed on 2026-10-26, on a human's decision, by the `09/fix:` commit that added `[profile.bench.package.jsonschema] codegen-units = 16` to the workspace `Cargo.toml`** (the shared file the guard allows). What it does and does not do:
- Only `jsonschema` in the **bench** profile changes. `[profile.release]` keeps `codegen-units = 1` and `lto = "thin"`, so the shipped images are built as before (and `jsonschema` is a dev-dependency of `hub-mcp`, so it is not in them). Checked with `cargo build --profile bench --unit-graph` and the same with `--profile release` (`RUSTC_BOOTSTRAP=1 cargo build ... --unit-graph -Z unstable-options --locked --offline -p hub-mcp --benches --tests`; it prints the plan and compiles nothing): the two plans differ in exactly one unit, `jsonschema@0.58.3 lib`, 16 against 1.
- No benchmark of `hub-mcp` (or of any crate) uses `jsonschema` (grep of `crates/*/benches`: no match), so no measured number is compiled with the relaxed setting. `.github/scripts/cargo-profile.test.mjs` holds the three facts: the release profile, the single override, and the benches.
- `cargo metadata --locked --format-version 1` passes and `Cargo.lock` is unchanged (`git diff --stat Cargo.lock` is empty).
- **The saving is UNMEASURED.** No cold compile was run here (the instruction was not to), so 28 minutes of `jsonschema` becoming shorter is a prediction, not a result, until the next cold-cache run of the bench job on GitHub. `bench.yml`'s 300-minute timeout is left as it is. Whoever reads that run should compare the `jsonschema` line of `cargo --timings` or the job's total with the 33 minutes above.

**Not a blocker, seen once: `web-e2e` shard 2 of 5** failed at `pnpm exec playwright install --with-deps chromium` after 10 minutes on the re-run, where the other shards took 20 seconds. If it repeats, the step needs a timeout and a retry.

## B21 (CI second run; fixed in `743337d`, and the fix held on the third run): the kind test's `hub image` stage was refused by the registry. Cause: joining the kind network took the published loopback port away

**Update after the third run (run 40374410436, head `13dd92e`): resolved.** The probe that `743337d` runs right after `docker network connect kind` printed `http://127.0.0.1:32768/v2/ does NOT answer (after the registry joined the kind network): curl: (7) Failed to connect` and `http://172.18.0.6:80/v2/ answers`, so **the cause is shown: on the runner, joining the kind network removes the published `127.0.0.1` port while the container lives**, and the earlier text below ("not established", the candidates) is history. The second route carried the push (`NOTE: ... the image is pushed there`), and `hub image` passed in 2 s, then `operator`, `database` and `s6`. Why Docker does it was not investigated. Where a container's address is not reachable from the host (Docker Desktop's VM), neither route would answer after the join; pushing before the join is the order that works there, not needed on the runner. **For a human:** nothing.

**What GitHub showed** (run 40366344156, head `176b2dd`, job `kind, the hub chart with CloudNativePG`, 17:29 UTC):

```
| cluster | PASS | 50 | |
| hub image | FAIL | 0 | |
Error: Get "https://localhost:32768/v2/": dial tcp [::1]:32768: connect: connection refused; Get "http://localhost:32768/v2/": dial tcp [::1]:32768: connect: connection refused
```

The images jobs of the same run are green (scan-test, image hub, image agent, policies and scripts), so the scanned archive existed and was downloaded intact. The cluster stage passed: kind v0.33.0, `kindest/node:v1.37.0`, one control plane and three workers in 50 s. The workers were `NotReady` (11 and 12 s old) in the node table that the stage printed before it passed. This is **not** the intended failure (A1: three replicas that log `refusing to start` and exit 1); the script never reached the hub stage.

**What the log proves, and what it does not.**
- Not a missing wait: the `cluster` stage waited for the registry (`curl http://127.0.0.1:32768/v2/`, up to 30 asks) and passed, 46 s before the push; a push right after `docker run` also succeeded three times out of three here.
- The only route the push used was the host port that Docker published on `127.0.0.1`, through the name `localhost`; it was refused, and nothing printed the container's state, ports or log.
- Inferred (Go's dial order, not observed): `localhost` is `::1` and `127.0.0.1` on the runner; Go dials `[::1]` first and reports only that error when both fail, so `127.0.0.1:32768` was refused too.
- **Not known: why a port that answered at 17:29:13 was refused at 17:29:59.** In between: `kind create cluster`, `docker network connect kind kind-registry.test`, four `hosts.toml` writes. Candidates (none shown): the loopback mapping lost when the container joined the kind network; the registry container exited; something about the runner's Docker networking. Not reproducible here: joining a user-defined network left the published port answering; there is no IPv6 in this sandbox, so `[::1]` cannot be tested; and the host is cgroup v1, so no kind cluster starts.

**The fix** (`743337d`, tests first, red before; see the evidence file, "Second run"): the registry is its own stage with a bounded wait (60 s); `hub image` asks again on every route (`127.0.0.1:<port>`, then the container's address on the kind network, port 80) and pushes to the first that answers, saying so; the push uses `127.0.0.1`; both routes are probed after the registry joins the kind network and at a failure; a failure prints the container's state, exit code, ports, addresses, log tail and the host's sockets on the port; a command that fails with no reason of its own is named with its exit code and line; the cluster stage waits for all four nodes to be Ready. `--registry-only` runs the registry stages without a cluster; `deploy/tests/kind/registry-test.sh` (`just kind-registry-test`, in `verify-09` and in the `policies and scripts` job) runs it against a real registry.

**What the next run will show, and what to read.** In the `kind-e2e` log: `the registry route ... answers|does NOT answer (after the registry joined the kind network)` says whether the join took the published port away; `NOTE: the published port ... does not answer any more; the registry answers at its address on the kind network` says the second route carried the push; a failure prints `---- the registry container kind-registry.test: state` with status and exit code (an exited container is the other cause). `diagnostics.txt` in the `kind-e2e-logs` artifact holds the same.

**Suspected, not shown, and no fix made** (read from the script and the charts, and run locally where it was possible; each is a thing the next run may find): (1) a node's pull of `kind-registry.test/lanekeeper/hub@sha256:...` (Docker's DNS name for the registry on the kind network; `hosts.toml` read by containerd 2.3.4 through the patched `config_path`); (2) the pulls of the operator and PostgreSQL images from `ghcr.io` on the runner (B9: layers refused here); (3) the CloudNativePG webhook and CRDs being ready when the db chart is installed right after `rollout status` (the operator's readiness probe is `/readyz` on the webhook port, which suggests yes); (4) that `condition=Ready` is what this CloudNativePG version sets on a Cluster; (5) that the pinned PostgreSQL image holds `vector` and `pg_trgm`; (6) kindnet and the hub's NetworkPolicy. **Shown right locally:** the CloudNativePG manifest's sha256 and the rewrite of its image to the pinned digest (exactly 2 places); the hub chart's StatefulSet name, `podManagementPolicy: Parallel`, selector and Secret names against what `run.sh` uses; the A1 text that `hub_failed` greps, against `crates/hub/src/main.rs`.

**For a human:** nothing yet. If the next run shows both routes dead with an exited container, the registry image or the host is at fault and the container's log says how; if the second route carries the push, the published port is the fragile part and the README of the kind test says so.

## B22 (CI third run, fixed locally in `fc34383`, not run again): the hub stage called a crashing-on-purpose hub a chart, database or cluster fault, because a log was no log

**What GitHub showed** (run 40374410436, job 121762447640): `registry` 3 s, `cluster` 47 s, `hub image` 2 s, `operator` 12 s, `database` 97 s and `s6` 1 s all PASS on a runner (the chart, CloudNativePG with the pinned image, and S6 on the running database work), then `hub` FAILED after 17 s: `the hub did not become Ready, and its logs are not the refusal of A1 (2 of 3 replicas): the chart, the database connection or the cluster is at fault`. All three replicas were in CrashLoopBackOff with exit 1 and restartCount 1, 2 and 1. The logs of `lanekeeper-hub-0` and `-2` held the A1 refusal; for `lanekeeper-hub-1` the script printed `unable to retrieve container logs for containerd://f0ccf06c...`.

**Cause (shown by the log, and reproduced with a stand-in).** `kubectl logs --previous` answers, with **exit 0**, with that sentence in place of a log when the kubelet has already removed the container. `run.sh` ran `logs --previous || logs`, so the second command never ran, the sentence was counted as the replica's log, 2 of 3 replicas refused, and the stage said that the chart, the database or the cluster was at fault. It was false: the replica had exited 1 in the same second as the others, and the database and S6 had just passed. The red was right (A1 is open and the stage must be red); the words were not.

**Fix** (`fc34383`, tests first): each replica's log is asked for from the current and the previous container for up to 30 s, the sentence and an empty log are no log, each replica's state is printed (restartCount, ready, waiting reason, exit code, started, finished), and a replica counts as an A1 refusal only if a retrieved log holds the exact text of `crates/hub/src/main.rs`. Verdicts, all red, P11 `NOT MEASURED`: all 3 refusals, A1 as designed; some refusals and the rest with no log but the same state (exit 1, never Ready), `A1 seen in the logs of N of 3 replicas; no log could be retrieved for ...; A1 is not claimed for it`; another error in a log, verbatim, `not A1`; a replica without a log in another state, `not explained by A1`; no logs, nothing claimed. It never blames the chart, the database or the cluster. The partial case is reported as what it is (not as A1 and not as inconclusive): one StatefulSet, one image and one configuration make the same exit likely the same cause, but a state is not a log.

**Also read from this run, nothing wrong found:** the `database` stage took 97 s of its 900 s bound (a runner nine times slower fits), and the stage bounds together stay inside the job's 60 minutes (about 41 with every wait at its limit, up to 15 more if the 1.3 MB manifest download used all three retries of a 300 s transfer, which is not credible; the run took 4); `s6` checked what the plan says (TLSv1.3 positive control; `pg_hba.conf rejects connection ... no encryption`, which is the chart's `hostnossl ... reject`; `tlsv1 alert protocol version` for a TLS 1.2 client; `vector`, `pg_trgm`, `en_US.utf8`); the nodes were all `Ready` when the cluster stage printed them (the wait of `743337d`). The `kind-e2e-logs` artifact could not be downloaded here (the blob host is refused); this is from the job log.

**What is still unproven:** the hub stage past A1 (A1 is open: the hub does not start), P11, the log fix on a runner (the next run is the test), kindnet and the hub's NetworkPolicy. **For a human:** nothing new; A1 is what keeps `kind-e2e` red.

## What is still unproven, said plainly

The three container fixes are proven for uid 1001 with the Docker group on a machine that is not a hosted runner (real grype database, real syft and rpm tools, real scripts end to end where possible); they are not proven on GitHub. The hub and agent images were not put through `sbom.sh` and `scan.sh` here (a hand-made image archive and a one-file image were). `scan.sh --image` (the database image, **B9**) was not run. Every job that was never reached because a job before it failed has not run at all: `sentinel` / `SBOM and scan`, `install on Rocky Linux 8.9 and 8.10`, `exposure score`, `sign with the release key`, `publish the release`; `images` / `publish`; `kind-e2e` (**A1**, **B16**). The bench timeout is an estimate from two measurements on different machines. The checker changes are proven on their fixtures and on 4,882 lines of this repository; the rule is text, and residual 28 says what it cannot see.

**After the second run (2026-10-26, 17:11 to 17:30 UTC, head `176b2dd`):** the container fixes, the hub and agent images through `sbom.sh` and `scan.sh`, build-twice reproducibility for both images, and the sentinel's signing with a throwaway key, `SBOM and scan`, `install on Rocky Linux` and `exposure score` are **proven on the runner** (jobs green). The cluster stage of `kind-e2e` is proven. Still unproven: everything in `kind-e2e` after the registry stage and the registry fix itself (B21) **(superseded by the next paragraph)**; `images` / `publish` and the sentinel's release-key jobs (OIDC, Sigstore, the release key: B13); `bench` through a cold cache (running when this was written).

**After the third run (2026-10-26, 18:21 to 18:25 UTC, head `13dd92e`):** the stages `registry`, `cluster`, `hub image`, `operator`, `database` and `s6` of `kind-e2e` are **proven on a runner** (the registry fix held, B21; CloudNativePG with the pinned manifest and image, the three-instance database with TLS 1.3 only, and S6 on it work). The hub crashes with the A1 refusal on all three replicas, as designed, and the stage's verdict text was wrong because of a log-collection race (B22, fixed in `fc34383`, not run again). Still unproven: the hub stage past A1, P11, the B22 fix on a runner, kindnet and the hub's NetworkPolicy; `images` / `publish` and the sentinel's release-key jobs (B13); `bench` through a cold cache.
