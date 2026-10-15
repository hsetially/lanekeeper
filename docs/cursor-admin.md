# Cursor administrator guide: the Lanekeeper MCP server

For the administrator who publishes the Lanekeeper plugin to the Cursor team marketplace and runs the MCP server it connects to
(prompt 07, T4; decisions D50, D51, D52). It covers the two authentication modes, draft mode, the plugin package, the marketplace
entry, the allowlists, the first install and what to do when something fails.

The tool allowlist and the tool counts below are **generated from the contract** (`docs/mcp-tools.md`, through
`crates/hub-mcp/src/schemas/tools.json`) and a test fails when they drift; run `just mcp-admin-doc` after a contract change. The
switch names, paths and numbers are the code's constants, and a test holds them too.

## Status: what is verified and what is not

This guide mixes four kinds of statement. Read the label before you act on one.

**Verified by tests in this repository (the server side).** What the hub answers: the 401 and its `WWW-Authenticate` challenge, the
discovery documents, how the verdicts of the token verifier map to 401, 403 and 503, personal tokens, the confirmation rules and
draft mode, and the package builder. The tests are in `crates/hub-mcp/tests/` (`auth_oauth`, `auth_pat`, `confirm`, `stub_protocol`, `plugin_package`,
`cursor_admin_doc`). They drive the server in process. They do not involve Cursor.

