# Hub hardening contract (S6, S16, S18)

What the pod that runs `crates/hub/Dockerfile` must look like, and what may leave it and reach it. The Helm chart
(prompt 09) renders the pods from this file; this prompt owns the contract and the image, and 09 owns the rendered chart
and its policy test. `tests/hardening.rs` reads the YAML blocks below, so the file and the claims in the code cannot drift
apart:

- `hub_dockerfile_is_hardened`: a distroless final image, a numeric non-root user, every base image pinned by digest, no
  shell, no package manager, and only three things copied in (the binary, `web/dist` and the embedding model checked
  against `deploy/images/hub-model.lock`).
- `hardening_md_pod_contract_is_complete`: the Pod block below has every control S16 lists, mounts nothing writable except
  the mirror volume and `/tmp`, mounts no service-account token, and passes only variables that `src/config.rs` reads.
- `hardening_md_network_policy_denies_by_default_and_lists_only_allowed_egress`: the NetworkPolicy blocks allow exactly the
  destinations S18 names, and ingress exactly the four listeners from exactly their callers.
- `hardening_lint_rejects_planted_violations`: each of those checks is shown to refuse a manifest that breaks it.

Values in `<angle brackets>` or marked "from values" are the chart's to fill in. The documentation addresses
(`203.0.113.0/24`, `198.51.100.0/24`) stand for the real load-balancer ranges.

## The image

- **Base:** `gcr.io/distroless/cc-debian12:nonroot`: glibc, libgcc and CA certificates. No shell, no package manager,
  nothing to `exec` into (S16).
- **Pinned by digest**, every base and the Dockerfile frontend. Renovate proposes new digests, and a person reviews them.
- **Three things in it:**
  - `/usr/local/bin/hub`, built with `cargo build --release --locked -p hub` (dependencies first, through cargo-chef);
  - `/app/web/dist`, the web app built with `pnpm install --frozen-lockfile && pnpm build`. The hub refuses to start when
    `index.html` has no `__CSP_NONCE__` placeholder (the web build must emit it, prompt 08);
  - `/app/models/<model>/`, the embedding model's files (D53; plan 09 A3: `bge-small-en-v1.5`, the ONNX graph and its
    tokenizer files). The `model` stage fetches each file named in `deploy/images/hub-model.lock` from the pinned Hugging
    Face commit and stops the build unless its size and sha256 match. The files are read-only to the hub's user; the
    embedding code (prompt 14) is pointed at this directory and never downloads a model at run time.
- **Reproducible:** `deploy/images/build.sh hub` builds it from `git archive HEAD` with `SOURCE_DATE_EPOCH` and rewritten
  timestamps, and `just images-repro` checks that two builds of a commit give one digest
  (`deploy/images/REPRODUCIBILITY.md`).
- **User:** `USER 65532:65532`, numeric.

## The pods

Three replicas (D62), each its own pod with its own mirror volume and a stable DNS name through the headless service, which
is how replicas find one another (`agent_connections.replica_address`). One container, no sidecars. The hub does not talk to
the Kubernetes API, so it mounts no service-account token; Workload Identity reaches Google through the metadata server.
The chart renders this pod as a StatefulSet template.

