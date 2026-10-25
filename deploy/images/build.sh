#!/usr/bin/env bash
# Builds one Lanekeeper image as an OCI archive and prints its digest (09/T1; S19, S20).
#
#   deploy/images/build.sh <hub|agent> [--out DIR] [--no-cache] [--worktree]
#
# By default the build context is `git archive HEAD`: exactly the committed files, with the commit's own timestamps and
# git's normalised modes, so two builds of one commit see the same input whatever else is in the checkout. `--worktree`
# builds the working tree instead (the revision label then ends in `-dirty`); that build is for development, not release.
#
# What makes the output reproducible (deploy/images/REPRODUCIBILITY.md):
#   - SOURCE_DATE_EPOCH is the commit time, and `rewrite-timestamp=true` clamps every file time in our layers to it;
#   - BuildKit's own provenance and SBOM attestations are off (they carry build times and the builder's identity); CI
#     attests provenance and the SBOM separately, over this digest;
#   - one platform, linux/amd64 (plan 09 A27), so the result is a single manifest, not an index.
#
# Output in DIR (default target/images): <image>.oci.tar, <image>.metadata.json and <image>.digest.
#
# Environment:
#   LK_BUILDER        a buildx builder to use (CI sets one up with a pinned BuildKit); the current builder otherwise.
#   LK_BUILD_CA       a CA bundle for builds behind a TLS-intercepting proxy. It is passed as the `build_ca` secret, so
#                     it never reaches a layer or the cache key.
#   LK_BUILD_NETWORK  passed to `--network` (for example `host`), when the build containers need it to reach the network.
#   HTTPS_PROXY, NO_PROXY
#                     when set, passed to the build as the predefined build args of the same names (BuildKit keeps them
#                     out of the image history). Behind an egress proxy the build containers cannot resolve hosts such as
#                     huggingface.co themselves, so the model stage needs them; with `host` as the network they reach a proxy
#                     on 127.0.0.1. CI on GitHub-hosted runners sets none of these.
set -euo pipefail

usage() { echo "usage: $0 <hub|agent> [--out DIR] [--no-cache] [--worktree]" >&2; exit 2; }
[ $# -ge 1 ] || usage
image="$1"
shift
case "$image" in hub | agent) ;; *) usage ;; esac

root="$(git rev-parse --show-toplevel)"
out="$root/target/images"
no_cache=""
worktree=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || usage; out="$2"; shift 2 ;;
        --no-cache) no_cache=1; shift ;;
        --worktree) worktree=1; shift ;;
        *) usage ;;
    esac
done
mkdir -p "$out"
out="$(cd "$out" && pwd)"

revision="$(git -C "$root" rev-parse HEAD)"
epoch="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct HEAD)}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ -n "$worktree" ]; then
    context="$root"
    [ -z "$(git -C "$root" status --porcelain)" ] || revision="$revision-dirty"
else
    context="$work/context"
    mkdir "$context"
    git -C "$root" archive --format=tar HEAD | tar -x -C "$context"
fi

args=(
    --file "$context/crates/$image/Dockerfile"
    --platform linux/amd64
    --provenance=false
    --sbom=false
    --build-arg "SOURCE_DATE_EPOCH=$epoch"
    --label "org.opencontainers.image.revision=$revision"
    --output "type=oci,dest=$out/$image.oci.tar,rewrite-timestamp=true"
    --metadata-file "$out/$image.metadata.json"
)
[ -z "$no_cache" ] || args+=(--no-cache)
[ -z "${LK_BUILDER:-}" ] || args+=(--builder "$LK_BUILDER")
[ -z "${LK_BUILD_NETWORK:-}" ] || args+=(--network "$LK_BUILD_NETWORK")
[ -z "${HTTPS_PROXY:-}" ] || args+=(--build-arg "HTTPS_PROXY=$HTTPS_PROXY")
[ -z "${NO_PROXY:-}" ] || args+=(--build-arg "NO_PROXY=$NO_PROXY")
if [ -n "${LK_BUILD_CA:-}" ]; then
    [ -r "$LK_BUILD_CA" ] || { echo "LK_BUILD_CA is not a readable file: $LK_BUILD_CA" >&2; exit 1; }
    args+=(--secret "id=build_ca,src=$LK_BUILD_CA")
fi

echo "build.sh: $image at $revision (SOURCE_DATE_EPOCH=$epoch)" >&2
docker buildx build "${args[@]}" "$context"
digest="$(jq -r '."containerimage.digest" // empty' "$out/$image.metadata.json")"
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "build.sh: no image digest in $out/$image.metadata.json" >&2; exit 1; }
echo "$digest" > "$out/$image.digest"
echo "$digest"