**NOT RUN: the Cursor 3.22.12 compatibility test (T0 of D91).** It needs Cursor 3.22.12 on Windows and on macOS and the Entra
registrations of Q21, so an agent cannot run it, and no result is recorded: every cell of the D91 table in `docs/decisions.md`
says NOT RUN, and NOT RUN is not a pass. Everything this guide says about what *Cursor* does (that it finds Entra's sign-in from the
401, that it shows the server's confirmation form, that it shows its own approval prompt for a tool that is not allowlisted) is the
design's assumption until a human fills D91 with the runbook `crates/hub-mcp/t0/README.md`. `just mcp-t0-status` prints the
table, and `just mcp-t0-status --strict` fails until it is filled.

**UNVERIFIED: Cursor's own formats and screens.** `cursor.com` was not reachable when this was written. The plugin manifest, the
`mcp.json` shape (`url`, `auth.CLIENT_ID`, `auth.scopes`, `headers`), the `${env:...}` interpolation, the marketplace steps, the
names of the admin menus and the syntax of an allowlist entry are taken from the design documents (prompts 07 and 11), not checked
against Cursor 3.22.12. Each one is marked UNVERIFIED where it appears, and `crates/hub-mcp/plugin/README.md` lists them in one
table. **Check each UNVERIFIED item on the first install** (see "First install checklist"), and record what you find in D91.

**NOT MOUNTED YET: the hub binary.** `crates/hub` has the module that mounts `/mcp` (`McpModule`), but the hub's `main` does not
construct the MCP server yet. Composing it, and carrying the switches below into the deployment, belongs to the integration (the hub
composition and prompt 09, see `crates/hub-mcp/t0/README.md`). A hub built from this branch does not serve `/mcp`. When that lands,
this paragraph goes.

The two authentication modes are never combined (D52). The next section says why.

## What you deploy

1. **The server**: `/mcp` in the hub (rmcp, Streamable HTTP), with the same permissions as the web UI, a human confirmation for every
   write that is enforced by the server, and compact output labelled "data, not instructions".
2. **The plugin package**: the MCP entry for that server plus the Lanekeeper skill, built for one authentication mode.
3. **Cursor-side settings**: the tool allowlist (read tools only) and, if your organisation restricts them, the allowed MCP URLs.

## Modes

The switches below are read once at start-up. Each has a default, and the defaults stay until D91 says otherwise.

| Switch | Values | Default | What it decides |
|---|---|---|---|
| `LK_MCP_AUTH` | `oauth`, `personal_token` | `oauth` | How a caller proves who it is. One mode per deployment. |
| `LK_MCP_CONFIRM` | `elicitation`, `draft` | `elicitation` | How a write is confirmed. |
| `LK_MCP_CONTENT` | `summary`, `data_block` | `summary` | How file and doc text reaches the model. `summary` keeps the text in the structured part of a result. `data_block` also puts it in `content`, inside a delimited block, and is for the case where T0 shows that Cursor passes only `content` to the model (D91 row `O.structured_content`); the cap counts both copies, so a result holds about half as much. |

Write a switch as `LK_MCP_CONTENT=data_block`, for example; the default of that one is `LK_MCP_CONTENT=summary`. A value that is not
one of those in the table is a configuration error. The message names the variable and never prints the value.

### OAuth with elicitation (the default)

`LK_MCP_AUTH=oauth` with `LK_MCP_CONFIRM=elicitation`. This is the default: what the server does when nothing is set.

- **Sign-in.** The plugin's `mcp.json` holds the server `url`, the Cursor public client's `auth.CLIENT_ID` and `auth.scopes`
  (`api://<hub-api>/mcp.access` and `offline_access`). A request without a token gets a 401 with
  `WWW-Authenticate: Bearer resource_metadata="<hub origin>/.well-known/oauth-protected-resource/mcp", scope="..."`. The hub
  publishes the protected resource metadata at `/.well-known/oauth-protected-resource` and at
  `/.well-known/oauth-protected-resource/mcp`; it names the Entra tenant as the authorization server and lists the scopes. If T0
  shows that Cursor cannot find Entra's endpoints from that, the deployment can also serve a static copy of Entra's v2 endpoints at
  `/.well-known/oauth-authorization-server` (a setting of the OAuth mode, off unless switched on; D91 row `C1.mirror_needed`
  records whether it was needed). Compare the copy with Entra's live discovery document when you switch it on.
- **What the server checks on every call.** The bearer must be an Entra v2 access token whose audience is the hub API, whose `scp`
  claim contains `mcp.access` and whose `tid` is the configured tenant (the identity crate's token verifier does that, prompt 03a;
  the MCP server maps its verdicts), and whose user is active in the hub. A pending or disabled user gets 403 on every tool (D36).
  The user is read again on every call, so a role change or a disabling takes effect on the next call.
- **Entra prerequisites** (Q21): the hub API exposes `mcp.access` and issues v2 tokens (`accessTokenAcceptedVersion` is 2), and the
  Cursor public client has the redirect URI and the delegated permission that Q21 records.
- **Confirmation** is by MCP elicitation: see "What a write looks like".

Whether Cursor 3.22.12 starts the sign-in from that 401 on its own, and whether Entra accepts the request it builds, is T0 check
`C1.discovery` and `C1.entra_accepts`: **NOT RUN**. If OAuth fails, the outcome rule of D91 is personal-token mode.

### Personal-token mode

`LK_MCP_AUTH=personal_token`. The fallback when T0 shows that OAuth does not work in Cursor 3.22.12 (D91 row `C1.headers` is the
check that the fallback itself works).

- **The token.** A user makes it while signed in to the web app, through `GET` and `POST /api/v1/me/mcp-tokens` and
  `DELETE /api/v1/me/mcp-tokens/{id}` (a personal token cannot make or revoke tokens, and in OAuth mode these routes answer 404).
  It is `lkpat_` followed by 256 random bits, valid for 7 days, shown **once** when it is created and never listed afterwards, and
  at most 5 live tokens per user (a sixth is a 409). The hub stores only an HMAC-SHA-256 of it, under a key held in Secret Manager
  (`mcp-token-hmac-key`). Revoking a token stops it at once on the replica that handled the revocation and within 5 seconds on
  the others, and the user's status is checked on every call. The screen for these routes is in the design of prompt 08 and is not
  built in this branch; until it is, the routes are the way to make a token.
- **On each machine.** The user sets `LANEKEEPER_TOKEN` in the environment that Cursor is started with. The plugin's `mcp.json`
  sends `Authorization: Bearer ${env:LANEKEEPER_TOKEN}` in `headers`. **UNVERIFIED:** that Cursor 3.22.12 fills `${env:...}` in
  `headers` (D91 row `C1.headers`). If it sends the text literally, the hub answers 401 and the fix is on the Cursor side.
- **No discovery.** In this mode the hub registers no `.well-known` route at all, and its 401 says `Bearer` and nothing else. A JWT
  in this mode is a 401, and a `lkpat_` token in OAuth mode is a 401.
- **The weaker mode.** A bearer that lives in an environment variable on a laptop for 7 days is a bigger target than a token that
  Entra refreshes. What limits it: the token is stored hashed, shown once, revocable, checked against the user's status on every
  call, and expires. Tell people to revoke a token they lose. There is one HMAC key and no rotation in this version: replacing the
  key ends every token.
- **Accepted deviation, S3 (CSRF).** `POST` and `DELETE` on the token routes should be behind a CSRF double-submit token and an exact
  `Origin`. In this branch they check `Origin` only: a request whose `Origin` is missing or is not the hub's public origin is
  refused before the body is read. The double-submit half is **not implemented** in prompt 07, because the hub composes no CSRF
  layer yet; it belongs to whoever composes prompt 03a's CSRF layer into the hub, in the integration. The human accepted this for
  now, and it is to be revisited. The residual risk is the loss of defence in depth: the exact `Origin` check is the only control
  against a cross-site request to these two routes. The reviewer decides whether that blocks the merge.

### Why the two modes never combine

D52 says personal-token authentication is a separate mode, never combined with OAuth. It holds in three places.

- **The server.** A deployment runs one mode. `LK_MCP_AUTH=oauth,personal_token` is a configuration error, and a server with that
  error refuses to start; so does a value that is not one of the two mode names. A JWT in personal-token mode and a `lkpat_` token in OAuth mode are both
  401, and personal-token mode publishes no discovery metadata, so an `auth` block has nothing to discover there.
- **The client entry.** The server accepts one kind of credential, so an entry that carries both `auth` and `headers` can have at
  most one half accepted. What Cursor would do with such an entry is not something to find out in production.
- **The package.** The builder takes one mode per run (`just mcp-plugin oauth` or `just mcp-plugin personal_token`), refuses OAuth
  settings in a personal-token build, and checks that the `mcp.json` it writes has `auth` or `headers` and not both. A test builds
  both packages and checks the same.

### Draft mode

`LK_MCP_CONFIRM=draft`, and also automatic for any client whose `initialize` did not declare elicitation support.

A write tool that acts on the world (those marked "yes" in the table under "Tool allowlist") then does nothing: no write, no
preview, no call to any service. It answers with `outcome: draft_link` and a `web_link`, and nothing is written.
The user opens the link and confirms in the web UI, where the change is shown and audited like any other. `propose_change` still
records a draft, because it has no side effect.

Use it when:

- T0 checks `C2.*` fail on a platform (the prompt, accept, decline, cancel or no-hang check). Force draft mode for that platform. If
  elicitation hangs on one operating system and not the other, that is a stop condition of prompt 07: escalate (D91);
- the deployment has several hub replicas and the load balancer does not pin an `Mcp-Session-Id` to a replica. A session and the
  answer to a confirmation live in one replica's memory (plan A5); an answer that reaches the wrong replica finds no pending
  question and writes nothing, which is safe but makes confirmation unreliable. Draft mode removes the dependency.

## What a write looks like

Every write tool marked "yes" in the table under "Tool allowlist" acts on the world, and the server asks the user before each of
them. In elicitation mode:

1. The server sends an MCP elicitation request. Its message is built by the server from its own data (the swimlane, the file or
   files, the change counts, the effect text of D85 and a web link). Anything the assistant supplied, such as a path or a PR title, is
   cut, stripped of control characters, quoted and labelled as supplied by the assistant. The form asks one boolean, `confirm`.
2. The server proceeds only when the user chooses accept with `confirm` true. Decline, or accept with `confirm` false, ends in
   `confirmation_declined`. A cancel, an answer that is not a confirmation, a timeout and a vanished client end in
   `confirmation_cancelled`. In every one of those cases nothing is written. The server waits 120 seconds for an answer.
3. The write goes through the write service with `via = mcp`, so it is audited as any other change. A retry of the same request
   never applies twice (D68). A user with `requires_approval` confirms like anybody else, and the result is `proposal_created`.

This confirmation comes **in addition to** Cursor's own approval prompt for a tool that is not allowlisted. Whether Cursor shows
that prompt every time (T0 check `C3.write_prompts`) is NOT RUN.

## The plugin package

**UNVERIFIED** in its layout: see `crates/hub-mcp/plugin/README.md` for the table of what could not be checked.

A package has three parts: `.cursor-plugin/plugin.json` (the manifest), one `mcp.json` (the entry for the server `lanekeeper`), and
`skills/lanekeeper/` (the skill of prompt 11, copied at build time, byte for byte). Build one for your mode:

```sh
# OAuth mode
LK_PLUGIN_HUB_ORIGIN=https://<hub host> \
LK_PLUGIN_CLIENT_ID=<the Cursor public client's Application (client) ID> \
LK_PLUGIN_API_AUDIENCE=api://<the hub API's Application ID> \
  just mcp-plugin oauth

# Personal-token mode
LK_PLUGIN_HUB_ORIGIN=https://<hub host> just mcp-plugin personal_token
```

The result is in `target/plugin/`. The builder reads your deployment's values from the environment, so none of them is committed to
the repository; it checks each one before it writes anything, names the variable of one it refuses, and refuses a missing skill. The
OAuth package holds the hub URL, the client id and the scopes, which are identifiers and not secrets. The personal-token package
holds only the reference `${env:LANEKEEPER_TOKEN}`; no token is ever in a package.

## Marketplace entry

**UNVERIFIED.** The steps to put a plugin into the Cursor team marketplace of 3.22.12 could not be checked, and this section does
not invent them. What the design says: the package built above is what the marketplace distributes, one package per mode, named
`lanekeeper`.

1. Build the package for your mode and keep the built folder out of this repository; it carries your host and client id.
2. Publish the folder the way your organisation's Cursor team marketplace takes a plugin. Which feature that is in 3.22.12 (a Git
   repository the marketplace reads, an upload in the admin dashboard, something else) is UNVERIFIED. Find it in Cursor's own
   documentation for your plan and follow it.
