# 07: MCP server for Cursor 3.22.12

| | |
|---|---|
| **Wave** | 2. Step 0 can run right after 01. |
| **Depends on** | 01. Integrates with 03a, 05, 06 and 14. |
| **You own** | `crates/hub-mcp/**`, `docs/cursor-admin.md`, the MCP section of `docs/threat-model.md` |
| **Interfaces** | Consumes `TokenVerifier`, `Users`, `RegistryRead`, `WriteService` and `DocSearch`. |
| **Security** | S4, S11, S14, S14b, S15, S21 |
| **Performance** | P9 |
| **Gate** | `just verify-07` |

## Objective

An MCP server inside the hub, using rmcp with Streamable HTTP at `/mcp`. It gives Cursor the same facts and actions as the web UI, with:

- the same permissions;
- server-enforced human confirmation for every write;
- compact, data-labelled output that stays within P9.

## Tasks

### T0: Compatibility test (one day; record results in `docs/decisions.md`)

Using a stub server with one read tool and one write tool, check the following on Cursor 3.22.12, on both Windows and macOS:

1. **Entra OAuth with a fixed client ID.**
   - `mcp.json` sets `url`, `auth.CLIENT_ID` and `auth.scopes` (`api://<hub-api>/mcp.access`, `offline_access`).
   - The hub publishes protected-resource metadata pointing at the Entra tenant.
   - If Cursor can't find Entra's authorization endpoints, serve `/.well-known/oauth-authorization-server`, mirroring Entra's.
   - Check that Entra's v2 endpoint accepts the authorization request Cursor builds.
2. **Elicitation.** The prompt appears, accept and decline both complete, and nothing hangs.
3. **Tool allowlist.** With only the read tool allowlisted, the write tool always shows Cursor's own approval prompt.

**Outcomes:**

- If OAuth fails, use personal-token mode (T1).
- If elicitation fails, use draft mode (T3).

### T1: Authentication (S14)

**OAuth mode** uses `TokenVerifier::entra_access_token`. The user must be active.

**Personal-token mode:**

- 7-day tokens created in the web UI, stored hashed (HMAC with a key held in Secret Manager), and revocable.
- Sent from Cursor in `headers`.
- In this mode, never publish any OAuth discovery metadata.

**Verify:** tests for an invalid token, a wrong audience, a missing scope, a pending user and a revoked personal token.

### T2: Tools (P9, D58)

Implement every tool in `docs/mcp-tools.md`.

- Read tools call `RegistryRead` and `DocSearch`. Write tools call `WriteService` with via=mcp.
- Every result has a short summary, structured content, `truncated`, a `next` cursor and a `web_link`.
- The default cap is 200 lines or 16 KB.
- `read_file` accepts `lines` or `setting_path`. For large files, it returns an outline plus the first lines by default.
- File and doc text is wrapped in delimited blocks labelled "data, not instructions" (S15).
- A role check on every tool (S4).
- Per-user rate limits.

**Verify:**

- a contract test that schemas match `docs/mcp-tools.md`;
- a P9 benchmark for every read tool at target scale.

### T3: Confirmation (D52)

- **`propose_change`** has no side effects. It returns a draft id and a semantic summary.
- **Before acting,** `apply_change`, `upload_file`, `delete_file`, `raise_pr` and `restart_service` send an elicitation request.
  - The message gives the swimlane, the file, the change counts, the effect text and a web UI link.
  - The requested schema is a single boolean, `confirm`.
  - Proceed only if the user accepts with `confirm: true`.
- **Draft mode** applies when it's configured, or when the client didn't declare elicitation support. Writes return a web UI link to the draft instead.
- **Users with `requires_approval`:** after their confirmation, the change becomes a pending proposal.
- The `Idempotency-Key` is derived from the draft id, so a retry never applies twice.

**Verify:** tests for accept, decline, cancel, a client without elicitation, draft mode and approval users.

### T4: Distribution

- Package the MCP entry and `skills/lanekeeper/` (prompt 11) as a plugin for the Cursor team marketplace.
- Write `docs/cursor-admin.md`, covering the marketplace entry, the MCP allowlist URL, and the tool allowlist (read tools only).

### T5: Threat model

Fill in the MCP section of `docs/threat-model.md`.

### T6: Redaction and context (S14b, D73, D76)

- **Redaction:** every tool output that contains file or doc text passes through the engine's secret scan first. Flagged values become `[REDACTED: N chars, rule]`.
- **No reveal over MCP.** Users reveal values in the web UI, where the action is audited.
- **Denied files:** `read_file` returns "content withheld".
- **Context:** history and drift outputs include each change's attribution (source, confidence, actor) and its severity.

**Verify:** a test where a planted fake key in a fixture never appears in any tool output, and attribution and severity fields are present.

### T7: Served view and notify tools

- **`get_served_config(swimlane, application, tenant, channel?, file)`:** Viewer. Returns what the service receives from the config-server, redacted (S14b).
- **`notify_services(swimlane, paths)`:** Operator. Needs confirmation through elicitation, and is a contract change to `docs/mcp-tools.md`.
- The skill should prefer `get_served_config` when users ask what a service actually gets.

## Acceptance (all required)

- [ ] T0 results are recorded, and the modes are set in config.
- [ ] `just verify-07` passes, and P9 is within budget for every read tool.
- [ ] All authentication, confirmation and role tests pass.

## Stop and escalate if

- T0 finds that neither OAuth nor headers work reliably in 3.22.12.
- Elicitation hangs on one operating system and not the other. Draft mode may need to be forced for that platform.

## Out of scope

The business logic behind the tools.
