# Plan 09 blockers

Status on 2026-10-25, T3 run: **B1 to B4 are resolved; B5 is open and stops `just verify-09` (and nothing else of T3).**
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
