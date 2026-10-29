#!/usr/bin/env bash
# The kind end-to-end test: the hub chart with CloudNativePG, on a cluster of one control plane and three workers (09, acceptance 3;
# D62, S6, S7, P11). It builds nothing: it takes the hub image that images.yml built and scanned, and installs what the charts say.
#
#   deploy/tests/kind/run.sh [--images-dir DIR] [--out DIR] [--keep] [--print-kind-config] [--registry-only]
#
#   --images-dir DIR     where hub.oci.tar and hub.digest are (default target/images; deploy/images/build.sh writes them, and CI
#                        downloads the artifact of the image job there).
#   --out DIR            logs, diagnostics and summary.md (default target/kind-e2e).
#   --keep               leave the cluster and the registry running, for a look with `docker exec`.
#   --print-kind-config  print the kind configuration that the test uses, and stop.
#   --registry-only      the stages `registry` and `hub image` without a cluster: start the registry, wait for it, push the archive, and
#                        stop (no node pulls it). It needs Docker and crane, not kind, helm or cgroup v2, so it runs where a cluster
#                        cannot, and deploy/tests/kind/registry-test.sh runs it against a real registry on every `just verify-09`.
#
# WHAT IT DOES, IN ORDER (each stage is timed, and the first failure stops the run with the stage named):
#   registry   a local registry (the pinned distribution image), published on 127.0.0.1 of the host only, and waited for: it must answer
#              /v2/ within LK_KIND_REGISTRY_WAIT seconds (default 60).
#   cluster    a kind cluster of 1 + 3 nodes from the pinned node image (pins.env). The registry joins the kind network, each node's
#              containerd learns it as a plain-HTTP mirror, and the four nodes must be Ready (kind's --wait waits for the control plane
#              only: the workers were still NotReady 11 s after the first CI run's create returned). The nodes pull the hub image from
#              that registry BY DIGEST, as a cluster pulls from GHCR.
#   hub image  pushes the archive that the image job scanned (deploy/images/publish.sh push: it refuses an archive whose digest is not
#              the recorded one) and makes sure that a node can pull it. The registry is asked again first, because it was last asked
#              before the cluster existed: it is reached on 127.0.0.1:<published port>, never on `localhost` (a runner resolves that to
#              ::1 first), and, if that port stops answering while the container lives, at the container's address on the kind network
#              (NOTE in the output says which). When nothing answers, the stage prints the container's state, ports, addresses and log.
#   operator   CloudNativePG, from the release's manifest, whose sha256 is checked before a byte is applied; the operator image is
#              named by digest.
#   database   the db chart with deploy/values/db/kind.yaml: three instances, TLS 1.3 only, pgvector and pg_trgm. The Secrets are made
#              here, as the chart's README says a person makes them (A7); the passwords exist for this run only.
#   s6         S6 on the running database: a TLS 1.3 connection with the app role works, and `psql sslmode=disable` is REJECTED by
#              pg_hba (hostnossl reject), as is a connection that offers TLS 1.2 at most. Also vector, pg_trgm and the collation.
#   hub        the hub chart with deploy/values/hub/kind.yaml and the image digest, judged by what the PRODUCTION image can do in kind:
#                1. the migration Job (`hub migrate`, a Helm pre-install hook, so `helm upgrade --install` waits for it) completes against the
#                   CloudNativePG database: TLS 1.3 with the server verified against the mounted CA, the migrator's `uri`, all migrations;
#                2. the seed Job (`hub seed`, as the application role, from the synthetic file of kind.yaml) completes;
#                3. each of the 3 replicas reaches ITS OWN REFUSAL AT THE FIRST GOOGLE STEP: it ends with exit code 1 (never 2: its 25 settings
#                   were read and accepted), its log holds the hub's typed start error for the web certificate or a CA, or the start timeout
#                   of that step (kind has no metadata server), and holds no setting error, no database error and no schema error. The
#                   database and schema checks run in parallel with the Google reads and every failure is logged (crates/hub/src/process.rs,
#                   `load`), so a log with only a Google error shows that the database was reached over TLS and the schema is complete.
#              Every other state fails the stage and says what was seen: a refused setting (exit 2, or a line "hub: ..."), a database error, a
#              schema error, a pod that is Ready (the production image cannot be in kind: an unexpected state for a person to read), a log that
#              no known error explains (printed), a replica without a log (nothing is asserted without one), an exit code other than 1. The
#              strings looked for are in ONE block of lib.sh, each a literal of the hub's Rust source, and kind-run.test.mjs fails when one is
#              gone. Every wait is bounded (LK_KIND_JOBS_TIMEOUT, LK_KIND_HUB_TIMEOUT, LK_KIND_LOG_WAIT).
#
# WHAT THIS JOB DOES NOT MEASURE, AND SAYS SO. The production hub cannot become Ready in kind: it reads the web certificate and the CA
# certificates from Secret Manager, signs with Cloud KMS and signs users in with Entra, and kind has none of them. That is by design (prompt
# 18, G23: no fake in a release build, no setting that points the image at a stub). So the summary of every run that reaches the hub stage
# says, in the sentence of lib.sh KIND_NOT_MEASURED: /readyz on 3 replicas and P11 on the pod are NOT MEASURED in kind, acceptance 3 is not
# met by this job. Nothing here fakes readiness, retries until it passes, or skips a step. (Should all 3 replicas ever be Ready, the stage
# fails as unexpected, and measures P11 for whoever reads it: lib.sh p11_verdict, budget P11 = 10 s per replica.)
#
# Needs: Docker (a daemon with cgroup v2: the pinned node image's kubelet refuses cgroup v1), kind, helm and crane at the versions of the
# Justfile (`just ops-tools-install kind helm crane`; `just kind-e2e` checks them), curl, jq, openssl and python3 (publish.sh). kubectl is the
# node image's own, run with `docker exec`, so the cluster is driven by the version it is.
#
# Environment (for a developer's machine; CI sets none):
#   LK_KIND_CONFIG_FILE  a kind configuration to use instead of the generated one (see --print-kind-config). The cgroup v2 check is then
#                        yours to make.
#   LK_KIND_DB_TIMEOUT, LK_KIND_JOBS_TIMEOUT, LK_KIND_HUB_TIMEOUT   seconds to wait for the database (default 900), for the helm install
#                        (which waits for the migration Job) and for each Job (300), and for the 3 replicas to end once or be Ready (300).
#   LK_KIND_POLL_STEP    seconds between two looks at the replicas (default 3; the tests set it lower).
#   LK_KIND_REGISTRY_WAIT, LK_KIND_NODES_TIMEOUT   seconds to wait for the registry (default 60) and for the four nodes to be Ready (180).
#   LK_KIND_LOG_WAIT, LK_KIND_LOG_STEP   seconds to ask for each hub replica's log (default 30) and between two asks (2).
#   LK_WAIT_STEP         seconds between two asks of the registry (default 1; the tests set it lower).
#
# A FAILURE IS NEVER SILENT. A stage that knows why it failed says so (the line `FAILED in the stage ...: reason`). A command that fails
# for no reason that the script handles ends the run through `set -e`, and the ERR trap below names it: the command, its exit code and
# its line, in that same line.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
PATH="$root/target/ops-tools/bin:$PATH"
# shellcheck source=deploy/tests/kind/pins.env
. "$here/pins.env"
# shellcheck source=deploy/images/tools.env
. "$root/deploy/images/tools.env"
# shellcheck source=deploy/tests/kind/lib.sh
. "$here/lib.sh"

