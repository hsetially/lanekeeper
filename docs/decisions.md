# Decisions

The team made every decision below explicitly during product design. Agents: treat them as fixed. If one blocks you, raise it rather than working around it. Record new decisions at the end, with a date.

## Product and scope

- **D1** Working name is Lanekeeper.
- **D2** v1 ships reads, writes and docs search together. Editing on a swimlane stays locked until an admin confirms that swimlane's baseline in the adoption pass.
- **D3** The tool does not copy Git to NFS in v1. The sync Job remains the only copier.
- **D4** Agents roll out to one or two SIT swimlanes first (the pilot), then to the rest.
- **D5** v1 covers only the NFS config directory, which holds `data/config` from the tenant repo and `config/` from the base repo.
- **D6** These are v2:
  - GLiNER reference map;
  - the decision model;
  - swimlane claims;
  - base-tag creation;
  - drift-label export;
  - release snapshots;
  - the drift-from-template check;
  - `data/atm-config` and the `data/*.json` files.

## Environment facts

- **D7** A swimlane is exactly one GKE cluster (for example `sitb`). Each cluster has:
  - its own NFS VM (a GCE instance running Rocky Linux 8.9 or 8.10);
  - its own config-server;
  - its own ArgoCD.
- **D8** Clusters span several GCP projects. Bastion VMs can reach the internet; reaching it from pods is still to be confirmed (Q1).
- **D9** Microservices don't mount NFS. The config-server in each cluster reads NFS and serves files to microservices.
- **D10** Microservices only pick up config changes when they restart.
- **D11** Sync Job:
  - it runs as a Kubernetes Job in the same cluster as the config-server;
  - it copies whole files from Git to NFS, replacing what's there without comparing.
- **D12** Repos:
  - `configuration-base-saas` has one branch, with tags marking versions. Config lives under `config/`, and docs live under `doc/`.
  - `csp-tenant-data` has branches `sit1` to `sitN` (growing), template branches under `templates/*` (one per Core, for example `templates/symx-template`), and `templates/common-docs` holding the shared docs. Tenant config lives under `data/config/`.
- **D13** File naming:
  - In Git, a tenant file has the same relative path as its base file.
  - On NFS, a tenant file sits beside the base file with the branch name as a suffix: `tx-infinity-core.yml` (base) and `tx-infinity-core-sit1.yml` (tenant, from branch `sit1`).
  - This applies to JSON and `.properties` files loaded from tenant data too.
- **D14** A service uses the tenant file for its deployed branch if it exists, otherwise the base file. This is the default merge mode, `whole_file`. Merge mode is configurable per file pattern, with `spring_merge` as the alternative, until the config-server code has been reviewed (Q4).
- **D15** Folder layout:
  - Folders usually, but not always, match the microservice name.
  - Folders can contain channel subfolders (`remote-itm-teller`) and nested service folders.
  - Root files (`application.properties`, `channels.yml`) are shared by every service.
- **D16** Every file under the config root is tracked:
  - YAML, JSON and `.properties` files are parsed;
  - XSL, text and files with no extension are diffed as text;
  - images and other binaries are compared by hash, with a preview.
- **D17** Files are mostly UTF-8 with CRLF line endings. Writes preserve each file's line endings, encoding and any BOM.
- **D18** Config files contain no secrets. Credentials are injected through environment placeholders such as `${TXSECURITY_KEYSTORE_PASSWORD_FILE}`. A value-based scan flagged six files for one human review (Q20).

## Source of truth and drift

- **D19** Git and NFS are both treated as truth. Drift is detected against a stored baseline: the Git commit and file hash at the last known sync.
- **D20** File states:
  - `in_sync`, `git_ahead`, `nfs_ahead`, `conflict` and `intentional_divergence`;
  - plus `unknown` (no baseline yet) and `untracked` (no Git source).
- **D21** Sync reporting is undecided. The hub processes a sync report the same way whatever its source, and prompt 10 investigates the options. Until then:
  - baselines come from the adoption pass;
  - files that end up equal to Git correct themselves;
  - a change in a swimlane's tenant suffix triggers branch re-matching, which an admin confirms.
- **D22** A one-time adoption pass per swimlane proposes the deployed branch (by similarity) and a baseline. An admin confirms them. Template branches (`templates/*`) are never treated as deployable branches.

## Architecture

- **D23** The central hub runs in the existing platform GKE cluster. There is one agent per swimlane cluster:
  - it runs as its own Deployment;
  - it mounts the same NFS export the config-server uses.
