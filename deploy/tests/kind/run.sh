#!/usr/bin/env bash
# The kind end-to-end test: the hub chart with CloudNativePG, on a cluster of one control plane and three workers (09, acceptance 3;
# D62, S6, S7, P11). It builds nothing: it takes the hub image that images.yml built and scanned, and installs what the charts say.
#
#   deploy/tests/kind/run.sh [--images-dir DIR] [--out DIR] [--keep] [--print-kind-config]
#
#   --images-dir DIR     where hub.oci.tar and hub.digest are (default target/images; deploy/images/build.sh writes them, and CI
#                        downloads the artifact of the image job there).
#   --out DIR            logs, diagnostics and summary.md (default target/kind-e2e).
#   --keep               leave the cluster and the registry running, for a look with `docker exec`.
#   --print-kind-config  print the kind configuration that the test uses, and stop.
#
# WHAT IT DOES, IN ORDER (each stage is timed, and the first failure stops the run with the stage named):
#   cluster    a local registry (the pinned distribution image) and a kind cluster of 1 + 3 nodes from the pinned node image
#              (pins.env). The nodes pull the hub image from that registry BY DIGEST, as a cluster pulls from GHCR.
#   hub image  pushes the archive that the image job scanned (deploy/images/publish.sh push: it refuses an archive whose digest is not
#              the recorded one) and makes sure that a node can pull it.
#   operator   CloudNativePG, from the release's manifest, whose sha256 is checked before a byte is applied; the operator image is
#              named by digest.
#   database   the db chart with deploy/values/db/kind.yaml: three instances, TLS 1.3 only, pgvector and pg_trgm. The Secrets are made
#              here, as the chart's README says a person makes them (A7); the passwords exist for this run only.
#   s6         S6 on the running database: a TLS 1.3 connection with the app role works, and `psql sslmode=disable` is REJECTED by
#              pg_hba (hostnossl reject), as is a connection that offers TLS 1.2 at most. Also vector, pg_trgm and the collation.
#   hub        the hub chart with deploy/values/hub/kind.yaml and the image digest: three replicas become Ready, and /readyz answers
#              on each of them (through the API server's pod proxy, so it is the real route to the operations port). Time to Ready is
#              measured for P11 (informational: kind's CPUs and disks are not the design scale).
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
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
PATH="$root/target/ops-tools/bin:$PATH"
# shellcheck source=deploy/tests/kind/pins.env
. "$here/pins.env"
# shellcheck source=deploy/images/tools.env
. "$root/deploy/images/tools.env"

CLUSTER="lanekeeper-e2e"
NS="lanekeeper"
# A name with a dot: without one (or a port) an image reference means a Docker Hub namespace, not a registry host.
REGISTRY="kind-registry.test"
HUB_SELECTOR="app.kubernetes.io/name=lanekeeper-hub"
DB_TIMEOUT="${LK_KIND_DB_TIMEOUT:-900}"
HUB_TIMEOUT="${LK_KIND_HUB_TIMEOUT:-300}"

die() { echo "kind-e2e: $*" >&2; exit 2; }
say() { echo "kind-e2e: $*"; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed -n '/^#   deploy/,/^# WHAT IT DOES/p' | sed 's/^# \{0,1\}//' | head -n -1 >&2; exit 2; }

images_dir="$root/target/images"
out="$root/target/kind-e2e"
keep=""
print_config=""
while [ $# -gt 0 ]; do
    case "$1" in
        --images-dir) [ $# -ge 2 ] || usage; images_dir="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || usage; out="$2"; shift 2 ;;
        --keep) keep=1; shift ;;
        --print-kind-config) print_config=1; shift ;;
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
for tool in docker kind helm crane curl jq openssl python3 sha256sum; do need "$tool"; done
docker info >/dev/null 2>&1 || die "no Docker daemon answers: this test needs one, and is never skipped without it"
if [ -z "${LK_KIND_CONFIG_FILE:-}" ] && [ "$(stat -fc %T /sys/fs/cgroup 2>/dev/null || true)" != "cgroup2fs" ]; then
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
hub_digest=""
app_pw=""

# kubectl is the node image's, so its version is the cluster's. `ki` passes standard input through (apply -f -).
k() { docker exec "$cp" kubectl --kubeconfig=/etc/kubernetes/admin.conf "$@"; }
ki() { docker exec -i "$cp" kubectl --kubeconfig=/etc/kubernetes/admin.conf "$@"; }

record() { # STAGE RESULT SECONDS [NOTE]
    printf '| %s | %s | %s | %s |\n' "$1" "$2" "$3" "${4:-}" >> "$summary"
}

stage() { # NAME FUNCTION
    current_stage="$1"
    stage_started=$SECONDS
    say "== $1"
    "$2"
    record "$1" PASS "$((SECONDS - stage_started))"
}

