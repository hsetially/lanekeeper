# Install Lanekeeper on GKE, step by step

This guide installs the whole application on Google Cloud by hand. It is for an engineer who knows Kubernetes and Google Cloud and does
not know this repository. Follow the steps in order. Each one says why, what to run, what you should see, and what to do if you do not.

**How to read it.**

- Every fact comes from a file of this repository, which the step names, or it is marked **Not verified**. Google's documentation
  (`docs.cloud.google.com`), `registry.k8s.io` and `api.github.com/meta` could not be reached when the charts were written
  (`plans/09-deployment-ci.blockers.md`, B5 and B6). So every Google-specific flag, `gcloud` command and console path that is not copied
  from a file is **Not verified**: check it against Google's current documentation, or against `--help`, before you run it. The same
  holds for Entra and GitHub screens.
- A block of commands is preceded by a line that says `Source:` (copied from the file it names) or `Not verified` (written from memory).
  A test keeps it so: `.github/scripts/install-guide.test.mjs`.
- A name in capitals between angle brackets, such as `<PROJECT_ID>`, is a placeholder. Step 0 defines every one. Replace it everywhere, with the
  angle brackets.
- A chart value is written `config.git.baseUrl` for the hub chart, and `gcp:kms.keyRing`, `db:backup.bucket` or `agent:hub.endpoint` for the
  other charts. It is the path inside that chart's `values.yaml`: `deploy/charts/hub/values.yaml`, and the same file in `deploy/charts/gcp`,
  `deploy/charts/db` and `deploy/charts/agent`.
- Commands run from the root of a checkout of this repository, on your machine. `kubectl` points at the platform cluster unless a
  command says `--context`.
- Never type a secret into a command line. The steps that need one read it with `read -rs` or make it in a pipe. It stays out of the
  shell history and out of the process list.
- No real project id, host name, address or secret is in this guide, and none may be (AGENTS.md, rule 12). Your real values go in the
  values files of your own repository, from step 7 on.
- Read step 18 before you start. It lists what is not proven. Several things here have never run on a real cluster.

## 0. Before you start

**Why.** The later steps use many names and decisions. Collect them now, and nobody has to stop in the middle of step 10.

**What you will have at the end.**

- The hub: three replicas on the platform cluster, behind a Gateway for users and a TCP load balancer for agents.
- A CloudNativePG database of three instances, backed up every day to a bucket.
- The Google Cloud objects they need: buckets, keys, secret containers, IAM, Cloud Armor and Binary Authorization.
- Sign-in with Entra ID, GitHub webhooks, and the MCP server for Cursor.
- One agent for each swimlane, in that swimlane's own cluster, and a signed sentinel package for each NFS VM.

**What it is made of.**

| Part | What it is | Runs in | Made by |
|---|---|---|---|
| hub | the web app, the API, the MCP server, the webhook and the agent gateway; three replicas of one StatefulSet `lanekeeper-hub` | platform cluster, namespace `lanekeeper` | `deploy/charts/hub` |
| db | a CloudNativePG cluster `lanekeeper-db`, three instances | platform cluster, namespace `lanekeeper` | `deploy/charts/db` |
| Google Cloud objects | buckets, service accounts, IAM, Cloud Armor, Cloud KMS keys, Secret Manager containers, Binary Authorization, as Config Connector objects | platform cluster, namespace `lanekeeper-gcp` | `deploy/charts/gcp` |
| agent | one Deployment `lanekeeper-agent` for each swimlane | that swimlane's own cluster | `deploy/charts/agent` |
| sentinel | a systemd service on each NFS VM, installed from a signed RPM | the NFS VM, not a cluster | `deploy/sentinel` |
| Argo CD | the platform cluster's deploys the hub, db and gcp charts. Each swimlane cluster has its own, which deploys its agent | each cluster, namespace `argocd` | you (step 4); the files are in `deploy/argocd` |

A swimlane is one GKE cluster with one NFS VM, one config-server, one Argo CD and one agent (`docs/domain-model.md`).

**Tools on your machine.**

| Tool | Version | Where it is pinned |
|---|---|---|
| `gcloud` | current | not pinned: **Not verified** against any version |
| `kubectl` | within one minor version of your cluster | not pinned (`deploy/schemas/sources.json` checks the charts against Kubernetes 1.34.0 schemas) |
| `git`, `openssl`, `curl` | any recent | not pinned |
| `node` | 22 or later | `package.json` (`engines`), and the header of `deploy/tools/github-hook-ranges.mjs` |
| `just` | 1.40.0, optional | `just_version` in `Justfile`. Only to run the repository's own checks |
| `helm` | 4.3.0, optional | `helm_version` in `Justfile`. Argo CD renders the charts, so you need it only to check them yourself (`just ops-tools-install helm`) |
| `cosign`, `crane` | 3.1.3 and 0.22.1, optional | `cosign_version` and `crane_version` in `Justfile`. Only to verify images yourself (`just ops-tools-install cosign crane`) |

**Who must be able to do what.** One person can be several of these.

