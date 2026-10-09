# Domain model

## Topology

- **Project:** a GCP project. It holds one or more clusters.
- **Swimlane:** exactly one GKE cluster (for example `sitb`), identified by a slug. It has one NFS VM, one config-server, one ArgoCD and one Lanekeeper agent.
- **Agent:** runs in the swimlane's cluster, mounts the same NFS export as the config-server, and connects outbound to the hub.
- **Channel:** a delivery channel listed in the base `channels.yml`, such as `remote-itm-teller`, `atm-iso` or `atm`. Channels show up both as subfolders and as keys inside files. For example, `txInfinityOptions.<channel>.enableAccountSorting`.

## Repos and branches

- **Base repo** `configuration-base-saas`:
  - config under `config/`, docs under `doc/`;
  - a single default branch;
  - tags mark versions.
- **Tenant repo** `csp-tenant-data`:
  - config under `data/config/`;
  - other paths under `data/` are out of scope for v1.
- **Tenant branches** fall into three kinds:
  - **Deployable branches** (`sit1` to `sitN`) are swimlane candidates.
  - **Template branches** (`templates/*`) are never deployable.
  - **`templates/common-docs`** is a docs source.
  The include and exclude patterns are configurable.
- **Base version label:** the nearest tag at or before the deployed base commit, plus the distance. For example, `v2.3.1 + 4 commits`.

## Path mapping (Git to NFS)

There is one rule per repo, and each rule is configurable:

| Repo | Repo path | NFS path |
|---|---|---|
| base | `config/<rel>/<name>.<ext>` | `<nfs_root>/<rel>/<name>.<ext>` |
| tenant, branch B | `data/config/<rel>/<name>.<ext>` | `<nfs_root>/<rel>/<name>-B.<ext>` |

- **Reverse mapping** (NFS to Git) strips the `-B` suffix and returns the repo path on branch B. This is how PRs work.
- **Suffix stripping:** strip only an exact `-<branch>` immediately before the extension, where `<branch>` is a known deployable branch. Names like `tx-infinity-core` contain hyphens of their own, so don't strip anything else.
- **Extension-less files:** the suffix goes at the end of the name.
- **Open (Q3):** whether the rename also applies to images and XSL files. The default assumes yes, for every file.

## Files on NFS

**Kinds:**

- **Base file:** has no tenant suffix and a base-repo source.
- **Tenant file:** carries the suffix of a known deployable branch.
- **Untracked:** has no Git source at all.

**Classes, and how each is compared:**

| Class | Files | Comparison |
|---|---|---|
| `structured` | `.yml`, `.yaml`, `.json`, `.properties` | parsed, then diffed setting by setting and as text |
| `text` | `.xsl`, `.xml`, `.txt`, and extension-less files that decode as UTF-8 | text diff |
| `binary` | everything else, such as images | hash comparison, with a preview for images |

**Logical file:** the NFS path with the tenant suffix removed. `tx-infinity-core-sit1.yml` and `tx-infinity-core.yml` are the same logical file.

**Line endings:**

- Record each file's line-ending style (CRLF, LF or mixed), encoding and BOM.
- Comparisons ignore line-ending-only differences by default, but flag them.
- Writes keep the original style.

## How the config-server resolves files (D82–D85)

**Request shape.** A service asks for `(application, profile = "<tenant>,default", label = master, [channel], file)`.

**Search locations** are the root, `{application}`, `{application}/{profile}`, and every directory except channel directories. They are computed when the config-server starts (D83).

**Property sources** form the *property view* for an application and tenant. These files are merged setting by setting, highest precedence first:

1. `<app>-<tenant>.*`
2. `application-<tenant>.*`
3. `<app>.*`
4. `application.*`

At equal rank, `.properties` beats `.yml`, and a later search location beats an earlier one.

**Resource files** are every other file. For a request:

1. The whole file is chosen: `<name>-<tenant>.<ext>` if it exists, otherwise `<name>.<ext>`.
2. If the request names a channel, the copy in `<folder>/<channel>/` wins.
3. Each `${KEY}` whose key exists in the property view is replaced with that value. Any other `${...}` is left as written, to be resolved later from the service's own environment.

**Effective content** of a resource file is defined per (application, tenant, channel). The engine renders it. The agent can fetch the config-server's own answer ("as served") to check the engine against it.

