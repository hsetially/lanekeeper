#!/usr/bin/env bash
# Proves that the scan of the database image (09/T6, S20) scans the image the values deploy, by digest, and fails closed: a tag, a missing
# or placeholder digest, a short digest, an option dressed as an image and a scan with nothing to scan are all refused before any
# image is pulled. It needs no Docker and no network: everything here stops before grype runs. The scan itself is the nightly job
# `db-image` (and `just images-scan-db`), which needs both.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
scan_db="$here/../scan-db.sh"
scan="$here/../scan.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

fail() {
    echo "scan-db-test: $*" >&2
    failures=$((failures + 1))
}

ok() { echo "  ok  $*"; }

# expect_refusal NAME FRAGMENT COMMAND...: the command must exit non-zero and say FRAGMENT.
expect_refusal() {
    local name="$1" fragment="$2" out
    shift 2
    if out="$("$@" 2>&1)"; then
        fail "$name: accepted"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment': $out"
    else
        ok "$name"
    fi
}

digest_a="sha256:$(printf 'a%.0s' {1..64})"
digest_b="sha256:$(printf 'b%.0s' {1..64})"
chart_values="$work/chart-values.yaml"
printf 'image:\n  repository: ghcr.io/cloudnative-pg/postgresql\n  tag: ""\n  digest: ""\n' >"$chart_values"

values_dir() { # NAME DIGEST: a directory with one values file that has the image digest DIGEST
    mkdir -p "$work/$1"
    printf 'image:\n  tag: "16.15"\n  digest: "%s"\nenvironment: prod\n' "$2" >"$work/$1/prod.yaml"
}

echo "-- the refs come from the values that are deployed"
refs="$("$scan_db" --list)"
prod_digest="$(grep -o 'sha256:[0-9a-f]\{64\}' "$root/deploy/values/db/prod.yaml" | head -n1)"
if [ "$(wc -l <<<"$refs")" -ne 1 ]; then
    fail "the committed values should give one image (kind, staging and prod run the same one), and give: $refs"
elif [ "$refs" != "ghcr.io/cloudnative-pg/postgresql@$prod_digest" ]; then
    fail "the committed values give '$refs', and prod deploys ghcr.io/cloudnative-pg/postgresql@$prod_digest"
else
    ok "the committed values give ghcr.io/cloudnative-pg/postgresql@$prod_digest"
fi

values_dir two "$digest_a"
printf 'image:\n  digest: "%s"\n' "$digest_b" >"$work/two/staging.yaml"
printf 'image:\n  digest: "%s"\n' "$digest_a" >"$work/two/kind.yaml"
got="$("$scan_db" --list --values "$work/two" --chart-values "$chart_values")"
want="$(printf 'ghcr.io/cloudnative-pg/postgresql@%s\nghcr.io/cloudnative-pg/postgresql@%s' "$digest_a" "$digest_b")"
if [ "$got" = "$want" ]; then ok "two digests give two images, and a digest used twice is scanned once"; else fail "two digests gave: $got"; fi

echo "-- scan-db.sh fails closed"
mkdir "$work/empty"
expect_refusal "no values file" "no values file" "$scan_db" --list --values "$work/empty" --chart-values "$chart_values"
mkdir -p "$work/notag"
printf 'image:\n  tag: "16.15"\nenvironment: prod\n' >"$work/notag/prod.yaml"
expect_refusal "a tag and no digest" "no digest" "$scan_db" --list --values "$work/notag" --chart-values "$chart_values"
values_dir placeholder "sha256:$(printf '0%.0s' {1..64})"
expect_refusal "the placeholder digest" "placeholder" "$scan_db" --list --values "$work/placeholder" --chart-values "$chart_values"
values_dir short "sha256:abc123"
expect_refusal "a short digest" "not a digest" "$scan_db" --list --values "$work/short" --chart-values "$chart_values"
values_dir upper "sha256:$(printf 'A%.0s' {1..64})"
expect_refusal "an upper-case digest" "not a digest" "$scan_db" --list --values "$work/upper" --chart-values "$chart_values"
printf 'image:\n  tag: ""\n' >"$work/norepo.yaml"
expect_refusal "no repository in the chart's values" "no image repository" "$scan_db" --list --values "$work/two" --chart-values "$work/norepo.yaml"
printf 'image:\n  repository: docker.io/library/postgres\n' >"$work/other-repo.yaml"
expect_refusal "an image of another project" "ghcr.io/cloudnative-pg/postgresql" "$scan_db" --list --values "$work/two" --chart-values "$work/other-repo.yaml"
expect_refusal "an unknown option" "usage" "$scan_db" --scan-everything

echo "-- scan.sh --image takes an image by digest and nothing else"
ref="ghcr.io/cloudnative-pg/postgresql"
expect_refusal "a tag" "name@sha256" "$scan" --image "$ref:16.15"
expect_refusal "a tag and a digest" "name@sha256" "$scan" --image "$ref:16.15@$digest_a"
expect_refusal "no digest" "name@sha256" "$scan" --image "$ref"
expect_refusal "a short digest" "name@sha256" "$scan" --image "$ref@sha256:abc123"
expect_refusal "a sha512 digest" "name@sha256" "$scan" --image "$ref@sha512:$(printf 'a%.0s' {1..64})"
expect_refusal "no registry" "name@sha256" "$scan" --image "postgres@$digest_a"
expect_refusal "an option as the image" "name@sha256" "$scan" --image "--output=json"
expect_refusal "a command in the image" "name@sha256" "$scan" --image "$ref@$digest_a;id"
expect_refusal "a space in the image" "name@sha256" "$scan" --image "$ref@$digest_a --verbose"
expect_refusal "an upper-case registry" "name@sha256" "$scan" --image "GHCR.io/x/y@$digest_a"
expect_refusal "no image at all" "usage" "$scan" --image
expect_refusal "two images" "usage" "$scan" --image "$ref@$digest_a" "$ref@$digest_b"

if [ "$failures" -ne 0 ]; then
    echo "scan-db-test: $failures case(s) failed" >&2
    exit 1
fi
echo "scan-db-test: the database image is scanned by the digest the values deploy, and every other form is refused"
