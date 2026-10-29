#!/usr/bin/env bash
# Tests for playwright-install.sh. Plain bash, no dependencies; every case runs the script against stubs (a stub install command, a
# stub sudo, a stub sleep), so nothing here touches apt, the network or /etc.
#
#   bash .github/scripts/playwright-install.test.sh [path/to/script-under-test]
#
# Exits non-zero if any case fails. The optional argument is for showing a case red against a script that lacks the behaviour.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
script=${1:-$here/playwright-install.sh}
work_root=$(mktemp -d)
trap 'rm -rf "$work_root"' EXIT

# The caller's environment must not leak into the script under test.
unset PLAYWRIGHT_INSTALL_CMD PLAYWRIGHT_INSTALL_ATTEMPTS PLAYWRIGHT_INSTALL_PAUSE PLAYWRIGHT_INSTALL_APT_CONF PLAYWRIGHT_INSTALL_SUDO \
  PLAYWRIGHT_INSTALL_LIMIT PLAYWRIGHT_INSTALL_TIMEOUT PLAYWRIGHT_INSTALL_SOURCES

failures=0
checks=0

ok() { checks=$((checks + 1)); }
bad() {
  checks=$((checks + 1))
  failures=$((failures + 1))
  printf '  FAIL: %s\n' "$1"
}

expect_eq() { # description expected actual
  if [ "$2" = "$3" ]; then ok; else bad "$1: expected '$2', got '$3'"; fi
}

expect_contains() { # description text needle
  case $2 in *"$3"*) ok ;; *) bad "$1: expected to find '$3' in: $(printf '%s' "$2" | tr '\n' '|')" ;; esac
}

expect_not_contains() { # description text needle
  case $2 in *"$3"*) bad "$1: did not expect '$3' in: $(printf '%s' "$2" | tr '\n' '|')" ;; *) ok ;; esac
}

expect_nonzero() { # description status
  if [ "$2" -ne 0 ]; then ok; else bad "$1: expected a non-zero exit status, got $2"; fi
}

lines_in() { # number of lines in a file, 0 if it does not exist
  if [ -f "$1" ]; then wc -l <"$1" | tr -d ' '; else echo 0; fi
}

slurp() { # content of a file, empty if it does not exist
  if [ -f "$1" ]; then cat "$1"; fi
}

joined() { # the lines of a file on one line, separated by single spaces
  slurp "$1" | tr '\n' ' ' | sed 's/ $//'
}

# A fresh directory per case with: bin/sleep (records its argument, returns at once), bin/sudo-stub (records its arguments and runs
# them), stub-install.sh (fails the first N times it runs, with the given exit code, then succeeds) and a cwd to run the script in.
case_dir=""
new_case() {
  case_dir=$(mktemp -d "$work_root/case.XXXXXX")
  mkdir -p "$case_dir/bin" "$case_dir/cwd"
  cat >"$case_dir/bin/sleep" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$(dirname "$0")/../sleeps"
EOF
  cat >"$case_dir/bin/sudo-stub" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$(dirname "$0")/../sudo-args"
exec "$@"
EOF
  cat >"$case_dir/stub-install.sh" <<'EOF'
#!/usr/bin/env bash
# usage: stub-install.sh COUNT_FILE FAILURES_BEFORE_SUCCESS|always EXIT_CODE
pwd -P >>"$1.cwd"
echo run >>"$1"
runs=$(wc -l <"$1" | tr -d ' ')
if [ "$2" = always ] || [ "$runs" -le "$2" ]; then
  echo "stub-install: run $runs fails"
  exit "$3"
fi
echo "stub-install: run $runs succeeds"
EOF
  # dpkg is stubbed so that a repair after a timeout never reaches the real one (the sudo stub runs its arguments through PATH).
  cat >"$case_dir/bin/dpkg" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$(dirname "$0")/../dpkg-args"
EOF
  # The apt sources file that the script may rewrite on a retry, in the shape of the runner's: the mirror list.
  printf '%s\n' 'Types: deb' 'URIs: mirror+file:/etc/apt/apt-mirrors.txt' 'Suites: noble' >"$case_dir/ubuntu.sources"
  chmod +x "$case_dir/bin/sleep" "$case_dir/bin/sudo-stub" "$case_dir/bin/dpkg" "$case_dir/stub-install.sh"
}

