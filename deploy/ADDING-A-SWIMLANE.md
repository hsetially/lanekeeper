# Adding a swimlane

A swimlane is exactly one GKE cluster, with one NFS VM, one config-server, one Argo CD and one Lanekeeper agent (`docs/domain-model.md`).
Adding one means the hub learns the swimlane, trusts its agent, lets its network in, and a baseline is adopted. Six steps, in order.
Most of them are a pull request that `just deploy-check` (the `deploy` workflow) checks; two are done in the cluster or the hub.

**Read this first.** The hub does not serve yet (plan 09 A1: `crates/hub/src/main.rs` exits until the binary is composed), so the steps
that use the hub (1, the registration in 2, and 6) describe the contract in `api/openapi.yaml` and `db/migrations/0001_init.sql`, not a
running system, and none of them has been run. Step 1 has a gap that is not this runbook's to close (below).

| Step | Who | Changes | Checked by |
|---|---|---|---|
| 1. Create the swimlane | platform team, then an Admin | the cluster, its NFS export, its Argo CD; the swimlane in the hub | nothing in this repository |
| 2. Workload Identity, or a join token | platform team, then an Admin | a Google service account and its binding; the agent's registration in the hub | nothing in this repository |
| 3. The values file | a developer, in a pull request | `deploy/values/agent/<swimlane>.yaml` | `deploy/tests/render.sh`: lint, kubeconform, the policies, the contract with `crates/agent/RBAC.md` and `HARDENING.md` |
| 4. The Argo CD Application | a developer, in a pull request; then the cluster's Argo CD admin | `deploy/argocd/swimlanes/<swimlane>.yaml` | `argocd.rego`, and the project against the chart's output |
| 5. The allowlist | a developer, in a pull request | `deploy/values/hub/{prod,staging}.yaml`, `network.agentSourceRanges` | the hub chart refuses an empty or world-wide list; `network.rego` |
| 6. The adoption pass | an Admin | the swimlane's baseline, in the hub | the hub |

The order matters. The hub must let the cluster in (5) before the agent starts (4), and the agent's identity must be registered (2) before it
joins, or it is refused.

## What to collect first

| Fact | Where it goes | Where it comes from |
|---|---|---|
| the swimlane's id: lower-case letters, digits and `-`, 63 characters at most (the `swimlanes.id` check) | file names, labels, `swimlane.id` | the platform team's name for the cluster (`sit1`, `sitb`) |
| the Google project and the cluster's name | `swimlane.project`, `swimlane.cluster` | the platform team |
| the NFS server address, export path, mount path, the uid and gid that own the files, the NFS version | `nfs.*` | Q11 (`docs/open-questions.md`); the uid and gid default to 1010 (D82), and `root_squash` and `anonuid` in `/etc/exports` decide which uid the agent must run as |
| the namespaces the config-server's files belong to | `namespaces` | the cluster; one Role and one RoleBinding are made in each (`crates/agent/RBAC.md`). `kube-system` cannot be watched |
| the cluster's outbound IP | step 5 | Q1: run the command in `docs/open-questions.md` from a pod |
| the Cloud KMS key for the spool's disk, and the number of the cluster's project | `spool.cmekKey`, and the grant in step 2 | Q29: the platform team's key ring; `gcloud projects describe <project> --format='value(projectNumber)'` |
| the cluster's control-plane address | `network.kubeApiAddresses` | `gcloud container clusters describe <cluster> --format='value(endpoint)'` |
| the address of the hub's agent load balancer | `network.hubAddresses`, `hub.endpoint` | the hub's `Service/lanekeeper-hub-agent`, and the DNS name that points at it; the host of `hub.endpoint` must be one of the hub's `config.agentEndpointHosts` (`LK_AGENT_ENDPOINT_HOSTS`: the names on the certificate the agent verifies) |
| the hub's own name, for the token's audience | `hub.audience` | `https://` and the hub's `gateway.hostname` (`LK_HUB_AUDIENCE`): the hub refuses every join whose token is for another audience, and an empty `hub.audience` is the endpoint, which is not that name. `deploy/tests/render.sh contract` checks both |
| the NFS VM's outbound IP | step 5 | the platform team (plan 09 A28: the sentinel reaches the agent port too) |
| the published image digest | `image.digest` | the job summary of the `publish` job of `images.yml` (plan 09 A29) |
| the hub's agent CA certificate, public | `hubCa.pem` | step 3 |

Use documentation addresses (RFC 5737) in anything you commit for a test; **a real address or cluster name goes into the values only when
the question that decides it is answered** (Q1, Q2, Q11), and no real tenant data ever goes into the repository (rule 12).

## 1. Create the swimlane

**The infrastructure** is the platform team's: the GKE cluster (with Workload Identity if it can, Q26), its NFS VM and export, the
config-server, and the cluster's own Argo CD in the namespace `argocd`. The agent needs a CNI that enforces NetworkPolicy, because the
chart's egress limits (`crates/agent/HARDENING.md`) are NetworkPolicy.

