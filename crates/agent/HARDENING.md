# Agent hardening contract (S16, S17, S18)

What the pod that runs `crates/agent/Dockerfile` must look like, and what may leave it. The Helm chart (prompt 09)
renders the pod from this file; this prompt owns the contract and the image, and 09 owns the rendered chart and its
policy test. `tests/hardening.rs` reads the YAML blocks below, so the file and the claims in the code cannot drift apart:

- `dockerfile_is_distroless_nonroot_pinned`: a distroless final image, a numeric non-root user, both base images pinned
  by digest, no shell, no package manager, one binary.
- `hardening_contract_is_s16_complete`: the Pod block below has every control S16 lists, mounts nothing writable
  except the three paths the code writes to, and passes the paths it uses to the code through the right variables.
- `hardening_contract_egress_is_hub_kube_configserver_metadata_dns_only`: the NetworkPolicy block allows exactly five
  destinations, none of them wider than a /24.
- `hardening_lint_rejects_planted_violations`: each of those checks is shown to refuse a manifest that breaks it.

Values in `<angle brackets>` or marked "from values" are the chart's to fill in. The documentation addresses
(`203.0.113.0/24`, `198.51.100.0/24`) stand for the real hub and API server addresses.

## The image

- **Base:** `gcr.io/distroless/cc-debian12:nonroot`: glibc, libgcc and CA certificates. No shell, no package manager,
  nothing to `exec` into (S16, S17). The agent starts no process itself (`no_shell_or_exec_in_source`).
- **Pinned by digest**, here and for the Rust builder. The digests were read from the registries on 2026-10-10; Renovate
  proposes new ones, and a person reviews them.
- **One file:** `/usr/local/bin/agent`, built with `cargo build --release --locked -p agent`, so the versions are the ones
  `cargo deny` and `cargo audit` checked.
- **User:** `USER 65532:65532`, numeric. The pod overrides it with the uid and gid that own the NFS files (below).

## The pod

One container, no sidecars. The agent talks to the Kubernetes API, so the service-account token is mounted
(`automountServiceAccountToken: true`); its Roles are `RBAC.md` and nothing else. Everything else S16 asks for is set.

**The uid and gid (Q11).** The NFS files are owned by uid and gid 1010 (recorded with the answer to Q4 in `docs/open-questions.md`), so the chart passes 1010
through `runAsUser`, `runAsGroup` and `fsGroup` from values. Whether the export uses `root_squash` and which `anonuid` it maps
to is still open (Q11): it must be settled before the agent writes on a pilot swimlane. A different uid is a value change,
not a code change.

**Resources.** The budget is 50 mCPU and 64 MiB at steady state (P4), measured by `agent/steady_state_cpu_mcores` and
`agent/steady_state_memory_mib`. The request is the memory budget, so the pod is scheduled where it fits; the limit leaves
room for a full rehash every 15 minutes, a 1-second walk burst while a change is arriving, and four delta messages of up
to 3 MiB in flight. The numbers come from the local benchmark and are to be confirmed on the pilot (A15).

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: lanekeeper-agent
  namespace: lanekeeper
  labels:
    app.kubernetes.io/name: lanekeeper-agent
spec:
  serviceAccountName: lanekeeper-agent
  automountServiceAccountToken: true
  hostNetwork: false
  hostPID: false
  hostIPC: false
  securityContext:
    runAsNonRoot: true
    runAsUser: 1010
    runAsGroup: 1010
    fsGroup: 1010
    seccompProfile:
      type: RuntimeDefault
  containers:
  - name: agent
    image: ghcr.io/<org>/lanekeeper-agent@sha256:<digest>
    securityContext:
      allowPrivilegeEscalation: false
      privileged: false
      readOnlyRootFilesystem: true
      capabilities:
        drop: ["ALL"]
    resources:
      requests: {cpu: 25m, memory: 64Mi}
      limits: {cpu: "1", memory: 128Mi}
    ports:
    - {name: ops, containerPort: 9090, protocol: TCP}
    livenessProbe:
      httpGet: {path: /healthz, port: ops}
      periodSeconds: 20
      timeoutSeconds: 3
      failureThreshold: 3
    readinessProbe:
      httpGet: {path: /readyz, port: ops}
      periodSeconds: 10
      timeoutSeconds: 3
    env:
    - name: LK_HUB_ENDPOINT
      value: https://<hub host>
    - name: LK_HUB_AUDIENCE
      value: https://<hub host>
    - name: LK_HUB_CA_FILE
      value: /etc/lanekeeper/hub-ca/ca.pem
    - name: LK_SWIMLANE
      value: <swimlane id>
    - name: LK_CLUSTER
      value: <cluster>
    - name: LK_PROJECT
      value: <project>
    - name: LK_NFS_SERVER
      value: <nfs server>
    - name: LK_NFS_EXPORT
      value: <export path>
    - name: LK_NFS_ROOT
      value: /mnt/csp-configuration
    - name: LK_CERT_SECRET
      value: lanekeeper-agent-cert
    - name: LK_JOIN_TOKEN_SECRET
      value: lanekeeper-agent-join-token
    - name: LK_NAMESPACES
      value: <namespaces>
    - name: LK_SPOOL_DIR
      value: /var/lib/lanekeeper/spool
    - name: LK_TMP_DIR
      value: /tmp
    - name: LK_CONFIG_SERVER_URL
      value: http://csp-configuration-server:8888
    - name: LK_HEALTH_ADDR
      value: 0.0.0.0:9090
    volumeMounts:
    - name: nfs
      mountPath: /mnt/csp-configuration
      readOnly: false
    - name: tmp
      mountPath: /tmp
    - name: spool
      mountPath: /var/lib/lanekeeper/spool
    - name: hub-ca
      readOnly: true
      mountPath: /etc/lanekeeper/hub-ca
  volumes:
  - name: nfs
    nfs: {server: <nfs server>, path: <export path>, readOnly: false}
  - name: tmp
    emptyDir: {sizeLimit: 64Mi}
  - name: spool
    persistentVolumeClaim: {claimName: lanekeeper-agent-spool}
  - name: hub-ca
    configMap: {name: lanekeeper-hub-ca}
