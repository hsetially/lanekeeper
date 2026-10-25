#!/usr/bin/env bash
# Vulnerability scan of the database image (09/T6, S20). The image is CloudNativePG's (ghcr.io/cloudnative-pg/postgresql), not one that
# images.yml builds, so it has no SBOM and no scan of ours, and the sandbox that chose it could not read its layers (plans/09-
# deployment-ci.blockers.md, B9). This scans, with the grype setup of scan.sh (`--fail-on critical`, the exceptions of grype.yaml), exactly
# the digest that the values files deploy, so a digest bumped in a values file is the digest scanned the same night.
#
#   deploy/images/scan-db.sh                          scan every distinct image of deploy/values/db/*.yaml
#   deploy/images/scan-db.sh --list                   print the images (repository@digest) and scan nothing
#   deploy/images/scan-db.sh --values DIR --chart-values FILE   read other files (the tests use this)
#
# It fails closed: a values file with a tag and no digest, a placeholder or malformed digest, a repository that is not CloudNativePG's,
# or no values file at all is an error, never a scan of nothing.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
values="$root/deploy/values/db"
chart_values="$root/deploy/charts/db/values.yaml"
list=0
usage() { echo "usage: $0 [--list] [--values DIR] [--chart-values FILE]" >&2; exit 2; }
while [ $# -gt 0 ]; do
    case "$1" in
        --list) list=1 ;;
        --values) [ $# -ge 2 ] || usage; values="$2"; shift ;;
        --chart-values) [ $# -ge 2 ] || usage; chart_values="$2"; shift ;;
        *) usage ;;
    esac
    shift
done

# The value of a key inside the top-level `image:` block of a YAML file (quotes and a trailing comment dropped).
image_field() { # FILE KEY
    awk -v key="$2" '
        /^image:/ { inside = 1; next }
        /^[^ #]/ { inside = 0 }
        inside {
            line = $0
            sub(/^ +/, "", line)
            if (index(line, key ":") == 1) {
                value = substr(line, length(key) + 2)
                sub(/ +#.*$/, "", value)
                gsub(/^ +| +$/, "", value)
                gsub(/^"|"$/, "", value)
                print value
                exit
            }
        }
    ' "$1"
}

repository="$(image_field "$chart_values" repository)"
[ -n "$repository" ] || { echo "scan-db.sh: no image repository in $chart_values" >&2; exit 1; }
[ "$repository" = "ghcr.io/cloudnative-pg/postgresql" ] || { echo "scan-db.sh: the repository is $repository, and the database image is ghcr.io/cloudnative-pg/postgresql" >&2; exit 1; }

shopt -s nullglob
files=("$values"/*.yaml)
[ ${#files[@]} -gt 0 ] || { echo "scan-db.sh: no values file in $values, so there is no image to scan" >&2; exit 1; }

refs=()
for f in "${files[@]}"; do
    digest="$(image_field "$f" digest)"
    [ -n "$digest" ] || { echo "scan-db.sh: $f has no digest for the image; a tag is never scanned, because it can move" >&2; exit 1; }
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "scan-db.sh: $f: '$digest' is not a digest (sha256: and 64 lower-case hex digits)" >&2; exit 1; }
    [[ "$digest" != "sha256:0000000000000000000000000000000000000000000000000000000000000000" ]] || { echo "scan-db.sh: $f has the placeholder digest, which names no image" >&2; exit 1; }
    refs+=("$repository@$digest")
done
mapfile -t refs < <(printf '%s\n' "${refs[@]}" | sort -u)

if [ "$list" -eq 1 ]; then
    printf '%s\n' "${refs[@]}"
    exit 0
fi

status=0
for ref in "${refs[@]}"; do
    echo "scan-db.sh: scanning $ref"
    "$here/scan.sh" --image "$ref" || status=1
done
exit "$status"
