#!/usr/bin/env bash
# Two builds of one commit give one binary and one RPM (09/T8; S19, S20): build.sh runs twice with --no-cache into two directories, and
# the files are compared byte for byte. The unsigned RPM is what is compared, because a signature is made later and carries its own time.
#
#   deploy/sentinel/tests/reproducible.sh [--worktree]
#
# --worktree builds the working tree (for development); by default the committed files are built, as in CI.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
extra=()
case "${1:-}" in
    "") ;;
    --worktree) extra=(--worktree) ;;
    *) echo "usage: $0 [--worktree]" >&2; exit 2 ;;
esac
out="$root/target/sentinel/repro"
rm -rf "$out"
for run in a b; do
    echo "reproducible: build $run (--no-cache)" >&2
    "$root/deploy/sentinel/build.sh" --no-cache --out "$out/$run" ${extra[@]+"${extra[@]}"} >/dev/null
done
if cmp -s "$out/a/SHA256SUMS" "$out/b/SHA256SUMS"; then
    echo "reproducible: both builds give the same files:"
    cat "$out/a/SHA256SUMS"
    exit 0
fi
echo "reproducible: the two builds differ:" >&2
diff "$out/a/SHA256SUMS" "$out/b/SHA256SUMS" >&2 || true
for f in lanekeeper-sentinel lanekeeper-sentinel-*.rpm; do
    for p in "$out"/a/$f; do
        b="$out/b/$(basename "$p")"
        cmp -s "$p" "$b" || { echo "reproducible: $(basename "$p") differs at: $(cmp -l "$p" "$b" | head -n 5 | tr '\n' ';')" >&2; }
    done
done
exit 1