**Ports.** `web` 8443 (HTTPS, behind the users' load balancer), `agent` 8444 (agents and the NFS sentinel, mTLS), `replica`
8445 (replica-to-replica, mTLS), `ops` 9090 (plain HTTP: `/healthz`, `/readyz`, `/metrics`, cluster-internal). T3 and T4 add
the settings that bind 8444 and 8445.

**Secrets are files, not variables.** The database URL holds a password, so it is read from a mounted Secret file
(`LK_DATABASE_URL_FILE`); no variable in this contract takes its value from a Secret reference, and none has a name that
suggests a secret. Everything else secret comes from Secret Manager at run time (S7).

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: lanekeeper-hub
  namespace: lanekeeper
  labels:
    app.kubernetes.io/name: lanekeeper-hub
spec:
  serviceAccountName: lanekeeper-hub
  automountServiceAccountToken: false
  hostNetwork: false
  hostPID: false
  hostIPC: false
  securityContext:
    runAsNonRoot: true
    runAsUser: 65532
    runAsGroup: 65532
    fsGroup: 65532
    seccompProfile:
      type: RuntimeDefault
  containers:
  - name: hub
    image: ghcr.io/<org>/lanekeeper-hub@sha256:<digest>
    securityContext:
      allowPrivilegeEscalation: false
      privileged: false
      readOnlyRootFilesystem: true
      capabilities:
        drop: ["ALL"]
    resources:
      requests: {cpu: 500m, memory: 1536Mi}
      limits: {cpu: "4", memory: 2Gi}
    ports:
    - {name: web, containerPort: 8443, protocol: TCP}
    - {name: agent, containerPort: 8444, protocol: TCP}
    - {name: replica, containerPort: 8445, protocol: TCP}
    - {name: ops, containerPort: 9090, protocol: TCP}
    livenessProbe:
      httpGet: {path: /healthz, port: ops}
      periodSeconds: 10
      timeoutSeconds: 3
      failureThreshold: 3
    readinessProbe:
      httpGet: {path: /readyz, port: ops}
      periodSeconds: 5
      timeoutSeconds: 3
    env:
    - name: LK_WEB_ADDR
      value: 0.0.0.0:8443
    - name: LK_OPS_ADDR
      value: 0.0.0.0:9090
    - name: LK_WEB_DIST
      value: /app/web/dist
    - name: LK_TRUSTED_PROXY_HOPS
      value: "2"
    - name: LK_DATABASE_URL_FILE
      value: /etc/lanekeeper/db/uri
    - name: LK_DB_CA_FILE
      value: /etc/lanekeeper/db-ca/ca.crt
    - name: LK_MIRROR_DIR
      value: /var/lib/lanekeeper/mirror
    - name: LK_OTLP_ENDPOINT
      value: http://otel-collector.monitoring.svc:4317
    volumeMounts:
    - name: tmp
      mountPath: /tmp
    - name: mirror
      mountPath: /var/lib/lanekeeper/mirror
    - name: db-credentials
      readOnly: true
      mountPath: /etc/lanekeeper/db
    - name: db-ca
      readOnly: true
      mountPath: /etc/lanekeeper/db-ca
  volumes:
  - name: tmp
    emptyDir: {sizeLimit: 256Mi}
  - name: mirror
    persistentVolumeClaim: {claimName: lanekeeper-hub-mirror}
  - name: db-credentials
    secret: {secretName: lanekeeper-db-app, defaultMode: 288}
  - name: db-ca
    secret: {secretName: lanekeeper-db-ca}
```

`LK_TRUSTED_PROXY_HOPS` is 2 behind Google's external HTTPS load balancer, which appends the client address and then its own
(`client_ip.rs` counts from the right). The value must be re-checked whenever the path to the hub changes.

### Where the hub writes

The root filesystem is read-only. Two mounts are writable, and the code writes nowhere else:

| Mount | Variable | What the hub writes there |
|---|---|---|
| PersistentVolumeClaim | `LK_MIRROR_DIR` | The bare Git mirrors, one per repository (T7). Each replica has its own volume; a lost volume is a full re-fetch. |
| `emptyDir` | none (`/tmp`) | Temporary files of the libraries. Capped at 256 MiB. |

The database credentials and the database CA are read-only Secret mounts (the first with mode `0440`, `defaultMode: 288`).
The CA certificates of the agent and the replica listeners are public and arrive through the settings of T3 and T4.

## The network

NetworkPolicies deny by default (S18): both policy types are set, so everything not listed is denied. A Kubernetes
NetworkPolicy cannot name a host, so the external destinations are a second policy, an FQDN network policy (GKE
`FQDNNetworkPolicy`, which needs FQDN network policies enabled on the cluster), and the first policy holds only in-cluster
destinations.

| Destination | Port | Why |
|---|---|---|
| `kube-dns` | 53/UDP and 53/TCP | Names. |
| CloudNativePG pods of `lanekeeper-db` | 5432/TCP | Postgres, TLS with full verification (S6). |
| Other hub replicas | 8445/TCP | Forwarded commands, mTLS (D62). |
| The OpenTelemetry collector in `monitoring` | 4317/TCP | Traces, which the collector forwards to Cloud Trace (A9). |
| `169.254.169.254` | 80/TCP | The GKE metadata server, for Workload Identity (S5, S7). |
| `login.microsoftonline.com` | 443/TCP | Entra sign-in and its signing keys (03a). |
| `github.com`, `api.github.com` | 443/TCP | Git fetch and the GitHub API (T7, 06). |
| `secretmanager.googleapis.com`, `cloudkms.googleapis.com`, `www.googleapis.com` | 443/TCP | Secret Manager, Cloud KMS (T2), and the keys that verify Google ID tokens (T3). |
| the Teams webhook host | 443/TCP | Notifications (Q9: the host depends on the answer). |

In: the web port from the users' load balancer range, the agent port from the agent load balancer range, the replica port
from the hub's own pods, and the operations port from the `monitoring` namespace. The kubelet's probes come from the node.
The load balancers' own source-IP allowlists, and the GitHub hook ranges on the webhook path, are the chart's (S18, Q28).

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: lanekeeper-hub
  namespace: lanekeeper
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: lanekeeper-hub
  policyTypes: [Ingress, Egress]
  ingress:
  - from:
    - ipBlock: {cidr: 203.0.113.0/24}
    ports:
    - {protocol: TCP, port: 8443}
  - from:
    - ipBlock: {cidr: 198.51.100.0/24}
    ports:
    - {protocol: TCP, port: 8444}
  - from:
    - podSelector:
        matchLabels:
          app.kubernetes.io/name: lanekeeper-hub
    ports:
    - {protocol: TCP, port: 8445}
  - from:
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: monitoring
    ports:
    - {protocol: TCP, port: 9090}
  egress:
  - to:
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: kube-system
      podSelector:
        matchLabels:
          k8s-app: kube-dns
    ports:
    - {protocol: UDP, port: 53}
    - {protocol: TCP, port: 53}
  - to:
    - podSelector:
        matchLabels:
          cnpg.io/cluster: lanekeeper-db
    ports:
    - {protocol: TCP, port: 5432}
  - to:
    - podSelector:
        matchLabels:
          app.kubernetes.io/name: lanekeeper-hub
    ports:
    - {protocol: TCP, port: 8445}
  - to:
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: monitoring
      podSelector:
        matchLabels:
          app.kubernetes.io/name: otel-collector
    ports:
    - {protocol: TCP, port: 4317}
  - to:
    - ipBlock: {cidr: 169.254.169.254/32}
    ports:
    - {protocol: TCP, port: 80}
---
apiVersion: networking.gke.io/v1alpha1
kind: FQDNNetworkPolicy
metadata:
  name: lanekeeper-hub-external
  namespace: lanekeeper
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: lanekeeper-hub
  egress:
  - matches:
    - pattern: login.microsoftonline.com
    - pattern: github.com
    - pattern: api.github.com
    - pattern: secretmanager.googleapis.com
    - pattern: cloudkms.googleapis.com
    - pattern: www.googleapis.com
    - pattern: <teams webhook host>
    ports:
    - {protocol: TCP, port: 443}
```

## What this contract cannot prove

These stay open after the tests pass. Each is a residual risk in `docs/threat-model.md` (Hub platform section).

- **The image build.** The Dockerfile has been linted and the digests were read from the registries, but whether the build
  succeeds, that `aws-lc-sys` compiles in the builder image, and that the binary runs on the distroless base are shown by
  `deploy/images/build.sh` and CI (09/T1), not by the tests here.
- **What the cluster enforces.** Whether the cluster's CNI enforces NetworkPolicy and FQDN policy at all is a property of
  the cluster. A cluster without enforcement ignores both policies; the chart's policy test (09) checks the rendering, not
  the cluster.
- **FQDN policy is name-based.** A destination that shares an address with an allowed name (a CDN) is reachable while the
  name resolves there. The hub's own checks (TLS to the named host, HMAC, signature verification) are what stand behind it.
- **Digests age.** A pinned base image does not receive security fixes until someone changes the pin.
