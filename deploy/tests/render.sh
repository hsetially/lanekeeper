#!/usr/bin/env bash
# Render every chart with every values file, and check what comes out (09/T3 to T6; S6, S7, S9, S10, S16, S18, S19, S20, P11).
#
#   deploy/tests/render.sh lint      helm lint --strict
#   deploy/tests/render.sh render    helm template | kubeconform -strict, offline, with the vendored schemas
#   deploy/tests/render.sh policy    helm template | conftest (deploy/policy), with perf/budgets.toml and the values files
#   deploy/tests/render.sh contract  the agent chart's Roles, NetworkPolicy and pod against crates/agent/RBAC.md and HARDENING.md,
#                                    the hub chart's environment against the variables the hub requires (crates/hub/tests/hardening.rs),
#                                    and the db, gcp, hub and agent charts against one another (bucket, service account, disk keys,
#                                    the hub's names for the agents)
#   deploy/tests/render.sh planted   the checks above must FAIL on planted violations, each for its own reason
#   deploy/tests/render.sh data      the data layer (09/T5) alone: the db chart and the gcp chart without Cloud Armor, through lint,
#                                    kubeconform and the policies, their planted violations, and the cross-chart check. A SUBSET of
#                                    `all`, for the part that does not wait for GitHub's hook ranges (B5); it is never the gate.
#   deploy/tests/render.sh argocd    the Argo CD files (09/T6) alone: kubeconform, the policies, the paths, the scope of each project against the
#                                    rendered charts, and their planted violations. A SUBSET of `all`: the scope of the gcp chart with Cloud
#                                    Armor waits for GitHub's ranges (B5) and is left out of it; it is never the gate.
#   deploy/tests/render.sh hub       the hub chart (09/fix) alone: lint, kubeconform, the policies, the hub's variables against the
#                                    contract, the chart agreeing with the db, gcp and agent charts, and the planted cases of the hub's
#                                    templates, schema and policies. A SUBSET of `all`, for working on the chart; it is never the gate.
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
case "$mode" in lint | render | policy | contract | planted | data | argocd | hub | all) ;; *)
    echo "usage: deploy/tests/render.sh lint|render|policy|contract|planted|data|argocd|hub|all" >&2
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
policy_namespaces=(hardening secrets topology resources storage network rbac database iam kms)
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

# The agent chart has contract files (RBAC.md, HARDENING.md) that conftest compares with the render. The hub's contract is crates/hub/HARDENING.md
# and the tests of crates/hub/tests/hardening.rs; its policies are in deploy/policy, and the variables the hub requires to start (exit 2
# without one) are read from REQUIRED_VARIABLES of that test, so the list cannot drift from the one the hub is held to (09/fix, E1).
# The other charts' contracts are in their policies.
contract_one() {
    local chart="$1" env="$2"
    case "$chart" in
        agent)
            echo "== contract $chart ($env)"
            render "$chart" "$env" >"$work/$chart-$env.yaml" || return 1
            check_contract "$work/$chart-$env.yaml" || return 1
            ;;
        hub)
            echo "== contract $chart ($env)"
            render "$chart" "$env" >"$work/$chart-$env.yaml" || return 1
            check_hub_variables "$work/$chart-$env.yaml" || return 1
            ;;
        *) return 0 ;;
    esac
}

# required_hub_variables: the variables the hub requires to start, one per line: REQUIRED_VARIABLES of crates/hub/tests/hardening.rs. The
# count the test declares (`[&str; 25]`) must equal the number of names read, so that a list this reader no longer understands is an
# error and never an empty list that every chart passes.
required_hub_variables() {
    local names declared count
    names="$(sed -n '/^const REQUIRED_VARIABLES/,/^];/p' crates/hub/tests/hardening.rs | grep -oE '"LK_[A-Z0-9_]+"' | tr -d '"' || true)"
    declared="$(sed -nE 's/^const REQUIRED_VARIABLES: \[&str; ([0-9]+)\].*/\1/p' crates/hub/tests/hardening.rs)"
    count="$(grep -c . <<<"$names" || true)"
    if [ -z "$declared" ] || [ "$count" != "$declared" ]; then
        echo "render.sh: read $count names from REQUIRED_VARIABLES of crates/hub/tests/hardening.rs and it declares '${declared:-nothing}'" >&2
        return 1
    fi
    printf '%s\n' "$names"
}

# sts_env_names FILE: the names of the variables that the hub StatefulSet's container sets, in order (not the Jobs').
sts_env_names() {
    awk '/^---$/ { sts = 0 } /^kind: StatefulSet$/ { sts = 1 } sts && /^ +- name: LK_[A-Z0-9_]+$/ { sub(/^ +- name: /, ""); print }' "$1"
}

# check_hub_variables FILE: the StatefulSet in FILE sets every variable the hub requires. Prints each one that is missing.
check_hub_variables() {
    local file="$1" required have name status=0
    required="$(required_hub_variables)" || return 1
    have="$(sts_env_names "$file")"
    while IFS= read -r name; do
        grep -qxF "$name" <<<"$have" || {
            echo "the StatefulSet of the hub chart does not set $name, which the hub requires (REQUIRED_VARIABLES of crates/hub/tests/hardening.rs): it exits 2 without it"
            status=1
        }
    done <<<"$required"
    return "$status"
}

# drop_env FILE NAME: FILE without the environment entries called NAME (the `- name:` line and the lines indented under it).
drop_env() {
    awk -v name="$2" '
        skip { match($0, /^ */); if (RLENGTH > indent) next; skip = 0 }
        match($0, /^ +- name: /) && substr($0, RLENGTH + 1) == name { indent = RLENGTH - 8; skip = 1; next }
        { print }
    ' "$1"
}

# volume_secret FILE VOLUME: the Secret that the pod volume VOLUME reads, in the first document that has one (empty when none).
volume_secret() {
    awk -v vol="$2" '/^ +- name: / { f = ($0 ~ ("^ +- name: " vol "$")) } f && /^ +secretName: / { print $2; exit }' "$1"
}

# seed_job_name FILE: the name of the seed Job in FILE (empty when there is none).
seed_job_name() {
    awk '/^---$/ { job = 0 } /^kind: Job$/ { job = 1 } job && /^  name: lanekeeper-hub-seed-/ { sub(/^  name: /, ""); print; exit }' "$1"
}

