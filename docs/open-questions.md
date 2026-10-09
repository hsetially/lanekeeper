# Open questions

These are facts nobody on the team knows yet. Each open question says who can answer it, which prompts it blocks, and what to assume until it's answered. When one gets answered, move it to "Answered" at the bottom, update docs/decisions.md if it changes a decision, and tell the owners of the blocked prompts.

**Resolve before the pilot goes live:** Q1, Q2, Q11, Q17, Q24 and Q26.

## Open

### Q1. Outbound connections from pods

Bastion VMs returned 200. Pods can take a different route out, so run this once from a pod in one cluster per project:

`kubectl run egress-test --rm -it --image=curlimages/curl --restart=Never -- curl -sS -m 5 https://api.ipify.org`

The command prints the cluster's outbound IP, which the agent load balancer's allowlist needs.

- **Who:** network team.
- **Blocks:** 02, 09.

### Q2. Address ranges for the two entry points

The external HTTPS endpoint is decided (D25). The network team still needs to supply:

- the GlobalProtect egress ranges, for the user entry point;
- the clusters' outbound IPs, from Q1, for the agent entry point;
- DNS names and TLS certificates.

- **Blocks:** 09.

### Q3. Sync Job details

Known: it's a Kubernetes Job in the swimlane cluster, and it replaces whole files. Still to find out:

- whether it deletes files that are no longer in Git;
- the order it copies in;
- whether the `-<branch>` rename also applies to images and XSL files;
- the exact NFS root path;
- what triggers it.

Find it with:

`kubectl get jobs -A | grep -i -E 'tenant|config|sync'`

- **Blocks:** 05, 10.
- **Until answered:** the rename applies to all files, no deletes happen, and path mappings are configurable.

### Q8. Pulling GHCR images into private clusters

Pulling private GHCR images usually needs a classic token with package read access, and that may conflict with a fine-grained-only policy. Choose one:

- **(a)** an image pull secret in every cluster, using a machine account's token;
- **(b)** an Artifact Registry remote repository in front of GHCR, so clusters pull with their Google identity.

- **Blocks:** 09.
- **Until answered:** (a), with the secret name in values.

### Q9. Teams notifications

Are Workflows webhooks allowed in your Microsoft 365 tenant? Which channels should get access requests, approvals and drift alerts?

- **Who:** M365 admin.
- **Blocks:** 06.
- **Until answered:** a notifier interface with a no-op default, plus webhook URLs configured by an admin.

### Q10. ArgoCD and the restart annotation

Test this once, in a SIT cluster with self-heal on:

```
kubectl -n <ns> rollout restart deployment/<name>
kubectl -n <ns> rollout status deployment/<name>
# wait about 3 minutes, then:
kubectl -n argocd get application <app> -o jsonpath='{.status.sync.status}{"\n"}'
kubectl -n <ns> rollout history deployment/<name>
```

- **Pass:** the app stays `Synced` and the history shows exactly one new revision.
- **Fail:** the app shows `OutOfSync`, or a second rollout appears.
- **Fix if it fails:** add `ignoreDifferences` for `/spec/template/metadata/annotations/kubectl.kubernetes.io~1restartedAt`, with `RespectIgnoreDifferences=true`.

- **Blocks:** 02, 06.

#### Q11. NFS mount details per cluster

You'll configure this later. It must be done before the agent writes on a pilot swimlane. Find:

- the server address, export path and mount path;
- the uid and gid that own the files;
- the export options in `/etc/exports` on the NFS VM: `root_squash` and `anonuid` decide which uid the agent must run as;
- the NFS version.

- **Blocks:** 02, 09.

### Q17. Read-only Git credential for background reads

Drift detection, the comparison grid, history and docs sync all read both repos continuously. That needs a read-only service credential, separate from users' tokens. It could be a read-only GitHub App or a service account's fine-grained token. Without it, drift detection doesn't work.

- **Who:** GitHub org admins and security.
- **Blocks:** 03b, 05, 14.
- **Until answered:** a token from a Kubernetes Secret, behind a `GitCredentials` trait.

### Q19. Is tenant data deployed as a Helm release?

The docs say the tenant repo's `pom.xml` artifactId becomes a Helm chart `csp-tenant-data-<branch>`. Check with:

```
helm list -A | grep -i tenant-data
kubectl get applications -A | grep -i tenant
```

If it is, the agent can report the deployed branch straight from the cluster.

- **Blocks:** 02, 05.
- **Until answered:** the agent reports Helm release names that match a configurable pattern, as hints only.

### Q20. Review six files flagged by the secret scan

The scan checked paths and value lengths only, not the values themselves. These six files have long, random-looking values:

- in base:
  - `security/security-issuers.yml`
  - `security/security-issuers-ext.yml`
  - `device-authz-service/device-authz-service.properties` (one value is 402 characters long)
  - `deferred-record-processor/deposit-notification-response-config.yml`
- in sit1:
  - `core-adapter-symxchange/tenant-adapter-config.yaml`
  - `tx-infinity-api/remote-itm-teller/miscCreditDebitAndGLItemsConfig.yml`

They're probably public keys or IDs, but confirm before the AI reads every file.

- **Blocks:** go-live of 07.

### Q21. Entra app registrations

Someone with Entra admin rights creates three app registrations:

- **Web app:** a confidential client with redirect `https://<hub>/auth/callback`.
- **Hub API:** exposes the scope `mcp.access`, with v2 access tokens.
- **Cursor:** a public client with redirect `http://localhost:8787/callback`, and delegated permission to the hub API scope.

