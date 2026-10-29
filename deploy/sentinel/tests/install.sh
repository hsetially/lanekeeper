#!/usr/bin/env bash
# Installs the signed sentinel RPM on Rocky Linux 8.9 and 8.10 and starts it (09/T8; S19, S20, S25, plan 09 A18).
#
#   deploy/sentinel/tests/install.sh --rpm SIGNED --unsigned UNSIGNED --public-key PUB --audit repo|stand-in [--tools IMAGE] [8.9] [8.10]
#
# For each release (both, unless named) it boots systemd in a container of the digest-pinned Rocky image of deploy/images/tools.env, so
# that the unit runs under the systemd 239 of that release, and checks:
#   - the signature: `rpm -K` refuses the package until the key is imported, passes after, refuses a changed byte, and `dnf` with
#     localpkg_gpgcheck refuses the unsigned package (dnf does not check a local file's signature unless it is told to);
#   - the dependency on audit is real: dnf refuses the package until `audit` is there;
#   - the install: the user and group, the files with their owners and modes, `rpm -V` clean, the audisp directory made by tmpfiles;
#   - the unit: `systemd-analyze verify` on systemd 239 prints nothing (it prints "Unknown lvalue" for a directive 239 does not know, and
#     exits 0 all the same, so the output is what is checked: this is how "every directive is supported on 239" is shown);
#   - `systemctl start` runs the packaged binary under every restriction (it is a stub today and exits at once: plan 09 A18), and a probe
#     in place of the binary, started through the same unit, shows that the restrictions are really on the process: no capability, no new
#     privileges, a system call filter, a read-only /etc, a writable spool, no home directories;
#   - removal and reinstall.
#
# --audit repo      installs the real `audit` package with dnf, from the release's repositories (CI).
# --audit stand-in  installs tests/audit-stand-in.spec instead, a package that only provides `audit` and its directories. It is for a
#                   place that cannot reach the Rocky repositories (plan 09 blockers, B13), says so loudly, and tests nothing about
#                   the real audit package. The workflow never uses it.
#
# Needs Docker, and the tools image of build.sh --tools for the stand-in. The containers are --privileged, because systemd must be able to
# set up the namespaces and the system call filter that the unit asks for; they run only this package and have no network (stand-in).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$root/deploy/images/tools.env"

usage() { sed -n '2,/^set -euo/p' "$0" | sed -n '/^#   deploy/,/^# Needs/p' | sed 's/^# \{0,1\}//' | head -n -1 >&2; exit 2; }
rpm_file="" unsigned="" pub="" audit="" tools="${LK_RPM_TOOLS_IMAGE:-lanekeeper-rpm-tools}"
releases=()
while [ $# -gt 0 ]; do
    case "$1" in
        --rpm) [ $# -ge 2 ] || usage; rpm_file="$2"; shift 2 ;;
        --unsigned) [ $# -ge 2 ] || usage; unsigned="$2"; shift 2 ;;
        --public-key) [ $# -ge 2 ] || usage; pub="$2"; shift 2 ;;
        --audit) [ $# -ge 2 ] || usage; audit="$2"; shift 2 ;;
        --tools) [ $# -ge 2 ] || usage; tools="$2"; shift 2 ;;
        8.9 | 8.10) releases+=("$1"); shift ;;
        *) usage ;;
    esac
done
[ ${#releases[@]} -gt 0 ] || releases=(8.9 8.10)
[ -f "$rpm_file" ] && [ -f "$unsigned" ] && [ -f "$pub" ] || usage
case "$audit" in repo | stand-in) ;; *) usage ;; esac
image_of() { case "$1" in 8.9) echo "$LK_ROCKY_8_9_IMAGE" ;; 8.10) echo "$LK_ROCKY_8_10_IMAGE" ;; esac; }

work="$(mktemp -d)"
containers=()
cleanup() {
    local c
    for c in ${containers[@]+"${containers[@]}"}; do docker rm -f "$c" >/dev/null 2>&1 || true; done
    rm -rf "$work"
}
trap cleanup EXIT