CLUSTER="lanekeeper-e2e"
NS="lanekeeper"
# A name with a dot: without one (or a port) an image reference means a Docker Hub namespace, not a registry host.
REGISTRY="kind-registry.test"
HUB_SELECTOR="app.kubernetes.io/name=lanekeeper-hub"
DB_TIMEOUT="${LK_KIND_DB_TIMEOUT:-900}"
HUB_TIMEOUT="${LK_KIND_HUB_TIMEOUT:-300}"
JOBS_TIMEOUT="${LK_KIND_JOBS_TIMEOUT:-300}"
POLL_STEP="${LK_KIND_POLL_STEP:-3}"
# The migration Job is a Helm hook with a fixed name; the seed Job's name is a hash, so it is found by its labels (charts/hub, _helpers.tpl).
MIGRATE_JOB="lanekeeper-hub-migrate"
SEED_SELECTOR="app.kubernetes.io/name=lanekeeper-hub-jobs,app.kubernetes.io/component=seed"
REGISTRY_WAIT="${LK_KIND_REGISTRY_WAIT:-60}"
NODES_TIMEOUT="${LK_KIND_NODES_TIMEOUT:-180}"
# How long a replica's log is asked for (the kubelet may not have it yet, or may have removed the container), and the pause between two asks.
LOG_WAIT="${LK_KIND_LOG_WAIT:-30}"
LOG_STEP="${LK_KIND_LOG_STEP:-2}"
# The Docker network that kind creates for its nodes: the registry joins it, and nodes resolve the registry's name there.
KIND_NETWORK="kind"

die() { echo "kind-e2e: $*" >&2; exit 2; }
say() { echo "kind-e2e: $*"; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed -n '/^#   deploy/,/^# WHAT IT DOES/p' | sed 's/^# \{0,1\}//' | head -n -1 >&2; exit 2; }

images_dir="$root/target/images"
out="$root/target/kind-e2e"
keep=""
print_config=""
registry_only=""
while [ $# -gt 0 ]; do
    case "$1" in
        --images-dir) [ $# -ge 2 ] || usage; images_dir="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || usage; out="$2"; shift 2 ;;
        --keep) keep=1; shift ;;
        --print-kind-config) print_config=1; shift ;;
        --registry-only) registry_only=1; shift ;;
        *) usage ;;
    esac
done

kind_config() {
    cat <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
name: $CLUSTER
# containerd reads /etc/containerd/certs.d/<registry>/hosts.toml, which start_cluster writes for the local registry. The node image's
# own config does not name a config_path.
containerdConfigPatches:
  - |-
    [plugins."io.containerd.grpc.v1.cri".registry]
      config_path = "/etc/containerd/certs.d"
nodes:
  - role: control-plane
    image: $LK_KIND_NODE_IMAGE
  - role: worker
    image: $LK_KIND_NODE_IMAGE
  - role: worker
    image: $LK_KIND_NODE_IMAGE
  - role: worker
    image: $LK_KIND_NODE_IMAGE
EOF
}
if [ -n "$print_config" ]; then
    kind_config
    exit 0
fi

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is not installed; run: just ops-tools-install $1 (curl, jq, openssl and python3 come from the system)"; }
if [ -n "$registry_only" ]; then tools="docker crane curl jq python3"; else tools="docker kind helm crane curl jq openssl python3 sha256sum"; fi
for tool in $tools; do need "$tool"; done
docker info >/dev/null 2>&1 || die "no Docker daemon answers: this test needs one, and is never skipped without it"
if [ -z "$registry_only" ] && [ -z "${LK_KIND_CONFIG_FILE:-}" ] && [ "$(stat -fc %T /sys/fs/cgroup 2>/dev/null || true)" != "cgroup2fs" ]; then
    die "this host uses cgroup v1, and the kubelet of the pinned node image ($LK_KIND_NODE_IMAGE) refuses to run on it. Use a cgroup v2 host (GitHub's ubuntu-latest is one), or give your own configuration in LK_KIND_CONFIG_FILE"