**Tenants.** A swimlane has a set of deployed tenants; each tenant is a tenant branch. A tenant file's suffix is its tenant id (D84).

## When changes take effect (D85)

Each change gets a state per consuming service:

| State | Applies to | Becomes live when |
|---|---|---|
| `live` | — | Already live |
| `live_within_ttl` | Resource files, for clients with a cache lifetime | The client cache expires (shows "by HH:MM"), or a notification arrives |
| `needs_notify_or_restart` | Property sources | A notification arrives (if the client has notifications enabled), or the service restarts |
| `needs_config_server_restart` | New folders, `channels.yml` changes | The config-server restarts |

"Pending restart" (D43) is now only the third state, for clients without notifications enabled.

## Services

- **Service:** a Kubernetes Deployment (namespace and name) in a swimlane cluster.
- **Folder mapping:** links a folder path, which can be nested (for example `csp-remote-teller/item-storage-service`), to one or more Deployments.
  - The tool suggests mappings by name similarity, and a person confirms them.
  - Mappings are global by default, with optional per-swimlane overrides.
  - Root files map to every service.
- **Pending restart:** a service is pending restart when any of its effective files changed after any of its currently running pods started. "Changed" means the detection time of the file's newest version.

## Versions

- **Blob:** file content, stored once and keyed by SHA-256.
- **Observation:** a record of swimlane, path, hash, observed time and source. Source is `scan`, `tool_write` or `sync`.
- **Out-of-band change:** an observed hash that neither a tool write nor a sync explains. It is recorded with author `unknown`.

## Baselines and drift

For each swimlane and NFS path, three hashes matter:

- **B** is the baseline hash, with the Git commit it came from.
- **N** is the current NFS hash.
- **G** is the current Git hash, found through the path mapping:
  - base files come from the head of the base repo's default branch;
  - tenant files come from the head of the deployed tenant branch.

Compare hashes after normalising line endings for `structured` and `text` files.

A missing file is treated as the hash value `∅`, and the table below still applies.

Check for `untracked` first. If N is present, G is `∅` and there is no baseline, the file is `untracked`. Otherwise, use this table:

| Condition | State |
|---|---|
| No baseline, N = G | `in_sync` (set B = N) |
| No baseline, N ≠ G | `unknown` |
| N = B, G = B | `in_sync` |
| N = B, G ≠ B | `git_ahead` |
| N ≠ B, G = B | `nfs_ahead` |
| N ≠ B, G ≠ B, N = G | `in_sync` (set B = N) |
| N ≠ B, G ≠ B, N ≠ G | `conflict` |

How the UI labels the missing-file cases:

| State | Condition | Label |
|---|---|---|
| `nfs_ahead` | N = `∅` | "deleted on NFS" |
| `git_ahead` | N = `∅` | "new in Git" |
| `git_ahead` | G = `∅` | "deleted in Git" |

**`intentional_divergence`** overrides `git_ahead`, `nfs_ahead`, `conflict` and `unknown`. It holds while N and G both still equal the hashes recorded when someone marked it. It clears automatically as soon as either one changes.

## Consistency checks (findings)

- **C1 `missed_base_changes`** (`whole_file` mode): a tenant file exists for a logical file, and the base file has commits after the tenant file's last commit.
  - Report those base commits.
  - Report the setting-level changes, comparing the base file as it was when the tenant file was last updated against the current base head.
  - The matching is by commit time. Say so in the UI.

  In `spring_merge` mode, C1 instead reports **dead overrides**: tenant settings whose base setting no longer exists.

  Real example: the sit1 copy of `tx-infinity-api/entryGroupConfig.yml` has no `atm` entry, which base has.
- **C2 `redundant_tenant_file`:** a tenant file that is semantically equal to base (structured files) or byte-equal (other files). It blocks future base changes for that branch.

  Real examples in sit1: `tx-infinity-api/account-sorting-config.yml`, `tx-infinity-api/miniStatementConfig.yml` and `document-service/resources/receipt-ci_1.bmp`.
