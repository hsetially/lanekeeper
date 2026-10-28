# Hub hardening contract (S6, S16, S18)

What the pod that runs `crates/hub/Dockerfile` must look like, and what may leave it and reach it. The Helm chart
(prompt 09) renders the pods from this file; this prompt owns the contract and the image, and 09 owns the rendered chart
and its policy test. `tests/hardening.rs` reads the YAML blocks below, so the file and the claims in the code cannot drift
apart:

- `hub_dockerfile_is_hardened`: a distroless final image, a numeric non-root user, every base image pinned by digest, no
  shell, no package manager, and only three things copied in (the binary, `web/dist` and the embedding model checked
  against `deploy/images/hub-model.lock`).
- `hardening_md_pod_contract_is_complete`: the Pod block below has every control S16 lists, mounts nothing writable except
  the mirror volume and `/tmp`, mounts no service-account token, mounts each database Secret through the one key the hub
  needs, and passes only variables that `src/config.rs` reads.
- `hardening_md_network_policy_denies_by_default_and_lists_only_allowed_egress`: the NetworkPolicy blocks allow exactly the
  destinations S18 names, and ingress exactly the four listeners from exactly their callers.
- `hardening_lint_rejects_planted_violations`: each of those checks is shown to refuse a manifest that breaks it.

The checklist that the chart and the install guide are matched against, in one place and in the order of a first deployment, is the last
section of `README.md`; this file is the detail behind it.

