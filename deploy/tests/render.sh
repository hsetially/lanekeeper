#!/usr/bin/env bash
# Render every chart with every values file, and check what comes out (09/T3 to T6; S6, S7, S9, S10, S16, S18, S19, S20, P11).
#
#   deploy/tests/render.sh lint      helm lint --strict
#   deploy/tests/render.sh render    helm template | kubeconform -strict, offline, with the vendored schemas
#   deploy/tests/render.sh policy    helm template | conftest (deploy/policy), with perf/budgets.toml and the values files
#   deploy/tests/render.sh contract  the agent chart's Roles, NetworkPolicy and pod against crates/agent/RBAC.md and HARDENING.md,
#                                    and the db, gcp and hub charts against one another (bucket, service account, disk keys)
#   deploy/tests/render.sh planted   the checks above must FAIL on planted violations, each for its own reason
#   deploy/tests/render.sh data      the data layer (09/T5) alone: the db chart and the gcp chart without Cloud Armor, through lint,
#                                    kubeconform and the policies, their planted violations, and the cross-chart check. A SUBSET of
#                                    `all`, for the part that does not wait for GitHub's hook ranges (B5); it is never the gate.
#   deploy/tests/render.sh argocd    the Argo CD files (09/T6) alone: kubeconform, the policies, the paths, the scope of each project against the
#                                    rendered charts, and their planted violations. A SUBSET of `all`: the scope of the gcp chart with Cloud
#                                    Armor waits for GitHub's ranges (B5) and is left out of it; it is never the gate.
#   deploy/tests/render.sh all       all of them
#
# The Argo CD files (deploy/argocd, 09/T6) go through the same stages: kubeconform (render), then the argocd policy, the files each Application
# names and the scope of each project against the charts it deploys (policy), and planted changes to the real files (planted).
#
# What it needs: helm and kubeconform (`just ops-tools-install helm kubeconform`), Docker (conftest runs from the image pinned in
# deploy/images/tools.env), node. It never skips a check and never fetches a schema: kubeconform has no -ignore-missing-schemas,
# so a kind without a vendored schema is an error (deploy/schemas).
#
# The gcp chart is also rendered as `gcp-storage`: the same chart and values with Cloud Armor off (`--set cloudArmor.enabled=false`), so
# that everything in it but Cloud Armor (the buckets, keys and IAM of 09/T5, the admission policy of T6) is checked on its own. That is an ADDITION to the gcp chart's own checks, never a
# replacement: the gcp chart with the committed values still refuses to render until GitHub's hook ranges exist (plan 09 blockers, B5),
# and every stage reports that failure. `gcp-storage` renders no ComputeSecurityPolicy (checked), and no committed gcp values file
# turns Cloud Armor off (checked), so it cannot stand in for the chart.
#
# Warnings. A policy warning is a fallback that is allowed to render (plan 09 A11, A13): kind has no zone, no Gateway and no FQDN
# policy, and a cluster without a CMEK key uses Google-managed encryption. In prod and staging a warning fails the run. Only the warnings
# of the images and binauthz policies are tolerated there, and the Argo CD policy's: the placeholder digest and revision (plan 09 A29) until
# the first publish, and Binary Authorization in dry run until Q24 is answered (A5). All of them are printed on every run.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export PATH="$PWD/target/ops-tools/bin:$PATH"

mode="${1:-all}"
case "$mode" in lint | render | policy | contract | planted | data | argocd | all) ;; *)
    echo "usage: deploy/tests/render.sh lint|render|policy|contract|planted|data|argocd|all" >&2
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
# Where the values files are. Only a test of this script changes it (the synthetic key stands in only while the values have none).
values_root=deploy/values
# Every namespace of the manifest policies. dockerfile.rego is for Dockerfiles and is run by `just images-policy-test`; contract.rego
# needs the two contract files and is run apart (`contract` mode).
policy_namespaces=(hardening secrets topology resources storage network rbac database iam)
# The agent chart refuses to render without the hub's CA, which no values file carries (deploy/tests/testdata/README.md).
synthetic_hub_ca=deploy/tests/testdata/synthetic-hub-ca.pem
# The gcp chart refuses to render without the public key of the CI signing key, which no values file carries until a person makes the
# key. This file stands in for it, and only while the environment's values have none (09/T6; deploy/tests/testdata/README.md).
synthetic_attestor_key=deploy/tests/testdata/synthetic-attestor-key.yaml

conftest() { docker run --rm -i -v "$PWD:/repo:ro" -v "$work:/work:ro" -w /repo "$LK_CONFTEST_IMAGE" "$@"; }

# The charts and the environments that have values. The hook ranges file lives next to the hub's values but belongs to the gcp
# chart, so it is not an environment.
combos=()
for chart in hub db gcp; do
    for env in kind staging prod; do
        [ -f "deploy/values/$chart/$env.yaml" ] && combos+=("$chart:$env")
    done
done
# The gcp chart without Cloud Armor (see above): its buckets, keys and IAM, which do not depend on GitHub's ranges.
for env in staging prod; do
    [ -f "deploy/values/gcp/$env.yaml" ] && combos+=("gcp-storage:$env")
done
# The agent has one values file per swimlane (deploy/values/agent/<swimlane>.yaml).
for f in deploy/values/agent/*.yaml; do
    [ -f "$f" ] && combos+=("agent:$(basename "$f" .yaml)")
done
[ ${#combos[@]} -gt 0 ] || {
    echo "render.sh: no values files under deploy/values" >&2
    exit 1
}

namespace_of() { case "$1" in hub | agent | db) echo lanekeeper ;; gcp | gcp-storage) echo lanekeeper-gcp ;; esac; }

# The Helm release name is the name of the Argo CD Application that deploys the chart (deploy/argocd), so the labels that the policies see
# are the labels that the cluster gets.
release_of() { case "$1" in hub | db | agent) echo "lanekeeper-$1" ;; gcp | gcp-storage) echo lanekeeper-gcp ;; esac; }

# The chart (and the values directory) of a name that is a chart or the gcp chart without Cloud Armor.
chart_dir() { case "$1" in gcp-storage) echo gcp ;; *) echo "$1" ;; esac; }

# value_args CHART ENV: the arguments for the values, in the order they apply.
value_args() {
    local chart="$1" env="$2"
    [ "$chart" = gcp ] && printf -- '-f\n%s\n' "$hook_ranges"
    printf -- '-f\n%s\n' "$values_root/$(chart_dir "$chart")/$env.yaml"
    # The synthetic key stands in only while the values have none: once a person puts the real public key in, that key is what is checked.
    if [ "$chart" = gcp ] || [ "$chart" = gcp-storage ]; then
        grep -q '^ *publicKeyPem:' "$values_root/gcp/$env.yaml" || printf -- '-f\n%s\n' "$synthetic_attestor_key"
    fi
    [ "$chart" = gcp-storage ] && printf -- '--set\ncloudArmor.enabled=false\n'
    [ "$chart" = agent ] && printf -- '--set-file\nhubCa.pem=%s\n' "$synthetic_hub_ca"
    return 0
}

# render CHART ENV [helm args...] > output
render() {
    local chart="$1" env="$2"
    shift 2
    local args=()
    mapfile -t args < <(value_args "$chart" "$env")
    helm template "$(release_of "$chart")" "deploy/charts/$(chart_dir "$chart")" --namespace "$(namespace_of "$chart")" --kube-version "$kube_version" "${args[@]}" "$@"
}

# check_policy FILE CHART ENV: conftest over one rendered file. Prints the output; returns conftest's status.
check_policy() {
    local rendered="$1" chart="$2" env="$3" status=0
    local files=("$rendered" perf/budgets.toml "deploy/values/$(chart_dir "$chart")/$env.yaml" "deploy/charts/$(chart_dir "$chart")/values.yaml")
    local data=()
    if [ "$chart" = gcp ]; then
        data=(--data "$hook_ranges")
        files+=("$hook_ranges")
    fi
    local ns=() n
    for n in "${policy_namespaces[@]}"; do ns+=(--namespace "$n"); done
    local fail_on_warn=()
    [ "$env" = kind ] || fail_on_warn=(--fail-on-warn)
    # The images and binauthz policies are run apart: their warnings are fallbacks that may render in every environment, and are printed
    # on every run: the placeholder digest until the first publish (plan 09 A29), and Binary Authorization in dry-run until Q24 is answered (A5).
    conftest test --combine "${fail_on_warn[@]}" --policy deploy/policy "${ns[@]}" "${data[@]}" --no-color "/work/${rendered##*/}" "${files[@]:1}" || status=$?
    conftest test --combine --policy deploy/policy --namespace images --namespace binauthz --no-color "/work/${rendered##*/}" || status=$?
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

# Which of the combinations a stage runs for: all of them, except in `data` mode.
combo_filter='.'

