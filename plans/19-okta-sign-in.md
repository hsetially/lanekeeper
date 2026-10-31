# Plan 19: Okta as an alternative to Microsoft Entra ID

Status: DRAFT
Contract-change: needed
Extra-paths: `crates/hub-identity/**`, `crates/hub-mcp/src/auth/**`, `crates/hub-mcp/src/config.rs`, `crates/hub-mcp/t0/README.md`, `crates/hub-registry/src/attribution/os_login.rs`, `crates/hub-registry/src/tool_writes/mod.rs`, `crates/hub-registry/src/read/swimlanes.rs`, `crates/hub-writes/src/prs/job.rs`, `crates/hub-writes/src/prs/links.rs`, `crates/hub-writes/src/proposals/store.rs`, `crates/hub-writes/src/drafts.rs`, `crates/hub/src/config/identity.rs`, `crates/hub/src/config/mcp.rs`, `crates/hub/src/identity.rs`, `crates/hub/src/mcp.rs`, `crates/hub/src/headers.rs`, `crates/hub/tests/hardening.rs`, `web/src/features/access/SignInPage.tsx`, `web/src/auth/**`, `deploy/charts/hub/**`, `deploy/values/hub/**`, `deploy/policy/network.rego`, `deploy/policy/network_test.rego`, `deploy/INSTALL-GKE.md`, `design/DEVIATIONS.md`, `docs/threat-model.md`, `docs/decisions.md`, `docs/open-questions.md`
Prompt: none yet. The proposed work order is in "Work order" below; G1 asks whether to turn it into `prompts/19-okta-sign-in.md`.
Branch: agent/19-okta-sign-in

<!--
Header rules. A human approves the plan; the orchestrator then flips Status to APPROVED and the plan is frozen.
- Contract-change is "needed" for CC1 to CC5 below. They go first, as their own `contract-change` PR, before any task here starts.
- Extra-paths: there is no prompt, so no path is owned yet. Every path this plan touches is listed; each is justified in "Extra paths and contract changes". If the human makes this a prompt, the list becomes its "You own" row.
-->

## Summary for the human

1. **What this adds.** A deployment can choose Okta (OIDC, one Okta org, one custom authorization server) instead of Entra for web sign-in and for MCP OAuth. Exactly one provider per deployment, chosen by `LK_IDP=entra|okta`. Absent means `entra`, so every existing deployment keeps working with no change. Naming any `LK_OKTA_*` variable while the provider is Entra, or any `LK_ENTRA_*` variable while it is Okta, is a refused setting (exit 2), on the same pattern D52 uses for MCP auth modes.
2. **What does not change.** Roles, approvals, access requests, sessions, CSRF, the audit chain, the GitHub token vault, agent and sentinel identity (Google), Teams notifications, and personal-token MCP mode. The browser still holds only `__Host-lk_session`. Users are still identified by a stable, immutable IdP id and never by email.
3. **The one hard part is the user id.** Today `UserId` is two GUIDs (`tid:oid`). Okta has no tenant GUID, and its user ids are 20-character base62 strings (`00u…`). The Postgres columns are already `text` of length 1 to 64, so **no migration is needed**. The domain type is a contract change, though (CC1), and so is the OpenAPI pattern (CC3). Eight non-test call sites in `hub-registry`, `hub-writes` and `hub-identity` rebuild a `UserId` from row columns with `Guid::parse(..).ok()?`. Left alone, they would **silently drop** an Okta user's drafts, proposals, PR jobs, PR links, tool writes, baseline attribution and OS Login mapping. T1 replaces all of them with one constructor, and a source-scan test stops new ones.
4. **Security posture versus Entra.** It is the same or stronger, with three Okta-specific traps that the plan closes:
   - An Okta access token's `sub` is the user's *login*, which is mutable and often an email. The identity is `uid`; `sub` is never read.
   - The `default` authorization server and its `api://default` audience are shared by every app in an org. The audience `api://default` is refused, and the token's `cid` must be on an allowlist (Cursor's client id).
   - Org authorization server tokens (issuer with no `/oauth2/<id>`) are not meant to be validated by a resource server. They are refused for MCP.
5. **Client authentication.** The preferred mode is `private_key_jwt`, signed by a non-exportable Cloud KMS RSA key through the existing `KmsSigner` port. No shared secret exists anywhere. Okta has no equivalent of Entra's federated credential (Q25). A client secret in Secret Manager is the fallback, as S7 already allows. S7's wording needs a human decision (G5).
6. **Contract changes (CC1 to CC5).** All are additive or in-repo renames:
   - `UserId` realm and subject (CC1);
   - `TokenVerifier::entra_access_token` renamed to `user_access_token` (CC2);
   - the OpenAPI `UserId` pattern and provider-neutral auth text (CC3);
   - provider-neutral MCP auth text in `docs/mcp-tools.md` (CC4);
   - two benchmark budgets for bearer-token verification, which has no budget today (CC5).
7. **Needs a human before approval:** G1 (prompt or not), G2 (does the org have Okta API Access Management, which is needed for MCP OAuth), G3 (the user-id string form), G5 (S1, S7, S14 and S18 wording), G6 (the sign-in button deviates from the design handoff), and G9 (moving an existing deployment from Entra to Okta is out of scope).

## Docs read

