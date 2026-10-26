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
# The hub binary exits 1 by design until it is composed (plan 09 A1). The stage that finds the replicas crashing must say what it knows
# about each replica, and no more. The first run that reached it (2026-10-26, run 40374410436) said "the chart, the database connection or the
# cluster is at fault" for three replicas that all crashed with the A1 refusal: `kubectl logs --previous` had answered for one of them, with
# exit 0, with the sentence "unable to retrieve container logs for containerd://..." (the kubelet had already removed that container), and
# run.sh took the sentence for the log.

# The refusal, as crates/hub/src/main.rs logs it (kind-run.test.mjs checks the two against each other).
HUB_REFUSAL_TEXT='this build has no identity module (prompt 03a) and no registry (prompt 05) linked in; refusing to start without them'

# hub_log_usable LOG: succeeds when LOG is a log: not empty, and not the sentence that kubectl prints in place of the log of a container that
# the kubelet no longer has.
hub_log_usable() {
    [ -n "${1//[[:space:]]/}" ] && [[ "$1" != "unable to retrieve container logs"* ]]
}

# hub_replica_class LOG TERMINATED EXIT READY: what one replica shows, from what was retrieved (TERMINATED is true or false: the container ended at
# least once; EXIT is the exit code of its last end; READY is true or false):
#   refused         a usable log holds the A1 refusal
#   other           a usable log holds something else (a real failure to read, or a log that was read too early)
#   unlogged-exit1  no usable log; the container ended with exit 1 and was never Ready, as the replicas that did log the refusal did
#   unexplained     no usable log, and a state that is not that (waiting to pull an image, still starting, ended with another code, ...)
hub_replica_class() {
    local log="$1" terminated="$2" exit_code="$3" ready="$4"
    if hub_log_usable "$log"; then
        if grep -q -F -- "$HUB_REFUSAL_TEXT" <<<"$log"; then echo refused; else echo other; fi
    elif [ "$terminated" = true ] && [ "$exit_code" = 1 ] && [ "$ready" != true ]; then
        echo unlogged-exit1
    else
        echo unexplained
    fi
}

# hub_verdict REFUSED OTHER UNLOGGED UNEXPLAINED (the counts of the classes above, 3 replicas in all). Prints one word:
#   other-error  a replica logged something that is not the refusal: a real failure, whatever the others did
#   unexplained  a replica without a log is in a state that A1 does not explain
#   a1           all 3 replicas logged the refusal: the verdict as designed
#   a1-partial   some logged the refusal, and each of the others has no log but ended with exit 1 and was never Ready. Reported as what it is
#                (A1 seen on N of 3, no log for the rest): the three pods are one StatefulSet with one image and one configuration, so the
#                same exit is likely the same cause, but a state is not a log, so A1 is not claimed for a replica that has none
#   no-logs      no log was retrieved for any replica: the exit code 1 of all three is consistent with A1 and shows nothing
hub_verdict() {
    local refused="$1" other="$2" unlogged="$3" unexplained="$4"
    if [ "$other" -gt 0 ]; then echo other-error
    elif [ "$unexplained" -gt 0 ]; then echo unexplained
    elif [ "$refused" -eq 3 ]; then echo a1
    elif [ "$refused" -gt 0 ] && [ "$((refused + unlogged))" -eq 3 ]; then echo a1-partial
    else echo no-logs
    fi
}