# Every stage runs for every chart and environment, and reports every failure, so that one broken values file does not hide the
# next. (Inside a function that is called from `||`, `set -e` is off, so each command says `|| return 1` for itself.)
each_combo() {
    local fn="$1" combo chart env status=0
    for combo in "${combos[@]}"; do
        [[ "$combo" =~ $combo_filter ]] || continue
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
    helm lint --strict "deploy/charts/$(chart_dir "$chart")" --namespace "$(namespace_of "$chart")" --kube-version "$kube_version" "${args[@]}" || return 1
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
    if out="$(helm template lanekeeper-agent deploy/charts/agent --namespace lanekeeper --kube-version "$kube_version" -f deploy/values/agent/sit1.yaml "$@" 2>&1 >/dev/null)"; then
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

# --- the data layer (09/T5): the db chart and the gcp chart's buckets, keys and IAM ---------------------------------------------------------

# expect_render_failure NAME CHART ENV FRAGMENT [helm args...]: helm must stop before it writes a manifest that breaks a rule the chart owns.
expect_render_failure() {
    local name="$1" chart="$2" env="$3" fragment="$4" out
    shift 4
    if out="$(render "$chart" "$env" "$@" 2>&1 >/dev/null)"; then
        fail "$name: helm rendered it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

planted_db_templates() {
    echo "== planted: the db templates and schema refuse values that break a rule"
    expect_render_failure "no image digest" db prod "digest" --set image.digest=
    expect_render_failure "a short digest" db prod "digest" --set image.digest=sha256:abc
    expect_render_failure "a tag instead of a digest" db prod "digest" --set image.digest=latest
    expect_render_failure "an image of another project" db prod "repository" --set image.repository=docker.io/library/postgres
    expect_render_failure "no environment" db prod "environment" --set environment=
    expect_render_failure "an environment that is not one" db prod "environment" --set environment=dev
    expect_render_failure "no backups in prod" db prod "only kind may run without backups" --set backup.enabled=false
    expect_render_failure "no backups in staging" db staging "only kind may run without backups" --set backup.enabled=false
    expect_render_failure "no bucket for the backups" db prod "backup.bucket is required" --set backup.bucket=
    expect_render_failure "no Google service account for the backups" db prod "gcpServiceAccount is required" --set serviceAccount.gcpServiceAccount=
    expect_render_failure "a Google service account that is not one" db prod "gcpServiceAccount" --set serviceAccount.gcpServiceAccount=someone@example.com
    expect_render_failure "a bucket that is not a bucket name" db prod "bucket" --set backup.bucket=Bad_Bucket
    expect_render_failure "a schedule that is not daily" db prod "schedule" --set-literal backup.schedule='*/5 * * * *'
    expect_render_failure "spread by host outside kind" db prod "for kind only" --set topology.mode=kind
    expect_render_failure "spread over zones in kind" db kind "topology.mode must be kind" --set topology.mode=zone
    expect_render_failure "one Secret for both roles" db prod "same Secret" --set secrets.migratorSecretName=lanekeeper-db-app
    expect_render_failure "the migrator in the app's Secret" db prod "the name the operator and the hub use" --set secrets.appSecretName=other --set secrets.migratorSecretName=lanekeeper-db-app
    expect_render_failure "a malformed CMEK key" db prod "cmekKey" --set storage.cmekKey=not-a-key
    expect_render_failure "a size that is not one" db prod "size" --set storage.size=lots
    expect_render_failure "a shared_buffers that is not one" db prod "sharedBuffers" --set postgresql.sharedBuffers=lots
    expect_render_failure "an unknown value" db prod "not_a_value" --set not_a_value=1
    expect_render_failure "a password in the values" db prod "password" --set secrets.password=hunter2
}

planted_db() {
    echo "== planted: the policies refuse the real output of the db chart with one thing wrong"
    local good="$work/good-db-prod.yaml"
    render db prod >"$good"
    echo "-- the unchanged output passes"
    check_policy "$good" db prod >/dev/null || fail "the unchanged prod render of the db chart was refused"
    render db kind >"$work/good-db-kind.yaml"
    check_policy "$work/good-db-kind.yaml" db kind >/dev/null || fail "the unchanged kind render of the db chart was refused"

    # S6: TLS, and who may log in how.
    plant "TLS 1.2 allowed" db prod 's/ssl_min_protocol_version: TLSv1.3/ssl_min_protocol_version: TLSv1.2/' "ssl_min_protocol_version"
    plant "TLS 1.2 as the highest" db prod 's/ssl_max_protocol_version: TLSv1.3/ssl_max_protocol_version: TLSv1.2/' "ssl_max_protocol_version"
    plant "a first rule that lets non-TLS in" db prod 's/^( *)- hostnossl all all all reject$/\1- host all all all scram-sha-256/' "first rule of postgresql.pg_hba"
    plant "trust for an address" db prod 's/^( *)- hostnossl all all all reject$/&\n\1- hostssl all all 192.0.2.0\/24 trust/' "trust"
    plant "md5" db prod 's/^( *)- hostnossl all all all reject$/&\n\1- hostssl lanekeeper lanekeeper_app all md5/' "md5"
    plant "weak password hashing" db prod 's/password_encryption: scram-sha-256/password_encryption: md5/' "password_encryption"
    plant "a parameter that is not ours" db prod 's/^( *)shared_buffers: "512MB"$/&\n\1fsync: "off"/' "postgresql.parameters.fsync"
    plant "superuser access" db prod 's/enableSuperuserAccess: false/enableSuperuserAccess: true/' "enableSuperuserAccess"

    # D62: the topology.
    plant "two instances" db prod 's/instances: 3/instances: 2/' "3 instances"
    plant "no PodDisruptionBudget" db prod 's/enablePDB: true/enablePDB: false/' "enablePDB"
    plant "spread by host in prod" db prod 's#topologyKey: topology.kubernetes.io/zone#topologyKey: kubernetes.io/hostname#' "topology.kubernetes.io/zone"
    plant "a preferred spread in prod" db prod 's/podAntiAffinityType: required/podAntiAffinityType: preferred/' "required"
    plant "anti-affinity off" db prod 's/enablePodAntiAffinity: true/enablePodAntiAffinity: false/' "enablePodAntiAffinity"

    # S19: no tag.
    plant "image by tag" db prod 's/@sha256:[0-9a-f]{64}//' "not pinned by digest"
    plant "image of another project" db prod 's#ghcr.io/cloudnative-pg/postgresql:#docker.io/library/postgres:#' "ghcr.io/cloudnative-pg/postgresql"

    # D124, db/migrations/README.md: the database and its roles.
    plant "collation C" db prod 's/localeCollate: en_US.utf8/localeCollate: C/' "en_US.utf8"
    plant "no pgvector" db prod '/CREATE EXTENSION IF NOT EXISTS vector/d' "vector"
    plant "no pg_trgm" db prod '/CREATE EXTENSION IF NOT EXISTS pg_trgm/d' "pg_trgm"
    plant "a grant at bootstrap" db prod 's/^( *)- CREATE EXTENSION IF NOT EXISTS pg_trgm$/&\n\1- GRANT ALL ON SCHEMA public TO lanekeeper_app/' "postInitApplicationSQL"
    plant "the app role owns the database" db prod 's/owner: lanekeeper_migrator/owner: lanekeeper_app/' "lanekeeper_migrator"
    plant "one Secret for both roles" db prod 's/^( *)name: lanekeeper-db-migrator$/\1name: lanekeeper-db-app/' "same Secret"
    plant "no Secret for the owner" db prod '/^      secret:$/,/^        name: lanekeeper-db-migrator$/d' "initdb.secret.name is empty"
    plant "the app role is a superuser" db prod 's/superuser: false/superuser: true/' "superuser"
    plant "the app role creates databases" db prod 's/createdb: false/createdb: true/' "createdb"
    plant "the app role creates roles" db prod 's/createrole: false/createrole: true/' "createrole"
    plant "the app role replicates" db prod 's/replication: false/replication: true/' "replication"
    plant "the app role bypasses row security" db prod 's/bypassrls: false/bypassrls: true/' "bypassrls"
    plant "the app role cannot log in" db prod 's/login: true/login: false/' "login"
    plant "the app role has no password" db prod '/^        passwordSecret:$/,/^          name: lanekeeper-db-app$/d' "passwordSecret"
    plant "another login role" db prod 's/^( *)- name: lanekeeper_app$/\1- name: analyst\n&/' "another managed role"

    # D29, D40, S7, S10: the backups.
    plant "no backup configuration" db prod 's/^  backup:$/  notbackup:/' "no backup is configured"
    plant "backups kept for a week" db prod 's/retentionPolicy: "30d"/retentionPolicy: "7d"/' "30d"
    plant "backups to another cloud" db prod 's#destinationPath: "gs://#destinationPath: "s3://#' "gs://"
    plant "backups to the root of a bucket" db prod 's#destinationPath: "gs://([^/"]+)/lanekeeper-db"#destinationPath: "gs://\1"#' "path"
    plant "backups with a key file" db prod 's/gkeEnvironment: true/applicationCredentials: {name: gcs-key, key: key.json}/' "applicationCredentials"
    plant "backups without Workload Identity" db prod 's/gkeEnvironment: true/gkeEnvironment: false/' "gkeEnvironment"
    plant "no Google service account" db prod '/iam.gke.io\/gcp-service-account/d' "iam.gke.io/gcp-service-account"
    plant "no scheduled backup" db prod 's/^kind: ScheduledBackup$/kind: Backup/' "ScheduledBackup"
    plant "a suspended scheduled backup" db prod 's/^  immediate: true$/&\n  suspend: true/' "suspend"
    plant "a weekly scheduled backup" db prod 's/schedule: "0 0 2 \* \* \*"/schedule: "0 0 2 * * 1"/' "daily"

    # S16: the pods the operator makes.
    plant "seccomp" db prod 's/^( *)type: RuntimeDefault$/\1type: Unconfined/' "seccompProfile"
    plant "allowPrivilegeEscalation" db prod 's/allowPrivilegeEscalation: false/allowPrivilegeEscalation: true/' "allowPrivilegeEscalation"
    plant "privileged" db prod 's/privileged: false/privileged: true/' "privileged"
    plant "readOnlyRootFilesystem" db prod 's/readOnlyRootFilesystem: true/readOnlyRootFilesystem: false/' "readOnlyRootFilesystem"
    plant "runAsNonRoot" db prod 's/runAsNonRoot: true/runAsNonRoot: false/' "runAsNonRoot"
    plant "drop ALL" db prod 's/drop: \["ALL"\]/drop: ["NET_RAW"]/' "drop ALL"
    plant "requests.cpu" db prod '/^      cpu: "1"$/d' "resources.requests.cpu"
    plant "limits.cpu" db prod '/^      cpu: "2"$/d' "resources.limits.cpu"
    plant "limits.memory" db prod '/^      memory: 2Gi$/d' "resources.limits.memory"

    # S10: the disks.
    plant "an unencrypted volume class" db prod 's/storageClass: lanekeeper-db-cmek/storageClass: standard-rwo/' "customer-managed"
    plant "a class that deletes the disk with its claim" db prod 's/reclaimPolicy: Retain/reclaimPolicy: Delete/' "Retain"
    plant "no key on the class" db prod '/disk-encryption-kms-key:/d' "customer-managed"

    # S18: who may reach the database.
    plant "no default deny" db prod 's/policyTypes: \[Ingress\]/policyTypes: []/' "not default deny"
    plant "every pod of the namespace" db prod 's/^( *)cnpg.io\/cluster: lanekeeper-db$/\1foo: bar/' "selects pods other than"
    plant "another app than the hub" db prod 's#app.kubernetes.io/name: lanekeeper-hub#app.kubernetes.io/name: other#' "ingress must be exactly"
    plant "the hub on ssh" db prod '0,/port: 5432/s/port: 5432/port: 22/' "ingress must be exactly"
    plant "the operator of any namespace" db prod 's#^( *)kubernetes.io/metadata.name: cnpg-system$#\1foo: bar#' "ingress must be exactly"
    plant "an environment that is not one" db prod 's#lanekeeper.io/environment: prod#lanekeeper.io/environment: dev#' "lanekeeper.io/environment"
    plant_append "a second policy that admits everyone" db prod $'apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: lanekeeper-db-open\n  namespace: lanekeeper\nspec:\n  podSelector:\n    matchLabels:\n      cnpg.io/cluster: lanekeeper-db\n  policyTypes: [Ingress]\n  ingress:\n    - ports:\n        - {protocol: TCP, port: 5432}' "policies add up"
    plant_append "a second cluster" db prod $'apiVersion: postgresql.cnpg.io/v1\nkind: Cluster\nmetadata:\n  name: other-db\n  namespace: lanekeeper\nspec:\n  instances: 3' "the only database is lanekeeper-db"

    # kind has no Google, but it does not excuse a weaker secret handling.
    plant "kind with a key file" db kind 's/^  affinity:$/  backup:\n    retentionPolicy: "30d"\n    barmanObjectStore:\n      destinationPath: "gs:\/\/b-bucket\/p"\n      googleCredentials: {applicationCredentials: {name: k, key: k}}\n&/' "applicationCredentials"
}

# --- charts/gcp: buckets, keys and IAM, without Cloud Armor ------------------------------------------------------------------------------

planted_gcp_templates() {
    echo "== planted: the gcp templates and schema refuse values that break a rule (buckets, keys and IAM)"
    expect_render_failure "no environment" gcp-storage prod "environment is required" --set environment=
    expect_render_failure "an environment that is not one" gcp-storage prod "environment" --set environment=dev
    expect_render_failure "no location" gcp-storage prod "location is required" --set location=
    expect_render_failure "no project number with keys" gcp-storage prod "projectNumber is required" --set projectNumber=
    expect_render_failure "a bucket key that is not a key" gcp-storage prod "cmekKey" --set buckets.docs.cmekKey=nonsense
    expect_render_failure "a bucket key in another location" gcp-storage prod "a key must be in the location of the bucket" --set buckets.docs.cmekKey=projects/example-project/locations/other-place/keyRings/r/cryptoKeys/k
    expect_render_failure "a disk key that is not a key" gcp-storage prod "diskKeys" --set-json 'cmek.diskKeys=["nonsense"]'
    expect_render_failure "a disk key in another location" gcp-storage prod "a key must be in the location of the bucket" --set-json 'cmek.diskKeys=["projects/example-project/locations/other-place/keyRings/r/cryptoKeys/k"]'
    expect_render_failure "no retention period" gcp-storage prod "retentionPeriodSeconds is required" --set buckets.auditCheckpoint.retentionPeriodSeconds=0
    expect_render_failure "a negative retention period" gcp-storage prod "retentionPeriodSeconds" --set buckets.auditCheckpoint.retentionPeriodSeconds=-1
    expect_render_failure "a lock in staging" gcp-storage staging "isLocked is true outside prod" --set buckets.auditCheckpoint.isLocked=true
    expect_render_failure "no bucket name for the backups" gcp-storage prod "buckets.backup.name is required" --set buckets.backup.name=
    expect_render_failure "no bucket name for the audit checkpoints" gcp-storage prod "buckets.auditCheckpoint.name is required" --set buckets.auditCheckpoint.name=
    expect_render_failure "no bucket name for the docs" gcp-storage prod "buckets.docs.name is required" --set buckets.docs.name=
    expect_render_failure "a bucket name that is not one" gcp-storage prod "name" --set buckets.docs.name=Bad_Name
    expect_render_failure "a Kubernetes service account that is not a name" gcp-storage prod "hubServiceAccount" --set workloadIdentity.hubServiceAccount=Bad_Name
    expect_render_failure "an unknown value" gcp-storage prod "not_a_value" --set not_a_value=1
}

planted_gcp_storage() {
    echo "== planted: the policies refuse the real output of the gcp chart (buckets, keys and IAM) with one thing wrong"
    local good="$work/good-gcp-storage-prod.yaml"
    render gcp-storage prod >"$good"
    echo "-- the unchanged output passes"
    check_policy "$good" gcp-storage prod >/dev/null || fail "the unchanged prod render of the gcp chart without Cloud Armor was refused"
    render gcp-storage staging >"$work/good-gcp-storage-staging.yaml"
    check_policy "$work/good-gcp-storage-staging.yaml" gcp-storage staging >/dev/null || fail "the unchanged staging render of the gcp chart without Cloud Armor was refused"

    local in_bucket='/^kind: StorageBucket$/,/^---$/'
    # S10: no bucket is public, all are encrypted with a key of ours.
    plant "buckets without a key" gcp-storage prod "$in_bucket"'s#^( *)external: .*#\1external: ""#' "customer-managed"
    plant "buckets with a key that is not one" gcp-storage prod "$in_bucket"'s#^( *)external: .*#\1external: "lanekeeper"#' "not a Cloud KMS key"
    plant "public access not prevented" gcp-storage prod "$in_bucket"'s/publicAccessPrevention: enforced/publicAccessPrevention: inherited/' "public access prevention"
    plant "object ACLs allowed" gcp-storage prod "$in_bucket"'s/uniformBucketLevelAccess: true/uniformBucketLevelAccess: false/' "uniform bucket-level access"
    plant "a bucket deleted with its resource" gcp-storage prod "$in_bucket"'s/deletion-policy: abandon/deletion-policy: delete/' "deletion-policy: abandon"
    plant "a bucket that a sync may prune" gcp-storage prod "$in_bucket"'s/Prune=false/Prune=true/' "Prune=false"
    plant "no location" gcp-storage prod "$in_bucket"'{/^  location: /d}' "names no location"
    plant "an environment that is not one" gcp-storage prod "$in_bucket"'s#lanekeeper.io/environment: prod#lanekeeper.io/environment: dev#' "lanekeeper.io/environment"
    plant "a bucket of no purpose" gcp-storage prod "$in_bucket"'s#lanekeeper.io/purpose: docs#lanekeeper.io/purpose: scratch#' "purpose"

    # S9: the audit checkpoint bucket.
    plant "an unlocked audit bucket in prod" gcp-storage prod 's/isLocked: true/isLocked: false/' "not locked in prod"
    plant "no retention policy" gcp-storage prod '/^  retentionPolicy:$/,/^    isLocked: /d' "no retention period"
    plant "a retention period of nothing" gcp-storage prod 's/retentionPeriod: [0-9]+/retentionPeriod: 0/' "must be positive"
    plant "a lock on the docs" gcp-storage prod 's/^  versioning:$/  retentionPolicy: {retentionPeriod: 100, isLocked: true}\n&/' "only the audit-checkpoint bucket may be locked"
    plant "the docs without versioning" gcp-storage prod '/^  versioning:$/,/^    enabled: true$/d' "versioning is not on"

    # S10: the key must be usable by the service that encrypts with it.
    plant "a key without a grant for Cloud Storage" gcp-storage prod '/name: lanekeeper-gcs-agent-5cff04fb0a/,/^---$/s#gs-project-accounts#compute-system#' "no IAMPolicyMember grants the Cloud Storage service agent"

    # S5, S7, S9, S10: IAM.
    plant "a person in a binding" gcp-storage prod 's#member: "serviceAccount:lanekeeper-hub@example-project.iam.gserviceaccount.com"#member: "user:someone@example.com"#' "not a service account"
    plant "everyone in a binding" gcp-storage prod 's#member: "serviceAccount:lanekeeper-hub@example-project.iam.gserviceaccount.com"#member: "allUsers"#' "allUsers"
    plant "owner" gcp-storage prod 's#role: roles/storage.objectUser#role: roles/owner#' "not one of the roles"
    plant "storage admin" gcp-storage prod 's#role: roles/storage.objectUser#role: roles/storage.admin#' "not one of the roles"
    plant "the audit bucket can be overwritten" gcp-storage prod '/name: lanekeeper-hub-audit-create/,/^---$/s#roles/storage.objectCreator#roles/storage.objectUser#' "may only create and read"
    plant "the hub in the backups" gcp-storage prod '/name: lanekeeper-db-backup/,/^---$/s#lanekeeper-db@#lanekeeper-hub@#' "only the database's service account reaches the backups"
    plant "the database in the docs" gcp-storage prod '/name: lanekeeper-hub-docs/,/^---$/s#lanekeeper-hub@#lanekeeper-db@#' "only the hub's service account reaches the docs"
    plant "the backups without delete" gcp-storage prod '/name: lanekeeper-db-backup/,/^---$/s#roles/storage.objectUser#roles/storage.objectCreator#' "needs to read, create and delete"
    plant "a bucket role on the project" gcp-storage prod '/name: lanekeeper-hub-docs/,/^---$/{s#kind: StorageBucket#kind: Project#;s#name: example-project-lanekeeper-docs-prod#external: projects/example-project#}' "granted on a StorageBucket"
    plant "Workload Identity for the default account" gcp-storage prod 's#\[lanekeeper/lanekeeper-hub\]#[lanekeeper/default]#' "default service account"
    plant "Workload Identity of another project's pool" gcp-storage prod 's#serviceAccount:example-project.svc.id.goog#serviceAccount:other-project.svc.id.goog#' "pool of"
    plant "a key granted to the hub" gcp-storage prod '/name: lanekeeper-gcs-agent-5cff04fb0a/,/^---$/s#serviceAccount:service-123456789012@gs-project-accounts.iam.gserviceaccount.com#serviceAccount:lanekeeper-hub@example-project.iam.gserviceaccount.com#' "Google-managed service agent"

    # The fallback (plan 09 A11b, Q29): a bucket without a key renders with a warning, which fails in prod and staging.
    local out
    if ! out="$(render gcp-storage prod --set buckets.docs.cmekKey= 2>&1)"; then
        fail "the fallback (no key for the docs bucket) did not render: $(tail -n1 <<<"$out")"
    else
        printf '%s\n' "$out" >"$work/fallback.yaml"
        expect_policy_failure "the fallback fails in prod" "$work/fallback.yaml" gcp-storage prod "Google-managed"
        check_policy "$work/fallback.yaml" gcp-storage kind >/dev/null 2>&1 || true
    fi

    # What the isolation is and is not.
    echo "== planted: the gcp chart without Cloud Armor cannot stand in for the gcp chart"
    if grep -q '^kind: ComputeSecurityPolicy$' "$good"; then
        fail "the gcp chart without Cloud Armor renders a ComputeSecurityPolicy, so it is not isolated"
    else
        echo "  ok  the isolated render has no Cloud Armor policy"
    fi
    local f
    for f in deploy/values/gcp/*.yaml; do
        if grep -Eq '^ *enabled: *false' "$f"; then
            fail "$f turns something off with 'enabled: false'; the committed gcp values must keep Cloud Armor on, so that the chart refuses until GitHub's ranges exist (B5)"
        fi
    done
    echo "  ok  no committed gcp values file turns Cloud Armor off"
}

# --- charts/gcp: admission (09/T6; S19, S20, plan 09 A5) ----------------------------------------------------------------------------

planted_gcp_admission_templates() {
    echo "== planted: the gcp templates and schema refuse values that break a rule (admission)"
    local ok_key='"keyVersion":"projects/example-project/locations/example-location/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1","signatureAlgorithm":"ECDSA_P256_SHA256"'
    local pem='-----BEGIN PUBLIC KEY-----\nMFkw\n-----END PUBLIC KEY-----\n'
    expect_render_failure "no public key" gcp-storage prod "binaryAuthorization.publicKeys is empty" --set-json 'binaryAuthorization.publicKeys=[]'
    expect_render_failure "a private key inside the public key" gcp-storage prod "holds a private key" --set-json "binaryAuthorization.publicKeys=[{$ok_key,\"publicKeyPem\":\"-----BEGIN PUBLIC KEY-----\\n-----BEGIN PRIVATE KEY-----\\nAAAA\\n-----END PRIVATE KEY-----\\n-----END PUBLIC KEY-----\\n\"}]"
    expect_render_failure "a private key as the key" gcp-storage prod "publicKeyPem" --set-json "binaryAuthorization.publicKeys=[{$ok_key,\"publicKeyPem\":\"-----BEGIN PRIVATE KEY-----\\nAAAA\\n-----END PRIVATE KEY-----\\n\"}]"
    expect_render_failure "a certificate as the key" gcp-storage prod "publicKeyPem" --set-json "binaryAuthorization.publicKeys=[{$ok_key,\"publicKeyPem\":\"-----BEGIN CERTIFICATE-----\\nAAAA\\n-----END CERTIFICATE-----\\n\"}]"
    expect_render_failure "a key version that is not one" gcp-storage prod "keyVersion" --set-json "binaryAuthorization.publicKeys=[{\"keyVersion\":\"my-key\",\"signatureAlgorithm\":\"ECDSA_P256_SHA256\",\"publicKeyPem\":\"$pem\"}]"
    expect_render_failure "a key (not a version) as the key version" gcp-storage prod "keyVersion" --set-json "binaryAuthorization.publicKeys=[{\"keyVersion\":\"projects/p/locations/l/keyRings/r/cryptoKeys/k\",\"signatureAlgorithm\":\"ECDSA_P256_SHA256\",\"publicKeyPem\":\"$pem\"}]"
    expect_render_failure "a weak signature algorithm" gcp-storage prod "signatureAlgorithm" --set-json "binaryAuthorization.publicKeys=[{\"keyVersion\":\"projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1\",\"signatureAlgorithm\":\"RSA_SIGN_PKCS1_2048_SHA256\",\"publicKeyPem\":\"$pem\"}]"
    expect_render_failure "a key without a signature algorithm" gcp-storage prod "signatureAlgorithm" --set-json "binaryAuthorization.publicKeys=[{\"keyVersion\":\"projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1\",\"publicKeyPem\":\"$pem\"}]"
    expect_render_failure "an enforcement mode that is not one" gcp-storage prod "enforcementMode" --set binaryAuthorization.enforcementMode=WARN
    expect_render_failure "no enforcement mode" gcp-storage prod "enforcementMode" --set binaryAuthorization.enforcementMode=
    expect_render_failure "an attestor name that is not a name" gcp-storage prod "attestorName" --set binaryAuthorization.attestorName=Bad_Name
    expect_render_failure "a pattern with no registry" gcp-storage prod "admissionWhitelistPatterns" --set-json 'binaryAuthorization.admissionWhitelistPatterns=["*"]'
    expect_render_failure "a pattern with a * in the middle" gcp-storage prod "admissionWhitelistPatterns" --set-json 'binaryAuthorization.admissionWhitelistPatterns=["ghcr.io/*/x"]'
    expect_render_failure "an unknown admission value" gcp-storage prod "not_a_value" --set binaryAuthorization.not_a_value=1

    # The committed values carry no key, so the chart refuses to render with them alone: that is how a missing key shows (the synthetic key
    # in deploy/tests/testdata is added by render.sh, never by the values). When a person has put the real key in, there is nothing to show.
    local env out
    for env in staging prod; do
        if grep -q '^ *publicKeyPem:' "deploy/values/gcp/$env.yaml"; then
            echo "  (deploy/values/gcp/$env.yaml has its public key, so the refusal without one is not shown)"
        elif out="$(helm template lanekeeper-gcp deploy/charts/gcp --namespace lanekeeper-gcp --kube-version "$kube_version" -f "deploy/values/gcp/$env.yaml" --set cloudArmor.enabled=false 2>&1 >/dev/null)"; then
            fail "the gcp chart rendered with the committed $env values and no public key for the attestor"
        elif ! grep -qF "binaryAuthorization.publicKeys is empty" <<<"$out"; then
            fail "the gcp chart with the committed $env values was refused, but not for the missing public key:"$'\n'"$out"
        else
            echo "  ok  the committed $env values have no public key, and the chart refuses them"
        fi
    done
}

planted_gcp_admission() {
    echo "== planted: the policies refuse the real output of the gcp chart's admission resources with one thing wrong"
    local good="$work/good-gcp-storage-prod.yaml"
    [ -f "$good" ] || render gcp-storage prod >"$good"
    local in_policy='/^kind: BinaryAuthorizationPolicy$/,/^---$/'
    local in_attestor='/^kind: BinaryAuthorizationAttestor$/,/^---$/'
    local in_note='/^kind: ContainerAnalysisNote$/,/^---$/'

    echo "-- the unchanged output passes, and says that it is in dry-run"
    local out
    if ! out="$(check_policy "$good" gcp-storage prod 2>&1)"; then
        fail "the unchanged prod render of the gcp chart was refused: $out"
    elif ! grep -qF "is in dry-run (DRYRUN_AUDIT_LOG_ONLY)" <<<"$out"; then
        fail "the unchanged prod render does not say that Binary Authorization is in dry-run"
    else
        echo "  ok  the committed mode is dry-run, and the policy says so"
    fi
    sed -E 's/enforcementMode: DRYRUN_AUDIT_LOG_ONLY/enforcementMode: ENFORCED_BLOCK_AND_AUDIT_LOG/' "$good" >"$work/enforced.yaml"
    if ! out="$(check_policy "$work/enforced.yaml" gcp-storage prod 2>&1)"; then
        fail "the enforcing render was refused: $out"
    elif grep -qF "dry-run" <<<"$out"; then
        fail "the enforcing render still says that it is in dry-run"
    else
        echo "  ok  the enforcing mode passes, without the dry-run warning"
    fi

    # S20: the default rule.
    plant "the default rule admits everything" gcp-storage prod 's/evaluationMode: REQUIRE_ATTESTATION/evaluationMode: ALWAYS_ALLOW/' "REQUIRE_ATTESTATION"
    plant "an enforcement mode that is not one" gcp-storage prod 's/enforcementMode: DRYRUN_AUDIT_LOG_ONLY/enforcementMode: WARN/' "enforcementMode"
    plant "no attestor is required" gcp-storage prod '/^    requireAttestationsBy:$/,/^      - name: lanekeeper-ci$/d' "requireAttestationsBy is empty"
    plant "an attestor that is not ours" gcp-storage prod 's/^      - name: lanekeeper-ci$/      - name: someone-elses/' "not an attestor in this output"
    plant "Google's global policy off" gcp-storage prod 's/globalPolicyEvaluationMode: ENABLE/globalPolicyEvaluationMode: DISABLE/' "globalPolicyEvaluationMode"
    plant "a rule that admits a namespace" gcp-storage prod 's/^  globalPolicyEvaluationMode: ENABLE$/&\n  kubernetesNamespaceAdmissionRules:\n    lanekeeper:\n      evaluationMode: ALWAYS_ALLOW\n      enforcementMode: ENFORCED_BLOCK_AND_AUDIT_LOG/' "kubernetesNamespaceAdmissionRules"
    plant "a rule that admits a cluster" gcp-storage prod 's/^  globalPolicyEvaluationMode: ENABLE$/&\n  clusterAdmissionRules:\n    europe-west2.some-cluster:\n      evaluationMode: ALWAYS_ALLOW\n      enforcementMode: ENFORCED_BLOCK_AND_AUDIT_LOG/' "clusterAdmissionRules"
    plant "our images skip the check" gcp-storage prod 's#^  globalPolicyEvaluationMode: ENABLE$#&\n  admissionWhitelistPatterns:\n    - namePattern: "ghcr.io/hsetially/lanekeeper/*"#' "covers ghcr.io/hsetially/lanekeeper/hub"
    plant "the whole registry skips the check" gcp-storage prod 's#^  globalPolicyEvaluationMode: ENABLE$#&\n  admissionWhitelistPatterns:\n    - namePattern: "ghcr.io/*"#' "covers"
    plant "everything skips the check" gcp-storage prod 's#^  globalPolicyEvaluationMode: ENABLE$#&\n  admissionWhitelistPatterns:\n    - namePattern: "*"#' "a bare * would skip the check"
    plant_append "a second policy" gcp-storage prod $'apiVersion: binaryauthorization.cnrm.cloud.google.com/v1beta1\nkind: BinaryAuthorizationPolicy\nmetadata:\n  name: second\n  namespace: lanekeeper-gcp\nspec:\n  projectRef: {external: "projects/example-project"}\n  defaultAdmissionRule: {evaluationMode: ALWAYS_ALLOW, enforcementMode: ENFORCED_BLOCK_AND_AUDIT_LOG}' "exactly one admission policy"

    # S20: what the attestor trusts.
    plant "a key that is not in Cloud KMS" gcp-storage prod "$in_attestor"'s#id: "//cloudkms.googleapis.com/v1/#id: "ni:///sha-256;#' "not a Cloud KMS key version"
    plant "a weak signature algorithm" gcp-storage prod "$in_attestor"'s/signatureAlgorithm: ECDSA_P256_SHA256/signatureAlgorithm: RSA_SIGN_PKCS1_2048_SHA256/' "signatureAlgorithm"
    plant "no key" gcp-storage prod "$in_attestor"'{/^    publicKeys:$/,/signatureAlgorithm: /d}' "no public key"
    plant "a private key as the public key" gcp-storage prod "$in_attestor"'s/-----BEGIN PUBLIC KEY-----/-----BEGIN PRIVATE KEY-----/' "holds a private key"
    plant "a note that is not in the output" gcp-storage prod "$in_attestor"'s/^      name: lanekeeper-ci$/      name: another-note/' "is not in this output"
    plant "a note without a name for its authority" gcp-storage prod "$in_note"'{/humanReadableName: /d}' "humanReadableName"
    plant "a policy of another project" gcp-storage prod "$in_policy"'s#external: "projects/example-project"#external: "projects/another-project"#' "spec.projectRef.external"

    # S20: removing a resource from Git must not remove it from the project.
    plant "the policy is deleted with its resource" gcp-storage prod "$in_policy"'s/cnrm.cloud.google.com\/deletion-policy: abandon/cnrm.cloud.google.com\/deletion-policy: delete/' "BinaryAuthorizationPolicy/lanekeeper: the annotation cnrm.cloud.google.com/deletion-policy: abandon"
    plant "the policy may be pruned" gcp-storage prod "$in_policy"'s/Prune=false/Prune=true/' "BinaryAuthorizationPolicy/lanekeeper: the annotation argocd.argoproj.io/sync-options: Prune=false"
    plant "the attestor is deleted with its resource" gcp-storage prod "$in_attestor"'{/cnrm.cloud.google.com\/deletion-policy: abandon/d}' "BinaryAuthorizationAttestor/lanekeeper-ci: the annotation cnrm.cloud.google.com/deletion-policy: abandon"
    plant "the note may be pruned" gcp-storage prod "$in_note"'{/argocd.argoproj.io\/sync-options: Prune=false/d}' "ContainerAnalysisNote/lanekeeper-ci: the annotation argocd.argoproj.io/sync-options: Prune=false"

    # The synthetic key stands in only while the values have none: with a key in the values, the output has that key and not the synthetic one.
    local real="$work/vr/gcp/prod.yaml" rendered
    mkdir -p "$work/vr/gcp"
    {
        cat deploy/values/gcp/prod.yaml
        printf 'binaryAuthorization:\n  publicKeys:\n    - keyVersion: projects/example-project/locations/example-location/keyRings/lanekeeper/cryptoKeys/a-real-key/cryptoKeyVersions/7\n'
        printf '      signatureAlgorithm: ECDSA_P256_SHA256\n      publicKeyPem: |\n        -----BEGIN PUBLIC KEY-----\n'
        printf '        MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEreal0real0real0real0real0real0real0real0real0\n'
        printf '        real0real0real0real0real0real0real0real0real0real0real0real0real0==\n        -----END PUBLIC KEY-----\n'
    } >"$real"
    if ! rendered="$(values_root="$work/vr" render gcp-storage prod 2>&1)"; then
        fail "the gcp chart did not render with a public key in the values: $(tail -n1 <<<"$rendered")"
    elif ! grep -q 'cryptoKeys/a-real-key/cryptoKeyVersions/7' <<<"$rendered" || grep -q 'lanekeeper-ci-cosign' <<<"$rendered"; then
        fail "with a public key in the values, the output does not have exactly that key: the synthetic key was layered over it"
    else
        echo "  ok  with a public key in the values, that key is the one rendered and checked"
    fi
}

# --- Argo CD (09/T6): the Applications and projects of deploy/argocd ---------------------------------------------------------------------

# How many times the argocd policy and the project scope were run: argocd_policy_all counts them against the files, so that a check that is
# deleted or skipped from it fails there and does not pass by doing nothing.
policy_runs=0
scope_runs=0

# conftest over the files that one Argo CD applies. The warnings (a placeholder revision, plan 09 A29) are printed and do not fail.
argocd_conftest() {
    local namespace="$1"
    shift
    [ "$namespace" != argocd ] || policy_runs=$((policy_runs + 1))
    conftest test --combine --policy deploy/policy --namespace "$namespace" --no-color "$@"
}

argocd_kubeconform() {
    echo "== kubeconform deploy/argocd"
    kubeconform "${schema_args[@]}" deploy/argocd/platform/*.yaml deploy/argocd/swimlanes/*.yaml
}

# The scalar a file gives for a key, at any indentation (quotes and a trailing comment dropped); empty when it has none.
yaml_scalar() { sed -nE "s@^ *$2: *\"?([^\"# ]+)\"?( *#.*)?\$@\1@p" "$1" | head -n1; }

# argocd_paths ARGO_DIR VALUES_DIR: every file that an Application names exists, and swimlanes and their Applications go in pairs. ARGO_DIR has
# platform/ and swimlanes/; VALUES_DIR has hub/, db/, gcp/ and agent/ as deploy/values does. The policy has already shown that the Application
# names exactly the files its labels say, so reading the labels here is reading the same thing.
argocd_paths() {
    local argo="$1" values="$2" status=0 f chart env id
    for f in "$argo"/platform/hub.yaml "$argo"/platform/db.yaml "$argo"/platform/gcp.yaml; do
        [ -f "$f" ] || { echo "$f does not exist"; status=1; continue; }
        chart="$(yaml_scalar "$f" path)"
        chart="${chart##*/}"
        env="$(yaml_scalar "$f" lanekeeper.io/environment)"
        if [ -z "$chart" ] || [ -z "$env" ]; then
            echo "$f: cannot read the chart and the environment from its path and labels"
            status=1
            continue
        fi
        [ -f "$values/$chart/$env.yaml" ] || { echo "$f deploys $values/$chart/$env.yaml, which does not exist"; status=1; }
        if [ "$chart" = gcp ]; then
            [ -f "$values/hub/github-hook-ranges.yaml" ] || { echo "$f deploys $values/hub/github-hook-ranges.yaml, which does not exist"; status=1; }
        fi
    done
    for f in "$argo"/swimlanes/*.yaml; do
        [ -f "$f" ] || continue
        id="$(basename "$f" .yaml)"
        [ -f "$values/agent/$id.yaml" ] || { echo "$f has no values file: $values/agent/$id.yaml does not exist"; status=1; }
    done
    for f in "$values"/agent/*.yaml; do
        [ -f "$f" ] || continue
        id="$(basename "$f" .yaml)"
        [ -f "$argo/swimlanes/$id.yaml" ] || { echo "$f is the values of the swimlane $id, which has values but no Application: $argo/swimlanes/$id.yaml does not exist"; status=1; }
    done
    return "$status"
}

# What each project allows against what its charts render. The platform's Applications deploy hub and db (the workloads project) and gcp (the
# cloud project); a swimlane's deploys agent. The gcp chart is rendered with Cloud Armor and without (B5: only the render without it
# exists until GitHub's ranges do).
argocd_scope_one() {
    local chart="$1" env="$2" projects="$3" status=0
    scope_runs=$((scope_runs + 1))
    echo "== argocd scope: $chart ($env) against $projects"
    render "$chart" "$env" >"$work/scope-$chart-$env.yaml" || return 1
    argocd_conftest argocd_scope "/work/scope-$chart-$env.yaml" "$projects" || status=1
    return "$status"
}

argocd_policy_all() {
    local status=0 f chart env id lanes=0
    policy_runs=0
    scope_runs=0
    echo "== argocd policy: the platform cluster's files"
    argocd_conftest argocd deploy/argocd/platform/*.yaml || status=1
    for f in deploy/argocd/swimlanes/*.yaml; do
        [ -f "$f" ] || continue
        id="$(basename "$f" .yaml)"
        echo "== argocd policy: the swimlane $id"
        argocd_conftest argocd "$f" "deploy/values/agent/$id.yaml" || status=1
    done
    echo "== argocd paths: every file that an Application names exists"
    argocd_paths deploy/argocd deploy/values || status=1
    for f in deploy/argocd/platform/hub.yaml deploy/argocd/platform/db.yaml deploy/argocd/platform/gcp.yaml; do
        chart="$(yaml_scalar "$f" path)"
        chart="${chart##*/}"
        env="$(yaml_scalar "$f" lanekeeper.io/environment)"
        if [ -z "$chart" ] || [ -z "$env" ]; then
            echo "$f: cannot read the chart and the environment" >&2
            status=1
            continue
        fi
        # The gcp chart with Cloud Armor waits for GitHub's ranges (B5); `argocd` mode (a subset, never the gate) leaves it out, and says so.
        if [ "$chart" = gcp ] && [ "${scope_full_gcp:-1}" = 0 ]; then
            echo "== argocd scope: gcp ($env) with Cloud Armor is left out of this mode (B5); \`all\` and \`policy\` check it"
        else
            argocd_scope_one "$chart" "$env" deploy/argocd/platform/projects.yaml || { echo "render.sh: argocd scope FAILED for the $chart chart with the $env values" >&2; status=1; }
        fi
        if [ "$chart" = gcp ]; then
            argocd_scope_one gcp-storage "$env" deploy/argocd/platform/projects.yaml || { echo "render.sh: argocd scope FAILED for the gcp chart without Cloud Armor with the $env values" >&2; status=1; }
        fi
    done
    for f in deploy/argocd/swimlanes/*.yaml; do
        [ -f "$f" ] || continue
        id="$(basename "$f" .yaml)"
        argocd_scope_one agent "$id" "$f" || { echo "render.sh: argocd scope FAILED for the agent chart of $id" >&2; status=1; }
    done
    # The tripwire. The policy runs once for the platform's files and once for each swimlane; the scope runs for hub, db, gcp (with Cloud Armor,
    # unless this mode leaves it out), gcp-storage, and each swimlane's agent. Counted from the directory, not from the loops above.
    for f in deploy/argocd/swimlanes/*.yaml; do
        [ -f "$f" ] && lanes=$((lanes + 1))
    done
    local want_policy=$((1 + lanes)) want_scope=$((4 + lanes))
    [ "${scope_full_gcp:-1}" != 0 ] || want_scope=$((want_scope - 1))
    if [ "$policy_runs" -ne "$want_policy" ] || [ "$scope_runs" -ne "$want_scope" ]; then
        echo "render.sh: the Argo CD checks ran the policy $policy_runs times and the scope $scope_runs times, and the files call for $want_policy and $want_scope" >&2
        status=1
    fi
    return "$status"
}

# --- planted violations of the Argo CD files ---

# A fresh copy of what the policy reads: deploy/argocd and the agent values, under $work/ap (the paths end the same way).
reset_argocd_copy() {
    rm -rf "$work/ap"
    mkdir -p "$work/ap/platform" "$work/ap/swimlanes" "$work/ap/values/agent"
    cp deploy/argocd/platform/*.yaml "$work/ap/platform/"
    cp deploy/argocd/swimlanes/*.yaml "$work/ap/swimlanes/"
    cp deploy/values/agent/*.yaml "$work/ap/values/agent/"
}

# The check of the group a file belongs to, over the copy: the platform's files together, or one swimlane's file with its values.
check_argocd_copy() {
    local file="$1" id
    local files=() f
    case "$file" in
        platform/*)
            for f in "$work"/ap/platform/*.yaml; do files+=("/work/ap/platform/$(basename "$f")"); done
            argocd_conftest argocd "${files[@]}"
            ;;
        swimlanes/*)
            id="$(basename "$file" .yaml)"
            argocd_conftest argocd "/work/ap/$file" "/work/ap/values/agent/$id.yaml"
            ;;
        values/agent/*)
            id="$(basename "$file" .yaml)"
            argocd_conftest argocd "/work/ap/swimlanes/$id.yaml" "/work/ap/$file"
            ;;
    esac
}

# plant_argocd NAME FILE SED FRAGMENT: apply SED to FILE (under deploy/argocd, or values/agent/..), which must change it; the check must refuse the copy.
plant_argocd() {
    local name="$1" file="$2" expr="$3" fragment="$4" out
    reset_argocd_copy
    cp "$work/ap/$file" "$work/ap-before"
    sed -E -i "$expr" "$work/ap/$file"
    if cmp -s "$work/ap-before" "$work/ap/$file"; then
        fail "$name: the sed expression changed nothing, so the case tests nothing"
        return
    fi
    if out="$(check_argocd_copy "$file" 2>&1)"; then
        fail "$name: the policy accepted it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

# expect_paths_failure NAME FRAGMENT: argocd_paths over the copy (and a copy of the values) must refuse, for FRAGMENT.
expect_paths_failure() {
    local name="$1" fragment="$2" out
    if out="$(argocd_paths "$work/ap" "$work/apv" 2>&1)"; then
        fail "$name: the paths check accepted it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

reset_values_copy() {
    rm -rf "$work/apv"
    mkdir -p "$work/apv"
    cp -r deploy/values/. "$work/apv/"
}

planted_argocd() {
    echo "== planted: the Argo CD policy refuses the real files with one thing wrong"
    reset_argocd_copy
    echo "-- the unchanged files pass"
    check_argocd_copy platform/hub.yaml >/dev/null || fail "the unchanged platform files were refused"
    check_argocd_copy swimlanes/sit1.yaml >/dev/null || fail "the unchanged swimlane files were refused"
    local out
    out="$(check_argocd_copy platform/hub.yaml 2>&1)" || true
    grep -qF "placeholder" <<<"$out" || fail "the placeholder revision of the committed files is not warned about"

    local hub=platform/hub.yaml db=platform/db.yaml gcp=platform/gcp.yaml projects=platform/projects.yaml lane=swimlanes/sit1.yaml
    # S19: what is deployed is a commit of this repository.
    plant_argocd "a branch as the revision" $hub 's/^( *)targetRevision: .*/\1targetRevision: main/' "targetRevision"
    plant_argocd "a tag as the revision" $hub 's/^( *)targetRevision: .*/\1targetRevision: "v1.0.0"/' "targetRevision"
    plant_argocd "a short commit as the revision" $hub 's/^( *)targetRevision: .*/\1targetRevision: "abcdef1"/' "targetRevision"
    plant_argocd "a branch as the revision of a swimlane" $lane 's/^( *)targetRevision: .*/\1targetRevision: main/' "targetRevision"
    plant_argocd "another repository" $hub 's#^( *)repoURL: .*#\1repoURL: https://github.com/someone/lanekeeper#' "repoURL"
    plant_argocd "a Helm repository as the source" $hub 's/^( *)path: deploy\/charts\/hub$/&\n\1chart: hub/' "Helm repository"
    plant_argocd "inline values" $hub 's/^( *)releaseName: lanekeeper-hub$/&\n\1values: "replicas: 1"/' "spec.source.helm.values"
    plant_argocd "inline parameters" $hub 's/^( *)releaseName: lanekeeper-hub$/&\n\1parameters:\n\1  - {name: image.digest, value: "sha256:x"}/' "spec.source.helm.parameters"
    plant_argocd "a missing values file ignored" $hub 's/^( *)releaseName: lanekeeper-hub$/&\n\1ignoreMissingValueFiles: true/' "spec.source.helm.ignoreMissingValueFiles"
    plant_argocd "another release name" $hub 's/releaseName: lanekeeper-hub/releaseName: lanekeeper/' "releaseName"
    plant_argocd "the staging values for prod" $hub 's#values/hub/prod.yaml#values/hub/staging.yaml#' "valueFiles"
    plant_argocd "the cloud resources without the hook ranges" $gcp '/github-hook-ranges.yaml/d' "valueFiles"
    plant_argocd "the cloud resources with the ranges last" $gcp 's#^( *)- \.\./\.\./values/hub/github-hook-ranges.yaml$#\1- ../../values/gcp/prod.yaml#;t;s#^( *)- \.\./\.\./values/gcp/prod.yaml$#\1- ../../values/hub/github-hook-ranges.yaml#' "valueFiles"
    plant_argocd "the agent with another swimlane's values" $lane 's#values/agent/sit1.yaml#values/agent/sit2.yaml#' "valueFiles"
    plant_argocd "a label that names no environment" $hub 's#lanekeeper.io/environment: prod#lanekeeper.io/environment: dev#' "lanekeeper.io/environment"
    plant_argocd "no environment label" $hub '/lanekeeper.io\/environment/d' "lanekeeper.io/environment"
    plant_argocd "a file that is not named after its swimlane" $lane 's#lanekeeper.io/swimlane: sit1#lanekeeper.io/swimlane: sit2#' "named after its swimlane"

    # S16: the project and the namespace are the chart's own.
    plant_argocd "the default project" $hub 's/project: lanekeeper-workloads/project: default/' "spec.project"
    plant_argocd "the cloud resources in the workloads project" $gcp 's/project: lanekeeper-cloud/project: lanekeeper-workloads/' "spec.project"
    plant_argocd "another chart" $hub 's#path: deploy/charts/hub#path: deploy/charts/db#' "spec.source.path"
    plant_argocd "the hub in the system's namespace" $hub 's/^( *)namespace: lanekeeper$/\1namespace: kube-system/' "spec.destination.namespace"
    plant_argocd "another cluster" $hub 's#server: https://kubernetes.default.svc#server: https://203.0.113.5#' "spec.destination.server"
    plant_argocd "an Application outside the Argo CD namespace" $hub 's/^  namespace: argocd$/  namespace: default/' "metadata.namespace"

    # D26: what is in Git is what runs.
    plant_argocd "manual sync" $hub '/^    automated:$/,/^      selfHeal: true$/d' "spec.syncPolicy.automated"
    plant_argocd "no self-heal" $hub 's/selfHeal: true/selfHeal: false/' "selfHeal"
    plant_argocd "an empty render synced" $hub 's/^( *)selfHeal: true$/&\n\1allowEmpty: true/' "allowEmpty"
    plant_argocd "the hub without prune" $hub 's/prune: true/prune: false/' "prune"
    plant_argocd "the database with prune" $db 's/prune: false/prune: true/' "prune"
    plant_argocd "the cloud resources with prune" $gcp 's/prune: false/prune: true/' "prune"
    plant_argocd "the agent without prune" $lane 's/prune: true/prune: false/' "prune"
    plant_argocd "replace" $hub 's/^( *)- CreateNamespace=true$/&\n\1- Replace=true/' "syncOptions"
    plant_argocd "the Application deletes the workloads with it" $hub 's/^  namespace: argocd$/&\n  finalizers:\n    - resources-finalizer.argocd.argoproj.io/' "resources-finalizer"
    plant_argocd "the Application deletes the database with it" $db 's/^  namespace: argocd$/&\n  finalizers:\n    - resources-finalizer.argocd.argoproj.io/' "resources-finalizer"

    # A14: the agent's certificate Secret.
    plant_argocd "the certificate Secret not ignored" $lane '/^  ignoreDifferences:$/,/^        - \/data$/d' "ignoreDifferences"
    plant_argocd "more than the data ignored" $lane 's#^( *)- /data$#&\n\1- /metadata#' "ignoreDifferences"
    plant_argocd "the whole Secret ignored" $lane 's#^( *)- /data$#\1- /#' "ignoreDifferences"
    plant_argocd "ignored but written" $lane '/- RespectIgnoreDifferences=true/d' "RespectIgnoreDifferences"
    plant_argocd "differences ignored on the hub" $hub 's/^  destination:$/  ignoreDifferences:\n    - group: apps\n      kind: StatefulSet\n      jsonPointers:\n        - \/spec\n&/' "only the agent's certificate Secret is ignored"

    # The projects.
    plant_argocd "a project that reads any repository" $projects 's#^( *)- https://github.com/hsetially/lanekeeper$#\1- "*"#' "sourceRepos"
    plant_argocd "a project that deploys to any namespace" $projects 's/^( *)namespace: lanekeeper-gcp$/\1namespace: "*"/' "namespace pattern"
    plant_argocd "a project that deploys to the system's namespace" $projects 's/^( *)namespace: lanekeeper$/\1namespace: kube-system/' "must not deploy to"
    plant_argocd "a project that deploys to Argo CD's namespace" $projects 's/^( *)namespace: lanekeeper-gcp$/\1namespace: argocd/' "must not deploy to"
    plant_argocd "a cluster-scoped kind in the workloads project" $projects 's/^      kind: StorageClass$/&\n    - group: rbac.authorization.k8s.io\n      kind: ClusterRole/' "clusterResourceWhitelist"
    plant_argocd "Secrets in the workloads project" $projects 's/^( *)kind: ServiceAccount$/&\n    - group: ""\n      kind: Secret/' "namespaceResourceWhitelist"
    plant_argocd "every kind in the cloud project" $projects 's/^( *)kind: StorageBucket$/&\n    - group: "*"\n      kind: "*"/' "namespaceResourceWhitelist"
    plant_argocd "project tokens" $projects 's/^  orphanedResources:$/  roles:\n    - name: ci\n      policies: ["p, proj:x:ci, applications, sync, x\/*, allow"]\n&/' "spec.roles"
    plant_argocd "the swimlane's project with a cluster role" $lane 's/^      name: lanekeeper$/&\n    - group: rbac.authorization.k8s.io\n      kind: ClusterRole/' "clusterResourceWhitelist"
    plant_argocd "a namespace made by a project that does not allow it" $lane '/^  clusterResourceWhitelist:$/,/^      name: lanekeeper$/d' "CreateNamespace=true"
    plant_argocd "every namespace allowed to a project" $projects 's/^      name: lanekeeper-gcp$/      name: "*"/' "clusterResourceWhitelist"
    plant_argocd "the swimlane's project in another namespace" $lane 's/^      namespace: sit1$/      namespace: sit2/' "namespaces of deploy/values/agent/sit1.yaml"
    plant_argocd "a watched namespace the project does not know" values/agent/sit1.yaml 's/^  - sit1$/&\n  - sit2/' "namespaces of deploy/values/agent/sit1.yaml"

    # The files the Applications name.
    echo "== planted: the paths check refuses an Application and its files that do not go together"
    reset_argocd_copy
    reset_values_copy
    argocd_paths "$work/ap" "$work/apv" >/dev/null || fail "the unchanged files were refused by the paths check"
    rm "$work/apv/hub/prod.yaml"
    expect_paths_failure "the hub's values file is gone" "hub/prod.yaml, which does not exist"
    reset_values_copy
    rm "$work/apv/hub/github-hook-ranges.yaml"
    expect_paths_failure "the hook ranges file is gone" "github-hook-ranges.yaml, which does not exist"
    reset_values_copy
    rm "$work/apv/agent/sit1.yaml"
    expect_paths_failure "a swimlane Application without values" "has no values file"
    reset_values_copy
    rm "$work/ap/swimlanes/sit1.yaml"
    expect_paths_failure "swimlane values without an Application" "has values but no Application"
    reset_argocd_copy
    sed -E -i 's#^( *)path: deploy/charts/hub$#\1path: deploy/charts/#' "$work/ap/platform/hub.yaml"
    expect_paths_failure "an Application whose chart cannot be read" "cannot read the chart"
    reset_argocd_copy
    rm "$work/ap/platform/db.yaml"
    expect_paths_failure "an Application file that is gone" "db.yaml does not exist"

    # What a project allows against what the charts render: a planted kind in the output.
    echo "== planted: a chart that makes a kind its project does not allow is refused"
    local rendered="$work/scope-planted.yaml"
    render hub prod >"$rendered"
    printf -- '---\napiVersion: v1\nkind: Secret\nmetadata:\n  name: planted\n  namespace: lanekeeper\n' >>"$rendered"
    if out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" deploy/argocd/platform/projects.yaml 2>&1)"; then
        fail "a Secret in the hub's output was accepted by the scope check"
    elif ! grep -qF "no project allows the kind Secret" <<<"$out"; then
        fail "a Secret in the hub's output was refused, but not for its kind:"$'\n'"$out"
    else
        echo "  ok  a Secret in the hub's output is not allowed by its project"
    fi
    render agent sit1 >"$rendered"
    printf -- '---\napiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: planted\n' >>"$rendered"
    if out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" deploy/argocd/swimlanes/sit1.yaml 2>&1)"; then
        fail "a ClusterRole in the agent's output was accepted by the scope check"
    elif ! grep -qF "cluster-scoped, and no project allows the kind ClusterRole" <<<"$out"; then
        fail "a ClusterRole in the agent's output was refused, but not for its kind:"$'\n'"$out"
    else
        echo "  ok  a ClusterRole in the agent's output is not allowed by its project"
    fi
}

