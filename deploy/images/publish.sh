#!/usr/bin/env bash
# Publishes an image that was built and scanned (09/T2; S19, S20): pushes that exact OCI archive, signs it, attaches its SBOM, and
# verifies what it signed. CI runs it in the `publish` job of images.yml, which holds the only write permissions of the workflow
# (the build jobs hold none, so no build step ever has a token that can push). Every command works on a digest, never on a tag.
#
#   deploy/images/publish.sh push IMAGE REPOSITORY TAG [--dir DIR]
#       Pushes DIR/IMAGE.oci.tar (default target/images) as REPOSITORY:TAG and prints REPOSITORY@sha256:<digest> on stdout. The
#       digest must be the one build.sh recorded in DIR/IMAGE.digest: the archive is read first, and a mismatch pushes nothing.
#   deploy/images/publish.sh sign REF [--key KEY]
#       Signs REF (a digest reference). Keyless by default: the signer is this workflow, proved by GitHub's OIDC token (the job needs
#       id-token: write) and recorded in the Sigstore log. With --key (a cosign key file, or a KMS URI such as gcpkms://...) the
#       signature is made with that key: this is the second signature that Binary Authorization checks (plan 09 A5).
#   deploy/images/publish.sh attest-sbom REF SBOM [--key KEY]
#       Attaches SBOM (a CycloneDX JSON file) to REF as a signed in-toto attestation, signed as above.
#   deploy/images/publish.sh verify REF --identity-regexp RE --issuer URL
#   deploy/images/publish.sh verify REF --key PUBLIC_KEY
#       Verifies the signature and the CycloneDX attestation of REF: for the keyless form, against the workflow identity RE and
#       the OIDC issuer URL; for the key form, against the public key.
#
# Needs cosign and crane at the versions pinned in the Justfile (`just ops-tools-install cosign crane`); both are looked up in
# target/ops-tools/bin first. Registry credentials come from the Docker config (CI: `crane auth login ghcr.io`).
#
# LK_COSIGN_NO_TLOG=1 skips the Sigstore log, for a test against a registry on localhost, where there is no certificate to log. It
# is refused for any other registry, so a mistake in a workflow cannot switch the log off.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
PATH="$root/target/ops-tools/bin:$PATH"

die() { echo "publish.sh: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed -n '/^#   deploy/,/^# Needs/p' | sed 's/^# \{0,1\}//' | head -n -1 >&2; exit 2; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is not installed; run: just ops-tools-install $1"; }

REPO_RE='^[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?(/[a-z0-9]+([._-][a-z0-9]+)*)+$'
TAG_RE='^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$'
DIGEST_REF_RE='^[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?(/[a-z0-9]+([._-][a-z0-9]+)*)+@sha256:[0-9a-f]{64}$'

# The registry host of a reference: localhost is the only place the Sigstore log may be skipped.
tlog_args_sign=()
tlog_args_verify=()
set_tlog() { # REF
    tlog_args_sign=()
    tlog_args_verify=()
    [ "${LK_COSIGN_NO_TLOG:-}" = "1" ] || return 0
    local host="${1%%/*}"
    case "$host" in
        localhost | localhost:* | 127.0.0.1 | 127.0.0.1:*) ;;
        *) die "LK_COSIGN_NO_TLOG=1 is for a registry on localhost, not $host" ;;
    esac
    tlog_args_sign=(--use-signing-config=false --tlog-upload=false)
    tlog_args_verify=(--insecure-ignore-tlog=true)
}

digest_ref() { # REF: must name a digest
    [[ "$1" =~ $DIGEST_REF_RE ]] || die "'$1' is not a digest reference (repository@sha256:<64 hex>); sign and verify digests, never tags"
}

cmd_push() {
    [ $# -ge 3 ] || usage
    local image="$1" repository="$2" tag="$3" dir="$root/target/images"
    shift 3
    while [ $# -gt 0 ]; do
        case "$1" in --dir) [ $# -ge 2 ] || usage; dir="$2"; shift 2 ;; *) usage ;; esac
    done
    case "$image" in hub | agent) ;; *) die "IMAGE must be hub or agent" ;; esac
    [[ "$repository" =~ $REPO_RE ]] || die "'$repository' is not a lower-case repository name such as ghcr.io/owner/repo/hub"
    [[ "$tag" =~ $TAG_RE ]] || die "'$tag' is not a valid tag"
    need crane
    need jq
    local archive="$dir/$image.oci.tar" recorded
    [ -f "$archive" ] || die "no $archive; build it first"
    recorded="$(cat "$dir/$image.digest" 2>/dev/null || true)"
    [[ "$recorded" =~ ^sha256:[0-9a-f]{64}$ ]] || die "$dir/$image.digest does not hold a sha256 digest"

    need python3
    # The archive crossed a job boundary (the image job made it, this job did not), so its structure is checked before any member
    # is extracted: an OCI layout holds oci-layout, index.json and blobs/sha256/<64 hex>, as plain files, and nothing else. A
    # symlink, a hard link, a device, a path that leaves the directory or an odd name is refused, so extraction cannot write
    # anywhere but into the new directory, and cannot replace a tool that runs later in this job.
    python3 -I - "$archive" <<'PY' || die "unexpected member in $archive (see above); nothing was extracted or pushed"
import re, sys, tarfile
MAX_MEMBERS = 4096
FILE = re.compile(r"^(oci-layout|index\.json|blobs/sha256/[0-9a-f]{64})$")
DIRS = {"blobs", "blobs/sha256"}
seen = set()
with tarfile.open(sys.argv[1], "r:") as tar:
    for count, m in enumerate(tar, 1):
        name = m.name.rstrip("/") if m.isdir() else m.name
        if count > MAX_MEMBERS:
            sys.exit("more than %d members" % MAX_MEMBERS)
        if name in seen:
            sys.exit("member listed twice: %r" % name)
        seen.add(name)
        if m.isdir():
            if name not in DIRS:
                sys.exit("directory %r" % m.name)
        elif m.isreg():
            if not FILE.match(name):
                sys.exit("file %r" % m.name)
        else:
            sys.exit("member %r is not a plain file or directory (type %r)" % (m.name, m.type))
for required in ("oci-layout", "index.json"):
    if required not in seen:
        sys.exit("no %s" % required)
PY

    local work
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' RETURN
    mkdir "$work/layout"
    tar --extract --no-same-owner --no-same-permissions --file "$archive" --directory "$work/layout"
    # The archive holds exactly one image, and it is the one that was scanned.
    jq -e '(.manifests | length) == 1 and (.manifests[0].mediaType == "application/vnd.oci.image.manifest.v1+json")' \
        "$work/layout/index.json" >/dev/null || die "$archive does not hold exactly one image manifest"
    local inside
    inside="$(jq -r '.manifests[0].digest' "$work/layout/index.json")"
    [ "$inside" = "$recorded" ] || die "the archive holds $inside but build.sh recorded $recorded; nothing was pushed"

    local pushed
    pushed="$(crane push "$work/layout" "$repository:$tag")"
    [ "$pushed" = "$repository@$recorded" ] || die "crane pushed '$pushed', expected $repository@$recorded"
    echo "$pushed"
}

