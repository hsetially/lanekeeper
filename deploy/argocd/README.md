# Argo CD (09/T6)

The Applications and projects that deploy Lanekeeper (D23, D26). **The platform cluster's Argo CD** deploys the hub, the database and the
Google Cloud resources; **each swimlane cluster's own Argo CD** deploys that swimlane's agent from the one shared chart, with one values
file per swimlane. Nothing here is applied by CI: these files are read by `deploy/tests/render.sh` and by people, and a person with
access to a cluster's Argo CD applies them (no automation holds a cluster credential, plan 09 A29).

| File | Applied to | Deploys | Project | Prune |
|---|---|---|---|---|
| `platform/projects.yaml` | the platform cluster | `lanekeeper-workloads` (hub, db) and `lanekeeper-cloud` (gcp) | | |
| `platform/hub.yaml` | the platform cluster | `deploy/charts/hub` with `values/hub/prod.yaml` into `lanekeeper` | workloads | yes |
| `platform/db.yaml` | the platform cluster | `deploy/charts/db` with `values/db/prod.yaml` into `lanekeeper` | workloads | **no** |
| `platform/gcp.yaml` | the platform cluster | `deploy/charts/gcp` with `values/hub/github-hook-ranges.yaml` and `values/gcp/prod.yaml` into `lanekeeper-gcp` | cloud | **no** |
| `swimlanes/<swimlane>.yaml` | that swimlane's cluster | `deploy/charts/agent` with `values/agent/<swimlane>.yaml` into `lanekeeper`, and the Roles into the namespaces the agent watches | `lanekeeper-agent` (in the same file) | yes |

There is no staging file: the plan lists one file per component, and which cluster staging runs on is not known. A staging platform
cluster takes the same three files with `lanekeeper.io/environment: staging` and `staging.yaml` in `valueFiles` (the policy accepts that
pair, and refuses `prod` values under a `staging` label).

## What every Application is, and why

`deploy/policy/argocd.rego` refuses an Application that is anything else.

- **One chart of this repository, at one commit.** `targetRevision` is a full commit id: a branch moves under a running system, and a
  tag can be moved. `source.path` is `deploy/charts/<chart>`, and `repoURL` is this repository. No Helm repository, no second source.
- **The values file of its environment or swimlane, and nothing inline.** `valueFiles` is exactly the file that
  `deploy/tests/render.sh` renders and checks, and `values`, `valuesObject`, `parameters` and `fileParameters` are refused, as is
  `ignoreMissingValueFiles`. What the policies checked is what runs. `releaseName` is the Application's name, and render.sh renders
  under that name, so even the labels are the checked ones (Argo CD warns that a release name that differs from the Application name
  can break selectors that use `app.kubernetes.io/instance`).
- **Its own project, which can deploy nothing else.** An Application in Argo CD's `default` project can deploy anything from anywhere to
  anywhere. `lanekeeper-workloads` may make the hub's and the database's kinds in `lanekeeper` and a StorageClass; `lanekeeper-cloud` may
  make the Config Connector kinds in `lanekeeper-gcp`; `lanekeeper-agent` may make the agent's kinds in `lanekeeper` and a Role and a
  RoleBinding in each namespace the agent watches. The list of what each may deploy is in `argocd.rego` (a new kind is a decision, so it
  goes there with a test), and `render.sh` renders every chart to show that its project allows everything the chart makes
  (`argocd_scope.rego`): without that, the first sync would stop at the first kind the project does not know. A project allows no
  repository but this one and no destination namespace but its own: not `kube-system`, and not `argocd` (a project that could deploy
  there could rewrite its own restrictions).
- **Automated, self-healing sync.** What is in Git is what runs, and a change made in the cluster by hand is put back. `allowEmpty` is
  refused: a broken template that renders nothing would otherwise prune everything. `Replace` and `Force` (they delete and recreate) and
  skipping the dry run are not sync options here.
- **Prune only where a removal from Git should remove it from the cluster:** the hub and the agent. Never the database (a CloudNativePG
  cluster is not deleted because a file left Git) and never the Google Cloud resources. The buckets, the admission policy, the attestor and
  the note also carry `Prune=false` and `deletion-policy: abandon` themselves (charts/gcp).
- **No `resources-finalizer`.** With it, deleting an Application deletes everything it made. Deleting an Application here leaves what
  runs; removing a workload is a deliberate act in the cluster.
- **`CreateNamespace=true`, and the project allows that Namespace by name.** Argo CD checks the namespace it makes against the
  project's cluster-scoped kinds like any other resource (`gitops-engine/pkg/sync/sync_context.go` at v3.5.4: the namespace task passes
  the permission validator), so each project lists the one Namespace it creates, by name, and nothing else cluster-scoped but the
  StorageClass. The policy checks that pairing.
