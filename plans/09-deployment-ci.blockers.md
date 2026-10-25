# Plan 09 blockers

Status on 2026-10-25, T5 run: **B1 to B4 are resolved; B5 is open and stops `just verify-09` (and nothing else of T3, T4 or T5); B7 and B8 are informational and need a human before a swimlane is deployed; B9 is informational; B10 is a decision that T5 needs from a human (the backup method and the image of the database), built meanwhile with the interim choice below.**
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
| B7 | **open, informational (T4)** | the agent contract dials the hub on 443; the hub chart's agent load balancer listens on 8444 (T4 notes below) |
| B8 | **open, informational (T4)** | the agent contract lets the agent reach the config-server only in the agent's own namespace (T4 notes below) |
| B9 | **open, informational (T5)** | `pkg-containers.githubusercontent.com` (where `ghcr.io` keeps image layers) is refused: the database image's layers and config cannot be read here (T5 notes below) |
| B10 | **open, decision needed (T5)** | the plan names CloudNativePG's `standard` image for the database and built-in barman for the backups; the `standard` image has no barman (T5 notes below) |

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
6. **A19: prod's audit bucket is locked in the committed prod values, with an EXAMPLE period of one year.** The policy requires the lock in prod, so
   the values must have it to pass; the file says in a comment that a person confirms the period before prod is applied, and the project in
   it is an example, so it cannot be applied as it is. Staging is never locked (the template and the policy refuse it).
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
  that was never run would be a guess, so it is left for a task of its own; `deploy/values/{hub,db}/kind.yaml` exist for it.
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
