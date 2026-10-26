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
#   hub        the hub chart with deploy/values/hub/kind.yaml and the image digest: three replicas become Ready. A pod's Ready condition
#              IS the answer of /readyz on the operations port (its readinessProbe), as the kubelet sees it; the test does not ask
#              again from outside the pod, because the hub chart's NetworkPolicy admits port 9090 only from the monitoring namespace and
#              Google's ranges, so a request through the API server's pod proxy would be refused by the policy it is meant to test.
#              P11 is asserted once they are Ready: each replica must be Ready within 10 s of its container's start (lib.sh,
#              p11_verdict). The budget is the design scale's and kind's CPUs and disks are not, so a miss is a finding to read
#              (the number includes the startupProbe's period of 2 s and the readinessProbe's of 5 s, statefulset.yaml:96-108, so a miss may be
#              probe overhead: lib.sh and README.md say how much), not a flake to retry. While the hub cannot start (below),
#              the summary says "NOT MEASURED" for P11; it never reports a number it did not measure.
#
# THE HUB DOES NOT BECOME READY TODAY, AND THIS TEST SAYS SO. crates/hub/src/main.rs exits 1 by design until the identity module (03a)
# and the registry (05) are composed into the binary (plan 09 A1; plans/09-deployment-ci.blockers.md). A hub that exits 1 cannot
# answer /readyz, so the hub stage fails, and the output says that the replicas started, read their settings and then refused: the chart
# is not the fault. Nothing here fakes readiness, retries until it passes, or skips the stage; the test is red until a human assigns
# the composition and the hub binary runs. Acceptance 3 stays unticked until then.
#
# Needs: Docker (a daemon with cgroup v2: the pinned node image's kubelet refuses cgroup v1), kind, helm and crane at the versions of the
# Justfile (`just ops-tools-install kind helm crane`; `just kind-e2e` checks them), curl, jq, openssl and python3 (publish.sh). kubectl is the
# node image's own, run with `docker exec`, so the cluster is driven by the version it is.
#
# Environment (for a developer's machine; CI sets none):
#   LK_KIND_CONFIG_FILE  a kind configuration to use instead of the generated one (see --print-kind-config). The cgroup v2 check is then
#                        yours to make.
#   LK_KIND_DB_TIMEOUT, LK_KIND_HUB_TIMEOUT   seconds to wait for the database (default 900) and the hub (default 300).
#   LK_KIND_REGISTRY_WAIT, LK_KIND_NODES_TIMEOUT   seconds to wait for the registry (default 60) and for the four nodes to be Ready (180).
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
REGISTRY_WAIT="${LK_KIND_REGISTRY_WAIT:-60}"
NODES_TIMEOUT="${LK_KIND_NODES_TIMEOUT:-180}"
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