# --- the charts agree with one another (09/T5) ---------------------------------------------------------------------------------------
# The db chart's backups go to the bucket that the gcp chart makes, as the Google service account that it makes and lets write there;
# the Kubernetes service accounts that Workload Identity maps are the ones the hub and the database run as; the keys of the disks are
# keys that Compute Engine may use. Each is a name in two charts, so a typo in one would be a database that cannot back up or a pod
# that cannot start, found on the first apply. The gcp side is the chart without Cloud Armor (it does not depend on GitHub's ranges).

# yaml_docs_with FILE REGEX FIELD: for every document of FILE that has a line matching REGEX, the value of the first line `FIELD: value`
# (at any indentation), quotes stripped.
doc_field() {
    awk -v want="$2" -v field="$3" '
        function flush() { if (found && value != "") print value; found = 0; value = "" }
        /^---$/ { flush(); next }
        $0 ~ want { found = 1 }
        {
            line = $0; sub(/^ +/, "", line)
            if (value == "" && index(line, field ": ") == 1) { value = substr(line, length(field) + 3); gsub(/^"|"$/, "", value) }
        }
        END { flush() }
    ' "$1"
}

# cross_check DB GCP HUB: print every disagreement; return 1 if there is one.
cross_check() {
    local db="$1" gcp="$2" hub="$3" status=0 bad
    local backup_bucket path_bucket
    backup_bucket="$(doc_field "$gcp" '^    lanekeeper.io/purpose: backup$' name)"
    path_bucket="$(grep -E '^ +destinationPath:' "$db" | sed -E 's#.*"gs://([^/"]+).*#\1#')"
    if [ -z "$backup_bucket" ] || [ "$backup_bucket" != "$path_bucket" ]; then
        echo "the database backs up to bucket '$path_bucket' and the gcp chart makes the backup bucket '$backup_bucket'"
        status=1
    fi

    local db_gsa db_member
    db_gsa="$(grep -E 'iam.gke.io/gcp-service-account:' "$db" | sed -E 's/.*: "?([^" ]+)"?$/\1/')"
    db_member="$(doc_field "$gcp" '^  name: lanekeeper-db-backup$' member)"
    if [ -z "$db_gsa" ] || [ "serviceAccount:$db_gsa" != "$db_member" ]; then
        echo "the database runs as Google service account '$db_gsa' and the gcp chart lets '$db_member' write the backups"
        status=1
    fi

    local hub_gsa hub_member
    hub_gsa="$(grep -E 'iam.gke.io/gcp-service-account:' "$hub" | sed -E 's/.*: "?([^" ]+)"?$/\1/')"
    hub_member="$(doc_field "$gcp" '^  name: lanekeeper-hub-docs$' member)"
    if [ -z "$hub_gsa" ] || [ "serviceAccount:$hub_gsa" != "$hub_member" ]; then
        echo "the hub runs as Google service account '$hub_gsa' and the gcp chart gives '$hub_member' the docs and the checkpoints"
        status=1
    fi

    # Workload Identity: PROJECT.svc.id.goog[NAMESPACE/NAME] of the gcp chart is where the hub's and the database's service accounts are.
    local hub_ns hub_ksa db_ns
    hub_ns="$(awk '/^kind: ServiceAccount$/ {f = 1} f && /^  namespace: / {print $2; exit}' "$hub")"
    hub_ksa="$(awk '/^kind: ServiceAccount$/ {f = 1} f && /^  name: / {print $2; exit}' "$hub")"
    db_ns="$(awk '/^kind: Cluster$/ {f = 1} f && /^  namespace: / {print $2; exit}' "$db")"
    if ! grep -qE "member: \"serviceAccount:[a-z0-9-]+\.svc\.id\.goog\[${hub_ns}/${hub_ksa}\]\"" "$gcp"; then
        echo "the gcp chart does not map the hub's Kubernetes service account ${hub_ns}/${hub_ksa} by Workload Identity"
        status=1
    fi
    # CloudNativePG makes the cluster's service account with the cluster's name.
    if ! grep -qE "member: \"serviceAccount:[a-z0-9-]+\.svc\.id\.goog\[${db_ns}/lanekeeper-db\]\"" "$gcp"; then
        echo "the gcp chart does not map the database's Kubernetes service account ${db_ns}/lanekeeper-db (named after the cluster) by Workload Identity"
        status=1
    fi

    # The keys of the disks.
    local compute_keys key
    compute_keys="$(doc_field "$gcp" 'compute-system.iam.gserviceaccount.com' external)"
    while IFS= read -r key; do
        [ -n "$key" ] || continue
        if ! grep -qxF "$key" <<<"$compute_keys"; then
            echo "the key '$key' encrypts a disk, and the gcp chart (cmek.diskKeys) does not let Compute Engine use it"
            status=1
        fi
    done < <(grep -hE '^ +(disk-encryption-kms-key):' "$db" "$hub" | sed -E 's/.*: "?([^" ]+)"?$/\1/' | sort -u)
    return "$status"
}

cross_chart() {
    local env status=0
    for env in staging prod; do
        [ -f "deploy/values/db/$env.yaml" ] && [ -f "deploy/values/gcp/$env.yaml" ] && [ -f "deploy/values/hub/$env.yaml" ] || continue
        echo "== the db, gcp and hub charts agree ($env)"
        render db "$env" >"$work/x-db-$env.yaml" || return 1
        render gcp-storage "$env" >"$work/x-gcp-$env.yaml" || return 1
        render hub "$env" >"$work/x-hub-$env.yaml" || return 1
        cross_check "$work/x-db-$env.yaml" "$work/x-gcp-$env.yaml" "$work/x-hub-$env.yaml" || status=1
        # The Cloud Armor policy the hub's backend policy names is the one the gcp chart makes: values on both sides.
        local hub_policy gcp_policy
        hub_policy="$(sed -nE 's/^ *securityPolicyName: *"?([^" ]+)"?$/\1/p' "deploy/values/hub/$env.yaml")"
        gcp_policy="$(sed -nE 's/^ *policyName: *"?([^" ]+)"?$/\1/p' "deploy/values/gcp/$env.yaml")"
        if [ "$hub_policy" != "$gcp_policy" ]; then
            echo "the hub attaches Cloud Armor policy '$hub_policy' and the gcp chart makes '$gcp_policy'"
            status=1
        fi
    done
    return "$status"
}

# expect_cross_failure NAME DB GCP HUB FRAGMENT: cross_check must refuse these three files, for FRAGMENT.
expect_cross_failure() {
    local name="$1" fragment="$5" out
    if out="$(cross_check "$2" "$3" "$4" 2>&1)"; then
        fail "$name: the cross-chart check accepted it"
    elif ! grep -qF -- "$fragment" <<<"$out"; then
        fail "$name: refused, but not for '$fragment':"$'\n'"$out"
    else
        echo "  ok  $name"
    fi
}

planted_cross() {
    echo "== planted: the cross-chart check refuses what the charts do not agree on"
    local db="$work/x-db-prod.yaml" gcp="$work/x-gcp-prod.yaml" hub="$work/x-hub-prod.yaml" bad="$work/x-bad.yaml"
    render db prod >"$db"
    render gcp-storage prod >"$gcp"
    render hub prod >"$hub"
    cross_check "$db" "$gcp" "$hub" >/dev/null || fail "the unchanged prod renders of the db, gcp and hub charts disagree"

    sed -E 's#gs://example-project-lanekeeper-backup-prod/#gs://example-project-lanekeeper-backup-prd/#' "$db" >"$bad"
    expect_cross_failure "backups to another bucket" "$bad" "$gcp" "$hub" "the database backs up to bucket"
    sed -E 's#(iam.gke.io/gcp-service-account: ")lanekeeper-db@#\1lanekeeper-databse@#' "$db" >"$bad"
    expect_cross_failure "backups as another Google service account" "$bad" "$gcp" "$hub" "the database runs as Google service account"
    sed -E 's#(iam.gke.io/gcp-service-account: ")lanekeeper-hub@#\1lanekeeper-hb@#' "$hub" >"$bad"
    expect_cross_failure "the hub as another Google service account" "$db" "$gcp" "$bad" "the hub runs as Google service account"
    sed -E 's#\[lanekeeper/lanekeeper-hub\]#[lanekeeper/lanekeeper-hb]#' "$gcp" >"$bad"
    expect_cross_failure "Workload Identity for another hub service account" "$db" "$bad" "$hub" "does not map the hub's Kubernetes service account"
    sed -E 's#\[lanekeeper/lanekeeper-db\]#[other/lanekeeper-db]#' "$gcp" >"$bad"
    expect_cross_failure "Workload Identity for the database in another namespace" "$db" "$bad" "$hub" "does not map the database's Kubernetes service account"
    sed -E 's#(disk-encryption-kms-key: ".*cryptoKeys/)lanekeeper-disk-prod#\1other-disk#' "$db" >"$bad"
    expect_cross_failure "a disk key that Compute Engine may not use" "$bad" "$gcp" "$hub" "encrypts a disk"
    sed -E 's#(disk-encryption-kms-key: ".*cryptoKeys/)lanekeeper-disk-prod#\1other-disk#' "$hub" >"$bad"
    expect_cross_failure "the hub's disk key that Compute Engine may not use" "$db" "$gcp" "$bad" "encrypts a disk"
}

planted_kubeconform() {
    echo "== planted: kubeconform refuses a field the schema does not have, and a kind it has no schema for"
    local out status=0
    out="$(kubeconform "${schema_args[@]}" deploy/tests/planted/kubeconform.yaml 2>&1)" || status=$?
    [ "$status" -ne 0 ] || fail "kubeconform accepted deploy/tests/planted/kubeconform.yaml"
    local kind
    for kind in GCPBackendPolicy HealthCheckPolicy FQDNNetworkPolicy ComputeSecurityPolicy Gateway StatefulSet Deployment Role RoleBinding NetworkPolicy ConfigMap Cluster ScheduledBackup StorageBucket IAMPolicyMember IAMServiceAccount BinaryAuthorizationPolicy BinaryAuthorizationAttestor ContainerAnalysisNote Application AppProject; do
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
    planted_db_templates
    planted_db
    planted_gcp_templates
    planted_gcp_storage
    planted_gcp_admission_templates
    planted_gcp_admission
    planted_cross
    planted_argocd
    if [ "$failures" -ne 0 ]; then
        echo "render.sh: $failures planted case(s) failed" >&2
        return 1
    fi
    echo "render.sh: every planted case was refused for its own reason"
}

# The data layer alone (09/T5), and nothing else: the db chart, and the gcp chart without Cloud Armor. It reports every failure and
# exits non-zero on any; it does not look at the gcp chart with Cloud Armor, which `all` does and which waits for B5.
data_all() {
    local status=0
    combo_filter='^(db|gcp-storage):'
    lint || status=1
    kubeconform_all || status=1
    policy_all || status=1
    cross_chart || status=1
    planted_db_templates
    planted_db
    planted_gcp_templates
    planted_gcp_storage
    planted_gcp_admission_templates
    planted_gcp_admission
    planted_cross
    if [ "$failures" -ne 0 ]; then
        echo "render.sh: $failures planted case(s) failed" >&2
        status=1
    fi
    [ "$status" -eq 0 ] && echo "render.sh: the data layer passes every check, and every planted case was refused for its own reason"
    return "$status"
}

# The Argo CD files alone (09/T6), and nothing else. It reports every failure and exits non-zero on any.
argocd_only() {
    local status=0
    scope_full_gcp=0
    argocd_kubeconform || status=1
    argocd_policy_all || status=1
    planted_argocd
    if [ "$failures" -ne 0 ]; then
        echo "render.sh: $failures planted case(s) failed" >&2
        status=1
    fi
    [ "$status" -eq 0 ] && echo "render.sh: the Argo CD files pass every check, and every planted case was refused for its own reason"
    return "$status"
}

case "$mode" in
    lint) lint ;;
    data) data_all ;;
    render)
        status=0
        kubeconform_all || status=1
        argocd_kubeconform || status=1
        exit "$status"
        ;;
    policy)
        status=0
        policy_all || status=1
        argocd_policy_all || status=1
        exit "$status"
        ;;
    contract) contract_all && cross_chart ;;
    planted) planted_all ;;
    argocd) argocd_only ;;
    all)
        status=0
        lint || status=1
        kubeconform_all || status=1
        argocd_kubeconform || status=1
        policy_all || status=1
        argocd_policy_all || status=1
        contract_all || status=1
        cross_chart || status=1
        planted_all || status=1
        exit "$status"
        ;;
esac
