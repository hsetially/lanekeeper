# hub chart

The Lanekeeper hub (09/T3; D62, S6, S7, S10, S16, S18, P11): three replicas of a StatefulSet, one volume each for the Git mirror,
hardened as `crates/hub/HARDENING.md` says, behind a Gateway for users and a TCP pass-through load balancer for agents.

The platform cluster's Argo CD deploys it with `deploy/argocd/platform/hub.yaml` (the prod values, at a pinned commit; `deploy/argocd/README.md`).

| Renders | Why |
|---|---|
| `StatefulSet/lanekeeper-hub` | 3 replicas (not a value), required anti-affinity over zones, `volumeClaimTemplates` for the mirror, the HARDENING.md pod |
| `PodDisruptionBudget`, headless `Service/lanekeeper-hub-replicas` | `minAvailable: 2`; the replicas find one another by DNS (`agent_connections.replica_address`) |
| `StorageClass/lanekeeper-hub-cmek` | with `storage.cmekKey`: disks encrypted with that Cloud KMS key (S10). Without it the claim says `lanekeeper.io/cmek: fallback` and the policy warns |
| `ServiceAccount/lanekeeper-hub` | `iam.gke.io/gcp-service-account` for Workload Identity; no token is mounted |
| `Service/lanekeeper-hub-agent` | `LoadBalancer` on **443**, the port the agents dial, forwarding to the container port named `agent` (8444, which the NetworkPolicy admits); `externalTrafficPolicy: Local`, `loadBalancerSourceRanges` = `network.agentSourceRanges` (never empty) |
| `Service/lanekeeper-hub-web`, `Gateway`, `HTTPRoute`, `GCPBackendPolicy`, `HealthCheckPolicy` | the users' door: HTTPS only, a named certificate (never a Secret), Cloud Armor attached, health check `/readyz` on 9090 |
| `NetworkPolicy/lanekeeper-hub`, `FQDNNetworkPolicy/lanekeeper-hub-external` | default deny; only what HARDENING.md lists. Without FQDN policy (`network.fqdnPolicy: false`) the external hosts are reached by address, which is weaker and marked |
| `Job/lanekeeper-hub-migrate`, `ServiceAccount/lanekeeper-hub-jobs`, `NetworkPolicy/lanekeeper-hub-jobs` | `hub migrate` as the migrator, a Helm `pre-install,pre-upgrade` hook (Argo CD: PreSync); its account and policy are hooks with a lower weight, so that a first install has them. See "The Jobs" |
| `Job/lanekeeper-hub-seed-<hash>`, `ConfigMap/lanekeeper-hub-seed-<hash>` | with `seed.enabled`: `hub seed` as the application role, after the migration; not a hook. The ConfigMap only with `seed.content` |

The chart refuses to render (`helm template` stops) without an image digest, a Google service account, non-empty agent source
ranges, and, for the Gateway, a host name, an address name and exactly one certificate. `values.schema.json` refuses unknown
values, a tunable the hub does not have, and a CA Secret that is not CloudNativePG's.

## The hub's settings

The hub exits 2 and starts nothing unless 25 variables are set and valid (`REQUIRED_VARIABLES` in `crates/hub/tests/hardening.rs`; the
contract is `crates/hub/HARDENING.md` and the checklist is the last section of `crates/hub/README.md`). The chart renders all of them, and
`deploy/tests/render.sh contract` reads that list from the test and shows each one in the rendered StatefulSet. Nothing is a secret: a
Secret Manager entry is the NAME of a secret, a KMS entry is a resource name, and the database URL is a mounted file (S7).

