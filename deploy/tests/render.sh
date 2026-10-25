#!/usr/bin/env bash
# Render every chart with every values file, and check what comes out (09/T3; S6, S7, S10, S16, S18, P11).
#
#   deploy/tests/render.sh lint      helm lint --strict
#   deploy/tests/render.sh render    helm template | kubeconform -strict, offline, with the vendored schemas
#   deploy/tests/render.sh policy    helm template | conftest (deploy/policy), with perf/budgets.toml and the values files
#   deploy/tests/render.sh planted   the checks above must FAIL on planted violations, each for its own reason
#   deploy/tests/render.sh all       all of them
#
# What it needs: helm and kubeconform (`just ops-tools-install helm kubeconform`), Docker (conftest runs from the image pinned in
# deploy/images/tools.env), node. It never skips a check and never fetches a schema: kubeconform has no -ignore-missing-schemas,
# so a kind without a vendored schema is an error (deploy/schemas).
#
# Warnings. A policy warning is a fallback that is allowed to render (plan 09 A11, A13): kind has no zone, no Gateway and no FQDN
# policy, and a cluster without a CMEK key uses Google-managed encryption. In prod and staging a warning fails the run. Only the
# image policy's warning, the placeholder digest of plan 09 A29, is tolerated there, until the first image is published.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export PATH="$PWD/target/ops-tools/bin:$PATH"

mode="${1:-all}"
case "$mode" in lint | render | policy | planted | all) ;; *)
    echo "usage: deploy/tests/render.sh lint|render|policy|planted|all" >&2
    exit 2
    ;;
esac

for tool in helm kubeconform node; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "render.sh: $tool is not installed; run: just ops-tools-install helm kubeconform" >&2
        exit 1
    }
done
if [ "$mode" != lint ] && [ "$mode" != render ]; then
    docker info >/dev/null 2>&1 || {
        echo "render.sh: conftest runs from the pinned image and needs a running Docker daemon" >&2
        exit 1
    }
fi

# shellcheck source=deploy/images/tools.env
. deploy/images/tools.env
kube_version="$(node -p "require('./deploy/schemas/sources.json').kubernetes.version")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

schema_args=(
    -strict -summary -kubernetes-version "$kube_version"
    -schema-location 'deploy/schemas/k8s/{{.NormalizedKubernetesVersion}}-standalone{{.StrictSuffix}}/{{.ResourceKind}}{{.KindSuffix}}.json'
    -schema-location 'deploy/schemas/crds/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json'
)
hook_ranges=deploy/values/hub/github-hook-ranges.yaml
# Every namespace of the manifest policies. dockerfile.rego is for Dockerfiles and is run by `just images-policy-test`.
policy_namespaces=(hardening secrets topology resources storage network)

conftest() { docker run --rm -i -v "$PWD:/repo:ro" -v "$work:/work:ro" -w /repo "$LK_CONFTEST_IMAGE" "$@"; }

# The charts and the environments that have values. The hook ranges file lives next to the hub's values but belongs to the gcp
# chart, so it is not an environment.
combos=()
for chart in hub gcp; do
    for env in kind staging prod; do
        [ -f "deploy/values/$chart/$env.yaml" ] && combos+=("$chart:$env")
    done
done
[ ${#combos[@]} -gt 0 ] || {
    echo "render.sh: no values files under deploy/values" >&2
    exit 1
}

namespace_of() { case "$1" in hub) echo lanekeeper ;; gcp) echo lanekeeper-gcp ;; esac; }

# value_args CHART ENV: the -f arguments, in the order they apply.
value_args() {
    local chart="$1" env="$2"
    [ "$chart" = gcp ] && printf -- '-f\n%s\n' "$hook_ranges"
    printf -- '-f\n%s\n' "deploy/values/$chart/$env.yaml"
}

# render CHART ENV [helm args...] > output
render() {
    local chart="$1" env="$2"
    shift 2
    local args=()
    mapfile -t args < <(value_args "$chart" "$env")
    helm template lanekeeper "deploy/charts/$chart" --namespace "$(namespace_of "$chart")" --kube-version "$kube_version" "${args[@]}" "$@"
}