```

### Where the agent writes

The root filesystem is read-only. Three mounts are writable, and the code writes nowhere else
(`no_fs_writes_outside_spool_and_tmp` scans the source for file-system writes, and `all_file_access_via_cap_std` for
ambient file access):

| Mount | Variable | What the agent writes there |
|---|---|---|
| NFS export | `LK_NFS_ROOT` | The files the hub asks it to write, byte-exact and only against the expected hash (S11). Each write goes to a `.lanekeeper-tmp-*` file beside its target and is renamed over it. |
| `emptyDir` | `LK_TMP_DIR` | Nothing today. It exists so that a library that wants a temporary file does not fail on a read-only root. Capped at 64 MiB. |
| PersistentVolumeClaim | `LK_SPOOL_DIR` | The durable spool of observed versions (D74, added in T9). Must not be inside the NFS root, or the spool would be tracked as config. |

The hub CA is a read-only ConfigMap mount. The certificate and key are not on disk: they are in the certificate Secret
(`RBAC.md`), and in memory.

The chart creates the PVC (small: the spool is bounded at 512 MiB by default, `LK_SPOOL_MAX_BYTES`) and the certificate
Secret (`RBAC.md` explains why the agent cannot create it). The volume must be larger than `LK_SPOOL_MAX_BYTES`: the bound
counts the segment files, and the small `state` file is on top of it. On a volume that is too small or full, an append
fails, the delta is not spooled and the hub learns of the change by root comparison; nothing else stops. The agent
creates its files with mode `0600`, and does not create the directory: it is a mounted volume, and the agent stops at
start if it is missing.

## The network

Everything the agent says goes to one of five places; the NetworkPolicy lists exactly those and nothing else, so
everything not listed is denied (S17, S18).

| Destination | Port | Why |
|---|---|---|
| The hub's address | 443/TCP | The gRPC stream (S6), and `Join`. |
| The Kubernetes API server | 443/TCP | Watches, restarts and the two Secrets (`RBAC.md`). |
| `app: csp-configuration-server` pods in the same namespace | 8888/TCP | `NotifyConfigServer` and `FetchServed` (T12), only when the hub asks. The endpoint has no authentication (Q37); that is why nothing else may reach it from here, and why the hub requires the Operator role and audits the call. |
| `169.254.169.254` | 80/TCP | The GKE metadata server, for the Workload Identity token (S5). |
| `kube-dns` in `kube-system` | 53/UDP and 53/TCP | Names. |

The NFS export is mounted by the node's kubelet, so it needs no rule here. The kubelet's probes come from the node.
The only other traffic in is Prometheus, from the `monitoring` namespace, on the health port.

How the hub's and the API server's addresses are found is open question Q1 (outbound connections from pods) for the first, and
the cluster's control-plane endpoint for the second; both are values, and a wider block than a /24 is refused by the test.

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: lanekeeper-agent
  namespace: lanekeeper
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: lanekeeper-agent
  policyTypes: [Ingress, Egress]
  ingress:
  - from:
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: monitoring
    ports:
    - {protocol: TCP, port: 9090}
  egress:
  - to:
    - ipBlock: {cidr: 203.0.113.10/32}
    ports:
    - {protocol: TCP, port: 443}
  - to:
    - ipBlock: {cidr: 198.51.100.2/32}
    ports:
    - {protocol: TCP, port: 443}
  - to:
    - podSelector:
        matchLabels:
          app: csp-configuration-server
    ports:
    - {protocol: TCP, port: 8888}
  - to:
    - ipBlock: {cidr: 169.254.169.254/32}
    ports:
    - {protocol: TCP, port: 80}
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
```

## What this contract cannot prove

These stay open after the tests pass. Each is a residual risk in `docs/threat-model.md` (Agent section).

- **The image build.** There is no Docker daemon where this was written. The Dockerfile has been linted, and the digests
  were read from the registries, but `docker build` has not run: that the build succeeds, that `aws-lc-sys` compiles in the
  builder image, and that the binary runs on the distroless base are shown by CI, not here.
- **What the cluster enforces.** Whether the cluster's CNI enforces NetworkPolicy at all (the policy is the contract; a
  cluster without enforcement ignores it), and whether the node's kubelet and the metadata server are reachable from the
  pod network, are properties of each cluster (Q1). The chart's policy test (09) checks the rendering, not the cluster.
- **The NFS server.** `root_squash`, `anonuid` and the export options are Q11. The pod runs as the uid that owns the
  files, which makes a compromised agent as powerful over the export as that uid is, and no more.
- **Digests age.** A pinned base image does not receive security fixes until someone changes the pin. The pin is renewed by
  Renovate; the image is rebuilt on each release.
- **The service-account token** is mounted because the agent needs the Kubernetes API. Anything that runs inside the
  container could read it. There is no shell and no second process to do that, and its Roles are the ones in `RBAC.md`, but
  the token is there.