diagnostics() {
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
        record "$current_stage" FAIL "$((SECONDS - stage_started))" "$fail_reason"
        say "FAILED in the stage '$current_stage'${fail_reason:+: $fail_reason}"
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

# ---- stage: cluster ----------------------------------------------------------------------------------------------------------

start_cluster() {
    kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
    docker rm -f "$REGISTRY" >/dev/null 2>&1 || true
    docker run -d --name "$REGISTRY" -p 127.0.0.1::80 -e REGISTRY_HTTP_ADDR=:80 "$LK_REGISTRY_IMAGE" >/dev/null
    registry_port="$(docker port "$REGISTRY" 80/tcp | head -n1 | sed 's/.*://')"
    [[ "$registry_port" =~ ^[0-9]+$ ]] || die "could not read the port of the local registry"
    local i
    for i in $(seq 1 30); do
        curl -fsS --max-time 2 "http://127.0.0.1:$registry_port/v2/" >/dev/null 2>&1 && break
        [ "$i" -lt 30 ] || die "the local registry does not answer on 127.0.0.1:$registry_port"
        sleep 1
    done
    local config="$work/kind.yaml"
    if [ -n "${LK_KIND_CONFIG_FILE:-}" ]; then cp "$LK_KIND_CONFIG_FILE" "$config"; else kind_config > "$config"; fi
    say "creating the cluster from $LK_KIND_NODE_IMAGE"
    kind create cluster --name "$CLUSTER" --config "$config" --kubeconfig "$KUBECONFIG" --wait 300s > "$out/kind-create.log" 2>&1 || {
        tail -n 25 "$out/kind-create.log" >&2
        fail_reason="kind could not create the cluster (the log is $out/kind-create.log)"
        return 1
    }
    # The registry joins the nodes' network, and each node's containerd learns that the registry (kind-registry.test) is a plain-HTTP one.
    docker network connect kind "$REGISTRY"
    local node
    for node in $(kind get nodes --name "$CLUSTER"); do
        docker exec -i "$node" sh -c "mkdir -p /etc/containerd/certs.d/$REGISTRY && cat > /etc/containerd/certs.d/$REGISTRY/hosts.toml" <<EOF
server = "http://$REGISTRY"

[host."http://$REGISTRY"]
  capabilities = ["pull", "resolve"]
EOF
    done
    k get nodes -o wide
    [ "$(k get nodes --no-headers | wc -l)" -eq 4 ] || { fail_reason="expected 4 nodes"; return 1; }
}

# ---- stage: hub image --------------------------------------------------------------------------------------------------------

push_hub_image() {
    local ref
    ref="$("$root/deploy/images/publish.sh" push hub "localhost:$registry_port/lanekeeper/hub" kind --dir "$images_dir")"
    hub_digest="${ref#*@}"
    [[ "$hub_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { fail_reason="publish.sh did not print a digest reference"; return 1; }
    [ "$hub_digest" = "$(cat "$images_dir/hub.digest")" ] || { fail_reason="the pushed digest is not the one recorded by build.sh"; return 1; }
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
    local t0=$SECONDS ready total crashing got i
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
    for i in 0 1 2; do
        # /readyz is on the operations port (9090). The API server's pod proxy is the route; a non-2xx answer fails the command.
        got="$(k get --raw "/api/v1/namespaces/$NS/pods/lanekeeper-hub-$i:9090/proxy/readyz")" || { fail_reason="/readyz on lanekeeper-hub-$i did not answer 200"; return 1; }
        say "/readyz on lanekeeper-hub-$i: $got"
    done
    # P11, informational: from the container's start to the Ready condition (the API's timestamps have a resolution of one second).
    k -n "$NS" get pods -l "$HUB_SELECTOR" -o json | jq -r '.items[] | "\(.metadata.name) \((((.status.conditions[] | select(.type == "Ready") | .lastTransitionTime) | fromdateiso8601) - (.status.containerStatuses[0].state.running.startedAt | fromdateiso8601))) s from the start of the container to Ready"' | tee "$out/time-to-ready.txt"
    say "P11 (informational: kind is not the design scale): all 3 replicas Ready ${to_ready} s after the install (P11's start-up budget is 10 s per replica)"
    record "hub time to Ready" INFO "$to_ready" "see time-to-ready.txt"
}

# ---- run ---------------------------------------------------------------------------------------------------------------------

say "pins: node image $LK_KIND_NODE_IMAGE; CloudNativePG $LK_CNPG_VERSION; kind $(kind version | awk '{print $2}'); helm $(helm version --short)"
stage "cluster" start_cluster
stage "hub image" push_hub_image
stage "operator" install_operator
stage "database" install_database
stage "s6" check_s6
stage "hub" install_hub