3. Install it on one machine first and run the "First install checklist". Then make it available to the organisation.
4. When the skill or the package changes, rebuild, bump `version` in `.cursor-plugin/plugin.json`, and publish again.

## MCP allowlist URL

**UNVERIFIED:** how Cursor 3.22.12 asks an administrator to allow an MCP server by URL, if it does at all.

The address to allow is the hub's public origin, `https://<hub host>`, with the endpoint `/mcp` on it. It must be exactly the
hub's public origin and not an internal alias: the server checks the `Host` of every request against it, and checks `Origin` when
one is sent, and answers 403 to anything else before it looks at the token.

What a client reaches, so that a network or URL allowlist can be written:

| Mode | Hub, on the hub's origin | Entra, OAuth mode only |
|---|---|---|
| OAuth | `/mcp`, `/.well-known/oauth-protected-resource`, `/.well-known/oauth-protected-resource/mcp`, and `/.well-known/oauth-authorization-server` when the mirror is on | `https://login.microsoftonline.com/<tenant-id>/oauth2/v2.0/authorize` and `/oauth2/v2.0/token` (another host in a sovereign cloud) |
| Personal token | `/mcp` only | none |

## Tool allowlist

**UNVERIFIED:** where Cursor 3.22.12 lets an administrator or a user choose which MCP tools run without asking (auto-run), and
how an entry names a tool. The server is `lanekeeper`, the key of the entry in `mcp.json`; the tools are named as below.

