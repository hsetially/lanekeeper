#!/usr/bin/env bash
# Proves the registry stages of deploy/tests/kind/run.sh (CI first run: `hub image` failed in 0 s with a refused connection) against a real
# registry container, a real crane and a real Docker, with no cluster:
#   - `run.sh --registry-only` starts the pinned registry on 127.0.0.1, waits for it, pushes a tiny image under its recorded digest, says
#     so, and leaves no container behind;
#   - an archive that is not the recorded one is refused, the stage is named, and the registry's state is printed;
#   - lib.sh's registry_report reads a real container (the Go templates of `docker inspect` are Docker's, not a stand-in's), and the
#     container's address on a Docker network answers and takes a push from this host: the second route of the `hub image` stage.
# What it cannot show: a published port that stops answering while its container lives (the first CI run's failure; .github/scripts/
# kind-run.test.mjs drives that with stand-ins) and anything that needs a kind cluster.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$root/deploy/images/tools.env"
export PATH="$root/target/ops-tools/bin:$PATH"
# shellcheck source=deploy/tests/kind/lib.sh
. "$here/lib.sh"
work="$(mktemp -d)"
probe="lk-kind-registry-test-$$"
cleanup() {
    docker rm --force "$probe" >/dev/null 2>&1 || true
    docker rm --force kind-registry.test >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT
fail() { echo "registry-test: $*" >&2; exit 1; }
for tool in docker crane jq python3; do command -v "$tool" >/dev/null || fail "need $tool"; done
# run.sh names its registry kind-registry.test and replaces one that exists, so a kind-e2e run in progress would lose its registry here.
[ -z "$(docker ps --all --quiet --filter name='^kind-registry.test$')" ] || fail "a container named kind-registry.test exists (a kind-e2e run?); remove it with: docker rm -f kind-registry.test"

mkdir "$work/images" "$work/layout"
python3 -I "$here/../../images/tests/tiny-oci.py" "$work/images/hub.oci.tar"
tar --extract --file "$work/images/hub.oci.tar" --directory "$work/layout" index.json
recorded="$(jq -r '.manifests[0].digest' "$work/layout/index.json")"
printf '%s\n' "$recorded" > "$work/images/hub.digest"

# 1. the stages, against a real registry.
if ! "$here/run.sh" --registry-only --images-dir "$work/images" --out "$work/out" > "$work/run.log" 2>&1; then cat "$work/run.log" >&2; fail "run.sh --registry-only failed"; fi
grep -q -E 'the local registry answers on 127\.0\.0\.1:[0-9]+' "$work/run.log" || { cat "$work/run.log" >&2; fail "the registry stage did not say that the registry answers on 127.0.0.1"; }
grep -q -E "the hub image is in the registry at 127\.0\.0\.1:[0-9]+/lanekeeper/hub@$recorded" "$work/run.log" || { cat "$work/run.log" >&2; fail "the hub image stage did not push $recorded to 127.0.0.1"; }
if grep -q 'localhost' "$work/run.log"; then cat "$work/run.log" >&2; fail "the run used localhost for the registry"; fi
grep -q '^| registry | PASS |' "$work/out/summary.md" || fail "the summary has no passing registry stage"
[ -z "$(docker ps --all --quiet --filter name='^kind-registry.test$')" ] || fail "the registry container was left behind"
echo "registry-test: --registry-only starts the registry, waits for it, and pushes $recorded to 127.0.0.1; nothing is left behind"

# 2. an archive that is not the recorded one is refused, with the stage named and the registry's state printed.
printf 'sha256:%064d\n' 0 > "$work/images/hub.digest"
if "$here/run.sh" --registry-only --images-dir "$work/images" --out "$work/out2" > "$work/bad.log" 2>&1; then cat "$work/bad.log" >&2; fail "run.sh pushed an archive that is not the recorded one"; fi
grep -q "FAILED in the stage 'hub image'" "$work/bad.log" || { cat "$work/bad.log" >&2; fail "the failure does not name the stage"; }
grep -q 'build.sh recorded' "$work/bad.log" || { cat "$work/bad.log" >&2; fail "the failure does not say why publish.sh refused the archive"; }
grep -q 'the registry container kind-registry.test: state' "$work/bad.log" || { cat "$work/bad.log" >&2; fail "the failure does not print the registry's state"; }
grep -q -E 'status=running running=true exit_code=0' "$work/bad.log" || { cat "$work/bad.log" >&2; fail "the registry's state line is not Docker's"; }
printf '%s\n' "$recorded" > "$work/images/hub.digest"
echo "registry-test: a refused archive fails the stage 'hub image' and prints the registry's state"

# 3. a real container: registry_report, and the address of the container on its network.
docker run --detach --name "$probe" --publish 127.0.0.1::5000 "$LK_REGISTRY_IMAGE" >/dev/null
port="$(docker port "$probe" 5000/tcp | sed -n '1s/.*://p')"
wait_for_http 60 "http://127.0.0.1:$port/v2/" || fail "the probe registry did not answer on 127.0.0.1:$port"
[ "$HTTP_ANSWERED" = "http://127.0.0.1:$port/v2/" ] || fail "wait_for_http says '$HTTP_ANSWERED'"
registry_report "$probe" "$port" 2> "$work/report.txt"
for want in "state" "status=running running=true exit_code=0 oom_killed=false" "published ports" "127.0.0.1:$port" "bridge 172\.17\.|bridge [0-9]+\." "the last 30 lines of its log"; do
    grep -q -E -- "$want" "$work/report.txt" || { cat "$work/report.txt" >&2; fail "registry_report lacks /$want/"; }
done
ip="$(registry_container_ip "$probe" bridge)"
[[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "registry_container_ip gave '$ip' for the bridge network"
[ -z "$(registry_container_ip "$probe" no-such-network)" ] || fail "registry_container_ip answered for a network the container is not on"
wait_for_http 30 "http://$ip:5000/v2/" || fail "the container's address $ip:5000 does not answer from this host"
pushed="$("$here/../../images/publish.sh" push hub "$ip:5000/lanekeeper/hub" kind --dir "$work/images" 2>/dev/null)"
[ "$pushed" = "$ip:5000/lanekeeper/hub@$recorded" ] || fail "a push to the container's address printed '$pushed'"
[ "$(crane digest "127.0.0.1:$port/lanekeeper/hub:kind")" = "$recorded" ] || fail "the registry does not hold $recorded under the published port after a push to its address"
echo "registry-test: registry_report reads a real container; its address $ip answers and takes a push that the published port then serves"
echo "registry-test: all cases behaved"
