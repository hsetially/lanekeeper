#!/usr/bin/env bash
# Render every chart with every values file, and check what comes out (09/T3; S6, S7, S10, S16, S18, P11).
#
#   deploy/tests/render.sh lint      helm lint --strict
#   deploy/tests/render.sh render    helm template | kubeconform -strict, offline, with the vendored schemas
#   deploy/tests/render.sh policy    helm template | conftest (deploy/policy), with perf/budgets.toml and the values files
#   deploy/tests/render.sh contract  the agent chart's Roles, NetworkPolicy and pod against crates/agent/RBAC.md and HARDENING.md
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
case "$mode" in lint | render | policy | contract | planted | all) ;; *)
    echo "usage: deploy/tests/render.sh lint|render|policy|contract|planted|all" >&2
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
# Every namespace of the manifest policies. dockerfile.rego is for Dockerfiles and is run by `just images-policy-test`; contract.rego
# needs the two contract files and is run apart (`contract` mode).
policy_namespaces=(hardening secrets topology resources storage network rbac)
# The agent chart refuses to render without the hub's CA, which no values file carries (deploy/tests/testdata/README.md).
synthetic_hub_ca=deploy/tests/testdata/synthetic-hub-ca.pem

conftest() { docker run --rm -i -v "$PWD:/repo:ro" -v "$work:/work:ro" -w /repo "$LK_CONFTEST_IMAGE" "$@"; }

# The charts and the environments that have values. The hook ranges file lives next to the hub's values but belongs to the gcp
# chart, so it is not an environment.
combos=()
for chart in hub gcp; do
    for env in kind staging prod; do
        [ -f "deploy/values/$chart/$env.yaml" ] && combos+=("$chart:$env")
    done
done
# The agent has one values file per swimlane (deploy/values/agent/<swimlane>.yaml).
for f in deploy/values/agent/*.yaml; do
    [ -f "$f" ] && combos+=("agent:$(basename "$f" .yaml)")
done
[ ${#combos[@]} -gt 0 ] || {
    echo "render.sh: no values files under deploy/values" >&2
    exit 1
}

namespace_of() { case "$1" in hub | agent) echo lanekeeper ;; gcp) echo lanekeeper-gcp ;; esac; }

# value_args CHART ENV: the -f arguments, in the order they apply.
value_args() {
    local chart="$1" env="$2"
    [ "$chart" = gcp ] && printf -- '-f\n%s\n' "$hook_ranges"
    printf -- '-f\n%s\n' "deploy/values/$chart/$env.yaml"
    [ "$chart" = agent ] && printf -- '--set-file\nhubCa.pem=%s\n' "$synthetic_hub_ca"
    return 0
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

# check_contract FILE: conftest over the agent chart's output and the two contract files, extracted from the Markdown that the
# agent's owner wrote and the agent's own tests lint (crates/agent/RBAC.md and HARDENING.md). Prints the output; returns conftest's
# status. The contract files are not part of check_policy's input: their Roles and Pod would be taken for the chart's.
check_contract() {
    local rendered="$1"
    mkdir -p "$work/contract"
    deploy/tests/contract-extract.sh crates/agent/RBAC.md "$work/contract/agent-rbac-contract.yaml" || return 1
    deploy/tests/contract-extract.sh crates/agent/HARDENING.md "$work/contract/agent-hardening-contract.yaml" || return 1
    conftest test --combine --policy deploy/policy --namespace contract --no-color "/work/${rendered##*/}" \
        /work/contract/agent-rbac-contract.yaml /work/contract/agent-hardening-contract.yaml
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

# Only the agent chart has a contract file (RBAC.md, HARDENING.md); the other charts' contracts are in their policies.
contract_one() {
    local chart="$1" env="$2"
    [ "$chart" = agent ] || return 0
    echo "== contract $chart ($env)"
    render "$chart" "$env" >"$work/$chart-$env.yaml" || return 1
    check_contract "$work/$chart-$env.yaml" || return 1
}