<!-- BEGIN GENERATED allowlist (from crates/hub-mcp/src/schemas/tools.json; run `just mcp-admin-doc`; do not edit by hand) -->

Allowlist these 20 read tools, and nothing else. Each is Viewer-level, asks nothing and changes nothing.

- `list_swimlanes`
- `get_swimlane`
- `list_files`
- `read_file`
- `get_effective_config`
- `get_served_config`
- `compare`
- `compare_tree`
- `compare_grid`
- `search_settings`
- `search_text`
- `find_inconsistencies`
- `get_drift`
- `get_pending_restarts`
- `get_history`
- `search_docs`
- `read_doc`
- `grep_docs`
- `get_docs_for_file`
- `get_feature_status`

<!-- END GENERATED allowlist -->

Only read tools may run without a person looking (D50). The write tools must always raise Cursor's own approval prompt, and the
server then asks its own question on top (D52). Allowlisting a write tool would take away Cursor's prompt (UNVERIFIED) and not the
server's confirmation; and if a client did not declare elicitation, the server does nothing and returns a `draft_link`.

<!-- BEGIN GENERATED write-tools (from crates/hub-mcp/src/schemas/tools.json; run `just mcp-admin-doc`; do not edit by hand) -->

These 7 write tools are never allowlisted. 6 of them change something outside the hub's database, and the server asks the user before each of them acts (see "What a write looks like").

