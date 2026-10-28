# Plan 09: the hub chart against prompt 18's contract (inventory only)

Written 2026-10-28 after the merge of `origin/main` (`8019e02`, which holds prompt 18, PR 41, and PRs 40, 42 and 43) into
`agent/09-deployment-ci` as `cb4367c`. **Nothing in `deploy/`, `.github/` or `crates/` was changed to write this.** It lists what
differs between what the hub now requires and what this branch provides, so that a human can decide the order of the work.

**How to read it.** *Sure* means I read it in a file at `cb4367c` or ran it. *Guess* means I inferred it and nothing ran.
`helm`, `kubeconform`, `conftest` and `kind` are not installed in this sandbox, so **no chart was rendered and no policy was run**:
every statement about what the chart renders is from reading the templates, and every statement about what a policy would say is from
reading the Rego (`NOT RUN`). Names are copied from `crates/hub/HARDENING.md` and `crates/hub/README.md`.

**What did run after the merge.** `node --test .github/scripts/*.test.mjs`: 155 of 157 pass; the two failures are named in section 11
and in section 10 (both come from prompt 18's content, neither from a conflict). `cargo test -p hub --test hardening`: 7 of 7.
`cargo test -p hub --test justfile_gate`: 8 of 8.

**Two things to know before the rest.**

1. **The release `hub` binary cannot start without real Google services and Entra, by design** (prompt 18, G23: no fakes in a release
   build). `Edges::google` is the only constructor outside tests and examples, and `GoogleEndpoints::production()` is the only value it
   uses (`crates/hub/src/edges.rs`), so no setting points the production image at a stub. `hub_dev` is an example target and is never in
   the image (`tests/release_build.rs`). Secret Manager (the web certificate and key and the two CA certificates are read at start, in parallel with the database), Cloud KMS (both CAs, the
   audit-checkpoint key, the vault key), Cloud Storage (the checkpoints) and Entra are all reached at start or soon after. So **`/readyz`
   on all 3 replicas, and P11 measured on the pod, cannot be had in kind with the production image.** Section 10 says what the kind job
   can prove instead, and what each way of asserting it costs. The choice is the human's.
2. **The kind job is the `kind-e2e` job of `.github/workflows/images.yml`, not a `kind.yml`** (there is no such file). Its script is
   `deploy/tests/kind/run.sh` with `lib.sh` and `pins.env`; its tests are `.github/scripts/kind-run.test.mjs`; its values are
   `deploy/values/hub/kind.yaml`; the recipe is `just kind-e2e`.

---

## 1. Required environment variables and Secret Manager names

**Gap E1 (sure): the chart renders 12 of the hub's variables; the hub exits 2 without the others.** I compared the names in the Pod
block of `HARDENING.md` (34), the 25 of `REQUIRED_VARIABLES` in `crates/hub/tests/hardening.rs`, and the `name: LK_...` lines of
`statefulset.yaml` (12: `LK_WEB_ADDR`, `LK_OPS_ADDR`, `LK_WEB_DIST`, `LK_TRUSTED_PROXY_HOPS`, `LK_DATABASE_URL_FILE`, `LK_DB_CA_FILE`,
`LK_MIRROR_DIR`, `LK_OTLP_ENDPOINT` when set, and the four `LK_GIT_*` when set). Not rendered, and required by the hub (19):

| Variable | The contract's example in the Pod block | Where the chart would get it (guess) |
|---|---|---|
| `LK_GCP_PROJECT` | `<gcp project id>` | a hub value; the gcp chart's `project` is the same project |
| `LK_WEB_CERT_REF`, `LK_WEB_KEY_REF` | `lanekeeper-web-cert`, `lanekeeper-web-key` | constants, or values |
| `LK_REPLICA_NAME` | downward API `fieldRef: metadata.name` | see section 4 |
| `LK_REPLICA_ADDRESS` | `$(LK_REPLICA_NAME).lanekeeper-hub.lanekeeper.svc:8445` | see section 3 |
| `LK_HUB_AUDIENCE` | `https://<hub hostname>` | `gateway.hostname` |
| `LK_AGENT_ENDPOINT_HOSTS` | `<agent endpoint hostname>` | a new value; must contain the host of the agent chart's `hub.endpoint` |
| `LK_AGENT_CA_KMS_REF`, `LK_REPLICA_CA_KMS_REF` | `<... a Cloud KMS resource name>` (key *versions*, which must differ) | values, or outputs of the gcp chart |
| `LK_AGENT_CA_CERT_REF`, `LK_REPLICA_CA_CERT_REF` | `lanekeeper-agent-ca-cert`, `lanekeeper-replica-ca-cert` (must differ) | constants, or values |
| `LK_PUBLIC_ORIGIN` | `https://<hub hostname>` | `gateway.hostname`; the hub checks `Host` and `Origin` against it |
| `LK_ENTRA_TENANT_ID`, `LK_ENTRA_CLIENT_ID` | GUIDs | values |
| `LK_ENTRA_CLIENT_AUTH_REF`, `LK_ENTRA_COOKIE_KEY_REF` | `lanekeeper-entra-client`, `lanekeeper-login-cookie` | constants, or values |
| `LK_AUDIT_CHECKPOINT_BUCKET` | `<Cloud Storage bucket with a retention lock>` | the gcp chart's `buckets.auditCheckpoint.name` |
| `LK_AUDIT_CHECKPOINT_KMS_REF` | `<audit checkpoint key version>` | a value |
| `LK_VAULT_KMS_REF` | `<vault key ... without a version>` | a value |

The Pod block also carries `LK_AGENT_ADDR` (`0.0.0.0:8444`) and `LK_REPLICA_ADDR` (`0.0.0.0:8445`), which the hub defaults, and the optional
`LK_BOOTSTRAP_ADMINS` (comma separated Entra object ids). The four `LK_GIT_*` are required by the hub, but the chart renders them only when
`config.git.baseUrl` is set (it fails if only some are), and the committed values set none of them, so a chart rendered from `prod.yaml`,
`staging.yaml` or `kind.yaml` has no Git settings either.
**Files:** `deploy/charts/hub/templates/statefulset.yaml`, `values.yaml`, `values.schema.json` (`additionalProperties: false`, so every new
value needs a schema entry), `deploy/values/hub/{kind,staging,prod}.yaml`. Which of these are values and which are constants is a design
choice for the plan.

**Gap E2 (sure): `deploy/tests/env-names.sh` reads only `crates/hub/src/config.rs`** (line 38), and prompt 18 moved settings into
`crates/hub/src/config/*.rs`. I counted the literal names: `LK_SEED_TIMEOUT_SECS`, `LK_BOOTSTRAP_ADMINS`, `LK_MCP_AUTH`,
`LK_ENTRA_AUTHORITY_HOST` and `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS` appear 0 times in `config.rs` and 3 to 13 times in `config/*.rs`. A chart
that sets any of them is called a typo by the check. (`LK_MIGRATOR_URL_FILE` and `LK_MIGRATE_TIMEOUT_SECS` do appear in `config.rs`, so the migration
Job would pass the check by luck.) **File:** `deploy/tests/env-names.sh` (the `check` call and its self-test).

**Gap E3 (sure): `config.tuning` refuses the hub's optional settings.** The schema allows 19 tunables. The hub also reads, and `README.md`
item 7 lists: `LK_START_TIMEOUT_SECS`, `LK_WEB_CERT_RELOAD_SECS`, the MCP group (`LK_MCP_AUTH`, `LK_MCP_CONFIRM`, `LK_MCP_CONTENT`,
`LK_MCP_NOTIFY_SERVICES`, `LK_MCP_AS_MIRROR`), `LK_RETENTION_ENABLED`, `LK_RETENTION_DRY_RUN`, `LK_EXTRA_DENY_GLOBS`, `LK_TEAMS_ALLOWED_HOSTS`,
`LK_GITHUB_API_BASE`, `LK_GIT_BASE_BRANCH`, `LK_CONFIG_SERVER_DEPLOYMENT`, `LK_WRITES_RECONCILE_SECS`, `LK_WRITES_EXPIRE_SECS`,
`LK_WRITES_PR_JOBS_SECS`, `LK_WRITES_CLEANUP_SECS`, `LK_CORPUS_CACHE_MIB`, `LK_ENTRA_AUTHORITY_HOST`, `LK_ENTRA_REDIRECT_PATH`,
`LK_ENTRA_API_AUDIENCES`, `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS`, `LK_GIT_READ_REF`, `LK_GIT_WEBHOOK_REF`. All are optional with defaults, so
this is not a start-up failure; it is that a real deployment cannot set the ones it needs. **A decision, not a fix:** which become first-class
values (guess: `LK_ENTRA_API_AUDIENCES` and `LK_CONFIG_SERVER_DEPLOYMENT` for sure-ish; `LK_MCP_*` by the answer to D52), and whether a policy
pins `LK_MCP_NOTIFY_SERVICES` (reaches the unauthenticated `/update-resources`, Q37) and `LK_RETENTION_ENABLED` (D116, D118) to off in prod and
staging (guess; today the schema makes both impossible to set, which is safe). **Files:** `values.schema.json`, `statefulset.yaml`,
possibly `deploy/policy/hardening.rego`.

**Gap E4 (sure): the Job-only variables must stay out of the pod.** `LK_MIGRATOR_URL_FILE`, `LK_MIGRATE_TIMEOUT_SECS` and
`LK_SEED_TIMEOUT_SECS` are refused by `tests/hardening.rs` for the *contract's* Pod. For the *chart*, the schema's closed list keeps them out of
`config.tuning`, but no Rego rule reads the StatefulSet's env for them (`hardening.rego` and `secrets.rego` do not name them). Adding the Jobs
makes the three names exist in the chart, so a rule would stop a later edit from putting one in the StatefulSet (guess that a rule is wanted).
**Files:** `deploy/policy/hardening.rego`, `hardening_test.rego`, `deploy/tests/planted`.

**Gap E5 (sure): no new name trips `secrets.rego`.** `secret_looking` flags `PASSWORD`, `TOKEN`, `SECRET`, `CREDENTIAL` anywhere and a `_KEY`
suffix. The 19 names end in `_REF`, `_ID`, `_HOSTS`, `_BUCKET`, `_ADDRESS`, `_NAME` or `_AUDIENCE`, so none is flagged (read, not run). `LK_GIT_TOKEN_FILE`
is the only hub name that would be, and the README says not to use it.

### Secret Manager names the hub reads

None exists in this repository's charts (see section 9). Each needs a container, a first version and `roles/secretmanager.secretAccessor`
for the hub's Google service account.

| Secret | Read when | Content | Who makes it (today: nobody in the repo) |
|---|---|---|---|
| `lanekeeper-web-cert`, `lanekeeper-web-key` (`LK_WEB_CERT_REF`, `LK_WEB_KEY_REF`) | start, then every `LK_WEB_CERT_RELOAD_SECS` (300) | PEM chain and key of the web listener (the hop from the load balancer to the pod) | a person; nothing rotates them, and `/readyz` fails once the certificate has expired |
| `lanekeeper-agent-ca-cert`, `lanekeeper-replica-ca-cert` | start | PEM certificate, public, from `hub ca-init` | a person, once for each CA |
| `lanekeeper-entra-client` (`LK_ENTRA_CLIENT_AUTH_REF`) | start | the app registration's client secret | a person |
| `lanekeeper-login-cookie` (`LK_ENTRA_COOKIE_KEY_REF`) | start | 32 random bytes as base64url | a person |
| `github-read-token`, `github-webhook-secret` (`LK_GIT_READ_REF`, `LK_GIT_WEBHOOK_REF` override the names) | Git fetch, webhook check | read-only credential; the webhook secret that GitHub also holds | a person |
| `write-preview-key` | start (the hub does not start without it) | at least 32 bytes | a person |
| `mcp-token-hmac-key` | start, only with `LK_MCP_AUTH=personal_token` | at least 32 random bytes as base64url | a person |
| one per Teams destination | at use | a webhook URL; the name is `notification_settings.destination_ref`, chosen by an admin at run time | an admin |

---

## 2. The migration Job and the seed Job

`HARDENING.md` ("The migration Job", "The seed Job") and `README.md` item 6 are the contract. The image's `ENTRYPOINT` is
`["/usr/local/bin/hub"]` (`crates/hub/Dockerfile:105`), so a Job passes only `args`.

| | Migration Job | Seed Job |
|---|---|---|
| Args | `["migrate"]` | `["seed", "/etc/lanekeeper/seed/seed.yaml"]` (and `--dry-run` for a preview) |
| Variables | `LK_MIGRATOR_URL_FILE`, `LK_DB_CA_FILE`, optional `LK_MIGRATE_TIMEOUT_SECS` (600, at most 7200) | `LK_DATABASE_URL_FILE`, `LK_DB_CA_FILE`, optional `LK_SEED_TIMEOUT_SECS` (120, at most 3600) |
| Volumes | the migrator's Secret (mode `0440`), the database CA (both read-only), `/tmp` the only writable mount | the application role's Secret (mode `0440`, the one the hub mounts), the database CA, the seed file from a `ConfigMap` (all read-only), `/tmp` |
| Network | Postgres on 5432 and DNS; nothing else; no ingress, no ports | the same |
| Order | before the hub rolls: Helm `pre-install` and `pre-upgrade` hook, or Argo CD `PreSync` | after the migration Job; **a Job, not a hook** (README, "The order of a first deployment", step f) |
| Role | `lanekeeper_migrator` (it refuses any other) | `lanekeeper_app` (it refuses any other) |
| Google | none | none |

**Gap J1 (sure): the hub chart has no Job and no ConfigMap.** Its templates are `NOTES.txt`, `_helpers.tpl`, `gateway`, `networkpolicy`,
`poddisruptionbudget`, `serviceaccount`, `services`, `statefulset`, `storageclass`. **Files:** new templates in `deploy/charts/hub/templates/`,
`values.yaml`, `values.schema.json`, the three values files.

**Gap J2 (sure): the migrator's Secret has no connection URL.** `hub migrate` reads a *file that holds the URL*. The db chart's README (lines
35 to 41) and the kind script (`run.sh`, `install_database`) create `lanekeeper-db-migrator` with `username` and `password` only; the app's
Secret has `uri`. A `uri` key for `lanekeeper_migrator` has to be added (guess that a `kubernetes.io/basic-auth` Secret accepts the extra key; the
operator reads only `username` and `password`), and the Job mounts that one key through `items`, as `db-credentials` does.
**Files:** `deploy/charts/db/README.md`, `deploy/tests/kind/run.sh`.

**Gap J3 (sure on the facts, the fix is a design choice): the Jobs' pods fit neither network policy.** The hub's `NetworkPolicy` selects
`app.kubernetes.io/name: lanekeeper-hub`, as `HARDENING.md` fixes, and `deploy/charts/db/templates/networkpolicy.yaml` admits 5432 from pods
with that same label, from the operator and from the instances, and from nothing else. A Job pod that carries the label is selected by the hub's policy and so gets the hub's whole egress list (the other replicas,
the metadata server, the FQDN hosts), which is not "Postgres and DNS only". A Job pod without the label can reach nothing in the database (the
db policy does not admit it) and, selected by no policy at all, is unrestricted in every other direction. So the Jobs need their own label, their
own `NetworkPolicy` (`Ingress` and `Egress` both set; egress `kube-dns` 53/UDP and 53/TCP and `cnpg.io/cluster: lanekeeper-db` 5432/TCP; no ingress),
and the database's policy has to admit that label too. **Files:** new `deploy/charts/hub/templates/` (a policy), `deploy/charts/db/templates/networkpolicy.yaml`
(and its values), `deploy/policy/network.rego` (+ test), `deploy/tests/render.sh` (the db and hub charts are checked against one another).

**Gap J4 (sure): no policy looks at a Job.** The generic rules of `hardening.rego` reach every container, Jobs included (non-root, read-only root,
no capabilities, seccomp, `resources`, no token), because `lk.pod_spec` lists `Job`. Everything specific to the hub (writable volumes, ports, probes,
the `uri` and `ca.crt` items, mode `0440`) keys on `lk.hub_statefulsets`. Nothing checks a Job's four volumes, its variables, its args, that it has no
ports, that it uses the hub's digest, or its `restartPolicy`, `backoffLimit` and `activeDeadlineSeconds`. **Files:** `deploy/policy/hardening.rego`,
`network.rego`, `resources.rego` (the Jobs need `resources`, which no value provides yet), their `_test.rego` files, `deploy/tests/planted`.

**Gap J5 (sure): the Argo CD project does not allow a Job or a ConfigMap.** `deploy/argocd/platform/projects.yaml`, project
`lanekeeper-workloads`, lists `ServiceAccount`, `Service`, `StatefulSet`, `PodDisruptionBudget`, `NetworkPolicy`, `FQDNNetworkPolicy`, `GCPBackendPolicy`,
`HealthCheckPolicy`, `Gateway`, `HTTPRoute`, the two CNPG kinds, and nothing in `batch` or `ConfigMap`. The scope check (`argocd_scope.rego`) renders the charts
against the project, so it would fail on the first Job (guess: `NOT RUN`). **Files:** `deploy/argocd/platform/projects.yaml`, `deploy/policy/argocd.rego`
(its list of what each project may deploy), `argocd_test.rego`, `argocd_scope_test.rego`.

**Gap J6 (guess): hook mechanics the plan has to settle.**
- A Helm `pre-install` hook runs before the chart's other resources exist (Helm behaviour; the same for Argo CD `PreSync`). The Job cannot name the
  chart's `ServiceAccount` on a first install unless that account is itself a hook with a lower weight, or the Job runs as the namespace's `default`
  account (with `automountServiceAccountToken: false` either way). Neither Job needs Google, so the hub's Workload Identity account is not needed.
- On a first install the database is another Argo CD application. The migration Job may start before CloudNativePG has made the cluster, the
  `lanekeeper-db-ca` Secret or the roles. `LK_MIGRATE_TIMEOUT_SECS` covers connecting and waiting, and the Job's `activeDeadlineSeconds` should be longer
  than it (and longer than `LK_SEED_TIMEOUT_SECS` for the seed Job) so that the process's own message is what the log shows.
- Argo CD turns Helm hooks into its own hooks (`pre-install` and `pre-upgrade` into `PreSync`) and has its own delete policies; I could not read its
  documentation here, so the exact annotations are the plan's to verify.
- The seed Job is not a hook, and a `Job`'s pod template is immutable: when the image digest or the seed file changes, the Job needs a new name (for
  example a hash of both) or it must be replaced, or an upgrade fails with an immutable-field error. With `selfHeal: true` (`deploy/argocd/platform/hub.yaml`) a
  finished Job that is not removed is not re-run, which is wanted.

