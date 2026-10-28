#!/usr/bin/env bash
# Installs Playwright's Chromium and the operating-system packages it needs (the fonts and a few libraries) on a GitHub-hosted
# Ubuntu runner, and does not let one stalled download fail the job. Run it from web/ (the workflow's `working-directory`): the
# command is the pinned Playwright's own, found by `pnpm exec`.
#
# Why this exists: `playwright install --with-deps` runs `apt-get install` for the fonts (fonts-freefont-ttf, fonts-unifont,
# fonts-wqy-zenhei, ...) and libraries that the runner image lacks. The Ubuntu mirror of the runner (azure.archive.ubuntu.com) now
# and then stalls on one of those downloads. apt's default is to wait minutes for data and then ignore the file, so the step ran to
# its limit and was killed before any test started, while the shards that did not hit a stall took about 20 seconds. Three defences:
#
#   1. An apt config file that gives each download 30 seconds without data and 5 retries, so a stall is abandoned and retried in
#      seconds. (/etc/apt/apt.conf.d/ is read by every later apt command, in this job only: the runner is thrown away after it.)
#   2. A bounded retry of the whole install: 3 attempts, 10 seconds apart.
#   3. A hard limit on each attempt (240 seconds; a good run takes about 20). Defence 1 is not enough: on 2026-10-26 (web-e2e run
#      40398723421, shard 3/5) `apt-get update` printed its last line and then nothing for 19 minutes with the 30 s timeout in
#      force, until the step's limit ended the job. An attempt that runs past its limit is stopped with `timeout` (SIGTERM, then
#      SIGKILL 20 s later). That is safe in the stalls seen so far, which are in the index update and the downloads and not in
#      dpkg; if dpkg was cut off all the same, `dpkg --configure -a` repairs the package database before the next attempt. From
#      the second attempt on, apt reads archive.ubuntu.com directly and not the runner's mirror list (azure.archive.ubuntu.com),
#      so the retry does not meet the same stalled mirror. Not shown by anything here: that this cures the stall (the tests use
#      stubs; only a CI run on a stalled runner can show it), and that sudo relays SIGTERM to a root apt-get as it normally does.
#
# The fonts stay: the visual tests compare screenshots, and the font set decides how the text is drawn.
#
# The defaults are the production values. The environment variables below exist for the tests of this script
# (playwright-install.test.sh); the workflow sets none of them.
#
#   PLAYWRIGHT_INSTALL_CMD       the install command, split on spaces (no quoting): pnpm exec playwright install --with-deps chromium
#   PLAYWRIGHT_INSTALL_ATTEMPTS  how many times to try, a positive integer: 3
#   PLAYWRIGHT_INSTALL_PAUSE     seconds between two attempts, a non-negative integer: 10
#   PLAYWRIGHT_INSTALL_LIMIT     seconds an attempt may run before it is stopped, a positive integer: 240
#   PLAYWRIGHT_INSTALL_TIMEOUT   the command that enforces the limit (called as: TIMEOUT --kill-after=20 LIMIT COMMAND...): timeout
#   PLAYWRIGHT_INSTALL_SOURCES   the apt sources files to point at archive.ubuntu.com on a retry, split on spaces:
#                                /etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list
#   PLAYWRIGHT_INSTALL_APT_CONF  the apt config file to write: /etc/apt/apt.conf.d/80-playwright-retries
#   PLAYWRIGHT_INSTALL_SUDO      the command that runs `tee` with the rights to write that file: sudo
set -euo pipefail

install_cmd=${PLAYWRIGHT_INSTALL_CMD:-pnpm exec playwright install --with-deps chromium}
attempts=${PLAYWRIGHT_INSTALL_ATTEMPTS:-3}
pause=${PLAYWRIGHT_INSTALL_PAUSE:-10}
limit=${PLAYWRIGHT_INSTALL_LIMIT:-240}
timeout_cmd=${PLAYWRIGHT_INSTALL_TIMEOUT:-timeout}
sources=${PLAYWRIGHT_INSTALL_SOURCES:-/etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list}
apt_conf=${PLAYWRIGHT_INSTALL_APT_CONF:-/etc/apt/apt.conf.d/80-playwright-retries}
sudo_cmd=${PLAYWRIGHT_INSTALL_SUDO:-sudo}

