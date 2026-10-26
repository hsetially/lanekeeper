#!/usr/bin/env bash
# Signs the sentinel's RPM, and checks signatures (09/T8; S19, S20, plan 09 A16). No key is stored anywhere: a signing key lives in the
# memory (a tmpfs) of a container that has no network and is removed when the command ends.
#
#   deploy/sentinel/sign.sh ephemeral --rpm FILE --out DIR
#       A pull request: makes a throwaway RSA key, signs a copy of FILE into DIR, and writes the key's public half to
#       DIR/RPM-GPG-KEY-ephemeral.pub. The secret half is gone when the command ends, so the signature proves that the package was
#       signed and has not changed since, and nothing about who signed it: that is what a pull request can show, and what the install
#       test needs to try `rpm -K` and `dnf` with a key.
#   deploy/sentinel/sign.sh release --rpm FILE --out DIR --public-key FILE < secret.asc
#       A release: reads the release key (ASCII-armoured secret key) from standard input, refuses unless its fingerprint is the one of
#       FILE (the committed RPM-GPG-KEY-lanekeeper.pub), signs a copy of FILE into DIR, and verifies the result with FILE. The release
#       job reads the key from Secret Manager with Workload Identity Federation and pipes it here; it never touches a disk.
#   deploy/sentinel/sign.sh new-release-key --out PUBLIC_KEY_FILE > secret.asc
#       Done once, by a person: makes the release key, writes its public half to PUBLIC_KEY_FILE (commit that as
#       deploy/sentinel/RPM-GPG-KEY-lanekeeper.pub) and prints the armoured secret key on standard output, and only there, to be piped
#       into Secret Manager (gcloud secrets versions add ... --data-file=-). Its fingerprint goes to standard error.
#   deploy/sentinel/sign.sh verify --rpm FILE --public-key FILE
#       Checks FILE's signature against that one key (rpm 4.18, in a container, with a database that holds only that key). Fails when
#       the package is unsigned, was signed by another key, or has changed.
#
# The tools are rpm and gpg from the image that `deploy/sentinel/build.sh --tools` makes (LK_RPM_TOOLS_IMAGE, default
# lanekeeper-rpm-tools). The key is RSA, which the rpm 4.14 of Rocky Linux 8 verifies (tests/install.sh shows it; no other type was tried),
# and has no subkey, so there is one fingerprint to compare. Signatures use SHA-256.
set -euo pipefail

