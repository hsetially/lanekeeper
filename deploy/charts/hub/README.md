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
| `Service/lanekeeper-hub-agent` | `LoadBalancer`, `externalTrafficPolicy: Local`, `loadBalancerSourceRanges` = `network.agentSourceRanges` (never empty) |
| `Service/lanekeeper-hub-web`, `Gateway`, `HTTPRoute`, `GCPBackendPolicy`, `HealthCheckPolicy` | the users' door: HTTPS only, a named certificate (never a Secret), Cloud Armor attached, health check `/readyz` on 9090 |
| `NetworkPolicy/lanekeeper-hub`, `FQDNNetworkPolicy/lanekeeper-hub-external` | default deny; only what HARDENING.md lists. Without FQDN policy (`network.fqdnPolicy: false`) the external hosts are reached by address, which is weaker and marked |

The chart refuses to render (`helm template` stops) without an image digest, a Google service account, non-empty agent source
ranges, and, for the Gateway, a host name, an address name and exactly one certificate. `values.schema.json` refuses unknown
values, a tunable the hub does not have, and a CA Secret that is not CloudNativePG's.

## Values files

`deploy/values/hub/{kind,staging,prod}.yaml`. They hold documentation addresses and example names until Q1, Q2, Q29 and the
GCP project are answered (rule 12), and the image digest is a placeholder of 64 zeros until the first image is published
(plan 09 A29): it cannot be pulled, and the images policy warns about it.

## Checks

```
just deploy-check        # schemas, lint, kubeconform -strict (offline), the policies on the real output, the planted cases
deploy/tests/render.sh policy      # one stage of it
```

`deploy/tests/render.sh planted` changes the real output of the chart one thing at a time (a hardening field, a range, a
host) and requires the policy that guards it to refuse it with its own message. The policies and their unit tests are in
`deploy/policy`; `deploy/schemas` holds the schemas kubeconform reads.

## Rendered with

Helm 4.3.0 (`just ops-tools-install helm`). The output also equals Helm 3.21.4's (checked once, apart from blank lines), so
an ArgoCD that renders with Helm 3 gets the same manifests.

## Not verified here

The sandbox that wrote this has no GKE cluster and could not read Google's documentation (`docs.cloud.google.com` is refused),
so the Google-specific shapes follow the vendored schemas and what is known of GKE, and the first real apply is their test:

- the Gateway's certificate options (`networking.gke.io/pre-shared-certs`, `/cert-manager-certs`) and `type: NamedAddress`;
- `cloud.google.com/l4-rbs` on the agent Service, and that `externalTrafficPolicy: Local` keeps the clients' addresses there;
- that Google's proxies and health checks for `gke-l7-global-external-managed` come from `35.191.0.0/16` and `130.211.0.0/22`
  (plan 09 A10), which the NetworkPolicy relies on;
- that the cluster enforces NetworkPolicy and FQDNNetworkPolicy at all (a property of the cluster, not of the chart);
- the schemas of `GCPBackendPolicy`, `HealthCheckPolicy` and `FQDNNetworkPolicy` come from the community catalog
  (`deploy/schemas/sources.json`), not from Google, and are looser than the others.