# hub_env FILE NAME: the value of the variable NAME in the hub StatefulSet of FILE, quotes stripped (empty when it has none).
hub_env() {
    awk -v name="$2" '
        /^---$/ { sts = 0 }
        /^kind: StatefulSet$/ { sts = 1 }
        sts && $0 ~ ("^ +- name: " name "$") { getline; if ($0 ~ /^ +value: /) { sub(/^ +value: /, ""); gsub(/^"|"$/, ""); print; exit } }
    ' "$1"
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

# plant_cmd NAME CHART ENV FRAGMENT COMMAND [ARG...]: like plant, for a change that sed cannot make: COMMAND reads the rendered output on
# standard input and writes the changed output, which must differ.
plant_cmd() {
    local name="$1" chart="$2" env="$3" fragment="$4"
    shift 4
    local good="$work/good-$chart-$env.yaml" bad="$work/planted.yaml"
    [ -f "$good" ] || render "$chart" "$env" >"$good"
    "$@" <"$good" >"$bad"
    if cmp -s "$good" "$bad"; then
        fail "$name: the command changed nothing, so the case tests nothing"
        return
    fi
    expect_policy_failure "$name" "$bad" "$chart" "$env" "$fragment"
}

# drop_doc KIND NAME: the rendered output (standard input) without the document of that kind and name.
drop_doc() {
    awk -v kind="$1" -v name="$2" '
        function flush() {
            if (n > 0 && !(is_kind && is_name)) { for (i = 1; i <= n; i++) print buf[i] }
            n = 0; is_kind = 0; is_name = 0
        }
        /^---$/ { flush(); buf[++n] = $0; next }
        { buf[++n] = $0 }
        $0 == "kind: " kind { is_kind = 1 }
        $0 == "  name: " name { is_name = 1 }
        END { flush() }
    '
}

# The LK_REPLICA_NAME entry moved after LK_REPLICA_ADDRESS, whose $(LK_REPLICA_NAME) Kubernetes then does not expand.
move_replica_name_after_address() {
    awk '
        /^ +- name: LK_REPLICA_NAME$/ {
            hold = $0
            while ((getline line) > 0 && line !~ /^ +- name: /) hold = hold "\n" line
            print line
            getline line
            print line
            print hold
            next
        }
        { print }
    '
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
    # B7: the agent load balancer listens on 443 and forwards to the container port named agent (the pods listen on 8444).
    plant "a port on the load balancer" hub prod '/name: lanekeeper-hub-agent$/,/targetPort: agent/s/port: 443/port: 22/' "must expose port 443 and nothing else"
    plant "the load balancer on the container port" hub prod '/name: lanekeeper-hub-agent$/,/targetPort: agent/s/port: 443/port: 8444/' "must expose port 443 and nothing else"
    plant "the load balancer forwarding to a number" hub prod '/name: lanekeeper-hub-agent$/,/protocol: TCP/s/targetPort: agent/targetPort: 8444/' "must forward to the container port named agent"
    plant "a wildcard external host" hub prod 's/pattern: github.com/pattern: "*.com"/' "wildcard"
    plant "external port 80" hub prod '/pattern: www.googleapis.com/,/port: 443/s/port: 443/port: 80/' "443"
    plant "no certificate" hub prod '/networking.gke.io\/pre-shared-certs/d' "certificate"
    plant "an HTTP listener" hub prod 's/protocol: HTTPS/protocol: HTTP/' "only HTTPS listeners"
    plant "no Cloud Armor" hub prod '/securityPolicy:/d' "Cloud Armor"
    plant "no hooks route" hub prod 's#value: /hooks/github#value: /hooks/other#' "/hooks/github"
    plant "web as a load balancer" hub prod '/name: lanekeeper-hub-web$/,/appProtocol: HTTPS/s/type: ClusterIP/type: LoadBalancer/' "Service/lanekeeper-hub-web"

    # S10: the disks.
    plant "an unencrypted volume class" hub prod 's/storageClassName: lanekeeper-hub-cmek/storageClassName: standard-rwo/' "customer-managed"

    # 09/fix, R2: the one reference the contract allows is the replica's own name, as exactly fieldRef metadata.name on LK_REPLICA_NAME.
    plant "a configMapKeyRef in the environment" hub kind '/^ +- name: LK_WEB_DIST$/{n;s/value: .*/valueFrom: {configMapKeyRef: {name: c, key: k}}/}' "LK_WEB_DIST takes its value from a valueFrom"
    plant "a resourceFieldRef in the environment" hub kind '/^ +- name: LK_WEB_DIST$/{n;s/value: .*/valueFrom: {resourceFieldRef: {resource: limits.memory}}/}' "LK_WEB_DIST takes its value from a valueFrom"
    plant "a fieldRef on another variable" hub kind '/^ +- name: LK_OPS_ADDR$/{n;s/value: .*/valueFrom: {fieldRef: {fieldPath: metadata.name}}/}' "LK_OPS_ADDR takes its value from a valueFrom"
    plant "the replica's name from another field" hub kind 's/fieldPath: metadata.name/fieldPath: metadata.namespace/' "LK_REPLICA_NAME takes its value from a valueFrom"
    plant "the replica's name fixed" hub kind '/^ +- name: LK_REPLICA_NAME$/{n;N;N;s/.*/              value: lanekeeper-hub-0/}' "LK_REPLICA_NAME must come from the pod's own name"

    # 09/fix, E4: the Jobs' variables are the Jobs' alone.
    plant "the migrator's credential in the hub" hub kind 's/^( *)- name: LK_WEB_DIST$/\1- name: LK_MIGRATOR_URL_FILE/' "LK_MIGRATOR_URL_FILE is a variable of the Jobs"
    plant "the migration timeout in the hub" hub kind 's/^( *)- name: LK_WEB_DIST$/\1- name: LK_MIGRATE_TIMEOUT_SECS/' "LK_MIGRATE_TIMEOUT_SECS is a variable of the Jobs"
    plant "the seed timeout in the hub" hub kind 's/^( *)- name: LK_WEB_DIST$/\1- name: LK_SEED_TIMEOUT_SECS/' "LK_SEED_TIMEOUT_SECS is a variable of the Jobs"

    # 09/fix, P1, P2: where the other replicas dial this one.
    plant "the replica address in the contract's example service" hub kind 's/lanekeeper-hub-replicas\.lanekeeper\.svc:8445/lanekeeper-hub.lanekeeper.svc:8445/' "LK_REPLICA_ADDRESS is"
    plant "the replica address in another namespace" hub kind 's/lanekeeper-hub-replicas\.lanekeeper\.svc:8445/lanekeeper-hub-replicas.other.svc:8445/' "LK_REPLICA_ADDRESS is"
    plant_cmd "the replica's name after the address" hub kind "LK_REPLICA_NAME must be defined before LK_REPLICA_ADDRESS" move_replica_name_after_address

    # 09/fix, README item 9: one name for the users and for Google, and it is the Gateway's.
    plant "an audience that is not the origin" hub kind '/^ +- name: LK_HUB_AUDIENCE$/{n;s#value: .*#value: "https://other.example.com"#}' "LK_PUBLIC_ORIGIN and LK_HUB_AUDIENCE must be one name"
    plant "an origin that is not https" hub kind '/^ +- name: LK_PUBLIC_ORIGIN$/{n;s#https://#http://#}' "LK_PUBLIC_ORIGIN must be https://<host>"
    plant "an origin that is not the Gateway's host" hub prod 's#https://lanekeeper-prod\.example\.com#https://elsewhere.example.com#' "the Gateway serves"

    # 09/fix, T1: the kubelet kills a hub that is still draining when the grace period is shorter than the shutdown.
    plant "a grace period shorter than the shutdown" hub kind 's/terminationGracePeriodSeconds: 45/terminationGracePeriodSeconds: 39/' "terminationGracePeriodSeconds is 39"

    planted_hub_jobs
}

# --- the hub image's Jobs (09/fix, J1 to J6): the real output with one thing wrong ---------------------------------------------------------
#
# The sed expressions are anchored to one document: the migration Job, the seed Job (and its ConfigMap, which has the same name), or the
# account and policy of the Jobs (both named lanekeeper-hub-jobs). In kind the seed is on, so the output has both Jobs.
planted_hub_jobs() {
    echo "== planted: the policies refuse the real output of the hub chart's Jobs with one thing wrong"
    local mig='/^  name: lanekeeper-hub-migrate$/,/^---$/' sd='/^  name: lanekeeper-hub-seed-[0-9a-f]+$/,/^---$/' jb='/^  name: lanekeeper-hub-jobs$/,/^---$/'
    local other='sha256:1111111111111111111111111111111111111111111111111111111111111111'

    # The image, the command and the ports.
    plant "a migration Job on another image" hub kind "$mig"'s/@sha256:0{64}/@'"$other"'/' "it must run the hub's image"
    plant "a seed Job on another image" hub kind "$sd"'s/@sha256:0{64}/@'"$other"'/' "it must run the hub's image"
    plant "a migration Job that serves" hub kind "$mig"'s/args: \["migrate"\]/args: ["serve"]/' "args must be exactly"
    plant "a seed Job with a dry run" hub kind "$sd"'s#args: \["seed", "/etc/lanekeeper/seed/seed.yaml"\]#args: ["seed", "--dry-run", "/etc/lanekeeper/seed/seed.yaml"]#' "args must be exactly"
    plant "a Job with a command" hub kind "$mig"'s/^( *)args: \["migrate"\]$/\1command: ["\/bin\/sh"]\n&/' "command is set"
    plant "a Job with a port" hub kind "$mig"'s/^( *)args: \["migrate"\]$/&\n\1ports:\n\1  - {name: debug, containerPort: 6060}/' "a Job has no ports"
    plant "a Job with a second container" hub kind "$mig"'s/^( *)containers:$/&\n\1  - name: sidecar\n\1    image: x/' "exactly one container"

    # The variables.
    plant "a migration Job with a variable of the hub" hub kind "$mig"'s/^( *)- name: LK_DB_CA_FILE$/\1- name: LK_GCP_PROJECT\n\1  value: x\n&/' "LK_GCP_PROJECT is not a variable this Job may carry"
    plant "a migration Job with the application credential" hub kind "$mig"'s/LK_MIGRATOR_URL_FILE/LK_DATABASE_URL_FILE/' "LK_DATABASE_URL_FILE is not a variable this Job may carry"
    plant "a seed Job with the migrator's credential" hub kind "$sd"'s/- name: LK_DATABASE_URL_FILE/- name: LK_MIGRATOR_URL_FILE/' "LK_MIGRATOR_URL_FILE is not a variable this Job may carry"
    plant "a seed Job with the migration timeout" hub kind "$sd"'s/- name: LK_SEED_TIMEOUT_SECS/- name: LK_MIGRATE_TIMEOUT_SECS/' "LK_MIGRATE_TIMEOUT_SECS is not a variable this Job may carry"
    plant "a migration credential outside its mount" hub kind "$mig"'s#value: /etc/lanekeeper/migrator/uri#value: /tmp/uri#' "LK_MIGRATOR_URL_FILE must be /etc/lanekeeper/migrator/uri"
    plant "a migration timeout over the hub's limit" hub kind "$mig"'{/- name: LK_MIGRATE_TIMEOUT_SECS$/{n;s/value: .*/value: "7201"/}}' "LK_MIGRATE_TIMEOUT_SECS must be a whole number"
    plant "a seed timeout over the hub's limit" hub kind "$sd"'{/- name: LK_SEED_TIMEOUT_SECS$/{n;s/value: .*/value: "3601"/}}' "LK_SEED_TIMEOUT_SECS must be a whole number"
    plant "a reference in the migration Job" hub kind "$mig"'{/- name: LK_DB_CA_FILE$/{n;s/value: .*/valueFrom: {configMapKeyRef: {name: c, key: k}}/}}' "LK_DB_CA_FILE takes its value from a valueFrom"

    # The volumes.
    plant "a Job with a volume it does not mount" hub kind "$mig"'s/^( *)volumes:$/&\n\1  - name: scratch\n\1    emptyDir: {}/' "volumes must be exactly"
    plant "the migrator's credential mounted writable" hub kind "$mig"'{/mountPath: \/etc\/lanekeeper\/migrator$/{n;d}}' "volumeMounts must be exactly"
    plant "the migrator's credential mounted elsewhere" hub kind "$mig"'s#mountPath: /etc/lanekeeper/migrator#mountPath: /etc/lanekeeper/db#' "volumeMounts must be exactly"
    plant "every key of the migrator's Secret" hub kind "$mig"'{/secretName: lanekeeper-db-migrator/,/path: uri/{/items:/d;/- key: uri/d;/path: uri/d}}' "only the uri key"
    plant "the migrator's credential readable by everyone" hub kind "$mig"'{/secretName: lanekeeper-db-migrator/,/path: uri/s/defaultMode: 288/defaultMode: 420/}' "world-readable"
    plant "every key of the Secret the seed reads" hub kind "$sd"'{/secretName: lanekeeper-db-app/,/path: uri/{/items:/d;/- key: uri/d;/path: uri/d}}' "only the uri key"
    plant "the whole CA secret in a Job" hub kind "$mig"'{/secretName: lanekeeper-db-ca/,/path: ca.crt/{/items:/d;/- key: ca.crt/d;/path: ca.crt/d}}' "ca.key"
    plant "the application role's Secret as the migrator's" hub kind "$mig"'s/secretName: lanekeeper-db-migrator/secretName: lanekeeper-db-app/' "the application role's Secret"
    plant "the migrator's Secret as the seed's" hub kind "$sd"'s/secretName: lanekeeper-db-app/secretName: lanekeeper-db-migrator/' "the Secret the hub mounts"
    plant "a seed file of another key" hub kind "$sd"'s/- key: seed.yaml/- key: other.yaml/' "the seed file must come from a ConfigMap"

    # The Job's own life.
    plant "a Job that restarts in place" hub kind "$mig"'s/restartPolicy: Never/restartPolicy: OnFailure/' "restartPolicy must be Never"
    plant "a Job without a backoff limit" hub kind "$mig"'{/^  backoffLimit:/d}' "backoffLimit is not set"
    plant "a Job that tries twenty times" hub kind "$mig"'s/^  backoffLimit: .*/  backoffLimit: 20/' "backoffLimit is 20"
    plant "a Job without a deadline" hub kind "$mig"'{/^  activeDeadlineSeconds:/d}' "activeDeadlineSeconds is not set"
    plant "a deadline that is the timeout" hub kind "$mig"'s/^  activeDeadlineSeconds: .*/  activeDeadlineSeconds: 600/' "activeDeadlineSeconds (600) must be longer"
    plant "a seed deadline that is the timeout" hub kind "$sd"'s/^  activeDeadlineSeconds: .*/  activeDeadlineSeconds: 120/' "activeDeadlineSeconds (120) must be longer"
    plant "a deadline of a day" hub kind "$mig"'s/^  activeDeadlineSeconds: .*/  activeDeadlineSeconds: 86400/' "a Job that has not finished"
    plant "a Job without resources" hub kind "$mig"'{/^ +memory: 512Mi$/d}' "resources.limits.memory"

    # Who the Jobs are.
    plant "a Job as the hub's account" hub kind "$mig"'s/serviceAccountName: lanekeeper-hub-jobs/serviceAccountName: lanekeeper-hub/' "serviceAccountName must be lanekeeper-hub-jobs"
    plant "a Job account with a Google identity" hub kind "$jb"'s/^  annotations:$/&\n    iam.gke.io\/gcp-service-account: x@example-project.iam.gserviceaccount.com/' "gives it a Google identity"
    plant "a Job account that mounts a token" hub kind "$jb"'s/^automountServiceAccountToken: false$/automountServiceAccountToken: true/' "automountServiceAccountToken must be stated and false; the Jobs"
    plant "a Job pod with the hub's label" hub kind "$mig"'s#app.kubernetes.io/name: lanekeeper-hub-jobs#app.kubernetes.io/name: lanekeeper-hub#' "the pod's label app.kubernetes.io/name must be lanekeeper-hub-jobs"
    plant_cmd "a Job account that is not rendered" hub kind "ServiceAccount/lanekeeper-hub-jobs is missing" drop_doc ServiceAccount lanekeeper-hub-jobs
    plant_cmd "a hub without its migration Job" hub kind "there is no migration Job" drop_doc Job lanekeeper-hub-migrate
    plant_append "a third Job of the hub's image" hub kind $'apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: lanekeeper-hub-backfill\n  namespace: lanekeeper\nspec:\n  template:\n    spec:\n      containers:\n        - name: c\n          image: x' "only two Jobs"

    # The order.
    plant "a migration Job that is not a hook" hub kind "$mig"'{/helm\.sh\/hook: /d}' "helm.sh/hook must be pre-install,pre-upgrade"
    plant "a migration Job after the install" hub kind "$mig"'s/helm.sh\/hook: .*/helm.sh\/hook: post-install,post-upgrade/' "helm.sh/hook must be pre-install,pre-upgrade"
    plant "a seed Job that is a hook" hub kind 's/^(  name: lanekeeper-hub-seed-[0-9a-f]+)$/\1\n  annotations:\n    helm.sh\/hook: pre-install,pre-upgrade/' "the seed Job is a Job, not a hook"
    plant "a Job account that is not a hook" hub kind "$jb"'{/helm\.sh\/hook: /d}' "it must be a hook of pre-install,pre-upgrade"
    plant "a Job account after the migration" hub kind "$jb"'s/hook-weight: "-10"/hook-weight: "5"/' "must be lower than the migration Job's"
    plant "a Job account that is deleted when the migration succeeds" hub kind "$jb"'s/hook-delete-policy: before-hook-creation/hook-delete-policy: hook-succeeded/' "hook-succeeded would delete it"

    # The Jobs' network (J3).
    plant_cmd "Jobs without their NetworkPolicy" hub kind "NetworkPolicy/lanekeeper-hub-jobs is missing" drop_doc NetworkPolicy lanekeeper-hub-jobs
    plant "a Jobs policy with an ingress rule" hub kind "$jb"'s/^  policyTypes: \[Ingress, Egress\]$/&\n  ingress:\n    - from:\n        - podSelector: {}\n      ports:\n        - {protocol: TCP, port: 8443}/' "admits nothing in"
    plant "a Jobs policy without egress deny" hub kind "$jb"'s/^  policyTypes: \[Ingress, Egress\]$/  policyTypes: [Ingress]/' "lanekeeper-hub-jobs: not default deny"
    plant "a Jobs policy to the metadata server" hub kind "$jb"'s/^  egress:$/&\n    - to:\n        - ipBlock: {cidr: 169.254.169.254\/32}\n      ports:\n        - {protocol: TCP, port: 80}/' "lanekeeper-hub-jobs: egress must be exactly"
    plant "a Jobs policy to the other replicas" hub kind "$jb"'s/^  egress:$/&\n    - to:\n        - podSelector:\n            matchLabels:\n              app.kubernetes.io\/name: lanekeeper-hub\n      ports:\n        - {protocol: TCP, port: 8445}/' "lanekeeper-hub-jobs: egress must be exactly"
    plant "a Jobs policy to the database on another port" hub kind "$jb"'s/port: 5432\}/port: 8000}/' "lanekeeper-hub-jobs: egress must be exactly"
    plant "a Jobs policy that selects the hub" hub kind "$jb"'s#app.kubernetes.io/name: lanekeeper-hub-jobs#app.kubernetes.io/name: lanekeeper-hub#' "it selects pods other than the Jobs'"

    # R2 and the seed's file: a ConfigMap is not a place for a key.
    local out="$work/seed-key.yaml"
    render hub kind --set-literal $'seed.content=version: 1\n# -----BEGIN PRIVATE KEY-----\nabc\n# -----END PRIVATE KEY-----\n' >"$out"
    expect_policy_failure "a private key in the seed file" "$out" hub kind "holds a private key"
}

# --- the hub's variables against the contract (09/fix, E1) ---------------------------------------------------------------------------------

planted_hub_variables() {
    echo "== planted: the hub's contract check refuses a StatefulSet that leaves out a variable the hub requires"
    local good="$work/good-hub-kind.yaml" bad="$work/planted-vars.yaml" name out count=0
    [ -f "$good" ] || render hub kind >"$good"
    check_hub_variables "$good" >/dev/null || fail "the unchanged kind render of the hub chart lacks a variable the hub requires"
    while IFS= read -r name; do
        drop_env "$good" "$name" >"$bad"
        count=$((count + 1))
        if cmp -s "$good" "$bad"; then
            fail "$name: the case changed nothing, so the render has no such variable"
        elif out="$(check_hub_variables "$bad" 2>&1)"; then
            fail "$name: the contract check accepted a StatefulSet without it"
        elif ! grep -qF -- "does not set $name," <<<"$out"; then
            fail "$name: refused, but not for it:"$'\n'"$out"
        else
            echo "  ok  without $name"
        fi
    done < <(required_hub_variables)
    [ "$count" -gt 0 ] || fail "no required variable was read from crates/hub/tests/hardening.rs"
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

    # D74, S10: the spool is a claim of its own on a customer-managed key, larger than its bound, written by one pod at a time, and kept.
    plant "the spool claim is not larger than its bound" agent sit1 's/^( *)storage: "1Gi"$/\1storage: "512Mi"/' "must be larger"
    plant "a bound that the claim cannot hold" agent sit1 's/^( *)value: "536870912"$/\1value: "2147483648"/' "must be larger"
    plant "a bound that is not a number" agent sit1 's/^( *)value: "536870912"$/\1value: "lots"/' "is not a number of bytes"
    plant "the spool on the cluster's default class" agent sit1 's/^( *)storageClassName: "lanekeeper-agent-cmek"$/\1storageClassName: "standard-rwo"/' "customer-managed"
    plant "the spool claim is read-write-many" agent sit1 's/^( *)- ReadWriteOnce$/\1- ReadWriteMany/' "exactly ReadWriteOnce"
    plant "the spool claim can be pruned" agent sit1 '/argocd.argoproj.io\/sync-options: Prune=false/d' "Prune=false is missing"
    plant "the spool claim goes with the release" agent sit1 '/^kind: PersistentVolumeClaim$/,/^spec:$/{/helm.sh\/resource-policy: keep/d}' "helm.sh/resource-policy: keep is missing"
    plant "a rolling update" agent sit1 '/^  strategy:$/{N;d}' "strategy.type must be Recreate"
    plant "the spool is an emptyDir" agent sit1 '/^        - name: spool$/{n;N;s/.*/          emptyDir: {}/}' "must be the PersistentVolumeClaim"
    plant "the spool path is not the mount" agent sit1 's#^( *)value: /var/lib/lanekeeper/spool$#\1value: /var/spool/lk#' "LK_SPOOL_DIR must be the path of the spool mount"
    plant "the spool is mounted read-only" agent sit1 's#^( *)mountPath: /var/lib/lanekeeper/spool$#&\n\1readOnly: true#' "spool is mounted read-only"
    plant "the spool is inside the export" agent sit1 's#^( *)value: /var/lib/lanekeeper/spool$#\1value: /mnt/csp-configuration/spool#;s#^( *)mountPath: /var/lib/lanekeeper/spool$#\1mountPath: /mnt/csp-configuration/spool#' "is inside the NFS root"

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
    # B8, crates/agent/HARDENING.md ("The config-server's namespace"): the rule for the config-server names ONE explicit namespace, chosen by
    # kubernetes.io/metadata.name, in the same `to` entry as the pods (latest-pci in sit1's values). A rule that is wider lets the agent's
    # requests reach, and be answered by, anything labelled app: csp-configuration-server in the cluster, and the endpoint has no
    # authentication (Q37). The seds change the lines of the NetworkPolicy's config-server entry only (the dns and ingress entries have other names).
    plant "the config-server in every namespace" agent sit1 '/^( *)- namespaceSelector:$/{N;N;/latest-pci/s/^( *)- namespaceSelector:\n[^\n]*\n[^\n]*$/\1- namespaceSelector: {}/}' "exactly one explicit namespace"
    plant "the config-server with no namespace" agent sit1 '/^( *)- namespaceSelector:$/{N;N;/latest-pci/{N;s/^( *)- namespaceSelector:\n[^\n]*\n[^\n]*\n *podSelector:/\1- podSelector:/}}' "exactly one explicit namespace"
    plant "the config-server's namespace chosen by an expression" agent sit1 '/^( *)matchLabels:$/{N;/latest-pci/s/^( *)matchLabels:\n( *)kubernetes.io\/metadata.name: latest-pci/\1matchExpressions:\n\2- {key: kubernetes.io\/metadata.name, operator: In, values: [latest-pci]}/}' "exactly one explicit namespace"
    plant "the config-server's namespace chosen by another label" agent sit1 's#kubernetes.io/metadata.name: latest-pci$#team: pci#' "exactly one explicit namespace"
    plant "the config-server's pods chosen by an expression" agent sit1 '/^( *)podSelector:$/{N;N;/csp-configuration-server/s/^( *)podSelector:\n( *)matchLabels:\n/\1podSelector:\n\2matchExpressions:\n\2- {key: app, operator: Exists}\n\2matchLabels:\n/}' "exactly one explicit namespace"
    plant "every pod of the config-server's namespace" agent sit1 '/^ *podSelector:$/{N;N;/csp-configuration-server/d}' "unrecognised"
    plant "the config-server in kube-system" agent sit1 's#kubernetes.io/metadata.name: latest-pci$#kubernetes.io/metadata.name: kube-system#' "namespace of the cluster's own components"
    plant "the config-server in kube-public" agent sit1 's#kubernetes.io/metadata.name: latest-pci$#kubernetes.io/metadata.name: kube-public#' "namespace of the cluster's own components"
    plant "the config-server in kube-node-lease" agent sit1 's#kubernetes.io/metadata.name: latest-pci$#kubernetes.io/metadata.name: kube-node-lease#' "namespace of the cluster's own components"
    plant "the config-server in the agent's own namespace" agent sit1 's#kubernetes.io/metadata.name: latest-pci$#kubernetes.io/metadata.name: lanekeeper#' "agent's own namespace"
    plant "the URL of the config-server by its short name" agent sit1 's#http://csp-configuration-server\.latest-pci\.svc\.cluster\.local:8888#http://csp-configuration-server:8888#' "LK_CONFIG_SERVER_URL"
    plant "the URL of the config-server in another namespace" agent sit1 's#\.latest-pci\.svc\.cluster\.local#.other-pci.svc.cluster.local#' "LK_CONFIG_SERVER_URL"
    plant "ingress on another port" agent sit1 's/port: 9090}/port: 8080}/' "9090"
    plant_append "a FQDN policy" agent sit1 $'apiVersion: networking.gke.io/v1alpha1\nkind: FQDNNetworkPolicy\nmetadata:\n  name: lanekeeper-agent-external\n  namespace: lanekeeper\nspec:\n  podSelector:\n    matchLabels:\n      app.kubernetes.io/name: lanekeeper-agent\n  egress:\n    - matches:\n        - pattern: example.com\n      ports:\n        - {protocol: TCP, port: 443}' "FQDNNetworkPolicy"
    plant_append "a second door" agent sit1 $'apiVersion: v1\nkind: Service\nmetadata:\n  name: lanekeeper-agent\n  namespace: lanekeeper\nspec:\n  type: LoadBalancer\n  ports:\n    - {port: 9090}' "LoadBalancer"

    # Without a key the claim is on the cluster's default class and says so, and the policy allows that with a warning (A11b, Q29). A swimlane
    # is a real cluster, so the run fails on the warning, as it does for prod and staging; only kind renders it.
    render agent sit1 --set spool.cmekKey= --set spool.fallbackClassName=premium-rwo >"$work/fallback.yaml"
    if ! grep -q '^ *lanekeeper.io/cmek: fallback$' "$work/fallback.yaml" || ! grep -q '^ *storageClassName: "premium-rwo"$' "$work/fallback.yaml" || grep -q '^kind: StorageClass$' "$work/fallback.yaml"; then
        fail "the fallback: the claim is not marked, or not on the fallback class, or a StorageClass was rendered without a key"
    fi
    expect_policy_failure "no customer-managed key for the spool (the fallback)" "$work/fallback.yaml" agent sit1 "Google-managed encryption"

    echo "== planted: the contract check refuses what the files do not say"
    plant_contract "the workload Role gives less than RBAC.md" 's/verbs: \["patch"\]/verbs: ["get"]/' "differs from RBAC.md"
    plant_contract "the workload Role gives more than RBAC.md" 's/resources: \["jobs"\]/resources: ["jobs", "cronjobs"]/' "differs from RBAC.md"
    plant_contract "a watched namespace without a Role" 's/^( *)value: "sit1"$/\1value: "sit1,sit2"/' "sit2"
    plant_contract "the config-server's port" 's/port: 8888/port: 8889/' "differs from HARDENING.md"
    plant_contract "the config-server in every namespace" '/^( *)- namespaceSelector:$/{N;N;/latest-pci/s/^( *)- namespaceSelector:\n[^\n]*\n[^\n]*$/\1- namespaceSelector: {}/}' "differs from HARDENING.md"
    plant_contract "the config-server's namespace chosen by another label" 's#kubernetes.io/metadata.name: latest-pci$#team: pci#' "differs from HARDENING.md"
    plant_contract "the config-server's rule without a namespace" '/^( *)- namespaceSelector:$/{N;N;/latest-pci/{N;s/^( *)- namespaceSelector:\n[^\n]*\n[^\n]*\n *podSelector:/\1- podSelector:/}}' "differs from HARDENING.md"
    plant_contract "the health port" 's/port: 9090}/port: 8080}/' "differs from HARDENING.md"
    plant_contract "the liveness probe" 's/periodSeconds: 20/periodSeconds: 60/' "livenessProbe"
    plant_contract "the readiness probe" 's#path: /readyz#path: /healthz#' "readinessProbe"
    plant_contract "the container's security context" 's/readOnlyRootFilesystem: true/readOnlyRootFilesystem: false/' "securityContext"
    plant_contract "no token" 's/^( *)automountServiceAccountToken: true$/\1automountServiceAccountToken: false/' "automountServiceAccountToken"
    plant_contract "the binding's role" '/^  kind: Role$/{n;s/name: lanekeeper-agent-secrets/name: lanekeeper-agent-workloads/}' "roleRef"
    # D74: the volumes, the mounts and the path variables equal HARDENING.md; its claim makes the pod Recreate, and the chart renders the claim.
    plant_contract "the spool claim of the pod" 's/claimName: lanekeeper-agent-spool/claimName: lanekeeper-agent-other/' "volume spool is"
    plant_contract "the spool mount's path" 's#^( *)mountPath: /var/lib/lanekeeper/spool$#\1mountPath: /var/spool/lk#' "volumeMounts differ from HARDENING.md"
    plant_contract "the spool directory" 's#^( *)value: /var/lib/lanekeeper/spool$#\1value: /var/spool/lk#' "LK_SPOOL_DIR is"
    plant_contract "a rolling update" '/^  strategy:$/{N;d}' "strategy.type must be Recreate, because the pod of HARDENING.md"
    plant_contract "no claim is rendered" '/^# Source: lanekeeper-agent\/templates\/pvc-spool.yaml$/,/^---$/d' "renders no PersistentVolumeClaim"
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
    accept_variant "another namespace and service for the config-server" --set configServer.namespace=other-pci --set configServer.service=config-server
    in_variant '^ +kubernetes.io/metadata.name: other-pci$' "another namespace and service for the config-server"
    in_variant '^ +value: "http://config-server.other-pci.svc.cluster.local:8888"$' "another namespace and service for the config-server"
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
    accept_variant "a larger spool with a raised bound" --set spool.size=4Gi --set spool.maxBytes=2147483648
    in_variant '^ +storage: "4Gi"$' "a larger spool with a raised bound"
    in_variant '^ +value: "2147483648"$' "a larger spool with a raised bound"
    accept_variant "a spool on faster disks" --set spool.diskType=pd-ssd
    in_variant '^ +type: pd-ssd$' "a spool on faster disks"
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
    # B8: the config-server's namespace is a required value of the swimlane, one explicit name; never one of the cluster's own, never the agent's.
    expect_agent_failure "no namespace for the config-server" "namespace" --set configServer.namespace=
    expect_agent_failure "the config-server in kube-system" "namespace" --set configServer.namespace=kube-system
    expect_agent_failure "the config-server in kube-public" "namespace" --set configServer.namespace=kube-public
    expect_agent_failure "the config-server in kube-node-lease" "namespace" --set configServer.namespace=kube-node-lease
    expect_agent_failure "the config-server in the agent's own namespace" "agent's own namespace" --set configServer.namespace=lanekeeper
    expect_agent_failure "a namespace for the config-server that is not a name" "namespace" --set configServer.namespace="Latest PCI"
    expect_agent_failure "a service for the config-server that is not a name" "service" --set configServer.service=Bad_Service
    expect_agent_failure "an unknown value of the config-server" "url" --set configServer.url=http://x
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
    # D74, S10: the spool's values. The tunable LK_SPOOL_MAX_BYTES is the chart's (spool.maxBytes), not a tunable, so the claim cannot be left
    # behind it.
    expect_agent_failure "a spool bound as a tunable" "LK_SPOOL_MAX_BYTES" --set-json 'config.tuning={"LK_SPOOL_MAX_BYTES":"1"}'
    expect_agent_failure "a spool path as a tunable" "LK_SPOOL_DIR" --set-json 'config.tuning={"LK_SPOOL_DIR":"/mnt/csp-configuration/spool"}'
    expect_agent_failure "a spool in a unit the policy cannot compare" "size" --set spool.size=1Ti
    expect_agent_failure "a spool size that is not a size" "size" --set spool.size=big
    expect_agent_failure "a spool bound under the agent's floor" "maxBytes" --set spool.maxBytes=1000
    expect_agent_failure "a spool bound over the agent's ceiling" "maxBytes" --set spool.maxBytes=68719476737
    expect_agent_failure "a spool key that is not a key" "cmekKey" --set spool.cmekKey=my-key
    expect_agent_failure "a spool disk type that is not one" "diskType" --set spool.diskType=pd-extreme
    expect_agent_failure "a spool value that does not exist" "reclaim" --set spool.reclaim=Retain
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
    # 09/fix, J3: the pods of the hub image's Jobs, by their own label and on 5432 only.
    plant "the Jobs missing from the callers" db prod '/^    - from:$/{N;N;N;N;N;/lanekeeper-hub-jobs/d}' "ingress must be exactly"
    plant "the Jobs on the status port" db prod '/lanekeeper-hub-jobs/,/port: 5432\}/s/port: 5432\}/port: 8000}/' "ingress must be exactly"
    plant "every pod of the part for the Jobs" db prod 's#app.kubernetes.io/name: lanekeeper-hub-jobs#app.kubernetes.io/part-of: lanekeeper#' "ingress must be exactly"
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
    # The hub's keys and secrets (09/fix, I1).
    expect_render_failure "no key ring" gcp-storage prod "kms.keyRing is required" --set kms.keyRing=
    expect_render_failure "a key ring that is not a name" gcp-storage prod "keyRing" --set kms.keyRing=Bad_Ring
    expect_render_failure "no agent CA key" gcp-storage prod "kms.keys.agentCa is required" --set kms.keys.agentCa=
    expect_render_failure "no replica CA key" gcp-storage prod "kms.keys.replicaCa is required" --set kms.keys.replicaCa=
    expect_render_failure "no audit checkpoint key" gcp-storage prod "kms.keys.auditCheckpoint is required" --set kms.keys.auditCheckpoint=
    expect_render_failure "no vault key" gcp-storage prod "kms.keys.vault is required" --set kms.keys.vault=
    expect_render_failure "one key for both CAs" gcp-storage prod "four different keys" --set kms.keys.replicaCa=lanekeeper-agent-ca-prod
    expect_render_failure "the vault key that is also the checkpoint key" gcp-storage prod "four different keys" --set kms.keys.vault=lanekeeper-audit-checkpoint-prod
    expect_render_failure "a key that is not a name" gcp-storage prod "vault" --set kms.keys.vault=Bad_Key
    expect_render_failure "a key held outside Google" gcp-storage prod "protectionLevel" --set kms.protectionLevel=EXTERNAL
    expect_render_failure "no protection level" gcp-storage prod "protectionLevel" --set kms.protectionLevel=
    expect_render_failure "a rotation period that is not seconds" gcp-storage prod "vaultRotationPeriod" --set kms.vaultRotationPeriod=weekly
    expect_render_failure "a destroy delay that is not seconds" gcp-storage prod "destroyScheduledDuration" --set kms.destroyScheduledDuration=30d
    expect_render_failure "a setting of a key that the chart does not have" gcp-storage prod "importOnly" --set kms.importOnly=true
    expect_render_failure "the same secret twice" gcp-storage prod "a name twice" --set secrets.hub.webKey=lanekeeper-web-cert
    expect_render_failure "the two CA certificates in one secret" gcp-storage prod "a name twice" --set secrets.hub.replicaCaCert=lanekeeper-agent-ca-cert
    expect_render_failure "a secret that takes a constant of the hub's code" gcp-storage prod "constants" --set secrets.hub.loginCookie=write-preview-key
    expect_render_failure "a secret name that is not a Kubernetes name" gcp-storage prod "webCert" --set secrets.hub.webCert=Web_Cert
    expect_render_failure "a secret the hub does not have" gcp-storage prod "extra" --set secrets.hub.extra=x
    expect_render_failure "a secrets key that is not a key" gcp-storage prod "cmekKey" --set secrets.cmekKey=nonsense
    expect_render_failure "a secrets key in another location" gcp-storage prod "a key must be in the location of the secret" --set secrets.cmekKey=projects/example-project/locations/other-place/keyRings/r/cryptoKeys/k
    expect_render_failure "a value for a secret" gcp-storage prod "value" --set secrets.value=hunter2
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

    # I1 (S5, S7, S8, S9, S10): the hub's keys and secrets, and the roles on them.
    local in_key='/^kind: KMSCryptoKey$/,/^---$/' in_secret='/^kind: SecretManagerSecret$/,/^---$/' in_ring='/^kind: KMSKeyRing$/,/^---$/'
    plant "cloudkms.admin for the hub" gcp-storage prod '/name: lanekeeper-hub-vault$/,/^---$/s#roles/cloudkms.cryptoKeyEncrypterDecrypter#roles/cloudkms.admin#' "not one of the roles"
    plant "signing without verifying" gcp-storage prod '/name: lanekeeper-hub-sign-agent-ca$/,/^---$/s#roles/cloudkms.signerVerifier#roles/cloudkms.signer#' "not one of the roles"
    plant "a signerVerifier on the vault key" gcp-storage prod '/name: lanekeeper-hub-sign-agent-ca$/,/^---$/s#name: lanekeeper-agent-ca-prod#name: lanekeeper-vault-prod#' "signerVerifier is for the agent CA"
    plant "an encrypterDecrypter on a CA key" gcp-storage prod '/name: lanekeeper-hub-vault$/,/^---$/s#name: lanekeeper-vault-prod#name: lanekeeper-agent-ca-prod#' "only the vault key"
    plant "a signerVerifier on the key ring" gcp-storage prod '/name: lanekeeper-hub-sign-agent-ca$/,/^---$/{s#kind: KMSCryptoKey#kind: KMSKeyRing#;s#name: lanekeeper-agent-ca-prod#name: lanekeeper#}' "on a KMSCryptoKey, never on"
    plant "a signerVerifier on the project" gcp-storage prod '/name: lanekeeper-hub-sign-agent-ca$/,/^---$/{s#kind: KMSCryptoKey#kind: Project#;s#name: lanekeeper-agent-ca-prod#external: projects/example-project#}' "on a KMSCryptoKey, never on"
    plant "a signerVerifier on a key that is not here" gcp-storage prod '/name: lanekeeper-hub-sign-agent-ca$/,/^---$/s#name: lanekeeper-agent-ca-prod#name: lanekeeper-another-prod#' "is not in this output"
    plant "the database signs" gcp-storage prod '/name: lanekeeper-hub-sign-replica-ca$/,/^---$/s#lanekeeper-hub@#lanekeeper-db@#' "only the hub's service account signs"
    plant "a person signs" gcp-storage prod '/name: lanekeeper-hub-sign-replica-ca$/,/^---$/s#member: ".*"#member: "user:someone@example.com"#' "not a service account"
    plant "a group signs" gcp-storage prod '/name: lanekeeper-hub-sign-audit-checkpoint$/,/^---$/s#member: ".*"#member: "group:everyone@example.com"#' "not a service account"
    plant "the vault key is used by a service agent" gcp-storage prod '/name: lanekeeper-hub-vault$/,/^---$/s#member: ".*"#member: "serviceAccount:service-123456789012@gs-project-accounts.iam.gserviceaccount.com"#' "made here"
    plant "the vault key is used by the database" gcp-storage prod '/name: lanekeeper-hub-vault$/,/^---$/s#lanekeeper-hub@#lanekeeper-db@#' "Google-managed service agent"
    plant "a secretAccessor at project level" gcp-storage prod '/name: lanekeeper-hub-read-write-preview-key$/,/^---$/{s#kind: SecretManagerSecret#kind: Project#;s#name: write-preview-key#external: projects/example-project#}' "on a SecretManagerSecret, never on"
    plant "a secretAccessor on a secret that is not here" gcp-storage prod '/name: lanekeeper-hub-read-write-preview-key$/,/^---$/s#name: write-preview-key#name: another-secret#' "is not in this output"
    plant "the database reads a secret" gcp-storage prod '/name: lanekeeper-hub-read-github-read-token$/,/^---$/s#lanekeeper-hub@#lanekeeper-db@#' "only the hub's service account reads"
    plant "a person reads a secret" gcp-storage prod '/name: lanekeeper-hub-read-lanekeeper-web-key$/,/^---$/s#member: ".*"#member: "user:someone@example.com"#' "not a service account"
    plant "everyone reads a secret" gcp-storage prod '/name: lanekeeper-hub-read-lanekeeper-web-key$/,/^---$/s#member: ".*"#member: "allAuthenticatedUsers"#' "which is everyone"
    plant "a secret administrator" gcp-storage prod '/name: lanekeeper-hub-read-write-preview-key$/,/^---$/s#roles/secretmanager.secretAccessor#roles/secretmanager.admin#' "not one of the roles"
    plant "a CA key that decrypts" gcp-storage prod "$in_key"'{/name: lanekeeper-agent-ca-prod$/,/^---$/s#purpose: ASYMMETRIC_SIGN#purpose: ENCRYPT_DECRYPT#}' "must be ASYMMETRIC_SIGN"
    plant "a CA key of another algorithm" gcp-storage prod "$in_key"'{/name: lanekeeper-replica-ca-prod$/,/^---$/s#algorithm: EC_SIGN_P256_SHA256#algorithm: RSA_SIGN_PSS_2048_SHA256#}' "EC_SIGN_P256_SHA256"
    plant "a checkpoint key of another curve" gcp-storage prod "$in_key"'{/name: lanekeeper-audit-checkpoint-prod$/,/^---$/s#algorithm: EC_SIGN_P256_SHA256#algorithm: EC_SIGN_P384_SHA384#}' "audit-checkpoint-signing"
    plant "a vault key that signs" gcp-storage prod "$in_key"'{/name: lanekeeper-vault-prod$/,/^---$/s#purpose: ENCRYPT_DECRYPT#purpose: ASYMMETRIC_SIGN#}' "must be ENCRYPT_DECRYPT"
    plant "a vault key of another algorithm" gcp-storage prod "$in_key"'{/name: lanekeeper-vault-prod$/,/^---$/s#algorithm: GOOGLE_SYMMETRIC_ENCRYPTION#algorithm: EC_SIGN_P256_SHA256#}' "GOOGLE_SYMMETRIC_ENCRYPTION"
    plant "a key in software in prod" gcp-storage prod "$in_key"'s#protectionLevel: HSM#protectionLevel: SOFTWARE#' "HSM in prod"
    plant "a key without a protection level" gcp-storage prod "$in_key"'{/protectionLevel: HSM/d}' "protectionLevel"
    plant "an imported key" gcp-storage prod "$in_key"'{/name: lanekeeper-agent-ca-prod$/,/^---$/s#^  purpose: ASYMMETRIC_SIGN$#&\n  importOnly: true#}' "imported"
    plant "a key without a first version" gcp-storage prod "$in_key"'{/name: lanekeeper-agent-ca-prod$/,/^---$/s#^  purpose: ASYMMETRIC_SIGN$#&\n  skipInitialVersionCreation: true#}' "skipInitialVersionCreation"
    plant "a signing key that rotates" gcp-storage prod "$in_key"'{/name: lanekeeper-agent-ca-prod$/,/^---$/s#^  destroyScheduledDuration:#  rotationPeriod: "7776000s"\n&#}' "rotationPeriod"
    plant "a key in a ring that is not here" gcp-storage prod "$in_key"'{/name: lanekeeper-agent-ca-prod$/,/^---$/s#^    name: lanekeeper$#    name: elsewhere#}' "key ring"
    plant "two keys of one purpose" gcp-storage prod "$in_key"'{/name: lanekeeper-replica-ca-prod$/,/^---$/s#lanekeeper.io/purpose: replica-ca#lanekeeper.io/purpose: agent-ca#}' "exactly one agent-ca key"
    plant "a key of no purpose" gcp-storage prod "$in_key"'{/name: lanekeeper-vault-prod$/,/^---$/s#lanekeeper.io/purpose: vault#lanekeeper.io/purpose: scratch#}' "lanekeeper.io/purpose"
    plant "keys deleted with their resource" gcp-storage prod "$in_key"'s#deletion-policy: abandon#deletion-policy: delete#' "deletion-policy: abandon"
    plant "keys that a sync may prune" gcp-storage prod "$in_key"'s#Prune=false#Prune=true#' "Prune=false"
    plant "a key ring deleted with its resource" gcp-storage prod "$in_ring"'s#deletion-policy: abandon#deletion-policy: delete#' "deletion-policy: abandon"
    plant "a key ring that a sync may prune" gcp-storage prod "$in_ring"'s#Prune=false#Prune=true#' "Prune=false"
    plant "a key ring without a location" gcp-storage prod "$in_ring"'{/^  location: /d}' "names no location"
    plant "secrets deleted with their resource" gcp-storage prod "$in_secret"'s#deletion-policy: abandon#deletion-policy: delete#' "deletion-policy: abandon"
    plant "secrets that a sync may prune" gcp-storage prod "$in_secret"'s#Prune=false#Prune=true#' "Prune=false"
    plant "a secret that holds a value" gcp-storage prod "$in_secret"'{/name: write-preview-key$/,/^---$/s#^spec:$#spec:\n  secretData: {value: hunter2}#}' "replication and nothing else"
    plant "a secret that expires" gcp-storage prod "$in_secret"'{/name: write-preview-key$/,/^---$/s#^spec:$#spec:\n  ttl: 3600s#}' "replication and nothing else"
    plant "a secret with a rotation" gcp-storage prod "$in_secret"'{/name: write-preview-key$/,/^---$/s#^spec:$#spec:\n  rotation: {rotationPeriod: 3600s}#}' "replication and nothing else"
    plant "secrets without a key" gcp-storage prod "$in_secret"'s#^( *)external: .*#\1external: ""#' "customer-managed key"
    plant "secrets with a key that is not one" gcp-storage prod "$in_secret"'s#^( *)external: .*#\1external: "lanekeeper-secrets"#' "not a Cloud KMS key"
    plant "automatic replication" gcp-storage prod "$in_secret"'{/name: write-preview-key$/,/^---$/{/^    userManaged:$/,/^---$/{/^---$/!d}}}' "userManaged"
    plant "secrets of no purpose" gcp-storage prod "$in_secret"'s#lanekeeper.io/purpose: secret#lanekeeper.io/purpose: docs#' "lanekeeper.io/purpose must be secret"
    plant "a key of the secrets that Secret Manager may not use" gcp-storage prod 's#gcp-sa-secretmanager#compute-system#' "no IAMPolicyMember grants the Secret Manager service agent"
    plant_append "a secret version, which holds a value" gcp-storage prod $'apiVersion: secretmanager.cnrm.cloud.google.com/v1beta1\nkind: SecretManagerSecretVersion\nmetadata:\n  name: write-preview-key-1\n  namespace: lanekeeper-gcp\nspec:\n  secretRef: {name: write-preview-key}\n  secretData: {value: aHVudGVyMg==}' "never a value"
    plant_append "a Kubernetes Secret in the cloud chart" gcp-storage prod $'apiVersion: v1\nkind: Secret\nmetadata:\n  name: write-preview-key\n  namespace: lanekeeper-gcp\nstringData:\n  value: hunter2' "charts render no Secret"

    # The fallback (plan 09 A11b, Q29): a bucket without a key renders with a warning, which fails in prod and staging.
    local out
    if ! out="$(render gcp-storage prod --set buckets.docs.cmekKey= 2>&1)"; then
        fail "the fallback (no key for the docs bucket) did not render: $(tail -n1 <<<"$out")"
    else
        printf '%s\n' "$out" >"$work/fallback.yaml"
        expect_policy_failure "the fallback fails in prod" "$work/fallback.yaml" gcp-storage prod "Google-managed"
        check_policy "$work/fallback.yaml" gcp-storage kind >/dev/null 2>&1 || true
    fi
    if ! out="$(render gcp-storage prod --set secrets.cmekKey= 2>&1)"; then
        fail "the fallback (no key for the secrets) did not render: $(tail -n1 <<<"$out")"
    else
        printf '%s\n' "$out" >"$work/fallback-secrets.yaml"
        expect_policy_failure "the fallback for the secrets fails in prod" "$work/fallback-secrets.yaml" gcp-storage prod "Google-managed"
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
    # 09/fix, J5: the workloads project allows the hub's Jobs and the ConfigMap of the seed file, and a project without them refuses the chart.
    local weaker="$work/projects-weaker.yaml"
    render hub kind >"$rendered"
    sed -E '/^    - group: batch$/{N;d}' deploy/argocd/platform/projects.yaml >"$weaker"
    if out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" "/work/projects-weaker.yaml" 2>&1)"; then
        fail "the hub's Jobs were accepted by a project without batch/Job"
    elif ! grep -qF "no project allows the kind Job" <<<"$out"; then
        fail "the hub's Jobs were refused by a project without batch/Job, but not for their kind:"$'\n'"$out"
    else
        echo "  ok  the hub's Jobs are not allowed by a project without batch/Job"
    fi
    sed -E '/^    - group: ""$/{N;/kind: ConfigMap/d}' deploy/argocd/platform/projects.yaml >"$weaker"
    if out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" "/work/projects-weaker.yaml" 2>&1)"; then
        fail "the seed's ConfigMap was accepted by a project without ConfigMap"
    elif ! grep -qF "no project allows the kind ConfigMap" <<<"$out"; then
        fail "the seed's ConfigMap was refused by a project without ConfigMap, but not for its kind:"$'\n'"$out"
    else
        echo "  ok  the seed's ConfigMap is not allowed by a project without ConfigMap"
    fi
    printf -- '---\napiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: planted\n  namespace: lanekeeper\n' >>"$rendered"
    if out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" deploy/argocd/platform/projects.yaml 2>&1)"; then
        fail "a CronJob in the hub's output was accepted by the scope check"
    elif ! grep -qF "no project allows the kind CronJob" <<<"$out"; then
        fail "a CronJob in the hub's output was refused, but not for its kind:"$'\n'"$out"
    else
        echo "  ok  a CronJob in the hub's output is not allowed by its project"
    fi
    # 09/fix, I1: the cloud project allows the hub's key ring, keys and secret containers, a project without one of the three refuses the chart,
    # and a version of a key or a secret (where a value is) is allowed by no project.
    render gcp-storage prod >"$rendered"
    local group kind entry
    for entry in "kms.cnrm.cloud.google.com KMSKeyRing" "kms.cnrm.cloud.google.com KMSCryptoKey" "secretmanager.cnrm.cloud.google.com SecretManagerSecret"; do
        read -r group kind <<<"$entry"
        sed -E "/^    - group: ${group//./\\.}\$/{N;/kind: ${kind}\$/d}" deploy/argocd/platform/projects.yaml >"$weaker"
        if cmp -s "$weaker" deploy/argocd/platform/projects.yaml; then
            fail "the sed expression that takes $kind out of the projects changed nothing, so the case tests nothing"
        elif out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" "/work/projects-weaker.yaml" 2>&1)"; then
            fail "the gcp chart's $kind was accepted by a project without it"
        elif ! grep -qF "no project allows the kind $kind" <<<"$out"; then
            fail "the gcp chart's $kind was refused by a project without it, but not for its kind:"$'\n'"$out"
        else
            echo "  ok  the cloud chart's $kind is not allowed by a project without it"
        fi
    done
    for entry in "secretmanager.cnrm.cloud.google.com/v1beta1 SecretManagerSecretVersion" "kms.cnrm.cloud.google.com/v1beta1 KMSCryptoKeyVersion"; do
        read -r group kind <<<"$entry"
        render gcp-storage prod >"$rendered"
        printf -- '---\napiVersion: %s\nkind: %s\nmetadata:\n  name: planted\n  namespace: lanekeeper-gcp\n' "$group" "$kind" >>"$rendered"
        if out="$(argocd_conftest argocd_scope "/work/scope-planted.yaml" deploy/argocd/platform/projects.yaml 2>&1)"; then
            fail "a $kind in the gcp chart's output was accepted by the scope check"
        elif ! grep -qF "no project allows the kind $kind" <<<"$out"; then
            fail "a $kind in the gcp chart's output was refused, but not for its kind:"$'\n'"$out"
        else
            echo "  ok  a $kind in the cloud chart's output is not allowed by its project"
        fi
    done
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

# hub_code_secrets: the Secret Manager secrets whose names are constants of the hub's code and not settings (crates/hub/HARDENING.md), one
# per line, read from the code so that a renamed constant is a failure here and not a hub that cannot start: the preview key (hub-writes), the
# personal-token key (hub-mcp), and the defaults of LK_GIT_READ_REF and LK_GIT_WEBHOOK_REF (hub), printed as `NAME=VARIABLE` for the last two,
# because a chart that sets the variable overrides the name. The count (4) must be what was read: a reader that no longer understands the code
# is an error, never an empty list that every chart passes (09/fix, I1).
hub_code_secrets() {
    local out count
    out="$(
        sed -nE 's/^pub const PREVIEW_KEY_SECRET: &str = "([^"]+)";.*/\1=/p' crates/hub-writes/src/config.rs
        sed -nE 's/^pub const KEY_SECRET_NAME: &str = "([^"]+)";.*/\1=/p' crates/hub-mcp/src/auth/pat.rs
        sed -nE 's/.*secret_ref_or\("(LK_GIT_(READ|WEBHOOK)_REF)", "([^"]+)"\).*/\3=\1/p' crates/hub/src/config/git.rs
    )"
    count="$(grep -c . <<<"$out" || true)"
    if [ "$count" != 4 ]; then
        echo "render.sh: read $count constant secret names from the hub's code (hub-writes config.rs, hub-mcp auth/pat.rs, hub config/git.rs) and expected 4" >&2
        return 1
    fi
    printf '%s\n' "$out"
}

# gcp_hub_objects FILE: what the gcp chart's output says about the hub's secrets and keys, one record a line:
#   secret|NAME|PROJECT           a SecretManagerSecret
#   key|FULL NAME|NAME|PURPOSE    a KMSCryptoKey, as projects/P/locations/L/keyRings/R/cryptoKeys/K, with its lanekeeper.io/purpose label
#   binding|ROLE|KIND|NAME|MEMBER an IAMPolicyMember on a resource of this output by name (an external resource has NAME external:...)
gcp_hub_objects() {
    awk '
        function flush() {
            if (kind == "SecretManagerSecret") printf "secret|%s|%s\n", name, project
            else if (kind == "KMSKeyRing") ring_location[name] = location
            else if (kind == "KMSCryptoKey") { n++; key_name[n] = name; key_project[n] = project; key_ring[n] = ring; key_purpose[n] = purpose }
            else if (kind == "IAMPolicyMember") printf "binding|%s|%s|%s|%s\n", role, refkind, refname, member
            kind = name = project = location = ring = purpose = role = member = refkind = refname = ""; inref = 0; inring = 0
        }
        /^---$/ { flush(); next }
        /^kind: / { kind = $2; next }
        /^  name: / && name == "" { name = $2; next }
        /^    cnrm.cloud.google.com\/project-id: / { project = $2; gsub(/"/, "", project); next }
        /^    lanekeeper.io\/purpose: / { purpose = $2; next }
        /^  location: / { location = $2; next }
        /^  keyRingRef:/ { inring = 1; next }
        inring && /^    name: / { ring = $2; inring = 0; next }
        /^  role: / { role = $2; next }
        /^  member: / { member = $2; gsub(/"/, "", member); next }
        /^  resourceRef:/ { inref = 1; next }
        inref && /^    kind: / { refkind = $2; next }
        inref && /^    name: / { refname = $2; next }
        inref && /^    external: / { refname = "external:" $2; next }
        END {
            flush()
            for (i = 1; i <= n; i++)
                printf "key|projects/%s/locations/%s/keyRings/%s/cryptoKeys/%s|%s|%s\n", key_project[i], ring_location[key_ring[i]], key_ring[i], key_name[i], key_name[i], key_purpose[i]
        }
    ' "$1"
}

# cross_check_hub_names GCP HUB: the hub reads each Secret Manager secret and uses each Cloud KMS key that the gcp chart makes, in the project
# it names, and its account may use each one for the one thing it is for (09/fix, I1; S5, S7, S8, S9). A name that differs in the two charts is a
# hub that cannot start (a secret that does not exist, a key that is not there) or a container that nothing reads; both are found here and not on
# the first apply. Print every disagreement; return 1 if there is one.
cross_check_hub_names() {
    local gcp="$1" hub="$2" status=0 objs gsa project member name var ref key key_purpose record
    objs="$(gcp_hub_objects "$gcp")"
    gsa="$(grep -E 'iam.gke.io/gcp-service-account:' "$hub" | sed -E 's/.*: "?([^" ]+)"?$/\1/')"
    member="serviceAccount:$gsa"
    project="$(hub_env "$hub" LK_GCP_PROJECT)"

    # The secrets: six named by the chart's variables, four constants of the code (two of them overridable by the chart's variables).
    local names=() entry code
    for var in LK_WEB_CERT_REF LK_WEB_KEY_REF LK_AGENT_CA_CERT_REF LK_REPLICA_CA_CERT_REF LK_ENTRA_CLIENT_AUTH_REF LK_ENTRA_COOKIE_KEY_REF; do
        name="$(hub_env "$hub" "$var")"
        if [ -z "$name" ]; then
            echo "the hub chart does not set $var, which names a Secret Manager secret the hub reads"
            status=1
            continue
        fi
        names+=("$name|$var")
    done
    code="$(hub_code_secrets)" || return 1
    while IFS= read -r entry; do
        name="${entry%%=*}"
        var="${entry#*=}"
        if [ -n "$var" ] && [ -n "$(hub_env "$hub" "$var")" ]; then
            name="$(hub_env "$hub" "$var")"
        else
            var="a constant of the hub's code"
        fi
        names+=("$name|$var")
    done <<<"$code"
    local seen=" "
    for entry in "${names[@]}"; do
        name="${entry%%|*}"
        var="${entry#*|}"
        seen+="$name "
        if ! grep -qxF -- "secret|$name|$project" <<<"$objs"; then
            record="$(grep -E '^secret\|' <<<"$objs" | grep -F -- "|$name|" | head -n1 || true)"
            if [ -n "$record" ]; then
                echo "the hub reads the secret '$name' ($var) from project '$project' (LK_GCP_PROJECT) and the gcp chart makes it in project '$(cut -d'|' -f3 <<<"$record")'"
            else
                echo "the hub reads Secret Manager secret '$name' ($var) and the gcp chart makes no SecretManagerSecret of that name"
            fi
            status=1
        fi
        if ! grep -qxF -- "binding|roles/secretmanager.secretAccessor|SecretManagerSecret|$name|$member" <<<"$objs"; then
            echo "the gcp chart does not give $member roles/secretmanager.secretAccessor on the secret '$name' ($var): the hub could not read it"
            status=1
        fi
    done
    while IFS='|' read -r _ name _; do
        [ -n "$name" ] || continue
        case "$seen" in *" $name "*) ;; *)
            echo "the gcp chart makes the secret '$name', which the hub does not read (its LK_*_REF variables and the constants of its code name none of that name)"
            status=1
            ;;
        esac
    done < <(grep -E '^secret\|' <<<"$objs")

    # The keys: three signing keys named by version, and the vault key named without one. Each has the role for its one use, and a key is for one use.
    local used=" " role refs=" "
    for entry in LK_AGENT_CA_KMS_REF:agent-ca:roles/cloudkms.signerVerifier LK_REPLICA_CA_KMS_REF:replica-ca:roles/cloudkms.signerVerifier \
        LK_AUDIT_CHECKPOINT_KMS_REF:audit-checkpoint-signing:roles/cloudkms.signerVerifier LK_VAULT_KMS_REF:vault:roles/cloudkms.cryptoKeyEncrypterDecrypter; do
        var="${entry%%:*}"
        key_purpose="${entry#*:}"
        role="${key_purpose#*:}"
        key_purpose="${key_purpose%%:*}"
        ref="$(hub_env "$hub" "$var")"
        if [ -z "$ref" ]; then
            echo "the hub chart does not set $var, which names a Cloud KMS key the hub uses"
            status=1
            continue
        fi
        if [ "$var" = LK_VAULT_KMS_REF ]; then
            case "$ref" in */cryptoKeyVersions/*)
                echo "$var names a key version ($ref); the vault key is named without one, so that KMS picks the primary version and a rotation changes nothing the hub is told"
                status=1
                ;;
            esac
        else
            case "$ref" in */cryptoKeyVersions/[0-9]*) ;; *)
                echo "$var names no key version ($ref); the hub signs with one version of the key and the setting is projects/P/locations/L/keyRings/R/cryptoKeys/K/cryptoKeyVersions/N"
                status=1
                ;;
            esac
        fi
        key="${ref%%/cryptoKeyVersions/*}"
        case "$refs" in *" $key "*)
            echo "$var names the same key as another of the hub's keys ($key): each key is used for one thing, and the two CAs must be two keys (S5)"
            status=1
            ;;
        esac
        refs+="$key "
        record="$(grep -E '^key\|' <<<"$objs" | grep -F -- "|$key|" | head -n1 || true)"
        if [ -z "$record" ]; then
            echo "the hub uses Cloud KMS key '$key' ($var) and the gcp chart makes no KMSCryptoKey of that name (project, location, key ring and key)"
            status=1
            continue
        fi
        used+="$key "
        name="$(cut -d'|' -f3 <<<"$record")"
        if [ "$(cut -d'|' -f4 <<<"$record")" != "$key_purpose" ]; then
            echo "$var names '$key', which is the $(cut -d'|' -f4 <<<"$record") key in the gcp chart and not the $key_purpose key: a key is used for one thing"
            status=1
        fi
        if ! grep -qxF -- "binding|$role|KMSCryptoKey|$name|$member" <<<"$objs"; then
            echo "the gcp chart does not give $member $role on the key '$name' ($var): the hub could not use it"
            status=1
        fi
    done
    while IFS='|' read -r _ key name _; do
        [ -n "$key" ] || continue
        case "$used" in *" $key "*) ;; *)
            echo "the gcp chart makes the key '$name', which the hub names in none of LK_AGENT_CA_KMS_REF, LK_REPLICA_CA_KMS_REF, LK_AUDIT_CHECKPOINT_KMS_REF and LK_VAULT_KMS_REF"
            status=1
            ;;
        esac
    done < <(grep -E '^key\|' <<<"$objs")
    return "$status"
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

    # S9: the bucket that takes the audit checkpoints is the one the gcp chart makes (with a retention lock in prod), and that the hub's
    # account may create objects in. A hub that names another bucket cannot write a checkpoint (09/fix, E1).
    local audit_bucket hub_bucket
    audit_bucket="$(doc_field "$gcp" '^    lanekeeper.io/purpose: audit-checkpoint$' name)"
    hub_bucket="$(hub_env "$hub" LK_AUDIT_CHECKPOINT_BUCKET)"
    if [ -z "$audit_bucket" ] || [ "$audit_bucket" != "$hub_bucket" ]; then
        echo "the hub writes its audit checkpoints to bucket '$hub_bucket' (LK_AUDIT_CHECKPOINT_BUCKET) and the gcp chart makes the audit-checkpoint bucket '$audit_bucket'"
        status=1
    fi

    # The Secrets the hub's pods read, and the pods the database admits (09/fix, J2, J3): the migration Job reads the owner's credential from
    # the Secret that the database's bootstrap names, the hub and the seed Job read the application role's from the one its managed role names,
    # and the Jobs' pods carry a label that the database's NetworkPolicy admits on 5432.
    local want got label
    want="$(awk '/^ +initdb:$/ { f = 1 } f && /^ +secret:$/ { s = 1; next } f && s && /^ +name: / { print $2; exit }' "$db")"
    got="$(volume_secret "$hub" migrator-credentials)"
    if [ -z "$got" ] || [ "$got" != "$want" ]; then
        echo "the hub's migration Job reads the migrator's credential from Secret '$got' and the database makes the owner's credential in '$want' (initdb.secret.name)"
        status=1
    fi
    want="$(awk '/^ +passwordSecret:$/ { s = 1; next } s && /^ +name: / { print $2; exit }' "$db")"
    got="$(volume_secret "$hub" db-credentials)"
    if [ -z "$got" ] || [ "$got" != "$want" ]; then
        echo "the hub reads the application role's credential from Secret '$got' and the database's managed role lanekeeper_app has its password in '$want' (passwordSecret)"
        status=1
    fi
    label="$(awk '/^---$/ { job = 0 } /^kind: Job$/ { job = 1 } job && /^ +app.kubernetes.io\/name: / { print $2; exit }' "$hub")"
    if [ -z "$label" ] || ! awk '/^---$/ { np = 0 } /^kind: NetworkPolicy$/ { np = 1 } np' "$db" | grep -qxF "              app.kubernetes.io/name: $label"; then
        echo "the pods of the hub's Jobs are labelled app.kubernetes.io/name '$label' and the database's NetworkPolicy does not admit that label on 5432"
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

    # The secrets and the keys of the hub (09/fix, I1).
    cross_check_hub_names "$gcp" "$hub" || status=1
    return "$status"
}

