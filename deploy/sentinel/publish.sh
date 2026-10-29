#!/usr/bin/env bash
# Signs the sentinel's release files with cosign and verifies them (09/T8; S19, S20): the RPM and its SBOM, as blobs.
#
#   deploy/sentinel/publish.sh sign-blob FILE [--key KEY]
#       Writes FILE.cosign.bundle beside FILE. Keyless by default: the signer is the workflow that runs this, proved by GitHub's OIDC token
#       (the job needs id-token: write) and recorded in the Sigstore log, which the bundle carries. With --key (a cosign key file) the signature
#       is made with that key, and nothing is logged: that is for the test, which has no OIDC token to use.
#   deploy/sentinel/publish.sh verify-blob FILE --identity-regexp RE --issuer URL
#   deploy/sentinel/publish.sh verify-blob FILE --key PUBLIC_KEY
#       Verifies FILE against FILE.cosign.bundle: for the keyless form, against the workflow identity RE and the OIDC issuer URL; for the
#       key form, against the public key.
#
# Needs cosign at the version pinned in the Justfile (`just ops-tools-install cosign`), looked up in target/ops-tools/bin first.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
PATH="$root/target/ops-tools/bin:$PATH"
die() { echo "publish.sh: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed -n '/^#   deploy/,/^# Needs/p' | sed 's/^# \{0,1\}//' | head -n -1 >&2; exit 2; }
command -v cosign >/dev/null 2>&1 || die "cosign is not installed; run: just ops-tools-install cosign"

[ $# -ge 2 ] || usage
sub="$1"
file="$2"
shift 2
[ -f "$file" ] || die "no such file '$file'"
bundle="$file.cosign.bundle"
key="" regexp="" issuer=""
while [ $# -gt 0 ]; do
    case "$1" in
        --key) [ $# -ge 2 ] || usage; key="$2"; shift 2 ;;
        --identity-regexp) [ $# -ge 2 ] || usage; regexp="$2"; shift 2 ;;
        --issuer) [ $# -ge 2 ] || usage; issuer="$2"; shift 2 ;;
        *) usage ;;
    esac
done

case "$sub" in
    sign-blob)
        [ -z "$regexp$issuer" ] || usage
        if [ -n "$key" ]; then
            [ -f "$key" ] || die "no such key file '$key'"
            cosign sign-blob --yes --key "$key" --use-signing-config=false --tlog-upload=false --bundle "$bundle" "$file" >/dev/null
        else
            cosign sign-blob --yes --bundle "$bundle" "$file" >/dev/null
        fi
        echo "publish.sh: signed $file ($bundle)"
        ;;
    verify-blob)
        [ -f "$bundle" ] || die "no $bundle"
        if [ -n "$key" ]; then
            [ -z "$regexp$issuer" ] || die "use either --key or --identity-regexp with --issuer"
            cosign verify-blob --key "$key" --insecure-ignore-tlog=true --bundle "$bundle" "$file" >/dev/null
        else
            [ -n "$regexp" ] && [ -n "$issuer" ] || die "keyless verification needs both --identity-regexp and --issuer"
            # An identity that matches everything verifies nothing.
            case "$regexp" in '' | '.*' | '^.*$' | '.+' | '^.+$') die "the identity regexp '$regexp' would accept any signer" ;; esac
            cosign verify-blob --certificate-identity-regexp "$regexp" --certificate-oidc-issuer "$issuer" --bundle "$bundle" "$file" >/dev/null
        fi
        echo "publish.sh: the signature of $file verifies"
        ;;
    *) usage ;;
esac
