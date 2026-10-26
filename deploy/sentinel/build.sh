#!/usr/bin/env bash
# Builds the sentinel binary and its (unsigned) RPM with deploy/sentinel/Dockerfile.build (09/T8; S19, S20, S25).
#
#   deploy/sentinel/build.sh [--out DIR] [--no-cache] [--worktree]
#       Writes DIR/lanekeeper-sentinel (the static musl binary), DIR/lanekeeper-sentinel-<version>-1.x86_64.rpm and DIR/SHA256SUMS
#       (default DIR: target/sentinel), and prints the RPM's sha256.
#   deploy/sentinel/build.sh --tools [--tag NAME]
#       Builds the `rpm` stage as a local image (default tag lanekeeper-rpm-tools) and prints the tag: rpm 4.18 and gpg, which sign.sh
#       and the tests run to sign and to build the stand-in packages. It holds no key and nothing of ours but the build tree of the RPM.
#
# By default the build context is `git archive HEAD`: exactly the committed files, with the commit's own timestamps and git's normalised
# modes, so two builds of one commit see the same input whatever else is in the checkout. `--worktree` builds the working tree instead;
# that build is for development, not release.
#
# What makes the output reproducible (tests/reproducible.sh checks it): SOURCE_DATE_EPOCH is the commit time, and the spec makes it the
# RPM's build time and the ceiling of every file time in it; the binary is built with --locked, stripped, with the build paths rewritten.
#
# Environment:
#   LK_BUILDER        a buildx builder to use (CI creates one with deploy/images/builder.sh, which runs the BuildKit pinned in tools.env).
#   LK_BUILD_CA       a CA bundle for builds behind a TLS-intercepting proxy, passed as the `build_ca` secret (never in a layer).
#   LK_BUILD_NETWORK  passed to `--network` (for example `host`), when the build containers need it to reach the network.
#   LK_APT_MIRROR     the Ubuntu archive for the rpm stage (default http://archive.ubuntu.com/ubuntu); https for a build behind such a proxy.
#   HTTPS_PROXY, NO_PROXY
#                     when set, passed as the predefined build args of the same names (BuildKit keeps them out of the image history).
#                     CI on GitHub-hosted runners sets none of these.
set -euo pipefail

usage() { echo "usage: $0 [--out DIR] [--no-cache] [--worktree] | --tools [--tag NAME]" >&2; exit 2; }

root="$(git rev-parse --show-toplevel)"
out="$root/target/sentinel"
no_cache=""
worktree=""
tools=""
tag="lanekeeper-rpm-tools"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || usage; out="$2"; shift 2 ;;
        --no-cache) no_cache=1; shift ;;
        --worktree) worktree=1; shift ;;
        --tools) tools=1; shift ;;
        --tag) [ $# -ge 2 ] || usage; tag="$2"; shift 2 ;;
        *) usage ;;
    esac
done
[[ "$tag" =~ ^[a-z0-9][a-z0-9_.-]*$ ]] || { echo "build.sh: --tag must be a lower case image name, not '$tag'" >&2; exit 2; }
if [ -n "${LK_APT_MIRROR:-}" ] && ! [[ "$LK_APT_MIRROR" =~ ^https?://[A-Za-z0-9._~/:-]+$ ]]; then
    echo "build.sh: LK_APT_MIRROR must be a plain http or https URL, not '$LK_APT_MIRROR'" >&2
    exit 2
fi

# The crate's version is the package's: x.y.z, no pre-release part (an RPM version has no '-').
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/crates/sentinel/Cargo.toml" | head -n1)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "build.sh: the version in crates/sentinel/Cargo.toml is '$version', not x.y.z" >&2; exit 1; }
epoch="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct HEAD)}"
[[ "$epoch" =~ ^[0-9]+$ ]] || { echo "build.sh: SOURCE_DATE_EPOCH '$epoch' is not a number" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ -n "$worktree" ]; then
    context="$root"
else
    context="$work/context"
    mkdir "$context"
    git -C "$root" archive --format=tar HEAD | tar -x -C "$context"
fi

args=(
    --file "$context/deploy/sentinel/Dockerfile.build"
    --platform linux/amd64
    --provenance=false
    --sbom=false
    --build-arg "SOURCE_DATE_EPOCH=$epoch"
    --build-arg "LK_VERSION=$version"
)
[ -z "$no_cache" ] || args+=(--no-cache)
[ -z "${LK_BUILDER:-}" ] || args+=(--builder "$LK_BUILDER")
[ -z "${LK_BUILD_NETWORK:-}" ] || args+=(--network "$LK_BUILD_NETWORK")
[ -z "${LK_APT_MIRROR:-}" ] || args+=(--build-arg "LK_APT_MIRROR=$LK_APT_MIRROR")
[ -z "${HTTPS_PROXY:-}" ] || args+=(--build-arg "HTTPS_PROXY=$HTTPS_PROXY")
[ -z "${NO_PROXY:-}" ] || args+=(--build-arg "NO_PROXY=$NO_PROXY")
if [ -n "${LK_BUILD_CA:-}" ]; then
    [ -r "$LK_BUILD_CA" ] || { echo "LK_BUILD_CA is not a readable file: $LK_BUILD_CA" >&2; exit 1; }
    args+=(--secret "id=build_ca,src=$LK_BUILD_CA")
fi

if [ -n "$tools" ]; then
    echo "build.sh: the rpm tools image $tag" >&2
    docker buildx build "${args[@]}" --target rpm --load --tag "$tag" "$context" >&2
    echo "$tag"
    exit 0
fi

rm -rf "$out"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
echo "build.sh: lanekeeper-sentinel $version at $(git -C "$root" rev-parse HEAD) (SOURCE_DATE_EPOCH=$epoch)" >&2
docker buildx build "${args[@]}" --target out --output "type=local,dest=$out" "$context" >&2

rpm_file="$(find "$out" -maxdepth 1 -name 'lanekeeper-sentinel-*.rpm' | sort)"
[ "$(wc -l <<<"$rpm_file")" = 1 ] && [ -f "$rpm_file" ] || { echo "build.sh: expected exactly one RPM in $out, found: $rpm_file" >&2; exit 1; }
[ "$(basename "$rpm_file")" = "lanekeeper-sentinel-$version-1.x86_64.rpm" ] || { echo "build.sh: unexpected RPM name $(basename "$rpm_file")" >&2; exit 1; }
[ -x "$out/lanekeeper-sentinel" ] || { echo "build.sh: no executable $out/lanekeeper-sentinel" >&2; exit 1; }
(cd "$out" && sha256sum lanekeeper-sentinel "$(basename "$rpm_file")" > SHA256SUMS)
sha256sum "$rpm_file" | cut -d' ' -f1
