# T0 runbook: does Cursor 3.22.12 work with the MCP server we are going to build?

Prompt 07, task T0. This is the human part. It takes about a day and needs Cursor 3.22.12 on Windows **and** on macOS. An agent
cannot do it: it has no Cursor and no Entra app registration (Q21). Until you have recorded results, every cell of the table in
D91 (`docs/decisions.md`) says `NOT RUN`, `just mcp-t0-status` says so, and `NOT RUN` is not a pass.

You test three things, and write down five more that the design depends on:

1. **Entra OAuth** with a fixed client id (`C1.*`), and the personal-token fallback (`C1.headers`).
2. **Elicitation**: the confirmation prompt appears, accept and decline both complete, and nothing hangs (`C2.*`).
3. **The tool allowlist**: with only the read tool allowlisted, the write tool always shows Cursor's own approval prompt (`C3.*`).
4. **Observations** (`O.*`) that decide the content mode, the session handling and the log filter.

What the results decide (prompt 07):

- OAuth fails: use personal-token mode (`LK_MCP_AUTH=personal_token`).
- Elicitation fails: use draft mode (`LK_MCP_CONFIRM=draft`). If it hangs on one operating system and not the other, **stop and
  escalate**: draft mode may have to be forced for that platform.
- Neither OAuth nor headers work reliably in 3.22.12: **stop and escalate**.

## What you need

- Cursor **3.22.12** on Windows and on macOS (Help, About shows the build). Another build does not count.
- The Entra registrations of Q21: the hub API (exposes `mcp.access`, v2 access tokens, so `accessTokenAcceptedVersion` is 2) and
  the Cursor public client (redirect `http://localhost:8787/callback`, delegated permission to the hub API scope). Write down
  the tenant id, the hub API's Application ID URI (`api://<hub-api>`) and the Cursor client's Application (client) id.
- Rust 1.88 to run the stub (`rust-toolchain.toml` pins it), or a build of `t0_stub` made for you.

**Port 8787 belongs to Cursor's sign-in.** Q21 gives the Cursor client the redirect `http://localhost:8787/callback`, so the stub
listens on **8799**. Do not move it to 8787.

## About the stub

`crates/hub-mcp/examples/t0_stub.rs` is a tiny MCP server with two tools:

- `t0_read` (read only). Its answer has two markers: `CONTENT-MARKER=alpha-7f3a` in `content`, and `bravo-91c2` only in
  `structuredContent`. With `{"mode": "error"}` it returns an error result whose `structuredContent` does not match its
  `outputSchema`.
- `t0_write` (not read only). It asks you to confirm, through MCP elicitation, with a form of one boolean, `confirm`. **It writes
  nothing, whatever you answer.** Without elicitation support it returns a link and does nothing (draft mode).

**It does not verify tokens.** In OAuth mode it reads the claims `aud`, `scp`, `iss`, `tid` and `ver` of the bearer token, and
logs those names and values and nothing else, because its job is to show what Cursor and Entra send. So it listens on loopback
only and refuses any other address unless you pass `--allow-non-loopback`. Do not do that. Stop the stub when you are done.

It logs to stderr, one line per event:

| Target | What it shows |
|---|---|
| `t0::request` | method, path, status, `origin`, `user_agent`, `mcp_protocol_version`, whether a session id was sent. Never the `Authorization` header. |
| `t0::claims` | `aud`, `scp`, `iss`, `tid`, `ver`, and three booleans saying whether they match the configuration. Never the token. |
| `t0::initialize` | the client's name and version, the protocol version it asked for and the one negotiated, and whether it declares elicitation. |
| `t0::write` | how each `t0_write` ended: `Confirmed`, `Declined`, `Cancelled`, `TimedOut`, `DraftLink` or `Aborted`. Read this, not the model's paraphrase. |

The lines hold no secret (they are claim values such as a tenant id and a scope name). You may paste them into the PR.

## Start the stub, OAuth mode

macOS or Linux:

```sh
export LK_MCP_AUTH=oauth
export LK_T0_TENANT_ID=<tenant id>
export LK_T0_API_AUDIENCE=api://<hub-api>
cargo run -p hub-mcp --example t0_stub
```

Windows PowerShell:

```powershell
$env:LK_MCP_AUTH = "oauth"
$env:LK_T0_TENANT_ID = "<tenant id>"
$env:LK_T0_API_AUDIENCE = "api://<hub-api>"
cargo run -p hub-mcp --example t0_stub
```

It says `t0 stub listening` with the address. The other settings are in the header of the example (`LK_T0_ELICIT_TIMEOUT_SECS`,
`LK_T0_AS_MIRROR`, `LK_MCP_CONFIRM`, and so on). A value it does not accept stops it with a message that names the variable.