- **C3 `uneven_tenant_files`:** a logical file has tenant files on some deployed branches but not on others. This is informational.
- **C4 `orphan_file`:** either a tenant file on a swimlane's NFS whose suffix isn't the deployed branch, or an untracked file.
- **C5 `drift`:** any state other than `in_sync` and `intentional_divergence`.
- **C6 `environment_host_mismatch`:** an external hostname in a setting value belongs to a different environment tier than the swimlane.
  - Hostnames are classified by configurable patterns (for example `-dev`, `-uat`, `.okta.com` versus `.oktapreview.com`).
  - Each swimlane has a tier.
  - Cluster-local service names (such as `core-adapter-symxchange`) are excluded.
  - In the real data, no config mentions a swimlane or branch name. External hosts are where environments differ.
- **C7 `overwritten_nfs_change`:** a sync replaced NFS content that was in `nfs_ahead` or `conflict`. The finding links to the lost version, which can be restored.
- **C9 `ambiguous_file_name`:** the same file name in more than one non-channel search folder. Which copy is served depends on search order. Real example: `authentication-config.yaml` appears in 12 core-adapter folders in base.
- **C10 `unresolved_placeholder`:** a `${KEY}` that is in neither the property view nor the environment-variable names of the consuming Deployments.
- **C11 `config_server_restart_required`:** a folder created after the config-server's pod started, a change to `channels.yml`, or a channel folder that `channels.yml` doesn't list (or the reverse).
- **C8 `duplicate_keys`:** a YAML mapping with the same key more than once. The last one wins.

  Real example in base: `security/security-roles.yml` defines `txRemoteJuniorTeller`, `txRemoteSeniorTeller` and `txRemoteSupervisor` twice.

## Comparison grid

The grid compares one logical file across the swimlanes the user selects, using the effective config for each.

- Rows are setting paths. Columns are swimlanes. Cells hold values, or "absent".
- By default, only rows whose values differ are shown.
- An optional channel filter matches setting paths that contain a channel name.

Flattening rules:

- Maps become `a.b.c`. Sequences become `a.b[0]`.
- Keys containing `.`, `[` or `]` become `a["x.y"]`.
- Multi-document YAML gets a `#<doc index>/` prefix.
- JSON follows the same rules as YAML. `.properties` keys are used as they are.
- Scalars keep their type tag and their source text. `${...}` placeholders are shown exactly as written; there are two kinds, environment-style and template variables.
- Don't treat `yes` or `on` as equal to `true`. Flag these values instead.

## Compare references

REST and MCP both use these references:

- `nfs:<swimlane>`
- `baseline:<swimlane>`
- `effective:<swimlane>`
- `git:base@<ref>`
- `git:tenant@<ref>`

## Users and access

- **User:** keyed by Entra tenant id and object id. Fields:
  - email and display name;
  - role (`none`, Viewer, Editor, Operator or Admin);
  - `requires_approval`;
  - status (`pending`, `active` or `disabled`);
  - `last_seen_at`;
  - linked GitHub login.
- **Access request:** created at a user's first sign-in. An admin approves it with a role, or rejects it.
- **Bootstrap admins:** Entra object ids listed in configuration.

## Proposals and drafts

- **Proposals** are created for users with `requires_approval`.
- **Drafts** are created by MCP `propose_change`.

Each record holds:

- author and action;
- swimlane and paths;
- base hashes;
- new content or action parameters;
- status: `draft`, `pending`, `approved`, `rejected`, `stale`, `expired` or `applied`;
- `expires_at`, approver, and the reason for the decision.

## Docs

- **Doc:** a title and a source:
  - `upload`; or
  - `git`, meaning a repo, branch and path, such as the tenant repo's `templates/common-docs` branch.

  Each doc points to its current version.
- **Doc version:** a GCS object, its hash, who uploaded or synced it, and when.
- **Chunk:** one section of a doc version, split at H2 headings.
  - It carries the doc title and heading path as context.
  - Tables are kept whole.
  - It stores the text, a full-text vector, an embedding, the related files it mentions (backticked paths relative to the config root) and the setting paths it mentions.
- **Documented flag:** parsed from a doc's Feature Flags table, which lists flag, file, channel, template default and description.
  - It resolves to a setting-path pattern. For example, `enableAccountSorting` in `tx-infinity-api/tx-infinity-core.yml` for channel `remote-itm-teller` resolves to `txInfinityOptions.remote-itm-teller.enableAccountSorting`.
  - This feeds the feature-status view: each flag's value per swimlane, compared with the template default.

## Setting locations

Each setting path maps to a line range, so findings, grid cells and search hits can link to exact lines.

