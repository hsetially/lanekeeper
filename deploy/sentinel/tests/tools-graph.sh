#!/usr/bin/env bash
# Shows, on BuildKit's own plan, that building a target of deploy/sentinel/Dockerfile.build builds no stage but its own (09/T8, review round 2;
# S7, S19, S20). The job that signs a release builds `rpm-tools`; if its plan held a step of the `binary` stage, `cargo build` and every
# crate's build script would run in the job that receives the credential and the key.
#
#   deploy/sentinel/tests/tools-graph.sh [TARGET]        default TARGET: rpm-tools
#
# It starts a build of TARGET with no network for the build steps (--network none) and an empty context, and reads the plan that BuildKit
# prints before it runs a step (tools-graph.mjs). Nothing is built or kept (no --tag, no --load; the build fails at its first step that needs
# the network), and the base images (pinned by digest) are the only thing pulled. Exit 0 when the plan is the target's own steps; 1 when it
# holds another stage; 2 when BuildKit stopped before it planned (the registry refused a base image, or the target is not in the file):
# nothing is proved then, and it is not a pass.
# Environment: LK_BUILDER, a buildx builder to use (CI sets none here: the default builder plans the same graph).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
target="${1:-rpm-tools}"
[[ "$target" =~ ^[a-z0-9][a-z0-9_.-]*$ ]] || { echo "tools-graph: '$target' is not a stage name" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/context"
args=(
    --file "$root/deploy/sentinel/Dockerfile.build"
    --platform linux/amd64
    --provenance=false
    --sbom=false
    --network none
    --target "$target"
    --progress=rawjson
)
[ -z "${LK_BUILDER:-}" ] || args+=(--builder "$LK_BUILDER")
# The build fails on purpose (no network), so its status says nothing; the plan it printed is what is read.
docker buildx build "${args[@]}" "$work/context" 2> "$work/progress.jsonl" >/dev/null || true
status=0
node "$here/tools-graph.mjs" "$target" "$work/progress.jsonl" || status=$?
if [ "$status" -eq 2 ]; then
    echo "tools-graph: what BuildKit printed instead (the end):" >&2
    grep -E '"(error|message|msg)"|ERROR|error' "$work/progress.jsonl" | tail -n 5 | cut -c1-300 >&2 || true
fi
exit "$status"