Cursor's `mcp.json` (the prompt names `url`, `auth.CLIENT_ID` and `auth.scopes`; the exact shape in 3.22.12 could not be checked
from the sandbox this runbook was written in, so if Cursor wants something else, write that in the `O.*` rows):

```json
{
  "mcpServers": {
    "lanekeeper-t0": {
      "url": "http://127.0.0.1:8799/mcp",
      "auth": {
        "CLIENT_ID": "<the Cursor client's Application (client) id>",
        "scopes": ["api://<hub-api>/mcp.access", "offline_access"]
      }
    }
  }
}
```

## Check 1: Entra OAuth

1. Add the server in Cursor and let it connect. Expect Cursor to say the server needs you to sign in. The stub log shows a
   request to `/mcp` with status 401. That 401 carries `resource_metadata`, which is how Cursor finds the rest.
2. **`C1.discovery`.** Cursor asks for `/.well-known/oauth-protected-resource` (the log shows it with status 200) and opens
   Entra's sign-in page **without you typing a URL**. `PASS` if so.
3. If Cursor never asks for it, or cannot find Entra's endpoints and stops, stop the stub and start it again with
   `LK_T0_AS_MIRROR=true`, which also serves `/.well-known/oauth-authorization-server`, a static copy of Entra's endpoints.
   Retry. **`C1.mirror_needed`** is `yes` if it only works with the mirror, `no` if it works without, or what you saw. Compare
   the mirror with Entra's live `https://login.microsoftonline.com/<tenant>/v2.0/.well-known/openid-configuration`
   (issuer, `authorization_endpoint`, `token_endpoint`, `jwks_uri`); a difference goes in the cell.
4. **`C1.entra_accepts`.** Entra accepts the authorization request Cursor builds (its v2 endpoint, your client id, the scopes)
   and the sign-in completes and returns to Cursor. `FAIL: <error>` with the `AADSTS` code if Entra refuses.
5. **`C1.tool_call`.** Ask the agent to call `t0_read`. It runs, and the stub logs a `t0::claims` line. Look at it: `ver=2.0`
   and `aud` equal to what the hub's verifier (prompt 03a) is configured to expect; `scp` contains `mcp.access`; `tid` is
   your tenant. The three booleans at the end say whether they match this stub's settings. A token with `ver=1.0` or an `aud`
   of the wrong form is a `FAIL: <what>`: it means the hub API registration is not v2. Paste the line in the PR.
6. **`C1.headers`** (do this too, it is the fallback). Stop the stub and run it in personal-token mode:

   ```sh
   export LK_MCP_AUTH=personal_token
   export LK_T0_PAT=<any 16 to 256 printable characters, no spaces>
   unset LK_T0_TENANT_ID LK_T0_API_AUDIENCE LK_T0_AS_MIRROR
   cargo run -p hub-mcp --example t0_stub
   ```

   (PowerShell: set `$env:LK_MCP_AUTH` and `$env:LK_T0_PAT`, and `Remove-Item Env:LK_T0_TENANT_ID` and the other two.)
   Remove the `auth` block from the server entry in `mcp.json` and use headers instead, with the token in an environment
   variable that Cursor was started with:

   ```json
   { "mcpServers": { "lanekeeper-t0": { "url": "http://127.0.0.1:8799/mcp", "headers": { "Authorization": "Bearer ${env:LK_T0_PAT}" } } } }
   ```

   `PASS` if `t0_read` runs and the log shows **no** request for `/.well-known/...` (this mode publishes no discovery
   metadata). Never put `auth` and `headers` in one entry: the two modes are never combined (D52).

## Check 2: elicitation

Restart the stub in OAuth mode (or personal-token mode, whichever you will use) with a short wait, so that the last step does not
take two minutes:

```sh
export LK_T0_ELICIT_TIMEOUT_SECS=30
```

Ask the agent to call `t0_write` with a note ("first"). Because `t0_write` is not allowlisted, Cursor first shows **its own**
approval prompt (that is check 3); approve it. Then:

1. **`C2.prompt`.** Cursor shows the stub's question: "Lanekeeper T0 stub: approve the test action "first" (supplied by the
   assistant)? Nothing is written anywhere; ..." with one yes/no choice. `PASS` if it appears.
2. **`C2.accept`.** Accept with the choice set to yes. The call completes; `t0::write` logs `Confirmed`.
3. **`C2.decline`.** Call it again and decline. It completes; `t0::write` logs `Declined`.
4. **`C2.cancel`.** Call it again and dismiss or cancel the prompt. It completes; `t0::write` logs `Cancelled`. (If Cursor sends
   `decline` for a dismissal, that is `Declined`; write it in the cell.)
