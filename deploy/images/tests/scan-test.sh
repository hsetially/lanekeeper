#!/usr/bin/env bash
# Proves scan.sh blocks a critical finding (09/T1, S20): the planted SBOM lists log4j-core 2.14.1 (CVE-2021-44228,
# critical), and the scan must fail and name it. A scan that fails for another reason (no database, no Docker) does not
# pass this test, because the finding must be in the output.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
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