fi
[ -f "$images_dir/hub.oci.tar" ] && [ -f "$images_dir/hub.digest" ] || die "no $images_dir/hub.oci.tar and hub.digest: build the hub image first (deploy/images/build.sh hub), or give --images-dir"

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
export KUBECONFIG="$work/kubeconfig" # kind writes here, never to the developer's own ~/.kube/config
summary="$out/summary.md"
: > "$out/diagnostics.txt"
cp="${CLUSTER}-control-plane"
current_stage="start"
stage_started=$SECONDS
fail_reason=""
registry_port=""
registry_addr=""
hub_digest=""
failed_cmd=""
failed_rc=0
failed_at=""
app_pw=""
p11_measured=""

# kubectl is the node image's, so its version is the cluster's. `ki` passes standard input through (apply -f -).
k() { docker exec "$cp" kubectl --kubeconfig=/etc/kubernetes/admin.conf "$@"; }
ki() { docker exec -i "$cp" kubectl --kubeconfig=/etc/kubernetes/admin.conf "$@"; }

# The ERR trap (set -E: functions and substitutions inherit it) remembers the command that failed, its exit code and its line. It fires only
# for a failure that set -e acts on, so a command that a stage tests (`cmd || { fail_reason=...; return 1; }`) leaves it alone; on_exit
# uses what it remembered when the stage gave no reason of its own.
set -E
trap 'failed_rc=$?; failed_cmd=$BASH_COMMAND; failed_at="${BASH_SOURCE[0]##*/}:${LINENO}"' ERR

record() { # STAGE RESULT SECONDS [NOTE]; a `|` in a note would split the table's cell
    local note="${4:-}"
    note="${note//|/\\|}"
    printf '| %s | %s | %s | %s |\n' "$1" "$2" "$3" "$note" >> "$summary"
}

stage_note="" # what a stage that passed wants to say in its row; the stage sets it
stage() { # NAME FUNCTION
    current_stage="$1"
    stage_started=$SECONDS
    stage_note=""
    say "== $1"
    "$2"
    record "$1" PASS "$((SECONDS - stage_started))" "$stage_note"
}

diagnostics() {
    registry_report "$REGISTRY" "$registry_port" >> "$out/diagnostics.txt" 2>&1 || true
    docker inspect "$cp" >/dev/null 2>&1 || { echo "no cluster to read" >> "$out/diagnostics.txt"; return 0; }
    {
        echo "== nodes"
        k get nodes -o wide
        echo "== pods, all namespaces"
        k get pods -A -o wide
        echo "== events, the last 80"
        k get events -A --sort-by=.lastTimestamp | tail -n 80
        local ns pod
        for ns in cnpg-system "$NS"; do
            for pod in $(k -n "$ns" get pods -o json | jq -r '.items[] | select(((.status.conditions // []) | any(.type == "Ready" and .status == "True")) | not) | .metadata.name'); do
                echo "== describe $ns/$pod"
                k -n "$ns" describe pod "$pod"
                # Both, always: for a container that the kubelet has removed, `logs --previous` exits 0 with a sentence instead of a log (the third CI run),
                # so a fallback to the current container's log after a failure would never run.
                echo "== logs $ns/$pod, previous container (the last 40 lines of each)"
                k -n "$ns" logs "$pod" --all-containers --tail=40 --previous 2>&1 || true
                echo "== logs $ns/$pod, current container (the last 40 lines of each)"
                k -n "$ns" logs "$pod" --all-containers --tail=40 2>&1 || true
            done
        done
    } >> "$out/diagnostics.txt" 2>&1 || true
}

cleanup() {
    if [ -n "$keep" ]; then
        say "--keep: the cluster $CLUSTER and the registry $REGISTRY are left running. Remove them with: kind delete cluster --name $CLUSTER; docker rm -f $REGISTRY"
    else
        kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
        docker rm -f "$REGISTRY" >/dev/null 2>&1 || true
    fi
    rm -rf "$work"
}

on_exit() {
    local status=$?
    trap - EXIT
    if [ "$status" -ne 0 ]; then
        trap - ERR
        if [ -z "$fail_reason" ] && [ -n "$failed_cmd" ]; then
            fail_reason="a command failed with exit $failed_rc at $failed_at, and nothing handled it: $failed_cmd"
        fi
        record "$current_stage" FAIL "$((SECONDS - stage_started))" "$fail_reason"
        say "FAILED in the stage '$current_stage'${fail_reason:+: $fail_reason}" >&2
        case "$current_stage" in
            registry | cluster | "hub image")
                registry_report "$REGISTRY" "$registry_port"
                registry_probe "at the failure" >&2
                ;;
        esac
        say "== the cluster, as it was when the stage failed (all of it is in $out/diagnostics.txt)"
        diagnostics
        grep -i -E "failed to pull|ErrImagePull|ImagePullBackOff|forbidden|403|denied|no such host|BackOff|Unhealthy" "$out/diagnostics.txt" | sort -u | head -n 12 || true
    fi
    # Whenever the hub stage was reached, pass or fail: what the production image cannot show in kind (the sentence is lib.sh's). When P11
    # was measured (all replicas Ready: unexpected), it has its own row and nothing is said to be not measured.
    if [ "$current_stage" = hub ] && [ -z "$p11_measured" ] && [ -f "$summary" ]; then
        record "/readyz on 3 replicas" "NOT MEASURED" "-" "the production image cannot be Ready without Google and Entra (prompt 18, G23), and kind has neither"
        record "P11 hub start-up (${P11_BUDGET_S} s per replica)" "NOT MEASURED" "-" "no replica is Ready, so no start-up time exists to judge"
        record "acceptance 3 (/readyz on 3 replicas)" "NOT MET by this job" "-" "see the sentence below"
        printf '\n**%s.**\n' "$KIND_NOT_MEASURED" >> "$summary"
    fi
    if [ -f "$summary" ]; then
        echo
        cat "$summary"
        if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then cat "$summary" >> "$GITHUB_STEP_SUMMARY"; fi
    fi
    cleanup
    if [ "$status" -eq 0 ]; then say "PASS"; else say "FAIL (exit $status), stage '$current_stage'"; fi
    exit "$status"
}
trap on_exit EXIT

