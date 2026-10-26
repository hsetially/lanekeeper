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
