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