- `AGENTS.md`: the protocol, ownership and contract paths, code rules 1 to 15, and the stack.
- `docs/decisions.md`:
  - D34 to D36 (Entra sign-in, app-held roles, approval on first sign-in);
  - D38 and D42 (the audit fields, the GitHub token at first sign-in);
  - D50 to D52 (Cursor, Entra OAuth, a fixed client id, the modes never combined);
  - D91 (the T0 table, `C1.*`, NOT RUN);
  - D126 ("a user whose id is not an Entra guid is left out");
  - D141 and D142 (the composed guard, no MCP variable for the tenant, audience or origin);
  - D144 (G9 settings rules, G26 Entra client authentication);
  - D145 (the KMS key rule).
- `docs/security.md`:
  - S1 (Entra, `tid` plus `oid`);
  - S2 to S4 (unchanged, re-checked);
  - S7 (federated credential, else a client secret);
  - S10 (no personal data in logs);
  - S11 (bounded inputs);
  - S14 (Entra access tokens for MCP, `tid` checked);
  - S18 (egress names Entra);
  - S21 and S22 (fuzzing every parser at a trust boundary);
  - S24 (the threat model section per component).
- `docs/performance.md`: P7, where authentication adds at most 2 ms (p95). P11 (startup).
- `docs/interfaces.md`: "Identity ports" (`TokenVerifier`, `Users`), `KmsSigner`, `SecretSource`.
- `docs/domain-model.md`: "Users and access" (a user is keyed by tenant id and object id; bootstrap admins are object ids).
- `docs/open-questions.md`:
  - Q6 and Q7 (answered: "Okta is replaced by Entra ID". This plan reopens that choice as an *option*, not a replacement);
  - Q21 (the app registrations);
  - Q25 (the federated credential);
  - Q32 (OS Login matched by email).
- `docs/threat-model.md`: the identity section (03a), the MCP section (07), the deployment section (09, the egress row and `EXPECTED_HOSTS`).
- The code that carries the Entra assumptions:
  - `crates/domain/src/ids.rs` (`Guid`, `UserId`) and `crates/domain/src/user.rs`;
  - `crates/ports/src/auth.rs`, `identity.rs`, `fakes/token_verifier.rs`, `conformance/auth.rs`, `conformance/sample.rs`;
  - `crates/hub-identity/src/config.rs`, `oidc/{jwt,login,discovery,client_auth,jwks}.rs`, `tokens/{mod,entra}.rs`, `users/mod.rs`;
  - `crates/hub-mcp/src/auth/{oauth,metadata}.rs`;
  - `crates/hub/src/config/{identity,mcp}.rs`;
  - `db/migrations/0001_init.sql` (`users`: `tid` and `oid` are `text`, 1 to 64);
  - `api/openapi.yaml` (`UserId` pattern, `/auth/*` descriptions);
  - `web/src/features/access/SignInPage.tsx`;
  - `design/handoff/screens/Lanekeeper Access Screen.dc.html` ("Sign in with Microsoft");
  - `deploy/charts/hub/templates/networkpolicy.yaml`, `deploy/charts/hub/values.yaml`, `deploy/policy/network.rego`, `crates/hub/tests/hardening.rs`.
- `prompts/03a-hub-identity-audit.md` and `plans/03a-hub-identity-audit.md`, for the original tasks and tests this plan mirrors.

## Work order (no prompt exists)

The table is proposed for the human. It becomes the prompt's metadata table if G1 says so.

| | |
|---|---|
| **Wave** | 3, after 18. It needs the composed hub. |
| **Depends on** | 03a, 07, 08, 09, 18 merged. The contract-change PR for CC1 to CC5 merged first. |
| **Owns** | The Extra-paths in the header. |
| **Interfaces** | `TokenVerifier` (renamed method, CC2), `Users`. Consumes `KmsSigner` and `SecretSource` unchanged. |
| **Security** | S1, S2, S4, S7, S10, S11, S14, S18, S21, S22, S24 |
| **Performance** | P7 (authentication at most 2 ms, p95). Two new budget entries for bearer verification (CC5). P11 (startup under 10 s with Okta discovery). |
| **Gate** | `just verify-19` (new Justfile recipe: the tests of every task below, `just fuzz-smoke` for the two changed targets, `cargo xtask bench-check`). |

## Approach

### A1. Choosing the provider