| Who | Needed for |
|---|---|
| Google project owner (project, billing, APIs, IAM, Cloud KMS) | steps 1, 3, 6, 7, 10, 12 |
| Admin of the platform cluster (`cluster-admin`) | steps 2, 3, 4, 10, 11, 12 |
| GitHub organisation admin (repository variables and environments, webhooks, a read-only token, GHCR packages) | steps 5, 6, 8, 13 |
| Entra administrator (app registration, API scope, pre-authorising Cursor) | step 9 |
| Network team (GlobalProtect egress ranges, clusters' outbound IPs, DNS) | steps 7, 12, 13, 15 (Q1 and Q2 in `docs/open-questions.md`) |
| Owner of the config-server of each swimlane (its namespace) | step 15 |
| Someone with root on the NFS VM | step 15 |
| A reviewer who can merge to `main` | every step that says "pull request" |

**Collect these values.** This table defines every placeholder of the guide. The right column says where the value comes from, or in which
step you will make it.

| Placeholder | What it is | Where it comes from |
|---|---|---|
| `<ENV>` | `prod` or `staging`. It names the values files `deploy/values/hub/<ENV>.yaml`, `deploy/values/db/<ENV>.yaml` and `deploy/values/gcp/<ENV>.yaml` | you. One Google project for each environment (step 1) |
| `<PROJECT_ID>` | the Google project of this environment | you (step 1) |
| `<PROJECT_NUMBER>` | that project's number, not its id | `gcloud projects describe`, **Not verified** (step 1) |
| `<LOCATION>` | the region of the buckets, the keys, the secrets and the disks. A customer-managed key must be in the same location as what it protects (`deploy/charts/gcp/values.yaml`) | you |
| `<CLUSTER>` | the platform cluster | you (step 2) |
| `<CLUSTER_LOCATION>` | its region or zone | you (step 2) |
| `<HUB_HOST>` | the DNS name users type. It is `gateway.hostname`, and the hub's `LK_PUBLIC_ORIGIN` is `https://<HUB_HOST>` | the network team (Q2) |
| `<AGENT_HOST>` | the DNS name agents dial. It is in `config.agentEndpointHosts`, and every agent's `agent:hub.endpoint` is `https://<AGENT_HOST>` | the network team (Q2) |
| `<REPO_OWNER>` | the GitHub owner of the repository that holds this code and runs its workflows | your GitHub organisation |
| `<REPO_NAME>` | that repository's name | your GitHub organisation |
| `<GITHUB_ORG>` | the GitHub owner of the two repositories the hub mirrors | your GitHub organisation |
| `<BASE_REPO>` | the name of the base configuration repository (`config.git.baseRepo` is `<GITHUB_ORG>/<BASE_REPO>`) | your GitHub organisation |
| `<TENANT_REPO>` | the name of the tenant data repository | your GitHub organisation |
| `<ENTRA_TENANT_ID>` | the Entra tenant, a GUID | Entra admin (step 9) |
| `<ENTRA_CLIENT_ID>` | the application (client) id of the app registration, a GUID | Entra admin (step 9) |
| `<ENTRA_ADMIN_OBJECT_IDS>` | the Entra object ids of the first administrators, comma separated | Entra admin (step 9) |
| `<CURSOR_CLIENT_ID>` | the client id of Cursor's own registration, to pre-authorise | Entra admin (step 9; Q21) |
| `<CMEK_RING>` | a Cloud KMS key ring you make by hand for the customer-managed keys and the CI signing key | you (step 1; Q29 has not chosen a ring) |
| `<CMEK_KEY>` | one of the keys in that ring. Five are for encryption (step 1), and each has its own name | you (step 1) |
| `<SECRETS_KEY>` | the secrets key as a full resource name, `projects/<PROJECT_ID>/locations/<LOCATION>/keyRings/<CMEK_RING>/cryptoKeys/<CMEK_KEY>`. It is `gcp:secrets.cmekKey` | you (step 1) |
| `<CI_KEY>` | the asymmetric signing key that CI signs images with | you (step 6) |
| `<KEYRING>` | the key ring that the gcp chart makes for the hub's four keys. It is `gcp:kms.keyRing` | you (step 7). Not the same ring as `<CMEK_RING>` |
| `<BACKUP_BUCKET>` | the database backup bucket. Names are global to Google Cloud | you (step 7) |
| `<AUDIT_BUCKET>` | the audit checkpoint bucket | you (step 7) |
| `<DOCS_BUCKET>` | the docs bucket | you (step 7) |
| `<CONFIG_CONNECTOR_GSA>` | the Google service account that Config Connector acts as | you (step 3) |
| `<CI_GSA>` | the Google service account that GitHub's CI acts as | you (step 6) |
| `<WIF_POOL>` | the Workload Identity Federation pool for CI | you (step 6) |
| `<WIF_PROVIDER>` | the provider in that pool | you (step 6) |
| `<CA_INIT_GSA>` | a temporary Google service account for `hub ca-init` | you (step 10) |
| `<CA_INIT_KSA>` | a temporary Kubernetes service account for `hub ca-init` | you (step 10) |
| `<AGENT_CA_KEY_NAME>` | the name of the agent CA key. It is `gcp:kms.keys.agentCa` | you (step 7) |
| `<REPLICA_CA_KEY_NAME>` | the name of the replica CA key. It is `gcp:kms.keys.replicaCa` | you (step 7) |
| `<AGENT_CA_KEY_VERSION>` | the resource name of the agent CA key version | you read it in step 7 |
| `<REPLICA_CA_KEY_VERSION>` | the resource name of the replica CA key version | you read it in step 7 |
| `<HUB_IMAGE>` | `image.repository` of `deploy/values/hub/<ENV>.yaml` | that file |
| `<HUB_DIGEST>` | the hub image's digest, `sha256:` and 64 hex digits | the `publish` job of `images.yml` (step 6) |
| `<AGENT_DIGEST>` | the agent image's digest | the same job |
| `<REVISION>` | a full commit id, 40 hex digits, of this repository | `git rev-parse origin/main` after a merge |
| `<GITHUB_USER>` | the GitHub user that owns the token for pulling images. Only if the packages are private | your GitHub organisation (step 6) |
| `<PULL_SECRET>` | the name of a Kubernetes Secret that lets nodes pull the images from GHCR. Only if the packages are private | you (step 6; Q8) |
| `<GATEWAY_IP_NAME>` | the name of a reserved global external IP address. It is `gateway.addressName` | you (step 12) |
| `<SSL_CERT_NAME>` | the name of a Compute Engine SSL certificate. It is `gateway.tls.preSharedCerts` | you (step 12) |
| `<ARMOR_POLICY>` | the Cloud Armor policy. It is `gateway.securityPolicyName` and `gcp:cloudArmor.policyName` | you (step 7) |
| `<USERS_RANGES>` | the GlobalProtect egress ranges that may reach the web app | the network team (Q2) |
| `<AGENT_RANGES>` | the outbound IPs of the swimlane clusters and the NFS VMs, as `/32` | the network team (Q1) |
| `<TEAMS_SECRET>` | the Secret Manager name of one Teams webhook destination | an administrator, at run time (step 8) |
| `<HUB_PROJECT>` | the id of the project row in the hub that groups swimlanes (`projects` in the seed file). It is not a Google project | you (step 15) |
| `<HUB_PROJECT_NAME>` | that project's display name | you (step 15) |
| `<SWIMLANE_ID>` | the swimlane's id: lower case letters, digits and `-`, at most 63 | the platform team (step 15) |
| `<SWIMLANE_NAME>` | its display name | the platform team |
| `<TIER>` | its environment tier | the platform team |
| `<SWIMLANE_PROJECT_ID>` | the Google project of the swimlane's cluster | the platform team |
| `<SWIMLANE_CLUSTER>` | the swimlane's cluster | the platform team |
| `<SWIMLANE_CLUSTER_LOCATION>` | its region or zone | the platform team |
| `<SWIMLANE_CONTEXT>` | the `kubectl` context that points at it | you |
| `<NFS_SERVER>` | the NFS server's address | the platform team (Q11) |
| `<NFS_EXPORT>` | the export path on that server | the platform team (Q11) |
| `<MOUNT_ROOT>` | where the export is mounted | the platform team (Q11) |
| `<CONFIG_SERVER_NAMESPACE>` | the one namespace the swimlane's config-server runs in | the config-server's owner (step 15) |
| `<AGENT_NAMESPACE>` | the agent's own namespace. The chart's Application uses `lanekeeper`. **It must differ from `<CONFIG_SERVER_NAMESPACE>`** | `deploy/argocd/swimlanes/<SWIMLANE_ID>.yaml` |
| `<WATCHED_NAMESPACES>` | the namespaces whose workloads the agent watches | the platform team (step 15) |
| `<TENANT>` | a tenant id of the swimlane | the platform team |
| `<NFS_VM>` | the NFS VM's name, as the sentinel's certificate carries it | the platform team |
| `<EXPORT_ROOT>` | the export root the sentinel watches | the platform team |
| `<SENTINEL_GSA>` | the Google service account of the sentinel | the platform team (step 15) |
| `<SEED_FILE>` | the path of the seed file on your machine. It lives outside this repository | you (step 12) |
| `<SEED_REVISION>` | the seed's revision: a positive whole number that is `seed.revision` and the end of the seed ConfigMap's name (`lanekeeper-hub-seed-<ENV>-<SEED_REVISION>`). It starts at 1 and **only moves forward**: never a number you used before, a revert included | you (steps 12 and 17.3) |
| `<SEED_JOB>` | the name of the seed Job, `lanekeeper-hub-seed-` and a hash | `kubectl -n lanekeeper get jobs` (step 12) |

**The pull requests come in rounds.** Argo CD runs the commit named in `targetRevision`, and a commit cannot name itself. So a change
reaches a cluster in two pull requests: the change, and then a one-line change of `targetRevision` to the commit that holds it
(`deploy/argocd/README.md`, "Moving a revision"). In this guide that happens in two rounds.

| Round | You prepare it in | Files | Then |
|---|---|---|---|
| A | steps 5, 7 and 11.3 (step 6 comes before its merge) | `deploy/values/hub/github-hook-ranges.yaml`, `deploy/values/gcp/<ENV>.yaml`, `deploy/values/db/<ENV>.yaml` | merge it (step 7.2); a second pull request sets `targetRevision` in `deploy/argocd/platform/gcp.yaml` and `deploy/argocd/platform/db.yaml` |
| B | step 12 | `deploy/values/hub/<ENV>.yaml` | merge it; a second pull request sets `targetRevision` in `deploy/argocd/platform/hub.yaml` |

The merge of round A is a push to `main`, which publishes images (step 6). Round B needs their digests and the Cloud KMS key versions that
round A makes.

**If not.** A value is missing and nobody knows it: stop and ask. Do not put a guess in a values file. Several are open questions with no
answer yet (`docs/open-questions.md`: Q1, Q2, Q11, Q24, Q29).

## 1. Google project and APIs

**Why.** The secret names are constants of the hub's code (for example `write-preview-key`), so two environments cannot share a project
(`deploy/charts/gcp/README.md`, "Which values feed which hub chart value"). Make one project for each environment. Everything below
is for one. The charts need the APIs of the objects they create and of the services the hub calls, and the buckets and disks need
customer-managed keys that exist before the first sync.

**Do.**

1.1 Make the project and link billing, the way your organisation does it. Then use its id as `<PROJECT_ID>`.

1.2 Enable the APIs. The ones with a file in the last column are named in the repository. The others are the services of the objects the charts
create, and their names are from memory.

| API | Needed because | Named in |
|---|---|---|
| `cloudkms.googleapis.com` | the hub's keys, and the hub calls it | `crates/hub/HARDENING.md` (egress list) |
| `secretmanager.googleapis.com` | the hub's secrets, and the hub calls it | `crates/hub/HARDENING.md` (egress list) |
| `storage.googleapis.com` | the buckets, and the hub writes the audit checkpoints | `crates/hub/HARDENING.md` (egress list) |
| `binaryauthorization.googleapis.com`, `containeranalysis.googleapis.com` | the admission policy, the attestor and its note | `deploy/argocd/README.md` (prerequisites) |
| `iam.googleapis.com` | the service accounts the chart makes | none: **Not verified** name |
| `compute.googleapis.com` | Cloud Armor, the IP address, the SSL certificate, the disks | none: **Not verified** name |
| `container.googleapis.com` | GKE | none: **Not verified** name |
| `cloudresourcemanager.googleapis.com` | project-level IAM, which Config Connector uses | none: **Not verified** name |
| `cloudtrace.googleapis.com` | the hub has `roles/cloudtrace.agent` | `deploy/charts/gcp/README.md` |
| `iamcredentials.googleapis.com`, `sts.googleapis.com` | Workload Identity Federation for CI (step 6) | none: **Not verified** names |
| `certificatemanager.googleapis.com` | only if you set `gateway.tls.certManagerCerts` instead of `gateway.tls.preSharedCerts` | none: **Not verified** name |

**Not verified: check against Google's current documentation.** The names above that are not "named in" a file.

```bash
gcloud services enable \
  cloudkms.googleapis.com secretmanager.googleapis.com storage.googleapis.com \
  binaryauthorization.googleapis.com containeranalysis.googleapis.com \
  iam.googleapis.com compute.googleapis.com container.googleapis.com \
  cloudresourcemanager.googleapis.com cloudtrace.googleapis.com \
  iamcredentials.googleapis.com sts.googleapis.com \
  --project <PROJECT_ID>
```

1.3 Make the key ring and the customer-managed keys by hand. The buckets, the disks and the secrets are encrypted with keys that the
charts name and do not make (`deploy/charts/gcp/README.md`, "The customer-managed keys of the buckets, the disks and the secrets are
not made here"). They must exist before the first sync of the gcp chart. Put them in a ring of their own: `<KEYRING>` is made by the chart
in step 7, and a key ring cannot be deleted once it is made (`deploy/charts/gcp/values.yaml`). Make five keys for encryption, one for each
use below, all in `<LOCATION>`, and one signing key in step 6.

| Key | Fills in |
|---|---|
| backups | `gcp:buckets.backup.cmekKey` |
| audit checkpoints | `gcp:buckets.auditCheckpoint.cmekKey` |
| docs | `gcp:buckets.docs.cmekKey` |
| disks | `gcp:cmek.diskKeys`, `hub:storage.cmekKey` and `db:storage.cmekKey`. The three must name the same key |
| secrets | `gcp:secrets.cmekKey` |

**Not verified: check against Google's current documentation.** Run the second command once for each of the five keys, with a `<CMEK_KEY>` of
its own.

```bash
gcloud kms keyrings create <CMEK_RING> --location <LOCATION> --project <PROJECT_ID>
gcloud kms keys create <CMEK_KEY> --keyring <CMEK_RING> --location <LOCATION> --purpose encryption --project <PROJECT_ID>
```

1.4 Make sure Google's service agents exist. The gcp chart gives the Cloud Storage, Compute Engine and Secret Manager service agents use of
the keys, and names them by `<PROJECT_NUMBER>` (`deploy/charts/gcp/README.md`). A binding to an agent that does not exist fails. The
repository documents only how to create the Secret Manager one.

Source: `deploy/charts/gcp/README.md`, step 1 of "What a person still does". The README itself marks this command as not verified, because the `beta` component could not be installed where it was written.

```bash
gcloud beta services identity create --service=secretmanager.googleapis.com --project=<PROJECT_ID>
```

The Cloud Storage and Compute Engine agents: the repository does not say how to create them. **Not verified**: they normally exist once the
API is enabled and a bucket and a disk have been used in the project. If a binding in step 7 fails because an account "does not exist",
this is why.

1.5 Read the project's number. Several values need the number, not the id.

**Not verified: check against Google's current documentation.**

```bash
gcloud projects describe <PROJECT_ID> --format='value(projectNumber)'
```

**You should see.**

- `gcloud services list --enabled --project <PROJECT_ID>` lists the APIs you enabled.
- `gcloud kms keys list --keyring <CMEK_RING> --location <LOCATION> --project <PROJECT_ID>` lists five keys.
- The last command prints the 12-digit project number. Use it as `<PROJECT_NUMBER>`.

**If not.** An API refuses to enable: the project is not billed, or an organisation policy forbids it. A key cannot be made in
`<LOCATION>`: check that Cloud KMS offers that location, and that the bucket, the disk and the secret region are the same one.

## 2. The platform cluster

**Why.** The charts need features that a plain GKE cluster does not always have. Check each before you install anything.

**Do.**

2.1 Make or choose the cluster. It must meet this table.

| The cluster must have | Needed by | If it is missing |
|---|---|---|
| Workload Identity. The pods act as Google service accounts through the `iam.gke.io/gcp-service-account` annotation | `deploy/charts/hub/templates/serviceaccount.yaml`, `deploy/charts/db` | the hub cannot read Secret Manager or Cloud KMS, and the database cannot write backups |
| nodes in at least three zones. The hub's pods and the database's instances have required anti-affinity over `topology.kubernetes.io/zone` | `hub:topology.mode`, `deploy/charts/db/README.md` | pods stay Pending |
| room for the pods: each hub replica asks for 500m CPU and 1536 MiB and may use 4 CPU and 2 GiB; each database instance is sized at 2 CPU, 2 GiB and 20 GiB (a placeholder until the load test) | `deploy/charts/hub/values.yaml`, `deploy/charts/db/README.md` | pods stay Pending |
| the Gateway controller with the class `gke-l7-global-external-managed`, and the kinds `GCPBackendPolicy` and `HealthCheckPolicy` | `deploy/charts/hub/templates/gateway.yaml` | the users' entry point does not exist |
| FQDN network policy (`FQDNNetworkPolicy`) enabled | `crates/hub/HARDENING.md` ("The network") | the hub cannot reach Entra, GitHub or Google by name. `hub:network.fqdnPolicy: false` reaches them by address instead, which is weaker and marked; it is for kind only, and `deploy/tests/render.sh policy` fails its warning for staging and prod |
| a network plugin that enforces `NetworkPolicy` | `crates/hub/HARDENING.md`, `crates/agent/HARDENING.md` ("What this contract cannot prove") | the policies are ignored, so every limit in them is gone |
| the persistent disk CSI driver `pd.csi.storage.gke.io` | `deploy/charts/hub/templates/storageclass.yaml` | the mirror volumes are never bound |
| Config Connector (step 3) | `deploy/charts/gcp` | the Google Cloud objects are not made |
| nodes that can reach `ghcr.io`, GitHub, Entra and Google by name | `crates/hub/HARDENING.md` ("The network") | the images are not pulled and the hub cannot start |
| Binary Authorization on the cluster | `deploy/charts/gcp/README.md` ("Admission"); Q24 | the project's policy is not evaluated, so not even the dry run's audit log fills (**Not verified**). Nothing is blocked either way |

**Not verified: check against Google's current documentation.** Every flag below. Compare them with `gcloud container clusters create --help`,
and add the switch for Binary Authorization when Q24 is answered.

```bash
gcloud container clusters create <CLUSTER> \
  --location <CLUSTER_LOCATION> --project <PROJECT_ID> \
  --workload-pool <PROJECT_ID>.svc.id.goog \
  --gateway-api standard \
  --enable-dataplane-v2 \
  --enable-fqdn-network-policy \
  --addons ConfigConnector
```

2.2 Point `kubectl` at it and check it.

**Not verified: check against Google's current documentation.** The first command.

```bash
gcloud container clusters get-credentials <CLUSTER> --location <CLUSTER_LOCATION> --project <PROJECT_ID>
kubectl get nodes -L topology.kubernetes.io/zone
kubectl get gatewayclass
kubectl api-resources | grep -E 'gcpbackendpolic|healthcheckpolic|fqdnnetworkpolic'
kubectl create namespace lanekeeper
```

The workload pool in `<PROJECT_ID>.svc.id.goog` is the form `deploy/ADDING-A-SWIMLANE.md` uses. The namespace is made now because steps 10
and 11 put Secrets in it before Argo CD would make it.

**You should see.**

- Nodes in at least three different zones.
- A GatewayClass `gke-l7-global-external-managed`.
- Three lines from `kubectl api-resources`: one for each of the three kinds.
- `namespace/lanekeeper created`.

**If not.** Fewer than three zones: add node pools, or make the cluster regional. A missing GatewayClass or kind: the Gateway controller is
not enabled (**Not verified**: how). No `fqdnnetworkpolic` line: FQDN network policy is not enabled. Do not go on without it: `hub:network.fqdnPolicy: false` is for
kind only, and the repository's check (`deploy/tests/render.sh policy`) fails the warning that goes with it for staging and prod.

## 3. Config Connector and its Google service account

**Why.** `deploy/charts/gcp` is a set of Config Connector objects, deployed to the namespace `lanekeeper-gcp`. Config Connector needs a Google
service account that is allowed to make them (`deploy/argocd/README.md`, "Prerequisites in each cluster").

**Do.**

3.1 Make the service account.

**Not verified: check against Google's current documentation.**

```bash
gcloud iam service-accounts create <CONFIG_CONNECTOR_GSA> --project <PROJECT_ID>
```

3.2 Grant it roles. **The repository does not say which roles it needs** (step 18). It lists the kinds the chart makes, in
`deploy/argocd/platform/projects.yaml`: `StorageBucket`, `IAMServiceAccount`, `IAMPolicyMember`, `ComputeSecurityPolicy`, `KMSKeyRing`,
`KMSCryptoKey`, `SecretManagerSecret`, `BinaryAuthorizationPolicy`, `BinaryAuthorizationAttestor` and `ContainerAnalysisNote`. The role set
has to cover creating each of them and granting IAM on a bucket, a key, a secret and the project. Choose it with your security team.

3.3 Tell Config Connector to act as that account, in the namespace `lanekeeper-gcp`. Config Connector's documentation for its
"namespaced mode" has the exact `ConfigConnectorContext` and the Workload Identity binding. **Not verified**: both, and the name of
Config Connector's own namespace and service account. The repository only says that a `ConfigConnectorContext` for `lanekeeper-gcp` is
needed.

3.4 Make the namespace the context lives in.

```bash
kubectl create namespace lanekeeper-gcp
```

**You should see.** Config Connector's pods running (its namespace is **Not verified**: `kubectl get pods -A | grep cnrm`), and a
`ConfigConnectorContext` in `lanekeeper-gcp` whose status is healthy.

**If not.** The context stays unhealthy: the Workload Identity binding is missing, or the service account cannot be impersonated. Read the
context's status (`kubectl -n lanekeeper-gcp describe configconnectorcontext`).

## 4. Argo CD and the Applications

**Why.** The platform cluster's Argo CD deploys the hub, the database and the Google Cloud objects from this repository
(`deploy/argocd/README.md`). **There is no parent Application ("app of apps")**. You apply four files by hand, in the order below.

**Do.**

4.1 Install Argo CD in the namespace `argocd` of the platform cluster. The repository does not install it, and does not say how. Its CRDs are
checked at Argo CD v3.5.4 (`deploy/schemas/sources.json`), so use that version or a newer one that you have checked. **Not verified**: the
install steps; see Argo CD's documentation.

4.2 Give Argo CD read access to this repository if it is private. **Not verified**: the repository does not say how.

4.3 Check that the repository the files name is yours. The four files and `deploy/policy/argocd.rego` name one repository in `repoURL`
and `sourceRepos`. If you install from another one, change the files, the policy and the image names together (step 18).

```bash
grep -n 'repoURL' deploy/argocd/platform/hub.yaml deploy/argocd/platform/db.yaml deploy/argocd/platform/gcp.yaml
```

4.4 Apply the projects. They limit what each Application may deploy and where.

Source: `deploy/argocd/README.md` ("Moving a revision", step 4), which applies a file this way.

```bash
kubectl apply -n argocd -f deploy/argocd/platform/projects.yaml
kubectl -n argocd get appprojects.argoproj.io
```

4.5 The Applications come next, one at a time, in the order of this table. Do not apply one early: each waits for something you make in a
later step.

| Order | File | Deploys | Applied in |
|---|---|---|---|
| 1 | `deploy/argocd/platform/projects.yaml` | the projects `lanekeeper-workloads` (hub, db) and `lanekeeper-cloud` (gcp) | this step |
| 2 | `deploy/argocd/platform/gcp.yaml` | the Application `lanekeeper-gcp`: three buckets, the Google service accounts, IAM, Cloud Armor, the hub's key ring and four keys, ten secret containers, Binary Authorization. Namespace `lanekeeper-gcp`. Needs GitHub's ranges and the CI public key in its values | step 7 |
| 3 | `deploy/argocd/platform/db.yaml` | the Application `lanekeeper-db`: the CloudNativePG cluster, its daily backup, its storage class, its network policy. Needs the backup bucket and the account from the gcp chart, the operator, and two Secrets | step 11 |
| 4 | `deploy/argocd/platform/hub.yaml` | the Application `lanekeeper-hub`: the StatefulSet, the Services, the Gateway, the policies, the migration Job (before the sync) and the seed Job | step 12 |
| per swimlane | `deploy/argocd/swimlanes/<SWIMLANE_ID>.yaml` | the agent, applied to that swimlane's own Argo CD | step 15 |

**You should see.** `appproject.argoproj.io/lanekeeper-workloads created` and `appproject.argoproj.io/lanekeeper-cloud created`, and both in the
list.

**If not.** `no matches for kind "AppProject"`: Argo CD's CRDs are not installed in this cluster. `Forbidden`: your account is not allowed to
write to the namespace `argocd`.

## 5. GitHub's webhook ranges

**Why.** Cloud Armor lets GitHub reach `/hooks/github`, and nothing else on that path, from GitHub's published webhook ranges. They are
generated from GitHub's `/meta` document and never typed in. Until the file holds them, the gcp chart refuses to render
(`plans/09-deployment-ci.blockers.md`, B5).

**Do.** On a machine that can reach `api.github.com` (the sandbox that wrote the tool could not), in a checkout of your repository:

Source: `deploy/tools/github-hook-ranges.mjs` (its header) and `deploy/charts/gcp/README.md` ("GitHub's ranges are generated, never typed").

```bash
node deploy/tools/github-hook-ranges.mjs --write
node deploy/tools/github-hook-ranges.mjs --check
git add deploy/values/hub/github-hook-ranges.yaml
git commit -m "values: GitHub's webhook ranges"
```

Behind a proxy on Node 24 or later, set `NODE_USE_ENV_PROXY=1` first (the tool's header). Do this on a branch: the file is part of pull request A (step 0).

**You should see.** `deploy/values/hub/github-hook-ranges.yaml` has a non-empty list under `cloudArmor.githubHookRanges`, and `--check` exits
with 0. The tool writes only if the whole list passes: it has at most 64 CIDRs, none wider than a /16.

**If not.** The tool names the reason and writes nothing: the answer is not JSON, it has no `hooks` list, the list is empty, or a range is
too wide. Check your network access to `https://api.github.com/meta`. The nightly job `hook-ranges` of `.github/workflows/nightly.yml`
fails when the committed file differs from GitHub's list. When it does, run the two commands again and move the revision of
`deploy/argocd/platform/gcp.yaml` (step 17).

## 6. Images and the supply chain

**Why.** The cluster runs images that this repository's workflows build, scan, sign and publish, and it runs them by digest only. A tag is
never deployed. Binary Authorization can check the signature that CI makes with a Cloud KMS key. This step makes that key, the identity CI
uses to reach it, and the repository variables the workflow reads. It must come before the merge of pull request A, so that the first
images are signed.

**What builds what.**

| Workflow | When | Result |
|---|---|---|
| `.github/workflows/images.yml` | every pull request builds and scans the hub and agent images twice and requires one digest. A push to `main` and a `v*` tag also publish | `ghcr.io/<REPO_OWNER>/<REPO_NAME>/hub` and `.../agent`, pushed by digest, signed keyless, with an SBOM and provenance, and signed again with the Cloud KMS key when the variables below are set. The job summary prints each reference |
| `.github/workflows/sentinel.yml` | a `sentinel-v*` tag publishes; every pull request builds and tests | the sentinel RPM (step 15) |

The repository name is lower-cased in the image name (`images.yml`, "Name the repository"). `hub:image.repository` in your values must match it.

**Do.**

6.1 Make the CI signing key. It signs images, and its public half goes into `gcp:binaryAuthorization.publicKeys` (step 7). Images are signed
once, so one key serves every environment.

Source: `deploy/charts/gcp/README.md` ("Admission") says an asymmetric signing key, `EC_SIGN_P256_SHA256` for example, in the project and location of the
environment. **Not verified**: the flags.

```bash
gcloud kms keys create <CI_KEY> --keyring <CMEK_RING> --location <LOCATION> \
  --purpose asymmetric-signing --default-algorithm ec-sign-p256-sha256 --project <PROJECT_ID>
```

6.2 Let GitHub's CI use it, without a stored key. The repository does not make this trust root (`plans/09-deployment-ci.blockers.md`, B12).
Its plan says the Workload Identity Federation condition must pin the repository, and `ref` to `main` or to `v*` tags
(`plans/09-deployment-ci.md`, "Risks"), and that only this identity may use the key. `images.yml` checks the signer against
`refs/heads/main` and `refs/tags/v...`.

**Not verified: check against Google's current documentation.** Every command and flag. Have your security team review the condition: it is what keeps
another repository or branch from signing as you.

```bash
gcloud iam workload-identity-pools create <WIF_POOL> --location global --project <PROJECT_ID>
gcloud iam workload-identity-pools providers create-oidc <WIF_PROVIDER> \
  --workload-identity-pool <WIF_POOL> --location global --project <PROJECT_ID> \
  --issuer-uri https://token.actions.githubusercontent.com \
  --attribute-mapping 'google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.ref=assertion.ref' \
  --attribute-condition "assertion.repository == '<REPO_OWNER>/<REPO_NAME>' && (assertion.ref == 'refs/heads/main' || assertion.ref.startsWith('refs/tags/v'))"
gcloud iam service-accounts create <CI_GSA> --project <PROJECT_ID>
gcloud iam service-accounts add-iam-policy-binding <CI_GSA>@<PROJECT_ID>.iam.gserviceaccount.com \
  --role roles/iam.workloadIdentityUser \
  --member "principalSet://iam.googleapis.com/projects/<PROJECT_NUMBER>/locations/global/workloadIdentityPools/<WIF_POOL>/attribute.repository/<REPO_OWNER>/<REPO_NAME>" \
  --project <PROJECT_ID>
gcloud kms keys add-iam-policy-binding <CI_KEY> --keyring <CMEK_RING> --location <LOCATION> --project <PROJECT_ID> \
  --member serviceAccount:<CI_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role roles/cloudkms.signerVerifier
```

`roles/cloudkms.signerVerifier` is the role the repository gives to its signing keys; that it is enough for cosign is **Not verified**.

6.3 Set the repository variables. These are the only ones that the workflows read for deployment (`grep 'vars\.' .github/workflows/*.yml`). No
workflow reads a repository secret: they use the workflow's own `github.token`.

| Variable | Read by | Value |
|---|---|---|
| `LK_COSIGN_KMS_KEY` | `images.yml`, job `publish` | the CI key as a cosign KMS URI: `gcpkms://projects/<PROJECT_ID>/locations/<LOCATION>/keyRings/<CMEK_RING>/cryptoKeys/<CI_KEY>`. `deploy/images/publish.sh` says "a KMS URI such as gcpkms://...". The exact form is **Not verified** |
| `LK_GCP_WIF_PROVIDER` | `images.yml`, job `publish` | the provider's full name: `projects/<PROJECT_NUMBER>/locations/global/workloadIdentityPools/<WIF_POOL>/providers/<WIF_PROVIDER>` (**Not verified**) |
| `LK_GCP_SERVICE_ACCOUNT` | `images.yml`, job `publish` | `<CI_GSA>@<PROJECT_ID>.iam.gserviceaccount.com` |
| `LK_RPM_WIF_PROVIDER`, `LK_RPM_SERVICE_ACCOUNT`, `LK_RPM_SIGNING_SECRET` | `sentinel.yml`, job `sign-release` | the RPM release key's own identity. Step 15 |
| `LK_BENCH_RUNNER` | `bench.yml`, `nightly.yml` | optional: the label of a larger runner. Not needed for an install |

If any of the first three is empty, `images.yml` skips the Cloud KMS signature and says so in a warning and in the job summary. The images are
still published and signed keyless. Binary Authorization then has nothing of yours to check.

6.4 Do not merge pull request A yet. Step 7 finishes it (the gcp values), and so does 11.3 (the database values). Its merge is a push to `main`, which runs
`images.yml` and publishes the images. Do 6.5 and 6.6 after the merge in 7.2.

6.5 After that merge, read the digests. Open the `images` run of your merge commit. The summary of `publish hub` and of `publish agent` has a line
`Published, signed and verified:` followed by a reference. The part after `@` is the digest.

6.6 Verify one yourself, if you want to. This is the check the workflow runs.

Source: `.github/workflows/images.yml`, the step "Verify the signature, the SBOM and the provenance from the registry". The workflow escapes
the dots of the owner and the name in the expression; do the same if yours have any.

```bash
just ops-tools-install cosign crane
export PATH="$PWD/target/ops-tools/bin:$PATH"
deploy/images/publish.sh verify "<HUB_IMAGE>@<HUB_DIGEST>" \
  --identity-regexp '^https://github\.com/<REPO_OWNER>/<REPO_NAME>/\.github/workflows/images\.yml@refs/(heads/main|tags/v.+)$' \
  --issuer https://token.actions.githubusercontent.com
```

6.7 If the GHCR packages are private, make a pull secret in the cluster, in the namespace `lanekeeper` (Q8, `docs/open-questions.md`, whose
default is an image pull secret with the secret's name in the values). The token needs package read access. **Not verified**: which GitHub
token type your organisation allows. Put the name in `hub:imagePullSecrets` of the hub values (step 12) and `agent:imagePullSecrets` of the agent values (step 15). Use `<PULL_SECRET>`
for the name.

**Not verified.** This is a Kubernetes Secret and not a Google one. It is written from memory: the JSON shape of `.dockerconfigjson`, and that `kubectl` reads it from standard input. `base64 -w0` is GNU's.

```bash
read -rs GHCR_TOKEN
printf '{"auths":{"ghcr.io":{"auth":"%s"}}}' "$(printf '%s' "<GITHUB_USER>:${GHCR_TOKEN}" | base64 -w0)" |
  kubectl -n lanekeeper create secret generic <PULL_SECRET> --type kubernetes.io/dockerconfigjson --from-file=.dockerconfigjson=/dev/stdin
unset GHCR_TOKEN
```

6.8 Move the placeholders (`plans/09-deployment-ci.md`, A29). Image digests in the values and `targetRevision` in the Application files are
placeholders (64 zeros and 40 zeros) until you replace them. `deploy/argocd/README.md`, "Moving a revision", is the procedure. In this guide:

| What | Where | Which pull request |
|---|---|---|
| hub digest | `hub:image.digest` in `deploy/values/hub/<ENV>.yaml` | B |
| agent digest | `agent:image.digest` in `deploy/values/agent/<SWIMLANE_ID>.yaml` | with the swimlane (step 15) |
| the database image | `db:image.digest`. It is a real digest already; the nightly job `db-image` scans it | A, only if you change it |
| `targetRevision` | the Application file | the second pull request of each round |

6.9 Leave Binary Authorization in dry run. `gcp:binaryAuthorization.enforcementMode` is `DRYRUN_AUDIT_LOG_ONLY` in the committed values, until
Q24 (is Binary Authorization available on the clusters?) is answered. The dry run writes an audit log entry for each image that would be
blocked and blocks nothing. Read `deploy/charts/gcp/README.md` ("Admission") before you switch it to `ENFORCED_BLOCK_AND_AUDIT_LOG`: the
third-party images in the cluster (the CloudNativePG operator, Argo CD, Config Connector) would be blocked unless you attest them or
list them.

**You should see.**

- Before the merge: the Cloud KMS key exists, and `LK_COSIGN_KMS_KEY`, `LK_GCP_WIF_PROVIDER` and `LK_GCP_SERVICE_ACCOUNT` are set in the repository's variables.
- After the merge in 7.2: two `publish` jobs are green, and their summaries each print a reference with a digest.
- The `publish.sh verify` command exits with 0.

**If not.** The summary says "No Cloud KMS signature": one of the three variables is empty. A `publish` job fails to authenticate: the provider's
condition does not match the run (the repository name, or the ref). The `publish` job of `images.yml` had never run on GitHub when this guide was
written, because it runs on `main` and on tags only (`plans/09-deployment-ci.blockers.md`, "What is still unproven"): its first run is the test. Whether a classic Binary Authorization
attestor accepts a cosign signature is unconfirmed (B12, point 1): the dry run is how you find out.

## 7. The gcp chart through Argo CD

**Why.** It makes everything in Google Cloud that the hub and the database need, so that you do not make it by hand. It makes containers
and keys, never values: you put the values in yourself (step 8, step 10).

**Do.**

7.1 In pull request A, edit `deploy/values/gcp/<ENV>.yaml`. Replace every example value. The committed `prod.yaml` and `staging.yaml` hold
example names. Put the database values (11.3) in the same pull request, so that one merge covers both.

| Value | What to put | Comes from |
|---|---|---|
| `gcp:project` | `<PROJECT_ID>` | step 1 |
| `gcp:projectNumber` | `<PROJECT_NUMBER>`, as a string | step 1 |
| `gcp:environment` | `<ENV>` | you |
| `gcp:location` | `<LOCATION>` | step 1 |
| `gcp:cloudArmor.policyName` | `<ARMOR_POLICY>`. The same string goes in `gateway.securityPolicyName` | you |
| `gcp:cloudArmor.usersRanges` | `<USERS_RANGES>` | the network team (Q2) |
| `gcp:cloudArmor.githubHookRanges` | nothing here: it comes from `deploy/values/hub/github-hook-ranges.yaml` (step 5) | step 5 |
| `gcp:buckets.backup.name`, `gcp:buckets.backup.cmekKey` | `<BACKUP_BUCKET>`, and the backups key as `projects/<PROJECT_ID>/locations/<LOCATION>/keyRings/<CMEK_RING>/cryptoKeys/<CMEK_KEY>` | step 1 |
| `gcp:buckets.auditCheckpoint.name`, `gcp:buckets.auditCheckpoint.cmekKey` | `<AUDIT_BUCKET>` and its key | step 1 |
| `gcp:buckets.auditCheckpoint.retentionPeriodSeconds`, `gcp:buckets.auditCheckpoint.isLocked` | prod: `7776000` (90 days) and `true`. **A lock cannot be undone.** Staging: never locked | `deploy/values/gcp/prod.yaml`; decided by a human on 2026-10-26 |
| `gcp:buckets.docs.name`, `gcp:buckets.docs.cmekKey` | `<DOCS_BUCKET>` and its key | step 1 |
| `gcp:cmek.diskKeys` | the disks key | step 1 |
| `gcp:kms.keyRing` | `<KEYRING>` | you. It cannot be deleted once made |
| `gcp:kms.protectionLevel` | `HSM` in prod (the policy requires it). `SOFTWARE` is allowed in staging | `deploy/policy/kms.rego` |
| `gcp:kms.keys.agentCa`, `gcp:kms.keys.replicaCa`, `gcp:kms.keys.auditCheckpoint`, `gcp:kms.keys.vault` | four key names. They are the last part of the hub's `config.kms.*` | you |
| `gcp:secrets.cmekKey` | the secrets key | step 1 |
| `gcp:binaryAuthorization.enforcementMode` | leave `DRYRUN_AUDIT_LOG_ONLY` | step 6 |
| `gcp:binaryAuthorization.publicKeys` | the CI key's public half (below) | step 6 |

Do not use the committed `prod.yaml` unchanged to try the chart out: its audit bucket is locked for 90 days. Start with `staging.yaml`.

Read the CI key's public half and put it in the values.

Source: `deploy/charts/gcp/README.md` ("Admission"), which uses version 1. **Not verified**: that a new key's first version is number 1; list the versions to see.

```bash
gcloud kms keys versions get-public-key 1 --key=<CI_KEY> --keyring=<CMEK_RING> --location=<LOCATION> --project=<PROJECT_ID> > ci-public.pem
```

```yaml
binaryAuthorization:
  publicKeys:
    - keyVersion: projects/<PROJECT_ID>/locations/<LOCATION>/keyRings/<CMEK_RING>/cryptoKeys/<CI_KEY>/cryptoKeyVersions/1
      signatureAlgorithm: ECDSA_P256_SHA256
      publicKeyPem: |
        -----BEGIN PUBLIC KEY-----
        (the lines of ci-public.pem)
        -----END PUBLIC KEY-----
```

7.2 Merge pull request A. That is the push that publishes the images: do 6.5 and 6.6 now. Then make the second pull request that sets `targetRevision` of `deploy/argocd/platform/gcp.yaml` (and of
`deploy/argocd/platform/db.yaml`) to `<REVISION>`, the commit of the first merge. Merge that. Then, on the platform cluster:

Source: `deploy/argocd/README.md` ("Moving a revision", step 4).

```bash
git pull
kubectl apply -n argocd -f deploy/argocd/platform/gcp.yaml
kubectl -n argocd get application lanekeeper-gcp -o jsonpath='{.status.sync.status}{"\n"}'
kubectl -n lanekeeper-gcp get storagebucket,kmskeyring,kmscryptokey,secretmanagersecret,iamserviceaccount,iampolicymember,computesecuritypolicy
```

The `jsonpath` is the one `docs/open-questions.md` (Q10) uses for the sync status. **Not verified**: the columns Config Connector prints.

7.3 What Config Connector makes (`deploy/charts/gcp/README.md`):

- three buckets (backups, audit checkpoints, docs) and the IAM on them;
- the service accounts `lanekeeper-hub` and `lanekeeper-db`, and their Workload Identity bindings;
- one key ring and four keys: the agent CA, the replica CA, the audit checkpoint key and the vault key;
- ten Secret Manager containers, and the roles of the hub's account on each key and each secret;
- the Cloud Armor policy;
- the Binary Authorization policy, attestor and note.

7.4 What you do by hand afterwards, in this order (`deploy/charts/gcp/README.md`, "What a person still does, in this order"):

| Do | Where |
|---|---|
| read the three key versions off the keys | below |
| put the values into the secrets | step 8 |
| make the two CA certificates | step 10 |
| name the key versions in the hub values | step 12 |

Read the key versions.

Source: `deploy/charts/gcp/README.md`, step 3 of "What a person still does". The README marks as not verified that Cloud KMS makes a first version when the key is made, and that it is number 1.

```bash
gcloud kms keys versions list --key=<AGENT_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID> --format='value(name)'
```

Run it for each of `gcp:kms.keys.agentCa` (use that name as `<AGENT_CA_KEY_NAME>`), `gcp:kms.keys.replicaCa` and `gcp:kms.keys.auditCheckpoint`. Each prints `projects/.../cryptoKeyVersions/N`.
That string is `config.kms.agentCa` (`LK_AGENT_CA_KMS_REF`), `config.kms.replicaCa` (`LK_REPLICA_CA_KMS_REF`) and `config.kms.auditCheckpoint`
(`LK_AUDIT_CHECKPOINT_KMS_REF`). Write the first two down as `<AGENT_CA_KEY_VERSION>` and `<REPLICA_CA_KEY_VERSION>`. The vault key
needs no version: `config.kms.vault` is `projects/<PROJECT_ID>/locations/<LOCATION>/keyRings/<KEYRING>/cryptoKeys/<vault key name>`.

**You should see.**

- `Synced` for the Application `lanekeeper-gcp`.
- Every Config Connector object ready.
- One version for each of the three signing keys.

**If not.** The Application shows the chart's refusal: the hook ranges are empty (step 5) or `binaryAuthorization.publicKeys` is empty (step 6). An object
stays not ready: read its condition with `kubectl -n lanekeeper-gcp describe <kind> <name>`. The usual causes are an API that is not
enabled (step 1), a Google service agent that does not exist (step 1.4), a key that is not in the same location as what it protects, or Config
Connector's account lacking a role (step 3). Config Connector's behaviour when it finds a ring or a key already there is not verified
(`deploy/charts/gcp/README.md`, "Not verified"): do not make the hub's ring or keys by hand.

## 8. Secret values

**Why.** The hub reads ten secrets from Secret Manager. The chart made the containers and the access. It never makes values (S7). The hub
does not start without most of them.

**Do.** The table says what each secret holds and who reads it. The commands are below it.

| Secret | What goes in | Made by | Read by | Hub value that names it |
|---|---|---|---|---|
| `lanekeeper-web-cert` | the PEM chain of the web certificate, leaf first, at most 8 certificates and 64 KiB | you, from your certificate (step 12) | every hub replica, at start and every `LK_WEB_CERT_RELOAD_SECS` (300) | `config.secretManager.webCert` (`LK_WEB_CERT_REF`) |
| `lanekeeper-web-key` | the PEM private key of that certificate, at most 16 KiB | you | the same | `config.secretManager.webKey` (`LK_WEB_KEY_REF`) |
| `lanekeeper-agent-ca-cert` | the PEM certificate of the agent CA | `hub ca-init` (step 10) | every hub replica, at start | `config.secretManager.agentCaCert` (`LK_AGENT_CA_CERT_REF`) |
| `lanekeeper-replica-ca-cert` | the PEM certificate of the replica CA. It must differ | `hub ca-init` (step 10) | every hub replica, at start | `config.secretManager.replicaCaCert` (`LK_REPLICA_CA_CERT_REF`) |
| `lanekeeper-entra-client` | the app registration's client secret | the Entra admin (step 9) | every hub replica, at start | `config.secretManager.entraClient` (`LK_ENTRA_CLIENT_AUTH_REF`) |
| `lanekeeper-login-cookie` | 32 random bytes, base64url | `openssl` (below) | every hub replica, at start | `config.secretManager.loginCookie` (`LK_ENTRA_COOKIE_KEY_REF`) |
| `write-preview-key` | at least 32 characters. **The hub does not start without it** | `openssl` (below) | every hub replica, at start | none: a constant of `crates/hub-writes/src/config.rs` |
| `github-read-token` | a read-only GitHub credential, fine-grained, for both Git repositories | the GitHub admin | the hub, for Git fetches | none: the default name; `LK_GIT_READ_REF` could change it |
| `github-webhook-secret` | a random secret that GitHub also holds | `openssl` (below) | the hub, for each webhook | none: the default name; `LK_GIT_WEBHOOK_REF` could change it |
| `mcp-token-hmac-key` | at least 32 random bytes, base64url, with no padding. Read only when `LK_MCP_AUTH` is `personal_token`; then the hub does not start without it | `openssl` (below) | the hub, only in that mode | none: a constant of `crates/hub-mcp/src/auth/pat.rs` |

Six of these names are the defaults of `config.secretManager.*`, and four are constants of the hub's code. The gcp chart makes the same ten containers
(`deploy/tests/render.sh contract` compares the two charts). The Teams secrets are below the commands.

8.1 The certificate and the key. They are files you hold: `web-cert.pem` (the chain, leaf first) and `web-key.pem`. Step 12.4 says what the repository
does and does not say about them. When both secrets and the Gateway's certificate (12.2) are made, delete the local copy of the key.

Source: `deploy/charts/gcp/README.md`, step 5 of "What a person still does", which shows `printf %s "<value>" | gcloud secrets versions add <name> --data-file=- --project=<project>`. **Not verified**: that `--data-file` also takes a file name.

```bash
gcloud secrets versions add lanekeeper-web-cert --data-file=web-cert.pem --project=<PROJECT_ID>
gcloud secrets versions add lanekeeper-web-key --data-file=web-key.pem --project=<PROJECT_ID>
```

8.2 The ones that you generate. Nothing is printed, and nothing is kept. The format of each is from the code: `crates/hub-identity/src/oidc/login.rs` (32 bytes as base64url, with or
without padding), `crates/hub-writes/src/config.rs` (at least 32 characters) and `crates/hub-mcp/src/auth/token.rs` (base64url without padding).

Source: the pattern of `deploy/charts/gcp/README.md`, step 5. **Not verified**: the `gcloud` flags.

```bash
openssl rand -base64 32 | tr '+/' '-_' | tr -d '=\n' | gcloud secrets versions add lanekeeper-login-cookie --data-file=- --project=<PROJECT_ID>
openssl rand -hex 32 | tr -d '\n' | gcloud secrets versions add write-preview-key --data-file=- --project=<PROJECT_ID>
openssl rand -hex 32 | tr -d '\n' | gcloud secrets versions add github-webhook-secret --data-file=- --project=<PROJECT_ID>
```

`mcp-token-hmac-key` needs no value unless you set `LK_MCP_AUTH` to `personal_token` (the default, `oauth`, reads no secret of its own). In that case make it the same way as
`lanekeeper-login-cookie`, but note that it must be base64url **without padding**: the `tr -d '=\n'` does that.

8.3 The read-only GitHub credential. The GitHub admin makes a fine-grained token with read access to both Git repositories (Q12 and Q17,
`docs/open-questions.md`). **Not verified**: GitHub's screens. Then, with the token on the clipboard:

Source: the pattern of `deploy/ADDING-A-SWIMLANE.md` (`read -rs`), and of `deploy/charts/gcp/README.md`, step 5. **Not verified**: the `gcloud` flags.

```bash
read -rs VALUE
printf '%s' "$VALUE" | gcloud secrets versions add github-read-token --data-file=- --project=<PROJECT_ID>
unset VALUE
```

The client secret of Entra goes in the same way, in step 9. The two CA certificates go in step 10.

8.4 Check that every secret has an enabled version.

**Not verified: check against Google's current documentation.**

```bash
gcloud secrets versions list lanekeeper-web-cert --project=<PROJECT_ID>
```

Run it for each secret that has a value once its step is done.

8.5 **Teams destinations are a run-time step.** An administrator names each destination in the app (`notification_settings.destination_ref`),
so the chart cannot make its secret ahead of time. For each one, after the administrator has chosen `<TEAMS_SECRET>`:

1. Make a container. Use the location and the customer-managed key of the other secrets (`gcp:location`, `gcp:secrets.cmekKey`).
2. Add a version that holds the webhook URL, in the same way as above.
3. Allow the hub's account to read it. The hub rejects a URL whose host is not on its list: `*.logic.azure.com`, `*.api.powerplatform.com` and
   `*.webhook.office.com` by default (`LK_TEAMS_ALLOWED_HOSTS` in `crates/hub/HARDENING.md`; the chart cannot change it).
4. Let the hub reach the host. The egress policy names it only if `hub:network.teamsWebhookHost` is set (`crates/hub/HARDENING.md`, "The network"). Set it to the host and move
   the revision of `hub.yaml`.

Source: `deploy/charts/gcp/README.md`, step 6 of "What a person still does", for the binding. **Not verified**: the other commands.

```bash
gcloud secrets create <TEAMS_SECRET> --replication-policy=user-managed --locations=<LOCATION> --kms-key-name=<SECRETS_KEY> --project=<PROJECT_ID>
read -rs TEAMS_URL
printf '%s' "$TEAMS_URL" | gcloud secrets versions add <TEAMS_SECRET> --data-file=- --project=<PROJECT_ID>
unset TEAMS_URL
gcloud secrets add-iam-policy-binding <TEAMS_SECRET> --member=serviceAccount:lanekeeper-hub@<PROJECT_ID>.iam.gserviceaccount.com --role=roles/secretmanager.secretAccessor --project=<PROJECT_ID>
```

**You should see.** Each secret that has a value listed with a version whose state is enabled. For a Teams destination, the administrator's first notification arrives in the channel.

**If not.** The hub starts and logs a failure that names a secret: the container is missing, the value is, or the hub's account has no access to it. The
name is in the log and never the value.

## 9. Entra ID: the app registration and the first administrators

**Why.** Users sign in through Entra. MCP clients such as Cursor get Entra access tokens. The hub accepts a token only from your tenant, for its
own audience and scope, for a user who is active at that moment (`crates/hub/HARDENING.md`, "The MCP server").

**Do.** An Entra administrator does 9.1 to 9.5. The facts are from `crates/hub/README.md`, item 10 of "What the chart and the install guide must match". **Not verified**: the screens of the Entra portal, and the Azure
CLI if you use it.

9.1 Make one app registration, **single tenant**. Note its tenant id (`<ENTRA_TENANT_ID>`) and application (client) id (`<ENTRA_CLIENT_ID>`).

9.2 Add the redirect URI `https://<HUB_HOST>/auth/callback`. It is `LK_PUBLIC_ORIGIN` plus the path in `LK_ENTRA_REDIRECT_PATH`, which defaults to `/auth/callback`.

9.3 Make a client secret and store it. The federated credential is not offered until Q25 is answered.

Source: the pattern of `deploy/ADDING-A-SWIMLANE.md` (`read -rs`) and `deploy/charts/gcp/README.md`, step 5. **Not verified**: the `gcloud` flags.

```bash
read -rs VALUE
printf '%s' "$VALUE" | gcloud secrets versions add lanekeeper-entra-client --data-file=- --project=<PROJECT_ID>
unset VALUE
```

9.4 For MCP, on the same registration (the defaults of `crates/hub/HARDENING.md`, "What OAuth takes from the Entra settings above"):

- the application ID URI is `api://<ENTRA_CLIENT_ID>`;
- add the scope `mcp.access`;
- issue v2 access tokens: the README writes `accessTokenAcceptedVersion: 2`;
- pre-authorise Cursor's client id `<CURSOR_CLIENT_ID>` for the scope (D91). Cursor has a registration of its own: a public client with the
  redirect `http://localhost:8787/callback` (Q21 in `docs/open-questions.md`).

**Not verified**: the manifest key for the token version has a different name in some Entra manifest editors (`requestedAccessTokenVersion`
in the Microsoft Graph one, from memory). What matters is that the access tokens are v2.

9.5 Choose the first administrators and find their Entra **object ids** (`<ENTRA_ADMIN_OBJECT_IDS>`). They go in `config.bootstrapAdmins` in
step 12. At their first sign-in they become active administrators. Everyone else starts pending. Remove them once real administrators
exist (step 14).

**You should see.** Nothing yet that you can test. Step 14 is the test.

**If not.** Later, a sign-in that bounces back to Entra with a redirect error means the URI in 9.2 is not exactly `LK_PUBLIC_ORIGIN` plus
`/auth/callback`. A 403 on every MCP call means the scope, the audience or the token version is wrong.

## 10. The two certificate authorities

**Why.** The hub refuses to start without two CA certificates, one for agents and sentinels and one for the replicas. The hub holds no CA
key: each CA is a Cloud KMS key version, and `hub ca-init` makes its certificate once (`crates/hub/README.md`, "Making the CA certificates").
**They must be two keys and two secrets**: an agent's certificate must never be accepted as a replica's (S5). A hub given one key for both, or one
secret for both, exits 2 and names the second variable (`crates/hub/HARDENING.md`).

**A problem to know before you run it.** The README says "a person" runs `hub ca-init`, as "an identity that may read the key's public half and sign
with it". But the hub binary gets its Google token from the metadata server only (`crates/hub-platform/src/gcp/token.rs`). So it cannot run on a laptop
with your own login. It must run where a metadata server answers for a service account that has the two roles. This guide uses a pod with Workload
Identity. Step 18 lists the difference between the README and the code.

**Do.**

10.1 Make a temporary identity for it. The role names and the binding commands are from the README of the gcp chart (it grants them to a person; here they
go to a service account). It gets no role yet: the two roles go on **one** key at a time (10.2, 10.3) and come off before the next, so that no identity can sign
with both CAs at once. An agent's certificate must never pass for a replica's (S5), and one account that held both keys, even for ten minutes, is the shape of that mistake.
Remove all of it in 10.6.

Source: `deploy/ADDING-A-SWIMLANE.md`, step 2 (the Workload Identity binding of a Kubernetes account). **Not verified**: the service-account commands.

```bash
gcloud iam service-accounts create <CA_INIT_GSA> --project <PROJECT_ID>
gcloud iam service-accounts add-iam-policy-binding <CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com \
  --project <PROJECT_ID> --role roles/iam.workloadIdentityUser \
  --member "serviceAccount:<PROJECT_ID>.svc.id.goog[lanekeeper/<CA_INIT_KSA>]"
kubectl -n lanekeeper create serviceaccount <CA_INIT_KSA>
kubectl -n lanekeeper annotate serviceaccount <CA_INIT_KSA> iam.gke.io/gcp-service-account=<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com
```

`<AGENT_CA_KEY_NAME>` and `<REPLICA_CA_KEY_NAME>` are `gcp:kms.keys.agentCa` and `gcp:kms.keys.replicaCa`. Both are defined in step 7.

The pods of 10.2 and 10.3 are not the hub's, and the chart's hardening does not reach them, so each carries it in `--overrides`: non-root, a read-only root filesystem, no capabilities,
the `RuntimeDefault` seccomp profile, and no Kubernetes token (Workload Identity asks the metadata server and does not use it). `ca-init` writes no file (`crates/hub/HARDENING.md`), so it needs no writable
volume. **Not verified**: the `--overrides` JSON of `kubectl run` (that the container entry is merged into the generated one by its name, which `kubectl run` gives the pod's name), that the image's
user 65532 exists, and that your cluster's policies let the pod run. If the pod does not start for one of those reasons, say so in a change to this guide; do not drop the hardening to make it start.

10.2 Run `hub ca-init` for the agent CA: give the temporary account its two roles on the agent key, run the pod, take the roles off. The image's entry point is the `hub` binary,
so the arguments start with `ca-init`. The options are from the README: `--kms-ref` is the key **version**, `--common-name` is for people, and `--valid-days` is 1 to 3650 and is the date
you must renew the CA by.

Source: `deploy/charts/gcp/README.md`, step 4 of "What a person still does" (the two roles on one key, with `add-iam-policy-binding`). **Not verified**: that these two roles are enough, and the exact commands.

```bash
for role in roles/cloudkms.publicKeyViewer roles/cloudkms.signerVerifier; do
  gcloud kms keys add-iam-policy-binding <AGENT_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID> \
    --member=serviceAccount:<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role="$role"
done
```

```bash
kubectl -n lanekeeper run ca-init-agent --restart=Never --image "<HUB_IMAGE>@<HUB_DIGEST>" \
  --overrides '{"spec":{"serviceAccountName":"<CA_INIT_KSA>","automountServiceAccountToken":false,"imagePullSecrets":[{"name":"<PULL_SECRET>"}],"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"ca-init-agent","securityContext":{"allowPrivilegeEscalation":false,"privileged":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}}]}}' \
  -- ca-init --kms-ref "<AGENT_CA_KEY_VERSION>" --common-name "Lanekeeper agent CA" --valid-days 3650
kubectl -n lanekeeper wait --for=jsonpath='{.status.phase}'=Succeeded pod/ca-init-agent --timeout=180s
kubectl -n lanekeeper logs pod/ca-init-agent | sed -n '/-----BEGIN CERTIFICATE-----/,/-----END CERTIFICATE-----/p' > agent-ca.pem
kubectl -n lanekeeper delete pod ca-init-agent
```

Leave out `imagePullSecrets` if the packages are public. The `sed` keeps only the certificate: the command prints its own messages on standard error, and the log merges the two.

Take the two roles off the agent key **before 10.3, whether the pod succeeded or not**. If it failed, fix the cause (see "If not") and begin 10.2 again.

Source: `deploy/charts/gcp/README.md`, step 4 ("remove-iam-policy-binding"). **Not verified**: the exact commands.

```bash
for role in roles/cloudkms.publicKeyViewer roles/cloudkms.signerVerifier; do
  gcloud kms keys remove-iam-policy-binding <AGENT_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID> \
    --member=serviceAccount:<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role="$role"
done
```

10.3 The same for the replica CA, with its own key, key version and name: its two roles on the replica key, the pod, and the roles off.

Source and **Not verified**: as 10.2.

```bash
for role in roles/cloudkms.publicKeyViewer roles/cloudkms.signerVerifier; do
  gcloud kms keys add-iam-policy-binding <REPLICA_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID> \
    --member=serviceAccount:<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role="$role"
done
```

```bash
kubectl -n lanekeeper run ca-init-replica --restart=Never --image "<HUB_IMAGE>@<HUB_DIGEST>" \
  --overrides '{"spec":{"serviceAccountName":"<CA_INIT_KSA>","automountServiceAccountToken":false,"imagePullSecrets":[{"name":"<PULL_SECRET>"}],"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"ca-init-replica","securityContext":{"allowPrivilegeEscalation":false,"privileged":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}}]}}' \
  -- ca-init --kms-ref "<REPLICA_CA_KEY_VERSION>" --common-name "Lanekeeper replica CA" --valid-days 3650
kubectl -n lanekeeper wait --for=jsonpath='{.status.phase}'=Succeeded pod/ca-init-replica --timeout=180s
kubectl -n lanekeeper logs pod/ca-init-replica | sed -n '/-----BEGIN CERTIFICATE-----/,/-----END CERTIFICATE-----/p' > replica-ca.pem
kubectl -n lanekeeper delete pod ca-init-replica
```

Source and **Not verified**: as 10.2.

```bash
for role in roles/cloudkms.publicKeyViewer roles/cloudkms.signerVerifier; do
  gcloud kms keys remove-iam-policy-binding <REPLICA_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID> \
    --member=serviceAccount:<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role="$role"
done
```

10.4 Check both files. The hub's own check at start is stricter, but you find a mistake here and not at the first rollout.

```bash
for f in agent-ca.pem replica-ca.pem; do
  grep -c 'BEGIN CERTIFICATE' "$f"
  openssl x509 -in "$f" -noout -subject -dates -ext basicConstraints,keyUsage
done
```

10.5 Store them, and keep `agent-ca.pem`: it is the public certificate that every swimlane's `agent:hubCa.pem` needs (step 15).

Source: `crates/hub/README.md` ("Making the CA certificates") says to store the output in the secret that `LK_AGENT_CA_CERT_REF` (or `LK_REPLICA_CA_CERT_REF`) names. **Not verified**: the `gcloud` flags.

```bash
gcloud secrets versions add lanekeeper-agent-ca-cert --data-file=agent-ca.pem --project=<PROJECT_ID>
gcloud secrets versions add lanekeeper-replica-ca-cert --data-file=replica-ca.pem --project=<PROJECT_ID>
```

10.6 Remove the temporary identity. **Run this even if 10.2 or 10.3 failed or you stopped half way**: a run that stopped leaves the two roles on the key it had reached, and the account
and its Workload Identity binding exist until you remove them. A command that names a binding which is already gone may report an error (**Not verified**); go on to the next.

Source: `deploy/charts/gcp/README.md`, step 4 ("remove-iam-policy-binding"). **Not verified**: the other commands.

```bash
kubectl -n lanekeeper delete serviceaccount <CA_INIT_KSA>
for key in <AGENT_CA_KEY_NAME> <REPLICA_CA_KEY_NAME>; do
  for role in roles/cloudkms.publicKeyViewer roles/cloudkms.signerVerifier; do
    gcloud kms keys remove-iam-policy-binding "$key" --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID> \
      --member=serviceAccount:<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role="$role"
  done
done
gcloud iam service-accounts delete <CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --project <PROJECT_ID>
```

Then check that no role is left on either key (**Not verified**: the command): `gcloud kms keys get-iam-policy <AGENT_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --project=<PROJECT_ID>`, and the same for `<REPLICA_CA_KEY_NAME>`, must not list `<CA_INIT_GSA>`.

**You should see.**

- Each file has exactly one certificate (`1` from `grep -c`). Its subject is the common name you gave.
- `X509v3 Basic Constraints` says `CA:TRUE, pathlen:0` and the key usage is `Certificate Sign, CRL Sign` (the README: a CA with a path length of 0, `keyCertSign` and `cRLSign`).
- The two files differ.

**If not.** Exit code 1 (the pod fails): KMS refused or could not be reached. The log's message names the step: reading the public key, or signing. Check the two
roles on **that** key and that the pod really runs as the temporary account. Exit code 2: an option is missing or invalid. The message names the option and never repeats
its value. `--kms-ref` without a version is refused. If the public key cannot be read although the two roles are on the key, `roles/cloudkms.signerVerifier` may not be enough;
the roles in the README are the ones to use (**Not verified**).

## 11. The database

**Why.** The hub needs PostgreSQL 16 with pgvector and pg_trgm, TLS 1.3 only, verified in full. CloudNativePG runs it as three instances
in three zones (`deploy/charts/db/README.md`).

**Do.**

11.1 Install the CloudNativePG operator, version 1.30.1. It is cluster-scoped, and the chart does not install it. These are the steps of the kind test, which pins the manifest's hash and the
operator's image digest.

Source: `deploy/tests/kind/run.sh`, `install_operator`, and the pins in `deploy/tests/kind/pins.env`.

```bash
. deploy/tests/kind/pins.env
curl --fail --silent --show-error --location --output cnpg.yaml "$LK_CNPG_MANIFEST_URL"
echo "$LK_CNPG_MANIFEST_SHA256  cnpg.yaml" | sha256sum --check
sed -i "s#${LK_CNPG_OPERATOR_IMAGE%@*}\$#${LK_CNPG_OPERATOR_IMAGE}#" cnpg.yaml
grep -c -F "$LK_CNPG_OPERATOR_IMAGE" cnpg.yaml
kubectl apply --server-side -f cnpg.yaml
kubectl -n cnpg-system rollout status deployment/cnpg-controller-manager --timeout=300s
```

11.2 Make the two Secrets. They never go in Git. The hub reads the key `uri` of `lanekeeper-db-app`; the migration Job reads the key `uri` of
`lanekeeper-db-migrator`. The passwords are made here and kept nowhere else. They go to the API server on standard input, so they are not in the shell history and not in the
process list (`deploy/tests/kind/run.sh` does it the same way; `deploy/charts/db/README.md`, "Before it can start", explains why the migrator has its own Secret).

Source: `deploy/tests/kind/run.sh`, `install_database`, and `deploy/charts/db/README.md`.

```bash
migrator_pw="$(openssl rand -hex 32)"
app_pw="$(openssl rand -hex 32)"
kubectl apply -f - <<EOF
apiVersion: v1
kind: Secret
metadata: {name: lanekeeper-db-migrator, namespace: lanekeeper}
type: kubernetes.io/basic-auth
stringData:
  username: lanekeeper_migrator
  password: "$migrator_pw"
  uri: "postgresql://lanekeeper_migrator:$migrator_pw@lanekeeper-db-rw:5432/lanekeeper"
---
apiVersion: v1
kind: Secret
metadata: {name: lanekeeper-db-app, namespace: lanekeeper}
type: kubernetes.io/basic-auth
stringData:
  username: lanekeeper_app
  password: "$app_pw"
  uri: "postgresql://lanekeeper_app:$app_pw@lanekeeper-db-rw:5432/lanekeeper"
EOF
unset migrator_pw app_pw
```

The URLs name no `sslmode`. The hub always connects with `verify-full` against `lanekeeper-db-ca`, whatever the URL says (`deploy/charts/db/README.md`).
Do not lose the Secrets: the passwords are in them and nowhere else.

11.3 In pull request A, edit `deploy/values/db/<ENV>.yaml`.

| Value | What to put |
|---|---|
| `db:environment` | `<ENV>` |
| `db:image.digest` | keep the committed digest (a real one); `db:image.tag` goes with it |
| `db:storage.cmekKey` | the disks key (step 1). The same key as `hub:storage.cmekKey` |
| `db:backup.bucket` | `<BACKUP_BUCKET>` |
| `db:serviceAccount.gcpServiceAccount` | `lanekeeper-db@<PROJECT_ID>.iam.gserviceaccount.com`, which the gcp chart makes |

11.4 When the gcp Application is `Synced` (step 7) and the second pull request of round A is merged:

```bash
kubectl apply -n argocd -f deploy/argocd/platform/db.yaml
kubectl -n lanekeeper wait --for=condition=Ready cluster.postgresql.cnpg.io/lanekeeper-db --timeout=900s
kubectl -n lanekeeper get cluster.postgresql.cnpg.io/lanekeeper-db -o wide
kubectl -n lanekeeper get pods -l cnpg.io/cluster=lanekeeper-db
kubectl -n lanekeeper get secret lanekeeper-db-ca
```

The first three commands are those of `deploy/tests/kind/run.sh`. The 900 seconds are its bound; the kind run needed 97.

**You should see.**

- `2` from the `grep -c` (the manifest names the operator's image twice), and the operator's rollout complete.
- `cluster.postgresql.cnpg.io/lanekeeper-db condition met`.
- Three instances (`lanekeeper-db-1`, `-2`, `-3`) running, one in each zone.
- The Secret `lanekeeper-db-ca`, which the operator makes.

**If not.** `kubectl -n lanekeeper describe cluster.postgresql.cnpg.io/lanekeeper-db` and the pods' events say why. A pod Pending with an unbound volume: the Compute Engine service agent has no use of the disks key
(`gcp:cmek.diskKeys`), or fewer than three zones have room. An image pull error: the CloudNativePG image is public on GHCR (the committed digest), so check the node's network.
Backups fail: the account `lanekeeper-db` cannot write to `<BACKUP_BUCKET>`. The operator's webhook not ready: wait for its rollout.

## 12. The hub chart

**Why.** This is the application. It is the first time the whole contract is exercised: **the production image has never been Ready in CI**, because it
cannot be without real Google services and Entra (`plans/09-deployment-ci.chart-alignment.md`, section 10). So the readiness of three replicas and the
startup time (P11) are measured here for the first time (step 16).

**Do.**

12.1 Reserve the Gateway's global external IP address. `<GATEWAY_IP_NAME>` is `gateway.addressName`. The chart refuses to render a Gateway without one.

**Not verified: check against Google's current documentation.**

```bash
gcloud compute addresses create <GATEWAY_IP_NAME> --global --project <PROJECT_ID>
gcloud compute addresses describe <GATEWAY_IP_NAME> --global --format='value(address)' --project <PROJECT_ID>
```

12.2 Make the certificate that the Gateway shows to users, as a Compute Engine SSL certificate named `<SSL_CERT_NAME>`. It must be valid for `<HUB_HOST>`. The chart
never takes a Kubernetes Secret for it (`deploy/charts/hub/templates/gateway.yaml`). It may be the same files as the pod's (12.4).

**Not verified: check against Google's current documentation.**

```bash
gcloud compute ssl-certificates create <SSL_CERT_NAME> --certificate web-cert.pem --private-key web-key.pem --global --project <PROJECT_ID>
```

12.3 The seed file. Without it no agent can join. It lists the service account of every swimlane's agent and every sentinel, and it is a reviewed change (whoever applies it makes a Google service account
the agent of a swimlane). **The real file lives outside this repository** (AGENTS.md, rule 12). Step 15 shows its shape. For the first install, make a file
that holds `version: 1` and the project row, and add the swimlanes in step 15 (**Not verified**: a file with only a project was not run). Then make the ConfigMap that
`seed.existingConfigMap` names. **Its name ends with the revision**, which is 1 for the first install (`lanekeeper-hub-seed-<ENV>-1`), and the chart refuses a name that does not.
The seed Job is the record of which file was applied, and the ConfigMap is the file: the chart keeps a ConfigMap only when it made it itself (`seed.content`, as kind does), so for this one
**the record holds only while you keep every version**. Every later change of the seed is a new ConfigMap and a new revision (17.3), and you never edit, replace or delete an old one in place.

Source: the comment in `deploy/values/hub/prod.yaml`, on `seed`.

```bash
kubectl -n lanekeeper create configmap lanekeeper-hub-seed-<ENV>-1 --from-file=seed.yaml=<SEED_FILE>
```

12.4 The web certificate and key. The hub serves HTTPS itself on its web port, and the load balancer speaks HTTPS to it (the Service `lanekeeper-hub-web` has `appProtocol: HTTPS`,
`deploy/charts/hub/templates/services.yaml`). The pod reads its certificate chain and key from the two secrets of step 8, not from a Kubernetes Secret. **What the repository says**
(`crates/hub/src/web_tls.rs`, `crates/hub/HARDENING.md`): the chain is leaf first, at most 8 certificates and 64 KiB; the key matches the leaf; a replacement that does not parse,
does not match or has expired changes nothing; and `/readyz` fails once the held certificate has expired. **What it does not say, and you must decide**: which CA issues it, which names it
carries, and who renews it. **Not verified**: whether Google's load balancer checks the backend certificate's name or chain. Until you know, use a certificate that is valid for `<HUB_HOST>` and
chains to a CA your browsers trust: the same files may serve both hops. Nothing rotates it for you (step 17).

12.5 In pull request B, edit `deploy/values/hub/<ENV>.yaml`. The chart refuses to render, and names the value, if a required one is missing. (This list was checked by rendering the chart with only these values, and then without each required one.) The last column says which are required:

| Value | What to put | Comes from | Required by the chart |
|---|---|---|---|
| `hub:image.digest` | `<HUB_DIGEST>` | step 6 | yes |
| `hub:image.repository` | `<HUB_IMAGE>`. Change it if your repository is not the one in the file | step 6 | no: has a default |
| `hub:serviceAccount.gcpServiceAccount` | `lanekeeper-hub@<PROJECT_ID>.iam.gserviceaccount.com` | the gcp chart | yes |
| `hub:network.agentSourceRanges` | `<AGENT_RANGES>`. Never empty or the internet | the network team (Q1); step 15 adds each swimlane | yes |
| `gateway.hostname` | `<HUB_HOST>`. It becomes `LK_PUBLIC_ORIGIN` and `LK_HUB_AUDIENCE`, both `https://<HUB_HOST>` | the network team (Q2) | yes |
| `gateway.addressName` | `<GATEWAY_IP_NAME>` | 12.1 | yes |
| `gateway.tls.preSharedCerts` | `<SSL_CERT_NAME>`. Exactly one of this and `gateway.tls.certManagerCerts` | 12.2 | yes |
| `gateway.securityPolicyName` | `<ARMOR_POLICY>` | step 7 | no: defaults to `lanekeeper-hub`, which is also the default of `gcp:cloudArmor.policyName`; the two must be the same string |
| `config.gcpProject` | `<PROJECT_ID>` (`LK_GCP_PROJECT`) | step 1 | yes |
| `config.entra.tenantId` | `<ENTRA_TENANT_ID>` (`LK_ENTRA_TENANT_ID`) | step 9 | yes |
| `config.entra.clientId` | `<ENTRA_CLIENT_ID>` (`LK_ENTRA_CLIENT_ID`) | step 9 | yes |
| `config.kms.agentCa` | `<AGENT_CA_KEY_VERSION>` (`LK_AGENT_CA_KMS_REF`) | step 7 | yes |
| `config.kms.replicaCa` | `<REPLICA_CA_KEY_VERSION>` (`LK_REPLICA_CA_KMS_REF`). A different key | step 7 | yes |
| `config.kms.auditCheckpoint` | the checkpoint key version (`LK_AUDIT_CHECKPOINT_KMS_REF`) | step 7 | yes |
| `config.kms.vault` | the vault key **without** a version (`LK_VAULT_KMS_REF`) | step 7 | yes |
| `config.auditCheckpointBucket` | `<AUDIT_BUCKET>` (`LK_AUDIT_CHECKPOINT_BUCKET`) | step 7 | yes |
| `config.agentEndpointHosts` | `<AGENT_HOST>` (`LK_AGENT_ENDPOINT_HOSTS`): at most 8, lower case | the network team (Q2) | yes |
| `config.git.baseUrl`, `config.git.tenantUrl` | `https://github.com/<GITHUB_ORG>/<BASE_REPO>.git` and `https://github.com/<GITHUB_ORG>/<TENANT_REPO>.git` (`LK_GIT_BASE_URL`, `LK_GIT_TENANT_URL`) | you | yes |
| `config.git.baseRepo`, `config.git.tenantRepo` | `<GITHUB_ORG>/<BASE_REPO>` and `<GITHUB_ORG>/<TENANT_REPO>` (`LK_GIT_BASE_REPO`, `LK_GIT_TENANT_REPO`) | you | yes |
| `hub:storage.cmekKey` | the disks key. Without it the volumes fall back to Google's keys with a warning | step 1 | no, but without it the volumes fall back to Google-managed keys and the policy warns |
| `seed.enabled`, `seed.existingConfigMap`, `seed.revision` | `true`, `lanekeeper-hub-seed-<ENV>-1`, `1` (a whole number, not a string). The chart refuses `seed.enabled` without a ConfigMap, a revision that is not a positive whole number, and a ConfigMap whose name does not end with `-` and the revision | 12.3 | no, but without a seed no agent can join |
| `config.bootstrapAdmins` | `<ENTRA_ADMIN_OBJECT_IDS>` (`LK_BOOTSTRAP_ADMINS`). Remove them later | step 9 | no |
| `hub:imagePullSecrets` | `<PULL_SECRET>`, only if the packages are private | step 6 | no |

These need no value, and the chart sets them: `config.secretManager.*` default to the six names of step 8 (`LK_WEB_CERT_REF` and the others); `config.trustedProxyHops` is 2 behind Google's external load
balancer (`LK_TRUSTED_PROXY_HOPS`; check it again on the real path); `LK_REPLICA_NAME` comes from the pod's name and `LK_REPLICA_ADDRESS` is built from it.
`config.otlpEndpoint` is empty, so the hub exports no traces. The optional settings the chart does not expose are listed in step 18.

12.6 Merge pull request B and make the second pull request that sets `targetRevision` of `deploy/argocd/platform/hub.yaml` to `<REVISION>`. Merge it, then apply the file.
Watch the three things that happen, in order.

```bash
kubectl apply -n argocd -f deploy/argocd/platform/hub.yaml
kubectl -n lanekeeper get jobs
kubectl -n lanekeeper logs job/lanekeeper-hub-migrate
kubectl -n lanekeeper get pods -l app.kubernetes.io/name=lanekeeper-hub
kubectl -n lanekeeper get pvc
kubectl -n lanekeeper logs job/<SEED_JOB>
```

- The migration Job `lanekeeper-hub-migrate` runs **before** the hub rolls (an Argo CD PreSync hook; **Not verified**: Argo CD's documentation was not reachable, and the hook mapping was written from its source). Its log has one JSON line for each migration it applied, then
  `"the schema is up to date"` with `"applied":8,"version":8` (`crates/hub/README.md`, "Migrating the database"; the migrations are `db/migrations/0001_init.sql` to
  `0008_blob_touch.sql`).
- The StatefulSet `lanekeeper-hub` starts three pods. Each fills its Git mirror volume from GitHub first, and is not ready until both repositories are fetched. That depends on the network, not on P11.
- The seed Job `lanekeeper-hub-seed-<hash>` (find the name with `kubectl get jobs`; use it as `<SEED_JOB>`) runs after the migration. Its report lists each row it created.

12.7 Check each replica's `/readyz`, through a port-forward to the **operations port** (9090). The check answers from memory, with a 2-second deadline.

Source: `crates/hub/README.md`, item 11, and `crates/hub/src/ready.rs`.

```bash
kubectl -n lanekeeper port-forward pod/lanekeeper-hub-0 9090:9090 &
curl -sS -i http://127.0.0.1:9090/readyz
kill %1
```

Repeat for `lanekeeper-hub-1` and `lanekeeper-hub-2`.

**You should see.**

- The migration Job complete (`COMPLETIONS 1/1`) and the seed Job complete.
- Three hub pods `1/1 Running`, and three volumes `mirror-lanekeeper-hub-0`, `-1` and `-2` bound.
- `HTTP/1.1 200 OK` and the body `ready` from each. A ready body may also say `ready (not available: docs_search)`: that is expected until prompt 14 lands, and it never gates.
- Time: note how long each pod took from start to ready (step 16).

**If not.** During a start that is failing, the startup probe fails too, because `/healthz` only answers once the hub has finished starting. That is the hub's behaviour (the hub README's "Probes"
paragraph says otherwise; step 18). Read the log, not the events: `kubectl -n lanekeeper logs lanekeeper-hub-0 --previous`. The hub exits **2** when a setting is missing or malformed
(the message names the variable and never its value) and **1** when a component cannot be reached or does not match (it names the component). A `503` from `/readyz` names the first check that
fails:

| Check | What it means | Usual cause |
|---|---|---|
| `postgres` | the database pool cannot reach the database | the database is not Ready; the Secret `lanekeeper-db-app` or `lanekeeper-db-ca`; the network policy |
| `schema` | a migration is missing | the migration Job did not run or failed (step 17); a restored older database |
| `certificates` | the web, agent or replica certificate that the hub holds has expired | the web certificate: step 17 |
| `events` | the event bus has lost its `LISTEN` connection | the database connection |
| `mirrors` | a Git mirror holds no branch | the first fetch has not finished, or GitHub or `github-read-token` is refusing |
| `jobs` | the leased job runner is not running | read the log |
| `tasks` | a task that should live as long as the process has ended | read the log |

## 13. Load balancers, DNS, Cloud Armor and the GitHub webhook

**Why.** Users reach the hub through a Gateway with Cloud Armor in front. Agents reach it through a TCP pass-through load balancer that accepts only listed addresses. GitHub reaches `/hooks/github`.

**Do.**

13.1 DNS. Make an A record `<HUB_HOST>` for the Gateway's address from 12.1. After the hub has synced, make an A record `<AGENT_HOST>` for the agent load balancer's address.
Reserve a regional address for it first if you want to know it beforehand: `hub:agentLoadBalancer.ipAddress`. Without one:

```bash
kubectl -n lanekeeper get service lanekeeper-hub-agent
kubectl -n lanekeeper get gateway lanekeeper-hub
```

13.2 Cloud Armor and source ranges. Three lists decide who may reach what. The first two are set by earlier steps.

| List | Where | Reaches |
|---|---|---|
| `<USERS_RANGES>` | `gcp:cloudArmor.usersRanges` (Cloud Armor, step 7) | the whole web app |
| GitHub's ranges | `deploy/values/hub/github-hook-ranges.yaml` (Cloud Armor, step 5) | `/hooks/github` and nothing else |
| `<AGENT_RANGES>` | `hub:network.agentSourceRanges` (the agent load balancer and the hub's network policy, one list) | port 443 of the agent load balancer, forwarded to the hub's 8444 |

The default Cloud Armor rule denies (403). To find a swimlane cluster's outbound IP, run the command in Q1 of `docs/open-questions.md` from a pod in that cluster.

13.3 Test the entry points.

```bash
curl -sS -o /dev/null -w '%{http_code}\n' https://<HUB_HOST>/
curl -sS -o /dev/null -w '%{http_code}\n' -X POST https://<HUB_HOST>/hooks/github
```

The first command runs from an address in `<USERS_RANGES>`. The second too: the webhook route is open to those ranges as well, and answers a request with no signature with a bare 401 (`crates/hub-platform/src/git/webhook.rs`).

13.4 The GitHub webhook, on **both** Git repositories (`<GITHUB_ORG>/<BASE_REPO>` and `<GITHUB_ORG>/<TENANT_REPO>`). **Not verified**: GitHub's screens.

| Setting | Value |
|---|---|
| Payload URL | `https://<HUB_HOST>/hooks/github` |
| Content type | `application/json`. The hub checks the HMAC over the raw body and parses that body as JSON; the form-encoded type would not parse (read from `webhook.rs`; **Not verified** by a delivery) |
| Secret | the value of `github-webhook-secret` |
| Events | `push`, and `pull_request`. Every other event is acknowledged and ignored. `ping` answers 200 |
| SSL verification | on |

Do not print the secret to the terminal: the scrollback and the shell's history keep it. Read it into a file only you can read, paste it from there into GitHub's field, and remove the file.

Source: `crates/hub-platform/src/git/webhook.rs` for what the hub does with each event. **Not verified**: the `gcloud` flags, and that `shred` removes the file on your file system (it may not on a journaled one or on an SSD).

```bash
(umask 077; gcloud secrets versions access latest --secret=github-webhook-secret --project=<PROJECT_ID> > webhook-secret.txt)
# open webhook-secret.txt in an editor that keeps no copy, copy the one line into GitHub's Secret field, close it, then:
shred -u webhook-secret.txt
```

Without a webhook, the hub polls every 60 seconds (Q28).

**You should see.**

- `kubectl get gateway` shows the Gateway with an address (**Not verified**: the columns).
- The first `curl` prints `200` from an allowed address and `403` from any other (Cloud Armor's default rule).
- The second prints `401` from an allowed user address, and `403` from an address that is in neither list.
- GitHub shows `200` for the first `ping`, and `202` for a push.

**If not.** `403` from an address that should be allowed: the range is not in the list, or the policy has not synced. `401` from GitHub: the secret differs from `github-webhook-secret`. No response at all: the
Gateway has no healthy backend: check `/readyz` (step 12.7), and that the health check's source ranges `35.191.0.0/16` and `130.211.0.0/22` can reach port 9090 (the hub's network policy admits them).
An agent whose connection times out: its outbound IP is not in `<AGENT_RANGES>`.

## 14. First sign-in and the first checks

**Why.** To show that Entra, the cookies, the load balancer and the database work together, and to replace the bootstrap administrators with real ones.

**Do.**

14.1 Open `https://<HUB_HOST>` in a browser from an allowed address, and sign in as one of the first administrators. The hub makes an active administrator of an
object id in `config.bootstrapAdmins` at its first sign-in. **Not verified**: the screens of the app.

14.2 Check who the hub thinks you are. While signed in, open `https://<HUB_HOST>/api/v1/me` (`api/openapi.yaml`).

14.3 Approve the other people who need access, in the app (`/api/v1/admin/users` is the API behind it). Everyone else starts pending. Each user adds their own fine-grained GitHub token at their first
sign-in after approval (D42).

14.4 Remove the bootstrap administrators once real administrators exist. They are a value, so the way out is a change: delete `config.bootstrapAdmins` from `deploy/values/hub/<ENV>.yaml`, merge, set
`targetRevision` of `hub.yaml` to the new commit, and apply the file again. The hub rolls. Git keeps the object ids in its history; the repository has no other
way to set them (step 18).

**You should see.** The app after sign-in, `/api/v1/me` showing your role as admin, and the users you approved able to sign in.

**If not.** Entra shows a redirect error: the URI of 9.2 differs from `https://<HUB_HOST>/auth/callback`. The hub answers 403 to every request: you are pending, so your object id is not in the list. A loop back to
sign-in: the cookies are not reaching the hub. The load balancer must pass `Cookie`, `Origin` and `X-CSRF-Token` unchanged and not cache `/api` or `/auth` (`crates/hub/README.md`, item 9). The hub checks
`Host` and `Origin` against `LK_PUBLIC_ORIGIN`, so `<HUB_HOST>` must be exactly the name you type.

## 15. A swimlane

**Why.** A swimlane is where the config lives: one cluster, one NFS VM, one agent and one sentinel. `deploy/ADDING-A-SWIMLANE.md` is the runbook and has the detail. Do not repeat it
here. This step gives the order, and the hand-offs between it and this guide.

**Do.** Do the runbook's steps in this order, not in its numbering. The agent must be let in (its step 5) before it starts (its step 4), and its identity must be registered (its step 2) before it joins.

| Order | Runbook step | What | Hand-off from this guide |
|---|---|---|---|
| 1 | step 1 | the swimlane's row in the hub | the seed file (15.1) and the ConfigMap |
| 2 | step 2 | Workload Identity, or a join token; the key for the spool's disk | `<PROJECT_ID>` of the swimlane, `<SWIMLANE_ID>` |
| 3 | step 5 | the cluster's and the NFS VM's outbound IPs in `hub:network.agentSourceRanges` | a pull request on the **hub** values, and the hub's revision (steps 12 and 17) |
| 4 | step 3 | `deploy/values/agent/<SWIMLANE_ID>.yaml` | `agent:hubCa.pem` is `agent-ca.pem` of step 10; `agent:hub.endpoint` is `https://<AGENT_HOST>`; `agent:hub.audience` is `https://<HUB_HOST>`; `agent:image.digest` is `<AGENT_DIGEST>`; `agent:network.hubAddresses` is the agent load balancer's address (step 13.1) |
| 5 | step 4 | the swimlane's Application, applied to **its own** Argo CD | the revision (step 6.8) |
| 6 | step 6 | the adoption pass, by an administrator in the app | a signed-in administrator (step 14) |

15.1 The seed file. Its shape is in `crates/hub/README.md` ("Registering swimlanes"), and `crates/hub/examples/seed.dev.yaml` is a synthetic example. A swimlane or a sentinel without a service account cannot join. A value that YAML could
read as a number (`010`, `1e3`) must be quoted. Keep the real file where your organisation keeps reviewed configuration, and keep every version you apply: the audit event records the file's hash and not the
person.

```yaml
version: 1
projects:
  - id: <HUB_PROJECT>
    display_name: <HUB_PROJECT_NAME>
swimlanes:
  - id: <SWIMLANE_ID>
    project: <HUB_PROJECT>
    display_name: <SWIMLANE_NAME>
    cluster: <SWIMLANE_CLUSTER>
    tier: <TIER>
    nfs_server: <NFS_SERVER>
    nfs_export: <NFS_EXPORT>
    mount_root: <MOUNT_ROOT>
    agent_service_account: lanekeeper-agent-<SWIMLANE_ID>@<SWIMLANE_PROJECT_ID>.iam.gserviceaccount.com
    tenants: [<TENANT>]
sentinels:
  - vm: <NFS_VM>
    swimlane: <SWIMLANE_ID>
    export_root: <EXPORT_ROOT>
    service_account: <SENTINEL_GSA>@<SWIMLANE_PROJECT_ID>.iam.gserviceaccount.com
```

To run the seed again with a changed file, make a **new** ConfigMap for it and move the revision (17.3). Do not replace the old ConfigMap or `kubectl apply` over it: the old seed Job points at it, and it is the only copy of the file that Job applied.

`hub seed --dry-run` previews the change and writes nothing, but the chart has no dry-run Job and the repository gives no procedure to run it in the cluster (step 18). Read the file carefully before you apply it.

15.2 The agent's values: copy `deploy/values/agent/sit1.yaml` to `deploy/values/agent/<SWIMLANE_ID>.yaml` and set these (the runbook's step 3 has the notes).
Every value below the digest is a fact of the swimlane.

| Value | What to put |
|---|---|
| `agent:image.digest` | `<AGENT_DIGEST>` |
| `agent:swimlane.id`, `agent:swimlane.cluster`, `agent:swimlane.project` | `<SWIMLANE_ID>`, `<SWIMLANE_CLUSTER>`, `<SWIMLANE_PROJECT_ID>` |
| `agent:hub.endpoint` | `https://<AGENT_HOST>`: a host name only, and one of the hub's `config.agentEndpointHosts` |
| `agent:hub.audience` | `https://<HUB_HOST>`. The hub refuses every join whose token is for another audience |
| `agent:configServer.namespace` | `<CONFIG_SERVER_NAMESPACE>` (15.3) |
| `agent:nfs.server`, `agent:nfs.exportPath` | `<NFS_SERVER>` and `<NFS_EXPORT>`. The uid and gid default to 1010 (Q11) |
| `agent:namespaces` | `<WATCHED_NAMESPACES>`. `kube-system` cannot be watched |
| `agent:serviceAccount.gcpServiceAccount` | `lanekeeper-agent-<SWIMLANE_ID>@<SWIMLANE_PROJECT_ID>.iam.gserviceaccount.com` (the runbook's step 2) |
| `agent:spool.cmekKey` | the Cloud KMS key for the spool's disk, in the swimlane's project |
| `agent:network.hubAddresses` | the agent load balancer's address, as `/32` (13.1) |
| `agent:network.kubeApiAddresses` | the cluster's control-plane address (below) |
| `agent:hubCa.pem` | the text of `agent-ca.pem` from step 10. It is a certificate, never a key; the chart refuses a key |
| `agent:imagePullSecrets` | `<PULL_SECRET>`, only if the packages are private |

Source: `deploy/ADDING-A-SWIMLANE.md`, "What to collect first" (the command there has no `--location` or `--project`; they are added here and are **Not verified**).

```bash
gcloud container clusters describe <SWIMLANE_CLUSTER> --location <SWIMLANE_CLUSTER_LOCATION> --project <SWIMLANE_PROJECT_ID> --format='value(endpoint)'
```

15.3 The agent's namespace and the config-server's. The agent chart's Application runs the agent in `<AGENT_NAMESPACE>` (`lanekeeper`). The agent chart needs `agent:configServer.namespace`: the
**one** namespace the swimlane's config-server runs in, which `<CONFIG_SERVER_NAMESPACE>` stands for. **It must differ from the agent's own**, and it can never be `kube-system`, `kube-public`
or `kube-node-lease`. The chart refuses a wrong shape; it cannot know the right namespace. Ask the swimlane's cluster:

Source: `deploy/ADDING-A-SWIMLANE.md`, "What to collect first".

```bash
kubectl --context <SWIMLANE_CONTEXT> get pods -A -l app=csp-configuration-server
```

The pods must be in one namespace. That is `<CONFIG_SERVER_NAMESPACE>`.

15.4 The swimlane's own Argo CD applies `deploy/argocd/swimlanes/<SWIMLANE_ID>.yaml` with the swimlane's revision. Then the agent starts, joins, and keeps its certificate in
`Secret/lanekeeper-agent-cert`, which Argo CD does not overwrite.

Source: `deploy/ADDING-A-SWIMLANE.md`, step 4.

```bash
kubectl --context <SWIMLANE_CONTEXT> apply -n argocd -f deploy/argocd/swimlanes/<SWIMLANE_ID>.yaml
```

15.5 The pull secret, if the packages are private, in the swimlane's cluster (6.7). Put its name in `agent:imagePullSecrets`.

15.6 **The sentinel on the NFS VM** (`deploy/sentinel/README.md`). **The binary is a stub today** (`plans/09-deployment-ci.md`, A18): installing the package proves that the unit loads and runs under its
restrictions, and records nothing yet. Its configuration is not defined either (prompt 17). So do these now, and the rest when the daemon exists:

1. Put the sentinel in the seed file (15.1) with its `service_account`, and let the NFS VM's outbound IP into `hub:network.agentSourceRanges`: the sentinel reaches the same load balancer on 443.
2. The package is signed by a release key that **does not exist yet** (`plans/09-deployment-ci.blockers.md`, B13): `deploy/sentinel/RPM-GPG-KEY-lanekeeper.pub` is a marker. Someone with project
   rights makes the key and sets `LK_RPM_WIF_PROVIDER`, `LK_RPM_SERVICE_ACCOUNT` and `LK_RPM_SIGNING_SECRET` (step 6.3), makes the protected GitHub environment `release`, and tags `sentinel-v<version>`.
   The commands are in `deploy/sentinel/README.md`, "B13".
3. Configuration management installs the RPM, enables and starts it. The package does neither. Check the signature on the VM with `rpm -K`, and set `localpkg_gpgcheck=1`,
   because dnf does not check a local file's signature otherwise (`deploy/sentinel/README.md`, "Signing").
4. Render the audit rule template `lanekeeper.rules.in` once for each `<EXPORT_ROOT>` into `/etc/audit/rules.d/` (`deploy/sentinel/audit/lanekeeper.rules.in`; the file name is yours).

**You should see.**

- `hub seed` reported the swimlane as created, and a second run changes nothing.
- The agent's log has `joined the hub`, and the agent's `/readyz` says `ready` once it is connected.
- `Secret/lanekeeper-agent-cert` has data.

**If not.** An agent that cannot join: its swimlane row has no `agent_service_account`, the account differs from the token's `email`, or the token's audience is not `https://<HUB_HOST>`. An agent whose
connection never completes: its outbound IP is not in the list (the load balancer drops it). A network policy that denies the config-server calls: the wrong namespace. A spool volume Pending: the
Compute Engine service agent of the swimlane's project has no use of the disk key (the runbook's step 2).

## 16. End-to-end verification

**Why.** Each line is a check with a command and the result that means it worked. The first real measurement of three ready replicas and of the startup time is here, because CI never could make one.

**Do.** Run each check in the table, in order. Rows 7 and 9 need time: come back to them later.

| # | Check | Command | Expected |
|---|---|---|---|
| 1 | the three Applications are synced | `kubectl -n argocd get application lanekeeper-gcp lanekeeper-db lanekeeper-hub` | each `Synced` and `Healthy` (the jsonpath of Q10 reads the sync status) |
| 2 | the hub runs on three replicas | `kubectl -n lanekeeper get statefulset lanekeeper-hub` | `3/3` ready |
| 3 | each replica is ready | the port-forward of 12.7, for `lanekeeper-hub-0`, `-1`, `-2` | `200` and `ready` from each |
| 4 | the database has three instances | `kubectl -n lanekeeper get cluster.postgresql.cnpg.io/lanekeeper-db -o wide` | three instances, ready |
| 5 | the schema is complete | `kubectl -n lanekeeper logs job/lanekeeper-hub-migrate` | `"applied":8,"version":8` on the first run |
| 6 | sign-in works | step 14 | you reach the app as an administrator |
| 7 | the audit chain writes its checkpoint within an hour (S9) | `gcloud storage ls gs://<AUDIT_BUCKET>/audit/checkpoints/` | at least one object `.../<number>.json`, one more each hour (**Not verified**: the `gcloud` command; the object name is `OBJECT_PREFIX` in `crates/hub-identity/src/audit/bucket.rs`) |
| 8 | an agent joined | `kubectl --context <SWIMLANE_CONTEXT> -n lanekeeper logs deployment/lanekeeper-agent` | a line `joined the hub` |
| 9 | its certificate renews | the same log, about 12 hours later | `renewed the certificate over the stream`. The hub's CA profile gives a certificate 24 hours (`crates/hub-platform/src/ca/profile.rs`) and the agent renews at half the lifetime (`crates/agent/src/identity/joiner.rs`) |
| 10 | a drift view loads | in the app, open the swimlane's drift view; the API behind it is `GET /api/v1/swimlanes/{id}/drift` | the page shows the swimlane's files. **Not verified**: the app's names for the screens |
| 11 | a write previews | in the app, preview a change to a file; the API is `POST /api/v1/swimlanes/{id}/files/preview` | an answer that says what the change would do, and nothing is written. Editing stays locked until an administrator confirms the baseline (the runbook, step 6) |
| 12 | the webhook is accepted | a push to a mirrored repository | GitHub shows `202` for the delivery |
| 13 | Cloud Armor denies | the `curl` of 13.3 from an address in neither list | `403` |
| 14 | the first measurement of start to ready (P11) | `kubectl -n lanekeeper get pod lanekeeper-hub-0 -o jsonpath='{.status.startTime}{"\n"}{.status.conditions[?(@.type=="Ready")].lastTransitionTime}{"\n"}'` | the time between the two. **This is not P11.** The pod's `startTime` is before the image is pulled, and `Ready` comes only after the first fetch of both Git repositories (the `mirrors` check gates `/readyz`), and P11 budgets 10 seconds for the hub's own start without either (`crates/hub/README.md`, item 11). The number is an upper bound on a first start, and says nothing about whether P11 is met; record it as that. The hub's own start is in its log, from the line `starting` to the line `listening` (**Not verified**: that the interval is what P11 measures) |

**You should see.** Every row as expected.

**If not.** Fix the earlier step that the row belongs to. A row that cannot pass because of something in step 18 is not a failure of your install.

## 17. Running it

**Why.** These are the changes you will make after the install, and what a failure of each looks like.

**Do.**

17.1 **Rotate the web certificate.** Write a new version of `lanekeeper-web-cert`, then of `lanekeeper-web-key`, as in 8.1. The hub rereads them every `LK_WEB_CERT_RELOAD_SECS` (300 s, jittered). A read can find the new certificate with
the old key. It is refused and the next read succeeds. Connections already open keep their certificate. The Gateway's certificate is separate (`crates/hub/HARDENING.md`, "The web certificate"): make a new Compute Engine certificate with a new name and change
`gateway.tls.preSharedCerts` to it (**Not verified**: from memory, such a certificate cannot be edited in place). **When it expires, `/readyz` fails on every replica at once and the load balancer has no healthy backend.** Alert on
`lanekeeper_web_cert_not_after_seconds` within a week of now, and on `lanekeeper_web_cert_reload_failures_total` rising (`crates/hub/README.md`, item 12).

17.2 **Deploy a new image.** A merge to `main` publishes new digests (step 6.5). Put the hub digest in `deploy/values/hub/<ENV>.yaml` (and the agent's in the agent values), merge, then change `targetRevision` in a second pull request
and apply the changed Application file (`deploy/argocd/README.md`, "Moving a revision"). The sync runs the migration Job first, and rolls the hub only when it completes. Migrations are additive, so the old
hub keeps working on the new schema while it rolls (`crates/hub/README.md`). A new seed or image is a new seed Job (its name carries a hash); a seed change follows 17.3.

17.3 **Change the seed.** A seed change is a new ConfigMap and a new revision, and nothing old is edited or deleted. The seed Job is the record of which file was applied, and the file is in the ConfigMap that the Job names. The chart keeps a ConfigMap
only when it made it (`seed.content`); the one in staging and prod is yours (`seed.existingConfigMap`), so it is the record only while you keep every version.

1. Choose the next revision: one more than the last you used (`<SEED_REVISION>`). Make a new ConfigMap from the new file, named with it. Use `create` and not `apply` or `replace`, so that an old ConfigMap cannot be overwritten (`create` fails with `AlreadyExists`, which is what you want).

```bash
kubectl -n lanekeeper create configmap lanekeeper-hub-seed-<ENV>-<SEED_REVISION> --from-file=seed.yaml=<SEED_FILE>
```

2. In `deploy/values/hub/<ENV>.yaml` set `seed.existingConfigMap` to that name and `seed.revision` to `<SEED_REVISION>`, a whole number. The chart refuses a name that does not end with `-` and the revision, and a revision that is not a positive whole number. Merge, then move the revision as in 17.2.
3. A new seed Job runs, and the old one stays, with the old ConfigMap that it points at: the Job and the ConfigMap that the chart makes carry `Prune=false`, so old seed Jobs accumulate by design. Check that the Job ran (`kubectl -n lanekeeper get jobs`: a new `lanekeeper-hub-seed-` Job that has completed). An Application that shows `Synced` does not show that the seed ran (see the next paragraph).

**Never use a revision twice, a revert included; always move forward.** The Job's name is a hash of the revision and the old Job is kept, so a revision that you use again names a Job that has already run. Argo CD finds it complete and the Application `Synced`, and `hub seed` does not run:
the hub keeps the rows of the later file while Git shows the earlier one, and those rows decide who may join as a swimlane's agent. To revert a seed change, make a new ConfigMap from the earlier file with the next revision number. The chart cannot see the revisions that came before, so it cannot refuse one you used; the review of the pull request that changes `seed.revision` is the check.

Delete a Job you no longer need with `kubectl -n lanekeeper delete job <name>`, and only afterwards the ConfigMap that it named: the record is the pair. **Not verified:** how Argo CD treats the annotation; it may show the Application as `OutOfSync` while an old Job exists. Run the dry run first if you can (step 18).

17.4 **Roll a Cloud KMS key version.** The hub pins a key **version**, so a roll is a deliberate change, and the gcp chart sets no rotation period on the signing keys.

- A CA key: make a new version, run `hub ca-init` for it (step 10), store the new certificate in the CA's secret, put the new version in `config.kms.agentCa` or `config.kms.replicaCa`, and move the revision. The README: "renewing is
  a new key version, a new certificate and a rolling restart". Renew before the certificate's `--valid-days`.
- The audit checkpoint key: the older versions must still verify old checkpoints (`LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS`), and **the chart cannot set that variable** (step 18). Do not roll this key before that is fixed.
- The vault key: `gcp:kms.vaultRotationPeriod` gives it a new primary version every 90 days. The hub names no version, so nothing changes in the values.

17.5 **A failed migration Job.** The sync stops at PreSync and the hub does not roll. The Application shows a failed operation; the Job is kept so that you can read it. (**Not verified**: all of this is how Argo CD is expected to treat a failed PreSync hook; the first failed sync is the test.)

```bash
kubectl -n lanekeeper get jobs
kubectl -n lanekeeper logs job/lanekeeper-hub-migrate
```

The log names a version number or a five-character SQLSTATE and never a host or a password. A migration that fails is rolled back whole. Common causes (`crates/hub/README.md`, "What it refuses"): the URL's role is not
`lanekeeper_migrator`; the database holds a migration this image does not know (run the Job of the newer image); a merged migration was edited; the run outlasted `migrate.timeoutSeconds` (600). After you fix it, sync again from Argo CD.

17.6 **Keep GitHub's ranges current.** When the nightly job `hook-ranges` fails, run the two commands of step 5 and move the revision of `gcp.yaml`.

17.7 **Remove the bootstrap administrators** (step 14.4) and keep every seed file you apply, with its ConfigMap (17.3): the audit event records the file's hash and not the person, and the file is what the hash is of.

**You should see.** After each change, the Applications `Synced`, the hub `3/3`, and `/readyz` `200` on each replica.

**If not.** `kubectl -n lanekeeper describe` the object, and read the log. A change that you cannot undo from Git: the audit bucket's lock, a deleted key ring, a deleted key. The charts mark the buckets, keys, secrets and the database so
that removing them from Git deletes nothing (`deploy/argocd/README.md`, "Prune").

## 18. Known gaps and things not verified

**Why.** Everything below is open. Some is in the charts, some in the guide, and some is a fact nobody could check. Nothing here has been run against a real cluster.

**Do.** Read it. Decide which of these matter to you before you go to production.

**What has never run.**

- **The production hub image has never been Ready.** It cannot be without real Google services and Entra, so kind cannot show it (`plans/09-deployment-ci.chart-alignment.md`, section 10). `/readyz` on three replicas and the
  startup time P11 were never measured in CI. The first real measurement is step 12.7 and step 16 of this guide. The kind job proves what it can: the Jobs, the database and S6, and that each replica stops at its first Google call.
- The `hub` stage of the kind job ran once in GitHub CI as far as is known here (the job "kind, the hub chart with CloudNativePG", run 40665155670 on `62d23dc`, read from the GitHub page, not by a command): it passed with the migration and seed Jobs
  completed and all three replicas stopped at their first Google step (exit 1), and `/readyz` on three replicas and P11 reported **NOT MEASURED**. Nothing here ran it again; the run for a later commit is the one to look at.
- Nothing has run on a real GKE cluster. Every Google-specific shape is taken from vendored schemas and from memory, because Google's documentation was refused (`plans/09-deployment-ci.blockers.md`, B6). The charts' READMEs list
  them: the Gateway's `networking.gke.io/pre-shared-certs` option and `NamedAddress`; the `cloud.google.com/l4-rbs` annotation; that `externalTrafficPolicy: Local` keeps clients' addresses; that Google's proxies and health checks come from
  `35.191.0.0/16` and `130.211.0.0/22`; that FQDN network policy and enforcement are enabled; the schemas of `GCPBackendPolicy`, `HealthCheckPolicy` and `FQDNNetworkPolicy`, which come from a community catalog.
- Argo CD's handling of the migration Job as a PreSync hook, of the hook's account and policy, and of the seed Job's `Prune=false` was written from its source and not read from its documentation (**Not verified**). The first sync is the test.
- Config Connector: that it makes a first version of each key, that it is version 1, that it accepts HSM keys in your location, that a regional key works for a one-replica secret, and what it does when it finds a ring or a key already there
  (`deploy/charts/gcp/README.md`, "Not verified").
- The seed with only a project, `hub seed --dry-run` in a cluster, a restore from the database backup, and the sentinel's daemon.

**Load balancer.**

- **Edit bodies of 12 MiB** (D137): two routes take 12 MiB plus 64 KiB, and nothing sets or tests the limit of the Google load balancer. Unproven.
- **MCP session affinity**: a session lives in one replica, and the hub needs the load balancer to keep a client on one replica by the `Mcp-Session-Id` header. `GCPBackendPolicy` cannot express that (it has no field for the header name). Without
  it, two requests in three land on the wrong replica and are answered 404, and the client starts again.
- **Drain against the health check**: on SIGTERM the hub fails `/readyz` and waits `LK_SHUTDOWN_GRACE_SECS` (5 s), but the health check needs 3 failures 5 seconds apart. For about 10 seconds the load balancer may still send new connections to a replica that has
  stopped accepting. The effect may be smaller if GKE removes a terminating pod's endpoint first. Not measured (`plans/09-deployment-ci.chart-alignment.md`, gap T2).
- **The agent entry point**: the agent dials 443 and the Service forwards to the hub's 8444 (B7, decided on 2026-10-26). No cluster has run the two charts together, and no check compares the agent's port with the hub's Service.
- **Event streams** and headers: that the load balancer does not buffer or cut `text/event-stream` before the hub's 15-second keep-alive, and passes `Cookie`, `Origin` and `X-CSRF-Token`, is assumed, not tested. `LK_TRUSTED_PROXY_HOPS` is 2 and must be rechecked on the real path.

**The hub chart.**

- The headless Service does not set `publishNotReadyAddresses`, so a replica that has failed `/readyz` disappears from DNS while it may still hold agent connections that another replica forwards commands to (`plans/09-deployment-ci.chart-alignment.md`, gap P3).
- **GitHub Enterprise** and **sovereign Entra clouds** cannot be used: `LK_GITHUB_API_BASE` and `LK_ENTRA_AUTHORITY_HOST` are not settable, because the egress policy would need to name the new host (`plans/09-deployment-ci.chart-alignment.md`, gap N1).
- **Optional settings the chart does not expose**, so the hub's defaults apply: `LK_MCP_NOTIFY_SERVICES` (off; it reaches the config-server's unauthenticated `/update-resources`, Q37), `LK_RETENTION_ENABLED` and `LK_RETENTION_DRY_RUN`
  (off), `LK_EXTRA_DENY_GLOBS`, `LK_TEAMS_ALLOWED_HOSTS`, `LK_GITHUB_API_BASE`, `LK_ENTRA_AUTHORITY_HOST`, `LK_ENTRA_REDIRECT_PATH`, `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS`, `LK_MCP_AS_MIRROR`, `LK_GIT_READ_REF`, `LK_GIT_WEBHOOK_REF`.
  A checkpoint key roll needs `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS` first.
- **The agent chart does not set `LK_CONFIG_SERVER_DEPLOYMENT`**, so the agent does not report when the config-server's pods started and check C11 is not reported. Adding it is a contract decision (the agent would watch the config-server's namespace).
- **The record of the seed holds only while a person keeps the ConfigMaps.** The chart keeps the seed ConfigMap only when it made it (`seed.content`); in staging and prod the file is in a ConfigMap you made (`seed.existingConfigMap`), the Job names it, and nothing alerts if it is edited or
  deleted. A revision used twice (a revert) runs nothing, and the chart cannot see that (17.3). Both are a person's discipline and a reviewer's check.
- The migration and seed Jobs' resources are a guess (`jobs.resources`). There is no dry-run Job for the seed. The web certificate's issuer, names and renewal are not defined by the repository, and nothing rotates it.
- **`/healthz` answers only when the whole start has finished** (`crates/hub/src/process.rs`: the operations socket is bound first and served last), and the start is six steps in a row that are each bounded by `LK_START_TIMEOUT_SECS`
  (8). The startup probe gives the pod 60 s, so the chart refuses a `LK_START_TIMEOUT_SECS` above 8; a larger one needs a wider probe window, which is a change of the contract in `crates/hub/HARDENING.md`, a human's decision.
  The caches are held to the memory limit in the same way (`deploy/charts/hub/README.md`). **Not verified:** that the P11 memory figure was measured with the default caches.
- **`hub ca-init` and the identity** (step 10): the README says a person runs it; the hub can authenticate only through a metadata server, so it cannot run on a laptop. This guide uses a pod with a temporary service account. The repository does not document that route.
  Whether `roles/cloudkms.signerVerifier` covers reading the public key is not verified.

**Supply chain.**

- **Binary Authorization is in dry run** until Q24 (`docs/open-questions.md`) is answered. Whether a classic attestor accepts a cosign signature is unconfirmed (B12). The policy is per project: a swimlane cluster in another project has none, and nothing in the repository makes it.
  Before it is enforced, the third-party images (the operator, Argo CD, Config Connector) must be attested or listed.
- The CI trust root (the signing key, the Workload Identity Federation pool and provider, the service account and the IAM) is not made by any chart (B12). Step 6 gives commands from memory. The RPM release key does not exist (B13).
- **Branch protection**: `ci.yml` no longer holds the jobs `rust`, `web` and `pinned-actions`; they moved to path-filtered workflows. A person must change the required status checks after the merge. No setting was changed from the repository.
- Images are amd64 only. A pinned base image gets no security fixes until its digest moves (Renovate proposes it, a person reviews).

**What the repository does not say.**

- Which roles Config Connector's Google service account needs (step 3).
- How to install Argo CD, and how to give it access to a private repository.
- The repository named in `repoURL` (also in `deploy/policy/argocd.rego` and in the image names) is fixed by the policy. Installing from another one needs changes there and in the planted cases of `deploy/tests/render.sh`.
- Staging has no Application files: copy a platform file and change its `lanekeeper.io/environment` label and `valueFiles` (the policy accepts that pair).
- Pod Security labels on the namespaces. The `restricted` profile may refuse the agent's NFS volume.
- The database backup uses CloudNativePG's built-in barman support, which is deprecated (`plans/09-deployment-ci.blockers.md`, B10). The database's pods have no egress policy, the sizing is a placeholder, and no restore has been exercised.
- The first administrators' object ids are in Git history once they are in the values file.
- A pull request that deletes a file this guide cites is caught on the next push to `main`, not always in the pull request (`.github/scripts/install-guide.test.mjs`, header).

**You should see.** You have read this list, and you know which of these items your installation depends on.

**If not.** Ask the owner of the part before you go to production: the hub (`crates/hub`), the charts (`deploy`), or the platform team for the cluster.
