# Changes in this version of the bundle

This version reflects your answers to the open questions and what the config repos and docs showed. If you read the first version, these are the changes that matter.

## Sign-in and roles

- **Microsoft Entra ID replaces Okta** for sign-in, in the web UI and in Cursor.
- **Roles are managed inside the app,** on a user-management screen, not through identity-provider groups.
- **New users get no access until an admin approves them.**
- **Editors and above are asked for their GitHub token** on their first sign-in after approval.

## How files are named

- **Git and NFS use different names for tenant files.**
  - In Git, a tenant file has the same path as its base file.
  - On NFS, it sits beside the base file with the branch as a suffix: `tx-infinity-core-sit1.yml` next to `tx-infinity-core.yml`.
  - Mapping between repo and NFS is therefore a rename rule, not just a folder mapping.
- **Every file under `data/config` is tracked,** not only YAML, JSON and `.properties`. That includes XSL templates, images and files with no extension.
- **Line endings are preserved on every write.** Almost all files use CRLF.
- **How a tenant file and its base file combine is configurable per file pattern.** The default is whole-file fallback; Spring-style merging is the alternative. This stays configurable until the config-server code has been reviewed.

## Checks

- **New check, C8:** duplicate keys. Real case: `security/security-roles.yml` in base defines three keys twice.
- **C6 redesigned.** It now flags external hosts that belong to the wrong environment tier (dev, UAT or prod). It no longer looks for swimlane names, because no config file contains any.
- **The secret check now looks at values, not key names.** Hundreds of harmless keys contain the word "token".

## Docs search ships in v1 (new prompt 14)

- Markdown docs are indexed automatically from the tenant repo's `templates/common-docs` branch.
- Editors and above can also upload docs in the web UI.
- Search combines keyword and vector search, using Postgres with pgvector.
- The Feature Flags tables in the docs feed a "feature status" view.

## Cursor

- **Sign-in:** Entra OAuth, with a fixed client ID in `mcp.json`.
- **Distribution:** the MCP server and the skill both go through the team marketplace.
- **Auto-run:** the tool allowlist lets only read tools run automatically.
- **First step of prompt 07:** a one-day test on Cursor 3.22.12, on Windows and macOS, covering Entra sign-in and the confirmation prompt (elicitation).

## Network and infrastructure

- **One external HTTPS endpoint, with two entry points.**
  - Agents connect through a TCP pass-through load balancer that only accepts the clusters' outbound IPs.
  - Users connect through an HTTPS load balancer that only accepts GlobalProtect address ranges.
- **Container images go to GHCR.** Pulling them into private clusters needs a decision (Q8).

## Scope

- **v1 covers only `data/config`.**
- **Sync reporting:** decided later, so prompt 10 is now an investigation.
- **Moved to v2:** GLiNER, the decision model, swimlane claims, tag creation, release snapshots and a drift-from-template check.

## Added after the library review

- **New libraries:**
  - rayon, for batch work;
  - ripgrep's `ignore`, `globset`, `grep-regex` and `grep-searcher` crates;
  - tree-sitter with the YAML grammar, for setting-to-line mapping.

  The reasons for not adopting the rest are recorded in D54.
- **New v1 feature:** text search across the current files of selected swimlanes, in the UI and as the MCP tool `search_text`.
- **Hash-chained audit log,** verified nightly.
- **Docs filesystem tools for Cursor:** `read_doc` and `grep_docs`.
- **Compact MCP output:** results are capped, with cursors and web UI links, and `read_file` reads by setting path or line range.
- **Dev tooling notes:** Vibe Kanban and rtk. Both are optional, and neither is part of the product.
- **New in v2:** surgical single-setting edits, using tree-sitter.

## Agent-first, security-first and performance-first revision

**New documents:**

- `docs/security.md`, with requirements S1–S24;
- `docs/performance.md`, with budgets P1–P13 for a stated design scale;
- `docs/interfaces.md`, defining the trait seams between prompts;
- `docs/threat-model.md`;
- `prompts/REVIEW.md`, the checklist for the reviewer agent.

**Agent workflow:**

- AGENTS.md is now an execution protocol: plan, wait for approval, work task by task, pass the gate, then reviewer-agent review.
- Every prompt follows one template: a metadata table, tasks with Verify commands, the S# and P# it covers, acceptance criteria, stop conditions, and a `just verify-NN` gate.
- Prompt 01 now also builds the port traits with fakes and conformance tests, the budget registry (`bench-check`), and a deterministic synthetic fixture generator at design scale. Wave 2 can start against fakes, without waiting for wave 1.