- **D24** Agents connect outbound to the hub over a gRPC bidirectional stream with mutual TLS. If egress has to go through an HTTP proxy, the fallback transport is WebSocket over HTTPS.
- **D25** The hub is exposed through an external HTTPS endpoint with two entry points:
  - **agents:** a TCP pass-through load balancer that only accepts the clusters' outbound IPs; the hub handles mutual TLS itself;
  - **users (browser and Cursor):** an HTTPS load balancer that only accepts GlobalProtect VPN address ranges.
- **D26** Each cluster's own ArgoCD deploys its agent from one shared Helm chart.
- **D27** Drift is detected by periodic hash scans, because inotify doesn't work across NFS mounts.
- **D28** For restarts, the agent patches the Deployment's restart annotation through its own cluster's Kubernetes API, with RBAC scoped to the config namespaces.
- **D29** Storage:
  - Postgres (with pgvector) runs in the platform cluster under the CloudNativePG operator, with backups to GCS;
  - uploaded doc originals are stored in a GCS bucket;
  - audit events are also copied to Cloud Logging.

## Stack

- **D30** Backend in Rust: Axum on Tokio, tonic, sqlx, octocrab, kube-rs and rmcp.
- **D31** Frontend: a TypeScript and React single-page app on Vite, with TanStack Query and Router, Monaco, shadcn/ui and Tailwind. There is no existing design system, so design for speed:
  - dense tables;
  - a Cmd+K command palette;
  - keyboard shortcuts in the diff view;
  - a URL for every view.
- **D32** One monorepo. GitHub Actions builds the images and pushes them to GHCR.
- **D33** Diffs, merge modes, drift, effective config and consistency checks are computed by deterministic Rust code in `crates/engine`. AI never computes them.

## Identity, permissions and audit

- **D34** Sign-in is with Microsoft Entra ID (OIDC, single tenant), handled by the hub (backend-for-frontend). The browser holds only a session cookie. Users are identified by Entra tenant id plus object id, never by email.
- **D35** Roles are managed in the app's own database, through an admin screen. They are global and cumulative: Viewer < Editor < Operator < Admin. A per-user `requires_approval` flag also lives in the app. The first admins come from configuration.
- **D36** On first sign-in, a user gets no access. An access request is created, and an admin approves it with a role. Pending and disabled users get 403 everywhere, including MCP.
- **D37** For users with `requires_approval`, NFS edits, uploads, deletes, restarts, PRs and AI-made changes become proposals.
  - Only admins approve.
  - Nobody can approve their own proposal.
  - At approval, the file hash is re-checked. If the file changed, the proposal is stale.
  - Proposals expire after 72 hours.
- **D38** Every state-changing action writes an audit event. That includes role changes, access approvals and doc changes. Each event records:
  - user and GitHub login (for PRs);
  - action, swimlane and file;
  - hash before and after, and the diff;
  - via (`ui`, `mcp`, `sync` or `agent_detected`);
  - approval id and timestamp.
  Reads are not audited.
- **D39** NFS changes made outside the tool are recorded with author `unknown`.
- **D40** Defaults:
  - sessions: 8 hours idle, 12 hours absolute;
  - token cleanup: after 30 days without sign-in;
  - backup retention: 30 days.

## GitHub

- **D41** PRs use the user's own fine-grained GitHub token, scoped to the two repos.
- **D42** Editors and above are asked for the token at their first sign-in after approval. It is:
  - checked with GitHub;
  - linked to the user's GitHub login;
  - saved encrypted with Cloud KMS envelope encryption;
  - used only inside that user's own authenticated request;
  - never logged or sent to AI.
- **D43** The PR dialog makes the user choose explicitly:
  - change the base file, which affects all swimlanes; or
  - change the tenant file on a branch, which affects only that branch and forks the file if it's new.
  PR paths are reverse-mapped from NFS names to repo paths. For example, the NFS file `x-sit1.yml` becomes `data/config/.../x.yml` on branch `sit1`.

## Write paths

- **D44** Every NFS write carries the expected current hash. A mismatch is a conflict, never an overwrite.
- **D45** Validation on save:
  - each format's parser checks the file; duplicate keys produce a warning, not a block;
  - a value-based secret check blocks the save on a match.