{
    echo "### kind end-to-end: the hub chart with CloudNativePG"
    echo
    echo "| stage | result | seconds | note |"
    echo "|---|---|---|---|"
} > "$summary"

# ---- stage: registry ---------------------------------------------------------------------------------------------------------

# The routes to the registry from this host, one URL per line: the port that Docker published on 127.0.0.1 (always), and the container's
# own address on the kind network (once it has joined it; a Linux host reaches a container's address, Docker Desktop's VM does not).
registry_urls() {
    local ip
    echo "http://127.0.0.1:$registry_port/v2/"
    ip="$(registry_container_ip "$REGISTRY" "$KIND_NETWORK")"
    if [ -n "$ip" ]; then echo "http://$ip:80/v2/"; fi
}

# One line per route: whether it answers right now, and what curl said when it does not. It reports and never fails.
registry_probe() { # LABEL
    local url said
    [ -n "$registry_port" ] || return 0
    while IFS= read -r url; do
        if said="$(curl -fsS --max-time 2 -o /dev/null "$url" 2>&1)"; then
            echo "kind-e2e: the registry route $url answers ($1)"
        else
            echo "kind-e2e: the registry route $url does NOT answer ($1): $(printf '%s' "$said" | tr '\n' ' ')"
        fi
    done < <(registry_urls)
    return 0
}

start_registry() {
    docker rm -f "$REGISTRY" >/dev/null 2>&1 || true
    # Published on the loopback address of the host only. The registry listens on 80 inside its container, where the nodes reach it.
    docker run -d --name "$REGISTRY" -p 127.0.0.1::80 -e REGISTRY_HTTP_ADDR=:80 "$LK_REGISTRY_IMAGE" >/dev/null
    registry_port="$(docker port "$REGISTRY" 80/tcp | sed -n '1s/.*://p')"
    [[ "$registry_port" =~ ^[0-9]+$ ]] || { fail_reason="could not read the published port of the local registry (docker port says: $(docker port "$REGISTRY" 2>&1 | head -n 3 | tr '\n' ';'))"; return 1; }
    wait_for_http "$REGISTRY_WAIT" "http://127.0.0.1:$registry_port/v2/" || {
        fail_reason="the local registry did not answer on 127.0.0.1:$registry_port in ${REGISTRY_WAIT} s (its state, ports, addresses and log follow)"
        return 1
    }
    say "the local registry answers on 127.0.0.1:$registry_port"
}

# ---- stage: cluster ----------------------------------------------------------------------------------------------------------

start_cluster() {
    kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
    local config="$work/kind.yaml"
    if [ -n "${LK_KIND_CONFIG_FILE:-}" ]; then cp "$LK_KIND_CONFIG_FILE" "$config"; else kind_config > "$config"; fi
    say "creating the cluster from $LK_KIND_NODE_IMAGE"
    kind create cluster --name "$CLUSTER" --config "$config" --kubeconfig "$KUBECONFIG" --wait 300s > "$out/kind-create.log" 2>&1 || {
        tail -n 25 "$out/kind-create.log" >&2
        fail_reason="kind could not create the cluster (the log is $out/kind-create.log)"
        return 1
    }
    # The registry joins the nodes' network, and each node's containerd learns that the registry (kind-registry.test) is a plain-HTTP one.
    docker network connect "$KIND_NETWORK" "$REGISTRY"
    registry_probe "after the registry joined the $KIND_NETWORK network"
    local node
    for node in $(kind get nodes --name "$CLUSTER"); do
        docker exec -i "$node" sh -c "mkdir -p /etc/containerd/certs.d/$REGISTRY && cat > /etc/containerd/certs.d/$REGISTRY/hosts.toml" <<EOF
server = "http://$REGISTRY"

[host."http://$REGISTRY"]
  capabilities = ["pull", "resolve"]
EOF
    done
    [ "$(k get nodes --no-headers | wc -l)" -eq 4 ] || { k get nodes -o wide >&2 || true; fail_reason="expected 4 nodes"; return 1; }
    # kind's --wait returned when the control plane was Ready; the workers were NotReady 11 s old (first CI run). Nothing is scheduled, loaded or
    # installed before all four are Ready.
    k wait --for=condition=Ready node --all --timeout="${NODES_TIMEOUT}s" > "$out/nodes-ready.log" 2>&1 || {
        cat "$out/nodes-ready.log" >&2
        k get nodes -o wide >&2 || true
        fail_reason="not all 4 nodes were Ready in ${NODES_TIMEOUT} s (the nodes are above)"
        return 1
    }
    k get nodes -o wide
}

# ---- stage: hub image --------------------------------------------------------------------------------------------------------

