# Agent RBAC (S17)

What the agent may ask of the Kubernetes API, and why. The Helm chart (prompt 09) renders these Roles. A test
(`tests/kube_rbac.rs`) reads the YAML blocks in this file, so the file and the code cannot drift apart:

- `rbac_manifest_has_no_secret_list` lints the manifest: nothing outside the allow-list below, no wildcard, no
  ClusterRole, and no Secret rule without `resourceNames` or with a verb other than `get` and `update`.
- `rbac_lint_rejects_planted_secret_list` proves that the lint refuses a list, a watch, an unnamed grant and the rest.
- `rbac_secret_rules_are_named_and_minimal` pins the two Secret grants below.
- `fake_api_log_has_no_configmap_and_only_named_secret_calls` runs the agent's real Kubernetes code against a fake API
  server and checks every call it made against this manifest.

## What the agent does

| Call | Verb | Where in the code |
|---|---|---|
| Watch Deployments, Pods and Jobs in each configured namespace (`LK_NAMESPACES`) | `list`, `watch` | `kube::watch` |
| Restart a Deployment: a merge patch of `spec.template.metadata.annotations["kubectl.kubernetes.io/restartedAt"]` | `patch` | `kube::restart` |
| Read the certificate Secret, and write the renewed certificate into it | `get`, `update` | `identity::store` |
| Read the join-token Secret, only when Workload Identity is not available (Q26) | `get` | `identity::jointoken` |

`get` on Deployments, Pods and Jobs is granted because the prompt lists it and a debugging session needs it; the agent
does not use it today.

## What it never does

- **No Secret `list` or `watch`**, ever, and no Secret access without a name. The two Secret grants name the Secrets in
  `resourceNames`; Kubernetes does not apply `resourceNames` to `list` and `watch`, so granting those verbs would
  expose every Secret in the namespace. This is acceptance 4 of the prompt.
- **No `create` on Secrets.** `create` cannot be limited by name, so the chart creates the certificate Secret (empty
  `tls.crt` and `tls.key`) and the agent only updates it (decision A2).
- No ConfigMaps, no `pods/exec`, no `pods/log`, no nodes, no namespaces, no events, no RBAC objects, no wildcards.
- No ClusterRole. Every Role is in a namespace the agent is configured for, or in the agent's own namespace. The
  agent cannot see a namespace it was not given, and `kube::restart` refuses to patch in one before it calls the API.

## The manifest

One Role and RoleBinding per entry of `LK_NAMESPACES` (here `sit1`) for the workloads; one pair in the agent's own
namespace (here `lanekeeper`) for the two Secrets. The names of the Secrets are the chart's defaults for
`LK_CERT_SECRET` and `LK_JOIN_TOKEN_SECRET`.

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: lanekeeper-agent-workloads
  namespace: sit1
rules:
  # Watch the Deployments (and read their pod template's environment variable names, selector and Helm labels).
  - apiGroups: ["apps"]
    resources: ["deployments"]
    verbs: ["get", "list", "watch"]
  # Restart: patch the pod template's restartedAt annotation. Nothing else is written, and nothing is deleted.
  - apiGroups: ["apps"]
    resources: ["deployments"]
    verbs: ["patch"]
  # Pods give the start times that show whether a change has been picked up.
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch"]
  # Jobs: the sync Jobs (D75) and Helm hints on Jobs (Q19).
  - apiGroups: ["batch"]
    resources: ["jobs"]
    verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: lanekeeper-agent-workloads
  namespace: sit1
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: lanekeeper-agent-workloads
subjects:
  - kind: ServiceAccount
    name: lanekeeper-agent
    namespace: lanekeeper
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: lanekeeper-agent-secrets
  namespace: lanekeeper
rules:
  # The certificate Secret: read the key and certificate at start, write the renewed certificate (S5).
  - apiGroups: [""]
    resources: ["secrets"]
    resourceNames: ["lanekeeper-agent-cert"]
    verbs: ["get", "update"]
  # The join-token Secret: read the one-time token, only when Workload Identity is unavailable (Q26).
  - apiGroups: [""]
    resources: ["secrets"]
    resourceNames: ["lanekeeper-agent-join-token"]
    verbs: ["get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: lanekeeper-agent-secrets
  namespace: lanekeeper
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: lanekeeper-agent-secrets
subjects:
  - kind: ServiceAccount
    name: lanekeeper-agent
    namespace: lanekeeper
```

## Notes for the chart

- The ServiceAccount is `lanekeeper-agent`, and its token is mounted (the agent is a Kubernetes client). Nothing else
  about it needs permission.
- If ArgoCD manages the Deployments, it may revert the `restartedAt` annotation at its next sync (Q10). The restart has
  already rolled the pods by then; whether ArgoCD should ignore the annotation is a chart decision.
- A Role cannot grant "all namespaces the agent is configured for" in one object. The chart renders one workload Role
  and RoleBinding per namespace in `LK_NAMESPACES`, and the agent's configuration and the Roles must name the same
  namespaces: a namespace in `LK_NAMESPACES` without a Role makes that namespace's watchers log `403` and retry.