5. **`C2.no_hang`.** Call it again and do not answer. After 30 seconds `t0::write` logs `TimedOut`, the call ends, and Cursor is
   not stuck (you can send another message, and the prompt is gone or disabled).

Do each of the four on **both** operating systems. A prompt that never appears, or a call that never ends, on one of them and
not the other is the stop condition. Write `FAIL: <what happened>`.

Also try draft mode once (`export LK_MCP_CONFIRM=draft`): `t0_write` should answer `DRAFT_LINK` without asking. That is the
fallback if elicitation fails.

## Check 3: the tool allowlist

In Cursor's MCP settings, allowlist (auto-run) **only** `t0_read` for this server. The menu names may differ in 3.22.12.

- **`C3.read_runs`.** Ask the agent to call `t0_read`. It runs without Cursor's own approval prompt.
- **`C3.write_prompts`.** Ask it to call `t0_write`. Cursor's own approval prompt appears, **every time**, also right after you
  approved it once. (The server's question comes after it; they are different prompts.)

## Observations

| Row | How to find out |
|---|---|
| `O.protocol_version` | The `t0::initialize` line: `requested_protocol_version` and `negotiated_protocol_version`. Also say whether the log shows an `initialize` request (the usual case). A client that never sends `initialize` and uses `server/discover` is served without a session, and the confirmation design needs a different mechanism. Write it down if you see it. |
| `O.elicitation_capability` | The same line: `declares_elicitation=true` or `false`; if you can tell, form or url. |
| `O.structured_content` | Ask: "Call t0_read and list every marker you can see, word for word." Both `alpha-7f3a` and `bravo-91c2`: write `content and structuredContent`. Only `alpha-7f3a`: write `content only`. **If `content only`, set `D.content` to `data_block`**, otherwise `summary`. |
| `O.error_vs_output_schema` | Ask: "Call t0_read with mode error and tell me exactly what it returned." If you see the message, `accepted`. If Cursor refuses it or reports a schema error, say how. |
| `O.session_404` | With a working session, stop the stub with Ctrl-C and start it again. Ask the agent to call `t0_read` again. The old session id gets a 404 (see `t0::request`). Does Cursor reconnect on its own, ask you to reconnect, or fail? |
| `O.origin_header` | The `t0::request` lines: `origin=None` (not sent) or the value. |
| `O.refresh` | OAuth mode only. Leave Cursor open until the access token has expired (about an hour for Entra by default), then call `t0_read`. A new `t0::claims` line with no browser sign-in means it refreshed silently. |

## Record the results

1. Edit the table in D91 of `docs/decisions.md`, in place, one cell per row and operating system. Nothing else in the file
   changes. The values are:
   - `META.cursor_build` exactly `3.22.12` (or `3.22.12 (note)`), `META.recorded_by` your name, `META.date` as `YYYY-MM-DD`;
   - a check (`C1.discovery`, `C1.entra_accepts`, `C1.tool_call`, `C1.headers`, every `C2.*` and `C3.*`) is `PASS`, `FAIL`
     or `FAIL: reason`;
   - an observation is plain text, at most 200 characters, never `NOT RUN` in other spelling;
   - a decision is a mode: `D.auth` `oauth` or `personal_token`, `D.confirm` `elicitation` or `draft`, `D.content` `summary` or
     `data_block`. Choose them from the outcome rules above, per operating system.
2. Run `just mcp-t0-status`. It prints what is recorded and exits 0. A table it cannot read is an error that names the row.
3. Run `just mcp-t0-status --strict` before go-live. It exits 1 until every cell is filled, the build is 3.22.12 and the
   decisions follow the rules (a failed OAuth check needs `D.auth` `personal_token`; a failed elicitation check needs `D.confirm`
   `draft`; failed OAuth **and** failed headers is a stop).
4. Set the chosen modes in the deployment (`LK_MCP_AUTH`, `LK_MCP_CONFIRM`, `LK_MCP_CONTENT`; prompt 09 carries them to the
   chart). The defaults are `oauth`, `elicitation` and `summary`.

## Row by row

| Row | Kind | Meaning |
|---|---|---|
| `META.cursor_build` | text | The build. Strict mode wants `3.22.12`. |
| `META.recorded_by` | text | Who ran it. |
| `META.date` | date | `YYYY-MM-DD`. |
| `C1.discovery` | check | Cursor finds Entra's sign-in from the 401's `resource_metadata`. |
| `C1.mirror_needed` | text | Whether the `/.well-known/oauth-authorization-server` mirror was needed. |
| `C1.entra_accepts` | check | Entra's v2 endpoint accepts Cursor's authorization request. |
| `C1.tool_call` | check | `t0_read` runs after sign-in; the claims are as the hub's verifier expects. |
| `C1.headers` | check | Personal-token mode with `headers` works. |
| `C2.prompt` | check | The elicitation prompt appears. |
| `C2.accept` | check | Accept completes. |
| `C2.decline` | check | Decline completes. |
| `C2.cancel` | check | Cancel or dismiss completes. |
| `C2.no_hang` | check | An unanswered prompt times out and nothing hangs. |
| `C3.read_runs` | check | An allowlisted read tool runs without Cursor's prompt. |
| `C3.write_prompts` | check | A write tool that is not allowlisted always shows Cursor's prompt. |
| `O.protocol_version` | text | Requested and negotiated protocol version, `initialize` or `server/discover`. |
| `O.elicitation_capability` | text | What Cursor declares. |
| `O.structured_content` | text | Whether the model sees `structuredContent`. Decides `D.content`. |
| `O.error_vs_output_schema` | text | Whether an error result that does not match `outputSchema` is accepted. |
| `O.session_404` | text | What Cursor does when its session id gets a 404. |
| `O.origin_header` | text | Whether Cursor sends `Origin`. |
| `O.refresh` | text | Silent token refresh. |
| `D.auth` | decision | The authentication mode for this operating system. |
| `D.confirm` | decision | The confirmation mode for this operating system. |
| `D.content` | decision | The content mode for this operating system. |

## What the agent part has already shown (server side, not Cursor)

`cargo test -p hub-mcp --tests` drives the very stub you run, in process, with no sockets and a paused clock:

| File | What it proves |
|---|---|
| `tests/stub_protocol.rs` | A 401 with `resource_metadata` and the scope; the protected-resource metadata points at the Entra tenant; the mirror is served only when switched on; personal-token mode publishes no discovery metadata and refuses a JWT (and OAuth mode refuses the token); the claims are logged by name and the token never is; an accept with `confirm: true` is the only thing that proceeds; decline, cancel, a malformed answer, a client without elicitation, draft mode and a timeout write nothing; the stub refuses a non-loopback address. |
| `tests/rmcp_spike.rs` | The three rmcp 3.5.0 APIs the design leans on (plan A19): the request's `Parts` reach the handler and a session is bound to one caller; a question that is open when the call is cancelled, the session ends or the server stops is withdrawn; a session manager can be wrapped and capped. And the findings below. |
| `tests/config.rs` | The three switches and their defaults; `oauth,personal_token` is an error. |
| `tests/t0_status.rs` | `just mcp-t0-status`: `NOT RUN` is never a pass; strict mode; an unreadable table is an error. |

Findings for T1 to T3, from `tests/rmcp_spike.rs`:

1. An `initialize` request always gets a classic stateful session, whatever protocol version it names. rmcp 3.5.0 also speaks
   2026-07-28 (no sessions; questions are multi-round-trip requests), which a client reaches only by skipping `initialize`.
   `O.protocol_version` tells you whether Cursor does.
2. A `GET` (the server-to-client stream) and a `DELETE` on a session never reach the handler, so the binding of a session to its
   user must be checked in the HTTP layer too, for every method.
3. rmcp's own cancellation token ends its streams but does not wake a handler that waits for an answer: a confirmation needs its
   own stop signal and always a timeout. Ending a session or a cancel from the client does wake it.
4. rmcp sends the withdrawal of a question on the standalone stream, not on the stream of the call.
5. rmcp logs whole requests and results at `debug` and notifications at `info`: **the hub's log filter must keep `rmcp` at
   `warn`**, or file content reaches the logs (S10, S21).
6. The session manager sees no HTTP request, so a per-user session cap is a layer in front of it; a cap on all sessions fits
   inside the manager.
7. The keep-alive timer of `sse-stream` is re-armed from the real clock, so a test on a paused clock turns the keep-alive off.

## Troubleshooting

- *Cursor says the sign-in page cannot be reached or loops.* Look at the `t0::request` lines: which paths did it ask for, and
  with what status? A 404 for `/.well-known/oauth-authorization-server` means it wants the mirror.
- *`AADSTS50011` (redirect URI mismatch) or `AADSTS700016`.* The Cursor client registration (Q21) is wrong; fix the registration,
  not the stub.
- *The stub exits with `refusing to listen`.* The address is not loopback. Use the default.
- *"address already in use".* Something else holds port 8799 (not 8787, which the stub never uses).
- *Cursor never shows the server's question.* Look at `t0::initialize`: `declares_elicitation=false` means the client did not
  declare it, and the stub answers with a draft link instead (that is the designed fallback).