cmd_sign() {
    [ $# -ge 1 ] || usage
    local ref="$1" key=""
    shift
    while [ $# -gt 0 ]; do
        case "$1" in --key) [ $# -ge 2 ] || usage; key="$2"; shift 2 ;; *) usage ;; esac
    done
    digest_ref "$ref"
    need cosign
    set_tlog "$ref"
    if [ -n "$key" ]; then
        cosign sign --yes --key "$key" ${tlog_args_sign[@]+"${tlog_args_sign[@]}"} "$ref"
    else
        [ ${#tlog_args_sign[@]} -eq 0 ] || die "a keyless signature always goes to the Sigstore log"
        cosign sign --yes "$ref"
    fi
}

cmd_attest_sbom() {
    [ $# -ge 2 ] || usage
    local ref="$1" sbom="$2" key=""
    shift 2
    while [ $# -gt 0 ]; do
        case "$1" in --key) [ $# -ge 2 ] || usage; key="$2"; shift 2 ;; *) usage ;; esac
    done
    digest_ref "$ref"
    [ -s "$sbom" ] || die "no SBOM at $sbom"
    jq -e '.bomFormat == "CycloneDX"' "$sbom" >/dev/null || die "$sbom is not a CycloneDX document"
    need cosign
    set_tlog "$ref"
    if [ -n "$key" ]; then
        cosign attest --yes --type cyclonedx --predicate "$sbom" --key "$key" ${tlog_args_sign[@]+"${tlog_args_sign[@]}"} "$ref"
    else
        [ ${#tlog_args_sign[@]} -eq 0 ] || die "a keyless attestation always goes to the Sigstore log"
        cosign attest --yes --type cyclonedx --predicate "$sbom" "$ref"
    fi
}

cmd_verify() {
    [ $# -ge 3 ] || usage
    local ref="$1" regexp="" issuer="" key=""
    shift
    while [ $# -gt 0 ]; do
        case "$1" in
            --identity-regexp) [ $# -ge 2 ] || usage; regexp="$2"; shift 2 ;;
            --issuer) [ $# -ge 2 ] || usage; issuer="$2"; shift 2 ;;
            --key) [ $# -ge 2 ] || usage; key="$2"; shift 2 ;;
            *) usage ;;
        esac
    done
    digest_ref "$ref"
    local who=()
    if [ -n "$key" ]; then
        [ -z "$regexp$issuer" ] || die "use either --key or --identity-regexp with --issuer"
        who=(--key "$key")
    else
        [ -n "$regexp" ] && [ -n "$issuer" ] || die "keyless verification needs both --identity-regexp and --issuer"
        # An identity that matches everything verifies nothing.
        case "$regexp" in '' | '.*' | '^.*$' | '.+' | '^.+$') die "the identity regexp '$regexp' would accept any signer" ;; esac
        who=(--certificate-identity-regexp "$regexp" --certificate-oidc-issuer "$issuer")
    fi
    need cosign
    set_tlog "$ref"
    cosign verify "${who[@]}" ${tlog_args_verify[@]+"${tlog_args_verify[@]}"} "$ref" >/dev/null
    echo "publish.sh: the signature of $ref verifies"
    cosign verify-attestation --type cyclonedx "${who[@]}" ${tlog_args_verify[@]+"${tlog_args_verify[@]}"} "$ref" >/dev/null
    echo "publish.sh: the CycloneDX attestation of $ref verifies"
}

[ $# -ge 1 ] || usage
sub="$1"
shift
case "$sub" in
    push) cmd_push "$@" ;;
    sign) cmd_sign "$@" ;;
    attest-sbom) cmd_attest_sbom "$@" ;;
    verify) cmd_verify "$@" ;;
    *) usage ;;
esac
