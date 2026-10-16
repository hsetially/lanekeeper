# The Lanekeeper plugin package for Cursor

Prompt 07, T4. This folder builds the package that the Cursor team marketplace distributes: the MCP entry for the hub's `/mcp`
endpoint plus the Lanekeeper skill of prompt 11. The guide for the administrator who publishes and runs it is
`docs/cursor-admin.md`; read that first.

## Status: read this before you rely on anything here

**The Cursor 3.22.12 plugin and marketplace format could not be checked.** `cursor.com` was not reachable from the sandbox that wrote
this, and the compatibility test (T0 of decision D91) is **NOT RUN**. The package is built as plan 07 describes it (plan ambiguity
A9), and the tests check **our own invariants only** (below). Every detail that depends on Cursor is **UNVERIFIED**:

| UNVERIFIED detail | Where it is written | What would show it is wrong |
|---|---|---|
| The manifest is `.cursor-plugin/plugin.json` and its fields are `name`, `version`, `description`, `author`, `keywords` | `.cursor-plugin/plugin.json` | Cursor rejects the plugin, or lists it without a name or description |
| An `mcp.json` at the root of the plugin is read as the plugin's MCP servers | `package.sh` writes `mcp.json` | The server `lanekeeper` does not appear after the install |
| A folder `skills/<name>/` with a `SKILL.md` at the root of the plugin is read as a skill | `package.sh` copies `skills/lanekeeper/` | The skill is not listed after the install |
| The shape of the `auth` block (`CLIENT_ID`, `scopes`) of an entry with a `url` | `mcp.oauth.json` | Cursor does not start the sign-in, or shows an error about the entry |
| `headers` with `${env:LANEKEEPER_TOKEN}` is filled from the environment Cursor starts with | `mcp.pat.json` | The hub sees `Bearer ${env:LANEKEEPER_TOKEN}` as the literal token (a 401) |
| Which marketplace feature takes a built folder, and how it is published | `docs/cursor-admin.md` | The publishing steps there do not match Cursor's admin screens |

**Check each UNVERIFIED detail on the first install**, on one machine, before the package goes to the organisation. The checklist
is in `docs/cursor-admin.md` ("First install checklist"). Record what you find in D91 (`docs/decisions.md`), and change the file
named in the table if Cursor wants something else.

## What is in the folder

| File | What it is |
|---|---|
| `.cursor-plugin/plugin.json` | The manifest. It names the plugin and says nothing about authentication. |
| `mcp.oauth.json` | The MCP entry for OAuth mode: `url` plus `auth` (`CLIENT_ID`, `scopes`). Placeholders `@HUB_ORIGIN@`, `@CLIENT_ID@`, `@API_AUDIENCE@` are filled at build time. |
| `mcp.pat.json` | The MCP entry for personal-token mode: `url` plus `headers`. The token is `${env:LANEKEEPER_TOKEN}`, read from the environment of the user's machine; no token is ever written into the package. |
| `package.sh` | The builder. One mode per run. |

## Building a package

One run builds **one** mode, because the two modes are never combined (D52): the server accepts one kind of credential per
deployment, so an entry with both `auth` and `headers` could have at most one half accepted, and personal-token mode publishes no
sign-in metadata for an `auth` block to find.

```sh
# OAuth mode (the default of the server). The values are your deployment's; none is committed to this repository.
LK_PLUGIN_HUB_ORIGIN=https://lanekeeper.example.org \
LK_PLUGIN_CLIENT_ID=<the Cursor public client's Application (client) ID> \
LK_PLUGIN_API_AUDIENCE=api://<the hub API's Application ID> \
  just mcp-plugin oauth

# Personal-token mode (the fallback when OAuth fails in T0)
LK_PLUGIN_HUB_ORIGIN=https://lanekeeper.example.org \
  just mcp-plugin personal_token
```

The package is built into `target/plugin/` (removed first), and has exactly three parts:

```
target/plugin/
  .cursor-plugin/plugin.json
  mcp.json                      one of mcp.oauth.json or mcp.pat.json, filled in
  skills/lanekeeper/            a copy of skills/lanekeeper/ (prompt 11), byte for byte
```

The builder checks every value before it writes anything, names the variable of a value it refuses and never prints the value,
refuses OAuth settings in a personal-token build, refuses a missing `SKILL.md` and a symlink inside the skill, and leaves nothing
behind when it fails. It never edits `skills/`; that folder belongs to prompt 11. The built package carries your host and client
id: publish it from wherever your organisation keeps such things, not from this repository.

## What the tests check (our invariants, not Cursor's acceptance)

`crates/hub-mcp/tests/plugin_package.rs` runs the real builder:

- the `mcp.json` of a package has exactly one of `auth` or `headers`, never both, and the personal-token package mentions no
  client id, scope, tenant, `.well-known` path or Entra host, in its `mcp.json` or its manifest;
- the skill of prompt 11 is inside, every file byte for byte, and nothing shaped like a personal token is in any file;
- the manifest is JSON with a kebab-case `name`, a `MAJOR.MINOR.PATCH` `version`, a `description` and an `author`;
- the builder refuses a bad mode, a missing or invalid setting, an `http://` origin, a path, quote or newline in a value, a
  non-empty target, a missing skill and a symlink, and `just mcp-plugin` exists.

`crates/hub-mcp/tests/cursor_admin_doc.rs` holds the guide: the allowlist and the tool counts are generated from the contract, and
the guide's names and numbers are the code's.

## Changing it

- A new tool or a changed role is a contract change (`docs/mcp-tools.md`); then `just mcp-schemas` and `just mcp-admin-doc`
  regenerate the embedded contract and the guide's allowlist.
- The skill is changed in `skills/lanekeeper/` by prompt 11, and the package is rebuilt. Bump `version` in
  `.cursor-plugin/plugin.json` when the package changes.
- When a human has checked a row of the table above on a real Cursor, remove UNVERIFIED from that row here and in
  `docs/cursor-admin.md`, and record the result in D91.
