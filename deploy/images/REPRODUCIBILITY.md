# Reproducible images (09/T1, S20)

Two builds of one commit must give one image digest. `just images-repro` (`deploy/images/reproducible-check.sh`) builds
each image twice without the build cache, from two separate `git archive` extractions, and fails unless the digests are
equal; on a difference it prints the config fields, layers and files that differ (`oci-diff.py`).

## What is pinned

| Input | How it is pinned |
|---|---|
| Base images (rust, node, distroless) | by digest in each `FROM` (policy: `deploy/policy/dockerfile.rego`) |
| The Dockerfile frontend | `# syntax=docker.io/docker/dockerfile:1.28.0@sha256:…` |
| Rust crates | `Cargo.lock`, `--locked` on `cargo chef cook` and `cargo build` |
| cargo-chef | `cargo install --locked cargo-chef --version 0.1.78` (its own lockfile) |
| The Rust toolchain | the one in the pinned rust image; `RUSTUP_TOOLCHAIN=1.88.0`, so rustup never fetches another |
| npm packages and pnpm | `pnpm-lock.yaml` with `--frozen-lockfile`; pnpm 10.34.6 through corepack (signature-checked) |
| The embedding model | `hub-model.lock`: commit, size and sha256 of every file, checked in the build |
| The supply-chain tools | digest-pinned images in `tools.env` |

## What would differ, and what removes it

| Source of difference | Removed by |
|---|---|
| Image `created` time and history times | `SOURCE_DATE_EPOCH` = the commit time (`build.sh`) |
| File times in our layers (download times, build times, checkout times) | `rewrite-timestamp=true` on the OCI exporter clamps every time to `SOURCE_DATE_EPOCH` |
| File modes and times in the checkout (umask, clone time) | the context is `git archive HEAD`: git's normalised modes and the commit's time |
| Untracked or modified files in the checkout | the same: only committed files are built (`--worktree` is for development and is labelled `-dirty`) |
| BuildKit's provenance and SBOM attestations (build start and end times, builder details) | `--provenance=false --sbom=false`; CI attests provenance and the SBOM over the digest instead |
| A multi-platform index | one platform, `linux/amd64` (plan 09 A27) |
| Build paths in the binaries (panic locations, debug strings) | `--remap-path-prefix` for `/src` and `/usr/local/cargo`; every build uses the same paths anyway |
| Parallel code generation | `codegen-units = 1` in `[profile.release]` |
| The build CA secret (`LK_BUILD_CA`) | a secret mount: never in a layer, never in the cache key |
| The revision label | the same for both builds of a commit, by definition |

## Known limits

- **The builder.** The layer encoding (tar headers, gzip) is BuildKit's. The claim is "same commit, same BuildKit version,
  same digest". CI pins BuildKit by digest (`LK_BUILDER`, T2); a local build through another Docker version's built-in
  BuildKit can give a different digest for the same files. `oci-diff.py` then reports "same files, metadata and content:
  the difference is in the tar or gzip encoding".
- **Registries.** A base-image digest pins its bytes, but a registry can delete it. The build then fails; it never picks up
  other bytes.
- **The model host.** The same applies to the model: Hugging Face serving different bytes for a pinned commit fails the
  `sha256sum -c` check.

## Measurements

Measured on 2026-10-25 with Docker 29.9.2 (its built-in BuildKit, containerd image store), 4 vCPUs, at commit `8563b6c`
(`SOURCE_DATE_EPOCH=1792910251`). The digest depends on the commit time, so another commit gives another digest; what is
checked is that one commit gives one.

| Image | Builds | Result |
|---|---|---|
| agent | `reproducible-check.sh agent`: two `--no-cache` builds from two `git archive` extractions | identical: `sha256:167548e1a6a654fdc2982d16f1e8a42547edf902be0fe76a7efd2144df9f96a4` |
| agent, again at commit `f02523a` (`SOURCE_DATE_EPOCH=1792911878`) | the same, with an empty build cache (`docker builder prune -af` first) | identical: `sha256:b175553354141423395634222f4ca3a9b4cbeaf83d3bd2287bbcdbebc1aa8e67` |
| hub without the `model` stage | the same two builds of the hub Dockerfile with the model stage and its `COPY` removed (a one-off check, the web app and the binary) | identical: `sha256:2810c71cb39c216b2f6303613747ee93fa5716a6778d64a65d4e133735670035` |
| hub | | not measured: `hub-model.lock` is unresolved (`plans/09-deployment-ci.blockers.md`, B1: the ONNX graph is served from Hugging Face's CDN, which the build environment cannot reach), so the `model` stage refuses to run. The model files are fixed by their sha256, so they cannot add a difference; their file times are rewritten like every other file's |

No differences were found, so none is listed as accepted.
