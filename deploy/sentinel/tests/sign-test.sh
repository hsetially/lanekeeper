#!/usr/bin/env bash
# Tests deploy/sentinel/sign.sh (09/T8; S19, S20, plan 09 A16): a pull request is signed with a throwaway key and verified with it, the
# release key signs only when it is the key that is committed, and a signature that does not belong fails. It runs the tools of the `rpm`
# stage (rpm 4.18 and gpg) in containers without a network, with the key in a tmpfs.
#
#   deploy/sentinel/tests/sign-test.sh [--rpm FILE] [--tools IMAGE]
#
# FILE is an unsigned RPM from build.sh (default: the one under target/sentinel). IMAGE is the tools image (default
# lanekeeper-rpm-tools, made by `deploy/sentinel/build.sh --tools`).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Started as root, step down to the unprivileged user that a CI runner is. As root, no permission is ever refused, and the container of
# sign.sh, which has no capability, could not write a directory that the runner owns (the first CI run): this test would not have seen it.
[ "$(id -u)" -ne 0 ] || exec "$here/../../tests/as-user.sh" "${BASH_SOURCE[0]}" "$@"
root="$(git -C "$here" rev-parse --show-toplevel)"
sign="$root/deploy/sentinel/sign.sh"
rpm_file=""
tools="${LK_RPM_TOOLS_IMAGE:-lanekeeper-rpm-tools}"
while [ $# -gt 0 ]; do
    case "$1" in
        --rpm) rpm_file="$2"; shift 2 ;;
        --tools) tools="$2"; shift 2 ;;
        *) echo "usage: $0 [--rpm FILE] [--tools IMAGE]" >&2; exit 2 ;;
    esac
done
if [ -z "$rpm_file" ]; then
    rpm_file="$(find "$root/target/sentinel" -name 'lanekeeper-sentinel-*.rpm' 2>/dev/null | sort | head -n1)"
fi
[ -f "$rpm_file" ] || { echo "sign-test: no RPM; build one with deploy/sentinel/build.sh" >&2; exit 1; }
export LK_RPM_TOOLS_IMAGE="$tools"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
ok() { echo "  ok  $1"; }
bad() { echo "  FAILED  $1" >&2; fails=$((fails + 1)); }
# check NAME COMMAND...: the command must succeed (its output is shown only when it does not).
check() {
    local name="$1"
    shift
    if "$@" >"$work/last.log" 2>&1; then ok "$name"; else cat "$work/last.log" >&2; bad "$name"; fi
}
# expect_fail NAME FRAGMENT COMMAND...: the command must fail, and say FRAGMENT.
expect_fail() {
    local name="$1" fragment="$2" out
    shift 2
    if out="$("$@" 2>&1)"; then
        bad "$name: it succeeded"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        bad "$name: it failed, but not for '$fragment':"$'\n'"$out"
    else
        ok "$name"
    fi
}
is_public_key() { grep -q 'BEGIN PGP PUBLIC KEY BLOCK' "$1" && ! grep -q 'PRIVATE KEY' "$1"; }
is_secret_key() { grep -q 'BEGIN PGP PRIVATE KEY BLOCK' "$1"; }
differ() { ! cmp -s "$1" "$2"; }
only_two_files() { [ "$(find "$1" -type f | wc -l)" = 2 ]; }
empty_dir() { [ -z "$(find "$1" -type f)" ]; }
# release STDIN_FILE OUT PUBLIC_KEY: sign with the key on standard input.
release() { "$sign" release --rpm "$rpm_file" --out "$2" --public-key "$3" <"$1"; }

