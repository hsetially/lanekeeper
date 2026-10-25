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
| The proxy of a build behind an egress proxy (`HTTPS_PROXY`, `NO_PROXY`) | passed as BuildKit's predefined build args, which are kept out of the image history and config (checked in the hub image's config); CI sets none |
| The revision label | the same for both builds of a commit, by definition |

## Known limits

- **The builder.** The layer encoding (tar headers, gzip) is BuildKit's. The claim is "same commit, same BuildKit version,
  same digest". CI pins BuildKit by digest (`LK_BUILDER`, T2); a local build through another Docker version's built-in
  BuildKit can give a different digest for the same files. `oci-diff.py` then reports "same files, metadata and content:
  the difference is in the tar or gzip encoding".
- **Registries.** A base-image digest pins its bytes, but a registry can delete it. The build then fails; it never picks up
  other bytes.
  Docker Hub also limits anonymous pulls per IP address (100 manifest requests an hour); an exhausted limit fails a build with
  `429 Too Many Requests`, never with other bytes.
- **The model host.** The same applies to the model: Hugging Face serving different bytes for a pinned commit fails the
  `sha256sum -c` check.

## Measurements

Measured on 2026-10-25 with Docker 29.9.2 (its built-in BuildKit, containerd image store) on 4 vCPUs, after the move to
Debian 13 (builder `rust:1.88.0-trixie`, runtime `gcr.io/distroless/cc-debian13:nonroot`), at commit `e2c199d`
(`SOURCE_DATE_EPOCH=1792949090`), with the build cache emptied first (`docker builder prune -af`) and the settings of a
build behind this environment's egress proxy (`LK_BUILD_CA`, `LK_BUILD_NETWORK=host`, `HTTPS_PROXY` and `NO_PROXY`). The digest
depends on the commit time, so another commit gives another digest; what is checked is that one commit gives one.

| Image | Builds | Result |
|---|---|---|
| hub | `reproducible-check.sh hub`: two `--no-cache` builds from two `git archive` extractions, including the `model` stage (the five files of `hub-model.lock` downloaded from Hugging Face and checked) | identical: `sha256:0117df0be05cd1a3fb6d49b318000dd81544a7a2bbee7a89f92da7c24a5c0e85`; the two OCI archives are byte for byte equal (`cmp`) |
| agent | `reproducible-check.sh agent`: the same | identical: `sha256:35d95ba845bd7398cb427553ecd6a0590c49dbde0cd0cf71d3d0d62ee3c4750c`; the two OCI archives are byte for byte equal (`cmp`) |

The whole run (`just images-repro`, four builds) took 22 minutes. No differences were found, so none is listed as accepted.
The earlier measurements on the Debian 12 images (agent at `8563b6c` and `f02523a`, hub without its model stage) also gave
identical digests; they are superseded by the table above, because the base and the builder changed.
