#!/usr/bin/env bash
# Runs a command as an unprivileged user, the way a GitHub-hosted runner runs it (09, CI first run).
#
#   deploy/tests/as-user.sh COMMAND [ARG...]
#
# A runner's user is `runner` (uid 1001), not root, and it owns the checkout. A container that this user starts with `--user $(id -u)`, or
# that drops capabilities, meets permission checks that a root shell never meets: the images of grype and syft have a /tmp that only root
# can write, and root without CAP_DAC_OVERRIDE cannot write a directory that the runner owns. Every one of the three failures of the first
# CI run was invisible in a sandbox whose shell is root, so the tests of the scripts that start such containers step down first:
#
#   - Started as a user other than root, this runs COMMAND as it is. That is the CI runner, and a developer's own shell.
#   - Started as root, it runs COMMAND as LK_AS_UID:LK_AS_GID (default 1001:1001, the runner's) with the group that owns the Docker socket,
#     so `docker` works, and with a HOME that user owns (a scratch directory, removed afterwards), git told to trust the checkout (which
#     root owns here), and LK_AS_USER=1 in the environment. The rest of the environment, the proxy variables and LK_BUILD_* among them, is
#     kept. The current directory must be readable by that user; the repository's target/ is not writable by it, so a test that writes
#     there takes its own directory from HOME.
#
# Needs setpriv (util-linux) when it starts as root.
set -euo pipefail

usage() { echo "usage: $0 COMMAND [ARG...]" >&2; exit 2; }
[ $# -ge 1 ] || usage
if [ "$(id -u)" -ne 0 ]; then
    exec "$@"
fi

uid="${LK_AS_UID:-1001}"
gid="${LK_AS_GID:-1001}"
[[ "$uid" =~ ^[1-9][0-9]*$ && "$gid" =~ ^[1-9][0-9]*$ ]] || { echo "as-user.sh: LK_AS_UID and LK_AS_GID must be numbers other than 0 ($uid:$gid)" >&2; exit 2; }
command -v setpriv >/dev/null 2>&1 || { echo "as-user.sh: setpriv (util-linux) is needed to step down from root" >&2; exit 1; }

# The group that owns the Docker socket, so that the user can use the daemon the way the runner user (a member of `docker`) does.
socket="${DOCKER_HOST:-unix:///var/run/docker.sock}"
groups="$gid"
if [[ "$socket" == unix://* ]] && [ -S "${socket#unix://}" ]; then
    groups="$gid,$(stat -c %g "${socket#unix://}")"
fi

home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
chown "$uid:$gid" "$home"

status=0
setpriv --reuid "$uid" --regid "$gid" --groups "$groups" \
    env HOME="$home" LK_AS_USER=1 \
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0='*' \
    "$@" || status=$?
exit "$status"