- **D46** Upload and delete show their effect before the user confirms. For example, deleting a tenant file makes the service fall back to the base file.
- **D47** Every file version is stored, keyed by hash, including versions caught by scans. Revert and restore are new audited edits.
- **D48** Each service shows a pending-restart state. Operators get a restart button. A change to a root file marks every service as pending.
- **D49** Notifications go to Microsoft Teams (mechanism per Q9): approvals, drift alerts and access requests.

## AI and docs

- **D50** AI lives only in Cursor 3.22.12, which is already shipped org-wide.
  - Cursor connects to an MCP server inside the hub.
  - Sign-in is Entra OAuth, with a fixed client ID in `mcp.json`.
  - The server and the skill are distributed through the Cursor team marketplace.
  - The MCP tool allowlist lets only read tools run automatically.
- **D51** Cursor's model does all reasoning and writing. There is no server-side LLM.
- **D52** AI-made changes are confirmed through MCP elicitation, enforced by the server. If elicitation doesn't work in practice, they become drafts applied in the web UI. Personal-token auth is a separate mode, never combined with OAuth.
- **D53** Docs search ships in v1:
  - **Sources:** Markdown from the tenant repo's `templates/common-docs` branch, indexed automatically, plus uploads in the web UI.
  - **Who can do what:** Editors and above upload, replace and delete. Every approved user can search every doc.
  - **Search:** hybrid, combining Postgres full-text search with pgvector. A small embedding model runs on CPU in the hub.
  - **Feature status:** Feature Flags tables in the docs feed a view of each flag's value per swimlane.

## Libraries, search and audit (added after the library review)

- **D54** Support libraries:
  - rayon for CPU-bound batch work, always off the async executor;
  - ripgrep's `ignore` crate for the agent's directory walk;
  - `globset` for every path-pattern rule;
  - `grep-regex` and `grep-searcher` for text search;
  - tree-sitter with its YAML grammar, for mapping setting paths to line ranges.

  Reviewed and not adopted, because the existing design already covers what they'd offer: iii, iroh, turbovec, loro, watchexec, polars, risingwave, BAML, rivet, direct use of crossbeam, wgpu, webrtc-rs, lapce, delta, and Buzz as a product.
- **D55** Text search ships in v1. It is a literal or regex search over the current contents of files in the `structured` and `text` classes, across the swimlanes a user selects. It's available in the UI and through the MCP tool `search_text`. Results are capped.
- **D56** The audit log is hash-chained:
  - each event stores the previous event's hash and its own;
  - appends are serialised;
  - a nightly job verifies the chain and alerts if it's broken.
- **D57** Two docs-filesystem MCP tools, `read_doc` and `grep_docs`, give read-only, exact-match access to the indexed docs, alongside `search_docs`.
- **D58** MCP output is compact:
  - results are capped, and return a summary plus a cursor or web UI link for the rest;
  - `read_file` accepts a line range or a setting path.
- **D59** Dev tooling: Vibe Kanban is suggested for running the prompts in parallel, and rtk is optional for engineers. Neither is a product dependency.

## Agent-built, performance-first and security-first revision

- **D60** AI agents build the project, and humans review and merge.
  - Prompts follow one fixed template.
  - Every prompt has a `just verify-NN` gate.
  - A reviewer agent (`prompts/REVIEW.md`) checks every PR before a human sees it.
  - Agents write a plan to `plans/` and wait for approval before coding.
- **D61** Priorities are security, then performance, then efficient features, ahead of implementation simplicity. This decision supersedes earlier simplifications:

| Earlier simplification | Replaced by |
|---|---|
| One hub replica | Three replicas, with routing between them (D62) |
| Manual join tokens | Workload Identity attestation, with tokens only as a fallback (S5) |
| A CA key held in a Kubernetes Secret | A Cloud KMS signing key (S5) |
| Kubernetes Secrets | Secret Manager (S7) |
| A 60-second scan | A 10-second stat walk with Merkle roots (D63) |
| Git polling every 2 minutes | GitHub webhooks, with 60-second polling as a fallback (D64) |
| Audit hash chain | Hash chain plus hourly KMS-signed checkpoints written to a locked GCS bucket (S9) |

- **D62** The hub runs as a StatefulSet of 3 replicas, with anti-affinity and a PodDisruptionBudget.
  - Each agent stream is registered in Postgres.
  - Commands are routed to the replica that owns the stream, over internal gRPC with mutual TLS.
  - Singleton jobs use Postgres leases.
  - Events fan out across replicas through Postgres LISTEN/NOTIFY.
