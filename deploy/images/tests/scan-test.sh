#!/usr/bin/env bash
# Proves scan.sh blocks a critical finding (09/T1, S20): the planted SBOM lists log4j-core 2.14.1 (CVE-2021-44228,
# critical), and the scan must fail and name it. A scan that fails for another reason (no database, no Docker) does not
# pass this test, because the finding must be in the output.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Started as root, step down to the unprivileged user that a CI runner is. As root, no permission is ever refused, and the grype container
# could not write its /tmp as the runner's uid (the first CI run): this test would not have seen it. The repository's target/ belongs to
# root here, so the database is cached under the user's HOME.
[ "$(id -u)" -ne 0 ] || exec "$here/../../tests/as-user.sh" "${BASH_SOURCE[0]}" "$@"
[ -z "${LK_AS_USER:-}" ] || export LK_GRYPE_DB_CACHE="$HOME/grype-db"
work="$(mktemp -d)"
log="$work/log"
trap 'rm -rf "$work"' EXIT
if "$here/../scan.sh" --sbom "$here/critical.cdx.json" >"$log" 2>&1; then
    cat "$log"
    echo "scan-test: the planted critical SBOM passed the scan"
    exit 1
fi
if ! grep -q 'CVE-2021-44228' "$log" || ! grep -qi 'critical' "$log"; then
    cat "$log"
    echo "scan-test: the scan failed, but not on the planted CVE-2021-44228"
    exit 1
fi
echo "scan-test: the planted critical finding (CVE-2021-44228) fails the scan"

# The scan of an image, as the image jobs run it (`scan.sh agent --out DIR`): grype unpacks the image archive into its /tmp, which is where
# the runner's user was refused, and reads the SBOM. A clean image of one file must pass, and scan.sh must say so.
mkdir "$work/image"
python3 -I "$here/tiny-oci.py" "$work/image/agent.oci.tar"
printf '{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"components":[]}\n' >"$work/image/agent.cdx.json"
if ! "$here/../scan.sh" agent --out "$work/image" >"$log" 2>&1; then
    cat "$log"
    echo "scan-test: a clean image archive and its SBOM did not pass the scan"
    exit 1
fi
if ! grep -q 'agent has no critical findings' "$log"; then
    cat "$log"
    echo "scan-test: the scan of a clean image passed without saying so"
    exit 1
fi
echo "scan-test: a clean image archive and its SBOM pass the scan, as uid $(id -u)"