**Restructured prompts:**

- The hub is split into **03a**, identity and audit, and **03b**, the platform.
- New **15**, security verification, and **16**, performance verification at design scale.

**Simplifications removed (D61):**

| Before | Now |
|---|---|
| One hub replica | 3 replicas with routing between them |
| Manual join tokens | Workload Identity attestation |
| CA key held in a Kubernetes Secret | KMS signing key |
| Kubernetes Secrets | Secret Manager |
| 60-second scans | 10-second stat walks with Merkle roots |
| 2-minute Git polling | Webhooks |
| Hash-chained audit | Hash chain plus KMS-signed checkpoints in a locked bucket |

**Added for performance and features:**

- settings index, Git tree index and effective tree hashes;
- an event-driven incremental pipeline;
- SSE live updates;
- whole-swimlane compare;
- idempotency keys on writes;
- strict CSP with Trusted Types;
- cap-std for agent file access;
- signed images with SBOMs, provenance and Binary Authorization.

## Design track

- **New `design/` folder** with Claude Design prompts 00–14:
  - project brief;
  - design system;
  - app shell;
  - eleven screen areas;
  - a sweep of states and accessibility;
  - clickable prototypes of the 7 key journeys;
  - handoff instructions.
- **Prompt 08** gains T0, importing the design handoff, plus visual-regression and keyboard-map acceptance tests. Deviations from the design are recorded in `design/DEVIATIONS.md` (D71).

## Patterns adopted from the config-watcher reference project

- **New prompt 17: NFS VM sentinel.** It runs on each Rocky Linux NFS VM and reads auditd records through the audisp socket, using OS Login identities. Edits made on the VM through IAP Desktop get named authors. It never reads file content (D72, S25).
- **Attribution.** Every change carries an attribution with a source and a confidence level. This replaces "unknown author" (D73).
- **Agent improvements:**
  - a durable spool, so intermediate versions survive hub outages (D74);
  - quiescence and sync-window tagging, so a sync produces one alert evaluation instead of an alert storm (D75).
- **Severity rules,** applied deterministically to changes and findings (D76).
- **PR handling:** PR links with state tracked through webhooks, duplicate PRs blocked, and PR creation as a resumable background job (D77).
- **Version retention** with reference-safe garbage collection (D78).
- **Sensitive data:**
  - agent deny globs for keystores and keys;
  - MCP redaction;
  - masked values in the UI, with an audited reveal action (D79).
- **Events and diffs:**
  - a transactional outbox, so events are published only after the commit (D80);
  - word-level highlights in diffs (D81).
- **Not adopted from the reference project:**
  - its security model: unauthenticated reads, CORS open to any origin, a shared enrollment secret, an agent port listening on `0.0.0.0`;
  - the Yew/WASM dashboard;
  - difftastic run as a subprocess;
  - git2 clones for PR creation;
  - BLAKE3 hashing;
  - YAML-only tracking.

## Config-server findings (csp-configuration-server repo)

- **Q4 answered:** Spring Cloud Config with a native NFS backend. There are two resolution rules: property sources are merged setting by setting; resource files are chosen whole (tenant file, then channel folder) and their placeholders substituted from the property view (D82).
- **Search locations** are fixed when the config-server starts. New folders and `channels.yml` changes need a config-server restart, and file names effectively share one namespace (D83).
- **The suffix is the tenant id,** which equals the branch name. Each swimlane now has a set of tenants (D84).
- **"Only after restart" was wrong.** Resource files go live within the client cache lifetime (20 minutes by default), or at once after a notification. Property sources refresh through notifications. New **"Notify services"** action, with optional auto-notify (D85).
- **The engine renders resource files** the way the config-server does. A new **"As served"** view fetches exactly what a service receives, and a nightly check compares it with the engine (D86).
- **New checks:**
  - C9, ambiguous file names (13 in base today);
  - C10, unresolved placeholders;
  - C11, config-server restart required (D87).
- **New questions:** Q34 (more than one tenant per swimlane), Q35 (change notifications in use), Q36 (does the dataload Job notify), Q37 (securing the unauthenticated `/update-resources`).