# cross_check_agent VALUES HUB...: the agent of a swimlane (its values file) and the hubs (rendered output) that it may be pointed at
# agree on the names that the agent verifies (09/fix, P4). The agent dials hub.endpoint and checks the hub's certificate against its host,
# which the hub puts on the agent listener's certificate from LK_AGENT_ENDPOINT_HOSTS; and it asks Google for a token for hub.audience (the
# endpoint when empty), which the hub accepts for Join only if it is LK_HUB_AUDIENCE. A mismatch in either is an agent that cannot join.
cross_check_agent() {
    local values="$1" endpoint audience host hub hosts hub_audience matched=0 status=0
    shift
    endpoint="$(yaml_scalar "$values" endpoint)"
    audience="$(yaml_scalar "$values" audience)"
    host="${endpoint#https://}"
    host="${host%%[:/]*}"
    for hub in "$@"; do
        hosts="$(hub_env "$hub" LK_AGENT_ENDPOINT_HOSTS)"
        tr ',' '\n' <<<"$hosts" | grep -qxF -- "$host" || continue
        matched=1
        hub_audience="$(hub_env "$hub" LK_HUB_AUDIENCE)"
        if [ "${audience:-$endpoint}" != "$hub_audience" ]; then
            echo "$values: the agent asks Google for a token for '${audience:-$endpoint}' (hub.audience, or hub.endpoint when it is empty) and the hub that serves its endpoint accepts '$hub_audience' (LK_HUB_AUDIENCE): every Join would be refused"
            status=1
        fi
    done
    if [ "$matched" = 0 ]; then
        echo "$values: the host '$host' of hub.endpoint is none of the LK_AGENT_ENDPOINT_HOSTS of the hub (staging, prod): the agent verifies the hub's certificate by that name, and the certificate would not carry it"
        status=1
    fi
    return "$status"
}