die() { echo "sign.sh: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed -n '/^#   deploy/,/^# The tools/p' | sed 's/^# \{0,1\}//' | head -n -1 >&2; exit 2; }

[ $# -ge 1 ] || usage
sub="$1"
shift
rpm_file="" out="" pub=""
while [ $# -gt 0 ]; do
    case "$1" in
        --rpm) [ $# -ge 2 ] || usage; rpm_file="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || usage; out="$2"; shift 2 ;;
        --public-key) [ $# -ge 2 ] || usage; pub="$2"; shift 2 ;;
        *) usage ;;
    esac
done
tools="${LK_RPM_TOOLS_IMAGE:-lanekeeper-rpm-tools}"
[[ "$tools" =~ ^[a-z0-9][a-z0-9_./:@-]*$ ]] || die "LK_RPM_TOOLS_IMAGE '$tools' is not an image name"
docker image inspect "$tools" >/dev/null 2>&1 || die "no image $tools; make it with: deploy/sentinel/build.sh --tools"

need_rpm() {
    [ -f "$rpm_file" ] || die "--rpm FILE: no such file '$rpm_file'"
    [[ "$(basename "$rpm_file")" =~ ^lanekeeper-sentinel-[0-9]+\.[0-9]+\.[0-9]+-1\.x86_64\.rpm$ ]] || die "'$rpm_file' is not lanekeeper-sentinel-<version>-1.x86_64.rpm"
}
need_out() {
    [ -n "$out" ] || die "--out DIR is required"
    mkdir -p "$out"
    out="$(cd "$out" && pwd)"
}

# The container: no network, no capability, a read-only root, a tmpfs for the key (GNUPGHOME) and one for the work, the RPM read-only, the
# output directory writable. It runs as the user who runs this script, and its tmpfs are that user's: the output directory belongs to that
# user (`mkdir -p` above made it, or a CI step did), and a container root without CAP_DAC_OVERRIDE, which is root with none of the
# capabilities this drops, cannot write a directory that another uid owns (the first CI run: `cp: cannot create regular file '/out/...'`).
# Running as that uid and not adding a capability keeps the isolation, and the files that land in --out belong to the caller.
run_tools() { # SCRIPT [docker run args...]
    local script="$1" uid gid
    uid="$(id -u)" gid="$(id -g)"
    shift
    docker run --rm -i --network none --read-only --cap-drop ALL --security-opt no-new-privileges --user "$uid:$gid" \
        --tmpfs "/gnupg:rw,mode=0700,uid=$uid,gid=$gid,size=32m" --tmpfs "/work:rw,mode=0755,uid=$uid,gid=$gid,size=128m" \
        --tmpfs /tmp:rw,mode=1777,size=16m \
        -e GNUPGHOME=/gnupg -e HOME=/tmp "$@" "$tools" sh -eu -c "$script" sh
}

# The pieces of shell that run inside the container: single quotes on purpose, nothing here is for the host's shell to expand.
# shellcheck disable=SC2016
common='
umask 077
fingerprint_of_secret() { gpg --batch --with-colons --list-secret-keys | awk -F: "/^fpr:/ {print \$10; exit}"; }
count_secret() { gpg --batch --with-colons --list-secret-keys | grep -c "^sec:" || true; }
sign_and_check() { # FPR PUBFILE
    cp /in/package.rpm /work/package.rpm
    rpmsign --addsign --define "__gpg /usr/bin/gpg" --define "_gpg_name $1" --define "_gpg_path /gnupg" --define "_gpg_digest_algo sha256" \
        --define "_gpg_sign_cmd_extra_args --batch --pinentry-mode loopback" /work/package.rpm >/dev/null
    mkdir /work/db
    rpm --dbpath /work/db --import "$2" >/dev/null
    rpm --dbpath /work/db -K /work/package.rpm | tee /work/check.txt
    grep -q "digests signatures OK" /work/check.txt
    cp /work/package.rpm "/out/$NAME"
}
'

case "$sub" in
    ephemeral)
        need_rpm
        need_out
        name="$(basename "$rpm_file")"
        # shellcheck disable=SC2016
        run_tools "$common"'
gpg --batch --quiet --pinentry-mode loopback --passphrase "" --quick-generate-key "Lanekeeper RPM signing (throwaway, this run only) <noreply@invalid>" rsa3072 sign never
fpr="$(fingerprint_of_secret)"
gpg --batch --armor --export-options export-minimal --export "$fpr" > /work/pub.asc
sign_and_check "$fpr" /work/pub.asc
cp /work/pub.asc /out/RPM-GPG-KEY-ephemeral.pub
echo "sign.sh: signed with the throwaway key $fpr"
' -v "$(realpath "$rpm_file"):/in/package.rpm:ro" -v "$out:/out" -e "NAME=$name"
        ;;
    release)
        need_rpm
        need_out
        [ -n "$pub" ] && [ -f "$pub" ] || die "--public-key FILE: no such file '$pub'"
        name="$(basename "$rpm_file")"
        # The key arrives on standard input; the container checks it before it signs anything. Nothing is written to --out until the
        # signature has been made and has verified with the committed public key.
        # shellcheck disable=SC2016
        run_tools "$common"'
