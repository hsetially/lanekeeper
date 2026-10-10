# The agent image (T7, S16). Build from the repository root:
#
#     docker build -f crates/agent/Dockerfile -t ghcr.io/<org>/lanekeeper-agent:<tag> .
#
# Both base images are pinned by digest (the multi-architecture index), so a build today and a build next year start from
# the same bytes. The digests were read from the registries on 2026-10-10:
#
#     curl -sI -H 'Accept: application/vnd.oci.image.index.v1+json' \
#         https://gcr.io/v2/distroless/cc-debian12/manifests/nonroot      # docker-content-digest
#     (docker.io/library/rust:1.88.0-bookworm: the same call on registry-1.docker.io, with a pull token)
#
# Renovate proposes new digests. The builder's Rust is the toolchain in rust-toolchain.toml (1.88.0), and Debian 12 on
# both sides keeps the glibc of the build and the run the same.

FROM docker.io/library/rust:1.88.0-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS build
WORKDIR /src
COPY . .
# --locked: exactly the versions in Cargo.lock, which `cargo deny` and `cargo audit` have checked. Symbols are stripped
# by cargo; there is no debugger in the final image to use them.
ENV CARGO_PROFILE_RELEASE_STRIP=symbols
RUN cargo build --release --locked -p agent

# Distroless: glibc, libgcc and CA certificates, and no shell, no package manager, nothing to exec into (S16, S17).
# `nonroot` runs as uid and gid 65532 unless the pod says otherwise; the chart sets the uid and gid that own the NFS files
# (Q11) with runAsUser and runAsGroup.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
LABEL org.opencontainers.image.title="lanekeeper-agent" \
      org.opencontainers.image.description="Lanekeeper in-cluster agent" \
      org.opencontainers.image.licenses="UNLICENSED"
COPY --from=build /src/target/release/agent /usr/local/bin/agent
USER 65532:65532
# /healthz, /readyz and /metrics (LK_HEALTH_ADDR).
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/agent"]
