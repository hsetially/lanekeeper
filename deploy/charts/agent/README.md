# agent chart

The Lanekeeper agent of one swimlane (09/T4; S16, S17, S18): one replica, hardened as `crates/agent/HARDENING.md` says, with the
Roles of `crates/agent/RBAC.md` and nothing wider. Each cluster's own ArgoCD deploys it from this chart, with one values file per
swimlane (D26, `deploy/values/agent/<swimlane>.yaml`).

The Application for a swimlane is `deploy/argocd/swimlanes/<swimlane>.yaml`, and `deploy/ADDING-A-SWIMLANE.md` is the runbook that adds one.

| Renders | Why |
|---|---|
| `Deployment/lanekeeper-agent` | 1 replica (not a value), the HARDENING.md pod: non-root as the export's owner (1010 by default, Q11), read-only root, seccomp `RuntimeDefault`, all capabilities dropped, the NFS export, `/tmp`, and the hub's CA |
| `ServiceAccount/lanekeeper-agent` | `iam.gke.io/gcp-service-account` for Workload Identity (S5, Q26). Its token **is** mounted: the agent calls the Kubernetes API |
| `Role` and `RoleBinding` `lanekeeper-agent-workloads`, in each namespace of `namespaces` | get, list, watch on Deployments, Pods and Jobs, and patch on Deployments for the restart annotation. RBAC.md, verbatim |
| `Role` and `RoleBinding` `lanekeeper-agent-secrets`, in the agent's namespace | get and update on `lanekeeper-agent-cert`, and get on `lanekeeper-agent-join-token` only with `joinToken.enabled`. By name; **no list, no watch, no create** (S17) |
| `Secret/lanekeeper-agent-cert` | empty and `helm.sh/resource-policy: keep`. The agent may update a Secret but cannot create one (`create` cannot be limited by name), so the chart makes it; the agent writes its certificate into it |
| `ConfigMap/lanekeeper-hub-ca` | the hub's CA certificate (`hubCa.pem`), public. The Deployment's `checksum/hub-ca` restarts the agent when it changes |
| `NetworkPolicy/lanekeeper-agent` | default deny; egress to exactly five places: the hub and the API server (address blocks on 443), the config-server's pods (8888), the metadata server (80) and kube-dns (53). Ingress: Prometheus, on the health port |

The chart refuses to render (`helm template` stops) without an image digest, a hub CA that is a certificate and not a key, a way to
join (Workload Identity or the join token), a swimlane, an NFS export, at least one watched namespace, and an address block for
the hub and for the API server. `values.schema.json` refuses unknown values, a replica count, a tunable the agent does not have,
`kube-system` as a watched namespace, a block wider than a /24, and root as the export's owner.

## The join token

`joinToken.enabled` is the fallback of S5 for a cluster without Workload Identity (Q26). With it the chart adds the grant on the
one Secret `lanekeeper-agent-join-token` (key `token`) and the variable `LK_JOIN_TOKEN_SECRET`, which holds the Secret's **name**.
**The chart never renders the token**: an Admin issues a one-time token for the swimlane in the hub and it is put into the Secret
out of band (`deploy/ADDING-A-SWIMLANE.md`). `LK_JOIN_MODE` is `workload-identity`, `token`, or `auto` (both), by what is set.

## The hub's CA

`hubCa.pem` is the public certificate of the hub's agent CA. It is not in the committed values files: whoever adds a swimlane puts
it in that swimlane's file, or passes it with `--set-file hubCa.pem=ca.pem`. `deploy/tests/render.sh` passes a synthetic one
(`deploy/tests/testdata/synthetic-hub-ca.pem`, a throwaway certificate whose key was deleted at once).

## Checks

```
just deploy-check        # schemas, lint, kubeconform -strict (offline), the policies on the real output, the contract, the planted cases
just contract-check      # this chart against RBAC.md and HARDENING.md
```

Three policies guard it, and each fails on its own: `rbac.rego` (what a Role may grant: the allow-list, never read from RBAC.md),
`network.rego` (the five destinations) and `hardening.rego` (S16 and the agent's contract). `contract.rego` shows that the rendered
Roles, bindings, NetworkPolicy and pod equal the two contract files, which `deploy/tests/contract-extract.sh` reads out of the
Markdown. A change to the chart that the files do not say, and a change to the files that the chart does not follow, are both
refused. `deploy/tests/render.sh planted` changes the real output one thing at a time (a Secret `list`, a wide address block, a
second replica, a missing probe) and requires the policy that guards it to refuse it with its own message.

## Not done yet

- **The spool (T8, D74).** The agent keeps its durable spool at `LK_SPOOL_DIR`, a persistent volume that the chart does not make
  yet, and it stops at start without the directory. T8 adds the volume claim on the CMEK class, the mount, the variable and
  `strategy: Recreate` (with one replica, a rolling update would start the new agent beside the old one). Until then the chart is
  checked but not deployable.
- **The environment, volumes and mounts are not compared with HARDENING.md by `contract.rego`.** `hardening.rego` checks the
  paths (the export, `/tmp`, the hub CA are the paths of their mounts) and `deploy/tests/env-names.sh` the names. T8 extends the
  comparison when the spool arrives.

## Not verified here

- That the hub accepts an agent on **443**. HARDENING.md has the agent dial the hub on 443, and the hub chart's agent load balancer
  listens on 8444 (`plans/09-deployment-ci.blockers.md`, B7). The chart follows the agent's contract; the two have to be reconciled
  before a swimlane is deployed.
- That the cluster enforces NetworkPolicy at all, and that the metadata server and the control plane are reachable from the pod
  network as the policy assumes (a property of each cluster, Q1).
- That the config-server runs in the agent's own namespace. The contract allows its pods in the agent's namespace only; if it runs
  elsewhere the policy needs a `namespaceSelector` and HARDENING.md changes first.

## Rendered with

Helm 4.3.0 (`just ops-tools-install helm`).