rpm_name="$(basename "$rpm_file")"
mkdir "$work/t" "$work/tampered"
cp "$rpm_file" "$work/t/$rpm_name"
cp "$pub" "$work/t/public.key"
cp "$unsigned" "$work/t/unsigned.rpm"
cp "$root/deploy/sentinel/lanekeeper-sentinel.service" "$work/t/unit.service"
python3 -I - "$rpm_file" "$work/t/tampered.rpm" <<'PY'
import sys
data = bytearray(open(sys.argv[1], 'rb').read())
data[-100] ^= 0xFF  # a byte of the payload
open(sys.argv[2], 'wb').write(data)
PY
if [ "$audit" = stand-in ]; then
    docker image inspect "$tools" >/dev/null 2>&1 || { echo "install.sh: no image $tools (it builds the stand-in); make it with: deploy/sentinel/build.sh --tools" >&2; exit 2; }
    docker run --rm --network none -e SOURCE_DATE_EPOCH=1793000000 -v "$here/audit-stand-in.spec:/spec/audit.spec:ro" -v "$work/t:/out" "$tools" \
        sh -c 'mkdir -p /b && rpmbuild -bb --define "_topdir /b" /spec/audit.spec >/dev/null 2>&1 && cp /b/RPMS/noarch/audit-*.rpm /out/audit-stand-in.rpm'
    echo "install.sh: AUDIT STAND-IN IN USE. The package 'audit' below is tests/audit-stand-in.spec, not the real one, because the Rocky repositories cannot be reached from here (plan 09 blockers, B13). The real audit package is tested by CI (--audit repo)."
fi

fails=0
ok() { echo "  ok  $1"; }
bad() { echo "  FAILED  $1" >&2; fails=$((fails + 1)); }

