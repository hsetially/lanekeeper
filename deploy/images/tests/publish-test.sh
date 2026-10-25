#!/usr/bin/env bash
# Proves deploy/images/publish.sh (09/T2; S19, S20) against a registry on localhost, with a throwaway key:
#   - push puts the archive in the registry under the digest build.sh recorded, and refuses an archive that holds another one;
#   - a signature and an SBOM attestation made with the key verify, with the key's public half and no other;
#   - an image nobody signed does not verify; a tag is never signed or verified; the checks of the arguments hold.
# What this cannot show is the keyless path (GitHub's OIDC token and the Sigstore log): CI exercises it in the publish job of
# images.yml, which signs, attaches the SBOM and verifies the certificate identity before it is done.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$here/../tools.env"
export PATH="$root/target/ops-tools/bin:$PATH"
publish="$here/../publish.sh"
work="$(mktemp -d)"
name="lk-publish-test-$$"
registry="lk-publish-test-registry-$$"
cleanup() {
    docker rm --force "$registry" >/dev/null 2>&1 || true
    docker buildx rm --force "$name" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT
fail() { echo "publish-test: $*" >&2; exit 1; }
expect_fail() { # MESSAGE-PATTERN COMMAND...
    local pattern="$1"
    shift
    if "$@" >"$work/out" 2>&1; then fail "this succeeded but must not: $*"; fi
    grep -qE -- "$pattern" "$work/out" || fail "the failure of '$*' does not match /$pattern/: $(cat "$work/out")"
}
for tool in cosign crane; do
    command -v "$tool" >/dev/null || fail "need $tool: just ops-tools-install cosign crane"
done

# A registry on localhost (the image may come through the Docker Hub mirror when LK_DOCKER_HUB_MIRROR is set).
image="$LK_REGISTRY_IMAGE"
[ -z "${LK_DOCKER_HUB_MIRROR:-}" ] || image="${image/#docker.io\//$LK_DOCKER_HUB_MIRROR/}"
docker run --detach --name "$registry" --publish 127.0.0.1::5000 "$image" >/dev/null
port="$(docker port "$registry" 5000/tcp | head -n1 | sed 's/.*://')"
host="127.0.0.1:$port"
for _ in $(seq 1 50); do curl -fsS "http://$host/v2/" >/dev/null 2>&1 && break; sleep 0.2; done
curl -fsS "http://$host/v2/" >/dev/null || fail "the registry on $host did not start"

# Two small images, built the way build.sh builds them (OCI archive, timestamps rewritten), and a minimal SBOM.
"$here/../builder.sh" "$name" >/dev/null
mkdir "$work/ctx" "$work/images"
printf 'FROM scratch\nCOPY hello /hello\n' > "$work/ctx/Dockerfile"
build() { # FILE-CONTENT IMAGE-NAME
    printf '%s\n' "$1" > "$work/ctx/hello"
    docker buildx build --builder "$name" --no-cache --provenance=false --sbom=false --build-arg SOURCE_DATE_EPOCH=1700000000 \
        --output "type=oci,dest=$work/images/$2.oci.tar,rewrite-timestamp=true" --metadata-file "$work/$2.json" "$work/ctx" >/dev/null 2>&1
    jq -r '."containerimage.digest"' "$work/$2.json" > "$work/images/$2.digest"
}
build one hub
build two agent
printf '{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"components":[{"type":"library","name":"example","version":"1.0.0"}]}\n' > "$work/images/hub.cdx.json"
hub_digest="$(cat "$work/images/hub.digest")"
agent_digest="$(cat "$work/images/agent.digest")"
[ "$hub_digest" != "$agent_digest" ] || fail "the two test images are the same image"

# push: the digest in the registry is the digest that was built.
repo="$host/lanekeeper/test/hub"
ref="$("$publish" push hub "$repo" sha-1 --dir "$work/images")"
[ "$ref" = "$repo@$hub_digest" ] || fail "push printed '$ref', expected $repo@$hub_digest"
[ "$(crane digest "$repo:sha-1")" = "$hub_digest" ] || fail "the registry holds another digest under sha-1"
echo "publish-test: push stores $hub_digest"

# push refuses an archive that is not the recorded one, and pushes nothing.
sed -i 's/^/x/' "$work/images/agent.digest"
expect_fail 'does not hold a sha256 digest' "$publish" push agent "$host/lanekeeper/test/agent" sha-1 --dir "$work/images"
printf '%s\n' "$hub_digest" > "$work/images/agent.digest" # a record that names the other image
expect_fail 'archive holds sha256:.* but build\.sh recorded' "$publish" push agent "$host/lanekeeper/test/agent" sha-1 --dir "$work/images"
if crane ls "$host/lanekeeper/test/agent" >/dev/null 2>&1; then
    [ -z "$(crane ls "$host/lanekeeper/test/agent")" ] || fail "a refused push left tags in the registry"
fi
echo "publish-test: a push of the wrong archive is refused and leaves nothing"
printf '%s\n' "$agent_digest" > "$work/images/agent.digest"

# push reads an archive that crossed a job boundary, so it must not trust its structure: a member that is not part of an OCI layout
# (a symlink, a hard link, a path that leaves the directory, a file in a place the layout has none) is refused before anything is
# extracted, and nothing is pushed. The cases are made from the real archive with one extra member each.
tamper() { # NAME KIND: copies hub.oci.tar to $work/tamper/NAME/hub.oci.tar with one extra member of KIND
    local dir="$work/tamper/$1"
    mkdir -p "$dir"
    cp "$work/images/hub.digest" "$dir/hub.digest"
    python3 -I - "$work/images/hub.oci.tar" "$dir/hub.oci.tar" "$2" <<'PY'
import sys, tarfile, io
src, dst, kind = sys.argv[1:4]
with tarfile.open(src) as a, tarfile.open(dst, "w") as b:
    for m in a.getmembers():
        b.addfile(m, a.extractfile(m) if m.isfile() else None)
    extra = tarfile.TarInfo()
    data = b"x"
    if kind == "symlink":
        extra.name, extra.type, extra.linkname = "blobs/sha256/" + "a" * 64, tarfile.SYMTYPE, "/etc/passwd"
    elif kind == "hardlink":
        extra.name, extra.type, extra.linkname = "blobs/sha256/" + "b" * 64, tarfile.LNKTYPE, "index.json"
    elif kind == "traversal":
        extra.name, extra.size = "../../escaped", 1
    elif kind == "absolute":
        extra.name, extra.size = "/tmp/lk-publish-test-absolute", 1
    elif kind == "stray":
        extra.name, extra.size = "stray.txt", 1
    elif kind == "badblob":
        extra.name, extra.size = "blobs/sha256/not-a-digest", 1
    elif kind == "device":
        extra.name, extra.type = "blobs/sha256/" + "c" * 64, tarfile.CHRTYPE
    if extra.type in (tarfile.REGTYPE, tarfile.AREGTYPE) and extra.size:
        b.addfile(extra, io.BytesIO(data))
    else:
        b.addfile(extra)
PY
}
for kind in symlink hardlink traversal absolute stray badblob device; do
    tamper "$kind" "$kind"
    expect_fail 'unexpected member' "$publish" push hub "$host/lanekeeper/test/tampered-$kind" sha-1 --dir "$work/tamper/$kind"
    if crane ls "$host/lanekeeper/test/tampered-$kind" >/dev/null 2>&1; then
        [ -z "$(crane ls "$host/lanekeeper/test/tampered-$kind")" ] || fail "an archive with a $kind member left tags in the registry"
    fi
done
[ ! -e /tmp/lk-publish-test-absolute ] || fail "an absolute path in an archive was written"
echo "publish-test: archives with a symlink, a hard link, a device, a stray file or a path that leaves the directory are refused"

# push refuses names that are not what they must be.
expect_fail 'repository name' "$publish" push hub 'Bad/Upper' sha-1 --dir "$work/images"
expect_fail 'not a valid tag' "$publish" push hub "$repo" 'bad tag' --dir "$work/images"
expect_fail 'IMAGE must be hub or agent' "$publish" push other "$repo" sha-1 --dir "$work/images"

# sign and attest with a throwaway key; verify with its public half.
(cd "$work" && COSIGN_PASSWORD=test-only cosign generate-key-pair --output-key-prefix good >/dev/null 2>&1 && COSIGN_PASSWORD=test-only cosign generate-key-pair --output-key-prefix other >/dev/null 2>&1)
export COSIGN_PASSWORD=test-only LK_COSIGN_NO_TLOG=1
"$publish" sign "$ref" --key "$work/good.key" >/dev/null 2>&1
"$publish" attest-sbom "$ref" "$work/images/hub.cdx.json" --key "$work/good.key" >/dev/null 2>&1
"$publish" verify "$ref" --key "$work/good.pub"
echo "publish-test: a signature and an SBOM attestation made with the key verify with its public half"

# ...and not with another key, nor on an image that was never signed.
expect_fail 'rror|no matching|fail' "$publish" verify "$ref" --key "$work/other.pub"
unsigned="$("$publish" push agent "$host/lanekeeper/test/agent" sha-1 --dir "$work/images")"
expect_fail 'rror|no matching|fail|not found' "$publish" verify "$unsigned" --key "$work/good.pub"
# A signature without the SBOM attestation is not enough: sign the agent image only.
"$publish" sign "$unsigned" --key "$work/good.key" >/dev/null 2>&1
expect_fail 'rror|no matching|fail|attestation' "$publish" verify "$unsigned" --key "$work/good.pub"
echo "publish-test: another key, an unsigned image and a missing SBOM attestation do not verify"

# Arguments: digests only, a CycloneDX SBOM only, an identity that means something, the log skipped on localhost only.
expect_fail 'not a digest reference' "$publish" sign "$repo:sha-1" --key "$work/good.key"
expect_fail 'not a digest reference' "$publish" verify "$repo:sha-1" --key "$work/good.pub"
printf '{"not":"a bom"}\n' > "$work/notbom.json"
expect_fail 'not a CycloneDX document' "$publish" attest-sbom "$ref" "$work/notbom.json" --key "$work/good.key"
expect_fail 'needs both --identity-regexp and --issuer' "$publish" verify "$ref" --identity-regexp '^https://github.com/o/r/'
expect_fail 'would accept any signer' "$publish" verify "$ref" --identity-regexp '.*' --issuer https://token.actions.githubusercontent.com
expect_fail 'either --key or' "$publish" verify "$ref" --key "$work/good.pub" --issuer https://x.example
expect_fail 'is for a registry on localhost, not ghcr\.io' "$publish" sign "ghcr.io/o/r/hub@$hub_digest" --key "$work/good.key"
expect_fail 'always goes to the Sigstore log' "$publish" sign "$ref"
echo "publish-test: the argument checks hold"
