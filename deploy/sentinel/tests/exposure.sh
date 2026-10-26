#!/usr/bin/env bash
# The exposure score of the sentinel's unit (09/T8; S25, plan 09 A15): 2.0 or lower, by directives that systemd 239 knows.
#
#   deploy/sentinel/tests/exposure.sh --rpm FILE        score the unit inside the RPM
#   deploy/sentinel/tests/exposure.sh --self-test       show that the check refuses worse units (it needs no RPM)
#
# WHY IT IS DONE THIS WAY. Rocky Linux 8 ships systemd 239. `systemd-analyze security` arrived in systemd 240, and its --offline mode
# (which scores a unit file without a running unit) in 252. So the unit is scored by the systemd-analyze of a newer systemd, the
# AlmaLinux 9.8 image of deploy/images/tools.env (systemd 252), and "a score of 2.0 or lower" would mean nothing if the unit got there with
# directives that Rocky 8 ignores: systemd ignores a directive it does not know, with a line in the journal, and the sandbox it was meant
# to make is not there. So the same unit is also checked by the systemd of the release it will run on: `systemd-analyze verify` in the Rocky
# Linux 8.9 and 8.10 images prints "Unknown lvalue" for such a directive (and exits 0, so the output is what is checked). The score is
# therefore of a unit that does only what 239 does; what 239 cannot do (ProtectClock, ProtectHostname, ProtectKernelLogs, ProtectProc,
# ProcSubset) is not set, and the newer systemd-analyze counts it against the unit.
#
# The packaged unit is the one scored: it is taken out of the RPM, and compared with the file in the repository.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$root/deploy/images/tools.env"
source_unit="$root/deploy/sentinel/lanekeeper-sentinel.service"
max_score="2.0"
threshold=20 # --threshold counts tenths of a point

usage() { echo "usage: $0 --rpm FILE | --self-test" >&2; exit 2; }
rpm_file="" self_test=""
while [ $# -gt 0 ]; do
    case "$1" in
        --rpm) [ $# -ge 2 ] || usage; rpm_file="$2"; shift 2 ;;
        --self-test) self_test=1; shift ;;
        *) usage ;;
    esac
