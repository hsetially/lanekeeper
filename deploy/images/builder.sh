#!/usr/bin/env bash
# Creates the BuildKit builder that builds the images, and prints its name (09/T2; S19, S20).
#
#   export LK_BUILDER="$(deploy/images/builder.sh)"
#   deploy/images/build.sh hub            # build.sh passes LK_BUILDER to `docker buildx build --builder`
#
# Why: the layer encoding of an image (tar headers, compression) is BuildKit's, so two builds of one commit give one digest only
# for one BuildKit version (REPRODUCIBILITY.md). The builder Docker ships inside its daemon changes with the Docker version, so
# CI and anyone who wants the digest CI publishes use this one instead: a `docker-container` builder that runs the BuildKit image
# pinned by digest in tools.env, checked against the version it reports. An existing builder of that name is removed first.
#
#   deploy/images/builder.sh [NAME]       NAME defaults to lanekeeper-buildkit
#
# Environment:
#   LK_DOCKER_HUB_MIRROR   a registry host that serves Docker Hub's content (CI: mirror.gcr.io). BuildKit then fetches the base
#                          images of the Dockerfiles from it first and from Docker Hub when it fails. Every base image is named by
#                          digest, so the mirror cannot change what is built (see the note in tools.env). Unset: Docker Hub only.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=deploy/images/tools.env
. "$here/tools.env"
name="${1:-lanekeeper-buildkit}"
[[ "$name" =~ ^[a-z0-9][a-z0-9_.-]*$ ]] || { echo "usage: $0 [NAME]  (NAME: lower case letters, digits, . _ -)" >&2; exit 2; }
mirror="${LK_DOCKER_HUB_MIRROR:-}"
if [ -n "$mirror" ] && ! [[ "$mirror" =~ ^[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?$ ]]; then
    echo "builder.sh: LK_DOCKER_HUB_MIRROR must be a plain host name (with an optional port), not '$mirror'" >&2
    exit 2
fi
ref="${LK_BUILDKIT_IMAGE%@*}"
version="${ref##*:}"
[[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "builder.sh: cannot read a BuildKit version from LK_BUILDKIT_IMAGE=$LK_BUILDKIT_IMAGE" >&2; exit 1; }

config="$(mktemp)"
trap 'rm -f "$config"' EXIT
{
    echo "# Written by deploy/images/builder.sh."
    if [ -n "$mirror" ]; then
        printf '[registry."docker.io"]\n  mirrors = ["%s"]\n' "$mirror"
    fi
} > "$config"

docker buildx rm --force "$name" >/dev/null 2>&1 || true
docker buildx create --name "$name" --driver docker-container --driver-opt "image=$LK_BUILDKIT_IMAGE" \
    --buildkitd-config "$config" >/dev/null
reported="$(docker buildx inspect --bootstrap "$name" | sed -n 's/^BuildKit version: *//p' | head -n1)"
if [ "$reported" != "$version" ]; then
    docker buildx rm --force "$name" >/dev/null 2>&1 || true
    echo "builder.sh: the builder reports BuildKit '$reported', the pin is $version" >&2
    exit 1
fi
echo "builder.sh: builder $name runs BuildKit $version${mirror:+ (Docker Hub through $mirror)}" >&2
echo "$name"
