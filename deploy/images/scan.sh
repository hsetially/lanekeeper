#!/usr/bin/env bash
# Vulnerability scan of one image (09/T1, S20): grype over the image itself and over its merged SBOM (which adds the Rust
# crates the image scan cannot see). Any finding of critical severity fails the scan, fixed or not.
#
#   deploy/images/scan.sh <hub|agent> [--out DIR]     reads DIR/<image>.oci.tar and DIR/<image>.cdx.json
#   deploy/images/scan.sh --sbom FILE                  scans one SBOM (the planted-critical test uses this)
#   deploy/images/scan.sh --image REF                  scans an image in a registry, by digest: REF is name@sha256:<64 hex digits>, with the
#                                                      registry host, never a tag (the database image of deploy/images/scan-db.sh)
#
# Exceptions live in deploy/images/grype.yaml, each with the vulnerability id, the reason and a review date; a human
# approves every one in review. `--by-cve` reports each finding by its CVE id where it has one (not the advisory's own
# GHSA id), so the output and the exceptions name findings the same way. The vulnerability database is cached in
# target/grype-db between runs.
#
# grype downloads that database from grype.anchore.io. Behind a TLS-intercepting proxy, the same variables as build.sh
# apply: LK_BUILD_NETWORK is passed to `--network` (for example `host`, so the container reaches a proxy on 127.0.0.1),
# LK_BUILD_CA is mounted read-only as the container's CA bundle, and HTTPS_PROXY and NO_PROXY are passed through by name
# when they are set. CI on GitHub-hosted runners sets none of them.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$here/tools.env"
usage() { echo "usage: $0 <hub|agent> [--out DIR] | $0 --sbom FILE | $0 --image REF" >&2; exit 2; }
[ $# -ge 1 ] || usage

cache="$root/target/grype-db"
mkdir -p "$cache"
net=()
[ -z "${LK_BUILD_NETWORK:-}" ] || net+=(--network "$LK_BUILD_NETWORK")
[ -z "${HTTPS_PROXY:-}" ] || net+=(-e HTTPS_PROXY)
[ -z "${NO_PROXY:-}" ] || net+=(-e NO_PROXY)
if [ -n "${LK_BUILD_CA:-}" ]; then
    [ -r "$LK_BUILD_CA" ] || { echo "LK_BUILD_CA is not a readable file: $LK_BUILD_CA" >&2; exit 1; }
    net+=(-v "$LK_BUILD_CA:/etc/ssl/certs/lk-build-ca.crt:ro" -e SSL_CERT_FILE=/etc/ssl/certs/lk-build-ca.crt)
fi
grype() {
    local dir="$1"
    shift
    docker run --rm --user "$(id -u):$(id -g)" "${net[@]}" \
        -e GRYPE_CHECK_FOR_APP_UPDATE=false -e GRYPE_DB_CACHE_DIR=/cache \
        -v "$cache:/cache" -v "$dir:/in:ro" -v "$here/grype.yaml:/grype.yaml:ro" \
        "$LK_GRYPE_IMAGE" --config /grype.yaml --fail-on critical --by-cve --output table "$@"
}

if [ "$1" = "--image" ]; then
    [ $# -eq 2 ] || usage
    # A digest names the bytes; a tag can be moved between the scan and the deploy. Checked before anything runs, because REF becomes an
    # argument of a container: it starts with a letter or a digit, so it cannot be an option, and it has no space or shell character.
    ref="$2"
    ref_pattern='^[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]+)?/[a-z0-9._/-]+@sha256:[0-9a-f]{64}$'
    [[ "$ref" =~ $ref_pattern ]] || { echo "scan.sh: --image takes name@sha256:<64 hex digits> with the registry host (ghcr.io/org/image@sha256:...), never a tag: $ref" >&2; exit 2; }
    grype "$here" "registry:$ref"
    echo "scan.sh: $ref has no critical findings"
    exit
fi

if [ "$1" = "--sbom" ]; then
    [ $# -eq 2 ] || usage
    file="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
    grype "$(dirname "$file")" "sbom:/in/$(basename "$file")"
    exit
fi

image="$1"
shift
case "$image" in hub | agent) ;; *) usage ;; esac
out="$root/target/images"
[ $# -eq 0 ] || { [ $# -eq 2 ] && [ "$1" = "--out" ] || usage; out="$2"; }
out="$(cd "$out" && pwd)"
for f in "$image.oci.tar" "$image.cdx.json"; do
    [ -f "$out/$f" ] || { echo "scan.sh: no $out/$f; run build.sh and sbom.sh first" >&2; exit 1; }
done

status=0
grype "$out" "oci-archive:/in/$image.oci.tar" || status=1
grype "$out" "sbom:/in/$image.cdx.json" || status=1
[ "$status" -eq 0 ] && echo "scan.sh: $image has no critical findings"
exit "$status"