# Sets registry_addr (host:port) to the first route at which the registry answers now, asking all of them in turn for up to REGISTRY_WAIT
# seconds. The registry was last asked before the cluster existed, so it is asked again. Prints which route it is when it is not the
# published port.
pick_registry_route() {
    local urls=() url
    while IFS= read -r url; do urls+=("$url"); done < <(registry_urls)
    wait_for_http "$REGISTRY_WAIT" "${urls[@]}" || {
        fail_reason="the local registry answers on none of its routes in ${REGISTRY_WAIT} s (${urls[*]}); its state, ports, addresses and log follow"
        return 1
    }
    registry_addr="${HTTP_ANSWERED#http://}"
    registry_addr="${registry_addr%%/*}"
    if [ "$registry_addr" != "127.0.0.1:$registry_port" ]; then
        say "NOTE: the published port 127.0.0.1:$registry_port does not answer any more; the registry answers at its address on the $KIND_NETWORK network, $registry_addr, and the image is pushed there"
    fi
}

push_hub_image() {
    pick_registry_route || return 1
    local ref
    ref="$("$root/deploy/images/publish.sh" push hub "$registry_addr/lanekeeper/hub" kind --dir "$images_dir")" || {
        fail_reason="publish.sh could not push the hub image to $registry_addr (its message is above)"
        return 1
    }
    hub_digest="${ref#*@}"
    [[ "$hub_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { fail_reason="publish.sh did not print a digest reference"; return 1; }
    [ "$hub_digest" = "$(cat "$images_dir/hub.digest")" ] || { fail_reason="the pushed digest is not the one recorded by build.sh"; return 1; }
    if [ -n "$registry_only" ]; then
        say "--registry-only: the hub image is in the registry at $registry_addr/lanekeeper/hub@$hub_digest; there is no cluster to pull it"
        return 0
    fi
    local worker
    worker="$(kind get nodes --name "$CLUSTER" | grep worker | head -n1)"
    docker exec "$worker" crictl pull "$REGISTRY/lanekeeper/hub@$hub_digest" > "$out/crictl-pull.log" 2>&1 || {
        cat "$out/crictl-pull.log" >&2
        fail_reason="a node cannot pull $REGISTRY/lanekeeper/hub@$hub_digest from the local registry (the registry, or containerd's /etc/containerd/certs.d, is not as run.sh expects)"
        return 1
    }
    say "the hub image is $REGISTRY/lanekeeper/hub@$hub_digest"
}

# ---- stage: operator ---------------------------------------------------------------------------------------------------------

install_operator() {
    local manifest="$work/cnpg.yaml" got base
    curl --fail --silent --show-error --location --retry 3 --max-time 300 --output "$manifest" "$LK_CNPG_MANIFEST_URL"
    got="$(sha256sum "$manifest" | cut -d' ' -f1)"
    [ "$got" = "$LK_CNPG_MANIFEST_SHA256" ] || { fail_reason="the CloudNativePG manifest has sha256 $got, the pin is $LK_CNPG_MANIFEST_SHA256"; return 1; }
    # The release names its image by tag. The digest of pins.env is added, so the operator that runs is the one that was read.
    base="${LK_CNPG_OPERATOR_IMAGE%@*}"
    sed -i "s#${base}\$#${LK_CNPG_OPERATOR_IMAGE}#" "$manifest"
    [ "$(grep -c -F "$LK_CNPG_OPERATOR_IMAGE" "$manifest")" -eq 2 ] || { fail_reason="the manifest does not name the operator image twice, as pins.env expects"; return 1; }
    ki apply --server-side -f - < "$manifest" > "$out/cnpg-apply.log" 2>&1 || { tail -n 20 "$out/cnpg-apply.log" >&2; fail_reason="the CloudNativePG manifest was not applied"; return 1; }
    k -n cnpg-system rollout status deployment/cnpg-controller-manager --timeout=300s || {
        fail_reason="the CloudNativePG operator did not become available in 300 s"
        return 1
    }
}

# ---- stage: database ---------------------------------------------------------------------------------------------------------

install_database() {
    local migrator_pw
    app_pw="$(openssl rand -hex 32)"
    migrator_pw="$(openssl rand -hex 32)"
    k create namespace "$NS"
    # The two Secrets of the db chart's README (plan 09 A7). They go to the API server on standard input, never on a command line.
    ki apply -f - <<EOF
apiVersion: v1
kind: Secret
metadata: {name: lanekeeper-db-migrator, namespace: $NS}
type: kubernetes.io/basic-auth
stringData:
  username: lanekeeper_migrator
  password: "$migrator_pw"
  uri: "postgresql://lanekeeper_migrator:$migrator_pw@lanekeeper-db-rw:5432/lanekeeper"
---
apiVersion: v1
kind: Secret
metadata: {name: lanekeeper-db-app, namespace: $NS}
type: kubernetes.io/basic-auth
stringData:
  username: lanekeeper_app
  password: "$app_pw"
  uri: "postgresql://lanekeeper_app:$app_pw@lanekeeper-db-rw:5432/lanekeeper"
EOF
    helm upgrade --install lanekeeper-db "$root/deploy/charts/db" --namespace "$NS" -f "$root/deploy/values/db/kind.yaml" > "$out/helm-db.log" 2>&1 || {
        tail -n 20 "$out/helm-db.log" >&2
        fail_reason="helm could not install the db chart"
        return 1
    }
    k -n "$NS" wait --for=condition=Ready cluster.postgresql.cnpg.io/lanekeeper-db --timeout="${DB_TIMEOUT}s" || {
        fail_reason="the database cluster did not become Ready in ${DB_TIMEOUT} s (an image that cannot be pulled, or an extension the image lacks, shows in the diagnostics)"
        return 1
    }
    k -n "$NS" get cluster.postgresql.cnpg.io/lanekeeper-db -o wide
}

# psql in the first database pod, as the app role, over the service. The password goes in on standard input. The answer is on standard
# output; what psql and kubectl said on standard error is in $psql_err, so that a warning of kubectl never changes an answer.
psql_err="$work/psql.err"
psql_app() { # CONNECTION-PARAMS QUERY
    printf '%s\n' "$app_pw" | docker exec -i "$cp" kubectl --kubeconfig=/etc/kubernetes/admin.conf -n "$NS" exec -i lanekeeper-db-1 -c postgres -- \
        sh -c 'IFS= read -r PGPASSWORD; export PGPASSWORD; exec psql -X -A -t -v ON_ERROR_STOP=1 "$0" -c "$1"' \
        "host=lanekeeper-db-rw.$NS.svc user=lanekeeper_app dbname=lanekeeper connect_timeout=10 $1" "$2" 2> "$psql_err"
}

check_s6() {
    local got
    # The positive control first: with TLS and the password the connection works, on TLS 1.3 (ssl_min_protocol_version, S6).
    got="$(psql_app "sslmode=require" "SELECT version FROM pg_stat_ssl WHERE pid = pg_backend_pid()")" || { cat "$psql_err" >&2; fail_reason="the app role cannot connect over TLS: the refusals below would prove nothing"; return 1; }
    [ "$got" = "TLSv1.3" ] || { fail_reason="the connection used '$got', not TLSv1.3 (S6)"; return 1; }
    say "S6: a TLS connection works, and it is $got"
    # The refusals. Each must fail, and for the reason that S6 names, not for a wrong password or a wrong host (the control passed).
    if psql_app "sslmode=disable" "SELECT 1" >/dev/null; then
        fail_reason="S6 VIOLATED: psql with sslmode=disable connected to the database"
        return 1
    fi
    grep -q -E 'pg_hba\.conf rejects connection .*no encryption' "$psql_err" || { cat "$psql_err" >&2; fail_reason="sslmode=disable failed, but not because pg_hba rejects an unencrypted connection (S6)"; return 1; }
    say "S6: psql sslmode=disable is rejected by pg_hba (hostnossl reject): $(head -n1 "$psql_err")"
    if psql_app "sslmode=require ssl_max_protocol_version=TLSv1.2" "SELECT 1" >/dev/null; then
        fail_reason="S6 VIOLATED: a client that offers TLS 1.2 at most connected to the database"
        return 1
    fi
    grep -q -i -E 'protocol version|tlsv1 alert|handshake|unsupported' "$psql_err" || { cat "$psql_err" >&2; fail_reason="the TLS 1.2 connection failed, but not because of the protocol version"; return 1; }
    say "S6: a client that offers TLS 1.2 at most is refused: $(head -n1 "$psql_err")"
    # The extensions and the collation of the database (A26, D124).
    got="$(psql_app "sslmode=require" "SELECT string_agg(extname, ',' ORDER BY extname) FROM pg_extension WHERE extname IN ('vector', 'pg_trgm')")" || { cat "$psql_err" >&2; fail_reason="the extensions could not be read"; return 1; }
    [ "$got" = "pg_trgm,vector" ] || { fail_reason="the extensions are '$got', expected pg_trgm,vector"; return 1; }
    got="$(psql_app "sslmode=require" "SELECT '[1,2,3]'::vector(3)")" || { cat "$psql_err" >&2; fail_reason="the vector type does not work"; return 1; }
    [ "$got" = "[1,2,3]" ] || { fail_reason="the vector type answered '$got'"; return 1; }
    got="$(psql_app "sslmode=require" "SELECT datcollate FROM pg_database WHERE datname = 'lanekeeper'")" || { cat "$psql_err" >&2; fail_reason="the collation could not be read"; return 1; }
    [ "$got" = "en_US.utf8" ] || { fail_reason="the collation is '$got', expected en_US.utf8 (D124)"; return 1; }
    say "pgvector and pg_trgm are installed, the vector type works, the collation is en_US.utf8"
}

# ---- stage: hub --------------------------------------------------------------------------------------------------------------

# "ready total settled": pods that are Ready, pods that exist, and pods that are Ready or have ended at least once. A container that has ended has a
# log to read and an exit code to judge; the kubelet restarts it after a back-off, and the next end is the same one.
hub_state() {
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o json | jq -r '
        [.items[] | ((.status.containerStatuses // [])[0] // {}) as $c
                  | {ready: ((.status.conditions // []) | any(.type == "Ready" and .status == "True")),
                     ended: (($c.state.terminated // $c.lastState.terminated) != null)}]
        | "\(map(select(.ready)) | length) \(length) \(map(select(.ready or .ended)) | length)"'
}

# What a person needs when a Job has not done its work, on standard error: its state, and the last lines of the log of each of its pods. It never fails.
job_report() { # JOB
    {
        echo "---- the Job $1"
        k -n "$NS" get job "$1" -o wide
        k -n "$NS" describe job "$1" | tail -n 30
        echo "---- the log of the Job $1 (the last 40 lines of each of its pods)"
        k -n "$NS" logs -l "job-name=$1" --all-containers --tail=40 --prefix
    } >&2 || true
}

# The Job must be complete within JOBS_TIMEOUT seconds (a bounded `kubectl wait`), and have succeeded once. On failure: its state and its log.
wait_for_job() { # JOB WHAT
    local job="$1" what="$2" succeeded
    if ! k -n "$NS" wait --for=condition=complete "job/$job" --timeout="${JOBS_TIMEOUT}s" > "$out/wait-$job.log" 2>&1; then
        cat "$out/wait-$job.log" >&2
        job_report "$job"
        fail_reason="$what ($job) did not complete in ${JOBS_TIMEOUT} s (its state and its log are above)"
        return 1
    fi
    succeeded="$(k -n "$NS" get job "$job" -o jsonpath='{.status.succeeded}')"
    if [ "$succeeded" != 1 ]; then
        job_report "$job"
        fail_reason="$what ($job) is complete but .status.succeeded is '$succeeded', not 1"
        return 1
    fi
    say "$what ($job) completed; the last lines of its log:"
    k -n "$NS" logs -l "job-name=$job" --all-containers --tail=10 || true
}

# Waits, within HUB_TIMEOUT seconds, until every replica is Ready or has ended once. Then it returns, whatever the state: the replicas are judged as
# they are, and one that never ended has no log and fails the stage by that.
await_replicas() {
    local t0=$SECONDS ready total settled
    while :; do
        read -r ready total settled <<<"$(hub_state || echo "0 0 0")"
        say "hub: $total pods, $ready Ready, $settled of 3 Ready or ended once, $((SECONDS - t0)) s after the install"
        if [ "$total" = 3 ] && [ "$settled" = 3 ]; then return 0; fi
        if [ "$((SECONDS - t0))" -ge "$HUB_TIMEOUT" ]; then
            say "hub: ${HUB_TIMEOUT} s have passed; the replicas are judged as they are"
            return 0
        fi
        sleep "$POLL_STEP"
    done
}

# One replica, as far as it can be known: its container state, and its log, retried for up to LOG_WAIT seconds. The log is that of the current
# container and that of the previous one (both usable ones, previous first: an error in either counts; lib.sh hub_log_usable), and the wait ends
# once there is a log and the container has ended (a log of a running container may be unfinished, and an exit code needs an end). A Ready replica
# needs no log. Sets R_LOG (empty when none could be retrieved), R_STATE (the container status as JSON), R_SUMMARY (what the verdict reads, as
# one line), R_TERMINATED, R_EXIT and R_READY.
collect_replica() { # POD
    local pod="$1" deadline=$((SECONDS + LOG_WAIT)) prev out args current previous
    R_LOG=""
    while :; do
        R_STATE="$(k -n "$NS" get pod "$pod" -o json 2>/dev/null | jq -c '.status.containerStatuses[0] // {}' 2>/dev/null || true)"
        [ -n "$R_STATE" ] || R_STATE="{}"
        R_SUMMARY="$(jq -c '{restartCount: (.restartCount // null), ready: (.ready // false), waiting: (.state.waiting.reason // null), exitCode: (.state.terminated.exitCode // .lastState.terminated.exitCode // null), startedAt: (.state.terminated.startedAt // .lastState.terminated.startedAt // .state.running.startedAt // null), finishedAt: (.state.terminated.finishedAt // .lastState.terminated.finishedAt // null)}' <<<"$R_STATE" 2>/dev/null || echo '{}')"
        R_TERMINATED="$(jq -r '((.state.terminated // .lastState.terminated) != null)' <<<"$R_STATE" 2>/dev/null || echo false)"
        R_EXIT="$(jq -r '(.state.terminated.exitCode // .lastState.terminated.exitCode // "")' <<<"$R_STATE" 2>/dev/null || true)"
        R_READY="$(jq -r '(.ready // false)' <<<"$R_STATE" 2>/dev/null || echo false)"
        if [ "$R_READY" = true ]; then return 0; fi
        current=""
        previous=""
        for prev in "" "--previous"; do
            args=(-n "$NS" logs "$pod" -c hub --tail=40)
            [ -z "$prev" ] || args+=("$prev")
            if out="$(k "${args[@]}" 2>&1)" && hub_log_usable "$out"; then
                if [ -z "$prev" ]; then current="$out"; else previous="$out"; fi
            fi
        done
        R_LOG="$previous${previous:+${current:+$'\n'}}$current"
        if [ -n "$R_LOG" ] && [ "$R_TERMINATED" = true ]; then return 0; fi
        [ "$SECONDS" -lt "$deadline" ] || return 0
        sleep "$LOG_STEP"
    done
}

# P11, measured for whoever reads an unexpected state: from each container's start to its Ready condition (the API's timestamps have a resolution
# of one second). Only called when all three replicas are Ready, which the production image cannot be in kind.
measure_p11() {
    local p11_rc=0
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o json | jq -r '.items[] | "\(.metadata.name) \(((.status.conditions[] | select(.type == "Ready") | .lastTransitionTime) | fromdateiso8601) - (.status.containerStatuses[0].state.running.startedAt | fromdateiso8601))"' > "$out/time-to-ready.txt" || {
        say "the start-up times of the replicas could not be read from the pods (P11 was not measured)"
        return 0
    }
    p11_measured=1
    p11_verdict "$P11_BUDGET_S" < "$out/time-to-ready.txt" > "$out/p11.txt" 2>&1 || p11_rc=$?
    cat "$out/p11.txt"
    if [ "$p11_rc" -ne 0 ]; then
        record "P11 hub start-up (${P11_BUDGET_S} s per replica)" FAIL "${P11_WORST:-?}" "slowest replica; $(tr '\n' ';' < "$out/p11.txt")"
    else
        record "P11 hub start-up (${P11_BUDGET_S} s per replica)" PASS "$P11_WORST" "slowest of 3 replicas, container start to Ready"
    fi
}

# Prints what each replica shows and judges the three (lib.sh hub_replica_class and hub_verdict). Returns 0 only when all three stopped at their first
# Google step; otherwise it sets fail_reason from the most serious class and returns 1. It says what a log or an exit code shows and no more: it
# never says that the chart, the database or the cluster is at fault for a replica whose log does not.
judge_replicas() {
    local pod i class verdict at=0 rc=0 line ready_count=0
    local -a classes=() pods=() codes=() states=() logs=()
    echo "---- the hub pods"
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o wide || true
    for i in 0 1 2; do
        pod="lanekeeper-hub-$i"
        collect_replica "$pod"
        class="$(hub_replica_class "$R_LOG" "$R_TERMINATED" "$R_EXIT" "$R_READY")"
        classes+=("$class")
        pods+=("$pod")
        codes+=("$R_EXIT")
        states+=("$R_SUMMARY")
        logs+=("$R_LOG")
        if [ "$class" = ready ]; then ready_count=$((ready_count + 1)); fi
        echo "---- $pod: $class; state (restartCount, ready, waiting reason, exit code, started, finished): $R_SUMMARY"
        if hub_log_usable "$R_LOG"; then printf '%s\n' "$R_LOG" | tail -n 20; elif [ "$R_READY" != true ]; then echo "(no log could be retrieved in ${LOG_WAIT} s)"; fi
    done
    verdict="$(hub_verdict "${classes[@]}")" || rc=$?
    for i in 0 1 2; do
        if [ "${classes[$i]}" = "$verdict" ]; then at=$i; break; fi
    done
    pod="${pods[$at]}"
    echo "----"
    case "$verdict" in
        google)
            say "all 3 replicas read their settings, ended with exit code 1 and stopped at their first Google step; no log holds a setting, database or schema error"
            stage_note="the migration and seed Jobs completed; all 3 replicas stopped at their first Google step (exit 1) with no setting, database or schema error in their logs"
            ;;
        setting)
            line="$(hub_first_line "${logs[$at]}" "$HUB_SETTING_PREFIX" || true)"
            fail_reason="a setting or the command line was refused by $pod, or it stopped before its start (exit code ${codes[$at]:-unknown}): ${line:-no log could be retrieved}"
            ;;
        schema)
            line="$(hub_first_line "${logs[$at]}" "${HUB_SCHEMA_MARKERS[@]}" || true)"
            fail_reason="the schema check failed on $pod although the migration Job completed: $line"
            ;;
        database)
            line="$(hub_first_line "${logs[$at]}" "${HUB_DATABASE_MARKERS[@]}" || true)"
            fail_reason="the database check failed on $pod: $line"
            ;;
        ready)
            if [ "$ready_count" -eq 3 ]; then measure_p11; fi
            fail_reason="UNEXPECTED: $pod is Ready. The production image cannot become Ready in kind (it needs real Google and Entra), so this is a state for a person to read: either the image or this environment is no longer what this stage was written for"
            ;;
        wrong-exit)
            if [ -n "${codes[$at]}" ]; then line="exit code ${codes[$at]}"; else line="no end with an exit code yet"; fi
            fail_reason="$pod shows a Google refusal in its log, but its last end was $line (expected 1)"
            ;;
        unknown)
            echo "---- the log of $pod, which holds none of the errors this job knows (all that was retrieved):"
            printf '%s\n' "${logs[$at]}"
            fail_reason="$pod stopped with no error that this job knows (exit code ${codes[$at]:-unknown}); its log is above and is the failure to read"
            ;;
        *)
            fail_reason="no log could be retrieved for $pod in ${LOG_WAIT} s (state: ${states[$at]}); nothing is asserted about a replica without a log"
            ;;
    esac
    return "$rc"
}

install_hub() {
    local seeds n
    local -a seed_jobs=()
    # The migration Job is a pre-install hook: helm waits for it (within --timeout), and fails when it fails. The seed Job is not a hook.
    helm upgrade --install lanekeeper-hub "$root/deploy/charts/hub" --namespace "$NS" -f "$root/deploy/values/hub/kind.yaml" \
        --set "image.repository=$REGISTRY/lanekeeper/hub" --set "image.digest=$hub_digest" --timeout "${JOBS_TIMEOUT}s" > "$out/helm-hub.log" 2>&1 || {
        tail -n 20 "$out/helm-hub.log" >&2
        job_report "$MIGRATE_JOB"
        fail_reason="helm could not install the hub chart (the migration Job is a pre-install hook, so a Job that failed or did not finish in ${JOBS_TIMEOUT} s ends here; the helm output and the Job's log are above)"
        return 1
    }
    wait_for_job "$MIGRATE_JOB" "the migration Job" || return 1
    seeds="$(k -n "$NS" get jobs -l "$SEED_SELECTOR" -o jsonpath='{.items[*].metadata.name}')"
    read -r -a seed_jobs <<<"$seeds"
    n="${#seed_jobs[@]}"
    if [ "$n" -ne 1 ]; then
        fail_reason="expected one seed Job (labels $SEED_SELECTOR; kind.yaml sets seed.enabled), found $([ "$n" -eq 0 ] && echo none || echo "$n: $seeds")"
        return 1
    fi
    wait_for_job "${seed_jobs[0]}" "the seed Job" || return 1
    await_replicas
    judge_replicas
}

# ---- run ---------------------------------------------------------------------------------------------------------------------

if [ -n "$registry_only" ]; then
    say "--registry-only: no cluster; the registry, and the push of the archive"
    stage "registry" start_registry
    stage "hub image" push_hub_image
    exit 0
fi
say "pins: node image $LK_KIND_NODE_IMAGE; CloudNativePG $LK_CNPG_VERSION; kind $(kind version | awk '{print $2}'); helm $(helm version --short)"
stage "registry" start_registry
stage "cluster" start_cluster
stage "hub image" push_hub_image
stage "operator" install_operator
stage "database" install_database
stage "s6" check_s6
stage "hub" install_hub