| Tool | Lowest role | The server asks the user |
|---|---|---|
| `propose_change` | editor | no, it only records a draft |
| `apply_change` | editor | yes |
| `upload_file` | editor | yes |
| `delete_file` | editor | yes |
| `raise_pr` | editor | yes |
| `restart_service` | operator | yes |
| `notify_services` | operator | yes |

<!-- END GENERATED write-tools -->

`notify_services` is **never** on the allowlist, and it ships switched off: `tools.notify_services.enabled` is `false` by default,
the tool is absent from `tools/list` while it is off, and a call to it is `forbidden`. It ends at the config-server's unauthenticated
`/update-resources` endpoint (Q37), so it stays off until Q37 is answered. When it is on, it is Operator only and asks the person
first: the question names the swimlane, the files (up to 100 paths), that the swimlane's agent calls the endpoint, and that the
endpoint has no authentication. After a yes it does nothing but call the write service's notify, which audits the call and sends it
through the agent; the hub itself never calls the endpoint. It is the only tool that requests a notification. The swimlane's
auto-notify setting is a different thing: an Admin turns it on or off in the web UI, no tool changes it, and while it is on the write
service notifies after a committed write, whichever client made the write.

T0 checks `C3.read_runs` (an allowlisted read tool runs without Cursor's prompt) and `C3.write_prompts` (a write tool always shows
it) are NOT RUN.

## First install checklist

**UNVERIFIED** until a person has done it. This is the T0 check made on the real package, on one machine, before the organisation
gets it. Do it on Windows and on macOS, and write down every difference from this guide in D91.

1. Build the package for your mode and install it from the marketplace (or from the built folder, if Cursor can install one).
2. The plugin appears as `lanekeeper`, its skill is listed, and the MCP server `lanekeeper` appears. If not, the manifest or
   `mcp.json` layout is wrong for this Cursor: see the table in `crates/hub-mcp/plugin/README.md`.
3. OAuth mode: connecting sends you to Entra's sign-in without typing a URL, the sign-in completes, and a read tool runs. If Cursor
   cannot find Entra's endpoints, switch the `/.well-known/oauth-authorization-server` copy on and try again (`C1.discovery`,
   `C1.mirror_needed`, `C1.entra_accepts`, `C1.tool_call`).
4. Personal-token mode: make a token, set `LANEKEEPER_TOKEN`, restart Cursor from that environment, and a read tool runs (`C1.headers`).
   The hub's log shows no request for a `.well-known` path.
5. Allowlist the read tools above and nothing else. A read tool runs without Cursor's prompt; a write tool shows Cursor's prompt,
   every time (`C3.read_runs`, `C3.write_prompts`).
6. Call a write tool and answer the server's question: accept, decline and cancel each complete, and an unanswered question times
   out without hanging Cursor (`C2.prompt`, `C2.accept`, `C2.decline`, `C2.cancel`, `C2.no_hang`). If the form never appears, the
   client did not declare elicitation: you get a `draft_link`, which is the designed fallback.
7. Record the results in D91, remove UNVERIFIED from what you confirmed (here and in `crates/hub-mcp/plugin/README.md`), and set
   `LK_MCP_AUTH`, `LK_MCP_CONFIRM` and `LK_MCP_CONTENT` as the outcome rules say.

