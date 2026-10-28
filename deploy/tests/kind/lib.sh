# shellcheck shell=bash
# Helpers of deploy/tests/kind/run.sh that need no cluster, so that .github/scripts/kind-run.test.mjs can test them. Source it; it runs nothing.

# P11: the hub starts in under 10 seconds (docs/performance.md; perf/budgets.toml, `P11`, 10000 ms, p95 at design scale).
P11_BUDGET_S=10

# p11_verdict [BUDGET-SECONDS, default P11_BUDGET_S] < "NAME SECONDS" lines
#   One line per replica: its name and the whole seconds from the start of its container to its Ready condition (the Ready condition is
#   the readinessProbe's, on /readyz of the operations port. The kubelet runs no readinessProbe until the startupProbe has succeeded, so the
#   number includes up to the startupProbe's period (2 s, deploy/charts/hub/templates/statefulset.yaml:96-100) and then up to the
#   readinessProbe's (5 s, lines 106-108), and the API's timestamps have a resolution of one second. A replica that is Ready within the
#   budget therefore started within it; a replica that is not may have lost the time to probe overhead (up to about 7 s of the 10 s in the
#   worst case, from the periods and not measured), so a red P11 on kind is read against that before it is read as a slow start). Prints one line per replica, sets
#   P11_WORST to the slowest, and returns 0 when none is over BUDGET, 1 when one is, and 2 when there is nothing to judge (no line, a line
#   without a whole number of seconds, a budget that is not a whole number): never a pass on nothing.
P11_WORST=""
p11_verdict() {
    local budget="${1-$P11_BUDGET_S}" name secs extra n=0 over=0
    P11_WORST=""
    [[ "$budget" =~ ^[0-9]+$ ]] || { echo "p11: the budget '$budget' is not a whole number of seconds" >&2; return 2; }
    while read -r name secs extra; do
        [ -n "$name" ] || continue
        if ! [[ "$secs" =~ ^[0-9]+$ ]] || [ -n "${extra:-}" ]; then
            echo "p11: $name has no whole number of seconds ('$secs${extra:+ $extra}'); the start-up time was not measured" >&2
            return 2
        fi
        n=$((n + 1))
        if [ -z "$P11_WORST" ] || [ "$secs" -gt "$P11_WORST" ]; then P11_WORST="$secs"; fi
        if [ "$secs" -gt "$budget" ]; then
            over=$((over + 1))
            echo "$name: $secs s from the start of the container to Ready, OVER the budget of $budget s (P11)"
        else
            echo "$name: $secs s from the start of the container to Ready, within the budget of $budget s (P11)"
        fi
    done
    [ "$n" -gt 0 ] || { echo "p11: no replica was measured" >&2; return 2; }
    [ "$over" -eq 0 ]
}

# ---- the local registry ------------------------------------------------------------------------------------------------------
# run.sh starts a registry in a container, publishes it on the loopback address of the host, and (once the cluster exists) joins it to
# the kind network. The first CI run (2026-10-26) showed that it answered when it was started and refused a connection on the published
# port 46 s later, with nothing in the log about the container. These functions wait for it within a bound, try every route to it, and say
# what the container is doing when none answers.