done
{ [ -n "$rpm_file" ] || [ -n "$self_test" ]; } || usage
[ -z "$rpm_file" ] || [ -f "$rpm_file" ] || { echo "exposure.sh: no such file '$rpm_file'" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# analyze UNIT: the score of a unit file, from the pinned newer systemd. Prints the table and the verdict; returns the analyzer's status.
analyze() {
    docker run --rm --network none --entrypoint sh -v "$1:/unit/lanekeeper-sentinel.service:ro" "$LK_SYSTEMD_ANALYZE_IMAGE" \
        -c "systemd-analyze security --offline=true --threshold=$threshold --no-pager /unit/lanekeeper-sentinel.service"
}

# verify_on_239 UNIT IMAGE: what the systemd of a Rocky Linux 8 release says about a unit file. Empty is clean.
verify_on_239() {
    docker run --rm --network none --entrypoint sh -v "$1:/unit/lanekeeper-sentinel.service:ro" "$2" \
        -c 'install -m 0644 /unit/lanekeeper-sentinel.service /etc/systemd/system/lanekeeper-sentinel.service && install -m 0755 /bin/true /usr/bin/lanekeeper-sentinel && systemd-analyze verify /etc/systemd/system/lanekeeper-sentinel.service 2>&1'
}

# judge UNIT LABEL: the whole check for one unit; prints what it found and returns 0 only when both halves hold.
judge() {
    local unit="$1" label="$2" out status=0 score release
    out="$(analyze "$unit" 2>&1)" || status=$?
    score="$(sed -n 's/^→ Overall exposure level for [^:]*: \([0-9][0-9]*\.[0-9]\).*/\1/p' <<<"$out")"
    if ! [[ "$score" =~ ^[0-9]+\.[0-9]$ ]]; then
        echo "$out" >&2
        echo "exposure: $label: no score in the output of systemd-analyze" >&2
        return 1
    fi
    local over=0
    awk -v s="$score" -v m="$max_score" 'BEGIN { exit !(s + 0 > m + 0) }' && over=1
    if [ "$over" = 1 ] || [ "$status" -ne 0 ]; then
        echo "exposure: $label: the score is $score, above $max_score (systemd-analyze exited $status)" >&2
        return 1
    fi
    for release in "$LK_ROCKY_8_9_IMAGE" "$LK_ROCKY_8_10_IMAGE"; do
        local said
        said="$(verify_on_239 "$unit" "$release")" || true
        if [ -n "$said" ]; then
            echo "exposure: $label: the systemd of $release does not read the unit as it is written:" >&2
            echo "$said" >&2
            return 1
        fi
    done
    echo "exposure: $label: score $score (at most $max_score), and the systemd 239 of Rocky Linux 8.9 and 8.10 reads every directive"
    return 0
}

if [ -n "$rpm_file" ]; then
    # The unit as packaged: out of the RPM (rpm2archive writes a tar.gz of the payload), and identical to the file in the repository.
    docker run --rm --network none --entrypoint sh -v "$(realpath "$rpm_file"):/pkg.rpm:ro" "$LK_SYSTEMD_ANALYZE_IMAGE" \
        -c 'rpm2archive - < /pkg.rpm | tar -xzO ./usr/lib/systemd/system/lanekeeper-sentinel.service' >"$work/packaged.service"
    [ -s "$work/packaged.service" ] || { echo "exposure: the RPM has no usr/lib/systemd/system/lanekeeper-sentinel.service" >&2; exit 1; }
    if [ "$(sha256sum <"$work/packaged.service")" != "$(sha256sum <"$source_unit")" ]; then
        echo "exposure: the unit in the RPM is not deploy/sentinel/lanekeeper-sentinel.service (a stale package?)" >&2
        exit 1
    fi
    echo "exposure: the unit in the RPM is the file in the repository"
    echo "exposure: scored by $(docker run --rm --network none --entrypoint sh "$LK_SYSTEMD_ANALYZE_IMAGE" -c 'systemd-analyze --version | head -n1'), image $LK_SYSTEMD_ANALYZE_IMAGE"
    analyze "$work/packaged.service" | grep -E '^(✗|→)' || true
    judge "$work/packaged.service" "the packaged unit"
    exit $?
fi

# --- the self-test: units that are worse must be refused, each for its own reason ----------------------------------------------------------
fails=0
# mutant NAME SED WANT: SED applied to the unit must change it, and the check must refuse the result with a message that has WANT.
mutant() {
    local name="$1" expr="$2" want="$3" out
    sed -E "$expr" "$source_unit" >"$work/mutant.service"
    if cmp -s "$source_unit" "$work/mutant.service"; then
        echo "  FAILED  $name: the sed expression changed nothing" >&2
        fails=$((fails + 1))
        return
    fi
    if out="$(judge "$work/mutant.service" "$name" 2>&1)"; then
        echo "  FAILED  $name: the check accepted it" >&2
        fails=$((fails + 1))
    elif ! grep -qF -- "$want" <<<"$out"; then
        echo "  FAILED  $name: refused, but not for '$want':" >&2
        echo "$out" >&2
        fails=$((fails + 1))
    else
        echo "  ok  $name"
    fi
}
echo "== the unit as it is"
if judge "$source_unit" "the unit in the repository"; then echo "  ok  it passes"; else echo "  FAILED  the unit in the repository does not pass" >&2; fails=$((fails + 1)); fi
echo "== worse units are refused"
mutant "no empty capability bounding set" '/^CapabilityBoundingSet=/d' "above $max_score"
mutant "no system call filter" '/^SystemCallFilter=/d' "above $max_score"
mutant "namespaces are not restricted" '/^RestrictNamespaces=/d' "above $max_score"
mutant "address families are not restricted" '/^RestrictAddressFamilies=/d' "above $max_score"
mutant "runs as root" '/^User=/d' "above $max_score"
mutant "nothing but ExecStart" '/^(User|Group|StateDirectory|ReadWritePaths|Capab|Ambient|NoNew|Protect|Private|Memory|Restrict|Lock|Remove|System|UMask|Keyring|Device)/d' "above $max_score"
echo "== a directive that systemd 239 does not know is refused, although the newer systemd-analyze would score it better"
mutant "ProtectClock" 's/^LockPersonality=yes$/&\nProtectClock=yes/' "does not read the unit as it is written"
mutant "ProtectProc" 's/^LockPersonality=yes$/&\nProtectProc=invisible/' "Unknown lvalue 'ProtectProc'"
mutant "ProtectHostname" 's/^LockPersonality=yes$/&\nProtectHostname=yes/' "Unknown lvalue 'ProtectHostname'"
mutant "a misspelt directive" 's/^NoNewPrivileges=yes$/NoNewPrivilege=yes/' "Unknown lvalue 'NoNewPrivilege'"
if [ "$fails" -ne 0 ]; then echo "exposure: the self-test failed ($fails)" >&2; exit 1; fi
echo "exposure: the check refuses every worse unit"
