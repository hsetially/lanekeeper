# syntax=docker.io/docker/dockerfile:1.28.0@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b
#
# The hub image (03b/T1, 09/T1; S16, S19, S20). Build it with deploy/images/build.sh, which sets the timestamps and output
# options that make two builds of one commit produce one digest (deploy/images/REPRODUCIBILITY.md):
#
#     deploy/images/build.sh hub
#
# Stages: `web` builds web/dist; `planner` writes the cargo-chef recipe (the dependency graph without our sources); `build`
# compiles the dependencies from the recipe (a layer that only changes when Cargo.lock or a manifest does) and then the
# hub; `model` fetches the embedding model's files named in deploy/images/hub-model.lock and checks each one's size and
# sha256; the final stage is distroless with those three things alone.
#
# Every base image is pinned by digest (the multi-architecture index), so a build today and a build next year start from
# the same bytes. The digests were read from the registries on 2026-10-25 (node, rust, distroless and the Dockerfile
# frontend 1.28.0):
#
#     docker buildx imagetools inspect gcr.io/distroless/cc-debian13:nonroot      # and the same for each base
#
# Renovate proposes new digests. The web stage's Node is the major in web/.nvmrc, and pnpm is the version in package.json;
# the Rust stages' toolchain is the one in rust-toolchain.toml (1.88.0), and Debian 13 (trixie) on both sides keeps glibc
# the same. The runtime is cc-debian13 because cc-debian12 is deprecated: it carries critical findings that will not be
# fixed, which the image scan refuses (plan 09 A32).
#
# Network steps take an optional `build_ca` secret: a CA bundle for builds behind a TLS-intercepting proxy. It is mounted
# for that one instruction and is never part of a layer. CI does not pass it.

FROM docker.io/library/node:22.20.0-trixie-slim@sha256:f3bca16c4b87ad0c7abfad97f6803898ee9b08a1f153a269140f0a08f50231d2 AS web
# Read only when the build_ca secret is mounted; Node warns and goes on without it.
ENV NODE_EXTRA_CA_CERTS=/run/secrets/build_ca
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
# --frozen-lockfile: exactly the packages in the lockfile, which the supply-chain checks have seen (S19).
RUN --mount=type=secret,id=build_ca,required=false corepack enable && corepack prepare pnpm@10.34.6 --activate && pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

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
# --locked: exactly the versions in Cargo.lock, which `cargo deny` and `cargo audit` have checked (S19).
RUN --mount=type=bind,from=planner,source=/recipe.json,target=/recipe.json \
    --mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false \
    cargo chef cook --release --locked -p hub --recipe-path /recipe.json
COPY . .
RUN cargo build --release --locked -p hub

# The embedding model (D53), fetched from the pinned Hugging Face revision in deploy/images/hub-model.lock. Each file must
# have the lock's size and sha256, or the build stops (S19, S20). The URL is built from the lock's repo, revision and path,
# so the lock cannot point anywhere else; only HTTPS is followed, redirects included.
FROM docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db AS model
RUN --mount=type=bind,source=deploy/images/hub-model.lock,target=/hub-model.lock \
    --mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false \
    set -eu; \
    name="$(awk '$1 == "model" { print $2 }' /hub-model.lock)"; \
    repo="$(awk '$1 == "repo" { print $2 }' /hub-model.lock)"; \
    rev="$(awk '$1 == "revision" { print $2 }' /hub-model.lock)"; \
    echo "$name" | grep -Eqx '[a-z0-9][a-z0-9.-]*'; \
    echo "$repo" | grep -Eqx '[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*'; \
    echo "$rev" | grep -Eqx '[0-9a-f]{40}'; \
    awk '$1 == "file" { print $2, $3, $4 }' /hub-model.lock > /tmp/files; \
    test -s /tmp/files; \
    while read -r sha size path; do \
        echo "$path" | grep -Eqx '([A-Za-z0-9_-][A-Za-z0-9._-]*/)*[A-Za-z0-9_-][A-Za-z0-9._-]*'; \
        out="/models/$name/$path"; \
        mkdir -p "$(dirname "$out")"; \
        curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
            --retry 3 --connect-timeout 30 --max-time 900 --max-filesize "$size" \
            --output "$out" "https://huggingface.co/$repo/resolve/$rev/$path"; \
        test "$(stat -c %s "$out")" = "$size"; \
        echo "$sha  $out" | sha256sum -c -; \
    done < /tmp/files; \
    chmod -R u=rwX,go=rX /models

# Distroless: glibc, libgcc and CA certificates, and no shell, no package manager, nothing to exec into (S16).
# `nonroot` runs as uid and gid 65532.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
LABEL org.opencontainers.image.title="lanekeeper-hub" \
      org.opencontainers.image.description="Lanekeeper hub" \
      org.opencontainers.image.licenses="UNLICENSED"
COPY --from=build /src/target/release/hub /usr/local/bin/hub
COPY --from=web /src/web/dist /app/web/dist
COPY --from=model /models /app/models
USER 65532:65532
# Web (HTTPS), agent and sentinel (gRPC over mTLS), replica-to-replica (gRPC over mTLS), operations (plain HTTP).
EXPOSE 8443 8444 8445 9090
ENTRYPOINT ["/usr/local/bin/hub"]