## Troubleshooting

| What you see | What it means | What to check |
|---|---|---|
| 401 with `WWW-Authenticate: Bearer resource_metadata="...", scope="..."` | No token was sent (OAuth mode). This is how the sign-in is meant to start. | Cursor should open Entra's sign-in from it (T0 `C1.discovery`, NOT RUN). If it does not, serve the `/.well-known/oauth-authorization-server` copy. |
| 401 with `error="invalid_token"` | A token was sent and is not good: expired, the wrong audience, no `mcp.access` scope, a user the hub does not know, or a personal token that is expired or revoked. | Read the token's `aud`, `scp`, `tid` and `ver` claims against the hub's settings. In personal-token mode, make a new token. |
| 401 in personal-token mode for a JWT, or in OAuth mode for a `lkpat_` token | The client is set up for the other mode. | Rebuild the package for the deployment's mode. The modes never combine. |
| 401 and the hub sees `Bearer ${env:LANEKEEPER_TOKEN}` | Cursor did not fill the variable. | `LANEKEEPER_TOKEN` must be in the environment Cursor was started from; UNVERIFIED that 3.22.12 fills `${env:...}` in `headers`. |
| 403 on every tool | The user is pending or disabled (D36), or `Host` or `Origin` is not the hub's public origin. | The user's status in the web app; the URL in `mcp.json` is the hub's public origin exactly. |
| `forbidden` on one tool | The user's role is below the tool's role. | `tools/list` shows only what the caller's role may call. `notify_services` is also `forbidden` while it is off. |
| 503 | The identity provider or the token store cannot answer. Not a verdict on the token. | Retry. Check Entra reachability from the hub, and the database. |
| 404 on a request that carries `Mcp-Session-Id` | The session ended, was idle for 30 minutes, or lives on another replica. | The client should start a new session (T0 `O.session_404` records what Cursor does). With several replicas, ask for affinity on `Mcp-Session-Id`, or use draft mode. |
| 429 on opening a session | The user is at the limit of 5 sessions per user. | Close an idle Cursor window, or wait: sessions end after 30 minutes idle. |
| `rate_limited` in a tool result, with `retry_after_secs` | The per-user rate limit of the tools. | Wait that many seconds. |
| 409 when making a personal token | The user already holds 5 live tokens. | Revoke one. |
| A write returns `draft_link` and nothing changed | Draft mode, or the client did not declare elicitation. | `LK_MCP_CONFIRM`; D91 rows `C2.*` and `O.elicitation_capability`. Confirm in the web UI at the link. |
| The server does not start and names `LK_MCP_AUTH` | The value names both modes, or something that is not a mode name. | Name exactly one of `oauth` and `personal_token`. |
| Entra shows `AADSTS50011` or `AADSTS700016` | The Cursor client's registration does not match what Cursor sends (the redirect URI or the client id). | Fix the registration (Q21), not the hub. |
| `/mcp` is not found at all | The hub is not composing the MCP module (see "NOT MOUNTED YET"). | The deployment's hub build. |

## Known limits and accepted deviations

- **S3 for the personal-token routes is partly met.** They check `Origin` only; the double-submit half is not implemented and
  waits for the hub's CSRF layer. Described under "Personal-token mode". Accepted for now by the human; to be revisited.
- **`/mcp` authenticates itself.** The hub's route table lists it as a route that proves its caller itself (bearer check, active
  user, `Host` and `Origin`) and not as one behind the hub's role guard, because that guard cannot add the `WWW-Authenticate`
  challenge that OAuth sign-in starts from. Accepted for now by the human; a small change in the platform crate can put it back
  behind the role guard.
- **`get_effective_config` is incomplete for resource files.** It returns `text` as null, and each setting with type `other` and no
  flag, until a later contract change adds the rendered text and a typed value (D92). `get_served_config` returns what the
  config-server serves, including a resource file's body.
- **Sessions live in one replica's memory.** See "Draft mode" and the 404 row above.
- **One HMAC key, no rotation.** Replacing it ends every personal token.
- **T0 is NOT RUN, and the Cursor formats are UNVERIFIED.** See "Status".
