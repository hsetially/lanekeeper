#!/usr/bin/env bash
# What build.sh asks of `docker buildx build` (09/T1, T2; S19, S20), checked with a stand-in `docker` that records its arguments:
# the settings that make a build reproducible are always there, LK_BUILDER selects the pinned builder, and LK_IMAGE_SOURCE becomes
# the label that links a GHCR package to this repository (and is refused unless it is a plain https URL). No build runs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/docker" <<'FAKE'
#!/usr/bin/env bash
# Records the arguments of `docker buildx build`, one per line, and writes the metadata file build.sh then reads.
[ "$1 $2" = "buildx build" ] || { echo "unexpected docker call: $*" >&2; exit 1; }
printf '%s\n' "$@" > "$FAKE_DOCKER_LOG"
while [ $# -gt 0 ]; do
    if [ "$1" = "--metadata-file" ]; then
        printf '{"containerimage.digest":"sha256:%064d"}\n' 7 > "$2"
    fi
    shift
done
FAKE
chmod +x "$work/bin/docker"

fail() { echo "build-args-test: $*" >&2; exit 1; }
build() { # extra environment as VAR=value arguments
    : > "$work/log"
    env "$@" PATH="$work/bin:$PATH" FAKE_DOCKER_LOG="$work/log" "$here/../build.sh" agent --out "$work/out" >"$work/stdout" 2>"$work/stderr"
}
has() { grep -qxF -- "$1" "$work/log" || fail "docker was not given: $1 (got: $(tr '\n' ' ' < "$work/log"))"; }
lacks() { ! grep -qE -- "$1" "$work/log" || fail "docker was given an argument that matches: $1"; }

# The settings that make the digest reproducible are always passed.
build LK_BUILDER= LK_IMAGE_SOURCE= LK_BUILD_NETWORK= LK_BUILD_CA= HTTPS_PROXY= NO_PROXY=
has --provenance=false
has --sbom=false
has linux/amd64
grep -qE '^type=oci,dest=.*agent\.oci\.tar,rewrite-timestamp=true$' "$work/log" || fail "the OCI exporter does not rewrite timestamps"
grep -qE '^SOURCE_DATE_EPOCH=[0-9]+$' "$work/log" || fail "SOURCE_DATE_EPOCH is not a build argument"
lacks '^--builder$'
lacks '^org\.opencontainers\.image\.source='
[ "$(cat "$work/out/agent.digest")" = "sha256:$(printf '%064d' 7)" ] || fail "build.sh did not record the digest from the metadata file"

# LK_BUILDER selects the builder.
build LK_BUILDER=lk-pinned LK_IMAGE_SOURCE= LK_BUILD_NETWORK= LK_BUILD_CA= HTTPS_PROXY= NO_PROXY=
has --builder
has lk-pinned

# LK_IMAGE_SOURCE becomes the label that links the package to the repository.
build LK_BUILDER= LK_IMAGE_SOURCE=https://github.com/example/lanekeeper LK_BUILD_NETWORK= LK_BUILD_CA= HTTPS_PROXY= NO_PROXY=
has org.opencontainers.image.source=https://github.com/example/lanekeeper

# ...and is refused unless it is a plain https URL: no other scheme, no spaces, no line breaks.
for bad in 'http://github.com/example/lanekeeper' 'git@github.com:example/lanekeeper' 'https://github.com/example/lanekeeper --build-arg X=1' "$(printf 'https://example.com/x\nFROM evil')"; do
    : > "$work/log"
    if build LK_BUILDER= "LK_IMAGE_SOURCE=$bad" LK_BUILD_NETWORK= LK_BUILD_CA= HTTPS_PROXY= NO_PROXY=; then
        fail "build.sh accepted LK_IMAGE_SOURCE=$bad"
    fi
    grep -q 'LK_IMAGE_SOURCE' "$work/stderr" || fail "the refusal does not name LK_IMAGE_SOURCE: $(cat "$work/stderr")"
    [ ! -s "$work/log" ] || fail "docker was called for a refused LK_IMAGE_SOURCE"
done
echo "build-args-test: reproducibility settings, LK_BUILDER and LK_IMAGE_SOURCE are passed as described"