for release in "${releases[@]}"; do
    image="$(image_of "$release")"
    name="lk-install-${release//./-}-$$"
    echo "== Rocky Linux $release ($image)"
    net=(--network none)
    [ "$audit" = repo ] && net=()
    docker run -d --name "$name" --privileged ${net[@]+"${net[@]}"} --tmpfs /run --tmpfs /tmp -v "$work/t:/t:ro" "$image" /usr/sbin/init >/dev/null
    containers+=("$name")
    state=""
    for _ in $(seq 1 60); do
        state="$(docker exec "$name" systemctl is-system-running 2>&1 || true)"
        case "$state" in running | degraded) break ;; esac
        sleep 1
    done
    case "$state" in running | degraded) ok "systemd $(docker exec "$name" systemctl --version | head -n1 | cut -d' ' -f1-3) is up ($state)" ;; *) bad "systemd did not come up: $state"; continue ;; esac

    # check NAME SHELL: the snippet runs in the container and must exit 0 (its output is shown when it does not).
    check() {
        local out
        if out="$(docker exec "$name" sh -c "$2" 2>&1)"; then ok "$1"; else echo "$out" >&2; bad "$1"; fi
    }
    # refuse NAME FRAGMENT SHELL: the snippet must exit non-zero and say FRAGMENT.
    refuse() {
        local out
        if out="$(docker exec "$name" sh -c "$3" 2>&1)"; then
            bad "$1: it succeeded"
        elif ! grep -qF -- "$2" <<<"$out"; then
            bad "$1: it failed, but not for '$2':"$'\n'"$out"
        else
            ok "$1"
        fi
    }

    echo "-- the signature"
    refuse "rpm -K refuses the package while the key is unknown" "NOT OK" "rpm -K /t/$rpm_name"
    check "rpm --import takes the public key" "rpm --import /t/public.key"
    check "rpm -K verifies the package with the key" "rpm -K /t/$rpm_name | grep -q 'digests signatures OK'"
    refuse "rpm -K refuses a package with a changed byte" "NOT OK" "rpm -K /t/tampered.rpm"

    echo "-- the dependency on audit"
    if [ "$audit" = stand-in ]; then
        refuse "dnf refuses the package while there is no audit" "nothing provides audit" "dnf -y --disablerepo='*' --setopt=localpkg_gpgcheck=1 install /t/$rpm_name"
        check "the audit stand-in is installed" "rpm -i /t/audit-stand-in.rpm"
        repo_args="--disablerepo='*'"
    else
        refuse "dnf refuses the package while there is no audit" "audit" "dnf -y --disablerepo='*' --setopt=localpkg_gpgcheck=1 install /t/$rpm_name"
        check "the real audit package is installed from the release's repositories" "dnf -y install audit"
        repo_args=""
    fi

    echo "-- dnf checks the signature of a local file when it is told to"
    refuse "dnf refuses the unsigned package (localpkg_gpgcheck)" "is not signed" "dnf -y $repo_args --setopt=localpkg_gpgcheck=1 install /t/unsigned.rpm"
    refuse "dnf refuses a package with a changed byte" "GPG check FAILED" "dnf -y $repo_args --setopt=localpkg_gpgcheck=1 install /t/tampered.rpm"

    echo "-- the install"
    check "dnf installs the signed package (signature checked by dnf itself)" "dnf -y $repo_args --setopt=localpkg_gpgcheck=1 install /t/$rpm_name"
    check "the user is a system account that cannot log in" "u=\$(getent passwd lanekeeper-sentinel) && [ \"\$(echo \"\$u\" | cut -d: -f3)\" -lt 1000 ] && echo \"\$u\" | grep -q ':/sbin/nologin\$' && echo \"\$u\" | grep -q ':/var/lib/lanekeeper-sentinel:'"
    check "its group exists" "getent group lanekeeper-sentinel"
    check "the files are where S25 and A17 put them, with their owners and modes" "
        test \"\$(stat -c '%a %U:%G' /usr/bin/lanekeeper-sentinel)\" = '755 root:root' &&
        test \"\$(stat -c '%a %U:%G' /usr/lib/systemd/system/lanekeeper-sentinel.service)\" = '644 root:root' &&
        test \"\$(stat -c '%a %U:%G' /etc/audit/plugins.d/lanekeeper-af_unix.conf)\" = '640 root:root' &&
        test \"\$(stat -c '%a %U:%G' /usr/lib/tmpfiles.d/lanekeeper-sentinel.conf)\" = '644 root:root' &&
        test \"\$(stat -c '%a %U:%G' /usr/share/lanekeeper-sentinel/audit/lanekeeper.rules.in)\" = '644 root:root' &&
        test \"\$(stat -c '%a %U:%G' /var/lib/lanekeeper-sentinel)\" = '700 lanekeeper-sentinel:lanekeeper-sentinel' &&
        test \"\$(stat -c '%a %U:%G' /run/lanekeeper-audisp)\" = '750 root:lanekeeper-sentinel'"
    check "rpm -V finds nothing changed" "test -z \"\$(rpm -V lanekeeper-sentinel)\""
    check "the binary is static" "! (ldd /usr/bin/lanekeeper-sentinel 2>&1 | grep -q 'libc.so') && (ldd /usr/bin/lanekeeper-sentinel 2>&1 | grep -Eq 'not a dynamic executable|statically linked')"
    check "the package does not enable or start the unit" "test \"\$(systemctl is-enabled lanekeeper-sentinel.service)\" = disabled && ! systemctl is-active lanekeeper-sentinel.service >/dev/null 2>&1"
    check "the plugin's socket is in the directory that tmpfiles makes" "
        conf=/etc/audit/plugins.d/lanekeeper-af_unix.conf
        sock=\$(sed -n 's/^args = 0666 //p' \$conf) && [ -n \"\$sock\" ] &&
        grep -q \"^d \$(dirname \$sock) 0750 root lanekeeper-sentinel -\$\" /usr/lib/tmpfiles.d/lanekeeper-sentinel.conf &&
        grep -q '^path = /sbin/audisp-af_unix\$' \$conf && grep -q '^active = yes\$' \$conf"
    check "the audit rule template has one placeholder, in one rule" "
        t=/usr/share/lanekeeper-sentinel/audit/lanekeeper.rules.in
        test \"\$(grep -v '^#' \$t | grep -c '@EXPORT_ROOT@')\" = 1 &&
        test \"\$(grep -v '^#' \$t | sed 's#@EXPORT_ROOT@#/export/csp-configuration#')\" = '-w /export/csp-configuration -p wa -k lanekeeper'"

    echo "-- the unit under the systemd of Rocky Linux $release"
    check "systemd-analyze verify prints nothing: every directive is known" "out=\$(systemd-analyze verify /usr/lib/systemd/system/lanekeeper-sentinel.service 2>&1); [ -z \"\$out\" ] || { echo \"\$out\"; exit 1; }"
    check "the unit is the one in the repository" "test \"\$(sha256sum < /usr/lib/systemd/system/lanekeeper-sentinel.service)\" = \"\$(sha256sum < /t/unit.service)\""
    check "systemctl start runs the packaged binary under the restrictions" "
        systemctl daemon-reload && systemctl start lanekeeper-sentinel.service
        for i in \$(seq 1 20); do [ \"\$(systemctl show -p ActiveState --value lanekeeper-sentinel.service)\" = inactive ] && break; sleep 0.5; done
        test \"\$(systemctl show -p Result --value lanekeeper-sentinel.service)\" = success &&
        test \"\$(systemctl show -p ExecMainStatus --value lanekeeper-sentinel.service)\" = 0 &&
        test \"\$(systemctl show -p NRestarts --value lanekeeper-sentinel.service)\" = 0 &&
        sleep 1 && journalctl -u lanekeeper-sentinel.service -o cat --no-pager | grep -q '^Started Lanekeeper NFS VM sentinel' &&
        journalctl -u lanekeeper-sentinel.service -o cat --no-pager | grep -q 'lanekeeper-sentinel.service: Succeeded'"
    check "systemctl show reports the restrictions as the unit sets them" "
        s() { systemctl show -p \"\$1\" --value lanekeeper-sentinel.service; }
        test \"\$(s User)\" = lanekeeper-sentinel && test \"\$(s CapabilityBoundingSet)\" = '' && test \"\$(s AmbientCapabilities)\" = '' &&
        test \"\$(s NoNewPrivileges)\" = yes && test \"\$(s ProtectSystem)\" = strict && test \"\$(s ProtectHome)\" = yes &&
        test \"\$(s PrivateTmp)\" = yes && test \"\$(s MemoryDenyWriteExecute)\" = yes && test \"\$(s RestrictSUIDSGID)\" = yes &&
        test \"\$(s ReadWritePaths)\" = /var/lib/lanekeeper-sentinel"
    # A probe in place of the binary, through the same unit: what the restrictions do to a real process. The drop-in changes ExecStart
    # only; it is removed afterwards.
    docker exec -i "$name" sh -c 'cat > /usr/local/bin/lk-probe.sh && chmod 0755 /usr/local/bin/lk-probe.sh' <<'PROBE'
#!/bin/sh
grep -E '^(Uid|CapInh|CapPrm|CapEff|CapBnd|CapAmb|NoNewPrivs|Seccomp):' /proc/self/status
if (echo x > /etc/lk-probe) 2>/dev/null; then echo "WRITE-ETC=allowed"; else echo "WRITE-ETC=refused"; fi
if (echo x > /usr/lk-probe) 2>/dev/null; then echo "WRITE-USR=allowed"; else echo "WRITE-USR=refused"; fi
if (echo x > /var/lib/lanekeeper-sentinel/lk-probe) 2>/dev/null; then echo "WRITE-SPOOL=allowed"; rm -f /var/lib/lanekeeper-sentinel/lk-probe; else echo "WRITE-SPOOL=refused"; fi
# A directory the sentinel's user owns, outside the spool: only ProtectSystem=strict (not file permissions) keeps it from being written.
if (echo x > /var/lib/lk-probe-other/f) 2>/dev/null; then echo "WRITE-OTHER=allowed"; else echo "WRITE-OTHER=refused"; fi
# nice(2) is in @resources, which the system call filter refuses: the process is stopped by the filter (SIGSYS), and the shell says no.
if (nice -n 1 true) 2>/dev/null; then echo "NICE=allowed"; else echo "NICE=refused"; fi
echo "HOME-ENTRIES=$(ls -A /home /root 2>/dev/null | wc -l)"
echo "TMP-ENTRIES=$(ls -A /tmp 2>/dev/null | wc -l)"
echo "DEV-ENTRIES=$(ls -A /dev | sort | tr '\n' ' ')"
PROBE
    docker exec "$name" sh -c 'install -d -o lanekeeper-sentinel -g lanekeeper-sentinel -m 0755 /var/lib/lk-probe-other'
    docker exec "$name" sh -c 'mkdir -p /etc/systemd/system/lanekeeper-sentinel.service.d && printf "[Service]\nExecStart=\nExecStart=/usr/local/bin/lk-probe.sh\n" > /etc/systemd/system/lanekeeper-sentinel.service.d/probe.conf && systemctl daemon-reload && systemctl start lanekeeper-sentinel.service; for i in $(seq 1 20); do [ "$(systemctl show -p ActiveState --value lanekeeper-sentinel.service)" = inactive ] && break; sleep 0.5; done; sleep 1; journalctl -u lanekeeper-sentinel.service -o cat --no-pager > /tmp/probe.out 2>&1; rm -rf /etc/systemd/system/lanekeeper-sentinel.service.d /usr/local/bin/lk-probe.sh /var/lib/lk-probe-other; systemctl daemon-reload' || true
    probe="$(docker exec "$name" cat /tmp/probe.out 2>/dev/null || true)"
    probe_has() { grep -Eq -- "$1" <<<"$probe"; }
    probe_has '^Uid:' || { echo "$probe" >&2; bad "the probe printed nothing: the unit did not run it"; }
    uid="$(docker exec "$name" id -u lanekeeper-sentinel)"
    if probe_has "^Uid:[[:space:]]+${uid}[[:space:]]+${uid}[[:space:]]+${uid}[[:space:]]+${uid}\$"; then ok "the process runs as the sentinel's user ($uid), not root"; else echo "$probe" >&2; bad "the process does not run as uid $uid"; fi
    for cap in CapInh CapPrm CapEff CapBnd CapAmb; do
        if probe_has "^${cap}:[[:space:]]+0000000000000000\$"; then ok "$cap is empty"; else echo "$probe" >&2; bad "$cap is not empty"; fi
    done
    if probe_has '^NoNewPrivs:[[:space:]]+1$'; then ok "NoNewPrivs is 1"; else bad "NoNewPrivs is not 1"; fi
    if probe_has '^Seccomp:[[:space:]]+2$'; then ok "a system call filter is on (Seccomp: 2)"; else bad "no system call filter on the process"; fi
    if probe_has '^WRITE-ETC=refused$' && probe_has '^WRITE-USR=refused$'; then ok "/etc and /usr are read-only"; else echo "$probe" >&2; bad "the process can write to /etc or /usr"; fi
    if probe_has '^WRITE-SPOOL=allowed$'; then ok "the spool is writable"; else echo "$probe" >&2; bad "the process cannot write to its spool"; fi
    if probe_has '^WRITE-OTHER=refused$'; then ok "a directory the user owns outside the spool is read-only (ProtectSystem=strict)"; else echo "$probe" >&2; bad "the process can write outside its spool"; fi
    if probe_has '^NICE=refused$'; then ok "the system call filter refuses a call of @resources"; else echo "$probe" >&2; bad "the system call filter lets a call of @resources through"; fi
    if probe_has '^HOME-ENTRIES=0$'; then ok "/home and /root show nothing"; else echo "$probe" >&2; bad "/home or /root are visible"; fi
    if probe_has '^TMP-ENTRIES=0$'; then ok "/tmp is private (empty)"; else echo "$probe" >&2; bad "/tmp is the host's"; fi
    if probe_has '^DEV-ENTRIES=.*null' && ! probe_has '^DEV-ENTRIES=.*(sda|vda|tty0|mem)'; then ok "/dev is a private one with the pseudo devices only"; else echo "$probe" >&2; bad "/dev is not private"; fi

    echo "-- removal and reinstall"
    check "dnf removes the package, and its files go" "dnf -y remove lanekeeper-sentinel && ! test -e /usr/bin/lanekeeper-sentinel && ! test -e /usr/lib/systemd/system/lanekeeper-sentinel.service && ! test -e /etc/audit/plugins.d/lanekeeper-af_unix.conf"
    check "the user and group stay (the spool may still be theirs)" "getent passwd lanekeeper-sentinel && getent group lanekeeper-sentinel"
    check "dnf installs the package again" "dnf -y $repo_args --setopt=localpkg_gpgcheck=1 install /t/$rpm_name && test -z \"\$(rpm -V lanekeeper-sentinel)\""
done

if [ "$fails" -ne 0 ]; then echo "install.sh: $fails check(s) failed" >&2; exit 1; fi
echo "install.sh: the package installs, verifies and starts on Rocky Linux ${releases[*]}"