- **The agent's certificate Secret (plan 09 A14).** The agent writes its certificate into `Secret/lanekeeper-agent-cert`, which the chart
  renders empty. With self-heal on, Argo CD would put the empty Secret back at the next sync. The agent's Application therefore has
  `ignoreDifferences` on exactly that Secret's `/data`, and `RespectIgnoreDifferences=true`, so that the sync does not write the ignored
  field either (without it Argo CD uses `ignoreDifferences` only to decide whether the app is out of sync; the sync still applies the
  whole desired state). The policy requires exactly that entry on the agent and none on any other Application. It does not cover the
  restart annotation of Q10, which is on the Applications that deploy the swimlane's other workloads (the config-server's), not on ours.

## Prerequisites in each cluster

Not installed or checked by anything here.

| Cluster | Needs |
|---|---|
| Platform | Argo CD (the files are applied to its namespace, `argocd`); the CloudNativePG operator (`cnpg-system`, the db chart's network policy admits it); Config Connector, with a `ConfigConnectorContext` for the namespace `lanekeeper-gcp` and a Google service account that may manage the resources of the chart; the GKE Gateway controller and Cloud Armor for the hub's Gateway; Workload Identity; `binaryauthorization.googleapis.com` and `containeranalysis.googleapis.com` enabled in the project |
| Swimlane | Argo CD, its own, in the namespace `argocd`; the NFS export (Q11); Workload Identity, or a join token (Q26); NetworkPolicy enforcement (a CNI that ignores policy ignores the agent's egress limits too) |

## Order on the platform cluster

1. `platform/projects.yaml`.
2. `platform/gcp.yaml`, after GitHub's ranges are resolved (`node deploy/tools/github-hook-ranges.mjs --write`, plan 09 blockers B5) and
   the environment's values have the public key of the CI signing key (charts/gcp README, "Admission"). Until both, the chart refuses to
   render and the Application shows the reason.
3. `platform/db.yaml`: it needs the backup bucket and the Google service account of step 2, and the two Secrets that a person makes once
   per environment (plan 09 A7).
4. `platform/hub.yaml`.

## Moving a revision

The Application files say which commit of this repository the cluster runs. A commit cannot name the commit that contains it, so the
revision in a file is always older than the file, and moving it is its own change. This is the same reason the image digests in the
values files are placeholders (64 zeros) until the first publish (plan 09 A29). Until a real commit is put in, `targetRevision` is 40
zeros: Argo CD cannot resolve it, nothing syncs, and `argocd.rego` says so on every run.

1. Change the charts or the values in a pull request, and merge it.
2. Take the commit that holds the change: `git rev-parse origin/main` after the merge (40 lower-case hexadecimal digits).
3. In a second pull request, put it in `targetRevision` of each Application that should move, and nothing else. CI refuses a branch, a
   tag, a short commit and an upper-case one. A reviewer reads the diff between the old and the new commit, which is the change that is
   about to run.
4. After it merges, whoever runs the cluster's Argo CD applies the changed file (`kubectl apply -n argocd -f deploy/argocd/platform/hub.yaml`).
   The cluster then runs the charts and the values of that commit, and of no other.

To go back, put the old commit back the same way.

## Checked by

`just deploy-check` (and the `deploy` workflow) runs everything; `just deploy-argocd` runs the Argo CD part alone.

- kubeconform, `-strict`, offline, against the vendored Argo CD schemas (v3.5.4): an unknown field is an error.
- `argocd.rego`: the rules above, each with a test that fails without it, and each refused on the real files by a planted change
  (`deploy/tests/render.sh planted`).
- `render.sh`: every file an Application names exists, a swimlane's values and its Application go in pairs, and each project allows what its charts render.

## Not verified here

- **That Argo CD behaves as described on a real cluster.** The behaviours that the policies rely on were read in Argo CD's source and
  documentation at the vendored commit (v3.5.4: the project's rule for kinds, `RespectIgnoreDifferences`, `CreateNamespace` and the
  value file paths). The swimlane clusters run Argo CD versions that nobody here has seen.
- **`ignoreDifferences` for the agent's Secret when the Secret does not exist yet.** `RespectIgnoreDifferences` is "only effective when the
  resource is already created in the cluster; if the Application is being created and no live state exists, the desired state is applied
  as-is". That is the first sync, where the empty Secret is what is wanted.
- **Pod Security admission.** The namespaces are made without Pod Security labels. The agent mounts the NFS export as a volume, and
  the `restricted` profile limits volume types to a list that, as far as the authors remember (not checked here), does not include NFS,
  so a `restricted` label on the agent's namespace needs a decision first.
- **The Applications for staging** (above), **the sentinel's port on the agent load balancer** (plan 09 blockers, B7) **and the config-server's namespace** (B8).