echo "== a pull request: a throwaway key, made for this run"
mkdir "$work/pr"
check "ephemeral signing" "$sign" ephemeral --rpm "$rpm_file" --out "$work/pr"
signed="$work/pr/$(basename "$rpm_file")"
pub="$work/pr/RPM-GPG-KEY-ephemeral.pub"
check "the signed package is written" test -f "$signed"
check "the public key is written" test -f "$pub"
check "nothing else is written (no secret key)" only_two_files "$work/pr"
check "the key file is a public key" is_public_key "$pub"
check "the signed package verifies with its own key" "$sign" verify --rpm "$signed" --public-key "$pub"
check "the signed package differs from the unsigned one" differ "$rpm_file" "$signed"
expect_fail "the input is left unsigned" "not signed" "$sign" verify --rpm "$rpm_file" --public-key "$pub"

echo "== it does not verify when it should not"
mkdir "$work/tampered"
python3 -I - "$signed" "$work/tampered/$(basename "$rpm_file")" <<'PY'
import sys
data = bytearray(open(sys.argv[1], 'rb').read())
data[-100] ^= 0xFF  # a byte of the payload
open(sys.argv[2], 'wb').write(data)
PY
expect_fail "a package with a changed byte" "BAD" "$sign" verify --rpm "$work/tampered/$(basename "$rpm_file")" --public-key "$pub"
mkdir "$work/other"
check "a second throwaway key" "$sign" ephemeral --rpm "$rpm_file" --out "$work/other"
expect_fail "a package signed by another key" "NOKEY" "$sign" verify --rpm "$signed" --public-key "$work/other/RPM-GPG-KEY-ephemeral.pub"
check "each run makes its own key" differ "$pub" "$work/other/RPM-GPG-KEY-ephemeral.pub"

echo "== the release key: only the one that is committed"
install -d -m 0700 "$work/rel"
secret="$work/rel/secret.asc"
relpub="$work/rel/RPM-GPG-KEY-lanekeeper.pub"
"$sign" new-release-key --out "$relpub" >"$secret" 2>"$work/rel.log" || { cat "$work/rel.log" >&2; bad "new-release-key failed"; }
check "the secret key is on stdout" is_secret_key "$secret"
check "the public key is in the file" is_public_key "$relpub"
mkdir "$work/release"
check "the release key signs when it is the committed one" release "$secret" "$work/release" "$relpub"
check "the release-signed package verifies with the committed key" "$sign" verify --rpm "$work/release/$(basename "$rpm_file")" --public-key "$relpub"
# A key that is not the committed one, a file that is not a key, and the unresolved marker each stop the signing before it starts.
mkdir "$work/refused"
cp "$relpub" "$work/pubonly.asc"
echo "# UNRESOLVED" >"$work/marker.pub"
expect_fail "a secret key that is not the committed key" "does not match" release "$secret" "$work/refused" "$work/other/RPM-GPG-KEY-ephemeral.pub"
expect_fail "a public key file that is the unresolved marker" "is not a public key" release "$secret" "$work/refused" "$work/marker.pub"
expect_fail "the committed file, which is not a key yet" "is not a public key" release "$secret" "$work/refused" "$here/../RPM-GPG-KEY-lanekeeper.pub"
expect_fail "no secret key on stdin" "no secret key" release /dev/null "$work/refused" "$relpub"
expect_fail "a public key where a secret key belongs" "no secret key" release "$work/pubonly.asc" "$work/refused" "$relpub"
check "a refused signing writes nothing" empty_dir "$work/refused"

echo "== the committed key file is the marker or a public key, never a private key"
committed="$here/../RPM-GPG-KEY-lanekeeper.pub"
if head -n1 "$committed" | grep -q '^# UNRESOLVED'; then
    ok "RPM-GPG-KEY-lanekeeper.pub is the unresolved marker (plan 09 blockers, B13)"
elif is_public_key "$committed"; then
    ok "RPM-GPG-KEY-lanekeeper.pub is a public key"
else
    bad "RPM-GPG-KEY-lanekeeper.pub is neither the unresolved marker nor a public key"
fi

if [ "$fails" -ne 0 ]; then echo "sign-test: $fails case(s) failed" >&2; exit 1; fi
echo "sign-test: every case behaved"