# wait_for_http SECONDS URL...
#   Asks every URL in turn, once per round (curl, 2 s each, LK_WAIT_STEP seconds between rounds, default 1: the tests set it lower), until
#   one answers with a 2xx: returns 0 and sets HTTP_ANSWERED to that URL, so a dead route never hides a live one. After SECONDS it returns 1
#   and prints, on standard error, what each URL said at its last attempt. 2 for a budget that is not a whole number of seconds or no URL.
HTTP_ANSWERED=""
# shellcheck disable=SC2034 # HTTP_ANSWERED is read by run.sh and registry-test.sh, which source this file
wait_for_http() {
    local budget="${1-}" url deadline step="${LK_WAIT_STEP:-1}" last i
    HTTP_ANSWERED=""
    [[ "$budget" =~ ^[0-9]+$ ]] || { echo "wait_for_http: the budget '$budget' is not a whole number of seconds" >&2; return 2; }
    shift
    [ $# -gt 0 ] || { echo "wait_for_http: no URL to wait for" >&2; return 2; }
    local -A said=()
    deadline=$((SECONDS + budget))
    while :; do
        for url in "$@"; do
            if last="$(curl -fsS --max-time 2 -o /dev/null "$url" 2>&1)"; then HTTP_ANSWERED="$url"; return 0; fi
            said["$url"]="$last"
        done
        [ "$SECONDS" -lt "$deadline" ] || break
        sleep "$step"
    done
    echo "wait_for_http: nothing answered in $budget s:" >&2
    for i in "$@"; do echo "  $i  $(printf '%s' "${said[$i]-}" | tr '\n' ' ')" >&2; done
    return 1
}

# registry_container_ip CONTAINER NETWORK
#   The address of the container on the Docker network NETWORK: empty when it is not on it (or docker cannot say).
registry_container_ip() {
    docker inspect --format "{{with index .NetworkSettings.Networks \"$2\"}}{{.IPAddress}}{{end}}" "$1" 2>/dev/null || true
}

# registry_report CONTAINER [HOST-PORT]
#   What a person needs when the registry does not answer, on standard error: the container's state (status, exit code, whether the
#   kernel killed it for memory), the ports Docker published, the networks it is on and its addresses, the last 30 lines of its log, and,
#   if `ss` exists, the sockets of the host that listen on the published port. It never fails.
registry_report() {
    local container="$1" port="${2:-}"
    {
        echo "---- the registry container $container: state"
        docker inspect --format 'status={{.State.Status}} running={{.State.Running}} exit_code={{.State.ExitCode}} oom_killed={{.State.OOMKilled}} error={{printf "%q" .State.Error}} started={{.State.StartedAt}} finished={{.State.FinishedAt}}' "$container" 2>&1 || echo "docker cannot inspect $container"
        echo "---- the registry container $container: published ports"
        docker port "$container" 2>&1 || true
        echo "---- the registry container $container: networks and addresses"
        docker inspect --format '{{range $n, $v := .NetworkSettings.Networks}}{{$n}} {{$v.IPAddress}}{{"\n"}}{{end}}' "$container" 2>&1 || true
        echo "---- the registry container $container: the last 30 lines of its log"
        docker logs --tail 30 "$container" 2>&1 || true
        if [ -n "$port" ] && command -v ss >/dev/null 2>&1; then
            echo "---- sockets of this host that listen on port $port"
            ss -ltn "sport = :$port" 2>&1 || true
        fi
    } >&2
}

# ---- the verdict on the hub replicas ------------------------------------------------------------------------------------------
# The production hub cannot become Ready in kind: it reads the web certificate and both CA certificates from Secret Manager and signs with
# Cloud KMS, and it signs users in with Entra, by design (prompt 18, G23: no fake in a release build, no setting that points the image at a
# stub). So the stage asks what the production image CAN do here, and the verdict below is made of it: each replica reads its settings and
# accepts them (exit 1, never exit 2), its database and schema checks pass, and it stops at its first Google step. Everything else is a
# failure that names what was seen. The first run that reached the stage (2026-10-26, run 40374410436) took a sentence of kubectl in place of
# a log for the log (`kubectl logs --previous` exits 0 with "unable to retrieve container logs for containerd://..." when the kubelet has
# removed the container), and blamed the chart for what the log would have shown; hub_log_usable keeps that from happening again.

# What this job does not measure, said in one sentence that run.sh prints and the kind-e2e job of images.yml appends to its summary.
# shellcheck disable=SC2034 # read by run.sh, which sources this file, and by images.yml
KIND_NOT_MEASURED='/readyz on 3 replicas and P11 on the pod are NOT MEASURED in kind (the production image cannot be Ready without Google and Entra), acceptance 3 is not met by this job'

# ---- the one place that couples this script to prompt 18's wording ----
# Every string below is looked for in a hub log with `grep -F` (never as a pattern). Each is a literal of the hub's Rust source, in the
# non-test code of crates/hub/src: .github/scripts/kind-run.test.mjs reads that source and fails when one is gone or reworded, so a change of
# the hub's message turns this job red in the pull request that changes it, and not on the first run after it. The text of a message is
# the Display of the error type that is named beside it. To change a marker, change the test's table of anchors with it.
#
# Google-side: the hub's typed start errors for what it reads from Secret Manager and signs with KMS. kind has no metadata server, so the
# error may be the timeout of the step instead of the failure of the read; both are Google's.
HUB_GOOGLE_MARKERS=(
    'the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF)' # StartError::WebCertificate (process.rs)
    'starting took too long: web certificate'                                   # StartError::TimedOut("web certificate") (process.rs, load)
    'the CA certificate secret could not be read'                               # CaStep::Secret, inside PlatformError::Ca (wiring/grpc.rs)
    'setting it up took too long'                                               # CaStep::TimedOut, inside PlatformError::Ca (wiring/grpc.rs)
)
# The database and the schema checks run in parallel with the Google reads (process.rs, load), each to its end, and every failure is logged.
HUB_DATABASE_MARKERS=(
    'the database is not usable'           # StartError::Database (process.rs)
    'starting took too long: database'     # StartError::TimedOut("database"): the connection and the schema check are one bounded step
)
HUB_SCHEMA_MARKERS=(
    'the database schema is not usable'    # StartError::Schema (process.rs)
)
# A setting or the command line refused, or the process stopped before its start: process.rs `say` writes "hub: <message>" to standard error.
# Exit code 2 says the same without a log.
HUB_SETTING_PREFIX='hub: '

# hub_markers: every marker above, one per line (what the test compares with the source).
hub_markers() {
    printf '%s\n' "${HUB_GOOGLE_MARKERS[@]}" "${HUB_DATABASE_MARKERS[@]}" "${HUB_SCHEMA_MARKERS[@]}" "$HUB_SETTING_PREFIX"
}

# hub_log_usable LOG: succeeds when LOG is a log: not empty, and not the sentence that kubectl prints in place of the log of a container that
# the kubelet no longer has.
hub_log_usable() {
    [ -n "${1//[[:space:]]/}" ] && [[ "$1" != "unable to retrieve container logs"* ]]
}

# hub_has_marker LOG MARKER...: succeeds when a line of LOG holds one of the markers (fixed strings).
hub_has_marker() {
    local log="$1" marker
    shift
    for marker in "$@"; do
        if grep -q -F -- "$marker" <<<"$log"; then return 0; fi
    done
    return 1
}

# hub_first_line LOG MARKER...: prints the first line of LOG that holds one of the markers, cut to 600 characters; 1 when there is none.
hub_first_line() {
    local log="$1" marker line
    shift
    for marker in "$@"; do
        line="$(grep -F -m1 -- "$marker" <<<"$log" || true)"
        if [ -n "$line" ]; then printf '%s\n' "${line:0:600}"; return 0; fi
    done
    return 1
}

# hub_replica_class LOG TERMINATED EXIT READY: what one replica shows, from what was retrieved (TERMINATED is true or false: the container
# ended at least once; EXIT is the exit code of its last end, empty if it has none; READY is true or false). One word:
#   ready       the pod is Ready. The production image cannot be in kind, so this is a state to look at, not a pass
#   setting     exit code 2 (a setting or the command line was refused), or a line "hub: ..." (the process stopped before its start)
#   schema      the hub's schema check failed (the migration did not run, or not against this database)
#   database    the hub's database check failed
#   google      the log holds a Google-side marker, no setting, database or schema error, and the last exit code is 1: the verdict as designed
#   wrong-exit  the same log, and an exit code that is not 1 (or none yet: the container has not ended)
#   unknown     a usable log that holds none of the markers: something this job does not know, to be read
#   unlogged    no usable log, and not exit code 2: nothing can be asserted
# A failure beside a Google error is the failure: the order is setting, schema, database, then Google.
hub_replica_class() {
    local log="$1" terminated="$2" exit_code="$3" ready="$4"
    if [ "$ready" = true ]; then echo ready; return 0; fi
    if [ "$terminated" = true ] && [ "$exit_code" = 2 ]; then echo setting; return 0; fi
    if ! hub_log_usable "$log"; then echo unlogged; return 0; fi
    if grep -q -- "^${HUB_SETTING_PREFIX}" <<<"$log"; then echo setting
    elif hub_has_marker "$log" "${HUB_SCHEMA_MARKERS[@]}"; then echo schema
    elif hub_has_marker "$log" "${HUB_DATABASE_MARKERS[@]}"; then echo database
    elif hub_has_marker "$log" "${HUB_GOOGLE_MARKERS[@]}"; then
        if [ "$terminated" = true ] && [ "$exit_code" = 1 ]; then echo google; else echo wrong-exit; fi
    else echo unknown
    fi
}

# hub_verdict CLASS CLASS CLASS (the three replicas' classes, in any order). Prints the class that decides and returns 0 only when all three
# are `google`; otherwise it prints the most serious class that occurs, in this order, and returns 1:
#   setting schema database ready wrong-exit unknown unlogged
# Anything else, and any count but three, is `unknown`: never a pass on nothing.
hub_verdict() {
    local n=0 google=0 class want
    for class in "$@"; do
        n=$((n + 1))
        if [ "$class" = google ]; then google=$((google + 1)); fi
    done
    if [ "$n" -eq 3 ] && [ "$google" -eq 3 ]; then echo google; return 0; fi
    for want in setting schema database ready wrong-exit unknown unlogged; do
        for class in "$@"; do
            if [ "$class" = "$want" ]; then echo "$want"; return 1; fi
        done
    done
    echo unknown
    return 1
}