stage() { # NAME FUNCTION
    current_stage="$1"
    stage_started=$SECONDS
    say "== $1"
    "$2"
    record "$1" PASS "$((SECONDS - stage_started))"
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
                echo "== logs $ns/$pod (previous, else current; the last 40 lines of each container)"
                k -n "$ns" logs "$pod" --all-containers --tail=40 --previous 2>&1 || k -n "$ns" logs "$pod" --all-containers --tail=40 2>&1 || true
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
        if [ "$current_stage" = hub ] && [ -z "$p11_measured" ]; then
            record "P11 hub start-up (10 s per replica)" "NOT MEASURED" "-" "the replicas were not all Ready, so no start-up time exists to judge"
        fi
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
stringData: {username: lanekeeper_migrator, password: "$migrator_pw"}
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

# "ready total crashing": pods that are Ready, pods that exist, and pods that are not Ready and have restarted at least twice.
hub_state() {
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o json | jq -r '
        [.items[] | {ready: ((.status.conditions // []) | any(.type == "Ready" and .status == "True")),
                     restarts: ((.status.containerStatuses // []) | map(.restartCount) | add // 0)}]
        | "\(map(select(.ready)) | length) \(length) \(map(select((.restarts >= 2) and (.ready | not))) | length)"'
}

hub_failed() {
    local pod i log refused=0
    echo "---- the hub pods"
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o wide || true
    for i in 0 1 2; do
        pod="lanekeeper-hub-$i"
        echo "---- $pod: state"
        k -n "$NS" get pod "$pod" -o json 2>/dev/null | jq -c '.status.containerStatuses[0] | {restartCount, state, lastState}' || true
        log="$(k -n "$NS" logs "$pod" --previous --tail=20 2>/dev/null || k -n "$NS" logs "$pod" --tail=20 2>/dev/null || true)"
        echo "---- $pod: log"
        echo "$log"
        grep -q 'refusing to start without them' <<<"$log" && refused=$((refused + 1))
    done
    if [ "$refused" -eq 3 ]; then
        cat <<'EOF'
----
THE HUB REFUSES TO START, ON PURPOSE (plan 09 A1). All three replicas started, read their settings and got past telemetry, and then
logged that this build has no identity module (prompt 03a) and no registry (prompt 05) linked in, and exited with code 1
(crates/hub/src/main.rs). A hub that exits 1 cannot answer /readyz. The chart, the database, the image and the cluster are not at fault.
This stays red until a human assigns the composition of the binary; acceptance 3 is not met until then, and this test does not
pretend otherwise.
EOF
        fail_reason="A1: the hub binary exits 1 by design until it is composed (03a and 05); the replicas started and refused"
    else
        fail_reason="the hub did not become Ready, and its logs are not the refusal of A1 (${refused} of 3 replicas): the chart, the database connection or the cluster is at fault"
    fi
    return 1
}

install_hub() {
    local t0=$SECONDS ready total crashing
    helm upgrade --install lanekeeper-hub "$root/deploy/charts/hub" --namespace "$NS" -f "$root/deploy/values/hub/kind.yaml" \
        --set "image.repository=$REGISTRY/lanekeeper/hub" --set "image.digest=$hub_digest" > "$out/helm-hub.log" 2>&1 || {
        tail -n 20 "$out/helm-hub.log" >&2
        fail_reason="helm could not install the hub chart"
        return 1
    }
    while :; do
        read -r ready total crashing <<<"$(hub_state || echo "0 0 0")"
        say "hub: $ready of 3 Ready ($total pods, $crashing restarting), $((SECONDS - t0)) s after the install"
        [ "$ready" = 3 ] && break
        if [ "${crashing:-0}" -gt 0 ]; then hub_failed; return 1; fi
        [ "$((SECONDS - t0))" -lt "$HUB_TIMEOUT" ] || { fail_reason="the hub was not Ready on all 3 replicas in ${HUB_TIMEOUT} s"; hub_failed || true; return 1; }
        sleep 3
    done
    local to_ready="$((SECONDS - t0))"
    # P11, asserted: from each container's start to its Ready condition (the API's timestamps have a resolution of one second).
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o json | jq -r '.items[] | "\(.metadata.name) \(((.status.conditions[] | select(.type == "Ready") | .lastTransitionTime) | fromdateiso8601) - (.status.containerStatuses[0].state.running.startedAt | fromdateiso8601))"' > "$out/time-to-ready.txt" || {
        fail_reason="the start-up times of the replicas could not be read from the pods (P11 was not measured)"
        return 1
    }
    p11_measured=1
    local p11_rc=0
    p11_verdict "$P11_BUDGET_S" < "$out/time-to-ready.txt" > "$out/p11.txt" 2>&1 || p11_rc=$?
    cat "$out/p11.txt"
    say "all 3 replicas Ready ${to_ready} s after the install; the slowest replica took ${P11_WORST:-?} s from its container's start (P11's budget is ${P11_BUDGET_S} s per replica; kind is not the design scale)"
    if [ "$p11_rc" -ne 0 ]; then
        record "P11 hub start-up (${P11_BUDGET_S} s per replica)" FAIL "${P11_WORST:-?}" "slowest replica; $(tr '\n' ';' < "$out/p11.txt")"
        fail_reason="P11: a replica was not Ready within ${P11_BUDGET_S} s of its container's start (the slowest took ${P11_WORST:-?} s): $(grep -E 'OVER|not measured|no replica' "$out/p11.txt" | head -n 3 | tr '\n' ';')"
        return 1
    fi
    record "P11 hub start-up (${P11_BUDGET_S} s per replica)" PASS "$P11_WORST" "slowest of 3 replicas, container start to Ready; install to all Ready ${to_ready} s"
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
