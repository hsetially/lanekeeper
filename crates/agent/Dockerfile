# syntax=docker.io/docker/dockerfile:1.28.0@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b
#
# The agent image (02/T7, 09/T1; S16, S19, S20). Build it with deploy/images/build.sh, which sets the timestamps and output
# options that make two builds of one commit produce one digest (deploy/images/REPRODUCIBILITY.md):
#
#     deploy/images/build.sh agent
#
# Stages: `planner` writes the cargo-chef recipe (the dependency graph without our sources), `build` compiles the
# dependencies from the recipe (a layer that only changes when Cargo.lock or a manifest does) and then the agent, and the
# final stage is distroless with the binary alone.
#
# Every base image is pinned by digest (the multi-architecture index), so a build today and a build next year start from
# the same bytes. The digests were read from the registries on 2026-10-25 (rust, distroless and the Dockerfile frontend
# 1.28.0):
#
#     docker buildx imagetools inspect docker.io/library/rust:1.88.0-trixie
#     docker buildx imagetools inspect gcr.io/distroless/cc-debian13:nonroot
#
# Renovate proposes new digests. The builder's Rust is the toolchain in rust-toolchain.toml (1.88.0), and Debian 13
# (trixie) on both sides keeps the glibc of the build and the run the same. The runtime is cc-debian13 because
# cc-debian12 is deprecated: it carries critical findings that will not be fixed, which the image scan refuses (plan 09
# A32).
#
# Network steps take an optional `build_ca` secret: a CA bundle for builds behind a TLS-intercepting proxy. It is mounted
# over the system bundle for that one instruction and is never part of a layer. CI does not pass it.

FROM docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db AS planner
# The toolchain the image ships, never one rustup would fetch for rust-toolchain.toml (whose components, rustfmt and clippy,
# the build does not need).
ENV RUSTUP_TOOLCHAIN=1.88.0 CARGO_NET_RETRY=3
# cargo-chef 0.1.78 (2026-08-12), from crates.io, with its own lockfile.
RUN --mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false \
    cargo install --locked cargo-chef --version 0.1.78
WORKDIR /src
COPY . .
RUN cargo chef prepare --recipe-path /recipe.json

FROM docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db AS build
ENV RUSTUP_TOOLCHAIN=1.88.0 CARGO_NET_RETRY=3
RUN --mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false \
    cargo install --locked cargo-chef --version 0.1.78
# The C compiler that the tree-sitter grammar and aws-lc-sys are built with. The build fails here, not halfway, without it.
RUN cc --version
# Symbols are stripped by cargo (there is no debugger in the final image to use them), and the build paths are rewritten,
# so the binary names neither the build directory nor the Cargo home.
ENV CARGO_PROFILE_RELEASE_STRIP=symbols \
    RUSTFLAGS="--remap-path-prefix=/src=. --remap-path-prefix=/usr/local/cargo=/cargo"
WORKDIR /src
# --locked: exactly the versions in Cargo.lock, which `cargo deny` and `cargo audit` have checked (S19). The recipe is bind
# mounted, so the binary is the only file this image copies from another stage.
RUN --mount=type=bind,from=planner,source=/recipe.json,target=/recipe.json \
    --mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false \
    cargo chef cook --release --locked -p agent --recipe-path /recipe.json
COPY . .
RUN cargo build --release --locked -p agent

# Distroless: glibc, libgcc and CA certificates, and no shell, no package manager, nothing to exec into (S16, S17).
# `nonroot` runs as uid and gid 65532 unless the pod says otherwise; the chart sets the uid and gid that own the NFS files
# (Q11) with runAsUser and runAsGroup.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
LABEL org.opencontainers.image.title="lanekeeper-agent" \
      org.opencontainers.image.description="Lanekeeper in-cluster agent" \
      org.opencontainers.image.licenses="UNLICENSED"
COPY --from=build /src/target/release/agent /usr/local/bin/agent
USER 65532:65532
# /healthz, /readyz and /metrics (LK_HEALTH_ADDR).
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/agent"]
