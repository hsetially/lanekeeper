#!/usr/bin/env bash
# Runs the hub Dockerfile's `model` stage script against synthetic locks, with `curl` replaced by a shim that serves fixed
# bytes, and checks that it accepts a matching lock and refuses each mismatch (09/T1; S19, S20). The script is taken from
# the Dockerfile itself, so this tests the instruction the image build runs, not a copy. Needs Docker (the stage's own
# pinned rust image) and no network.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dockerfile="$here/../../../crates/hub/Dockerfile"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The model stage's base image and its RUN instruction, continuation lines joined and the --mount flags removed.
python3 -I - "$dockerfile" "$work" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
lines = text.splitlines()
start = next(i for i, l in enumerate(lines) if l.startswith("FROM ") and l.endswith(" AS model"))
open(f"{sys.argv[2]}/image", "w").write(lines[start].split()[1])
logical, current = [], ""
for line in lines[start + 1:]:
    if line.startswith("FROM "):
        break
    if line.lstrip().startswith("#") or not line.strip():
        continue
    if line.rstrip().endswith("\\"):
        current += line.rstrip()[:-1] + " "
        continue
    logical.append(current + line)
    current = ""
runs = [l for l in logical if l.startswith("RUN ")]
assert len(runs) == 1, f"expected one RUN in the model stage, found {len(runs)}"
script = re.sub(r"^RUN\s+", "", runs[0])
while script.startswith("--mount="):
    script = script.split(None, 1)[1]
open(f"{sys.argv[2]}/script.sh", "w").write(script + "\n")
PY

# The shim: writes $SHIM_BYTES to the --output file and records the URL it was asked for.
mkdir -p "$work/shim"
cat > "$work/shim/curl" <<'SH'
#!/bin/sh
out=""; url=""
while [ $# -gt 0 ]; do
    case "$1" in --output) out="$2"; shift 2 ;; --*) case "$1" in --proto|--proto-redir|--retry|--connect-timeout|--max-time|--max-filesize) shift 2 ;; *) shift ;; esac ;; *) url="$1"; shift ;; esac
done
printf '%s' "$SHIM_BYTES" > "$out"
echo "$url" >> /tmp/urls
SH
chmod +x "$work/shim/curl"

bytes="lanekeeper model stage test"
sha="$(printf '%s' "$bytes" | sha256sum | cut -d' ' -f1)"
size="${#bytes}"
rev="0123456789abcdef0123456789abcdef01234567"
lock() {
    printf 'model bge-small-en-v1.5\nrepo BAAI/bge-small-en-v1.5\nrevision %s\ndate 2026-01-01T00:00:00Z\n' "$1"
    printf 'file %s %s onnx/model.onnx\nfile %s %s tokenizer.json\n' "$2" "$3" "$2" "$3"
    [ -z "${4:-}" ] || printf '%s\n' "$4"
}
run() {
    docker run --rm --network none -e SHIM_BYTES="$bytes" \
        -v "$work/lock:/hub-model.lock:ro" -v "$work/shim:/shim:ro" -v "$work/script.sh:/script.sh:ro" \
        "$(cat "$work/image")" sh -c 'PATH=/shim:$PATH; . /script.sh && find /models -type f | sort && cat /tmp/urls' 2>&1
}

failures=0
expect_pass() {
    if output="$(run)"; then
        for want in "/models/bge-small-en-v1.5/onnx/model.onnx" "/models/bge-small-en-v1.5/tokenizer.json" \
            "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/$rev/onnx/model.onnx"; do
            grep -qxF "$want" <<<"$output" || { echo "FAIL $1: missing $want"; echo "$output"; failures=$((failures + 1)); return; }
        done
        echo "ok   $1"
    else
        echo "FAIL $1: refused"; echo "$output"; failures=$((failures + 1))
    fi
}
expect_refused() {
    if output="$(run)"; then
        echo "FAIL $1: accepted"; echo "$output"; failures=$((failures + 1))
    else
        echo "ok   $1 (refused)"
    fi
}

lock "$rev" "$sha" "$size" > "$work/lock"; expect_pass "a matching lock"
lock "$rev" "$(printf '%064d' 0)" "$size" > "$work/lock"; expect_refused "a wrong sha256"
lock "$rev" "$sha" "$((size + 1))" > "$work/lock"; expect_refused "a wrong size"
lock "UNRESOLVED" "$sha" "$size" > "$work/lock"; expect_refused "an unresolved revision"
lock "main" "$sha" "$size" > "$work/lock"; expect_refused "a branch for a revision"
lock "$rev" "$sha" "$size" "file $sha $size ../../usr/local/bin/hub" > "$work/lock"; expect_refused "a path that leaves /models"
lock "$rev" "$sha" "$size" | sed 's#repo BAAI/bge-small-en-v1.5#repo evil.example/x/y#' > "$work/lock"; expect_refused "a repo with a host"
lock "$rev" "$sha" "$size" | grep -v '^file ' > "$work/lock"; expect_refused "a lock with no files"
[ "$failures" -eq 0 ] || { echo "model-stage-test: $failures case(s) failed"; exit 1; }
echo "model-stage-test: the model stage accepts the matching lock and refuses every planted mismatch"