- **D63** Agents keep a Merkle tree of the NFS root.
  - Heartbeats carry the root hash.
  - The hub asks for a delta only when the root changes.
  - The stat walk runs every 10 seconds, with a full rehash every 15 minutes.
- **D64** GitHub push webhooks trigger fetches, verified by HMAC and with replay protection. Polling every 60 seconds is the fallback.
- **D65** Indexes for speed:
  - a settings index, built once per unique blob;
  - a Git tree index per commit;
  - effective tree hashes per directory;
  - an event-driven incremental recompute pipeline.
- **D66** Live updates reach the web UI through Server-Sent Events.
- **D67** Whole-swimlane compare: two swimlanes, or a swimlane against a Git ref, compared at directory level. Identical subtrees are skipped using effective tree hashes.
- **D68** Write requests accept an `Idempotency-Key` header, so a retry never applies a change twice.
- **D69** Supply chain:
  - SBOMs, cosign signatures and provenance attestations;
  - GKE Binary Authorization;
  - Renovate with a 14-day minimum release age;
  - GitHub Actions pinned by commit SHA.
- **D70** The security baseline (`docs/security.md`, S1–S24) and the performance budgets (`docs/performance.md`, P1–P13) are mandatory. They are verified by prompts 15 and 16, and by every prompt's own gate.

## Design

- **D71** The UI is designed in Claude Design before it's built.
  - **How:** a published Lanekeeper design system, then every screen and state, then clickable prototypes for the 7 key journeys, using design prompts 00–14.
  - **Handoff:** the bundle is committed to `design/handoff/`.
  - **Implementation:** prompt 08 implements it to visual parity, checked by Playwright screenshot and keyboard-map tests.
  - **Conflicts:** where a design conflicts with S12 (CSP and Trusted Types) or a P10 budget, security and performance win. Each such case is recorded in `design/DEVIATIONS.md`.
  - **Keeping in sync:** after implementation, `/design-sync` keeps Claude Design aligned with the coded components.

## Patterns adopted from the config-watcher reference project

- **D72** An **NFS VM sentinel** runs on every Rocky Linux NFS VM (prompt 17).
  - It is a small Rust daemon that consumes auditd events for the export root, delivered through the audisp af_unix plugin.
  - OS Login is on, so the original login identity of an edit (kept by auditd even through `sudo`) maps to a real Google identity. Edits made through IAP get named authors.
  - Writes that arrive over NFS produce no local audit record, so they're distinguishable from local edits.
  - The sentinel never reads or sends file content.
  - It proves its identity with the VM's service-account ID token (S5) and connects outbound only, over mutual TLS.
- **D73** Every observed change carries an **attribution with a source and a confidence level**:

| Source | Confidence |
|---|---|
| `tool_write` | certain |
| `sentinel_login` | high |
| `sync_job` | high |
| `nfs_client` (an NFS write while no sync Job was running) | medium |
| `fs_owner_hint` | low |
| `unknown` | none |

  The UI, the audit log and MCP show both the source and the confidence. This supersedes D35, which recorded every out-of-band change as `unknown`.
- **D74** The agent keeps a **durable spool**.
  - A small per-agent volume holds every observed version until the hub acknowledges it, so intermediate versions survive hub outages.
  - The spool is bounded. If it overflows, the agent reports a gap.
- **D75** **Quiescence and sync awareness.**
  - The agent reports a change only after the tree has been quiet for 3 seconds, deferring at most 30 seconds.
  - Changes observed while a sync Job is running are tagged with that Job.
  - The hub holds drift alerts until the Job finishes, then evaluates once.
- **D76** Deterministic **severity rules** (low, medium, high or critical) apply to every change and finding. They are admin-configurable, and they set the priority of Teams alerts and the order of lists.
- **D77** **PR linkage.**
  - Every PR is linked to the swimlane, path and NFS hash it came from. Its state (open, merged or closed) is tracked through GitHub `pull_request` webhooks.
  - A second PR for a change that already has an open PR is blocked.
  - PR creation runs as a persisted background job with states, and its progress is pushed over SSE.
- **D78** **Version retention.**
  - Always kept: baselines, and every version referenced by an audit event, proposal, draft or PR link.
  - Also kept: the last 50 versions per file, and every version from the last 180 days (Q33).
  - Unreferenced blobs are garbage-collected by a leased job.
  - Audit events keep their own diff text, so evidence survives garbage collection.
