#!/usr/bin/env bash
# Proves deploy/images/builder.sh (09/T2; S19, S20): the builder runs the BuildKit pinned in tools.env and says so, it accepts
# a Docker Hub mirror, it refuses a mirror that is not a plain host, and the OCI archive it writes is the same twice only
# because of rewrite-timestamp (a control build without it differs, so this test can tell the two apart).
# The images need no network here: the test image is FROM scratch.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=deploy/images/tools.env
. "$here/../tools.env"
work="$(mktemp -d)"
names=("lk-builder-test-$$-a" "lk-builder-test-$$-b")
cleanup() {
    for n in "${names[@]}"; do docker buildx rm --force "$n" >/dev/null 2>&1 || true; done
    rm -rf "$work"
}
trap cleanup EXIT

fail() { echo "builder-test: $*" >&2; exit 1; }

# 1. The builder starts and reports the pinned version (builder.sh itself fails when it does not).
got="$("$here/../builder.sh" "${names[0]}")"
[ "$got" = "${names[0]}" ] || fail "builder.sh printed '$got', not the builder's name"
version="${LK_BUILDKIT_IMAGE%@*}"
version="${version##*:}"
docker buildx inspect "${names[0]}" | grep -q "BuildKit version: *$version" || fail "the builder does not report BuildKit $version"
echo "builder-test: builder ${names[0]} runs BuildKit $version"

# 2. A mirror is accepted: BuildKit parses the generated config and starts.
LK_DOCKER_HUB_MIRROR="$LK_CI_DOCKER_HUB_MIRROR" "$here/../builder.sh" "${names[1]}" >/dev/null
echo "builder-test: a builder with mirror $LK_CI_DOCKER_HUB_MIRROR starts"

# 3. A mirror that is not a host name is refused before anything is created.
for bad in 'https://mirror.example' 'mirror.example/library' 'x;touch /tmp/lk-builder-test-pwned'; do
    if LK_DOCKER_HUB_MIRROR="$bad" "$here/../builder.sh" "lk-builder-test-$$-c" >/dev/null 2>"$work/err"; then
        fail "builder.sh accepted the mirror '$bad'"
    fi
    grep -q 'LK_DOCKER_HUB_MIRROR' "$work/err" || fail "the refusal of '$bad' does not name LK_DOCKER_HUB_MIRROR: $(cat "$work/err")"
done
[ ! -e /tmp/lk-builder-test-pwned ] || fail "a mirror value was run as a command"
docker buildx inspect "lk-builder-test-$$-c" >/dev/null 2>&1 && fail "a builder was created for a refused mirror"
echo "builder-test: refused mirrors create nothing"

# 4. The same files with other modification times give one archive with rewrite-timestamp, and two without it. The times are
# later than SOURCE_DATE_EPOCH (1700000000): rewrite-timestamp clamps later times down to it, as a build's own files always are,
# and leaves earlier ones alone.
ctx="$work/ctx"
mkdir "$ctx"
printf 'FROM scratch\nCOPY hello /hello\n' > "$ctx/Dockerfile"
printf 'hello\n' > "$ctx/hello"
build() { # DEST MTIME REWRITE(true|false)
    touch -d "@$2" "$ctx/hello"
    docker buildx build --builder "${names[0]}" --no-cache --provenance=false --sbom=false \
        --build-arg SOURCE_DATE_EPOCH=1700000000 \
        --output "type=oci,dest=$work/$1.tar,rewrite-timestamp=$3" --metadata-file "$work/$1.json" "$ctx" >/dev/null
    jq -r '."containerimage.digest"' "$work/$1.json"
}
a="$(build a 1800000000 true)"
b="$(build b 1900000000 true)"
[ "$a" = "$b" ] || fail "with rewrite-timestamp the digests differ: $a and $b"
c="$(build c 1800000000 false)"
d="$(build d 1900000000 false)"
[ "$c" != "$d" ] || fail "without rewrite-timestamp the digests are equal ($c): the control cannot tell reproducible from not"
echo "builder-test: rewrite-timestamp makes $a twice; without it $c and $d differ"