cross_chart() {
    local env status=0 hubs=() values
    for env in staging prod; do
        [ -f "deploy/values/db/$env.yaml" ] && [ -f "deploy/values/gcp/$env.yaml" ] && [ -f "deploy/values/hub/$env.yaml" ] || continue
        echo "== the db, gcp and hub charts agree ($env)"
        render db "$env" >"$work/x-db-$env.yaml" || return 1
        render gcp-storage "$env" >"$work/x-gcp-$env.yaml" || return 1
        render hub "$env" >"$work/x-hub-$env.yaml" || return 1
        hubs+=("$work/x-hub-$env.yaml")
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
    # The agents of the swimlanes (one values file each) are pointed at one of these hubs.
    for values in deploy/values/agent/*.yaml; do
        [ -f "$values" ] || continue
        echo "== the agent of $(basename "$values" .yaml) and the hub agree on the hub's names"
        cross_check_agent "$values" "${hubs[@]}" || status=1
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
    render hub staging >"$work/x-hub-staging.yaml"
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
    sed -E 's#^( *)name: lanekeeper-db-migrator$#\1name: lanekeeper-db-owner#' "$db" >"$bad"
    expect_cross_failure "an owner's credential in a Secret the migration Job does not read" "$bad" "$gcp" "$hub" "the hub's migration Job reads the migrator's credential from Secret"
    sed -E 's#^( *)name: lanekeeper-db-app$#\1name: lanekeeper-db-application#' "$db" >"$bad"
    expect_cross_failure "an application credential in a Secret the hub does not read" "$bad" "$gcp" "$hub" "the hub reads the application role's credential from Secret"
    sed -E 's#app.kubernetes.io/name: lanekeeper-hub-jobs#app.kubernetes.io/name: lanekeeper-hub-batch#' "$hub" >"$bad"
    expect_cross_failure "Jobs whose label the database does not admit" "$db" "$gcp" "$bad" "the database's NetworkPolicy does not admit that label"
    sed -E '/^    - from:$/{N;N;N;N;N;/lanekeeper-hub-jobs/d}' "$db" >"$bad"
    expect_cross_failure "a database that does not admit the Jobs" "$bad" "$gcp" "$hub" "the database's NetworkPolicy does not admit that label"
    sed -E 's#(value: "example-project-lanekeeper-audit-)prod"#\1prd"#' "$hub" >"$bad"
    expect_cross_failure "audit checkpoints to a bucket that the gcp chart does not make" "$db" "$gcp" "$bad" "the hub writes its audit checkpoints to bucket"
    sed -E 's#^(  name: example-project-lanekeeper-audit-)prod$#\1prd#' "$gcp" >"$bad"
    expect_cross_failure "an audit-checkpoint bucket that the hub does not name" "$db" "$bad" "$hub" "the hub writes its audit checkpoints to bucket"

    # I1: the secrets the hub reads and the keys it signs and wraps with are made by the gcp chart, in the project the hub names, and the hub's
    # account may use each one, for the one thing the key is for.
    local secret_doc=$'apiVersion: secretmanager.cnrm.cloud.google.com/v1beta1\nkind: SecretManagerSecret\nmetadata:\n  name: teams-hook\n  namespace: lanekeeper-gcp\n  annotations:\n    cnrm.cloud.google.com/project-id: "example-project"\nspec:\n  replication: {}'
    local key_doc=$'apiVersion: kms.cnrm.cloud.google.com/v1beta1\nkind: KMSCryptoKey\nmetadata:\n  name: lanekeeper-spare-prod\n  namespace: lanekeeper-gcp\n  labels:\n    lanekeeper.io/purpose: vault\n  annotations:\n    cnrm.cloud.google.com/project-id: "example-project"\nspec:\n  keyRingRef:\n    name: lanekeeper\n  purpose: ENCRYPT_DECRYPT'
    sed -E 's#(value: )"lanekeeper-web-cert"#\1"lanekeeper-web-certificate"#' "$hub" >"$bad"
    expect_cross_failure "the hub reads a web certificate that no secret holds" "$db" "$gcp" "$bad" "makes no SecretManagerSecret of that name"
    sed -E 's#(value: )"lanekeeper-entra-client"#\1"entra-client"#' "$hub" >"$bad"
    expect_cross_failure "the hub reads an Entra client secret that no secret holds" "$db" "$gcp" "$bad" "'entra-client' (LK_ENTRA_CLIENT_AUTH_REF)"
    drop_doc SecretManagerSecret write-preview-key <"$gcp" >"$bad"
    expect_cross_failure "no container for a secret that is a constant of the hub's code" "$db" "$bad" "$hub" "'write-preview-key'"
    drop_doc SecretManagerSecret mcp-token-hmac-key <"$gcp" >"$bad"
    expect_cross_failure "no container for the personal-token key" "$db" "$bad" "$hub" "'mcp-token-hmac-key'"
    drop_doc IAMPolicyMember lanekeeper-hub-read-github-webhook-secret <"$gcp" >"$bad"
    expect_cross_failure "a secret the hub may not read" "$db" "$bad" "$hub" "does not give serviceAccount:lanekeeper-hub@example-project.iam.gserviceaccount.com roles/secretmanager.secretAccessor on the secret 'github-webhook-secret'"
    sed -E '/name: lanekeeper-hub-read-lanekeeper-login-cookie$/,/^---$/s#lanekeeper-hub@#lanekeeper-db@#' "$gcp" >"$bad"
    expect_cross_failure "a secret that only the database may read" "$db" "$bad" "$hub" "the secret 'lanekeeper-login-cookie'"
    sed -E '/^  name: github-read-token$/,/^---$/s#project-id: "example-project"#project-id: "other-project"#' "$gcp" >"$bad"
    expect_cross_failure "a secret made in another project than the hub reads" "$db" "$bad" "$hub" "in project 'other-project'"
    sed -E 's#(value: )"example-project"#\1"other-project"#' "$hub" >"$bad"
    expect_cross_failure "a hub that reads its secrets and keys from another project" "$db" "$gcp" "$bad" "(LK_GCP_PROJECT)"
    append_doc "$gcp" "$secret_doc" >"$bad"
    expect_cross_failure "a container for a secret that nothing reads" "$db" "$bad" "$hub" "makes the secret 'teams-hook', which the hub does not read"
    sed -E 's#(value: ").*(/cryptoKeys/lanekeeper-agent-ca-prod/cryptoKeyVersions/1")#\1projects/example-project/locations/example-location/keyRings/elsewhere\2#' "$hub" >"$bad"
    expect_cross_failure "a CA key in a key ring that the gcp chart does not make" "$db" "$gcp" "$bad" "makes no KMSCryptoKey of that name"
    sed -E 's#cryptoKeys/lanekeeper-audit-checkpoint-prod/#cryptoKeys/lanekeeper-checkpoint-prod/#' "$hub" >"$bad"
    expect_cross_failure "a checkpoint key that the gcp chart does not make" "$db" "$gcp" "$bad" "(LK_AUDIT_CHECKPOINT_KMS_REF)"
    sed -E '/name: LK_REPLICA_CA_KMS_REF/{n;s#lanekeeper-replica-ca-prod#lanekeeper-agent-ca-prod#}' "$hub" >"$bad"
    expect_cross_failure "one key for both CAs" "$db" "$gcp" "$bad" "the same key"
    sed -E '/name: LK_AUDIT_CHECKPOINT_KMS_REF/{n;s#lanekeeper-audit-checkpoint-prod#lanekeeper-replica-ca-prod#}' "$hub" >"$bad"
    expect_cross_failure "a checkpoint key that is the replica CA" "$db" "$gcp" "$bad" "is the replica-ca key in the gcp chart"
    sed -E '/name: LK_VAULT_KMS_REF/{n;s#lanekeeper-vault-prod#lanekeeper-agent-ca-prod#}' "$hub" >"$bad"
    expect_cross_failure "a vault that is the agent CA" "$db" "$gcp" "$bad" "is the agent-ca key in the gcp chart"
    sed -E '/name: LK_VAULT_KMS_REF/{n;s#(lanekeeper-vault-prod)#\1/cryptoKeyVersions/1#}' "$hub" >"$bad"
    expect_cross_failure "a vault key with a version" "$db" "$gcp" "$bad" "LK_VAULT_KMS_REF names a key version"
    sed -E '/name: LK_AGENT_CA_KMS_REF/{n;s#/cryptoKeyVersions/1##}' "$hub" >"$bad"
    expect_cross_failure "a CA key without a version" "$db" "$gcp" "$bad" "LK_AGENT_CA_KMS_REF names no key version"
    drop_doc IAMPolicyMember lanekeeper-hub-sign-replica-ca <"$gcp" >"$bad"
    expect_cross_failure "a replica CA that the hub may not sign with" "$db" "$bad" "$hub" "roles/cloudkms.signerVerifier on the key 'lanekeeper-replica-ca-prod'"
    drop_doc IAMPolicyMember lanekeeper-hub-sign-audit-checkpoint <"$gcp" >"$bad"
    expect_cross_failure "a checkpoint key that the hub may not sign with" "$db" "$bad" "$hub" "roles/cloudkms.signerVerifier on the key 'lanekeeper-audit-checkpoint-prod'"
    drop_doc IAMPolicyMember lanekeeper-hub-vault <"$gcp" >"$bad"
    expect_cross_failure "a vault key that the hub may not use" "$db" "$bad" "$hub" "roles/cloudkms.cryptoKeyEncrypterDecrypter on the key 'lanekeeper-vault-prod'"
    sed -E '/name: lanekeeper-hub-sign-agent-ca$/,/^---$/s#lanekeeper-hub@#lanekeeper-db@#' "$gcp" >"$bad"
    expect_cross_failure "an agent CA that only the database may sign with" "$db" "$bad" "$hub" "roles/cloudkms.signerVerifier on the key 'lanekeeper-agent-ca-prod'"
    append_doc "$gcp" "$key_doc" >"$bad"
    expect_cross_failure "a key that the hub does not name" "$db" "$bad" "$hub" "makes the key 'lanekeeper-spare-prod', which the hub names in none of"
    # A person editing one side only: the secrets of the gcp chart are the hub chart's names, so changing the hub's default is a failure until the gcp values follow.
    sed -E 's#(value: )"lanekeeper-agent-ca-cert"#\1"agent-ca-certificate"#' "$hub" >"$bad"
    expect_cross_failure "a CA certificate secret renamed in the hub chart only" "$db" "$gcp" "$bad" "'agent-ca-certificate' (LK_AGENT_CA_CERT_REF)"

    # P4: the agent and the hub agree on the names the agent verifies.
    local agent=deploy/values/agent/sit1.yaml hubs=("$hub" "$work/x-hub-staging.yaml") out
    cross_check_agent "$agent" "${hubs[@]}" >/dev/null || fail "the unchanged sit1 values and the staging and prod hubs disagree"
    expect_agent_cross_failure() {
        local name="$1" fragment="$2"
        shift 2
        if out="$(cross_check_agent "$bad" "$@" 2>&1)"; then
            fail "$name: the cross-chart check accepted it"
        elif ! grep -qF -- "$fragment" <<<"$out"; then
            fail "$name: refused, but not for '$fragment':"$'\n'"$out"
        else
            echo "  ok  $name"
        fi
    }
    sed -E 's#agents\.lanekeeper-prod\.example\.com#agents.lanekeeper-elsewhere.example.com#' "$agent" >"$bad"
    expect_agent_cross_failure "an agent that dials a host the hub's certificate does not carry" "is none of the LK_AGENT_ENDPOINT_HOSTS" "${hubs[@]}"
    sed -E 's#^( *audience:).*#\1 https://other.example.com#' "$agent" >"$bad"
    expect_agent_cross_failure "an agent that asks Google for another audience" "every Join would be refused" "${hubs[@]}"
    sed -E '/^ *audience:/d' "$agent" >"$bad"
    expect_agent_cross_failure "an agent whose audience is its endpoint, not the hub's name" "every Join would be refused" "${hubs[@]}"
    cp "$agent" "$bad"
    sed -E 's#(value: )"agents\.lanekeeper-prod\.example\.com"#\1"agents.lanekeeper-other.example.com"#' "$hub" >"$work/x-hub-bad.yaml"
    expect_agent_cross_failure "a hub that no longer carries the agent's host" "is none of the LK_AGENT_ENDPOINT_HOSTS" "$work/x-hub-bad.yaml" "$work/x-hub-staging.yaml"
}

planted_kubeconform() {
    echo "== planted: kubeconform refuses a field the schema does not have, and a kind it has no schema for"
    local out status=0
    out="$(kubeconform "${schema_args[@]}" deploy/tests/planted/kubeconform.yaml 2>&1)" || status=$?
    [ "$status" -ne 0 ] || fail "kubeconform accepted deploy/tests/planted/kubeconform.yaml"
    local kind
    for kind in GCPBackendPolicy HealthCheckPolicy FQDNNetworkPolicy ComputeSecurityPolicy Gateway StatefulSet Job Deployment Role RoleBinding NetworkPolicy ConfigMap Cluster ScheduledBackup StorageBucket IAMPolicyMember IAMServiceAccount BinaryAuthorizationPolicy BinaryAuthorizationAttestor ContainerAnalysisNote KMSKeyRing KMSCryptoKey SecretManagerSecret Application AppProject; do
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

    # 09/fix, E1: what the hub requires to start has no default, and the chart names the value that is missing.
    expect_helm_failure "no Google project" "config.gcpProject is required" --set config.gcpProject=
    expect_helm_failure "a Google project that is not one" "gcpProject" --set config.gcpProject=Bad_Project
    expect_helm_failure "no Entra tenant" "config.entra.tenantId is required" --set config.entra.tenantId=
    expect_helm_failure "no Entra client" "config.entra.clientId is required" --set config.entra.clientId=
    expect_helm_failure "an Entra tenant that is not a GUID" "tenantId" --set config.entra.tenantId=not-a-guid
    expect_helm_failure "an Entra client that is not a GUID" "clientId" --set config.entra.clientId=22222222-2222-4222-8222
    expect_helm_failure "no key for the agent CA" "config.kms.agentCa is required" --set config.kms.agentCa=
    expect_helm_failure "no key for the replica CA" "config.kms.replicaCa is required" --set config.kms.replicaCa=
    expect_helm_failure "no key for the audit checkpoints" "config.kms.auditCheckpoint is required" --set config.kms.auditCheckpoint=
    expect_helm_failure "no key for the vault" "config.kms.vault is required" --set config.kms.vault=
    expect_helm_failure "a CA key that is a key and not a version" "agentCa" --set config.kms.agentCa=projects/p/locations/l/keyRings/r/cryptoKeys/k
    expect_helm_failure "a checkpoint key that is a key and not a version" "auditCheckpoint" --set config.kms.auditCheckpoint=projects/p/locations/l/keyRings/r/cryptoKeys/k
    expect_helm_failure "a vault key that is a version" "vault" --set config.kms.vault=projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1
    expect_helm_failure "one CA key for both CAs" "LK_REPLICA_CA_KMS_REF" --set config.kms.agentCa=projects/p/locations/l/keyRings/r/cryptoKeys/ca/cryptoKeyVersions/1 --set config.kms.replicaCa=projects/p/locations/l/keyRings/r/cryptoKeys/ca/cryptoKeyVersions/1
    expect_helm_failure "one CA certificate for both CAs" "LK_REPLICA_CA_CERT_REF" --set config.secretManager.replicaCaCert=lanekeeper-agent-ca-cert
    expect_helm_failure "no bucket for the audit checkpoints" "config.auditCheckpointBucket is required" --set config.auditCheckpointBucket=
    expect_helm_failure "a bucket name that is not one" "auditCheckpointBucket" --set config.auditCheckpointBucket=Bad_Bucket
    expect_helm_failure "no host for the agents" "config.agentEndpointHosts is required" --set-json 'config.agentEndpointHosts=[]'
    expect_helm_failure "an agent host in capitals" "agentEndpointHosts" --set-json 'config.agentEndpointHosts=["Agents.Example.com"]'
    expect_helm_failure "a wildcard agent host" "agentEndpointHosts" --set-json 'config.agentEndpointHosts=["*.example.com"]'
    expect_helm_failure "nine agent hosts" "agentEndpointHosts" --set-json 'config.agentEndpointHosts=["a.example.com","b.example.com","c.example.com","d.example.com","e.example.com","f.example.com","g.example.com","h.example.com","i.example.com"]'
    expect_helm_failure "no base repository URL" "config.git.baseUrl is required" --set config.git.baseUrl=
    expect_helm_failure "no tenant repository URL" "config.git.tenantUrl is required" --set config.git.tenantUrl=
    expect_helm_failure "no base repository name" "config.git.baseRepo is required" --set config.git.baseRepo=
    expect_helm_failure "no tenant repository name" "config.git.tenantRepo is required" --set config.git.tenantRepo=
    expect_helm_failure "a Secret Manager name with a slash" "webCert" --set config.secretManager.webCert=a/b
    expect_helm_failure "a Secret Manager name with a space" "entraClient" --set 'config.secretManager.entraClient=has space'
    expect_helm_failure "a Secret Manager name that does not exist" "apiKey" --set config.secretManager.apiKey=x
    expect_helm_failure "a first administrator that is not a GUID" "bootstrapAdmins" --set-json 'config.bootstrapAdmins=["someone@example.com"]'
    expect_helm_failure "the same first administrator twice" "bootstrapAdmins" --set-json 'config.bootstrapAdmins=["33333333-3333-4333-8333-333333333333","33333333-3333-4333-8333-333333333333"]'

    # 09/fix, E3: the optional settings a deployment needs are tunables, within the ranges the hub accepts; the ones it must not be able to
    # switch on from a values file are not.
    local name
    for name in LK_MCP_NOTIFY_SERVICES LK_RETENTION_ENABLED LK_RETENTION_DRY_RUN LK_EXTRA_DENY_GLOBS LK_TEAMS_ALLOWED_HOSTS LK_GITHUB_API_BASE LK_ENTRA_AUTHORITY_HOST LK_ENTRA_REDIRECT_PATH LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS LK_GIT_TOKEN_FILE; do
        expect_helm_failure "the tunable $name is not the chart's to set" "$name" --set-json "config.tuning={\"$name\":\"true\"}"
    done
    expect_helm_failure "a start timeout of zero" "LK_START_TIMEOUT_SECS" --set-json 'config.tuning={"LK_START_TIMEOUT_SECS":0}'
    expect_helm_failure "a start timeout over the hub's limit" "LK_START_TIMEOUT_SECS" --set-json 'config.tuning={"LK_START_TIMEOUT_SECS":121}'
    expect_helm_failure "a certificate reload of zero" "LK_WEB_CERT_RELOAD_SECS" --set-json 'config.tuning={"LK_WEB_CERT_RELOAD_SECS":0}'
    expect_helm_failure "a certificate reload over a day" "LK_WEB_CERT_RELOAD_SECS" --set-json 'config.tuning={"LK_WEB_CERT_RELOAD_SECS":86401}'
    expect_helm_failure "a reconcile interval of zero" "LK_WRITES_RECONCILE_SECS" --set-json 'config.tuning={"LK_WRITES_RECONCILE_SECS":0}'
    expect_helm_failure "an expiry interval over a day" "LK_WRITES_EXPIRE_SECS" --set-json 'config.tuning={"LK_WRITES_EXPIRE_SECS":86401}'
    expect_helm_failure "a pull-request interval over five minutes" "LK_WRITES_PR_JOBS_SECS" --set-json 'config.tuning={"LK_WRITES_PR_JOBS_SECS":301}'
    expect_helm_failure "a cleanup interval under a minute" "LK_WRITES_CLEANUP_SECS" --set-json 'config.tuning={"LK_WRITES_CLEANUP_SECS":59}'
    expect_helm_failure "a corpus cache under 16 MiB" "LK_CORPUS_CACHE_MIB" --set-json 'config.tuning={"LK_CORPUS_CACHE_MIB":15}'
    expect_helm_failure "a corpus cache over 4 GiB" "LK_CORPUS_CACHE_MIB" --set-json 'config.tuning={"LK_CORPUS_CACHE_MIB":4097}'
    expect_helm_failure "a base branch with a space" "LK_GIT_BASE_BRANCH" --set-json 'config.tuning={"LK_GIT_BASE_BRANCH":"a b"}'
    expect_helm_failure "a base branch that climbs" "LK_GIT_BASE_BRANCH" --set-json 'config.tuning={"LK_GIT_BASE_BRANCH":"a/../b"}'
    expect_helm_failure "a base branch Git would refuse (a .lock ref)" "LK_GIT_BASE_BRANCH" --set-json 'config.tuning={"LK_GIT_BASE_BRANCH":"main.lock"}'
    expect_helm_failure "a config-server that is not a name" "LK_CONFIG_SERVER_DEPLOYMENT" --set-json 'config.tuning={"LK_CONFIG_SERVER_DEPLOYMENT":"Config Server"}'
    expect_helm_failure "MCP in both modes" "LK_MCP_AUTH" --set-json 'config.tuning={"LK_MCP_AUTH":"oauth,personal_token"}'
    expect_helm_failure "an MCP mode that does not exist" "LK_MCP_AUTH" --set-json 'config.tuning={"LK_MCP_AUTH":"basic"}'
    expect_helm_failure "an MCP confirmation that does not exist" "LK_MCP_CONFIRM" --set-json 'config.tuning={"LK_MCP_CONFIRM":"never"}'
    expect_helm_failure "an MCP content mode that does not exist" "LK_MCP_CONTENT" --set-json 'config.tuning={"LK_MCP_CONTENT":"everything"}'
    expect_helm_failure "API audiences with a space" "LK_ENTRA_API_AUDIENCES" --set-json 'config.tuning={"LK_ENTRA_API_AUDIENCES":"api://x, y"}'
    expect_helm_failure "OAuth with audiences that have no application ID URI" "LK_ENTRA_API_AUDIENCES" --set-json 'config.tuning={"LK_ENTRA_API_AUDIENCES":"22222222-2222-4222-8222-222222222222"}'

    # 09/fix, T1: the grace period (45 s, the contract's) must outlast the hub's shutdown by 10 s.
    expect_helm_failure "a drain that outlasts the grace period" "terminationGracePeriodSeconds" --set-json 'config.tuning={"LK_SHUTDOWN_DRAIN_SECS":40}'
    expect_helm_failure "a wait for the load balancer that outlasts it" "terminationGracePeriodSeconds" --set-json 'config.tuning={"LK_SHUTDOWN_GRACE_SECS":31}'
    expect_helm_failure "two waits that together outlast it" "terminationGracePeriodSeconds" --set-json 'config.tuning={"LK_SHUTDOWN_GRACE_SECS":20,"LK_SHUTDOWN_DRAIN_SECS":20}'
    expect_helm_failure "a drain that is not a number" "LK_SHUTDOWN_DRAIN_SECS" --set-json 'config.tuning={"LK_SHUTDOWN_DRAIN_SECS":"long"}'
    expect_helm_failure "a drain over the hub's limit" "LK_SHUTDOWN_DRAIN_SECS" --set-json 'config.tuning={"LK_SHUTDOWN_DRAIN_SECS":121}'

    # 09/fix, J1 to J3, J6, J7: the migration Job and the seed Job.
    expect_helm_failure "a seed with no file" "seed.content or seed.existingConfigMap is required" --set seed.enabled=true --set seed.existingConfigMap=
    expect_helm_failure "a seed with two files" "set only one of seed.content and seed.existingConfigMap" --set seed.enabled=true --set-literal seed.content='version: 1' --set seed.existingConfigMap=my-seed
    expect_helm_failure "a seed ConfigMap that is not a name" "existingConfigMap" --set seed.enabled=true --set seed.existingConfigMap='My Seed'
    # 256 KiB and a byte: over the file the hub reads. It goes in a values file, because an argument that long is refused by the shell.
    printf 'seed:\n  enabled: true\n  existingConfigMap: ""\n  content: "%s"\n' "$(head -c 262145 /dev/zero | tr '\0' 'a')" >"$work/seed-too-long.yaml"
    expect_helm_failure "a seed file over the hub's limit" "content" -f "$work/seed-too-long.yaml"
    expect_helm_failure "a seed revision with a space" "revision" --set seed.revision='a b'
    expect_helm_failure "a migration timeout of zero" "timeoutSeconds" --set migrate.timeoutSeconds=0
    expect_helm_failure "a migration timeout over the hub's limit" "timeoutSeconds" --set migrate.timeoutSeconds=7201
    expect_helm_failure "a seed timeout over the hub's limit" "timeoutSeconds" --set seed.timeoutSeconds=3601
    expect_helm_failure "a migration that tries ten times" "backoffLimit" --set migrate.backoffLimit=10
    expect_helm_failure "a seed that tries ten times" "backoffLimit" --set seed.backoffLimit=10
    expect_helm_failure "the migrator's Secret that is the application role's" "database.migratorSecretName is the same Secret as database.credentialsSecretName" --set database.migratorSecretName=lanekeeper-db-app
    expect_helm_failure "a migrator's Secret that is not a name" "migratorSecretName" --set database.migratorSecretName='Bad Name'
    expect_helm_failure "Jobs without limits" "limits" --set-json 'jobs.resources.limits=null'
    expect_helm_failure "Jobs without requests" "requests" --set-json 'jobs.resources.requests=null'
    expect_helm_failure "a Job value that does not exist" "hook" --set migrate.hook=false
    expect_helm_failure "a seed value that does not exist" "hook" --set seed.hook=true
}

# accept_hub_variant NAME HELM_ARG...: the hub chart (prod values) with other values must still pass every check (kubeconform, the
# policies and the variables of the contract), so that the checks above are not passing only because the chart has one shape. The output is
# left in $work/variant.yaml.
accept_hub_variant() {
    local name="$1"
    shift
    local out="$work/variant.yaml" ok=1
    if ! render hub prod "$@" >"$out" 2>"$work/variant.err"; then
        fail "$name: the chart did not render: $(tail -n1 "$work/variant.err")"
        return 0
    fi
    kubeconform "${schema_args[@]}" "$out" >/dev/null || { fail "$name: kubeconform refused it"; ok=0; }
    check_policy "$out" hub prod >/dev/null 2>&1 || { fail "$name: the policies refused it"; ok=0; }
    check_hub_variables "$out" >/dev/null 2>&1 || { fail "$name: the StatefulSet lacks a variable the hub requires"; ok=0; }
    [ "$ok" = 1 ] && echo "  ok  $name"
    return 0
}

hub_variants() {
    echo "== the hub chart with other values still passes every check"
    accept_hub_variant "the settings of 09/fix, E3"  --set-json 'config.tuning={"LK_START_TIMEOUT_SECS":12,"LK_WEB_CERT_RELOAD_SECS":600,"LK_WRITES_RECONCILE_SECS":45,"LK_WRITES_EXPIRE_SECS":120,"LK_WRITES_PR_JOBS_SECS":10,"LK_WRITES_CLEANUP_SECS":7200,"LK_CORPUS_CACHE_MIB":512,"LK_GIT_BASE_BRANCH":"develop","LK_CONFIG_SERVER_DEPLOYMENT":"csp-configuration-server","LK_ENTRA_API_AUDIENCES":"api://22222222-2222-4222-8222-222222222222,22222222-2222-4222-8222-222222222222"}'
    in_variant '^ +- name: LK_START_TIMEOUT_SECS$' "the settings of E3: the start timeout"
    in_variant '^ +- name: LK_WRITES_CLEANUP_SECS$' "the settings of E3: the cleanup interval"
    in_variant '^ +value: "develop"$' "the settings of E3: the base branch"
    in_variant '^ +value: "api://22222222-2222-4222-8222-222222222222,22222222-2222-4222-8222-222222222222"$' "the settings of E3: the audiences"
    accept_hub_variant "MCP in personal-token mode, with drafts and data blocks" --set-json 'config.tuning={"LK_MCP_AUTH":"personal_token","LK_MCP_CONFIRM":"draft","LK_MCP_CONTENT":"data_block"}'
    in_variant '^ +value: "personal_token"$' "MCP in personal-token mode"
    accept_hub_variant "audiences without an application ID URI, in personal-token mode" --set-json 'config.tuning={"LK_MCP_AUTH":"personal_token","LK_ENTRA_API_AUDIENCES":"22222222-2222-4222-8222-222222222222"}'
    accept_hub_variant "the longest shutdown that the grace period allows" --set-json 'config.tuning={"LK_SHUTDOWN_GRACE_SECS":10,"LK_SHUTDOWN_DRAIN_SECS":25}'
    in_variant '^ +terminationGracePeriodSeconds: 45$' "the longest shutdown that the grace period allows"
    accept_hub_variant "a wait of zero for the load balancer" --set-json 'config.tuning={"LK_SHUTDOWN_GRACE_SECS":0}'
    in_variant '^ +value: "0"$' "a wait of zero for the load balancer"
    accept_hub_variant "two first administrators" --set-json 'config.bootstrapAdmins=["33333333-3333-4333-8333-333333333333","44444444-4444-4444-8444-444444444444"]'
    in_variant '^ +value: "33333333-3333-4333-8333-333333333333,44444444-4444-4444-8444-444444444444"$' "two first administrators"
    accept_hub_variant "two agent hosts" --set-json 'config.agentEndpointHosts=["agents.lanekeeper-prod.example.com","agents2.lanekeeper-prod.example.com"]'
    in_variant '^ +value: "agents.lanekeeper-prod.example.com,agents2.lanekeeper-prod.example.com"$' "two agent hosts"
    accept_hub_variant "other Secret Manager names" --set config.secretManager.webCert=my-web-cert --set config.secretManager.loginCookie=my_login_cookie
    in_variant '^ +value: "my-web-cert"$' "other Secret Manager names"
    accept_hub_variant "numbers of a million or more, and a fraction" --set-json 'config.tuning={"LK_BODY_MAX_BYTES":67108864,"LK_BUCKET_CAPACITY":10000000,"LK_IP_RATE_PER_SEC":0.5}'
    in_variant '^ +value: "67108864"$' "numbers of a million or more are printed whole (Go prints 6.7108864e+07)"
    in_variant '^ +value: "10000000"$' "numbers of a million or more are printed whole (Go prints 1e+07)"
    in_variant '^ +value: "0.5"$' "a fraction"
    # J1, J2, J7: the timeouts the Jobs are told and the deadlines they are given (the timeout and five minutes).
    accept_hub_variant "longer timeouts for the Jobs" --set migrate.timeoutSeconds=1800 --set seed.enabled=true --set seed.existingConfigMap=my-seed --set seed.timeoutSeconds=300
    in_variant '^ +value: "1800"$' "longer timeouts for the Jobs: the migration timeout"
    in_variant '^  activeDeadlineSeconds: 2100$' "longer timeouts for the Jobs: the migration deadline is the timeout and five minutes"
    in_variant '^  activeDeadlineSeconds: 600$' "longer timeouts for the Jobs: the seed deadline is the timeout and five minutes"
    # J7: no seed unless it is on; the file comes from a ConfigMap that exists, or from one the chart makes.
    accept_hub_variant "no seed" --set seed.enabled=false
    ! grep -q 'lanekeeper-hub-seed' "$work/variant.yaml" || fail "no seed: the output has a seed Job or a ConfigMap for it"
    grep -q '^  name: lanekeeper-hub-migrate$' "$work/variant.yaml" || fail "no seed: there is no migration Job"
    accept_hub_variant "a seed from a ConfigMap made out of band" --set seed.enabled=true --set seed.existingConfigMap=my-seed
    in_variant '^ +name: my-seed$' "a seed from a ConfigMap made out of band"
    ! grep -q '^kind: ConfigMap$' "$work/variant.yaml" || fail "a seed from a ConfigMap made out of band: the chart rendered a ConfigMap"
    accept_hub_variant "a seed from a file in the values" --set seed.enabled=true --set seed.existingConfigMap= --set-literal seed.content=$'version: 1\nprojects: []\n'
    in_variant '^kind: ConfigMap$' "a seed from a file in the values"
    local first named
    first="$(seed_job_name "$work/variant.yaml")"
    [[ "$first" =~ ^lanekeeper-hub-seed-[0-9a-f]{10}$ ]] || fail "a seed from a file in the values: the Job is called '$first'"
    # The Job's pod template cannot change, so everything that changes it changes the name: the file, the revision, the image, the ConfigMap.
    # The same inputs, the same name.
    # (Not in a command substitution: a `fail` in a subshell would not be counted.)
    seed_name=""
    seed_name_of() {
        seed_name=""
        if render hub prod --set seed.enabled=true --set seed.existingConfigMap= "$@" >"$work/variant2.yaml" 2>"$work/variant.err"; then
            seed_name="$(seed_job_name "$work/variant2.yaml")"
        else
            fail "a seed with $*: the chart did not render: $(tail -n1 "$work/variant.err")"
        fi
    }
    local file=$'version: 1\nprojects: []\n'
    seed_name_of --set-literal seed.content="$file"
    [ "$seed_name" = "$first" ] || fail "the same seed in two renders has two names ('$seed_name' and '$first')"
    seed_name_of --set-literal seed.content="$file# changed"
    [ -n "$seed_name" ] && [ "$seed_name" != "$first" ] || fail "a changed seed file has the name of the old Job: the sync would try to change a Job's pod template"
    seed_name_of --set seed.revision=2 --set-literal seed.content="$file"
    [ -n "$seed_name" ] && [ "$seed_name" != "$first" ] || fail "a new seed revision has the name of the old Job"
    seed_name_of --set-string seed.revision=2 --set-literal seed.content="$file"
    [ -n "$seed_name" ] && [ "$seed_name" != "$first" ] || fail "a new seed revision (a string) has the name of the old Job"
    seed_name_of --set image.digest=sha256:2222222222222222222222222222222222222222222222222222222222222222 --set-literal seed.content="$file"
    [ -n "$seed_name" ] && [ "$seed_name" != "$first" ] || fail "a new image has the name of the old seed Job"
    seed_name_of --set seed.timeoutSeconds=300 --set-literal seed.content="$file"
    [ -n "$seed_name" ] && [ "$seed_name" != "$first" ] || fail "a new timeout has the name of the old seed Job"
    seed_name_of --set jobs.resources.limits.memory=1Gi --set-literal seed.content="$file"
    [ -n "$seed_name" ] && [ "$seed_name" != "$first" ] || fail "new resources have the name of the old seed Job"
    seed_name_of --set seed.existingConfigMap=my-seed
    named="$seed_name"
    [ -n "$named" ] || fail "a seed from a ConfigMap has no Job"
    seed_name_of --set seed.existingConfigMap=my-other-seed
    [ -n "$seed_name" ] && [ "$seed_name" != "$named" ] || fail "a seed from another ConfigMap has the name of the old Job"
    accept_hub_variant "a collector" --set config.otlpEndpoint=http://otel-collector.monitoring.svc:4317
    in_variant '^ +- name: LK_OTLP_ENDPOINT$' "a collector"
}

planted_all() {
    planted_kubeconform
    planted_templates
    hub_variants
    planted_hardening
    planted_hub_variables
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

# The hub chart alone (09/fix), and nothing else: for working on the chart. It reports every failure and exits non-zero on any.
hub_only() {
    local status=0
    combo_filter='^hub:'
    lint || status=1
    kubeconform_all || status=1
    policy_all || status=1
    contract_all || status=1
    cross_chart || status=1
    planted_templates
    hub_variants
    planted_hardening
    planted_hub_variables
    planted_cross
    if [ "$failures" -ne 0 ]; then
        echo "render.sh: $failures planted case(s) failed" >&2
        status=1
    fi
    [ "$status" -eq 0 ] && echo "render.sh: the hub chart passes every check, and every planted case was refused for its own reason"
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
    contract)
        # Both run and both are reported: the agent's contract failing must not hide the charts disagreeing, as `&&` did.
        status=0
        contract_all || status=1
        cross_chart || status=1
        exit "$status"
        ;;
    planted) planted_all ;;
    argocd) argocd_only ;;
    hub) hub_only ;;
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