| Value | Variable | Default |
|---|---|---|
| `gateway.hostname` | `LK_PUBLIC_ORIGIN` and `LK_HUB_AUDIENCE`, both `https://<hostname>` | none, required (also where the Gateway is off) |
| `config.gcpProject` | `LK_GCP_PROJECT` | none, required |
| `config.entra.tenantId`, `config.entra.clientId` | `LK_ENTRA_TENANT_ID`, `LK_ENTRA_CLIENT_ID` | none, required (GUIDs) |
| `config.kms.agentCa`, `config.kms.replicaCa` | `LK_AGENT_CA_KMS_REF`, `LK_REPLICA_CA_KMS_REF` | none, required: key *versions*, two different keys (S5) |
| `config.kms.auditCheckpoint` | `LK_AUDIT_CHECKPOINT_KMS_REF` | none, required: a key version |
| `config.kms.vault` | `LK_VAULT_KMS_REF` | none, required: a crypto key *without* a version |
| `config.auditCheckpointBucket` | `LK_AUDIT_CHECKPOINT_BUCKET` | none, required: the bucket `charts/gcp` makes (`render.sh contract` compares them) |
| `config.agentEndpointHosts` | `LK_AGENT_ENDPOINT_HOSTS` | none, required: the host of `hub.endpoint` of every agent values file is one of them (`render.sh contract`) |
| `config.git.*` | `LK_GIT_BASE_URL`, `LK_GIT_TENANT_URL`, `LK_GIT_BASE_REPO`, `LK_GIT_TENANT_REPO` | none, required |
| `config.secretManager.*` | `LK_WEB_CERT_REF`, `LK_WEB_KEY_REF`, `LK_AGENT_CA_CERT_REF`, `LK_REPLICA_CA_CERT_REF`, `LK_ENTRA_CLIENT_AUTH_REF`, `LK_ENTRA_COOKIE_KEY_REF` | `lanekeeper-web-cert`, `lanekeeper-web-key`, `lanekeeper-agent-ca-cert`, `lanekeeper-replica-ca-cert`, `lanekeeper-entra-client`, `lanekeeper-login-cookie`; the two CA certificates must differ |
| `config.bootstrapAdmins` | `LK_BOOTSTRAP_ADMINS` | none (optional): Entra object ids; remove them once real administrators exist |
| (the pod itself) | `LK_REPLICA_NAME` from the downward API (`metadata.name`), then `LK_REPLICA_ADDRESS` = `$(LK_REPLICA_NAME).lanekeeper-hub-replicas.<namespace>.svc:8445` | the one `valueFrom` the contract allows; it must come first, because Kubernetes expands `$(NAME)` only for variables defined earlier |

`config.tuning` takes the optional settings, by variable name, within the ranges the hub accepts: the limits and timeouts of the hub, the
write paths' intervals (`LK_WRITES_*_SECS`), `LK_START_TIMEOUT_SECS`, `LK_WEB_CERT_RELOAD_SECS`, `LK_CORPUS_CACHE_MIB`, `LK_GIT_BASE_BRANCH`,
`LK_CONFIG_SERVER_DEPLOYMENT`, `LK_ENTRA_API_AUDIENCES` and the three MCP switches (`LK_MCP_AUTH`, `LK_MCP_CONFIRM`, `LK_MCP_CONTENT`).
Not settable on purpose, because the hub's defaults are the safe state and a change is a decision of a person: `LK_MCP_NOTIFY_SERVICES`
(it reaches the config-server's unauthenticated `/update-resources`, Q37), `LK_RETENTION_ENABLED` and `LK_RETENTION_DRY_RUN` (they delete
blobs, D116, D118), `LK_EXTRA_DENY_GLOBS`, `LK_TEAMS_ALLOWED_HOSTS`, `LK_GITHUB_API_BASE`, `LK_ENTRA_AUTHORITY_HOST` (each of the hosts
would need the FQDN egress policy to name it too, gap N1), `LK_ENTRA_REDIRECT_PATH` and `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS`.

