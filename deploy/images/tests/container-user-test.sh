#!/usr/bin/env bash
# The tool containers of the image scripts work for an unprivileged user, and say why when they do not (09, CI first run; S20).
#
#   deploy/images/tests/container-user-test.sh
#
# The first CI run failed three ways that a root shell cannot show: grype and syft, started with `--user "$(id -u):$(id -g)"` as the runner
# (uid 1001), could not write the /tmp of their image (root's, and not writable by anyone else), and `sbom.sh` ran syft with --quiet, so
# the step ended with exit code 1 and no message. This test runs the syft helper of container.sh (the one `sbom.sh` of the images and of the
# sentinel call) as such a user, against an OCI archive made here and a directory, without a network for the second, and checks the output,
# that the user owns it, that nothing is left in the temporary directory, and that a failure shows the tool's own message and a reason
# line. scan-test.sh is the same test for grype, and sign-test.sh for the container that signs. Started as root it steps down first
# (deploy/tests/as-user.sh); syft's image (about 100 MB) is the only thing it pulls.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ "$(id -u)" -ne 0 ] || exec "$here/../../tests/as-user.sh" "${BASH_SOURCE[0]}" "$@"
# shellcheck source=deploy/images/tools.env
. "$here/../tools.env"
# shellcheck source=deploy/images/container.sh
. "$here/../container.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# The scratch directories that lk_container_run makes are made here, so that the test can see whether any is left.
export TMPDIR="$work/tmp"
mkdir "$TMPDIR" "$work/in" "$work/dir" "$work/out" "$work/bad"
fails=0
ok() { echo "  ok  $1"; }
bad() { echo "  FAILED  $1" >&2; fails=$((fails + 1)); }
check() { # NAME COMMAND...
    local name="$1"
    shift
    if "$@" >"$work/last.log" 2>&1; then ok "$name"; else cat "$work/last.log" >&2; bad "$name"; fi
}
owned_by_me() { [ "$(stat -c %u "$1")" = "$(id -u)" ]; }
is_cyclonedx() { jq -e '.bomFormat == "CycloneDX"' "$1" >/dev/null; }
nothing_left() { [ -z "$(ls -A "$TMPDIR")" ]; }

# An OCI image of one layer holding one file, written by hand, so that no builder and no registry is needed.
python3 -I - "$work/in/image.oci.tar" <<'PY'
import hashlib, io, json, sys, tarfile

def blob(data):
    return hashlib.sha256(data).hexdigest(), data

def tar_of(members):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode='w') as t:
        for name, content in members:
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mtime = 0
            t.addfile(info, io.BytesIO(content))
    return buf.getvalue()

layer_digest, layer = blob(tar_of([('hello', b'hello\n')]))
config_digest, config = blob(json.dumps({'architecture': 'amd64', 'os': 'linux', 'config': {},
                                          'rootfs': {'type': 'layers', 'diff_ids': ['sha256:' + layer_digest]}}).encode())
manifest_digest, manifest = blob(json.dumps({
    'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.manifest.v1+json',
    'config': {'mediaType': 'application/vnd.oci.image.config.v1+json', 'digest': 'sha256:' + config_digest, 'size': len(config)},
    'layers': [{'mediaType': 'application/vnd.oci.image.layer.v1.tar', 'digest': 'sha256:' + layer_digest, 'size': len(layer)}],
}).encode())
index = json.dumps({'schemaVersion': 2, 'manifests': [{
    'mediaType': 'application/vnd.oci.image.manifest.v1+json', 'digest': 'sha256:' + manifest_digest, 'size': len(manifest),
    'platform': {'architecture': 'amd64', 'os': 'linux'}}]}).encode()
files = [('oci-layout', b'{"imageLayoutVersion":"1.0.0"}'), ('index.json', index)]
files += [('blobs/sha256/' + d, b) for d, b in (blob(layer), blob(config), blob(manifest))]
with tarfile.open(sys.argv[1], 'w') as t:
    for name, content in files:
        info = tarfile.TarInfo(name)
        info.size = len(content)
        info.mtime = 0
        t.addfile(info, io.BytesIO(content))
PY
printf 'not an image\n' >"$work/bad/image.oci.tar"
echo hello >"$work/dir/file.txt"

echo "== syft, run as uid $(id -u) (a CI runner's user, not root)"
check "an OCI archive: syft writes its SBOM" lk_syft "$work/in" "oci-archive:/in/image.oci.tar" "$work/out" image.cdx.json
check "the SBOM is CycloneDX" is_cyclonedx "$work/out/image.cdx.json"
check "the SBOM belongs to the user who ran it" owned_by_me "$work/out/image.cdx.json"
check "a directory, with no network (the sentinel's case)" lk_syft "$work/dir" "dir:/in" "$work/out" dir.cdx.json --network none
check "that SBOM is CycloneDX too" is_cyclonedx "$work/out/dir.cdx.json"
check "no temporary directory is left behind" nothing_left

echo "== a failure is not silent"
status=0
lk_syft "$work/bad" "oci-archive:/in/image.oci.tar" "$work/out" bad.cdx.json >"$work/bad.out" 2>&1 || status=$?
if [ "$status" -eq 0 ]; then bad "a file that is not an image was accepted"; else ok "a file that is not an image fails (exit $status)"; fi
if grep -q 'could not determine source' "$work/bad.out"; then ok "syft's own message is shown"; else cat "$work/bad.out" >&2; bad "syft's own message is not shown (a --quiet hides it)"; fi
if grep -q "syft exited with code $status, run as uid $(id -u)" "$work/bad.out"; then ok "one line says which tool failed, with what exit code and as whom"; else cat "$work/bad.out" >&2; bad "no reason line"; fi
check "a failed run leaves no temporary directory either" nothing_left

echo "== the grype cache: a directory the user cannot write is refused in one line, before any container starts"
mkdir -m 0555 "$work/read-only-cache"
status=0
LK_GRYPE_DB_CACHE="$work/read-only-cache" "$here/../scan.sh" --sbom "$here/critical.cdx.json" >"$work/cache.out" 2>&1 || status=$?
if [ "$status" -ne 0 ] && grep -q 'is not writable by uid' "$work/cache.out"; then ok "an unwritable cache is refused with its reason"; else cat "$work/cache.out" >&2; bad "an unwritable cache was not refused in one line (exit $status)"; fi

if [ "$fails" -ne 0 ]; then echo "container-user-test: $fails case(s) failed" >&2; exit 1; fi
echo "container-user-test: every case behaved"