- `hub_identity::config::IdentityConfig` gets `provider: ProviderConfig`, either `Entra(EntraConfig)` (today's struct, unchanged) or `Okta(OktaConfig)`. Each validates itself once at start, and an error names the setting, never its value.
- `crates/hub/src/config/identity.rs` reads `LK_IDP`. When it is absent, the provider is `entra`.

**Okta variables:**

| Variable | Shape | Notes |
|---|---|---|
| `LK_OKTA_ISSUER` | `https://<okta-domain>/oauth2/<authServerId>` | A custom authorization server. An org issuer (no `/oauth2/` path) is refused while `LK_MCP_AUTH=oauth`, and allowed only with `personal_token` (G2). `/oauth2/default` is allowed but warned about (A4). |
| `LK_OKTA_ORG_ID` | `00o` plus 17 base62 characters | The realm half of every user id (A2). It is checked once at start against the org, if Okta's public org endpoint answers (G4). |
| `LK_OKTA_CLIENT_ID` | the web app's client id | |
| `LK_OKTA_CLIENT_AUTH` | `private_key_jwt` (default) or `secret` | |
| `LK_OKTA_CLIENT_KEY_KMS_REF` and `LK_OKTA_CLIENT_KEY_ID` | KMS key version, `kid` | When `private_key_jwt`. |
| `LK_OKTA_CLIENT_SECRET_REF` | Secret Manager name | When `secret`. |
| `LK_OKTA_COOKIE_KEY_REF` | Secret Manager name | The login-cookie key, as today's `LK_ENTRA_COOKIE_KEY_REF`. |
| `LK_OKTA_REDIRECT_PATH` | path, default `/auth/callback` | Same rules as Entra's. |
| `LK_OKTA_API_AUDIENCES` | 1 to 8 values, required in OAuth mode | `api://default` is refused (A4). |
| `LK_OKTA_MCP_CLIENT_IDS` | 1 to 8 client ids, required in OAuth mode | Allowed `cid` values (Cursor's native app). |

`LK_BOOTSTRAP_ADMINS` keeps its name. It is parsed as Entra object ids or Okta user ids, by provider.

There are no new MCP variables, which keeps D142: the MCP issuer, audience and scope derive from the identity settings, as they do for Entra today.

### A2. The user id (CC1)

- `UserId` becomes `{ realm: Realm, subject: Subject }`:
  - `Realm` is `Entra(Guid)` or `Okta(OktaId)`;
  - `Subject` is `Entra(Guid)` or `Okta(OktaId)`;
  - the realm kind and the subject kind always match. The constructor is `UserId::entra(tid, oid)` or `UserId::okta(org, uid)`, never a mix.
- `OktaId` is 1 to 64 ASCII alphanumerics, **case-sensitive** and never lower-cased (Okta ids are case-sensitive; `Guid` lower-cases). It contains no `-`, so its shape can never be a `Guid`, and parsing a string is unambiguous.
- The string form stays `<realm>:<subject>`. An Entra id is byte for byte what it is today, so stored rows, audit `actor_ref`, vault AAD and session rows are unchanged. G3 asks whether the human wants a visible prefix (`okta:<org>:<uid>`) instead.
- `UserId::from_columns(tid: &str, oid: &str) -> Option<UserId>` is the one way to rebuild an id from a row. It replaces the eight `Guid::parse(..).ok()?` pairs (T1).
- `UserId::tid()` and `oid()` are renamed `realm()` and `subject()`, returning the column text. The Entra-only accessors go away.
- The columns keep their names `tid`/`oid` (`user_tid`/`user_oid`, `*_by_tid`/`*_by_oid`). Renaming them would be an expand-then-contract migration across ten tables for no behavioural gain. `db/migrations/README.md` gets a sentence saying they hold realm and subject (doc only; it is a contract path, so it is part of CC1).
- **Postgres comparison.** `text =` under the database's deterministic collation is byte-exact. Two Okta ids that differ only in case are two users, which is what Okta means. A test proves the round trip of a mixed-case id through `users` and through a foreign key.

### A3. Web sign-in with Okta (authorization code and PKCE, the same flow)

- **Discovery.** `{issuer}/.well-known/openid-configuration`. `discovery::parse` already requires the document's issuer to equal the configured one and every endpoint to sit on the authority host. For Okta that host is the issuer's host (the Okta domain or the org's custom domain). The HTTPS client's host allowlist gets that one host.
- **ID token checks.** RS256 only, signature first, then:
  - `iss` equal to the issuer;
  - `aud` containing the client id;
  - `nonce` equal to the cookie's (constant time);
  - `exp`, and `iat` with 60 s skew. `nbf` is checked if present; Okta does not send it.
- **No `tid` check for Okta.** The issuer pins the org, and the realm comes from `LK_OKTA_ORG_ID`, not from the token.
- **Identity.** The ID token's `sub` (the Okta user id) must parse as an `OktaId`. `email`, `name` and `preferred_username` are for display only, as today.
- **Scopes requested.** `openid profile email`, unchanged.
- **Logout** ends the hub session only, as it does for Entra. Calling Okta's end-session endpoint is out of scope (G8).

### A4. MCP access tokens from Okta (S14)

A new `tokens/okta.rs` sits next to `tokens/entra.rs`, on the same `JwtVerifier`.

**The profile.** It accepts a token only if all of these hold:

| Claim | Check |
|---|---|
| `iss` | Equals the custom AS issuer. |
| `aud` | Is in `LK_OKTA_API_AUDIENCES`. |
| `scp` | Contains `mcp.access`. Okta writes `scp` as a JSON *array*, while Entra writes a space-separated *string*. `RawClaims.scp` accepts both, and `has_scope` matches whole entries either way. |
| `cid` | Is in `LK_OKTA_MCP_CLIENT_IDS`. This is defence in depth on top of the authorization server's access policy, and the counterpart of Entra pre-authorizing Cursor. |
| `uid` | Is present and parses as an `OktaId`. A client-credentials token has no `uid` and is refused, so no service token can act as a user. |
| `exp`, `iat` | Required, with 60 s skew. `nbf` is optional (Okta omits it). Entra's rule that `nbf` is required stays for Entra. |

- **`sub` is never read.** In an Okta access token it is the user's login (mutable, often an email). A test plants a token whose `sub` names another user and proves that the identity is `uid`.
- **`api://default` is refused** as a configured audience, at start. Any app in the org can get tokens for the default authorization server. A dedicated authorization server with its own audience is the documented setup. `/oauth2/default` as an *issuer* stays allowed, because `cid` and the custom scope still pin the client; it is logged as a warning at start (G7).
- **The rest is unchanged.** `McpAuthenticator` still calls `Users::require_active` after a valid token, so pending and disabled users get 403 (D36). `HubTokenVerifier` holds `UserTokens::Entra(..)` or `UserTokens::Okta(..)` and answers `user_access_token` (CC2). An ID token is still not an access token: an Okta ID token has `aud` equal to the client id, not an API audience, and no `scp`, so it is refused, and a test proves it. Google tokens and IdP tokens still do not cross.

### A5. Client authentication to Okta's token endpoint (S7)

**`private_key_jwt` (default).** The hub builds a client assertion (RFC 7523):

- **Header:** `alg` RS256, `kid` = `LK_OKTA_CLIENT_KEY_ID`.
- **Claims:**
  - `iss` and `sub` = client id;
  - `aud` = the token endpoint from discovery;
  - `iat`, and `exp` 300 s later;
  - `jti` = 128 random bits.

The hub hashes the signing input with SHA-256 and signs the digest through `KmsSigner::sign_digest`, with an RSA PKCS#1 v1.5 SHA-256 Cloud KMS key. **The private key never exists in the hub's memory.** That keeps D145 as it is: no new private key is held, and the source scan is unchanged.

The public key goes to Okta once, as a JWK. `hub okta-jwk` is a new subcommand: it reads the public key PEM from a file and prints the JWK with the `kid`. It is offline and makes no KMS call. The operator gets the PEM with `gcloud kms keys versions get-public-key`, documented in the install guide.

**`secret`.** `client_secret_post` from `SecretSource`, read on every exchange, the same as Entra's secret mode.

**Bounds.** The KMS call has the existing port's timeout. One sign-in costs one KMS call. That is not on a P# path, and its latency is measured in the evidence.

### A6. MCP discovery for Okta (hub-mcp)

- `OAuthSettings` becomes `Entra { tenant_id, api_audience, authority_host, mirror }` or `Okta { issuer, scope: "mcp.access", mirror }`.
- **Protected resource metadata.** `authorization_servers: [<okta issuer>]` and `scopes_supported: ["mcp.access", "offline_access"]`. Entra's request scope is `api://<id>/mcp.access`; Okta's custom scope is the bare name.
- **The optional mirror** (`LK_MCP_AS_MIRROR`) serves a static copy of the endpoints that Okta's metadata names: `{issuer}/v1/authorize`, `/v1/token`, `/v1/keys`. It **omits** `registration_endpoint`, so Cursor never attempts dynamic client registration.
- **T0.** `crates/hub-mcp/t0/README.md` gets the Okta rows `C1.discovery`, `C1.okta_accepts`, `C1.tool_call` and `C1.mirror_needed`. They are NOT RUN until a human runs Cursor 3.22.12 on Windows and macOS against a real Okta org (D91's rule: NOT RUN is not a pass). An RFC 8414 client builds the path `/.well-known/oauth-authorization-server/oauth2/<id>`, while Okta serves it after the issuer path. Whether Cursor handles that is exactly what `C1.discovery` and `C1.mirror_needed` observe.

### A7. Web app

- **The sign-in label.** The design handoff says "Sign in with Microsoft". The hub already writes `index.html` with a per-response nonce (`crates/hub/src/headers.rs`). It also writes `<meta name="lk-idp" content="entra|okta">`, a constant chosen at start that holds no user data. `SignInPage` reads that meta and shows "Sign in with Microsoft" or "Sign in with Okta", making no request (the page stays request-free, as it is today). This is a design deviation, so it gets an entry in `design/DEVIATIONS.md` for the human (G6).
- **Ids in the web app.** `names.ts` and `model.ts` only split the id on `:` and take the first eight characters of the subject, which works for Okta ids. Their comments become provider-neutral. MSW fixtures gain Okta-shaped ids, so the attribution tests cover both shapes.
- **The generated client.** orval is regenerated from CC3. The `UserId` pattern becomes the union of the two shapes.

### A8. Deployment

- **Chart values.** `identity.provider: entra|okta` and an `okta:` block (issuer, org id, client id, client auth, KMS key ref and `kid`, audiences, MCP client ids). The chart refuses to render with both blocks set, or with the chosen block missing (`fail`).
- **Egress (S18).** The FQDN egress policy names the Okta domain (the issuer's host) *instead of* `login.microsoftonline.com`. Exactly one IdP host is ever named.
- **Policies and tests.** `network.rego`, `network_test.rego` and `crates/hub/tests/hardening.rs::EXPECTED_HOSTS` change from a fixed list to "the fixed hosts plus exactly the chosen IdP's host". Planted cases cover:
  - an Entra host in an Okta deployment;
  - two IdP hosts;
  - a wildcard Okta host.
- **`deploy/INSTALL-GKE.md`** gets an "Okta" section:
  - a custom authorization server with a dedicated audience;
  - the scope `mcp.access`;
  - an access policy that admits only Cursor's client, with authorization code and refresh token grants;
  - a web app (OIDC, `private_key_jwt`, redirect `https://<hub>/auth/callback`);
  - a native app for Cursor (PKCE, no secret, redirect `http://localhost:8787/callback`, to be confirmed in T0);
  - assignment to the groups that may request access;
  - the KMS key and JWK steps.

## Tasks and commits

The contract-change PR for CC1 to CC5 merges first. Each task below is one commit, in order.

| Task | Commit message | Files and types created or changed | Verify |
|---|---|---|---|
| T1 | `19/T1: one constructor rebuilds a user id from its columns, for both providers` | Updates call sites to CC1: `hub-identity/src/users/mod.rs`, `hub-registry/src/{attribution/os_login.rs, tool_writes/mod.rs, read/swimlanes.rs}`, `hub-writes/src/{prs/job.rs, prs/links.rs, proposals/store.rs, drafts.rs}` use `UserId::from_columns`. Test fixtures that build ids keep `UserId::entra`. New `crates/hub-identity/tests/user_id_sites.rs`: a source scan that fails on `Guid::parse` next to a `tid`/`oid` column outside `crates/domain`. | `cargo test -p hub-identity -p hub-registry -p hub-writes` (real Postgres) |
| T2 | `19/T2: identity config chooses one provider; Okta settings are validated at start` | `hub-identity/src/config.rs`: `ProviderConfig`, `OktaConfig`, the new `ConfigError` variants, `bootstrap_set` by provider. | `cargo test -p hub-identity config::` |
| T3 | `19/T3: the JWT claims accept Okta's shapes; profiles say which identity claims they need` | `oidc/jwt.rs`: `scp` as string or array, `uid` and `cid` in `RawClaims` and `Claims`. `Profile.tenant` becomes `Profile.realm_check: Option<TidCheck>`, plus `require_nbf: bool`. The fuzz target `identity_jwt` drives both profiles; corpus entries are added. | `cargo test -p hub-identity oidc::jwt`; `just fuzz-smoke` (identity_jwt) |
| T4 | `19/T4: Okta web sign-in against a mock Okta provider` | `oidc/login.rs`: identity extraction by provider. `oidc/discovery.rs`: the Okta host. A mock Okta provider in `tests/support` serves discovery, `/v1/keys` and `/v1/token`, issues Okta-shaped tokens, and can rotate keys. | `cargo test -p hub-identity --test oidc_login` |
| T5 | `19/T5: private_key_jwt client authentication signed by KMS, and the secret fallback` | `oidc/client_auth.rs`: `ClientAuth::PrivateKeyJwt { key, kid }`, an assertion builder (pure, so tests and fuzz can drive it), `KmsSigner` signing. `hub okta-jwk` (in `crates/hub/src`, PEM to JWK, offline). | `cargo test -p hub-identity client_auth`; `cargo test -p hub okta_jwk` |
| T6 | `19/T6: Okta access tokens for MCP: uid identity, scp array, cid allowlist` | `tokens/okta.rs`, `tokens/mod.rs` (`UserTokens` enum, `user_access_token`), `McpAuthenticator` unchanged in logic. Benchmarks `hub-identity/bearer_verify_entra` and `bearer_verify_okta` (CC5). | `cargo test -p hub-identity tokens::`; `cargo test -p hub-identity --test corpus_replay`; `cargo xtask bench-check` |
| T7 | `19/T7: MCP discovery and challenge name the Okta authorization server` | `hub-mcp/src/auth/{oauth,metadata}.rs`: the `OAuthSettings` enum, the Okta mirror without `registration_endpoint`. `hub-mcp/src/config.rs`. The Okta rows in `t0/README.md`, NOT RUN. | `cargo test -p hub-mcp`; `just mcp-t0-status` (shows the Okta rows NOT RUN) |
| T8 | `19/T8: the hub reads LK_IDP and LK_OKTA_*, refuses mixed or unsafe settings, and composes Okta` | `hub/src/config/identity.rs`, `config/mcp.rs`, `identity.rs`, `mcp.rs`; `hub/tests/refuses_to_start.rs` and `identity_module.rs` cases; `hub/examples/support/dev_login.rs` gets an Okta dev path against the mock. | `cargo test -p hub` |
| T9 | `19/T9: the sign-in page names the configured provider` | `hub/src/headers.rs` (the `lk-idp` meta), `web/src/features/access/SignInPage.tsx`, a reader in `web/src/auth/`, Vitest and Playwright cases for both labels, MSW fixtures with Okta ids, `design/DEVIATIONS.md` entry. | `pnpm -C web test --run && pnpm -C web test:e2e -g sign-in` |
| T10 | `19/T10: the chart deploys Okta, and egress names exactly one identity provider` | `deploy/charts/hub/{values.yaml, templates/*}`, `deploy/values/hub/*.yaml` (an Okta example in `kind.yaml` only), `deploy/policy/network{,_test}.rego`, `crates/hub/tests/hardening.rs`, `deploy/INSTALL-GKE.md`. | `just verify-09` (helm and conftest), `cargo test -p hub --test hardening` |
| T11 | `19/T11: threat model, decision and open questions for Okta` | `docs/threat-model.md` (identity, MCP and deployment sections), `docs/decisions.md` (D146, proposed for the human), `docs/open-questions.md` (Q38 and Q39, and Q6/Q7 annotated). | `cargo test -p hub --test threat_model` (every named test exists) |

## Traceability

| Requirement | Task | Proof |
|---|---|---|
| One provider per deployment; mixed settings are refused, exit 2, naming the variable | T8 | `refuses_to_start::okta_and_entra_variables_together_are_refused`, `refuses_to_start::lk_idp_unknown_value_is_refused`, `identity_module::absent_lk_idp_is_entra_and_unchanged` |
| Existing Entra deployments are unchanged | T1, T8 | The whole existing Entra suite passes unchanged. `user_id::entra_string_form_is_byte_identical`, `identity_module::entra_settings_compose_as_before` |
| S1: Okta OIDC with PKCE; issuer, audience, nonce, expiry, signature against cached JWKS | T3, T4 | `oidc_login::okta_callback_creates_session`, `okta_callback_rejects_wrong_issuer`, `okta_callback_rejects_wrong_audience`, `okta_callback_rejects_replayed_nonce`, `okta_callback_rejects_expired`, `okta_callback_rejects_unknown_kid_after_one_rate_limited_refresh`, `okta_keys_rotate_without_restart` |
| S1: the user is identified by an immutable id, never by email | T1, T4, T6 | `user_id::okta_ids_are_case_sensitive_and_round_trip_through_postgres`, `okta_access_token_identity_is_uid_not_sub`, `okta_id_token_identity_is_sub_and_email_is_display_only` |
| Okta users' records are never dropped by a GUID-only parse | T1 | `user_id_sites::no_guid_parse_of_user_columns_outside_domain`; integration tests in hub-writes and hub-registry: `okta_author_draft_is_listed`, `okta_proposal_round_trips`, `okta_pr_job_resolves_its_user`, `okta_baseline_confirmer_is_named` |
| S1, D36: pending and disabled users get 403, including MCP | T6 | `mcp_okta_pending_user_is_refused`, `mcp_okta_disabled_user_is_refused` (the existing ones, run against the Okta profile) |
| S7: no shared secret where avoidable; secrets by name; `Secret<T>` | T5 | `client_auth::private_key_jwt_assertion_has_exact_claims_and_kid`, `client_auth::assertion_is_signed_through_kms_and_verifies_with_the_public_key`, `client_auth::secret_mode_reads_the_secret_on_every_exchange`, `client_auth::debug_shows_names_only` |
| D145: no new private key held by the hub | T5 | The existing scan `ca_key_use::a_private_key_must_never_be_built_from_bytes_in_the_hub` passes unchanged |
| S14: audience, scope `mcp.access`, realm, active user | T6 | `okta_access_token_valid`, `…_rejects_forged_signature`, `…_rejects_expired`, `…_rejects_wrong_audience`, `…_rejects_wrong_issuer`, `…_rejects_missing_mcp_scope`, `…_rejects_scope_as_substring`, `…_rejects_cid_not_allowlisted`, `…_rejects_missing_uid` (a client-credentials token), `…_rejects_oversize_token`, `okta_id_token_is_not_an_access_token`, `entra_and_okta_tokens_do_not_cross`, `hub_token_verifier_passes_port_conformance` (both providers) |
| S14: the org AS and `api://default` traps | T2, T8 | `config::okta_org_issuer_is_refused_in_oauth_mode`, `config::okta_default_audience_is_refused`, `refuses_to_start::okta_org_issuer_with_oauth_mcp_is_refused` |
| S14: the discovery documents name only this authorization server | T7 | `discovery_documents_are_the_metadata_of_this_okta_server`, `okta_mirror_has_no_registration_endpoint`, `pat_mode_publishes_no_discovery_metadata` (unchanged) |
| S4: default deny; the route table is unchanged | T8 | `route_table` and `assert_hub_route_table` pass unchanged; no route is added |
| S10: no token, claim value or email in logs | T4, T6 | `log_capture::okta_sign_in_logs_no_claim_values`, `log_capture::okta_token_refusal_logs_the_variant_only` |
| S11: bounded inputs | T3, T4 | `okta_access_token_rejects_oversize_token`, `jwt::scp_array_is_bounded` (at most 64 entries of at most 256 bytes) |
| S18: egress names exactly one IdP host | T10 | `network_test.rego::test_hub_egress_exact_okta`, `test_deny_entra_host_in_okta_deployment`, `test_deny_two_idp_hosts`, `test_deny_wildcard_okta_host`; `hardening.rs` planted cases |
| S21, S22: a fuzz target for every changed parser at a trust boundary | T3, CC1 | `identity_jwt` (Okta profile and the `scp` array), new `user_id` target in `fuzz/` (`UserId::from_str` and `from_columns`), `identity_cookie` (unchanged, still run) |
| S24: the threat model updated | T11 | `threat_model` test: every test it names exists |
| P7: authentication at most 2 ms | T6 | `P7.auth_middleware` and `_cold` unchanged (the cookie path does not touch the IdP); new `P7.bearer_verify_entra` and `P7.bearer_verify_okta` (CC5) |
| P11: startup under 10 s | T8 | `process::okta_start_with_mock_discovery_is_under_budget` (with the mock's latency) |
| The sign-in page names the provider without a request | T9 | `SignInPage.test.tsx`: "shows Okta when the meta says okta", "makes no request"; Playwright `sign-in.spec.ts` for both |

## Tests, benchmarks and fuzz targets

- **Unit tests.**
  - `OktaId` and `UserId` parsing: shape, case kept, the Entra form unchanged, a mixed realm and subject impossible.
  - Config validation: every new `ConfigError`.
  - Claim parsing: `scp` both shapes, `uid`, `cid`, `nbf` optional for Okta and required for Entra.
  - The assertion builder: exact claims, `exp - iat == 300`, `jti` is unique.
  - The metadata documents.
- **Property tests.**
  - `UserId` display then parse is the identity, for both kinds.
  - `from_columns(display parts)` round-trips.
  - No `OktaId` ever parses as a `Guid`, and the other way round.
  - `has_scope` on arrays matches only whole entries.
- **Golden tests (insta).**
  - The Okta protected resource metadata, the Okta mirror and the `WWW-Authenticate` challenge.
  - The `lk-idp` meta in `index.html`.
  - The rendered chart for an Okta deployment (`helm template` snapshot).
- **Integration tests against real Postgres (testcontainers).**
  - The Okta sign-in end to end against the mock provider: a user is created pending, an access request is created, a bootstrap admin is active.
  - A mixed-case Okta id through `users` and its foreign keys.
  - The T1 regression tests in hub-writes and hub-registry.
  - MCP `require_active` with Okta users.
- **Fuzz.**
  - `identity_jwt` extended with the Okta profile and array `scp` seeds.
  - New `fuzz/fuzz_targets/user_id.rs` (a parser at a trust boundary: admin routes take user ids in paths).
  - Both in `just fuzz-smoke`.
- **Benchmarks.** `hub-identity/bearer_verify_entra` and `bearer_verify_okta`: a warm key set, one valid token, verify and claim extraction. Registered in `perf/budgets.toml` (CC5) with p95 at most 0.5 ms. MCP calls are hot, and nothing benchmarks them today. These go in the 2 ms of P7.

## Extra paths and contract changes

### Contract changes (one `contract-change` PR, before T1, following the `lanekeeper-contract-change` skill)

- **CC1, `crates/domain`.**
  - `OktaId`, `Realm`, `Subject`;
  - `UserId::{entra, okta, from_columns, realm, subject}`; `UserId::new`, `tid` and `oid` are removed;
  - the doc comments of `Guid` and `UserRef` say "IdP" where they say "Entra".
  - Every in-repo caller is updated in the same PR, compile-only (test fixtures to `UserId::entra`). The behavioural call-site change is T1.
  - Also `db/migrations/README.md`, a doc sentence: the `tid`/`oid` columns hold realm and subject. **No migration.**
- **CC2, `crates/ports`.**
  - `TokenVerifier::entra_access_token` is renamed `user_access_token`.
  - The fake's `accept_entra` and `accept_expired_entra` become `accept_user` and `accept_expired_user`.
  - The conformance fixture gains an Okta sample.
  - `docs/interfaces.md` "Identity ports" is updated.
  - Callers are hub-identity, hub-mcp and the hub test rigs, all in-repo. The alternative is an additive method with a default that calls the old one, kept for one release. It is not proposed, because nothing outside the repo implements the trait.
- **CC3, `api/openapi.yaml`.**
  - `UserId.pattern` becomes the union of `<guid>:<guid>` and `<alnum 1-64>:<alnum 1-64>`.
  - The descriptions of `UserId`, `UserRef`, `/auth/login` and `/auth/callback` become provider-neutral.
  - orval is regenerated (`web/src/api/generated/**`).
  - Every existing value stays valid, so the change is backward compatible for clients.
- **CC4, `docs/mcp-tools.md`.** The authentication paragraph names both providers' token checks.
- **CC5, `perf/budgets.toml`.** Two additive entries, `P7.bearer_verify_entra` and `P7.bearer_verify_okta`.
- **Not a contract change.** `docs/security.md` and `docs/domain-model.md` are not contract paths, but they relax or extend S-requirements. AGENTS.md says such a change needs a human decision recorded in `decisions.md`. The proposed wording is in G5, and the edit is made only after that decision.

### Extra paths (no prompt owns them yet)

- **`crates/hub-identity/**`**: sign-in, the verifiers, client auth and config. This is 03a's crate. 03a is merged.
- **`crates/hub-mcp/src/auth/**`, `src/config.rs`, `t0/README.md`**: the MCP discovery and challenge for Okta, and the T0 rows.
- **The seven files in `hub-registry` and `hub-writes`**: only the `from_columns` substitution (T1), plus regression tests under those crates. Nothing else in them changes.
- **`crates/hub/src/{config/identity.rs, config/mcp.rs, identity.rs, mcp.rs, headers.rs}`, `crates/hub/tests/hardening.rs`**: composition, the settings, the meta tag, and the egress list.
- **`web/src/features/access/SignInPage.tsx`, `web/src/auth/**`**: the label.
- **`design/DEVIATIONS.md`**: the entry for the label (a human approves it). `design/handoff/**` is **not** touched.
- **`deploy/**` (listed paths)**: the chart, the policies and the install guide.
- **`docs/threat-model.md`, `docs/decisions.md`, `docs/open-questions.md`**: T11.

## Ambiguities and open questions

| # | Question | Where it came from | Proposed default (configurable) | Needs a human? |
|---|---|---|---|---|
| G1 | There is no prompt for this work. Should this plan become `prompts/19-okta-sign-in.md` (so `/run-prompt 19` and the guard hook can enforce ownership), or run as a one-off plan? | AGENTS.md protocol; `prompts/README.md` | Write the prompt from "Work order" and approve both together. | Yes |
| G2 | Does the org have Okta **API Access Management**, the paid SKU that provides custom authorization servers? Without it, Okta access tokens cannot be validated by the hub, and MCP OAuth cannot work. | Okta product rule; A4 | Web sign-in may use the org authorization server, and MCP then runs in personal-token mode (D52). The start refuses an org issuer together with `LK_MCP_AUTH=oauth`. | Yes |
| G3 | The string form of an Okta user id: `<orgId>:<userId>` (the shapes cannot collide with GUIDs, and there is no data change) or `okta:<orgId>:<userId>` (self-describing, but a three-part form)? | A2, CC1, CC3 | `<orgId>:<userId>`: the Entra form stays byte-identical, and the parse is unambiguous by shape. | Yes |
| G4 | Okta tokens carry no org id. Check `LK_OKTA_ORG_ID` at start against Okta's public org metadata (`https://<domain>/.well-known/okta-organization`)? Its availability on custom domains is to be confirmed in T0. | A1, A2 | Check it when the endpoint answers. Refuse to start on a mismatch. Log a warning and go on when the endpoint is absent (the issuer still pins the org). | No, unless the endpoint does not exist |
| G5 | Wording of S1, S7, S14 and S18 in `docs/security.md`. Proposed: S1 "Microsoft Entra ID (single tenant) **or Okta (one org, one custom authorization server)**; identify users by `tid`+`oid` (Entra) or org id plus `uid`/`sub` (Okta)". S7 "**Okta: `private_key_jwt` with a Cloud KMS key, else a client secret**". S14 "Okta access tokens checked for audience, `mcp.access` in `scp`, `cid` allowlist and `uid`". S18 "egress to the configured IdP". | AGENTS.md: S# change only by a recorded human decision | As written, recorded as D146. | Yes |
| G6 | The sign-in button for Okta deviates from the handoff ("Sign in with Microsoft"). Accept the `lk-idp` meta and the label "Sign in with Okta", or a neutral "Sign in" for both providers? | `design/handoff/screens/Lanekeeper Access Screen.dc.html`; A7 | A provider label through the meta, with an entry in `DEVIATIONS.md`. Entra stays pixel-identical. | Yes |
| G7 | Allow Okta's `default` authorization server as the issuer? | A4 | Allowed, with a start-up warning. The audience `api://default` is refused either way, and `cid` and `mcp.access` are required. | No |
| G8 | Should hub sign-out also end the Okta session (RP-initiated logout)? | A3; Entra does not today | No. Same as Entra. A follow-up for both providers if wanted. | No |
| G9 | Moving an existing deployment from Entra to Okta: every user would sign in as a new, pending user, and history stays attributed to the old Entra ids. Is a linking tool in scope? | A2 | Out of scope. Recorded as Q39 with a sketch: an admin-audited "link identity" action that maps old to new, which needs its own contract change. | Yes |
| G10 | Cursor's redirect URI and loopback behaviour with an Okta native app (`http://localhost:8787/callback`, as for Entra in Q21). | D50, D91 | As for Entra, confirmed by `C1.okta_accepts` in T0. Until then the Okta OAuth mode is unproven, and personal-token mode is the fallback (D52). | A human runs T0 |
| G11 | One prompt or two? The work crosses identity, MCP, the hub, web and deploy. | Size | One prompt: the change is one feature and the tasks are ordered; T9 and T10 could split off as `19b` if the human prefers smaller PRs. | No |
| G12 | New open questions for the Okta admins: Q38, "Okta configuration": the authorization server, scope, access policy, two apps, group assignment, and whether a custom domain is used. It replaces Q21 for Okta deployments. | A8 | Added in T11. | No (an Okta admin answers it later) |

## Risks

- **Silent data loss for Okta users (highest).** A GUID-only parse of user columns drops records without an error. T1 fixes all eight known sites. The source scan stops new ones. The regression tests prove the drafts, proposals, PR jobs and attribution paths with Okta ids against real Postgres.
- **Taking the wrong claim as identity.** Okta's access-token `sub` is a mutable login. Only `uid` (access token) and `sub` (ID token) are read, each by a named test, and email never enters identity. Both are the Okta user id.
- **Accepting tokens minted for another app in the org.** These four checks are all needed:
  - an issuer that pins one authorization server;
  - an audience that is not `api://default`;
  - `mcp.access` as a whole `scp` entry;
  - `cid` on the allowlist;
  - plus `uid` present, so client-credentials tokens are refused.
- **The two providers' tokens crossing.** One provider per process. The verifier holds exactly one profile, and an Entra token at an Okta hub (and the reverse) is refused by the issuer check. A test sends each.
- **Client credential exposure.** `private_key_jwt` through KMS means there is no secret to leak. The secret fallback reuses the existing `SecretSource` path (`Secret<T>`, read each time, never logged).
- **Performance.** The cookie path (P7) is untouched. Bearer verification gets its first budget (CC5). Okta's key endpoint is rate limited, and the existing single-flight, at-most-one-refresh-every-30-s rule covers it. Startup does one more discovery fetch, with the existing 5 s timeout, inside P11.
- **Unverified against the real service.** The mock Okta provider is not Okta. The token shapes, key rotation, the org endpoint (G4), the `private_key_jwt` acceptance and Cursor's behaviour are unproven until a real Okta org is used. T0's Okta rows and the go-live sign-off (S24) hold that. The threat model records it as a residual, as it does for Entra today.