`terminationGracePeriodSeconds` is 45, the contract's number. The chart refuses a `LK_SHUTDOWN_GRACE_SECS` and `LK_SHUTDOWN_DRAIN_SECS` whose sum
plus 10 s is more than that (the hub's defaults, 5 and 25, are used for the one that is not set). In the same way it refuses a pod that the
kubelet would kill, whatever the schema allows: a `LK_START_TIMEOUT_SECS` above 50 (the startup probe gives the hub 60 s to answer `/healthz`,
which it does once the database, the schema, the web certificate and both CAs have loaded, and the chart keeps 10 s for the process to begin), and
`LK_BLOB_CACHE_MIB` plus `LK_CORPUS_CACHE_MIB` more than the memory limit's headroom over the request above their defaults (512 and 256 MiB: the
request, 1536 MiB, is the P11 budget at design scale, so with the limit at 2 Gi the two together may be 1280 MiB). **Not verified:** that the P11
memory figure was measured with the default caches; a person who measures the pod should confirm the 768 MiB that the check subtracts.

**Made outside this chart** (this chart creates none of it; `crates/hub/README.md` items 2 to 4 are the checklist). `charts/gcp` makes the
KMS keys, the Secret Manager containers of the table, `write-preview-key`, `github-read-token`, `github-webhook-secret` and
`mcp-token-hmac-key` (read only with `LK_MCP_AUTH=personal_token`), and the roles for the hub's Google service account on each, and
`render.sh contract` compares the names with this chart's (its README lists the steps). What stays a person's: the key *versions* to
put into `config.kms.*`; the values of the secrets (`write-preview-key` is read at start: the hub does not start without it; the mcp key is
at least 32 random bytes as base64url, and the hub does not start in that mode without it); the CA certificates, made once with `hub
ca-init`; the Entra app registration.

## The Jobs

`crates/hub/HARDENING.md` ("The migration Job", "The seed Job") is the contract; `deploy/policy/hardening.rego`, `network.rego` and
`database.rego` hold the rendering to it, and `render.sh planted` shows each rule refusing a change.

| | `hub migrate` | `hub seed` |
|---|---|---|
| Runs | before the hub rolls out: Helm `pre-install` and `pre-upgrade` hook; Argo CD makes it a `PreSync` hook and the weight its sync wave | after the migration, in the sync itself: **a Job, not a hook** (a missing seed must not stop a deploy) |
| As | `lanekeeper_migrator`: the Secret `database.migratorSecretName`, key `uri` only, mode `0440`, at `/etc/lanekeeper/migrator` | `lanekeeper_app`: the Secret the hub mounts (`database.credentialsSecretName`), key `uri` only, at `/etc/lanekeeper/db` |
| Variables | `LK_MIGRATOR_URL_FILE`, `LK_DB_CA_FILE`, `LK_MIGRATE_TIMEOUT_SECS` (`migrate.timeoutSeconds`, 600) | `LK_DATABASE_URL_FILE`, `LK_DB_CA_FILE`, `LK_SEED_TIMEOUT_SECS` (`seed.timeoutSeconds`, 120) |
| Other volumes | `/tmp`, the database CA (`ca.crt` only) | `/tmp`, the database CA, the seed file at `/etc/lanekeeper/seed/seed.yaml` from a ConfigMap |
| Arguments | `["migrate"]` | `["seed", "/etc/lanekeeper/seed/seed.yaml"]` |
| Life | `restartPolicy: Never`, `backoffLimit` 2, deadline = timeout + 300 s | the same |

Both run the StatefulSet's image by the same digest, with the hub pod's security context, `jobs.resources` (not measured), no ports, no
service-account token and no Google identity. Their pods carry `app.kubernetes.io/name: lanekeeper-hub-jobs`, not the hub's label: the hub's
NetworkPolicy does not select them, their own (`lanekeeper-hub-jobs`) admits nothing in and lets out kube-dns and the database on 5432,
and the database chart's policy admits that label on 5432 and nothing wider. The Jobs' account and policy are hooks of the same events with
weight -10 (the Job's is 0), kept after the sync (`before-hook-creation`, never `hook-succeeded`, which would delete them before the seed Job
runs): on a first install a pre-install hook runs before the rest of the chart exists.

