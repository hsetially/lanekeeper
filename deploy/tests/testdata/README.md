# Test data for deploy/tests

`synthetic-hub-ca.pem` is a certificate made for the render tests of the agent chart (09/T4). It is not the hub's CA and trusts
nothing: a throwaway EC key signed it on 2026-10-25, and the key was deleted at once (`openssl req -x509 -newkey ec ... -nodes`,
subject `SYNTHETIC test data`). `deploy/tests/render.sh` passes it to the chart with `--set-file hubCa.pem=...`, because the chart
refuses to render without a CA and the values files do not carry one (a swimlane's real CA is the hub's public CA certificate, put
in its values file by whoever adds the swimlane, `deploy/ADDING-A-SWIMLANE.md`).