# Runs the script under test in $case_dir/cwd with the stubs in place; extra NAME=value arguments are added to its environment.
# Sets $status and $out (stdout and stderr together).
status=0
out=""
run_script() {
  (
    cd "$case_dir/cwd" &&
      env PATH="$case_dir/bin:$PATH" \
        PLAYWRIGHT_INSTALL_SUDO="$case_dir/bin/sudo-stub" \
        PLAYWRIGHT_INSTALL_APT_CONF="$case_dir/80-playwright-retries" \
        PLAYWRIGHT_INSTALL_SOURCES="$case_dir/ubuntu.sources" \
        "$@" bash "$script"
  ) >"$case_dir/out" 2>&1
  status=$?
  out=$(cat "$case_dir/out")
}

stub_cmd() { # failures-before-success exit-code
  printf 'bash %s/stub-install.sh %s/count %s %s' "$case_dir" "$case_dir" "$1" "$2"
}

# --- Cases -----------------------------------------------------------------------------------------------------------------------

case_first_attempt_succeeds() {
  echo "case: the install succeeds first time"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 0 1)"
  expect_eq "exit status" 0 "$status"
  expect_eq "install runs" 1 "$(lines_in "$case_dir/count")"
  expect_contains "announces the attempt" "$out" "playwright-install: attempt 1 of 3"
  expect_not_contains "no second attempt" "$out" "attempt 2 of 3"
  expect_eq "no pause when nothing failed" 0 "$(lines_in "$case_dir/sleeps")"
  expect_eq "runs in the current directory" "$(cd "$case_dir/cwd" && pwd -P)" "$(slurp "$case_dir/count.cwd" | head -n 1)"
}

case_fails_twice_then_succeeds() {
  echo "case: the install fails twice, then succeeds"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 2 7)" PLAYWRIGHT_INSTALL_PAUSE=4
  expect_eq "exit status" 0 "$status"
  expect_eq "install runs" 3 "$(lines_in "$case_dir/count")"
  expect_contains "attempt 1" "$out" "playwright-install: attempt 1 of 3"
  expect_contains "attempt 2" "$out" "playwright-install: attempt 2 of 3"
  expect_contains "attempt 3" "$out" "playwright-install: attempt 3 of 3"
  expect_contains "reports the exit code of a failed attempt" "$out" "exit code 7"
  expect_eq "pauses between the attempts, with the configured length" "4 4" "$(joined "$case_dir/sleeps")"
}

case_always_fails() {
  echo "case: the install always fails"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd always 7)" PLAYWRIGHT_INSTALL_PAUSE=4
  expect_nonzero "exit status" "$status"
  expect_eq "exit status is the last exit code" 7 "$status"
  expect_eq "exactly three runs" 3 "$(lines_in "$case_dir/count")"
  expect_not_contains "no fourth attempt" "$out" "attempt 4"
  expect_contains "the final message names the last exit code" "$out" "giving up after 3 attempts"
  expect_contains "the final message names the exit code" "$out" "last exit code was 7"
  expect_eq "no pause after the last attempt" 2 "$(lines_in "$case_dir/sleeps")"
}

case_attempts_and_command_are_overridable() {
  echo "case: the attempt count is overridable"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd always 3)" PLAYWRIGHT_INSTALL_ATTEMPTS=5
  expect_eq "exit status" 3 "$status"
  expect_eq "five runs" 5 "$(lines_in "$case_dir/count")"
  expect_contains "announces five" "$out" "playwright-install: attempt 5 of 5"
}

case_apt_config_is_written() {
  echo "case: the apt config is written, through sudo, before the install runs"
  new_case
  # The stub install fails (exit 9) unless the config is already in place when it starts.
  cat >"$case_dir/needs-conf.sh" <<'EOF'
#!/usr/bin/env bash
grep -q '^Acquire::Retries "5";$' "$1" || exit 9
EOF
  run_script PLAYWRIGHT_INSTALL_CMD="bash $case_dir/needs-conf.sh $case_dir/80-playwright-retries" PLAYWRIGHT_INSTALL_ATTEMPTS=1
  expect_eq "exit status (the config existed when the install started)" 0 "$status"
  expected=$(printf '%s\n' 'Acquire::Retries "5";' 'Acquire::http::Timeout "30";' 'Acquire::https::Timeout "30";')
  expect_eq "file content" "$expected" "$(slurp "$case_dir/80-playwright-retries")"
  expect_eq "written with sudo tee" "tee $case_dir/80-playwright-retries" "$(slurp "$case_dir/sudo-args")"
}

