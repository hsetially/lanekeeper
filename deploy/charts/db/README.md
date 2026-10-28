# db chart

The Lanekeeper database (09/T5; D29, D40, D62, D124, S6, S7, S10, S16, S18): a CloudNativePG `Cluster` of three PostgreSQL 16
instances with pgvector and pg_trgm, TLS 1.3 only, backed up to Google Cloud Storage for 30 days, with a NetworkPolicy of its own.

The platform cluster's Argo CD deploys it with `deploy/argocd/platform/db.yaml` (the prod values, at a pinned commit, without prune: a database is
never deleted because a file left Git). The database image is scanned for critical findings every night by digest (`just images-scan-db`, the
`db-image` job of `nightly.yml`), because it is CloudNativePG's and not built by `images.yml` (plans/09-deployment-ci.blockers.md, B9, B10).

| Renders | Why |
|---|---|
| `Cluster/lanekeeper-db` | 3 instances (not a value), required anti-affinity over zones, `enablePDB`, the image by digest, `ssl_min_protocol_version: TLSv1.3` and `pg_hba` that rejects `hostnossl` first, no superuser Secret, the database `lanekeeper` owned by `lanekeeper_migrator` on `en_US.utf8`, the managed role `lanekeeper_app`, barman to `gs://BUCKET/lanekeeper-db` through Workload Identity (`retentionPolicy: 30d`, no key file), the operator's pod defaults written out (S16) |
| `ScheduledBackup/lanekeeper-db-daily` | one base backup a day; the WAL is archived continuously by the cluster |
| `StorageClass/lanekeeper-db-cmek` | with `storage.cmekKey`: disks encrypted with that Cloud KMS key (S10), `reclaimPolicy: Retain`. Without it the cluster says `lanekeeper.io/cmek: fallback` and the policy warns |
| `NetworkPolicy/lanekeeper-db` | ingress only from the hub's pods (5432), the operator (8000 and 5432, as its documentation says) and the instances (both) |

The chart refuses to render (`helm template` stops) without an image digest, an environment, and, outside kind, a bucket and a Google
service account for the backups. `values.schema.json` refuses unknown values, an image of another repository, and a schedule that is
not daily. The cluster's name is not a value: the hub's NetworkPolicy and `deploy/policy/database.rego` name it.

## Values files

`deploy/values/db/{kind,staging,prod}.yaml`. They hold example names (the project, the bucket, the key) until Q29 and the GCP project
are answered (rule 12). The image is real: the digest of a published CloudNativePG image, read from `ghcr.io`.

## Before it can start (once per environment, by a person; plan 09 A7)

1. **The operator and its CRDs**, installed apart (cluster-scoped): CloudNativePG **1.30.1**, the version whose CRDs `deploy/schemas`
   holds. In the namespace named by `network.operatorNamespace` (default `cnpg-system`).
2. **Two Secrets** of type `kubernetes.io/basic-auth` in the cluster's namespace. They are never in Git, never in the chart's output,
   and never in an environment variable (`deploy/policy/secrets.rego`). The passwords are made on the spot and kept nowhere else:

   ```
   ns=lanekeeper
   migrator_pw="$(openssl rand -hex 32)"
   app_pw="$(openssl rand -hex 32)"
   kubectl -n "$ns" create secret generic lanekeeper-db-migrator --type=kubernetes.io/basic-auth \
     --from-literal=username=lanekeeper_migrator --from-literal=password="$migrator_pw" \
     --from-literal=uri="postgresql://lanekeeper_migrator:${migrator_pw}@lanekeeper-db-rw:5432/lanekeeper"
   kubectl -n "$ns" create secret generic lanekeeper-db-app --type=kubernetes.io/basic-auth \
     --from-literal=username=lanekeeper_app --from-literal=password="$app_pw" \
     --from-literal=uri="postgresql://lanekeeper_app:${app_pw}@lanekeeper-db-rw:5432/lanekeeper"
   unset migrator_pw app_pw
   ```

   `uri` of the application role's Secret is what the hub reads (`LK_DATABASE_URL_FILE`), and the seed Job with it. `uri` of the
   migrator's is what the migration Job reads (`hub migrate`, `LK_MIGRATOR_URL_FILE`; charts/hub, `database.migratorSecretName`): the operator
   reads only `username` and `password` of a `basic-auth` Secret, so the extra key is the chart's own, and the Job mounts that one key and no
   other. `hub migrate` refuses a URL whose role is not `lanekeeper_migrator`, and `hub seed` one whose role is not `lanekeeper_app`. The URLs
   name no `sslmode`: the hub always connects with `verify-full` against the
   CA in `lanekeeper-db-ca` (`LK_DB_CA_FILE`), whatever the URL says (`crates/hub/src/db.rs`). The migrator has a Secret of its own
   because, left to the operator, the owner's credentials would be made in `lanekeeper-db-app`, the name the hub reads: the app would
   log in as the owner of the database.
