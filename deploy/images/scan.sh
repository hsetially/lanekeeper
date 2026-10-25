#!/usr/bin/env bash
# Vulnerability scan of one image (09/T1, S20): grype over the image itself and over its merged SBOM (which adds the Rust
# crates the image scan cannot see). Any finding of critical severity fails the scan, fixed or not.
#
#   deploy/images/scan.sh <hub|agent> [--out DIR]     reads DIR/<image>.oci.tar and DIR/<image>.cdx.json
#   deploy/images/scan.sh --sbom FILE                  scans one SBOM (the planted-critical test uses this)
#
# Exceptions live in deploy/images/grype.yaml, each with the vulnerability id, the reason and a review date; a human
# approves every one in review. The vulnerability database is cached in target/grype-db between runs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$here/tools.env"
usage() { echo "usage: $0 <hub|agent> [--out DIR] | $0 --sbom FILE" >&2; exit 2; }
[ $# -ge 1 ] || usage

cache="$root/target/grype-db"
mkdir -p "$cache"
grype() {
    local dir="$1"
    shift
    docker run --rm --user "$(id -u):$(id -g)" \
        -e GRYPE_CHECK_FOR_APP_UPDATE=false -e GRYPE_DB_CACHE_DIR=/cache \
        -v "$cache:/cache" -v "$dir:/in:ro" -v "$here/grype.yaml:/grype.yaml:ro" \
        "$LK_GRYPE_IMAGE" --config /grype.yaml --fail-on critical --output table "$@"
}

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