case_apt_config_is_overwritten() {
  echo "case: re-running overwrites the apt config instead of appending"
  new_case
  printf '%s\n' 'Acquire::Retries "1";' 'Something::Else "stale";' >"$case_dir/80-playwright-retries"
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 0 1)"
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 0 1)"
  expect_eq "exit status" 0 "$status"
  expect_eq "three lines after two runs over a stale file" 3 "$(lines_in "$case_dir/80-playwright-retries")"
  expect_not_contains "stale content is gone" "$(slurp "$case_dir/80-playwright-retries")" "stale"
  expect_eq "one Retries line" 1 "$(slurp "$case_dir/80-playwright-retries" | grep -c '^Acquire::Retries')"
}

case_production_defaults() {
  echo "case: the defaults are the production command, 3 attempts and a 10 s pause"
  new_case
  # A fake pnpm that records its arguments and always fails; only the sudo and the config path are overridden.
  cat >"$case_dir/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$(dirname "$0")/../pnpm-args"
exit 5
EOF
  # A fake timeout, first in PATH, that records its first two arguments and then runs the command it was given.
  cat >"$case_dir/bin/timeout" <<'EOF'
#!/usr/bin/env bash
echo "$1 $2" >>"$(dirname "$0")/../timeout-args"
shift 2
exec "$@"
EOF
  chmod +x "$case_dir/bin/pnpm" "$case_dir/bin/timeout"
  run_script
  expect_eq "exit status" 5 "$status"
  expect_eq "runs" 3 "$(lines_in "$case_dir/pnpm-args")"
  expect_eq "command line" "exec playwright install --with-deps chromium" "$(slurp "$case_dir/pnpm-args" | head -n 1)"
  expect_eq "pause" "10 10" "$(joined "$case_dir/sleeps")"
  expect_eq "every attempt is capped at 240 s and killed 20 s later if it ignores the signal" \
    "--kill-after=20 240 --kill-after=20 240 --kill-after=20 240" "$(joined "$case_dir/timeout-args")"
  expect_eq "no repair when the attempts only failed" 0 "$(lines_in "$case_dir/dpkg-args")"
}

case_config_write_failure_is_not_fatal() {
  echo "case: a failed config write is reported and the install still runs"
  new_case
  printf '#!/usr/bin/env bash\nexit 1\n' >"$case_dir/bin/sudo-fails"
  chmod +x "$case_dir/bin/sudo-fails"
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 0 1)" PLAYWRIGHT_INSTALL_SUDO="$case_dir/bin/sudo-fails"
  expect_eq "exit status" 0 "$status"
  expect_eq "install runs" 1 "$(lines_in "$case_dir/count")"
  expect_contains "warns" "$out" "warning"
}

case_bad_attempts_is_refused() {
  echo "case: a bad attempt count is refused before anything runs"
  for bad_value in 0 -1 abc 1.5; do
    new_case
    run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 0 1)" PLAYWRIGHT_INSTALL_ATTEMPTS="$bad_value"
    expect_eq "exit status for '$bad_value'" 2 "$status"
    expect_eq "nothing ran for '$bad_value'" 0 "$(lines_in "$case_dir/count")"
  done
}

# A stub install that hangs on its first N runs (a stalled apt-get: no output, no exit) and records the apt sources file as it found
# it. It sleeps with /bin/sleep, because the stub `sleep` of the case answers at once.
hang_stub() { # hangs-before-success
  {
    printf '%s\n' '#!/usr/bin/env bash'
    printf 'echo run >>"%s/count"\n' "$case_dir"
    printf 'cat "%s/ubuntu.sources" >>"%s/sources-seen"\n' "$case_dir" "$case_dir"
    printf 'echo %s >>"%s/sources-seen"\n' "'--'" "$case_dir"
    # The single quotes are deliberate: this line and the next are written into the stub script, and the $(...) and $runs in them are for the
    # stub to expand when it runs later, not for this shell now.
    # shellcheck disable=SC2016
    printf 'runs=$(wc -l <"%s/count" | tr -d " ")\n' "$case_dir"
    # shellcheck disable=SC2016
    printf 'if [ "$runs" -le %s ]; then /bin/sleep 30; fi\n' "$1"
  } >"$case_dir/hang-install.sh"
  printf 'bash %s/hang-install.sh' "$case_dir"
}