# check_policy FILE CHART ENV: conftest over one rendered file. Prints the output; returns conftest's status.
check_policy() {
    local rendered="$1" chart="$2" env="$3" status=0
    local files=("$rendered" perf/budgets.toml "deploy/values/$chart/$env.yaml" "deploy/charts/$chart/values.yaml")
    local data=()
    if [ "$chart" = gcp ]; then
        data=(--data "$hook_ranges")
        files+=("$hook_ranges")
    fi
    local ns=() n
    for n in "${policy_namespaces[@]}"; do ns+=(--namespace "$n"); done
    local fail_on_warn=()
    [ "$env" = kind ] || fail_on_warn=(--fail-on-warn)
    # The images policy is run apart: its warning is the placeholder digest, tolerated until the first publish.
    conftest test --combine "${fail_on_warn[@]}" --policy deploy/policy "${ns[@]}" "${data[@]}" --no-color "/work/${rendered##*/}" "${files[@]:1}" || status=$?
    conftest test --combine --policy deploy/policy --namespace images --no-color "/work/${rendered##*/}" || status=$?
    return "$status"
}

# Every stage runs for every chart and environment, and reports every failure, so that one broken values file does not hide the
# next. (Inside a function that is called from `||`, `set -e` is off, so each command says `|| return 1` for itself.)
each_combo() {
    local fn="$1" combo chart env status=0
    for combo in "${combos[@]}"; do
        chart="${combo%%:*}"
        env="${combo##*:}"
        "$fn" "$chart" "$env" || {
            echo "render.sh: $fn FAILED for the $chart chart with the $env values" >&2
            status=1
        }
    done
    return "$status"
}

lint_one() {
    local chart="$1" env="$2" args
    mapfile -t args < <(value_args "$chart" "$env")
    echo "== helm lint $chart ($env)"
    helm lint --strict "deploy/charts/$chart" --namespace "$(namespace_of "$chart")" --kube-version "$kube_version" "${args[@]}" || return 1
}

kubeconform_one() {
    local chart="$1" env="$2"
    echo "== kubeconform $chart ($env)"
    render "$chart" "$env" >"$work/$chart-$env.yaml" || return 1
    kubeconform "${schema_args[@]}" "$work/$chart-$env.yaml" || return 1
}

policy_one() {
    local chart="$1" env="$2"
    echo "== conftest $chart ($env)"
    render "$chart" "$env" >"$work/$chart-$env.yaml" || return 1
    check_policy "$work/$chart-$env.yaml" "$chart" "$env" || return 1
}

lint() { each_combo lint_one; }
kubeconform_all() { each_combo kubeconform_one; }
policy_all() { each_combo policy_one; }

# --- planted violations ---------------------------------------------------------------------------------------------------
# Each case changes the real output of the chart (a line of text, so no YAML parser is needed), and the check that guards that
# field must refuse it with its own message. A case that is refused for another reason, or not at all, fails the run.

failures=0
fail() {
    echo "PLANTED FAILED: $*" >&2
    failures=$((failures + 1))
}