**In the hub** a swimlane is a row of `swimlanes`: its id, project, display name, cluster, tier (the environment tier for check C6), and
the NFS server, export and mount root. Nothing in this repository can create that row, and **nothing in the hub's code does either yet**:
the API registers an agent's service account for a swimlane (`PUT /api/v1/admin/agents/{swimlane}`, `adminPutAgent`) and edits a
swimlane's tenant set and auto-notify, and the hub reads `swimlanes.agent_service_account` when an agent joins, but no operation
creates the swimlane itself, and `INSERT INTO swimlanes` appears only in tests (plan 09 blockers, B11). Until a person decides how the
swimlane registry is filled (an endpoint, a seed, a migration), this step cannot be completed by following this runbook.

## 2. Workload Identity, or a join token

The agent proves who it is when it joins (S5). Workload Identity is the normal way; the join token is the fallback for a cluster without it.

**Workload Identity (Q26).** The agent runs as the Kubernetes service account `lanekeeper-agent` in the namespace `lanekeeper` (the
chart makes it). It acts as a Google service account that exists only for it and has no roles at all:

```
gcloud iam service-accounts create lanekeeper-agent-<swimlane> --project <project> --display-name "Lanekeeper agent <swimlane>"
gcloud iam service-accounts add-iam-policy-binding lanekeeper-agent-<swimlane>@<project>.iam.gserviceaccount.com \
  --project <project> --role roles/iam.workloadIdentityUser \
  --member "serviceAccount:<project>.svc.id.goog[lanekeeper/lanekeeper-agent]"
```

`<project>` is the project of the **cluster**: its Workload Identity pool is `<project>.svc.id.goog`. Check the cluster has one:
`gcloud container clusters describe <cluster> --format='value(workloadIdentityConfig.workloadPool)'`. An Admin then registers the account
in the hub: `PUT /api/v1/admin/agents/<swimlane>` with `{"service_account_email": "lanekeeper-agent-<swimlane>@<project>.iam.gserviceaccount.com"}`.
The hub checks the `email` of the agent's Google-signed ID token (audience: the hub) against that account.

**The join token (the fallback).** Set `joinToken.enabled: true` in the values (step 3), and leave `serviceAccount.gcpServiceAccount`
empty if the cluster has no Workload Identity. An Admin issues a one-time token: `POST /api/v1/admin/agents/<swimlane>/join-token`. It is
shown once, stored hashed, valid for 24 hours, and bound to the one swimlane. **It is never put in Git, in the values, or in a command
line.** Put it into the Secret that the chart's Role names, in the agent's namespace, from standard input:

```
kubectl create namespace lanekeeper   # unless Argo CD has made it already
read -rs TOKEN && printf '%s' "$TOKEN" | kubectl -n lanekeeper create secret generic lanekeeper-agent-join-token --from-file=token=/dev/stdin; unset TOKEN
# paste the token and press Enter: it is not echoed, not kept in the shell's history, and not in the process list
```

Do this within the 24 hours and before the agent first starts. The Secret is not rendered by the chart and Argo CD does not manage it.

**The key for the spool's disk (S10).** The agent keeps a volume of 1 GiB for its spool (D74), encrypted with a customer-managed key. The
Compute Engine service agent of the **cluster's project** must be allowed to use that key, or the claim stays `Pending`:

```
gcloud kms keys add-iam-policy-binding <key> --keyring <ring> --location <location> --project <key project> \
  --member "serviceAccount:service-<project number of the cluster's project>@compute-system.iam.gserviceaccount.com" \
  --role roles/cloudkms.cryptoKeyEncrypterDecrypter
```

The gcp chart makes this grant for the platform's own project (`cmek.diskKeys`) and for no other; a swimlane in another project gets it from
this command. The member is the one the gcp chart uses; Google's documentation could not be read to confirm the command (plan 09 blockers,
B6). Without a key (Q29) the chart falls back to the cluster's default class and the policy warns.

## 3. The values file

Copy `deploy/values/agent/sit1.yaml` to `deploy/values/agent/<swimlane>.yaml` and set, in this order:

| Value | Notes |
|---|---|
| `image.digest` | the published digest; the placeholder (64 zeros) cannot be pulled, and the policy warns about it |
| `swimlane.id`, `.cluster`, `.project` | |
| `hub.endpoint` | `https://` and a host name only; the agent dials it |
| `nfs.server`, `nfs.exportPath` (and `nfs.uid`, `nfs.gid` if they are not 1010) | Q11. The agent mounts the export read-write, as the uid and gid that own its files; the config-server mounts the same export read-only (D82) |
| `namespaces` | the namespaces to watch |
| `serviceAccount.gcpServiceAccount` | step 2; empty only with `joinToken.enabled` |
| `network.hubAddresses` | the hub's agent load balancer, as `/32` or a small block; the agent can reach nothing else of the hub's |
| `network.kubeApiAddresses` | the cluster's control plane |
| `spool.cmekKey` | the Cloud KMS key of step 2, `projects/P/locations/L/keyRings/R/cryptoKeys/K`. The size (1 GiB) and the bound (512 MiB) are the defaults; the policy keeps the size larger than the bound |
| `imagePullSecrets` | the name of the pull secret for GHCR, made out of band in the cluster (Q8) |
| `hubCa.pem` | the **public** certificate of the hub's agent CA, below |