case_a_hanging_attempt_is_killed_repaired_and_retried() {
  echo "case: an attempt that hangs is killed after the limit, dpkg is repaired, and the next attempt reads archive.ubuntu.com"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(hang_stub 1)" PLAYWRIGHT_INSTALL_LIMIT=1 PLAYWRIGHT_INSTALL_PAUSE=0
  expect_eq "exit status" 0 "$status"
  expect_eq "two runs: the first was killed" 2 "$(lines_in "$case_dir/count")"
  expect_contains "says the attempt timed out" "$out" "attempt 1 of 3 timed out after 1 s"
  expect_contains "attempt 2 starts" "$out" "playwright-install: attempt 2 of 3"
  expect_eq "dpkg was repaired once, through sudo" "--configure -a" "$(slurp "$case_dir/dpkg-args")"
  seen=$(slurp "$case_dir/sources-seen")
  first=${seen%%--*}
  expect_contains "the first attempt saw the runner's mirror list" "$first" "mirror+file:/etc/apt/apt-mirrors.txt"
  expect_contains "the second attempt saw archive.ubuntu.com" "$seen" "URIs: http://archive.ubuntu.com/ubuntu/"
  expect_contains "the sources file keeps its other lines" "$(slurp "$case_dir/ubuntu.sources")" "Suites: noble"
  expect_not_contains "the sources file no longer names the mirror list" "$(slurp "$case_dir/ubuntu.sources")" "mirror+file"
}

case_every_attempt_hangs() {
  echo "case: when every attempt hangs the script gives up with the timeout's exit code"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(hang_stub 9)" PLAYWRIGHT_INSTALL_LIMIT=1 PLAYWRIGHT_INSTALL_ATTEMPTS=2 PLAYWRIGHT_INSTALL_PAUSE=0
  expect_eq "exit status is the timeout's" 124 "$status"
  expect_eq "two runs" 2 "$(lines_in "$case_dir/count")"
  expect_contains "gives up" "$out" "giving up after 2 attempts"
  expect_contains "names the code" "$out" "last exit code was 124"
}

case_an_ordinary_failure_does_not_repair_dpkg() {
  echo "case: a failure that is not a timeout does not repair dpkg, but the retry still reads archive.ubuntu.com"
  new_case
  run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 2 7)" PLAYWRIGHT_INSTALL_PAUSE=0
  expect_eq "exit status" 0 "$status"
  expect_eq "dpkg not run: nothing was cut off" 0 "$(lines_in "$case_dir/dpkg-args")"
  expect_not_contains "not reported as a timeout" "$out" "timed out"
  expect_contains "the retry reads archive.ubuntu.com" "$(slurp "$case_dir/ubuntu.sources")" "URIs: http://archive.ubuntu.com/ubuntu/"
  expect_eq "the mirror is switched once, not on every retry" 1 "$(printf '%s' "$out" | grep -c 'apt now reads archive.ubuntu.com')"
}

case_bad_limit_is_refused() {
  echo "case: a bad per-attempt limit is refused before anything runs"
  for bad_value in 0 -5 abc 2.5; do
    new_case
    run_script PLAYWRIGHT_INSTALL_CMD="$(stub_cmd 0 1)" PLAYWRIGHT_INSTALL_LIMIT="$bad_value"
    expect_eq "exit status for '$bad_value'" 2 "$status"
    expect_eq "nothing ran for '$bad_value'" 0 "$(lines_in "$case_dir/count")"
  done
}

for c in \
  case_first_attempt_succeeds \
  case_fails_twice_then_succeeds \
  case_always_fails \
  case_attempts_and_command_are_overridable \
  case_apt_config_is_written \
  case_apt_config_is_overwritten \
  case_production_defaults \
  case_config_write_failure_is_not_fatal \
  case_bad_attempts_is_refused \
  case_a_hanging_attempt_is_killed_repaired_and_retried \
  case_every_attempt_hangs \
  case_an_ordinary_failure_does_not_repair_dpkg \
  case_bad_limit_is_refused; do
  before=$failures
  "$c"
  if [ "$failures" -eq "$before" ]; then
    printf '  => PASS %s\n' "$c"
  else
    printf '  => RED  %s (%d checks failed)\n' "$c" "$((failures - before))"
  fi
done

echo
if [ "$failures" -ne 0 ]; then
  echo "playwright-install.test: $failures of $checks checks FAILED"
  exit 1
fi
echo "playwright-install.test: all $checks checks passed"