Also decide whether to set "assignment required" to limit who can even request access.

- **Who:** Entra admins.
- **Blocks:** 03a, 07.

### Q22. Template branches

Are `sitN` branches created from a Core template branch (for example `templates/symx-template`)? If they are, a drift-from-template check is worth building in v2.

- **Blocks:** nothing in v1.

### Q23. Docs from Git

Docs are indexed automatically from `templates/common-docs` (default: on). Should the base repo's `doc/` folder be indexed as well? The docs link to it.

- **Blocks:** 14.
- **Until answered:** `templates/common-docs` and uploads only.

### Q24. GKE Binary Authorization

Is Binary Authorization available on the platform cluster and the swimlane clusters? It's needed to enforce that only signed images run (S20).

- **Who:** platform team.
- **Blocks:** 09.
- **Until answered:** images are signed and verified in CI, without enforcement in the clusters.

### Q25. Entra federated credential for the hub

Can the hub's Entra app registration trust the GKE cluster's Workload Identity issuer? That would let the hub sign in to Entra with no client secret at all.

- **Who:** Entra admins.
- **Blocks:** 03a.
- **Until answered:** a client secret stored in Secret Manager.

### Q26. Workload Identity on swimlane clusters

Is Workload Identity enabled on every swimlane cluster, and can each agent get its own Google service account? This is needed for agent attestation (S5).

- **Who:** platform team.
- **Blocks:** 02, 03b.
- **Until answered:** one-time join tokens, as a fallback per cluster.

### Q27. Design scale

Confirm the design scale in `docs/performance.md`: 40 swimlanes, 2,000 files each, 100 web users and 30 MCP sessions.

- **Blocks:** 16.

### Q28. GitHub webhooks

Can GitHub reach the hub's `/hooks/github` path? That means allowing GitHub's published hook IP ranges on the user-facing load balancer, for that path only.

- **Who:** network and security teams.
- **Blocks:** 03b, 09.
- **Until answered:** polling every 60 seconds.

### Q29. CMEK keys

Customer-managed encryption keys for the GCS buckets (docs, audit checkpoints) and the persistent disks. Which KMS key ring should be used?

- **Blocks:** 09.

### Q30. Installing the sentinel on NFS VMs

Is it acceptable to install a signed RPM (the sentinel), an auditd watch rule on the export root, and the audisp af_unix plugin on the Rocky Linux NFS VMs? Which configuration-management tool should install them?

- **Who:** platform and security teams.
- **Blocks:** 17.

### Q32. Mapping OS Login identities to app users

Are the Google identities used by OS Login the same email addresses as the Entra user names? If they are, the hub maps them automatically. If not, admins maintain a mapping table.

- **Blocks:** 17, 05.
- **Until answered:** automatic matching by email, plus an admin mapping table.

### Q33. Retention periods

Confirm the defaults: 180 days of versions, at least the last 50 per file, and audit events kept indefinitely.

- **Blocks:** 05.

### Q34. More than one tenant per swimlane?

Can a swimlane host more than one tenant (more than one `csp-tenant-data-<branch>` release) at the same time? The model supports a set of tenants, but checks C3 and C4 behave differently when there's more than one.

- **Who:** DevOps.
- **Blocks:** 05.

### Q35. Do services subscribe to change notifications?

Is ActiveMQ deployed in the swimlanes? Do services run tx-config-client with `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED` on? The agent detects the setting per Deployment; this confirms the intent.

- **Who:** service owners.
- **Blocks:** 06 (notify), 08.

### Q36. Does the dataload Job call the update endpoint?

Does the configuration-dataload (sync) Job already call `/update-resources` after copying?

- **Who:** DevOps.
- **Blocks:** 10.

### Q37. Securing the config-server's update endpoint

`/update-resources` has no authentication, and the chart's NetworkPolicy is off by default. Will the config-server owners enable a NetworkPolicy that allows POSTs only from the dataload Job and the Lanekeeper agent, and approve Lanekeeper calling the endpoint?

- **Who:** config-server owners and security.
- **Blocks:** 06 (notify).

## Answered

- **Q4. How files are combined:** the config-server is Spring Cloud Config with a native NFS backend. Property sources are merged setting by setting. Resource files are chosen whole (tenant file, then channel folder) and their placeholders are substituted from the property view (D82–D83). Q11 is partly answered too: the NFS path is `{nfsMount}/{cluster}/{namespace}/csp-configuration`, and uid/gid is 1010.

- **Q31. OS Login:** it is on for the NFS VMs, so sentinel attribution resolves to Google identities (D72).

- **Q5. Cursor version:** 3.22.12. The approach is in D50 to D52, and prompt 07 starts with a test of sign-in and elicitation.
- **Q6 and Q7. Sign-in:** Okta is replaced by Entra ID, with roles managed in the app (D34 to D36). The GitHub token is requested at first sign-in after approval (D42).
- **Q12. Token type:** fine-grained tokens.
- **Q13. JSON and `.properties`:** they follow the `-<branch>` naming when loaded from tenant data.
- **Q14. Decision model:** it runs locally under Ollama, and moves to v2. Before then, check that Ollama can run its custom decision head. The model returns probabilities through its own code, not generated text.
- **Q15. Defaults:** all confirmed (D40).
- **Q16. GLiNER:** moved to v2.
- **Q18. Design system:** none exists, so design for speed and ease of use (D31).
- **New users:** no access until an admin approves (D36).
- **Docs:** search ships in v1. Editors and above upload. Every approved user can search everything (D53).
- **Scope:** only `data/config` in v1 (D5).