cat > /work/secret.asc
gpg --batch --quiet --import /work/secret.asc 2>/dev/null || true
rm -f /work/secret.asc
[ "$(count_secret)" = 1 ] || { echo "sign.sh: no secret key on standard input (expected exactly one armoured secret key)" >&2; exit 2; }
fpr="$(fingerprint_of_secret)"
gpg --batch --quiet --no-default-keyring --keyring /work/committed.kbx --import /in/public.pub 2>/dev/null || true
committed="$(gpg --batch --with-colons --no-default-keyring --keyring /work/committed.kbx --list-keys 2>/dev/null | awk -F: "/^fpr:/ {print \$10}")"
[ -n "$committed" ] || { echo "sign.sh: $PUBNAME is not a public key (it is the unresolved marker until a person makes the release key: plan 09 blockers, B13)" >&2; exit 2; }
[ "$(printf "%s\n" "$committed" | wc -l)" = 1 ] || { echo "sign.sh: $PUBNAME holds more than one key" >&2; exit 2; }
[ "$committed" = "$fpr" ] || { echo "sign.sh: the secret key ($fpr) does not match the committed public key ($committed); nothing was signed" >&2; exit 2; }
gpg --batch --armor --export-options export-minimal --export "$fpr" > /work/pub.asc
sign_and_check "$fpr" /work/pub.asc
echo "sign.sh: signed with the release key $fpr"
' -v "$(realpath "$rpm_file"):/in/package.rpm:ro" -v "$(realpath "$pub"):/in/public.pub:ro" -v "$out:/out" -e "NAME=$name" -e "PUBNAME=$(basename "$pub")"
        ;;
    new-release-key)
        need_out_file() { [ -n "$out" ] || die "--out PUBLIC_KEY_FILE is required"; }
        need_out_file
        [ ! -e "$out" ] || [ -f "$out" ] || die "--out '$out' is not a file"
        pub_tmp="$(mktemp)"
        trap 'rm -f "$pub_tmp"' EXIT
        # The secret key is the container's standard output and nothing else is; the public key is the second thing it prints, after a
        # marker line, so that one stream carries both and nothing is written to a disk inside the container.
        # shellcheck disable=SC2016
        full="$(run_tools "$common"'
gpg --batch --quiet --pinentry-mode loopback --passphrase "" --quick-generate-key "Lanekeeper RPM release signing <noreply@invalid>" rsa4096 sign never
fpr="$(fingerprint_of_secret)"
echo "FINGERPRINT $fpr"
gpg --batch --armor --export-options export-minimal --export "$fpr"
echo "SECRET-KEY-FOLLOWS"
gpg --batch --armor --export-secret-keys "$fpr"
')"
        fingerprint="$(sed -n '1s/^FINGERPRINT //p' <<<"$full")"
        [[ "$fingerprint" =~ ^[0-9A-F]{40}$ ]] || die "the container did not report a fingerprint"
        sed -n '2,/^-----END PGP PUBLIC KEY BLOCK-----$/p' <<<"$full" >"$pub_tmp"
        grep -q 'BEGIN PGP PUBLIC KEY BLOCK' "$pub_tmp" || die "no public key came back"
        install -m 0644 "$pub_tmp" "$out"
        echo "sign.sh: the release key is $fingerprint; its public half is in $out" >&2
        sed -n '/^-----BEGIN PGP PRIVATE KEY BLOCK-----$/,$p' <<<"$full"
        ;;
    verify)
        need_rpm
        [ -n "$pub" ] && [ -f "$pub" ] || die "--public-key FILE: no such file '$pub'"
        grep -q 'BEGIN PGP PUBLIC KEY BLOCK' "$pub" || die "$pub is not a public key"
        # rpm -K prints "digests signatures OK" for a package signed with a key it has, "digests OK" for an unsigned one (and exits 0),
        # and NOKEY or BAD for the rest. Only the first is a pass.
        # shellcheck disable=SC2016
        run_tools '
mkdir /work/db
rpm --dbpath /work/db --import /in/public.pub >/dev/null
rpm --dbpath /work/db -K /in/package.rpm > /work/check.txt 2>&1 || { cat /work/check.txt; echo "sign.sh: the signature does not verify (BAD or NOKEY above)"; exit 1; }
cat /work/check.txt
grep -q "digests signatures OK" /work/check.txt || { echo "sign.sh: the package is not signed, only its digests are intact"; exit 1; }
' -v "$(realpath "$rpm_file"):/in/package.rpm:ro" -v "$(realpath "$pub"):/in/public.pub:ro"
        ;;
    *) usage ;;
esac
