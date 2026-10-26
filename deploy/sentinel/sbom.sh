#!/usr/bin/env bash
# The CycloneDX SBOM of the sentinel RPM (09/T8; S20): what syft finds in the RPM (the package, with its version and architecture), merged
# with what `cargo cyclonedx` lists for the Rust binary, whose crates a scanner cannot see in a stripped static executable. The document
# describes the RPM by name and sha256.
#
#   deploy/sentinel/sbom.sh --rpm FILE [--out DIR]
#
# Writes DIR/<name of the RPM without .rpm>.cdx.json (default DIR: the RPM's own directory). Give it the file that is distributed (the
# signed RPM), because the hash in the document is of that file. deploy/images/scan.sh --sbom FILE scans the result.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
# shellcheck source=deploy/images/tools.env
. "$root/deploy/images/tools.env"
usage() { echo "usage: $0 --rpm FILE [--out DIR]" >&2; exit 2; }
rpm_file="" out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --rpm) [ $# -ge 2 ] || usage; rpm_file="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || usage; out="$2"; shift 2 ;;
        *) usage ;;
    esac
done
[ -f "$rpm_file" ] || usage
rpm_file="$(cd "$(dirname "$rpm_file")" && pwd)/$(basename "$rpm_file")"
[[ "$(basename "$rpm_file")" =~ ^lanekeeper-sentinel-[0-9]+\.[0-9]+\.[0-9]+-1\.x86_64\.rpm$ ]] || { echo "sbom.sh: '$rpm_file' is not lanekeeper-sentinel-<version>-1.x86_64.rpm" >&2; exit 2; }
[ -n "$out" ] || out="$(dirname "$rpm_file")"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

cargo cyclonedx --version 2>/dev/null | grep -qxE "cargo-cyclonedx(-cyclonedx)? ${LK_CARGO_CYCLONEDX_VERSION//./\\.}" || {
    echo "sbom.sh: need cargo-cyclonedx $LK_CARGO_CYCLONEDX_VERSION: cargo install --locked cargo-cyclonedx --version $LK_CARGO_CYCLONEDX_VERSION" >&2
    exit 1
}

work="$(mktemp -d)"
[ -z "$(git -C "$root" ls-files -- 'crates/*/*.cdx.json')" ] || { echo "sbom.sh: tracked *.cdx.json under crates/" >&2; exit 1; }
cleanup() {
    {
        git -C "$root" ls-files --others -- 'crates/*/*.cdx.json'
        git -C "$root" ls-files --others --ignored --exclude-standard -- 'crates/*/*.cdx.json'
    } | sort -u | while read -r f; do rm -f "$root/$f"; done
    rm -rf "$work"
}
trap cleanup EXIT

# 1. The RPM, as a package syft knows.
mkdir "$work/in"
cp "$rpm_file" "$work/in/"
docker run --rm --network none --user "$(id -u):$(id -g)" -e SYFT_CHECK_FOR_APP_UPDATE=false \
    -v "$work/in:/in:ro" -v "$work:/out" "$LK_SYFT_IMAGE" "dir:/in" --output "cyclonedx-json=/out/rpm.cdx.json" --quiet

# 2. The crates in the binary: every transitive normal and build dependency of the sentinel package, for the target it is built for.
cargo cyclonedx --manifest-path "$root/crates/sentinel/Cargo.toml" --all --format json --spec-version 1.5 \
    --target x86_64-unknown-linux-musl --quiet
[ -f "$root/crates/sentinel/sentinel.cdx.json" ] || { echo "sbom.sh: cargo cyclonedx wrote no SBOM for the sentinel" >&2; exit 1; }
cp "$root/crates/sentinel/sentinel.cdx.json" "$work/crates.cdx.json"

# 3. One document: the RPM as the subject, and the union of both component lists (by purl, else by name and version), with the sentinel
# crate itself (cargo lists the dependencies apart from the package). The component the
# document is about is the RPM that syft found, and the file it came from is what the document hashes.
name="$(basename "$rpm_file" .rpm)"
sha="$(sha256sum "$rpm_file" | cut -d' ' -f1)"
jq -n --slurpfile rpm "$work/rpm.cdx.json" --slurpfile rs "$work/crates.cdx.json" --arg name "$(basename "$rpm_file")" --arg sha "$sha" '
  ($rpm[0]) as $i | ($rs[0]) as $r
  | $i
  | .metadata.component = {"type": "file", "name": $name, "hashes": [{"alg": "SHA-256", "content": $sha}]}
  | .components = ((($i.components // []) | map(select(.type != "file"))) + [$r.metadata.component | select(. != null)] + ($r.components // [])
      | unique_by(.purl // ((.name // "") + "@" + (.version // ""))))
  | .dependencies = (($i.dependencies // []) + ($r.dependencies // []) | unique_by(.ref))
' > "$out/$name.cdx.json"
echo "sbom.sh: $out/$name.cdx.json ($(jq '.components | length' "$out/$name.cdx.json") components, subject sha256 $sha)"