**The seed file** names the service account of every swimlane's agent and of every NFS VM's sentinel; until the Job has run with it, no agent can
join. It is a reviewed change, and the real one lives outside this repository (AGENTS.md rule 12): give `seed.existingConfigMap` (a ConfigMap
that holds it under the key `seed.yaml`, made out of band) or, for a synthetic file such as kind's, `seed.content`, which the chart makes a ConfigMap
of. Exactly one. A Job's pod template cannot change, so the Job is named with a short hash of its pod template (the image, the resources, the
timeout, the Secrets), the revision, the file or the name of its ConfigMap, and the number of tries: a new image or file is a new Job, and
Argo CD prunes the old one. A ConfigMap that was edited in place is run again by changing `seed.revision`. `hub seed --dry-run FILE`, run by a
person before the change, prints what it would do (`crates/hub/README.md`).

**Argo CD's handling of the hook annotations** was written from the source of gitops-engine and not read from its documentation
(`argo-cd.readthedocs.io` is not reachable from where this was written): `pre-install` and `pre-upgrade` become `PreSync`; the weight
(`helm.sh/hook-weight`) orders the hooks of a phase, and the chart also sets `argocd.argoproj.io/sync-wave` to the same number, so the order does not
rest on that mapping; `before-hook-creation` is the same policy under Argo CD's name. The first sync is the test of it.

## Values files

`deploy/values/hub/{kind,staging,prod}.yaml`. They hold documentation addresses and example names until Q1, Q2, Q29 and the
GCP project are answered (rule 12), and the image digest is a placeholder of 64 zeros until the first image is published
(plan 09 A29): it cannot be pulled, and the images policy warns about it. The Entra GUIDs, the KMS keys, the audit bucket, the Git
repositories and the agents' host names are EXAMPLE values too; kind's are obviously synthetic and parse, so that the release hub, which
cannot reach Google in kind, stops at its first call to Google (exit 1) and not at a setting (exit 2).

## Checks

```
just deploy-check        # schemas, lint, kubeconform -strict (offline), the policies on the real output, the planted cases
deploy/tests/render.sh policy      # one stage of it
just kind-e2e            # the chart with CloudNativePG on kind (Docker, cgroup v2, network): the Jobs complete, each replica stops at its first Google step
```

`deploy/tests/render.sh planted` changes the real output of the chart one thing at a time (a hardening field, a range, a
host) and requires the policy that guards it to refuse it with its own message. The policies and their unit tests are in
`deploy/policy`; `deploy/schemas` holds the schemas kubeconform reads.

## Rendered with

Helm 4.3.0 (`just ops-tools-install helm`). The output also equals Helm 3.21.4's (checked once, apart from blank lines), so
an ArgoCD that renders with Helm 3 gets the same manifests.

## Not verified here

- the memory and CPU of the Jobs (`jobs.resources`), and that the migration Job finds the database ready on a first install: it waits up to
  `migrate.timeoutSeconds` for a database that CloudNativePG has not made yet, and then fails the sync, which runs again;
- Argo CD's PreSync handling of the hook account and policy as described above;

The sandbox that wrote this has no GKE cluster and could not read Google's documentation (`docs.cloud.google.com` is refused),
so the Google-specific shapes follow the vendored schemas and what is known of GKE, and the first real apply is their test:

- the Gateway's certificate options (`networking.gke.io/pre-shared-certs`, `/cert-manager-certs`) and `type: NamedAddress`;
- `cloud.google.com/l4-rbs` on the agent Service, and that `externalTrafficPolicy: Local` keeps the clients' addresses there;
- that Google's proxies and health checks for `gke-l7-global-external-managed` come from `35.191.0.0/16` and `130.211.0.0/22`
  (plan 09 A10), which the NetworkPolicy relies on;
- that the cluster enforces NetworkPolicy and FQDNNetworkPolicy at all (a property of the cluster, not of the chart);
- the schemas of `GCPBackendPolicy`, `HealthCheckPolicy` and `FQDNNetworkPolicy` come from the community catalog
  (`deploy/schemas/sources.json`), not from Google, and are looser than the others.
