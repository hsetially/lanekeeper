#!/usr/bin/env bash
# The T1 Verify (09; S20): builds each image twice from the same commit, without the build cache, and fails unless the two
# digests are equal. On a difference it prints which config fields, layers and files differ (oci-diff.py), so the cause
# can be fixed or, if it cannot be removed, documented in deploy/images/REPRODUCIBILITY.md.
#
#   deploy/images/reproducible-check.sh [hub] [agent]      (both when none is named)
#
# The two builds use separate `git archive` extractions, so file times and modes in the checkout cannot leak in. The
# environment variables of build.sh apply (LK_BUILDER, LK_BUILD_CA, LK_BUILD_NETWORK).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
images=("$@")
[ ${#images[@]} -gt 0 ] || images=(hub agent)
work="$root/target/images/repro"
rm -rf "$work"
mkdir -p "$work"

status=0
for image in "${images[@]}"; do
    first="$("$here/build.sh" "$image" --no-cache --out "$work/a")"
    second="$("$here/build.sh" "$image" --no-cache --out "$work/b")"
    if [ "$first" = "$second" ]; then
        echo "reproducible: $image $first"
    else
        echo "NOT reproducible: $image $first != $second"
        python3 -I "$here/oci-diff.py" "$work/a/$image.oci.tar" "$work/b/$image.oci.tar" || true
        status=1
    fi
done
exit "$status"