# expect_policy_failure NAME FILE CHART ENV FRAGMENT
expect_policy_failure() {
    local name="$1" file="$2" chart="$3" env="$4" fragment="$5" out
    if out="$(check_policy "$file" "$chart" "$env" 2>&1)"; then
        fail "$name: the policy accepted it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

# plant NAME CHART ENV SED FRAGMENT: apply SED to the rendered output, which must differ from it.
plant() {
    local name="$1" chart="$2" env="$3" expr="$4" fragment="$5"
    local good="$work/good-$chart-$env.yaml" bad="$work/planted.yaml"
    [ -f "$good" ] || render "$chart" "$env" >"$good"
    sed -E "$expr" "$good" >"$bad"
    if cmp -s "$good" "$bad"; then
        fail "$name: the sed expression changed nothing, so the case tests nothing"
        return
    fi
    expect_policy_failure "$name" "$bad" "$chart" "$env" "$fragment"
}

planted_hardening() {
    echo "== planted: the policies refuse the real output of the chart with one thing wrong"
    local good="$work/good-hub-kind.yaml"
    render hub kind >"$good"
    echo "-- the unchanged output passes"
    check_policy "$good" hub kind >/dev/null || fail "the unchanged kind render of the hub chart was refused"

    # S16: one case per field.
    plant "runAsNonRoot" hub kind '/^ *runAsNonRoot: true$/d' "runAsNonRoot"
    plant "runAsUser" hub kind '/^ *runAsUser: 65532$/d' "runAsUser"
    plant "runAsGroup" hub kind '/^ *runAsGroup: 65532$/d' "runAsGroup"
    plant "seccomp" hub kind 's/^( *)type: RuntimeDefault$/\1type: Unconfined/' "seccomp"
    plant "allowPrivilegeEscalation" hub kind 's/allowPrivilegeEscalation: false/allowPrivilegeEscalation: true/' "allowPrivilegeEscalation"
    plant "privileged" hub kind 's/privileged: false/privileged: true/' "privileged"
    plant "readOnlyRootFilesystem" hub kind 's/readOnlyRootFilesystem: true/readOnlyRootFilesystem: false/' "readOnlyRootFilesystem"
    plant "drop ALL" hub kind 's/drop: \["ALL"\]/drop: ["NET_RAW"]/' "drop ALL"
    plant "requests.cpu" hub kind '/^ *cpu: 500m$/d' "resources.requests.cpu"
    plant "limits.cpu" hub kind '/^ *cpu: "4"$/d' "resources.limits.cpu"
    plant "limits.memory" hub kind '/^ *memory: 2Gi$/d' "resources.limits.memory"
    plant "requests.memory" hub kind '/^ *memory: 1536Mi$/d' "resources.requests.memory"
    plant "automountServiceAccountToken" hub kind '/automountServiceAccountToken: false/d' "automountServiceAccountToken"
    plant "hostNetwork" hub kind 's/^( *)enableServiceLinks: false$/&\n\1hostNetwork: true/' "hostNetwork"
    plant "hostPID" hub kind 's/^( *)enableServiceLinks: false$/&\n\1hostPID: true/' "hostPID"
    plant "hostIPC" hub kind 's/^( *)enableServiceLinks: false$/&\n\1hostIPC: true/' "hostIPC"

    # P11: the memory request is the budget, whatever else is right.
    plant "memory request above the budget" hub kind 's/memory: 1536Mi/memory: 1792Mi/' "P11"
    plant "no startup probe" hub kind '/startupProbe:/,/failureThreshold: 30/d' "startupProbe"

    # S19: no tag.
    plant "image by tag" hub kind 's/@sha256:[0-9a-f]{64}/:latest/' "not pinned by digest"

    # S6, S7: what the pod gets from the database's Secrets.
    plant "the whole CA secret" hub kind '/secretName: lanekeeper-db-ca/,/path: ca.crt/{/items:/d;/- key: ca.crt/d;/path: ca.crt/d}' "ca.key"
    plant "every key of the credentials" hub kind '/secretName: lanekeeper-db-app/,/path: uri/{/items:/d;/- key: uri/d;/path: uri/d}' "only the uri"
    plant "world-readable credentials" hub kind 's/defaultMode: 288/defaultMode: 420/' "world-readable"
    plant "a secret in the environment" hub kind 's/^( *)- name: LK_WEB_DIST$/\1- name: LK_GIT_TOKEN_VALUE/' "looks like a secret"
    plant "the database URL in the environment" hub kind 's/LK_DATABASE_URL_FILE/LK_DATABASE_URL/' "LK_DATABASE_URL_FILE"

    # D62: the topology.
    plant "two replicas" hub kind 's/replicas: 3/replicas: 2/' "3 replicas"
    plant "budget keeps one" hub kind 's/minAvailable: 2/minAvailable: 1/' "minAvailable: 2"
    plant "not headless" hub kind 's/clusterIP: None/clusterIP: 10.0.0.1/' "headless"
    plant "no zone spread (prod)" hub prod 's#topology.kubernetes.io/zone#kubernetes.io/hostname#' "zone"

    # S18: the network.
    plant "egress only" hub kind 's/policyTypes: \[Ingress, Egress\]/policyTypes: [Egress]/' "default deny"
    plant "ingress from the internet" hub kind 's#cidr: "198.51.100.0/24"#cidr: "0.0.0.0/0"#' "too wide"
    plant "an open load balancer" hub prod 's#- "198.51.100.0/24"#- "0.0.0.0/0"#' "too wide"
    plant "a port on the load balancer" hub prod 's/port: 8444/port: 22/' "8444"
    plant "a wildcard external host" hub prod 's/pattern: github.com/pattern: "*.com"/' "wildcard"
    plant "external port 80" hub prod '/pattern: www.googleapis.com/,/port: 443/s/port: 443/port: 80/' "443"
    plant "no certificate" hub prod '/networking.gke.io\/pre-shared-certs/d' "certificate"
    plant "an HTTP listener" hub prod 's/protocol: HTTPS/protocol: HTTP/' "only HTTPS listeners"
    plant "no Cloud Armor" hub prod '/securityPolicy:/d' "Cloud Armor"
    plant "no hooks route" hub prod 's#value: /hooks/github#value: /hooks/other#' "/hooks/github"
    plant "web as a load balancer" hub prod '/name: lanekeeper-hub-web$/,/appProtocol: HTTPS/s/type: ClusterIP/type: LoadBalancer/' "Service/lanekeeper-hub-web"

    # S10: the disks.
    plant "an unencrypted volume class" hub prod 's/storageClassName: lanekeeper-hub-cmek/storageClassName: standard-rwo/' "customer-managed"
}

planted_gcp() {
    echo "== planted: Cloud Armor"
    if ! render gcp prod >"$work/good-gcp-prod.yaml" 2>"$work/gcp.err"; then
        echo "  (the gcp chart does not render with the committed hook ranges, so the Cloud Armor cases are not run: $(tail -n1 "$work/gcp.err"))"
        failures=$((failures + 1))
        return
    fi
    plant "the default rule allows" gcp prod 's/action: deny\(403\)/action: allow/' "default rule"
    plant "a hook range for every path" gcp prod "s#request.path == '/hooks/github' && ##" "hooks path"
    plant "no users' ranges" gcp prod '/^ *- "192\.0\.2\.0\/24"$/d' "no source ranges"
}

planted_kubeconform() {
    echo "== planted: kubeconform refuses a field the schema does not have, and a kind it has no schema for"
    local out status=0
    out="$(kubeconform "${schema_args[@]}" deploy/tests/planted/kubeconform.yaml 2>&1)" || status=$?
    [ "$status" -ne 0 ] || fail "kubeconform accepted deploy/tests/planted/kubeconform.yaml"
    local kind
    for kind in GCPBackendPolicy HealthCheckPolicy FQDNNetworkPolicy ComputeSecurityPolicy Gateway StatefulSet; do
        grep -E -q -- "- $kind planted is invalid" <<<"$out" || fail "kubeconform did not refuse the planted $kind:"$'\n'"$out"
    done
    grep -q -- "could not find schema for SomethingWithoutASchema" <<<"$out" || fail "a kind without a schema was skipped, not refused"
    echo "  ok  kubeconform"
}

# helm must stop before it writes a manifest that breaks a rule the chart owns.
expect_helm_failure() {
    local name="$1" fragment="$2"
    shift 2
    local out
    if out="$(render hub prod "$@" 2>&1 >/dev/null)"; then
        fail "$name: helm rendered it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

planted_templates() {
    echo "== planted: the templates refuse values that break a rule"
    expect_helm_failure "no image digest" "digest" --set image.digest=
    expect_helm_failure "a short digest" "digest" --set image.digest=sha256:abc
    expect_helm_failure "a tag instead of a digest" "digest" --set image.digest=latest
    expect_helm_failure "empty agent source ranges" "agentSourceRanges" --set-json 'network.agentSourceRanges=[]'
    expect_helm_failure "the internet as the agent source range" "whole internet" --set-json 'network.agentSourceRanges=["0.0.0.0/0"]'
    expect_helm_failure "no Google ranges" "googleRanges" --set-json 'network.googleRanges=[]'
    expect_helm_failure "a certificate of each kind" "exactly one" --set gateway.tls.certManagerCerts=also-this
    expect_helm_failure "no certificate" "exactly one" --set gateway.tls.preSharedCerts=
    expect_helm_failure "no host name" "hostname" --set gateway.hostname=
    expect_helm_failure "a malformed CMEK key" "cmekKey" --set storage.cmekKey=not-a-key
    expect_helm_failure "part of the Git settings" "config.git" --set-json 'config.git={"baseUrl":"https://github.com/x/y.git"}'
    expect_helm_failure "a tunable the hub does not have" "LK_DATABASE_URL" --set-json 'config.tuning={"LK_DATABASE_URL":"x"}'
    expect_helm_failure "an unknown value" "not_a_value" --set not_a_value=1
    expect_helm_failure "no Google service account" "gcpServiceAccount" --set serviceAccount.gcpServiceAccount=
    expect_helm_failure "a CA secret that is not CloudNativePG's" "caSecretName" --set database.caSecretName=anything
}

planted_all() {
    planted_kubeconform
    planted_templates
    planted_hardening
    planted_gcp
    if [ "$failures" -ne 0 ]; then
        echo "render.sh: $failures planted case(s) failed" >&2
        return 1
    fi
    echo "render.sh: every planted case was refused for its own reason"
}

case "$mode" in
    lint) lint ;;
    render) kubeconform_all ;;
    policy) policy_all ;;
    planted) planted_all ;;
    all)
        status=0
        lint || status=1
        kubeconform_all || status=1
        policy_all || status=1
        planted_all || status=1
        exit "$status"
        ;;
esac