is_number() { # a string of digits, and nothing else
  case $1 in
    '' | *[!0-9]*) return 1 ;;
    *) return 0 ;;
  esac
}

if ! is_number "$attempts" || [ "$attempts" -lt 1 ]; then
  echo "playwright-install: PLAYWRIGHT_INSTALL_ATTEMPTS must be a positive integer, not '$attempts'" >&2
  exit 2
fi
if ! is_number "$pause"; then
  echo "playwright-install: PLAYWRIGHT_INSTALL_PAUSE must be a non-negative integer, not '$pause'" >&2
  exit 2
fi

if ! is_number "$limit" || [ "$limit" -lt 1 ]; then
  echo "playwright-install: PLAYWRIGHT_INSTALL_LIMIT must be a positive integer, not '$limit'" >&2
  exit 2
fi

read -r -a install_words <<<"$install_cmd"
read -r -a sudo_words <<<"$sudo_cmd"
read -r -a timeout_words <<<"$timeout_cmd"
read -r -a source_files <<<"$sources"

# Points apt at archive.ubuntu.com instead of the runner's mirror list. Lines that do not name the mirror list are left alone, and a
# file that is missing is skipped; the original is kept next to it as .lk-orig (sed -i with a suffix works with GNU and BSD sed).
use_archive_mirror() {
  local f
  for f in "${source_files[@]}"; do
    [ -f "$f" ] || continue
    "${sudo_words[@]}" sed -i.lk-orig 's#mirror+file:/etc/apt/apt-mirrors\.txt#http://archive.ubuntu.com/ubuntu/#' "$f" || true
  done
  echo "playwright-install: apt now reads archive.ubuntu.com directly, not the runner's mirror list"
}

# Overwrites the file each time (tee, not tee -a), so a second run on the same runner leaves the same three lines. A failure to
# write it is reported and not fatal: the install worked without it before, and only the speed of a stall's recovery is at stake.
if printf '%s\n' 'Acquire::Retries "5";' 'Acquire::http::Timeout "30";' 'Acquire::https::Timeout "30";' |
  "${sudo_words[@]}" tee "$apt_conf" >/dev/null; then
  echo "playwright-install: wrote $apt_conf (apt: 5 retries, 30 s without data)"
else
  echo "playwright-install: warning: could not write $apt_conf; the install runs with apt's default timeouts" >&2
fi

attempt=1
while true; do
  echo "playwright-install: attempt $attempt of $attempts"
  status=0
  "${timeout_words[@]}" --kill-after=20 "$limit" "${install_words[@]}" || status=$?
  if [ "$status" -eq 0 ]; then
    exit 0
  fi
  timed_out=0
  case $status in
    124 | 137) timed_out=1 ;; # timeout's own code, and SIGKILL after --kill-after
  esac
  if [ "$timed_out" -eq 1 ]; then
    echo "playwright-install: attempt $attempt of $attempts timed out after $limit s and was stopped (exit code $status)" >&2
    # Best effort: if dpkg was cut off, leave the package database usable for the next attempt.
    "${sudo_words[@]}" dpkg --configure -a || echo "playwright-install: warning: dpkg --configure -a failed" >&2
  else
    echo "playwright-install: attempt $attempt of $attempts failed with exit code $status" >&2
  fi
  if [ "$attempt" -ge "$attempts" ]; then
    echo "playwright-install: giving up after $attempts attempts; the last exit code was $status. The Playwright browser install did not complete." >&2
    exit "$status"
  fi
  echo "playwright-install: trying again in $pause s"
  sleep "$pause"
  attempt=$((attempt + 1))
  if [ "$attempt" -eq 2 ]; then
    use_archive_mirror
  fi
done
