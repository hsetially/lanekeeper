#!/usr/bin/env bash
# Tests deploy/sentinel/publish.sh with a throwaway cosign key (09/T8; S20). Keyless signing needs GitHub's OIDC token and the Sigstore
# services, and runs in CI only (the `release` job of sentinel.yml signs and then verifies from the same job); what is tested here is the
# part that does not: that a signature is made and checked against the bytes, and that a changed file, another key and an identity that
# matches everything are refused.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
publish="$root/deploy/sentinel/publish.sh"
export PATH="$root/target/ops-tools/bin:$PATH"
command -v cosign >/dev/null 2>&1 || { echo "publish-test: cosign is not installed; run: just ops-tools-install cosign" >&2; exit 1; }
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
ok() { echo "  ok  $1"; }
bad() { echo "  FAILED  $1" >&2; fails=$((fails + 1)); }
check() {
    local name="$1"
    shift
    if "$@" >"$work/last.log" 2>&1; then ok "$name"; else cat "$work/last.log" >&2; bad "$name"; fi
}
expect_fail() {
    local name="$1" fragment="$2" out
    shift 2
    if out="$("$@" 2>&1)"; then bad "$name: it succeeded"; elif ! grep -qF -- "$fragment" <<<"$out"; then bad "$name: it failed, but not for '$fragment':"$'\n'"$out"; else ok "$name"; fi
}

mkdir "$work/keys" "$work/f"
(cd "$work/keys" && COSIGN_PASSWORD="" cosign generate-key-pair >/dev/null 2>&1 && mv cosign.key a.key && mv cosign.pub a.pub && COSIGN_PASSWORD="" cosign generate-key-pair >/dev/null 2>&1 && mv cosign.key b.key && mv cosign.pub b.pub)
printf 'the release file\n' >"$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm"
export COSIGN_PASSWORD=""

check "sign-blob with a key writes a bundle" "$publish" sign-blob "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm" --key "$work/keys/a.key"
check "the bundle is written beside the file" test -s "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm.cosign.bundle"
check "verify-blob accepts the file with the public key" "$publish" verify-blob "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm" --key "$work/keys/a.pub"
expect_fail "another key does not verify" "invalid signature" "$publish" verify-blob "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm" --key "$work/keys/b.pub"
printf 'the release file, changed\n' >"$work/f/changed"
cp "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm.cosign.bundle" "$work/f/changed.cosign.bundle"
expect_fail "a changed file does not verify" "invalid signature" "$publish" verify-blob "$work/f/changed" --key "$work/keys/a.pub"
expect_fail "an identity that matches everything is refused" "would accept any signer" "$publish" verify-blob "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm" --identity-regexp '.*' --issuer https://token.actions.githubusercontent.com
expect_fail "keyless verification needs an issuer" "needs both" "$publish" verify-blob "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm" --identity-regexp '^https://github.com/o/r/.*$'
expect_fail "a key and an identity together are refused" "either --key or" "$publish" verify-blob "$work/f/lanekeeper-sentinel-0.1.0-1.x86_64.rpm" --key "$work/keys/a.pub" --identity-regexp x --issuer y
expect_fail "a file that does not exist" "no such file" "$publish" verify-blob "$work/f/does-not-exist" --key "$work/keys/a.pub"
printf x >"$work/f/changed2"
expect_fail "a file with no bundle" "no $work/f/changed2.cosign.bundle" "$publish" verify-blob "$work/f/changed2" --key "$work/keys/a.pub"

if [ "$fails" -ne 0 ]; then echo "publish-test: $fails case(s) failed" >&2; exit 1; fi
echo "publish-test: every case behaved"