**Gap J7 (sure on the fact, a decision on the shape): where the seed file comes from.** The real file lists service accounts and lives outside this
repository (AGENTS.md rule 12; README: "the real file lives outside this repository"). The chart can take an existing `ConfigMap` by name (like the
database Secrets, created out of band) or a value that it renders as one. `crates/hub/examples/seed.dev.yaml` is the synthetic file that kind can use.

---

## 3. Ports, the replica Service and `LK_REPLICA_ADDRESS`

**Matches (sure):** the container ports are `web` 8443, `agent` 8444, `replica` 8445, `ops` 9090 (`_helpers.tpl`; `hardening.rego` holds exactly these). The agent
Service is a `LoadBalancer` on 443 forwarding to `agent` (8444); the replica Service is headless `lanekeeper-hub-replicas` on 8445; the StatefulSet's
`serviceName` is `lanekeeper-hub-replicas`. The sentinel reaches the same 443 (B7, resolved).

**Gap P1 (sure): the name in the contract's example is not the chart's.** `HARDENING.md` writes `LK_REPLICA_ADDRESS` as
`$(LK_REPLICA_NAME).lanekeeper-hub.lanekeeper.svc:8445`, which names a headless Service called `lanekeeper-hub` in the namespace `lanekeeper`. The chart's is
`lanekeeper-hub-replicas`, and `topology.rego` (line 10, `headless_name`) requires that name. `tests/hardening.rs` accepts any service name (it checks that the
value starts with `$(LK_REPLICA_NAME).` and ends with `:8445`), and `README.md` says `<headless service>`. So the chart's value has to be
`$(LK_REPLICA_NAME).lanekeeper-hub-replicas.{{ .Release.Namespace }}.svc:8445`, and the example in `HARDENING.md` is a stale spelling. The host is the DNS name
on the replica certificate, so a different name from the one the StatefulSet's DNS makes (`<pod>.<serviceName>.<namespace>.svc`) breaks the name check between
replicas. **Files:** `statefulset.yaml`; the contract example (`crates/hub/HARDENING.md`) is prompt 18's file.