- **D79** **Sensitive files and output protection.**
  - The agent has deny globs (`*.jks`, `*.p12`, `*.pfx`, `*.pem`, `*.key`, `*.keystore`, `*private*`). Matching files are reported by name, size and hash only. Their content is never read beyond hashing, never sent, and never written.
  - MCP output redacts values the secret scan flags.
  - The web UI masks flagged values by default. Revealing one is an audited action, allowed for Editors and above.
- **D80** Events are published through a **transactional outbox**. An event is written in the same transaction as its change and fanned out only after commit. Realtime updates come after durability.
- **D81** **Word-level highlighting.** Diff hunks include the changed character ranges within each line.

## How the config-server actually resolves files (from the csp-configuration-server repo)

- **D82** Q4 is answered. The config-server is Spring Cloud Config Server.
  - **Backend:** the native filesystem backend reading NFS. On the NFS server the path is `{nfsMount}/{cluster}/{namespace}/csp-configuration`, mounted read-only at `/ncrtxapp/mnt/config`. The pods run as uid and gid 1010.
  - **Two kinds of file are resolved differently:**
    - **Property sources:** `application.*`, `<app>.*`, `application-<tenant>.*` and `<app>-<tenant>.*`, in `.properties` or `.yml`. These are merged setting by setting. A tenant-specific file beats a general one, and at equal rank `.properties` beats `.yml`. The profile requested is `<tenant>,default`.
    - **Resource files:** every other file. The whole file is chosen: `<name>-<tenant>.<ext>` if it exists, otherwise `<name>.<ext>`. A file in a channel folder wins when the request names that channel. Finally, every `${KEY}` whose key exists in the merged property view is replaced with that value.
  - This supersedes D14's per-pattern merge rule. A file's merge behaviour now follows from its role.
- **D83** **Search locations.** The config-server searches `<root>/{application}`, `<root>/{application}/{profile}`, and every directory except channel directories (those listed in `channels.yml`).
  - The list is computed once, when the config-server starts. A new directory, or a change to `channels.yml`, isn't served until the config-server restarts.
  - Because every non-channel directory is a search location, file names effectively share one namespace.
- **D84** **The suffix is the tenant id, which equals the tenant branch name.** The tenant data is deployed as the Helm chart `csp-tenant-data-<branch>`, whose artifactId is `csp-tenant-data-TENANT_ID`. Each swimlane is modelled with a set of deployed tenants, not exactly one (Q34).
- **D85** **When changes take effect.** This replaces "only after restart" (D10, D43). It applies to services using tx-config-client, detected from each Deployment's environment:
  - **Resource files:** live within the client cache lifetime (`CONFIG_CLIENT_CACHE_TTL`, default 20 minutes), or immediately after a notification.
  - **Property sources:** a notification invalidates the client cache and refreshes the Spring context, if `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED` is on. Otherwise the service must restart.
  - **New folders and `channels.yml`:** need a config-server restart (D83).

  Lanekeeper adds a **"Notify services"** action. The agent POSTs the changed paths to the in-cluster config-server's `/update-resources` endpoint (form field `path`, header `backend: filesystem`). It requires Operator, is audited, and batches the paths after each write. Per swimlane, an admin can turn on auto-notify, which is off by default.
- **D86** **Rendered view and "as served".**
  - The engine renders resource files the way the config-server does.
  - The agent can fetch exactly what a service receives (`GET /{app}/{tenant},default/master[/{channel}]/{file}`). This is read-only and rate-limited, and is used both for the "As served" view and for nightly differential checks of the engine. A mismatch is a high-severity finding.
- **D87** **New checks:**
  - **C9 ambiguous file name:** the same name in more than one non-channel folder. There are 13 such names in base today, for example `authentication-config.yaml` in 12 core-adapter folders.
  - **C10 unresolved placeholder:** a placeholder that isn't in the property view, and isn't an environment variable name on any consuming Deployment.
  - **C11 config-server restart required:** a folder created after the config-server started, a `channels.yml` change, or a mismatch between channel folders and `channels.yml`.
- **D88** **What the agent reports about Deployments.**
  - **Values,** for an allowlist only: `SPRING_APPLICATION_NAME`, `SPRING_PROFILES_ACTIVE`, `CONFIG_CLIENT_CACHE_TTL`, `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED`.
  - **Names only** for every other environment variable, never their values. This feeds C10.
  - **The config-server pod's start time,** which feeds C11.