- **YAML:** ranges come from tree-sitter.
- **JSON:** ranges come from parser positions.
- **`.properties`:** one line per key.

## Text search

- Searches the current file versions stored in the hub, never NFS directly.
- Covers files in the `structured` and `text` classes. Binary files are skipped.
- Filters: selected swimlanes, an optional path glob, literal (the default) or regex, and a case-sensitivity option.
- Each result gives the swimlane, path, line number, the line text (truncated) and the match ranges. Line text is reported without the `\r` from CRLF line endings.
- Results are capped, both in total and per file. Truncation is reported.

## Audit chain

- Each audit event stores `prev_hash` and `hash`, where `hash = SHA-256(prev_hash || canonical event JSON)`.
- Appends are serialised.
- A nightly job recomputes the chain and alerts on any mismatch.

## Docs filesystem (MCP)

- **Virtual tree:** indexed docs appear as read-only paths, `/git/<branch>/<path>` and `/uploads/<name>`.
- **`read_doc`** reads a path, with an optional line range.
- **`grep_docs`** runs a literal or regex search, with context lines.
- **Implementation:** the grep crates, over stored doc text. There is no shell.

## Performance structures

- **Agent Merkle tree.**
  - The hash of a file is its SHA-256.
  - The hash of a directory is the SHA-256 of its entries, sorted by name, each written as name, kind and child hash.
  - The root hash is sent in every heartbeat. A changed root triggers `RequestDelta(since_root)`.
- **Settings index.** Rows of blob hash, setting path, value text, value type, start line and end line.
  - Written once per unique structured blob.
  - A pg_trgm index on setting path.
  - The settings index backs settings search, the grid and feature status.
- **Git tree index.** Rows of repo, commit, repo path, git object id and SHA-256.
  - Built once per commit.
  - When a head moves, the tree diff limits recompute to the paths that changed.
- **Effective tree hash.** A hash per swimlane per directory, computed over (logical file, effective hash) pairs. Whole-swimlane compare uses it to skip identical subtrees.
- **Idempotency keys.** Stored per user and key, with a hash of the request and the stored response, for 24 hours.

## Attribution (D73)

Each observation and audit event carries `attribution { source, confidence, actor?, evidence }`.

**How the hub correlates sources:**

1. **A tool write with the expected hash.** Source `tool_write`; confidence certain.
2. **A sentinel record on the same path within ±60 seconds of the change.** Source `sentinel_login`; confidence high. The actor is the OS Login user, mapped to an app user (Q32). The evidence is the executable, the original login identity, and the time.
3. **The change was observed while a sync Job was running.** Source `sync_job`; confidence high. The actor is the Job's name and uid.
4. **No local audit record and no Job running.** Source `nfs_client`; confidence medium. The write came through an NFS client.
5. **Only a file-owner hint.** Source `fs_owner_hint`; confidence low.
6. **Otherwise:** source `unknown`.

Attribution can be upgraded later, for example when a delayed sentinel record arrives. It is never downgraded. Every upgrade is audited.

## Severity (D76)

`severity(change | finding)` returns low, medium, high or critical, together with the ids of the rules that matched. Admins can configure the rules. The defaults are:

- **Critical:**
  - any change under `security/**`;
  - an overwritten NFS change (C7);
  - a denied file changing.
- **High:**
  - a root file changing (affects every service);
  - keys removed from a structured file;
  - a value changing type;
  - a conflict;
  - an environment host mismatch (C6).
- **Medium:** any other out-of-band change; missed base changes (C1).
- **Low:** redundant or uneven tenant files (C2, C3); duplicate keys (C8).

## PR links (D77)

- **Link record:** swimlane, path, NFS hash, repo, branch, PR number and state (open, merged or closed). One open PR per (swimlane, path, NFS hash).
- **PR jobs** move through `pending → preparing → committing → opening → completed | failed`. They're resumable and idempotent.

## Retention (D78)

- **Kept forever:** a blob referenced by a baseline, audit event, proposal, draft or PR link.
- **Otherwise kept** if it's among the last 50 versions of its file, or newer than 180 days.
- **Garbage collection:** a leased nightly job collects everything else, in batches.

## Denied files (D79)

- **Matching:** deny globs are applied by the agent.
- **What's recorded:** name, size, hash and class `denied`.
- **What's blocked:** content endpoints return "content withheld", and writes are refused.