3. **The Google side**: `charts/gcp` makes the backup bucket, the service account `lanekeeper-db` and the binding that lets it write
   there, and lets Compute Engine use the disk key (`cmek.diskKeys`). `deploy/tests/render.sh contract` checks that the names in the
   two charts agree.

## What the roles may do

`lanekeeper_migrator` owns the database and runs the migrations; `lanekeeper_app` is a managed role with `login` and nothing else
(`NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`, no `inRoles`): what it may do is what the migrations grant, table by
table (`db/migrations/README.md`). **`postInitApplicationSQL` creates the two extensions and nothing else.** `deploy/dev/init.sql`
also grants `CONNECT` and `USAGE` to the roles; here that is neither possible nor needed. The operator makes a managed role after the
bootstrap, so a grant to it in `postInitApplicationSQL` would fail the bootstrap; and on PostgreSQL 16 the owner of the database owns
`public` (through `pg_database_owner`), while `CONNECT` and `USAGE` are `PUBLIC`'s by default.

## Checks

```
just deploy-check                  # everything, including the gcp chart with Cloud Armor, which waits for GitHub's ranges (B5)
just deploy-data                   # the data layer alone: this chart, and charts/gcp without Cloud Armor
deploy/tests/render.sh planted     # the real output, changed one field at a time, refused for its own reason
```

## Not done here

- **Egress of the database pods is not restricted.** The plan's row asks for ingress only. A default-deny egress needs the cluster's
  API-server addresses (as in the agent chart) and Google Cloud Storage by name, which needs `FQDNNetworkPolicy` rules that are today
  written for the hub alone (`network.rego`). Proposed as a follow-up; until then the database pods can reach anything the node can.
- **Monitoring** (`9187`) is not admitted. Add the monitoring namespace when there is something to scrape it.
- **Sizing is a placeholder** (2 CPU, 2 GiB, 20 GiB, `shared_buffers` 512 MB) until the load test of prompt 16 measures the database at
  design scale. No planner parameter is changed: the plans in the registry's tests were taken with the defaults (D124).
- **The backup method is the operator's built-in barman support**, which CloudNativePG has deprecated since 1.26 in favour of the Barman
  Cloud plugin, and it needs the image variant `system`, which is deprecated with it (`plans/09-deployment-ci.blockers.md`, B10). The
  plan named the `standard` image, which has no barman. A person decides whether to move to the plugin.

## Not verified here

The sandbox that wrote this has no Kubernetes cluster, and could read the image's registry entry but not its layers
(`pkg-containers.githubusercontent.com` is refused, B10), so:

- that the image has pgvector, pg_trgm, the `en_US.utf8` locale and barman is taken from its build recipe
  (`cloudnative-pg/postgres-containers`, `Dockerfile`: `standard` adds `locales-all` and pgvector, `system` adds barman), not from the image;
  the first `CREATE EXTENSION` in kind (CI) is the test;
- that CloudNativePG 1.30.1 accepts every field as written is checked against its CRD (kubeconform, strict) and its documentation at the
  commit the schemas come from, not by running it;
- that a cluster enforces NetworkPolicy at all is a property of the cluster, not of the chart.
