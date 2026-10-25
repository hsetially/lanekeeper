# Test data for deploy/tests

`synthetic-hub-ca.pem` is a certificate made for the render tests of the agent chart (09/T4). It is not the hub's CA and trusts
nothing: a throwaway EC key signed it on 2026-10-25, and the key was deleted at once (`openssl req -x509 -newkey ec ... -nodes`,
subject `SYNTHETIC test data`). `deploy/tests/render.sh` passes it to the chart with `--set-file hubCa.pem=...`, because the chart
refuses to render without a CA and the values files do not carry one (a swimlane's real CA is the hub's public CA certificate, put
in its values file by whoever adds the swimlane, `deploy/ADDING-A-SWIMLANE.md`).

`synthetic-attestor-key.yaml` (09/T6) is a values file that holds a throwaway **public** key, for the render tests of the gcp chart. It is
not the CI signing key and trusts nothing: the key pair was made on 2026-10-25 with `openssl genpkey -algorithm EC -pkeyopt
ec_paramgen_curve:P-256 | openssl pkey -pubout`, the private key was never written to disk, and no signature can verify against it. The
gcp chart refuses to render without the public key of the CI signing key (`binaryAuthorization.publicKeys`), and the values files do
not carry one, because the key is made in Cloud KMS by a person and its public half is put into the values by whoever makes it
(`deploy/charts/gcp/README.md`, "Admission"). `deploy/tests/render.sh` layers this file over the gcp chart's values **only while the
environment's values have no `publicKeyPem`**, so that once the real key is in, the real key is what is checked.
