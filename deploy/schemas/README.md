# schemas

The JSON Schemas that `kubeconform -strict` reads, vendored so that it runs offline and never skips a kind (no
`-ignore-missing-schemas`): a resource whose kind has no schema here is an error. Nothing is fetched in CI.

- `k8s/v1.34.0-standalone-strict/`: the built-in kinds the charts use, from `yannh/kubernetes-json-schema` (kubeconform's own
  repository) at a pinned commit.
- `crds/<group>/<kind>_<version>.json`: CloudNativePG, Gateway API, Argo CD and Config Connector, made by `crd2schema.py` from the
  projects' own CRD files at a pinned tag and commit; and the three GKE policy kinds (`GCPBackendPolicy`, `HealthCheckPolicy`,
  `FQDNNetworkPolicy`), copied from the community catalog `datreeio/CRDs-catalog`, because Google publishes no CRD file for them.
- `sources.json` says where each comes from; `schemas.lock.json` holds the sha256 of every source and of every file made from it.

`node deploy/schemas/verify.mjs` (CI) fails if a file differs from the lock, is not in it, or if the lock names a commit that
`sources.json` does not pin. To move a source, on a developer machine: edit `sources.json`, run
`python3 -B -I deploy/schemas/fetch.py` (needs PyYAML), read the diff, commit it all. `fetch.py --check` fetches again and compares.

`crd2schema.py` closes every object that lists `properties` (`additionalProperties: false`), as kubectl does, but not inside
`allOf`/`anyOf`/`oneOf`, whose branches only check a value (Gateway API's `addresses` needs this; `test_crd2schema.py`).
