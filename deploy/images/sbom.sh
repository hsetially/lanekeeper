#!/usr/bin/env bash
# The CycloneDX SBOM of one image (09/T1, S20): what syft finds in the image (the distroless packages, the web app's and
# the model's files), merged with what `cargo cyclonedx` lists for the Rust binary, whose crates a scanner cannot see in a
# stripped binary.
#
#   deploy/images/sbom.sh <hub|agent> [--out DIR]
#
# Reads DIR/<image>.oci.tar (from build.sh) and writes DIR/<image>.cdx.json. CI attaches that file to the image as an
# attestation and gives it to scan.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$here/tools.env"
# shellcheck source=deploy/images/container.sh
. "$here/container.sh"
lk_explain_failures
usage() { echo "usage: $0 <hub|agent> [--out DIR]" >&2; exit 2; }
[ $# -ge 1 ] || usage
image="$1"
shift
case "$image" in hub | agent) ;; *) usage ;; esac
out="$root/target/images"
[ $# -eq 0 ] || { [ $# -eq 2 ] && [ "$1" = "--out" ] || usage; out="$2"; }
out="$(cd "$out" && pwd)"
[ -f "$out/$image.oci.tar" ] || { echo "sbom.sh: no $out/$image.oci.tar; run build.sh $image first" >&2; exit 1; }

# `cargo cyclonedx --version` prints "cargo-cyclonedx-cyclonedx <version>" (the subcommand name is doubled).
cargo cyclonedx --version 2>/dev/null | grep -qxE "cargo-cyclonedx(-cyclonedx)? ${LK_CARGO_CYCLONEDX_VERSION//./\\.}" || {
    echo "sbom.sh: need cargo-cyclonedx $LK_CARGO_CYCLONEDX_VERSION: cargo install --locked cargo-cyclonedx --version $LK_CARGO_CYCLONEDX_VERSION" >&2
    exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# 1. The image, as the registry will hold it. syft runs as this user with a /tmp that this user owns, and is not quiet: on a CI runner
# the image's own /tmp, root's, refused it, and `--quiet` hid the message, so the job ended with exit code 1 and no line of output.
lk_syft "$out" "oci-archive:/in/$image.oci.tar" "$work" image.cdx.json

# 2. The crates in the binary: every transitive normal and build dependency of the binary's package (`--all`; the default
# is top-level only), for the target it runs on, with the default features `cargo build -p <image>` uses. cargo-cyclonedx
# writes one `<crate>.cdx.json` beside each workspace member's Cargo.toml; only ours is kept, and every file it wrote is
# removed again (the files it may overwrite are never tracked: `git ls-files` is checked first).
[ -z "$(git -C "$root" ls-files -- 'crates/*/*.cdx.json')" ] || { echo "sbom.sh: tracked *.cdx.json under crates/" >&2; exit 1; }
cleanup() {
    {
        git -C "$root" ls-files --others -- 'crates/*/*.cdx.json'
        git -C "$root" ls-files --others --ignored --exclude-standard -- 'crates/*/*.cdx.json'
    } | sort -u | while read -r f; do rm -f "$root/$f"; done
    rm -rf "$work"
}
trap cleanup EXIT
# Its output is kept and shown when it fails: `--quiet` would hide the reason.
cargo cyclonedx --manifest-path "$root/crates/$image/Cargo.toml" --all --format json --spec-version 1.5 \
    --target x86_64-unknown-linux-gnu >"$work/cyclonedx.log" 2>&1 || {
    cat "$work/cyclonedx.log" >&2
    echo "sbom.sh: cargo cyclonedx failed for crates/$image (its output is above)" >&2
    exit 1
}
[ -f "$root/crates/$image/$image.cdx.json" ] || { echo "sbom.sh: cargo cyclonedx wrote no SBOM for $image" >&2; exit 1; }
cp "$root/crates/$image/$image.cdx.json" "$work/crates.cdx.json"

# 3. One document: the image's metadata, and the union of both component lists (by purl, else by name and version).
# The document describes the image by name and digest, not by the path syft read it from.
digest="$(cat "$out/$image.digest" 2>/dev/null || true)"
jq -n --slurpfile img "$work/image.cdx.json" --slurpfile rs "$work/crates.cdx.json" \
    --arg name "lanekeeper-$image" --arg digest "$digest" '
  ($img[0]) as $i | ($rs[0]) as $r
  | $i
  | .metadata.component.name = $name
  | .metadata.component.version = $digest
  | .components = ((($i.components // []) + ($r.components // []))
      | unique_by(.purl // ((.name // "") + "@" + (.version // ""))))
  | .dependencies = (($i.dependencies // []) + ($r.dependencies // []) | unique_by(.ref))
' > "$out/$image.cdx.json"
echo "sbom.sh: $out/$image.cdx.json ($(jq '.components | length' "$out/$image.cdx.json") components)"