lint() { each_combo lint_one; }
kubeconform_all() { each_combo kubeconform_one; }
policy_all() { each_combo policy_one; }
contract_all() { each_combo contract_one; }

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

# --- the agent chart (09/T4): the same checks on the real output, one planted change at a time -----------------------------------------

# append_doc FILE TEXT: the output with one more YAML document.
append_doc() { printf '%s\n---\n%s\n' "$(cat "$1")" "$2"; }

# plant_append NAME CHART ENV TEXT FRAGMENT: the rendered output with one more document, which the policies must refuse.
plant_append() {
    local name="$1" chart="$2" env="$3" text="$4" fragment="$5"
    local good="$work/good-$chart-$env.yaml" bad="$work/planted.yaml"
    [ -f "$good" ] || render "$chart" "$env" >"$good"
    append_doc "$good" "$text" >"$bad"
    expect_policy_failure "$name" "$bad" "$chart" "$env" "$fragment"
}

# expect_contract_failure NAME FILE FRAGMENT: the contract check must refuse FILE, for FRAGMENT.
expect_contract_failure() {
    local name="$1" file="$2" fragment="$3" out
    if out="$(check_contract "$file" 2>&1)"; then
        fail "$name: the contract check accepted it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

# plant_contract NAME SED FRAGMENT: apply SED to the agent's output, which must differ from it.
plant_contract() {
    local name="$1" expr="$2" fragment="$3"
    local good="$work/good-agent-sit1.yaml" bad="$work/planted.yaml"
    [ -f "$good" ] || render agent sit1 >"$good"
    sed -E "$expr" "$good" >"$bad"
    if cmp -s "$good" "$bad"; then
        fail "$name: the sed expression changed nothing, so the case tests nothing"
        return
    fi
    expect_contract_failure "$name" "$bad" "$fragment"
}

planted_agent() {
    echo "== planted: the policies refuse the real output of the agent chart with one thing wrong"
    local good="$work/good-agent-sit1.yaml"
    render agent sit1 >"$good"
    echo "-- the unchanged output passes"
    check_policy "$good" agent sit1 >/dev/null || fail "the unchanged render of the agent chart was refused"
    check_contract "$good" >/dev/null || fail "the unchanged render of the agent chart was refused by the contract check"

    # S16: one case per field.
    plant "runAsNonRoot" agent sit1 '/^ *runAsNonRoot: true$/d' "runAsNonRoot"
    plant "runAsUser" agent sit1 '/^ *runAsUser: 1010$/d' "runAsUser"
    plant "runAsGroup" agent sit1 '/^ *runAsGroup: 1010$/d' "runAsGroup"
    plant "root" agent sit1 's/^( *)runAsUser: 1010$/\1runAsUser: 0/' "is root"
    plant "seccomp" agent sit1 's/^( *)type: RuntimeDefault$/\1type: Unconfined/' "seccomp"
    plant "allowPrivilegeEscalation" agent sit1 's/allowPrivilegeEscalation: false/allowPrivilegeEscalation: true/' "allowPrivilegeEscalation"
    plant "privileged" agent sit1 's/privileged: false/privileged: true/' "privileged"
    plant "readOnlyRootFilesystem" agent sit1 's/readOnlyRootFilesystem: true/readOnlyRootFilesystem: false/' "readOnlyRootFilesystem"
    plant "drop ALL" agent sit1 's/drop: \["ALL"\]/drop: ["NET_RAW"]/' "drop ALL"
    plant "requests.cpu" agent sit1 '/^ *cpu: 25m$/d' "resources.requests.cpu"
    plant "limits.cpu" agent sit1 '/^ *cpu: "1"$/d' "resources.limits.cpu"
    plant "limits.memory" agent sit1 '/^ *memory: 128Mi$/d' "resources.limits.memory"
    plant "requests.memory" agent sit1 '/^ *memory: 64Mi$/d' "resources.requests.memory"
    plant "hostNetwork" agent sit1 's/hostNetwork: false/hostNetwork: true/' "hostNetwork"
    plant "hostPID" agent sit1 's/hostPID: false/hostPID: true/' "hostPID"
    plant "hostIPC" agent sit1 's/hostIPC: false/hostIPC: true/' "hostIPC"

    # S16, S17: the token is the agent's, and one agent is one.
    plant "no token for the agent" agent sit1 's/^( *)automountServiceAccountToken: true$/\1automountServiceAccountToken: false/' "must be stated and true"
    plant "two agents" agent sit1 's/replicas: 1/replicas: 2/' "1 replica"
    plant "another service account" agent sit1 's/serviceAccountName: lanekeeper-agent/serviceAccountName: default/' "serviceAccountName"
    plant "the export is not a mount" agent sit1 's#^( *)value: /mnt/csp-configuration$#\1value: /mnt/elsewhere#' "LK_NFS_ROOT"
    plant "the hub CA is writable" agent sit1 '/^ *readOnly: true$/d' "writable"
    plant "a secret in the environment" agent sit1 's/^( *)- name: LK_TMP_DIR$/\1- name: LK_HUB_TOKEN/' "looks like a secret"
    plant "the certificate Secret's name" agent sit1 's/^( *)value: lanekeeper-agent-cert$/\1value: lanekeeper-db-app/' "must be lanekeeper-agent-cert"
    plant "image by tag" agent sit1 's/@sha256:[0-9a-f]{64}/:latest/' "not pinned by digest"

    # S7: the Secret and the ConfigMap.
    plant "the certificate Secret is not kept" agent sit1 '/helm.sh\/resource-policy: keep/d' "helm.sh/resource-policy"
    plant "a private key as the CA" agent sit1 's/-----BEGIN CERTIFICATE-----/-----BEGIN PRIVATE KEY-----/' "private key"

    # S17: what the Roles grant.
    plant "list on Secrets" agent sit1 's/verbs: \["get", "update"\]/verbs: ["get", "list", "update"]/' "the verb list on Secrets"
    plant "watch on Secrets" agent sit1 's/verbs: \["get", "update"\]/verbs: ["get", "watch", "update"]/' "the verb watch on Secrets"
    plant "create on Secrets" agent sit1 's/verbs: \["get", "update"\]/verbs: ["get", "create", "update"]/' "the verb create on Secrets"
    plant "an unnamed Secret grant" agent sit1 '/resourceNames: \["lanekeeper-agent-cert"\]/d' "without resourceNames"
    plant "another Secret" agent sit1 's/resourceNames: \["lanekeeper-agent-cert"\]/resourceNames: ["lanekeeper-db-app"]/' "lanekeeper-db-app"
    plant "delete on Deployments" agent sit1 's/verbs: \["patch"\]/verbs: ["patch", "delete"]/' "the verb delete on deployments"
    plant "a wildcard" agent sit1 's/resources: \["jobs"\]/resources: ["*"]/' "a wildcard"
    plant "a binding to a ClusterRole" agent sit1 's/^  kind: Role$/  kind: ClusterRole/' "binds a ClusterRole"
    plant "a binding to a service account elsewhere" agent sit1 's/^    namespace: lanekeeper$/    namespace: kube-system/' "the agent's namespace"
    plant_append "a ClusterRole" agent sit1 $'apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: lanekeeper-agent-everything\nrules:\n  - apiGroups: [""]\n    resources: ["secrets"]\n    verbs: ["get", "list"]' "ClusterRole"
    plant_append "a Role in a namespace the agent does not watch" agent sit1 $'apiVersion: rbac.authorization.k8s.io/v1\nkind: Role\nmetadata:\n  name: lanekeeper-agent-workloads\n  namespace: kube-system\nrules:\n  - apiGroups: ["apps"]\n    resources: ["deployments"]\n    verbs: ["get"]' "kube-system"

    # S17, S18: the network.
    plant "egress only" agent sit1 's/policyTypes: \[Ingress, Egress\]/policyTypes: [Egress]/' "not default deny"
    plant "the hub on a wide block" agent sit1 's#cidr: "203.0.113.10/32"#cidr: "203.0.0.0/16"#' "unrecognised"
    plant "the internet as the API server" agent sit1 's#cidr: "198.51.100.2/32"#cidr: "0.0.0.0/0"#' "unrecognised"
    plant "the config-server on another port" agent sit1 's/port: 8888/port: 8889/' "egress must be exactly"
    plant "ssh to the hub" agent sit1 '0,/port: 443/s/port: 443/port: 22/' "egress must be exactly"
    plant "ingress from another namespace" agent sit1 's#kubernetes.io/metadata.name: monitoring#kubernetes.io/metadata.name: default#' "monitoring"
    plant "ingress on another port" agent sit1 's/port: 9090}/port: 8080}/' "9090"
    plant_append "a FQDN policy" agent sit1 $'apiVersion: networking.gke.io/v1alpha1\nkind: FQDNNetworkPolicy\nmetadata:\n  name: lanekeeper-agent-external\n  namespace: lanekeeper\nspec:\n  podSelector:\n    matchLabels:\n      app.kubernetes.io/name: lanekeeper-agent\n  egress:\n    - matches:\n        - pattern: example.com\n      ports:\n        - {protocol: TCP, port: 443}' "FQDNNetworkPolicy"
    plant_append "a second door" agent sit1 $'apiVersion: v1\nkind: Service\nmetadata:\n  name: lanekeeper-agent\n  namespace: lanekeeper\nspec:\n  type: LoadBalancer\n  ports:\n    - {port: 9090}' "LoadBalancer"

    echo "== planted: the contract check refuses what the files do not say"
    plant_contract "the workload Role gives less than RBAC.md" 's/verbs: \["patch"\]/verbs: ["get"]/' "differs from RBAC.md"
    plant_contract "the workload Role gives more than RBAC.md" 's/resources: \["jobs"\]/resources: ["jobs", "cronjobs"]/' "differs from RBAC.md"
    plant_contract "a watched namespace without a Role" 's/^( *)value: "sit1"$/\1value: "sit1,sit2"/' "sit2"
    plant_contract "the config-server's port" 's/port: 8888/port: 8889/' "differs from HARDENING.md"
    plant_contract "the health port" 's/port: 9090}/port: 8080}/' "differs from HARDENING.md"
    plant_contract "the liveness probe" 's/periodSeconds: 20/periodSeconds: 60/' "livenessProbe"
    plant_contract "the readiness probe" 's#path: /readyz#path: /healthz#' "readinessProbe"
    plant_contract "the container's security context" 's/readOnlyRootFilesystem: true/readOnlyRootFilesystem: false/' "securityContext"
    plant_contract "no token" 's/^( *)automountServiceAccountToken: true$/\1automountServiceAccountToken: false/' "automountServiceAccountToken"
    plant_contract "the binding's role" '/^  kind: Role$/{n;s/name: lanekeeper-agent-secrets/name: lanekeeper-agent-workloads/}' "roleRef"
}

# accept_variant NAME HELM_ARG...: the agent chart with other values must still pass every check (kubeconform, the policies and the
# contract), so that the checks above are not passing only because the chart has one shape. The output is left in $work/variant.yaml.
accept_variant() {
    local name="$1"
    shift
    local out="$work/variant.yaml" ok=1
    if ! render agent sit1 "$@" >"$out" 2>"$work/variant.err"; then
        fail "$name: the chart did not render: $(tail -n1 "$work/variant.err")"
        return 1
    fi
    kubeconform "${schema_args[@]}" "$out" >/dev/null || { fail "$name: kubeconform refused it"; ok=0; }
    check_policy "$out" agent sit1 >/dev/null 2>&1 || { fail "$name: the policies refused it"; ok=0; }
    check_contract "$out" >/dev/null 2>&1 || { fail "$name: the contract check refused it"; ok=0; }
    [ "$ok" = 1 ] && echo "  ok  $name"
    return 0
}

# in_variant REGEX NAME: the last variant's output has a line that matches.
in_variant() {
    grep -Eq -- "$1" "$work/variant.yaml" || fail "$2: the output does not have a line matching '$1'"
}

agent_variants() {
    echo "== the agent chart with other values still passes every check"
    accept_variant "Workload Identity only (the default)"
    in_variant '^ +value: "workload-identity"$' "Workload Identity only"
    accept_variant "the join token and Workload Identity" --set joinToken.enabled=true
    in_variant '^ +value: "auto"$' "the join token and Workload Identity"
    in_variant '^ +resourceNames: \["lanekeeper-agent-join-token"\]$' "the join token and Workload Identity"
    in_variant '^ +- name: LK_JOIN_TOKEN_SECRET$' "the join token and Workload Identity"
    accept_variant "the join token alone, on a cluster without Workload Identity" --set joinToken.enabled=true --set serviceAccount.gcpServiceAccount=
    in_variant '^ +value: "token"$' "the join token alone"
    ! grep -q 'iam.gke.io/gcp-service-account' "$work/variant.yaml" || fail "the join token alone: the service account has a Workload Identity annotation"
    accept_variant "two watched namespaces" --set-json 'namespaces=["sit1","sit2"]'
    local ns
    for ns in sit1 sit2; do
        [ "$(grep -c "^  namespace: $ns\$" "$work/variant.yaml")" = 2 ] || fail "two watched namespaces: expected a Role and a RoleBinding in $ns"
    done
    in_variant '^ +value: "sit1,sit2"$' "two watched namespaces"
    accept_variant "other ids for the export's owner" --set nfs.uid=2020 --set nfs.gid=2021
    in_variant '^ +fsGroup: 2021$' "other ids"
    accept_variant "several addresses for the hub" --set-json 'network.hubAddresses=["203.0.113.10/32","203.0.113.11/32","192.0.2.0/24"]'
    accept_variant "tunables" --set-json 'config.tuning={"LK_LOG":"debug","LK_SYNC_JOB_LABEL":"sync=true","LK_POOL_THREADS":2}'
    in_variant '^ +- name: LK_LOG$' "tunables"
    accept_variant "an image pull secret" --set-json 'imagePullSecrets=["ghcr-pull"]'
    in_variant '^ +- name: "ghcr-pull"$' "an image pull secret"
    accept_variant "a different hub audience" --set hub.audience=https://hub-audience.example.com
    in_variant 'value: "https://hub-audience.example.com"' "a different hub audience"
}

# The agent chart must stop before it writes a manifest that breaks a rule it owns. $1 is the case, $2 the fragment of the message,
# and the rest are helm arguments; render_agent_plain does not add the synthetic CA, for the cases about the CA.
expect_agent_failure() {
    local name="$1" fragment="$2"
    shift 2
    local out
    if out="$(render agent sit1 "$@" 2>&1 >/dev/null)"; then
        fail "$name: helm rendered it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

expect_agent_failure_without_ca() {
    local name="$1" fragment="$2"
    shift 2
    local out
    if out="$(helm template lanekeeper deploy/charts/agent --namespace lanekeeper --kube-version "$kube_version" -f deploy/values/agent/sit1.yaml "$@" 2>&1 >/dev/null)"; then
        fail "$name: helm rendered it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

planted_agent_templates() {
    echo "== planted: the agent templates and schema refuse values that break a rule"
    expect_agent_failure "no image digest" "digest" --set image.digest=
    expect_agent_failure "a short digest" "digest" --set image.digest=sha256:abc
    expect_agent_failure "a tag instead of a digest" "digest" --set image.digest=latest
    expect_agent_failure_without_ca "no CA for the hub" "hubCa.pem is required"
    expect_agent_failure "a private key as the CA" "private key" --set-literal $'hubCa.pem=-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----'
    expect_agent_failure "a CA bundle with a private key in it" "private key" --set-literal $'hubCa.pem=-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----'
    expect_agent_failure "a CA that is not a certificate" "not a PEM certificate" --set-literal hubCa.pem=hello
    expect_agent_failure "no way to join" "no way to join" --set serviceAccount.gcpServiceAccount=
    expect_agent_failure "a Google service account that is not one" "gcpServiceAccount" --set serviceAccount.gcpServiceAccount=someone@example.com
    expect_agent_failure "no address for the hub" "hubAddresses" --set-json 'network.hubAddresses=[]'
    expect_agent_failure "a wide block for the hub" "hubAddresses" --set-json 'network.hubAddresses=["203.0.113.0/16"]'
    expect_agent_failure "the internet as the hub" "hubAddresses" --set-json 'network.hubAddresses=["0.0.0.0/0"]'
    expect_agent_failure "a bare address for the hub" "hubAddresses" --set-json 'network.hubAddresses=["203.0.113.10"]'
    expect_agent_failure "no address for the API server" "kubeApiAddresses" --set-json 'network.kubeApiAddresses=[]'
    expect_agent_failure "the internet as the API server" "kubeApiAddresses" --set-json 'network.kubeApiAddresses=["0.0.0.0/0"]'
    expect_agent_failure "no namespace" "namespaces" --set-json 'namespaces=[]'
    expect_agent_failure "kube-system" "namespaces" --set-json 'namespaces=["kube-system"]'
    expect_agent_failure "a namespace twice" "namespaces" --set-json 'namespaces=["sit1","sit1"]'
    expect_agent_failure "a namespace that is not a name" "namespaces" --set-json 'namespaces=["Sit 1"]'
    expect_agent_failure "root as the owner of the export" "uid" --set nfs.uid=0
    expect_agent_failure "an export that is not a path" "exportPath" --set nfs.exportPath=export
    expect_agent_failure "a hub that is not https" "endpoint" --set hub.endpoint=http://hub.example.com
    expect_agent_failure "a hub with credentials" "endpoint" --set hub.endpoint=https://user:pass@hub.example.com
    expect_agent_failure "a hub with a path" "endpoint" --set hub.endpoint=https://hub.example.com/agents
    expect_agent_failure "a swimlane id that is not one" "id" --set swimlane.id=Bad_Id
    expect_agent_failure "a tunable the agent does not have" "LK_CERT_SECRET" --set-json 'config.tuning={"LK_CERT_SECRET":"x"}'
    expect_agent_failure "a path as a tunable" "LK_NFS_ROOT" --set-json 'config.tuning={"LK_NFS_ROOT":"/x"}'
    expect_agent_failure "a log level that is not one" "LK_LOG" --set-json 'config.tuning={"LK_LOG":"loud"}'
    expect_agent_failure "a replica count" "replicas" --set replicas=2
    expect_agent_failure "an unknown value" "not_a_value" --set not_a_value=1
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
    for kind in GCPBackendPolicy HealthCheckPolicy FQDNNetworkPolicy ComputeSecurityPolicy Gateway StatefulSet Deployment Role RoleBinding NetworkPolicy ConfigMap; do
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
    planted_agent_templates
    planted_agent
    agent_variants
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
    contract) contract_all ;;
    planted) planted_all ;;
    all)
        status=0
        lint || status=1
        kubeconform_all || status=1
        policy_all || status=1
        contract_all || status=1
        planted_all || status=1
        exit "$status"
        ;;
esac