**Gap P2 (sure, Kubernetes behaviour): variable order.** Kubernetes expands `$(NAME)` only for variables defined earlier in the same list, so `LK_REPLICA_NAME`
must come before `LK_REPLICA_ADDRESS` in `env`. **File:** `statefulset.yaml`.

**Gap P3 (guess): `publishNotReadyAddresses` on the headless Service.** A pod's own DNS name resolves only while it is ready. A replica that has failed `/readyz`
(it does so first on SIGTERM) disappears from DNS while it may still hold agent connections for another replica to forward commands to. Whether that matters
depends on how long forwarded commands take against the 5 s grace; I did not trace it. **File:** `services.yaml`.

**Gap P4 (guess): nothing ties the agent chart's endpoint to `LK_AGENT_ENDPOINT_HOSTS`.** The agents verify the hub's certificate by the names in
`LK_AGENT_ENDPOINT_HOSTS`, and the host of the agent chart's `hub.endpoint` (`values/agent/sit1.yaml`: `agents.lanekeeper-prod.example.com`) has to be one of them.
`render.sh contract` compares the db, gcp and hub charts (bucket, service account, disk keys), not this. **Files:** `deploy/tests/render.sh`, the values files.

---

## 4. `LK_REPLICA_NAME` from the downward API

**Sure:** no conftest rule forbids `valueFrom: fieldRef`. `deploy/policy/secrets.rego` (lines 59 to 70) refuses `valueFrom.secretKeyRef` and `envFrom.secretRef`, and nothing else
in `deploy/policy`, `deploy/tests` or `deploy/charts` mentions `valueFrom` or `fieldRef` (grep); the only test is `secrets_test.rego:54`, a planted `secretKeyRef`. The
rule that forbids every other reference is in the *contract test*: `tests/hardening.rs` allows `valueFrom` only on `LK_REPLICA_NAME` and only as exactly
`fieldRef: {fieldPath: metadata.name}`.

**Gap R1 (sure):** the chart has no `LK_REPLICA_NAME`. **Gap R2 (sure):** the chart is not held to the contract's rule; once `valueFrom` exists in the template, any
variable could take a `fieldRef`, a `configMapKeyRef` or a `resourceFieldRef` and pass. A rule for the StatefulSet (and the Jobs) that allows exactly the one
`fieldRef` on exactly that variable, with a planted case, would close it (guess that this is wanted). **Files:** `statefulset.yaml`;
`deploy/policy/secrets.rego` or `hardening.rego` and the tests.

---

## 5. `terminationGracePeriodSeconds`

**Differs (sure):** the chart has **60** (`statefulset.yaml:48`, "Drain (25 s) and grace (5 s) ... with room"); the Pod block and `README.md` item 5 say **45**.
The contract test only requires `at least grace + drain + 10` (`hardening.rs:430`), which is 40, so 60 passes it and nothing is red. It is a difference between
the chart and the contract's written number, for a human to settle one way (the chart to 45, or the contract to say what the chart does).

**Gap T1 (sure): nothing keeps the grace period above the shutdown tunables.** The schema lets `config.tuning` set `LK_SHUTDOWN_GRACE_SECS` (0 to 60) and
`LK_SHUTDOWN_DRAIN_SECS` (1 to 120), and the template has no check that `grace + drain + 10` is not more than 60. The contract test has a planted case for exactly
this ("a longer drain than the termination period allows"); the chart has none. **Files:** `statefulset.yaml` (a `fail`) or `hardening.rego`, with a planted case.

**Gap T2 (sure on the numbers, guess on the effect): the load balancer may keep sending after the hub stops accepting.** On SIGTERM the hub fails `/readyz`, waits
`LK_SHUTDOWN_GRACE_SECS` (5) "so that the load balancer notices", then stops accepting. The chart's `HealthCheckPolicy` (`gateway.yaml`) checks every 5 s and needs 3
failures: 15 s to notice. For about 10 s the load balancer may still route new connections to a replica that has stopped accepting. GKE removes a terminating pod's endpoint
from the network endpoint group, and `GCPBackendPolicy` has a `connectionDraining` field (vendored schema), so the real effect may be smaller; I did not check. Options:
a larger grace (a tunable plus the period above), or a shorter check. **Files:** `gateway.yaml`, `statefulset.yaml`, values.

---

## 6. Probes

**Matches (sure):** liveness `/healthz` and readiness `/readyz` on `ops`, with the same periods and thresholds as the Pod block, and a `startupProbe` on `/healthz` (2 s period, 30 tries)
that both have. `resources.rego` requires the startup probe and compares its window with P11.