**The hub's CA certificate** is the output of the one-off ceremony `hub ca-init` (plan 03b A8: the CA key is made inside Cloud KMS and
never leaves it; the certificate is public configuration, kept in Secret Manager or a ConfigMap). The subcommand is not wired into the hub
binary yet (plan 09 A1), so ask whoever holds the ceremony's output. It is a certificate, never a key: the chart refuses a PEM that holds
a private key, and refuses to render without one. The committed `sit1.yaml` has none, on purpose.

Then run `just deploy-check`. `render.sh` checks every file under `deploy/values/agent` with a throwaway CA that stands in for the real
one, so the check passes before the real certificate is in.

## 4. The Argo CD Application

Copy `deploy/argocd/swimlanes/sit1.yaml` to `deploy/argocd/swimlanes/<swimlane>.yaml` (the file is named after the swimlane, and the policy
refuses one that is not) and change: the label `lanekeeper.io/swimlane` in both documents, the project's destinations (the agent's namespace
and **exactly** the namespaces of the values file; the policy compares them), and `valueFiles`. Leave `targetRevision` as the placeholder
until the values are merged: a commit cannot name the commit that contains it.

After the pull request merges, **move the revision** to the commit that holds the values (`deploy/argocd/README.md`, "Moving a revision"),
and have the cluster's Argo CD admin apply the file to the **swimlane's cluster**, not the platform's:

```
kubectl --context <swimlane-cluster> apply -n argocd -f deploy/argocd/swimlanes/<swimlane>.yaml
```

The Application ignores the data of `Secret/lanekeeper-agent-cert` (plan 09 A14), because the agent writes its certificate into it.

**Before the restart feature is used on this swimlane (Q10):** the agent restarts a Deployment by patching its restart annotation, and a
Argo CD that manages that Deployment may revert the annotation at its next sync. Run the test in `docs/open-questions.md` (Q10) once on the
swimlane's config-server Application; if it fails, that Application (the cluster's own, not ours) needs `ignoreDifferences` on
`/spec/template/metadata/annotations/kubectl.kubernetes.io~1restartedAt` with `RespectIgnoreDifferences=true`.

## 5. Add the cluster's outbound IP to the allowlist

The hub's agent entry point is a TCP pass-through load balancer that accepts only the clusters' outbound IPs (D25, S18). In
`deploy/values/hub/prod.yaml` (and `staging.yaml`, if the swimlane reports to staging), add the cluster's outbound IP from Q1 to
`network.agentSourceRanges`, as a `/32`:

```yaml
network:
  agentSourceRanges:
    - 198.51.100.0/24   # existing (an example address)
    - 203.0.113.7/32    # <swimlane>'s outbound IP (an example address)
```

**Also add the NFS VM's outbound IP** (plan 09 A28): the sentinel that runs on the NFS VM reaches the agent load balancer on 443 from there, through
the same list. The one list feeds both the load balancer's `loadBalancerSourceRanges` and the hub's NetworkPolicy, so the two cannot differ.
The chart refuses an empty list and the policy a range wider than a `/16`. The sentinel itself is a signed RPM that configuration management installs on the NFS VM (`deploy/sentinel/README.md`: how to verify it, and what the package does and does not do); this runbook only lets its address in.

Then move the hub's revision and let the platform Argo CD sync it (`deploy/argocd/README.md`). **Do this before the agent starts**: an agent
whose address is not in the list is dropped at the load balancer and shows only as a connection that never completes.
(The load balancer listens on 443, the port the agent dials (`crates/agent/HARDENING.md`), and forwards to the hub pods' port 8444
(`crates/hub/HARDENING.md`); the agent's `hub.endpoint` needs no port. `plans/09-deployment-ci.blockers.md`, B7, decided by a human on 2026-10-26.)

## 6. Run the adoption pass

Editing on a swimlane stays locked until an Admin confirms its baseline (D2). When the agent has joined and its first scan has reached the
hub, an Admin opens the swimlane in the app and runs the adoption wizard (D22): the hub proposes which branch is deployed (by similarity;
`templates/*` branches are never proposed) and a baseline, and the Admin confirms them. One audit event is written for each confirmation
(D93). Check the swimlane's tenant set while you are there (`adminGetTenants`). Auto-notify is off by default (D85); leave it off until
the swimlane has been read for a while.

## Admission: the images in the swimlane's cluster

Binary Authorization is per Google project (`deploy/charts/gcp/README.md`, "Admission"). The platform project's policy is made by the gcp
chart; **a swimlane cluster in another project has no policy until one is made there that requires the same attestor, and nothing in this
repository makes it** (Q24 asks whether Binary Authorization is available on the swimlane clusters at all). The agent's image is signed
by CI either way.