Values in `<angle brackets>` or marked "from values" are the chart's to fill in. `198.51.100.0/24` is a documentation
address that stands for the real source ranges of the agents (the clusters' outbound IPs, Q1). `35.191.0.0/16` and
`130.211.0.0/22` are real: they are where Google's load balancers, proxies and health checks come from (plan 09 A10).

## The image

- **Base:** `gcr.io/distroless/cc-debian13:nonroot`: glibc, libgcc and CA certificates. No shell, no package manager,
  nothing to `exec` into (S16). Debian 13 because `cc-debian12` is deprecated and carries critical findings that will not
  be fixed (plan 09 A32); the Rust builder (`rust:1.88.0-trixie`) is the same release.
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
The chart renders this pod as a StatefulSet template: the `mirror` volume below is the set's `volumeClaimTemplate`, so each
replica gets its own claim (`mirror-lanekeeper-hub-0`, `-1`, `-2`), on a StorageClass that encrypts with a customer-managed key
(S10), and the replicas are kept in different zones (D62).

**Ports.** `web` 8443 (HTTPS, behind the users' load balancer), `agent` 8444 (agents and the NFS sentinel, mTLS), `replica`
8445 (replica-to-replica, mTLS), `ops` 9090 (plain HTTP: `/healthz`, `/readyz`, `/metrics`, cluster-internal). `LK_AGENT_ADDR` and
`LK_REPLICA_ADDR` bind 8444 and 8445 (plan 18, T3).

**Secrets are files, not variables.** The database URL holds a password, so it is read from a mounted Secret file
(`LK_DATABASE_URL_FILE`); no variable in this contract takes its value from a Secret reference, and none has a name that
suggests a secret. Everything else secret comes from Secret Manager at run time (S7). Each Secret is mounted through the one
key the hub needs (`items`): the credentials Secret only `uri`, not the user name or the password it also holds; and
CloudNativePG's CA Secret only `ca.crt`, because it also holds `ca.key`, which signs server certificates (S6, plan 09 A9).

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
  terminationGracePeriodSeconds: 45
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
    startupProbe:
      httpGet: {path: /healthz, port: ops}
      periodSeconds: 2
      timeoutSeconds: 2
      failureThreshold: 30
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
    - name: LK_GCP_PROJECT
      value: <gcp project id>
    - name: LK_WEB_CERT_REF
      value: lanekeeper-web-cert
    - name: LK_WEB_KEY_REF
      value: lanekeeper-web-key
    - name: LK_AGENT_ADDR
      value: 0.0.0.0:8444
    - name: LK_REPLICA_ADDR
      value: 0.0.0.0:8445
    - name: LK_REPLICA_NAME
      valueFrom:
        fieldRef: {fieldPath: metadata.name}
    - name: LK_REPLICA_ADDRESS
      value: $(LK_REPLICA_NAME).lanekeeper-hub.lanekeeper.svc:8445
    - name: LK_HUB_AUDIENCE
      value: https://<hub hostname>
    - name: LK_AGENT_ENDPOINT_HOSTS
      value: <agent endpoint hostname>
    - name: LK_AGENT_CA_KMS_REF
      value: <agent CA key version, a Cloud KMS resource name>
    - name: LK_AGENT_CA_CERT_REF
      value: lanekeeper-agent-ca-cert
    - name: LK_REPLICA_CA_KMS_REF
      value: <replica CA key version, a Cloud KMS resource name>
    - name: LK_REPLICA_CA_CERT_REF
      value: lanekeeper-replica-ca-cert
    - name: LK_GIT_BASE_URL
      value: https://github.com/<org>/configuration-base-saas.git
    - name: LK_GIT_TENANT_URL
      value: https://github.com/<org>/csp-tenant-data.git
    - name: LK_GIT_BASE_REPO
      value: <org>/configuration-base-saas
    - name: LK_GIT_TENANT_REPO
      value: <org>/csp-tenant-data
    - name: LK_PUBLIC_ORIGIN
      value: https://<hub hostname>
    - name: LK_ENTRA_TENANT_ID
      value: <Entra tenant id, a GUID>
    - name: LK_ENTRA_CLIENT_ID
      value: <Entra application (client) id, a GUID>
    - name: LK_ENTRA_CLIENT_AUTH_REF
      value: lanekeeper-entra-client
    - name: LK_ENTRA_COOKIE_KEY_REF
      value: lanekeeper-login-cookie
    - name: LK_AUDIT_CHECKPOINT_BUCKET
      value: <Cloud Storage bucket with a retention lock>
    - name: LK_AUDIT_CHECKPOINT_KMS_REF
      value: <audit checkpoint key version, a Cloud KMS resource name>
    - name: LK_BOOTSTRAP_ADMINS
      value: <comma separated Entra object ids of the first admins>
    - name: LK_VAULT_KMS_REF
      value: <vault key, a Cloud KMS crypto key resource name without a version>
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
    secret:
      secretName: lanekeeper-db-app
      defaultMode: 288
      items:
      - {key: uri, path: uri}
  - name: db-ca
    secret:
      secretName: lanekeeper-db-ca
      items:
      - {key: ca.crt, path: ca.crt}
```

**Probes.** Start-up is budgeted at 10 s (P11), and the startup probe allows 60 s (30 tries, 2 s apart) so that a slow node is
not a crash loop. It asks `/healthz`, which does not depend on the database: a database outage must never look like a failed
start, and the liveness probe does not begin until the start has succeeded. `/readyz` is readiness only, and it is also what
the users' load balancer's health check asks.

`LK_TRUSTED_PROXY_HOPS` is 2 behind Google's external HTTPS load balancer, which appends the client address and then its own
(`client_ip.rs` counts from the right). The value must be re-checked whenever the path to the hub changes.

**The web certificate.** The web listener serves HTTPS with a certificate chain and a key that the hub reads from two Secret
Manager secrets, named by `LK_WEB_CERT_REF` and `LK_WEB_KEY_REF` (plan 18, G3). They are not a Kubernetes Secret, not a mounted file
and not a variable, so the pod has no volume for them. The hub reads them at start and again every `LK_WEB_CERT_RELOAD_SECS` (300 by
default, each wait jittered by a fifth): a rotated certificate is served from the next handshake on, connections already open keep the
one they started with, and a replacement that does not parse, does not match its key or has already expired changes nothing and is
counted (`lanekeeper_web_cert_reload_failures_total`). The two secrets are rotated one after the other, so a read can find the new
certificate beside the old key; that read is refused and the next one succeeds. The certificate in use is exported
(`lanekeeper_web_cert_not_after_seconds`), and `/readyz` fails once it has expired. The names are `*_REF` and not `*_SECRET` or `*_KEY`
because the pod contract flags any variable whose name looks like a secret, and these hold a name, not a secret.

**The platform: two CAs and this replica's identity (plan 18, T3, G10; S5, S6).** The hub holds no private key. Agents and sentinels
prove who they are with a certificate that a CA issues after checking their Google ID token, and replicas prove who they are to each
other the same way, from a second CA, so that an agent's certificate is never a replica's. Each CA is a Cloud KMS key version
(`LK_AGENT_CA_KMS_REF`, `LK_REPLICA_CA_KMS_REF`: a resource name, `projects/P/locations/L/keyRings/R/cryptoKeys/K/cryptoKeyVersions/N`)
and the public certificate that `hub ca-init` made for it, kept in a Secret Manager secret (`LK_AGENT_CA_CERT_REF`,
`LK_REPLICA_CA_CERT_REF`). At start the hub has KMS sign one probe digest and checks the signature against the certificate's key: a
certificate that does not belong to the key, a CA that has expired, or a secret that holds anything but one certificate stops the
start (exit 1, naming the CA and the two variables, never a value). It then has each CA issue this replica's own server certificate,
in memory, with a key made at start, and renews it at half its life of 24 hours.

| Variable | What it is |
|---|---|
| `LK_REPLICA_NAME` | This replica's stable name: the StatefulSet pod name, from the downward API (`fieldRef: metadata.name`, the one `valueFrom` the pod contract allows). It is in the certificate's identity (`spiffe://lanekeeper/hub/<name>`), in `agent_connections` and as the holder of every lease. |
| `LK_REPLICA_ADDRESS` | Where the other replicas dial this one: `<pod name>.<headless service>.<namespace>.svc:8445`. Its host is the DNS name on the replica certificate. |
| `LK_HUB_AUDIENCE` | The audience a Google ID token must carry for `Join`: the hub's own name, which the agents are told to ask Google for. |
| `LK_AGENT_ENDPOINT_HOSTS` | The DNS names agents reach the agent endpoint by (D25), comma separated, lower case, at most eight. They are on the agent listener's certificate, which the agents verify. |
| `LK_AGENT_CA_KMS_REF`, `LK_REPLICA_CA_KMS_REF` | The two CA keys (Cloud KMS key versions, `EC_SIGN_P256_SHA256`). They must differ: the same value in both is exit 2, naming `LK_REPLICA_CA_KMS_REF` (S5). |
| `LK_AGENT_CA_CERT_REF`, `LK_REPLICA_CA_CERT_REF` | The Secret Manager secrets that hold the two CA certificates (PEM, public). They must differ too (exit 2, naming `LK_REPLICA_CA_CERT_REF`). |
| `LK_AGENT_ADDR`, `LK_REPLICA_ADDR` | The two listeners' bind addresses (8444 and 8445 unless told otherwise). |
| `LK_GIT_BASE_URL`, `LK_GIT_TENANT_URL`, `LK_GIT_BASE_REPO`, `LK_GIT_TENANT_REPO` | The two repositories the hub mirrors (HTTPS, no credential in the URL) and the `owner/name` GitHub reports for them in webhooks. All four are required. `LK_GIT_READ_REF` and `LK_GIT_WEBHOOK_REF` name the Secret Manager secrets of the read-only credential and the webhook secret (`github-read-token` and `github-webhook-secret` unless set). |

The Google identity the pod runs as (Workload Identity) needs `roles/cloudkms.signerVerifier` on the two CA key versions and nothing
else on KMS, `roles/secretmanager.secretAccessor` on the secrets above, and no other role (S7). The replica listener's NetworkPolicy
ingress is the hub's own pods only; the agent listener's is the agent load balancer's range.

**Identity: sign-in, sessions and the audit chain's checkpoints (plan 18, T4, G1, G2, G26; S1, S2, S3, S9).** Users sign in with
Entra (OpenID Connect, authorization code with PKCE) and get a session cookie. The hub holds no credential of its own for it: the
Entra client secret and the key that seals the login cookie are Secret Manager secrets, named by `*_REF` variables and read at start
(a wrong name stops the start, exit 1, naming the variable). There is no variable, volume or token file for them. Every route of every
module is behind the hub's session layer and a CSRF layer (a double-submit token and an exact `Origin`) on every state-changing
method, so a module cannot omit either.

| Variable | What it is |
|---|---|
| `LK_PUBLIC_ORIGIN` | `https://<host>[:port]`, no path: the one origin a state-changing request may name in `Origin`, and the host Entra sends the browser back to. It must be the name the users type. |
| `LK_ENTRA_TENANT_ID`, `LK_ENTRA_CLIENT_ID` | The Entra tenant and the app registration (GUIDs). A token or a sign-in from another tenant is refused (S1). |
| `LK_ENTRA_CLIENT_AUTH_REF` | The Secret Manager secret that holds the app registration's client secret (Q25: the federated credential is not offered, because it would need a fifth volume). |
| `LK_ENTRA_COOKIE_KEY_REF` | The Secret Manager secret that holds 32 random bytes as base64url, which seal the short-lived login-state cookie. Rotating it only ends sign-ins that are in flight. |
| `LK_AUDIT_CHECKPOINT_BUCKET` | The Cloud Storage bucket, with a retention lock set by whoever owns the project (not by the hub), that takes one object per checkpoint of the audit chain (S9). |
| `LK_AUDIT_CHECKPOINT_KMS_REF` | The Cloud KMS key version (`EC_SIGN_P256_SHA256`) that signs each checkpoint. The hub signs with it and never holds the key. |
| `LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS` | Optional. Older versions of that key, comma separated, at most eight, whose signatures the nightly verification still accepts after a rotation. |
| `LK_BOOTSTRAP_ADMINS` | Optional. Entra object ids, comma separated, at most the library's limit, that become active admins at their first sign-in. Everyone else starts pending (S1). Remove them once real admins exist. |
| `LK_ENTRA_AUTHORITY_HOST`, `LK_ENTRA_REDIRECT_PATH`, `LK_ENTRA_API_AUDIENCES` | Optional. The authority (`login.microsoftonline.com`; a sovereign cloud names its own, and the FQDN policy follows), the path Entra returns to (`/auth/callback`) and the audiences an Entra access token for MCP may carry (`api://<client id>` and the client id). |

The sign-in cookie is `__Host-lk_session` (Secure, HttpOnly, SameSite=Lax, Path=/, no Domain), so it cannot be set for, or sent to, another
host. The hub reserves `/api`, `/hooks`, `/auth`, `/mcp` and `/.well-known` for itself: a path under them that is not a route is a `404`
and never the web app. A second cookie, `__Host-lk_csrf` (readable by the page on purpose), carries the double-submit token that the page
sends back in `X-CSRF-Token`. The users' load balancer must pass `Cookie`, `Origin` and `X-CSRF-Token` unchanged and must not cache
`/api` or `/auth`.

The Google identity the pod runs as also needs, for this group: `roles/cloudkms.signerVerifier` on the checkpoint key version (and on
each older version named for verifying), `roles/secretmanager.secretAccessor` on the two secrets above, and on the bucket
`roles/storage.objectCreator` and `roles/storage.objectViewer` (create each checkpoint and read them back for the nightly
verification) and no more: not `objectAdmin`, not `storage.objects.delete`. Three replicas run one checkpoint job and one verification
job between them (leases), so the bucket sees one writer at a time.

**The registry and the write paths (plan 18, T5, G7, G8, G9, G16; S4, S8, S9, S11, S18).** The registry (reports from agents and
sentinels, the pipeline, the read API, the blob endpoint's policy) and the write paths (edits, proposals, the token vault, pull
requests, Teams) are in the one process. They need one required variable and four Secret Manager secrets, and they add no volume, no
port and no egress host: GitHub and the Teams webhook host are already in the egress list below.

| Variable | What it is |
|---|---|
| `LK_VAULT_KMS_REF` | **Required.** The Cloud KMS *crypto key* (not a version: `projects/P/locations/L/keyRings/R/cryptoKeys/K`) that wraps the data key of each stored GitHub token (S8). A fresh AES-256-GCM data key seals each token and KMS wraps it with the user and credential ids as associated data. The hub asks KMS to wrap and unwrap and never holds the key. |
| `LK_GITHUB_API_BASE` | Optional. `https://api.github.com`, or the `https://host` of a GitHub Enterprise Server. The client reaches that host and no other (S18), so the FQDN egress policy must name it too. |
| `LK_GIT_BASE_BRANCH` | Optional. The default branch of the base repository (`main`): where a pull request for a base file goes. |
| `LK_CONFIG_SERVER_DEPLOYMENT` | Optional. The config-server's Deployment name (`csp-configuration-server`): the one service a restart for check C11 is offered for (D83). |
| `LK_TEAMS_ALLOWED_HOSTS` | Optional. The host patterns a Teams webhook URL may name, comma separated (`*.logic.azure.com`, `*.api.powerplatform.com` and `*.webhook.office.com` by default), at most 16. The URLs are Secret Manager secrets named by `notification_settings.destination_ref`; nothing in the environment holds one. |
| `LK_EXTRA_DENY_GLOBS` | Optional. Globs added to the built-in deny list (D79), comma separated, at most 256. **One list**: agents are told it, the read API withholds it, the blob endpoint refuses its hashes and the write path refuses its paths. A glob that does not compile is a refused setting. |
| `LK_RETENTION_ENABLED`, `LK_RETENTION_DRY_RUN` | Optional. `registry-retention` deletes blobs, and D116 and D118 are still to be confirmed at review, so the job is **not registered** unless `LK_RETENTION_ENABLED=true`, and then it reports and deletes nothing until `LK_RETENTION_DRY_RUN=false`. Both take `true` or `false` and nothing else. |
| `LK_CORPUS_CACHE_MIB` | Optional. What the text-search corpora may hold in memory on each replica (256). |
| `LK_WRITES_RECONCILE_SECS`, `LK_WRITES_EXPIRE_SECS`, `LK_WRITES_PR_JOBS_SECS`, `LK_WRITES_CLEANUP_SECS` | Optional. How often the holder of each write job's lease runs it: 30, 60, 5 and 3600 seconds. |

Secret Manager secrets the write paths read (names are constants of the code, not settings; each needs `roles/secretmanager.secretAccessor`):
`write-preview-key` (at least 32 bytes: signs the preview ids of an upload or a delete, D46; the hub reads it at start and does not start
without it), `github-webhook-secret` and `github-read-token` (named by `LK_GIT_WEBHOOK_REF` and `LK_GIT_READ_REF`, above), and one
secret per Teams destination, named in the `notification_settings` table by an admin. The Google identity the pod runs as also needs
`roles/cloudkms.cryptoKeyEncrypterDecrypter` on the vault key and nothing more there.

**The leased jobs.** Eighteen jobs (nineteen with retention) run on one replica at a time each, under a lease of their own name, through the platform's runner:
four of the platform's, the registry's eleven (`registry-pipeline`, `registry-nightly`, `registry-pickup-sweeper`,
`registry-structure-sweeper`, `registry-index-sweeper`, `registry-tool-write-matcher`, `registry-served-verify`,
`registry-attribution-grace`, `registry-sentinel-partitions`, `registry-sync-watch`, and `registry-retention` only when switched on) and
the write paths' four (`writes-reconcile`, `writes-expire-proposals`, `writes-pr-jobs`, `writes-credential-cleanup`). All three replicas
register all of them; each lease has one holder, and a replica that dies passes its jobs to another within one lease time. The jobs' queries
use the pool of the `lanekeeper_app` role: nothing here needs another credential. The pool is 16 connections by default; with 19 leased jobs
polling, the event listener, the session batcher and the request path, three replicas need `3 x LK_DB_MAX_CONNECTIONS` plus the migration
Job's and an operator's from the database's `max_connections`.

**The MCP server (plan 18, T6, G5, G12; S1, S3, S4, S14, S14b, S15; D52, D91, D109).** The MCP server is in the one process, on the web
listener: `/mcp` (POST, GET and DELETE), the OAuth discovery documents under `/.well-known/` and the three personal-token routes under
`/api/v1/me/mcp-tokens`. It adds no port, no volume and no egress host (Entra's host is already in the list below, for sign-in). Every
variable is optional and none is in the pod block above: the defaults are the design's.

| Variable | What it is |
|---|---|
| `LK_MCP_AUTH` | Optional. `oauth` (the default) or `personal_token`, **never both** (a list that names both is a refused setting, D52). OAuth takes Entra access tokens; personal-token mode takes the 7-day `lkpat_` tokens that a signed-in person makes in the web app. |
| `LK_MCP_CONFIRM` | Optional. `elicitation` (the default) or `draft`. Whether a write is confirmed by asking the person through MCP, or left as a draft they confirm in the web app (D52). |
| `LK_MCP_CONTENT` | Optional. `summary` (the default) or `data_block`: whether file and document text also reaches the model inside a delimited block in `content` (D91). |
| `LK_MCP_NOTIFY_SERVICES` | Optional, `true` or `false`, **off**. Turns on the one tool that reaches the config-server's `/update-resources`, which has no authentication (Q37). Leave it off until Q37 is answered. |
| `LK_MCP_AS_MIRROR` | Optional, `true` or `false`, off. Also serves a static copy of Entra's authorization-server metadata at `/.well-known/oauth-authorization-server`; turn it on only if the Cursor test (D91, `C1.mirror_needed`) says Cursor needs it. |

**What OAuth takes from the Entra settings above.** There are no MCP variables for the tenant, the audience or the origin. The resource is
the hub itself at `LK_PUBLIC_ORIGIN`: the `Host` and `Origin` of every request are checked against it, so it must be the name Cursor is
given. The audience is the first `api://...` entry of `LK_ENTRA_API_AUDIENCES` (by default `api://<LK_ENTRA_CLIENT_ID>`) and the scope a
client asks for is `<that URI>/mcp.access`; a list with no `api://` entry is a refused setting in OAuth mode. The app registration must
expose that application ID URI and the `mcp.access` scope, issue v2 access tokens (`accessTokenAcceptedVersion: 2`), and pre-authorize
Cursor's client ID for the scope (D91). The hub accepts a token only from `LK_ENTRA_TENANT_ID`, for that audience, with that scope, and
for a user who is active at that moment (S1); a pending or disabled user is a `403` on every call.

**The one secret, in personal-token mode only.** The Secret Manager secret `mcp-token-hmac-key` (the name is a constant of `hub-mcp`):
at least 32 random bytes as base64url, the key under which tokens are hashed. The hub reads it at start and **does not start** in this
mode without a usable one (exit 1, naming the secret), so a deployment cannot run with a mode that accepts no token. Every replica
reads the same secret, since any replica may be asked to check a token, and the pod's identity needs `roles/secretmanager.secretAccessor`
on it. OAuth mode reads no secret of its own.

**Sessions and the load balancer.** An MCP session lives in the memory of the replica that opened it, and a request that names a session
another replica opened is answered `404` (it fails closed). Three replicas therefore need the users' load balancer to **keep a client on
one replica by the `Mcp-Session-Id` header** (a Google backend service with `sessionAffinity: HEADER_FIELD` on that header and a
consistent-hash policy is the shape; whether the external HTTPS load balancer accepts that was not verified here), or each client
re-initialises on every `404`. `LK_MCP_CONFIRM=draft` does not remove the need. It only removes the question that is asked through
the open stream. The load balancer must also pass `Authorization` and `Mcp-Session-Id` unchanged and must not buffer or time out a
`text/event-stream` response before the hub's keep-alive (15 seconds) and idle limit (30 minutes) do; its backend timeout is the longest
a stream stays open. A hub that is told to stop ends the open streams at once.

**The document search is not there yet (G5).** `hub-docsearch` is empty until prompt 14, so the five docs tools (`search_docs`,
`read_doc`, `grep_docs`, `get_docs_for_file`, `get_feature_status`) answer a typed `unavailable` error (retryable, no data), the REST
docs operations are not routed, `/readyz` stays `200` with the body `ready (not available: docs_search)`, `lanekeeper_docs_search_available`
is `0`, and the start logs one warning. This is the one stand-in in the hub (`UnavailableDocs`; `tests/release_build.rs` holds that it is
the only one). A probe that gates on the status is unaffected; an alert on the gauge is the way to notice.

**Request bodies (D137, S11).** 4 MiB for every route, 1 MiB for the GitHub webhook, and 12 MiB + 64 KiB for `PUT /api/v1/swimlanes/{id}/files`
and `POST /api/v1/drafts` from a request that carries a live session of at least an Editor. An unauthenticated request, a pending user and a
Viewer have 4 MiB on those two routes too. The users' load balancer must pass a request body of that size and its own timeouts must not be
shorter than `LK_REQUEST_TIMEOUT_SECS` (30).

### Making a CA certificate (`hub ca-init`)

Nothing can start without the CA certificates and nothing else makes them, so `hub ca-init` is the first step of a new environment
(plan 18, T3, G15). A person runs it once for each CA, with an identity that may read the key's public half and sign with it
(`roles/cloudkms.publicKeyViewer` and `roles/cloudkms.signerVerifier` on that key version), from the same image:

```
hub ca-init --kms-ref projects/P/locations/L/keyRings/R/cryptoKeys/agent-ca/cryptoKeyVersions/1 \
            --common-name "Lanekeeper agent CA" --valid-days 3650 > agent-ca.pem
```

It reads the key's public key, has KMS sign a self-signed certificate that may sign leaf certificates and no other CA (path length 0),
and prints the certificate (PEM, public) on standard output and nothing else there. It holds no secret, writes no file and changes
nothing in Google; the person stores the output in the Secret Manager secret that `LK_AGENT_CA_CERT_REF` (or `LK_REPLICA_CA_CERT_REF`)
names. Exit 0: the certificate is on standard output. Exit 1: KMS refused or was unreachable (the message names the step). Exit 2: an
option is missing or not valid (the message names the option and never repeats its value). Use a key version that nothing else uses,
and make the two CAs from two different keys: a hub that is given one key for both would start, and would accept an agent's
certificate as a replica's.

**Starting and stopping.** A setting that is missing or malformed is exit 2 and nothing starts; anything that cannot be reached while
starting is exit 1, because the hub never falls back to a fake. Each step of starting is bounded by `LK_START_TIMEOUT_SECS` (8), and
the log line names the component and the variable, never a value. On SIGTERM or SIGINT the hub fails `/readyz` at once, waits
`LK_SHUTDOWN_GRACE_SECS` (5) so that the load balancer notices, stops accepting, gives requests in flight `LK_SHUTDOWN_DRAIN_SECS`
(25), and exits 0; a second signal exits at once. The pod's `terminationGracePeriodSeconds` must be longer than the two together, or
the kubelet kills a hub that is still draining; `tests/hardening.rs` holds the Pod block to grace + drain + 10 seconds.

### The migration Job

The schema is changed by a Job and not by the hub (plan 18, T2, G4). The hub runs as `lanekeeper_app`, which reads and writes rows and
cannot change a table (S9, S21). Migrating at start would put a credential that can change the schema into a pod that faces users,
and would have three replicas race for it. So `hub migrate` runs first, as a Job from the same image (`args: ["migrate"]`), and the
hub only checks, at start and in `/readyz` (the check is named `schema`), that the migrations it needs are there. A hub on an
unmigrated or older schema exits 1 and names the first migration that is missing; it never migrates for itself.

| | The Job |
|---|---|
| Variables | `LK_MIGRATOR_URL_FILE` (the path of the mounted Secret that holds the `lanekeeper_migrator` URL), `LK_DB_CA_FILE` (the same CA file the hub uses), and optionally `LK_MIGRATE_TIMEOUT_SECS` (600 by default) |
| Volumes | the migrator's Secret (mode `0440`) and the database CA, both read-only; `/tmp` as the only writable mount |
| Network | Postgres on 5432 and DNS; nothing else, no ingress, no ports |
| Security context | the hub pod's, unchanged: non-root, read-only root, no capabilities, no service-account token |
| Exit codes | 0 the schema is complete (whether or not this run changed anything), 1 it could not be completed, 2 a setting was refused |
| Order | before the hub rolls (a Helm pre-install and pre-upgrade hook, or an Argo CD `PreSync` hook); safe to retry, and safe to run twice at once |

The Job's variables are the Job's alone. `tests/hardening.rs` fails if the hub pod carries `LK_MIGRATOR_URL_FILE` or
`LK_MIGRATE_TIMEOUT_SECS`, and its volume allowlist already refuses a pod that mounts a fifth volume, so the migrator's credential
cannot be in a serving pod. How to run the command, and what it refuses, is in `README.md`.

### The seed Job

The rows that say which Google service account may join as which swimlane's agent, or as which NFS VM's sentinel, are written by
`hub seed` and by nothing else (plan 18, T7, G24). It is a Job (or a command run by a person) from the same image, like the
migration Job, and it holds **the hub's own database credential**: it runs as `lanekeeper_app` and refuses every other role, before it
connects and again once connected, so it can do what the serving hub can do to those four tables and nothing more.

A swimlane whose row has no `agent_service_account` refuses every `Join`, the right token included, and so does a sentinel without a
`service_account`: until the Job has run with a file that carries the account of each swimlane and sentinel that should have an
agent, nothing can join. The file must carry them.

| | The Job |
|---|---|
| Variables | `LK_DATABASE_URL_FILE` (the path of the mounted Secret that holds the `lanekeeper_app` URL, the same Secret the hub mounts), `LK_DB_CA_FILE` (the same CA file the hub uses), and optionally `LK_SEED_TIMEOUT_SECS` (120 by default) |
| Arguments | `["seed", "/etc/lanekeeper/seed/seed.yaml"]`, and `--dry-run` for a preview |
| Volumes | the application role's Secret (mode `0440`) and the database CA, both read-only; the seed file, read-only, from a `ConfigMap` that holds it (it names service accounts and holds no secret); `/tmp` as the only writable mount |
| Network | Postgres on 5432 and DNS; nothing else, no ingress, no ports |
| Security context | the hub pod's, unchanged: non-root, read-only root, no capabilities, no service-account token |
| Exit codes | 0 the database matches the file (or, for `--dry-run`, whatever would change), 1 the run could not be completed, 2 the file or a setting was refused |
| Order | after the migration Job; the hub starts and serves with no swimlanes, so the rollout need not wait for it (it is what lets agents join, and a missing seed must not stop a deploy); safe to retry, and safe to run twice at once |

**What the Job must never be given.** The migrator's credential (it refuses it), a variable of the hub other than the two database ones,
a mount other than the four above. `LK_SEED_TIMEOUT_SECS` is the Job's alone: `tests/hardening.rs` fails if the hub pod carries it.

**Why the file is a reviewed change.** Whoever can apply a seed file can make a service account the agent of a swimlane: the file is
a change of who may join, and is reviewed as one. It is validated (a service account must be a `*.gserviceaccount.com` address, in
lower case: a person's address is refused), it never deletes, `--dry-run` shows the exact change first, and every changed row is one
audit event with the SHA-256 of the file. The residual risk: the operator who ran the Job is not in the event (the actor is the
component), so the hash and the Job's own record (who started it, from which `ConfigMap`) are how a change is traced to a person.

### Where the hub writes

The root filesystem is read-only. Two mounts are writable, and the code writes nowhere else:

| Mount | Variable | What the hub writes there |
|---|---|---|
| PersistentVolumeClaim | `LK_MIRROR_DIR` | The bare Git mirrors, one per repository (T7). Each replica has its own volume; a lost volume is a full re-fetch. |
| `emptyDir` | none (`/tmp`) | Temporary files of the libraries. Capped at 256 MiB. |

The database credentials and the database CA are read-only Secret mounts (the first with mode `0440`, `defaultMode: 288`).
The CA certificates of the agent and the replica listeners are public and arrive from Secret Manager (`LK_AGENT_CA_CERT_REF`,
`LK_REPLICA_CA_CERT_REF`).

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
| `storage.googleapis.com` | 443/TCP | Cloud Storage: the audit-checkpoint writer puts each checkpoint in the locked bucket (`GcsCheckpointBucket::HOST`, S9). |
| the Teams webhook host | 443/TCP | Notifications (Q9: the host depends on the answer). |

In: the web port from the users' load balancer, whose proxies come from Google's two ranges (the client address is checked by
Cloud Armor, not here); the agent port from the agent load balancer's source ranges, which are the same list as the load
balancer's own allowlist; the replica port from the hub's own pods; and the operations port from the `monitoring` namespace and
from Google's two ranges, because the users' load balancer health-checks `/readyz` on it (plan 09 A10) and no other range may.
The kubelet's probes come from the node. The load balancers' own source-IP allowlists, and the GitHub hook ranges on the
webhook path, are the chart's (S18, Q28).

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
    - ipBlock: {cidr: 35.191.0.0/16}
    - ipBlock: {cidr: 130.211.0.0/22}
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
  - from:
    - ipBlock: {cidr: 35.191.0.0/16}
    - ipBlock: {cidr: 130.211.0.0/22}
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
    - pattern: storage.googleapis.com
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
- **Everything that needs Google, Entra or GitHub.** The tests run with software at the edges. That the pod's identity holds exactly the
  roles named above, that Secret Manager and KMS answer as the clients expect, that Entra's tokens carry the claims the hub checks, and
  that GitHub's API behaves as the faked one does, are shown by prompt 09's kind test and the first deployment (plan 18, T8).
- **The load balancers.** The requirements in `README.md` ("The users' load balancer") are statements about a product this repository
  does not run: header-based affinity for MCP sessions, 12 MiB bodies, unbuffered event streams, and the number of proxies behind
  `LK_TRUSTED_PROXY_HOPS`. Each is a residual risk in `docs/threat-model.md` until someone has tested it on the real path.
