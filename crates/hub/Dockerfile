# The hub image (T1, S16). Build from the repository root:
#
#     docker build -f crates/hub/Dockerfile -t ghcr.io/<org>/lanekeeper-hub:<tag> .
#
# Three stages, every base image pinned by digest (the multi-architecture index), so a build today and a build next year
# start from the same bytes. The digests were read from the registries on 2026-10-11:
#
#     curl -sI -H 'Accept: application/vnd.oci.image.index.v1+json' \
#         https://gcr.io/v2/distroless/cc-debian12/manifests/nonroot      # docker-content-digest
#     (docker.io/library/node and docker.io/library/rust: the same call on registry-1.docker.io, with a pull token)
#
# Renovate proposes new digests. The web stage's Node is the major in web/.nvmrc, and pnpm is the version in package.json;
# the Rust stage's toolchain is the one in rust-toolchain.toml (1.88.0), and Debian 12 on both sides keeps glibc the same.

FROM docker.io/library/node:22.20.0-bookworm-slim@sha256:b21fe589dfbe5cc39365d0544b9be3f1f33f55f3c86c87a76ff65a02f8f5848e AS web
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
# --frozen-lockfile: exactly the packages in the lockfile, which the supply-chain checks have seen (S19).
RUN corepack enable && corepack prepare pnpm@10.34.6 --activate && pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM docker.io/library/rust:1.88.0-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS build
WORKDIR /src
COPY . .
# --locked: exactly the versions in Cargo.lock, which `cargo deny` and `cargo audit` have checked. Symbols are stripped
# by cargo; there is no debugger in the final image to use them.
ENV CARGO_PROFILE_RELEASE_STRIP=symbols
RUN cargo build --release --locked -p hub

# Distroless: glibc, libgcc and CA certificates, and no shell, no package manager, nothing to exec into (S16).
# `nonroot` runs as uid and gid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
LABEL org.opencontainers.image.title="lanekeeper-hub" \
      org.opencontainers.image.description="Lanekeeper hub" \
      org.opencontainers.image.licenses="UNLICENSED"
COPY --from=build /src/target/release/hub /usr/local/bin/hub
COPY --from=web /src/web/dist /app/web/dist
USER 65532:65532
# Web (HTTPS), agent and sentinel (gRPC over mTLS), replica-to-replica (gRPC over mTLS), operations (plain HTTP).
EXPOSE 8443 8444 8445 9090
ENTRYPOINT ["/usr/local/bin/hub"]