**Gap B1 (sure): the stated reason for the startup probe is no longer true of the hub.** `HARDENING.md` ("Probes", lines 215 to 218) and `statefulset.yaml:94-95` say `/healthz`
"does not depend on the database: a database outage must never look like a failed start". In `crates/hub/src/process.rs` the operations socket is bound first (`Sockets::bind`) but is handed
to `serve_hub` only after `load` (database, schema, web certificate, both CAs) and `compose` have finished. Until then nothing answers `/healthz`: the probe fails, and a database that
cannot be reached at start is an exit 1 (CrashLoopBackOff), not an unready pod. The probe is still useful (without it, the liveness probe would judge a start that has not finished),
and the 60 s window is not affected (each step is bounded by `LK_START_TIMEOUT_SECS`, 8). The paragraph and the comment are what is stale. **Files:** `crates/hub/HARDENING.md` (prompt 18's
file, so through the plan's Extra-paths), `statefulset.yaml`.

**`/readyz` (sure, `README.md` item 11 and `crates/hub/src/ready.rs`):** 200 when `postgres`, `schema`, `certificates` (the web, agent and replica certificates held are unexpired), `events`, `mirrors`,
`jobs` and `tasks` all hold, 503 naming the first that does not; a ready body may add `(not available: docs_search)` and never gates. The whole check has a 2 s deadline
(`ops::READY_DEADLINE`), inside the readiness probe's 3 s and the health check's 3 s. All of it is answered from memory, so the probe makes no call. Consequences for the chart:
- **`schema`** needs the migration Job to have run. A hub on an unmigrated database exits 1 at start and names the first missing migration. The `schema` check is made with
  `lanekeeper_app`, which cannot read `_sqlx_migrations`, so it looks for one marker object per migration: the app role must have the grants on those objects. That is the
  db chart's `postInitApplicationSQL` and default privileges (plan T5 says they follow `deploy/dev/init.sql`), and only a run shows whether they cover the 8 migrations (guess).
- **`mirrors`**: a replica on an empty volume is not ready until both repositories are fetched. At a first install all three replicas fetch from GitHub through `github.com` and
  `api.github.com` (in the egress list) with `github-read-token`. The readiness is therefore bounded by GitHub, not by P11.
- **`certificates`**: the web certificate's expiry is a rotation chore (section 1) that fails readiness on all replicas at once.

---

## 7. Egress and ingress

**Matches (sure, by comparing the Pod's NetworkPolicy block with `networkpolicy.yaml`):** ingress web 8443 from `35.191.0.0/16` and `130.211.0.0/22`; agent 8444 from
`network.agentSourceRanges`; replica 8445 from the hub's pods; 9090 from `monitoring` and from the two Google ranges. Egress: `kube-dns` 53/UDP and TCP; `cnpg.io/cluster` 5432;
the hub's pods 8445; `169.254.169.254` 80; the OTLP collector 4317 (the chart renders it only when `config.otlpEndpoint` is set, which is plan 09 A30, not a new gap); and 443 to
`login.microsoftonline.com`, `github.com`, `api.github.com`, `secretmanager.googleapis.com`, `cloudkms.googleapis.com`, `www.googleapis.com`, `storage.googleapis.com` and the Teams host
when `network.teamsWebhookHost` is set. Prompt 18's `ASSEMBLY_HOSTS` and this branch's `EXPECTED_HOSTS` name the same extra host, `storage.googleapis.com`, and the test takes the union.

**Gap N1 (sure): two conditional hosts cannot be expressed.** `LK_GITHUB_API_BASE` (a GitHub Enterprise Server) and `LK_ENTRA_AUTHORITY_HOST` (a sovereign cloud) each need the FQDN policy to
name the new host ("the FQDN egress policy must name it too", `HARDENING.md`). The chart has one value, `network.teamsWebhookHost`, and nothing for these. Only matters if either
setting is used (E3). **Files:** `networkpolicy.yaml`, `values.yaml`, `values.schema.json`, `network.rego`.

**Gap N2:** the Jobs' egress, ingress and label: gap J3.

---

## 8. Load balancer and Cloud Armor

What `README.md` item 9 and `HARDENING.md` ask of the users' load balancer, against `gateway.yaml` and `cloudarmor.yaml`:

| Requirement | The chart now | Verdict |
|---|---|---|
| Request timeout no shorter than `LK_REQUEST_TIMEOUT_SECS` (30 s) | `GCPBackendPolicy` `timeoutSec: 3600`; the schema's minimum is 30 | **meets (sure)**; also long enough for a stream |
| Bodies of 12 MiB + 64 KiB on `PUT /api/v1/swimlanes/{id}/files` and `POST /api/v1/drafts` (D137) | nothing sets or tests a body limit | **unproven (guess):** I do not know whether the global external Application Load Balancer limits a request body. The Cloud Armor rules here are IP ranges and one path (sure), so Armor does not inspect a body |
| Keep an MCP client on one replica by `Mcp-Session-Id` | none: no `sessionAffinity` is set (so `NONE`) | **cannot be expressed with this object (sure):** the vendored `GCPBackendPolicy` schema (`deploy/schemas/crds/networking.gke.io/gcpbackendpolicy_v1.json`) lists `sessionAffinity.type` as `CLIENT_IP`, `GENERATED_COOKIE`, `HEADER_FIELD`, `HTTP_COOKIE`, `NONE`, and the object has only `type` and `cookieTtlSec`. `HEADER_FIELD` exists, but there is no field to name the header. Whether the load balancer can hash on `Mcp-Session-Id` at all is the open point `HARDENING.md` already records. Without it a request that lands on another replica is answered 404 and the client re-initialises: with three replicas that is two requests in three |
| Pass `Cookie`, `Origin`, `X-CSRF-Token` unchanged | the `HTTPRoute` has no header filter (sure) | **meets by absence (guess):** I believe the load balancer forwards request headers as they are |
| Do not cache `/api` or `/auth` | no CDN is configured, and the vendored `GCPBackendPolicy` has no CDN field (sure) | **meets by absence (guess), asserted by nothing:** no policy fails a chart that turns caching on by another route |
| Do not buffer or cut `text/event-stream` before the 15 s keep-alive | `timeoutSec: 3600` | **unproven (guess)** |
| Two proxies behind `LK_TRUSTED_PROXY_HOPS` | `config.trustedProxyHops: 2` (prod, staging), 0 in kind | **meets (sure)**; the value must be re-checked on the real path |
| Serve the name users type, which is `LK_PUBLIC_ORIGIN` | `gateway.hostname` exists; `LK_PUBLIC_ORIGIN` does not | **gap E1:** one name must feed both, and nothing checks that |
| Health check on `/readyz` of the ops port | `HealthCheckPolicy` HTTP, port 9090, `/readyz`, every 5 s, 3 failures | **meets (sure)**; the interplay with shutdown is T2 |
| The agent load balancer passes TCP to 8444 and does not terminate TLS | `Service` `LoadBalancer`, 443 to `agent` | **meets (sure)** |

**Files if any changes:** `gateway.yaml`, `values.yaml`, `values.schema.json`, `deploy/policy/network.rego` (+ test), `crates/hub/HARDENING.md` for the affinity wording. The first real apply is the only test of the guesses
(the hub chart's README already says so for the other Google-specific shapes).

---

## 9. IAM and Secret Manager objects: the gcp chart or the install guide

**Exists (sure):** `IAMServiceAccount` `lanekeeper-hub` and `lanekeeper-db`; Workload Identity bindings; on the audit-checkpoint bucket `roles/storage.objectCreator` and `roles/storage.objectViewer`
for the hub and no more (this is what `HARDENING.md` asks for: not `objectAdmin`, no delete); `roles/storage.objectUser` on the docs bucket; `roles/cloudtrace.agent`; key grants for the buckets and the disks.
The retention period and the prod lock are set.

**Gap I1 (sure): nothing for KMS, Secret Manager or their IAM.** `deploy/charts/gcp/README.md` says so: the signing keys "and the hub's Secret Manager and KMS bindings are not in this commit: their names are
the hub's configuration, which is not composed yet (plan 09 A1)". The names are now known, and plan 09 A4 already lists these keys as the gcp chart's. To create or document:

| Object | Name or setting | Kind | Note |
|---|---|---|---|
| agent CA key | `LK_AGENT_CA_KMS_REF`, a key *version*; algorithm `EC_SIGN_P256_SHA256`, asymmetric signing | Config Connector `KMSKeyRing`, `KMSCryptoKey` (CRD schemas are vendored in `deploy/schemas/crds/kms.cnrm.cloud.google.com`) | used for nothing else; must differ from the replica CA's |
| replica CA key | `LK_REPLICA_CA_KMS_REF`, same algorithm | same | |
| audit checkpoint key | `LK_AUDIT_CHECKPOINT_KMS_REF` (+ older versions in `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS`) | same | |
| vault key | `LK_VAULT_KMS_REF`, a crypto key *without* a version, symmetric | same | wraps the GitHub token data keys (S8) |
| `roles/cloudkms.signerVerifier` | on the two CA keys and the checkpoint key | `IAMPolicyMember` for the hub's account | Cloud KMS grants IAM on a key or a key ring, not on a version (guess; I could not read Google's documentation), so "on the key version" in `HARDENING.md` means "on the key that holds it" |
| `roles/cloudkms.cryptoKeyEncrypterDecrypter` | on the vault key | same | and nothing more there |
| `roles/secretmanager.secretAccessor` | on each secret of section 1, one binding per secret | same | `iam.rego` refuses a role on the whole project (but Cloud Trace) |
| Secret containers | the names in section 1 | Config Connector `SecretManagerSecret` (schema vendored) | "containers only, never values" (README of the gcp chart) |

- **`deploy/policy/iam.rego`** lists the roles and the resource kinds it allows ("each on the one resource kind it is for"), and has to learn these. **`deploy/argocd/platform/projects.yaml`**, project
  `lanekeeper-cloud`, allows `StorageBucket`, `IAMServiceAccount`, `IAMPolicyMember`, `ComputeSecurityPolicy` and the three Binary Authorization kinds; it has no KMS or Secret Manager kind.
- **Teams secrets** are named at run time by an admin, so a per-secret binding cannot be rendered ahead. Either an IAM condition on a name prefix (guess) or a step in the admin's runbook. A decision.
- **A person** runs `hub ca-init` and needs `roles/cloudkms.publicKeyViewer` and `roles/cloudkms.signerVerifier` on that one key. `iam.rego` refuses a person, so this is documentation, not a manifest.
- **Document, not render:** the Entra app registration (`README.md` item 10: single tenant; redirect `LK_PUBLIC_ORIGIN` + `/auth/callback`; client secret in Secret Manager; for MCP the application ID URI,
  scope `mcp.access`, `accessTokenAcceptedVersion: 2`, Cursor's client ID pre-authorized); the first administrators in `LK_BOOTSTRAP_ADMINS` (and removing them later); the web certificate and key;
  the GitHub webhook and the read-only credential; the order of a first deployment (`README.md` item 2, steps a to f).
- **There is no install guide** (sure: `deploy/` has `ADDING-A-SWIMLANE.md` for the agent side, `argocd/README.md`, and the chart READMEs). Prompt 18's README calls this list "the checklist that the deployment (prompt 09's chart and its install guide) is matched against".

---

## 10. The kind job: what it asserts now, and what it can and cannot prove with the production image

### What it does now (sure)

`kind-e2e` builds nothing: it takes the scanned hub image, makes a cluster of one control plane and three workers, installs CloudNativePG, the db chart (with the two Secrets) and then the hub chart with `values/hub/kind.yaml`.
Stages: `registry`, `cluster`, `hub image`, `operator`, `database`, `s6`, `hub`. The first six passed on a runner (run 40374410436). The `hub` stage is written around the old stub:
- `HUB_REFUSAL_TEXT` (`deploy/tests/kind/lib.sh:109`) is `this build has no identity module (prompt 03a) and no registry (prompt 05) linked in; refusing to start without them`. **`crates/hub/src/main.rs` no longer
  contains it**, so `kind-run.test.mjs:408` ("the refusal that run.sh looks for is the one that crates/hub/src/main.rs logs") is red now.
- With the real binary no replica can be classified `refused`; `hub_replica_class` returns `other` and the verdict is "not A1: the log of ... shows another error". The stage stays red, for a reason the script has no word for.
- 38 lines of `kind-run.test.mjs` (tests at lines 77, 413 to 460, 486 to 497), the section "Where the hub stage stands" of `deploy/tests/kind/README.md`, and the header and line 213 of `images.yml` all describe the A1 refusal.
- It never runs a migration (the schema does not exist in the test's database), so a real hub would exit 1 at the `schema` check; it creates the migrator's Secret without a `uri` (gap J2); it sets none of the 19 settings (gap E1).

### What the `hub` stage can prove with the production image

If the chart gains the settings (E1), the migration Job and the seed Job (J1 to J5), the stage can show, **with `hub migrate`, `hub seed` and `hub serve` of the image that would ship**:

| It can prove | Why, from the code |
|---|---|
| The migration Job completes (exit 0) against CloudNativePG: TLS 1.3 with the server verified against the mounted `lanekeeper-db-ca` `ca.crt`, the `uri` key, role `lanekeeper_migrator`, the extensions `vector` and `pg_trgm`, all 8 migrations (`db/migrations/0001_init.sql` to `0008_blob_touch.sql`) | `hub migrate` needs only the two files; no Google. Sure from the code and the README; not run here |
| The seed Job completes as `lanekeeper_app` from a ConfigMap (synthetic `examples/seed.dev.yaml`) and a second run changes nothing | `hub seed` needs only the two files; no Google |
| Each replica reads its settings and accepts them: **exit 1, not exit 2**. The 25 required variables are present and valid in the pod | `settings.require_serving()` runs first and a missing or malformed setting is exit 2 (`process.rs`, `StartError::exit_code`) |
| The four listeners bind in the pod (8443, 8444, 8445, 9090) | `Sockets::bind` runs before `load` |
| The database is reachable from the pod through the hub's NetworkPolicy (where kindnet enforces it) with full TLS verification, and the schema check passes as `lanekeeper_app` after the Job | `load` runs `db::connect` and `schema::verify` in parallel with the Google reads, and logs every failure (`"the hub could not start"`), returning the first in the order database, web certificate, CAs. A log with a Google error and no database or schema error shows both passed |
| Then the replica refuses at the first Google call with a typed message that names the component and the variables: `the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF): ...`, with the CA failure logged beside it | `StartError::WebCertificate`; messages "name the setting or the component and never repeat a value" (README). **Guess on the exact text:** kind has no metadata server at `169.254.169.254`, and the text depends on whether that fails fast or runs to `LK_START_TIMEOUT_SECS` (8), where the error would be `starting took too long: web certificate`. Not run |
| The pod contract holds on a real kubelet: non-root, read-only root filesystem, no token, the four volumes mount (the `items` of the Secrets, `defaultMode: 288`, `fsGroup`), `LK_REPLICA_NAME` is the pod's name | a start that gets as far as the database proves the mounts; `LK_REPLICA_NAME` is only used after the CAs, so it is **not** proven by this |

### What it cannot prove

- **`/readyz` 200 on 3 replicas, and a pod Ready at all.** The operations listener serves only after the whole start (gap B1), and the start needs Secret Manager, KMS and Entra.
- **P11 on the pod** (10 s per replica), the readiness probe, the startup probe succeeding, the PodDisruptionBudget, a rolling update, the SIGTERM drain with `terminationGracePeriodSeconds` (the hub must be serving to drain).
  P11 for the composition with software edges does have a number: `crates/hub/benches/startup.rs` (`hub/startup`), judged by `bench-check`. What only a cluster adds is probe timing and real Google latency.
- Workload Identity, any IAM role, KMS signing, Secret Manager reads, Cloud Storage retention, Entra sign-in, the Gateway, Cloud Armor, the health check, the FQDN policy (`kind.yaml` sets `fqdnPolicy: false`), the zone spread (`topology.mode: kind`).
- The `mirrors` check and Git fetch (needs GitHub and a credential), the headless-service names between replicas and the replica certificate's name, `LK_REPLICA_NAME` in the identity, the `schema` and `certificates` parts of `/readyz` as served.
- That kindnet enforces NetworkPolicy at all (already listed as unproven in `deploy/tests/kind/README.md`).

### The ways to decide what the kind job asserts (no pick)

| # | Option | What it gives | What it costs |
|---|---|---|---|
| 1 | **Assert the typed refusal.** The hub stage passes when the migration and seed Jobs complete and each replica exits 1 with a Google refusal and no database, schema or setting error. P11 stays `NOT MEASURED`, and acceptance 3 stays unticked. | Everything in "can prove", on every PR that touches `deploy/**`, with the production image. | Rewrite the verdict code of `run.sh` and `lib.sh` and the 38 lines of tests that encode A1; couple the script to prompt 18's message text (needs a replacement for the `main.rs` check, e.g. against `process.rs`); the exact text depends on kind's metadata-server behaviour (unrun). A green stage next to an unmet acceptance 3 must say so in the summary. Needs the E1 and J1 to J5 work first. |
| 2 | **Leave the stage red and reword it.** It keeps failing, now with "the production image refuses without Google", and the earlier stages carry the weight. | Cheapest; nothing is claimed that is not shown. | A permanently red job teaches people to ignore it and can hide a regression in the earlier stages unless they are split out. Still needs the settings and Jobs, or the failure says "setting missing" (exit 2) and proves nothing. |
| 3 | **Split the job.** `kind-e2e` becomes green for registry, cluster, operator, database, S6, migrate and seed (and the refusal check of option 1, if wanted); readiness and P11 move to a separate job that is manual or nightly against a real project. | A job that can go green, and a place for the real proof. | Two jobs to keep; the second needs option 5. |
| 4 | **A test-only image built from the `hub_dev` example**, run in kind, to measure readiness and P11 of the composition without Google. | `/readyz` ×3 and a pod-level P11 in kind. | A second binary that is not the one that ships, so the P11 is not the production number, and it goes against the intent of G23 (the example is never in an image); needs a second Dockerfile or target, its policies (`dockerfile.rego` and the images policy), and a rule that it is never published. A human would have to accept the exception. |
| 5 | **A real Google project** (a GKE test cluster; kind has no Workload Identity, and a service-account key file would break S7) with a real Entra test tenant, Secret Manager, KMS and a bucket; GitHub Actions authenticates through Workload Identity Federation. | The only way to show `/readyz` on 3 replicas, P11 with real Google latency, the load balancer and the IAM roles of section 9. | The most: a project and a cluster to pay for and keep, secrets and a federation in CI, Entra app registrations, all the objects of section 9 created for real, and a decision where its results sit in the PR gate. |
| 6 | **Make Google's endpoints settable in the release binary** so a stub in the cluster can answer. | `/readyz` ×3 with the production image. | Changes G23 and a decision in `docs/decisions.md` by a human; adds a way to point a production hub at another endpoint, which is a security change (S5, S7, S18) and prompt 18's to make, not this plan's. Listed because it is the only option that keeps one image, not because it is advised. |

---

## 11. What else the merge brought that is red or stale

- **`hub-fuzz` is not in the fuzz matrices (sure).** Prompt 18 added the Justfile recipe `hub-fuzz` (targets `hub_cli_args hub_pem hub_seed_file hub_settings`). `consistency.test.mjs:71`
  ("every fuzz recipe of the Justfile is in the fuzz matrix of rust.yml and nightly.yml") fails: the matrix has `fuzz-smoke`, `agent-fuzz`, `identity-fuzz`, `platform-fuzz`, `engine-fuzz`, `registry-fuzz`,
  `mcp-fuzz`, `writes-fuzz`. The fix is one line in each of `.github/workflows/rust.yml` (the list at lines 178 to 186) and `.github/workflows/nightly.yml` (29 to 37). I did not make it: this step
  was told not to change workflow files, and the test is not to be weakened. The recipe runs `cargo deny` and `cargo audit` on `crates/hub/fuzz` and needs the nightly toolchain and cargo-fuzz that the other
  fuzz jobs install (guess that the existing install step covers it; not run).
- **The kind refusal check is red (sure):** section 10.
- **`HARDENING.md` "Probes"** paragraph: gap B1. **The `LK_REPLICA_ADDRESS` example's service name:** gap P1. **`deploy/charts/gcp/README.md`'s sentence that the hub is not composed:** gap I1.
- **`crates/hub/HARDENING.md` is the file with the 09 paragraphs in it.** The merge kept this branch's paragraphs on the startup probe, the `volumeClaimTemplate`, the Google ranges and the two Secret `items`, inside prompt 18's file. They agree with the rest
  except B1.

---

## 12. B8 (the agent's config-server egress): unblocked by PR 42

**B8 (config-server egress needs `namespaceSelector` on `latest-pci` + `podSelector` `app=csp-configuration-server`, and `LK_CONFIG_SERVER_URL` spelt out as
`<service>.<namespace>.svc.cluster.local`) is now unblocked, because PR 42 is on `origin/main` and is in this branch since `cb4367c`.** `crates/agent/HARDENING.md` now requires one explicit
namespace chosen by `kubernetes.io/metadata.name`, in the same `to` entry as the pods, never `namespaceSelector: {}`, never an expression or another label, never `kube-system`,
`kube-public` or `kube-node-lease`; its example URL is `http://csp-configuration-server.latest-pci.svc.cluster.local:8888`. The agent chart still renders the old shape, and two policies require it:
`network.rego` (line 671, `agent_destination` "config-server") classifies a rule as the config-server's only when `not d.namespaceSelector`, so the contract's own shape would be an unknown destination, and
`contract.rego` compares destinations that are not an `ipBlock` literally (`normal_destination`), so the agent chart is already different from the contract on both counts (sure by reading; `NOT RUN`).

Files that B8 touches (not changed in this step):
- `deploy/charts/agent/templates/networkpolicy.yaml` (add the `namespaceSelector` to the config-server entry);
- `deploy/charts/agent/templates/deployment.yaml` (line 131, `LK_CONFIG_SERVER_URL`, today `http://csp-configuration-server:8888`);
- `deploy/charts/agent/values.yaml` and `values.schema.json` (a per-swimlane namespace, and the service name if it can differ; the chart sets no `LK_CONFIG_SERVER_DEPLOYMENT`, which is `namespace/name` and whose namespace the agent requires to be in `LK_NAMESPACES`);
- `deploy/values/agent/sit1.yaml` (and each later swimlane file);
- `deploy/policy/network.rego` (the "config-server" classification and a rule for the allowed namespaces) with `network_test.rego` and `fixtures_agent_test.rego`;
- `deploy/policy/contract.rego` (the namespace in the contract's example is `latest-pci`, a value: `normal_destination` has to treat it as one, or the sit1 chart can never equal it) with `contract_test.rego`;
- `deploy/ADDING-A-SWIMLANE.md` (the runbook must ask for the config-server's namespace);
- `deploy/tests/planted` (a planted wide namespace, and a missing one);
- `plans/09-deployment-ci.blockers.md` (the B8 section, at the next step).

---

## Order of work, only as a suggestion of dependencies (not a plan)

The settings (E1) and the Jobs with their network and policy pieces (J1 to J5) are needed by everything in section 10. The kind decision (section 10) can be taken before them. The gcp objects (I1) and the
install guide are independent of the chart and are the longest to review. B8 is independent of the hub chart.

---

## Status (written after the two fix commits)

`C1` is `30b53a1` (`09/fix: the hub chart renders every setting prompt 18 requires`), `C2` is `66ba049` (`09/fix: migration Job and seed Job for the hub chart`).
This time `helm` 4.3.0, `kubeconform`, `conftest` 0.71.1, `shellcheck` and `actionlint` ran (there is no Docker daemon: a stand-in runs the pinned conftest binary), so
the statements below about the chart are from a render, not from reading. `kind` did not run (no Docker), and the hub binary did not run at all.

### Closed

| Gap | Commit | How it is shown |
|---|---|---|
| E1 (the 25 variables, the Secret Manager names, the derived origin and audience) | C1 | `render.sh contract` reads `REQUIRED_VARIABLES` from `crates/hub/tests/hardening.rs` and finds each in the StatefulSet of kind, staging and prod; `planted_hub_variables` drops each of the 25 in turn and the check names it |
| E2 (`env-names.sh` reads `config/*.rs`) | C1 | its self-test refuses a name no file knows and accepts one that only the directory knows |
| E3 (the tunables, `config.bootstrapAdmins`) | C1 | the schema takes the ones asked for within the hub's ranges; each of the ten that stay out is refused by a planted case; the audiences need an `api://` entry in OAuth mode (the hub refuses a list without one) |
| E4 (the Jobs' variables out of the StatefulSet) | C1 | `hardening.rego`, three planted cases |
| E5 (no new name looks like a secret) | C1 | `env-names.sh` and `secrets.rego` run over the new names and pass |
| P1, P2 (the replica address, the order of the two variables) | C1 | `topology.rego`: the address is `$(LK_REPLICA_NAME).lanekeeper-hub-replicas.<namespace>.svc:8445` and the name comes first; planted cases for the contract's example service, another namespace, and the swapped order |
| P4 (the agent's endpoint host in `LK_AGENT_ENDPOINT_HOSTS`) | C1 | `render.sh contract`, four planted cases |
| P5 (new: the agent's audience is the hub's, `LK_HUB_AUDIENCE`) | C1 | the agent chart's `hub.audience` defaults to the endpoint, which is not the hub's name (`https://<gateway.hostname>`), so every `Join` would have been refused; `sit1.yaml` now sets it, and `render.sh contract` checks it |
| R1, R2 (`LK_REPLICA_NAME`, the one `valueFrom`) | C1 (C2 for the Jobs) | `hardening.rego`: only `fieldRef: {fieldPath: metadata.name}` on `LK_REPLICA_NAME`, in the StatefulSet and the hub's Jobs; planted `configMapKeyRef`, `resourceFieldRef`, a fieldRef on another variable and to another field |
| T1 (45 s, and the sum of the shutdown tunables) | C1 | template `fail`, `hardening.rego` on the rendered output, planted cases for both |
| B1 (the stale comment in `statefulset.yaml`) | C1 | the same stale sentence in `deploy/policy/resources.rego` is corrected too |
| J1, J6 (the migration Job as a hook, with its account and policy) | C2 | rendered; `hardening.rego` holds the hook events, the weights and the delete policy |
| J2 (the migrator's `uri`) | C2 | `deploy/charts/db/README.md`, `deploy/tests/kind/run.sh` `install_database`; `render.sh contract` compares the Secret names of the Jobs with the database's bootstrap and managed role |
| J3 (label, policy, the database admits it) | C2 | `network.rego`, `database.rego`, `render.sh contract` |
| J4 (rules for the Jobs) | C2 | 71 `_test.rego` cases in `hardening_test.rego` (and more in `network_test.rego`, `database_test.rego` and the two Argo CD tests), and 53 planted cases on the real output |
| J5 (the Argo CD project) | C2 | `projects.yaml`, `argocd.rego`, the tests, and three planted scope cases (without `batch/Job`, without `ConfigMap`, a `CronJob`) |
| J7 (the seed file) | C2 | `seed.existingConfigMap` or `seed.content`; the Job's name is a hash, shown to change with the file, revision, image, timeout, resources and ConfigMap |
| (new) the `Job` schema | C2 | `kubeconform -strict` had no schema for `Job`: `job-batch-v1.json` is vendored from the pinned commit with `fetch.py` (nothing else changed in the lock), and a planted `Job` is refused |
| N2 | C2 | it is J3 |

### Left as known gaps, on purpose

- **P3** (`publishNotReadyAddresses` on the headless Service), **T2** (the load balancer's health check against the shutdown), **N1** (the egress policy for `LK_GITHUB_API_BASE` and
  `LK_ENTRA_AUTHORITY_HOST`, which are not settable for that reason), and **section 8** (the load balancer): not touched, as asked. The install guide lists them.
- `LK_MCP_AS_MIRROR`, `LK_GIT_READ_REF` and `LK_GIT_WEBHOOK_REF` are still not settable from a values file (the hub's defaults, `github-read-token` and `github-webhook-secret`, apply). Nobody asked for them.
- **B8** (the agent's config-server egress). It is why `render.sh contract` and `render.sh planted` are red at this HEAD and were red at `1f31164`: the agent chart
  against `crates/agent/HARDENING.md` is one failure, and it makes the unchanged-agent check and 11 agent variants `PLANTED FAILED` lines. With the one line for Cloud Armor
  (B5: the gcp chart cannot render, so its planted cases are not run) that is the 13 that `render.sh planted` reports. They are the same lines before and after both commits (diffed).
- **B5** (GitHub's hook ranges): the gcp chart with Cloud Armor cannot render, so `render`, `policy` and `planted` show the gcp staging and prod failures, as before.
- **Section 10** (the kind job) and **section 11** (`hub-fuzz` in the matrices, the kind refusal text): not touched. `kind.yaml` now has every setting and a seed file, and the kind
  script's `install_database` makes the migrator's `uri`, but the hub stage still encodes the A1 refusal.

### For another PR

- **Prompt 18's `crates/hub/HARDENING.md`** (not edited here): the "Probes" paragraph says `/healthz` does not depend on the database (it answers only after the database, schema, web
  certificate and CAs have loaded: B1); the `LK_REPLICA_ADDRESS` example spells the headless Service `lanekeeper-hub` (the chart's is `lanekeeper-hub-replicas`, which `topology.rego`
  requires: P1); and its paragraph on the Jobs could say that the Jobs' pods carry `app.kubernetes.io/name: lanekeeper-hub-jobs`.
- **Section 9** (KMS keys, Secret Manager secrets and their IAM in the gcp chart, `iam.rego`, the project `lanekeeper-cloud`) and the **install guide**: the settings are now known and rendered,
  nothing creates what they name. `ADDING-A-SWIMLANE.md` still says that nothing creates the swimlane row (B11); `hub seed` does.
- **Resources of the Jobs** are a guess (100m/128Mi requests, 1/512Mi limits): measure them in the first real run.

### Assumptions made

- Argo CD's documentation (`argo-cd.readthedocs.io`) could not be fetched (the host is not on the session's network allow-list). The mapping is from memory of gitops-engine:
  `pre-install` and `pre-upgrade` are `PreSync`, the Helm weight orders hooks as a sync wave unless `argocd.argoproj.io/sync-wave` is set, and the delete policies map one to one. The chart sets
  the Argo CD wave as well as the Helm weight, so the order does not rest on the weight mapping, and uses only `before-hook-creation`. The first real sync is the test.
- The hook account and policy (`lanekeeper-hub-jobs`) are hooks kept after the sync, so a first install has them and the seed Job (not a hook) can use them. Argo CD does not manage hooks
  after the sync, so a deleted hook policy returns at the next sync, not at a self-heal.
- `LK_PUBLIC_ORIGIN` and `LK_HUB_AUDIENCE` are both `https://<gateway.hostname>`; `gateway.hostname` is required even where the Gateway is off (kind sets a synthetic one).
- staging and prod set `seed.enabled: true` with `existingConfigMap: lanekeeper-hub-seed-<env>`, a placeholder name for a ConfigMap that a person makes out of band; until it exists the
  seed Job waits and fails at its deadline, and the hub is not held back. The default in `values.yaml` is `false`, as asked.
- The Job's `activeDeadlineSeconds` is the timeout plus 300 s; `backoffLimit` at most 3 (policy); a deadline over 10800 s is refused.
- `LK_SHUTDOWN_GRACE_SECS` and `LK_SHUTDOWN_DRAIN_SECS` are now integers in the hub's ranges (they were `integer|string`), because the template adds them.
- A Helm number is a float, and Go prints a million as `1e+06`: tunables go through one helper that prints whole numbers whole.
- Schema errors differ between Helm 4 (`at '/config/entra/tenantId'`) and Helm 3 (`config.entra.tenantId:`): the planted cases look for the last key (`tenantId`), which both print.
- The seed Job's name hashes the pod template as the chart renders it, so a chart change that alters the template is a new Job too.
- `render.sh contract` no longer stops after the agent's failure (`&&`), so a cross-chart disagreement is reported even while B8 is open; `render.sh hub` runs the hub chart alone and is never the gate.
- `.github/workflows/deploy.yml` now also runs for `crates/hub/src/config/**` and `crates/hub/tests/hardening.rs`, which `env-names.sh` and `render.sh contract` read.

---

## Status (written after the two fix commits of 2026-10-28)

`C3` is `972b361` (`09/fix: the kind hub stage asserts what the production image can do`), `C4` is `77481be` (`09/fix: the agent chart names the config-server's namespace`).
The tools of the session ran as before (helm 4.3.0, kubeconform, conftest 0.71.1 behind a stand-in for Docker, shellcheck, actionlint; zizmor was installed with
`just ops-tools-install zizmor`). **`kind` did not run, and the `hub` stage of `kind-e2e` has not run at all** (no Docker daemon, a cgroup v1 kernel): what shows the script is the node
test, which runs the real `run.sh` and `lib.sh` with a stand-in docker, kind, helm, curl and crane and a fake `kubectl` behind `docker exec`, for each state, plus `shellcheck -x`, `bash -n` and `actionlint`.

### Closed

| Item | Commit | How it is shown |
|---|---|---|
| Section 10, option 1 (the kind job asserts what the production image can do) | C3 | `run.sh`/`lib.sh`: the migration Job (helm waits for the hook; then a bounded `kubectl wait` and `.status.succeeded`), the seed Job (found by label), then each replica: exit code 1, a Google marker, no setting, database or schema error. Classes `google`, `setting`, `schema`, `database`, `ready`, `wrong-exit`, `unknown`, `unlogged` (`hub_replica_class`, `hub_verdict`); 17 node tests more than before in `kind-run.test.mjs` (47), each class run through the real script, the bounds, the order, and the Jobs' failures |
| The `HUB_REFUSAL_TEXT` of the old stub and its check against `main.rs` | C3 | gone. One block of `lib.sh` holds every string looked for in a hub log; `kind-run.test.mjs` extracts the string literals of the non-test code of `crates/hub/src` (comments and `#[cfg(test)]` left out), requires each marker's anchors to be literals, and requires the marker list and the anchors to be the same set. Shown to fail: a reworded literal (on a copy of the source), a database marker that never matches, a migration Job not waited for, an exit code not checked |
| The summary says what is not measured | C3 | `KIND_NOT_MEASURED` in `lib.sh`: "`/readyz` on 3 replicas and P11 on the pod are NOT MEASURED in kind (the production image cannot be Ready without Google and Entra), acceptance 3 is not met by this job". `run.sh` puts it and three table rows in the summary; the `kind-e2e` job of `images.yml` has an `if: always()` step that appends it. Nothing ticks acceptance 3 |
| Section 11, `hub-fuzz` in the fuzz matrices | C3 | one line in `rust.yml` and `nightly.yml`; `consistency.test.mjs:71` passes. The recipe depends on `fuzz-tools-check`, which the job's install step covers (the nightly, `cargo-fuzz`, `cargo-deny`, `cargo-audit`, a C compiler on the runner) |
| Section 11, the stale A1 sentences | C3 | `kind/README.md`, `images.yml`, the `Justfile` recipe comment, the hub and gcp chart READMEs and `ADDING-A-SWIMLANE.md`; a test fails on a return of the old wording |
| Section 12 / B8 (the agent's config-server egress) | C4 | `configServer.namespace` (required) and `configServer.service` in the agent chart, one helper for the rule and the URL; `network.rego` (each wrong shape its own message; 31 more policy tests, 1193 in all), `contract.rego` (the namespace is a value, the shape is compared), `sit1.yaml` (`latest-pci`), the runbook, `render.sh` (35 more planted or variant cases that pass: 648 ok against 613). `render.sh contract`: 34 of 34, exit 0 (it had 1 failure). `render.sh planted`: no `PLANTED FAILED` line (it had 12) |
| B11 (step 1 of the runbook says nothing creates the swimlane row) | C4 | the runbook now says `hub seed` does, with the file's shape and the Job; text only, nothing in the repository holds the real seed file |

### Left open, on purpose

- **`render.sh planted` still exits 1, for one reason: the gcp chart cannot render (B5, the hook ranges), so the Cloud Armor cases are not run and the run counts that as one failure.** `render` and `policy` fail
  for the gcp staging and prod values, and `argocd`'s full scope for gcp prod, for the same reason. Nothing else is red in `render.sh lint|render|policy|contract|planted|argocd`.
- **`LK_CONFIG_SERVER_DEPLOYMENT` is not rendered.** The contract's pod does not list it and the agent treats it as optional, so the agent does not report when the config-server's pods started (C11,
  `crates/agent/tests/kube_configserver.rs`). Adding it is a contract decision: HARDENING.md's pod gets the variable, and `latest-pci` goes into `namespaces` (the agent refuses to start otherwise), which
  also renders a workload Role there and a destination in the Argo CD project. Recorded in the B8 section of the blockers file.
- **No policy knows which namespace is the right one.** The runbook asks for it and says how to check it (`kubectl get pods -A -l app=csp-configuration-server`).
- **`crates/hub/fuzz/Cargo.lock` has `rustls-webpki` 0.103.16 and `webpki-root-certs` 1.0.10**, as the root `Cargo.lock` has; the other fuzz lockfiles have 0.103.15 and 1.0.9. They are not 14 days old until
  2026-11-02 and 2026-11-03. Nothing checks the age of a locked version (only Renovate's `minimumReleaseAge` for proposals), and `cargo deny --manifest-path crates/hub/fuzz/Cargo.toml check` and
  `cargo audit --file crates/hub/fuzz/Cargo.lock` exit 0. Not changed.
- **`just ci-lint` fails on one file that is not 09's change:** `.github/scripts/playwright-install.test.sh` lines 247 and 248 (SC2016, info), which came with the merge of `main` (`22f1416`). `actionlint`,
  `check-pins.mjs`, `check-workflows.mjs` and zizmor (offline, with `.github/zizmor.yml`) pass, and `shellcheck -x` passes on every other script.
- **Stale in files this step may not edit:** `docs/threat-model.md` (the "Never run" row and item 13 of the residual risks still describe A1 and the old kind test), and the records `plans/09-deployment-ci.pr.md` and
  `plans/09-deployment-ci.evidence.md`. A person should refresh them with the PR text.
- **What the kind `hub` stage will show on its first real run is a guess in two places:** the exact first Google error (a failed read, or `starting took too long: web certificate`: both are accepted), and that
  Helm 4 waits for a Job hook with `--timeout` as described. A log with a different first error fails the stage, prints the log, and is the thing to read.

### Assumptions made

- The config-server of a swimlane runs in **one** namespace; the agent's own namespace is never that one. The contract says so in its paragraph on the namespace ("the config-server does not run in the agent's
  namespace"), so the chart and the policy refuse it, though the table above that paragraph says "usually".
- `configServer.service` is optional with the default of the contract's example; a Service named otherwise is accepted by the URL rule (any DNS label).
- A missing `LK_CONFIG_SERVER_URL` is refused: the NetworkPolicy lets the agent reach a config-server that it would then never call, and the chart always sets it.
- "Pod Ready" in the kind `hub` stage is a failure ("unexpected"); if all three replicas are Ready, P11 is still measured and printed for whoever reads it (`p11_verdict` is kept for that).
- A log is judged over the current and the previous container together, so an error in either counts; `kubectl logs --tail=40` of each.
- `hub_replica_class` puts a failure beside a Google error first (setting, then schema, then database): a replica whose database check failed is a database failure however many Google errors it logged too.
- The marker `setting it up took too long` (a CA that timed out) is accepted without the CA's name in front of it; it exists in the Rust source only as `CaStep::TimedOut`'s message.
- `.github/scripts/kind-run.test.mjs` now has 47 tests, and `node --test .github/scripts/*.test.mjs` 174 (the brief expected 157: the A1 tests were replaced by more tests, none removed without a successor).

---

## Status (written after the two fix commits of 2026-10-28, gap I1)

`C5` is `b5db76d` (`09/fix: the gcp chart makes the hub's KMS keys, Secret Manager containers and their IAM`), `C6` is `5a3ab7d` (`09/fix: kind and CI lint findings that came in from main`).
The tools of the session ran as before (helm 4.3.0, kubeconform, conftest 0.71.1 behind the stand-in for Docker, shellcheck, actionlint, zizmor). The hub binary and `kind` did not run.

### Closed

| Item | Commit | How it is shown |
|---|---|---|
| I1: the hub's KMS keys, Secret Manager containers and their IAM (section 9) | C5 | `charts/gcp` renders a `KMSKeyRing`, four `KMSCryptoKey` (agent CA, replica CA, audit checkpoint: `ASYMMETRIC_SIGN`, `EC_SIGN_P256_SHA256`; vault: `ENCRYPT_DECRYPT`), ten `SecretManagerSecret` containers (never a value), and the `IAMPolicyMember`s: `signerVerifier` on the three signing keys, `cryptoKeyEncrypterDecrypter` on the vault key, one `secretAccessor` on each secret, and Secret Manager's service agent on the secrets' key. `helm template | kubeconform -strict` passes for `gcp-storage` staging and prod (the CRD schemas were already vendored: no change to the lock) |
| Policy for them | C5 | `kms.rego` (new, 61 tests in `kms_test.rego`) checks purpose, algorithm, protection level, no import, kept on delete and prune, a secret is its replication and nothing else, a `SecretManagerSecretVersion` is refused; `iam.rego` learns the two roles and the hub's use of the vault key, each on its own kind and for the hub's account only (`cloudkms.admin`, a `signerVerifier` on the vault key, an `encrypterDecrypter` on a CA key, a `secretAccessor` on the project, a person or a group, a document with no spec: all planted); `argocd.rego` and `projects.yaml` allow `KMSKeyRing`, `KMSCryptoKey` and `SecretManagerSecret` and no version kind. `conftest verify`: 1309 of 1309 (it was 1193) |
| The two charts cannot disagree on a name | C5 | `render.sh contract` (`cross_check_hub_names`) reads the hub's rendered `LK_*_REF` and `LK_*_KMS_REF` and the four constants of the hub's code (`hub-writes` `PREVIEW_KEY_SECRET`, `hub-mcp` `KEY_SECRET_NAME`, the two defaults in `hub/src/config/git.rs`), and fails for a name with no container or key here, in another project, with no binding for the hub's account, a key used for another purpose, a version where there is none (vault) or none where there should be one (CA, checkpoint), the two CAs as one key, and for a container or key here that the hub does not use. 22 planted cases, all seen failing first |
| Documented | C5 | `charts/gcp/README.md`: the objects, which value feeds which hub chart value, what a person does in order (first key versions and the `LK_*_KMS_REF` to read off them, `hub ca-init` and the two roles on one key that its runner needs, the values of the eight other secrets, the Teams secrets as a runbook step) and a list of what is not verified. The hub chart's README, `values.yaml` and `NOTES.txt` no longer say that nothing creates the containers and the roles |
| The `just ci-lint` finding from `main` (SC2016 in `playwright-install.test.sh`) | C6 | a reason and `# shellcheck disable=SC2016` above each of the two lines, as `sign.sh` does; `shellcheck -x` passes, the script's own test passes (73 checks), `just ci-lint` exits 0 (35 shell scripts) and has no other finding |

Checks at this HEAD, each run on the committed tree: `conftest verify` 1309/1309; `node --test .github/scripts/*.test.mjs` 174/174; `render.sh lint` exit 0; `render.sh contract` exit 0
(the cross-chart check on the real files; its 22 planted cases are run by `render.sh planted` and `data`); `render.sh data` exit 0 (284 ok); `render.sh argocd` exit 0 (67 ok);
`render.sh planted`: 746 ok, no `PLANTED FAILED` line, exit 1 for the one counted failure (Cloud Armor, B5); `render.sh render` and `policy` fail only for the gcp chart with staging and prod values
(B5). With a throwaway `githubHookRanges` (documentation addresses in a file outside the repository, not committed) `render.sh all` exits 0 with 749 ok and nothing failed, so the whole
gcp chart, Cloud Armor included, passes lint, kubeconform, the policies, the scope against `lanekeeper-cloud` and the planted cases.

### Left open, on purpose

- **B5** (GitHub's hook ranges) as before: a person runs `node deploy/tools/github-hook-ranges.mjs --write`.
- **Nothing was applied.** That Config Connector makes the ring and the keys, makes a first version of each signing key (number 1) and accepts HSM keys in `location`, that Secret Manager takes a regional key
  for a one-replica secret, and the name of its service agent are **not verified** (Google's documentation could not be read: B6). The README lists each; the first apply is the test.
- **`LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS` is still not a setting of the hub chart**, so a rotation of the checkpoint key needs it first. The role covers the old versions.
- **The install guide** (section 9, last bullet) is still not written. The README of the gcp chart holds the steps for the keys and secrets and is what it should link to.
- **A stale `docs/threat-model.md`** (not editable here) has no row for the keys and the secrets the chart now makes; nor does anything in `docs/` list the decision on HSM keys in prod.
- **Teams webhook secrets** are a runbook step (container, value, one `secretAccessor` binding for the hub's account), because their names are chosen at run time.

### Assumptions made

- No key convention existed in the chart (the keys were external). The names are the ones the hub's example values already had: ring `lanekeeper`, keys `lanekeeper-agent-ca-<env>`, `lanekeeper-replica-ca-<env>`, `lanekeeper-audit-checkpoint-<env>`,
  `lanekeeper-vault-<env>`. The ring name is an example (Q29 is open) and cannot be deleted once made.
- Protection level is HSM, written out (it is immutable) and required in prod by `kms.rego`; staging may say SOFTWARE. A human relaxes the prod rule in `docs/decisions.md`. The destroy delay is 30 days; the vault key
  rotates every 90 days (the older versions stay enabled); the signing keys do not rotate, because the hub pins a key version.
- The secrets are `userManaged` with one replica in the chart's `location`, encrypted with `secrets.cmekKey`, a key of its own (not the vault key and not a signing key: those are used for nothing else). Without it they fall back
  to Google-managed encryption with the warning that fails prod and staging, like the buckets.
- One Google project for each environment: the secret names are constants of the hub's code and cannot carry an environment. The names in `secrets.hub.*` are Kubernetes names; the hub chart allows more.
- `mcp-token-hmac-key` has a container although the default MCP mode (`oauth`) never reads it, because the brief lists it and the personal-token mode does not start without it.
- The `lanekeeper.io/purpose` label of the checkpoint key is `audit-checkpoint-signing`, not `audit-checkpoint`: the bucket has that value, and `render.sh` finds documents by this label.
- `iam.rego` now reads `member`, `role` and the reference from the document's root, so a binding whose `spec:` is empty is refused (it was skipped before). `.github/workflows/deploy.yml` also runs for
  `crates/hub-writes/src/config.rs` and `crates/hub-mcp/src/auth/pat.rs`, which `render.sh contract` now reads.
