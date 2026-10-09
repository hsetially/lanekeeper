# 03a: Hub identity, sessions, authorization and audit

| | |
|---|---|
| **Wave** | 1 |
| **Depends on** | 01 |
| **You own** | `crates/hub-identity/**`, the identity and audit sections of `docs/threat-model.md` |
| **Interfaces** | Implements `TokenVerifier`, `Users` and `AuditLog`. Consumes `SecretSource`, `KmsSigner`, `Notifier`, `EventBus` and `Leases` through fakes. |
| **Security** | S1, S2, S3, S4, S7, S9, S10, S21, S22 |
| **Performance** | P7 (auth overhead under 2 ms per request), P12 |
| **Gate** | `just verify-03a` |

## Objective

This crate controls who can do what, and keeps a tamper-evident record of what they did. It provides:

- Entra sign-in for the web UI;
- token verification for MCP and for agent join;
- users, roles and access requests, held in the app;
- default-deny authorization;
- the hash-chained, KMS-checkpointed audit log.

## Tasks

### T1: Entra sign-in (backend-for-frontend; S1, S7)

**Flow.** Authorization code with PKCE, against the single-tenant v2 endpoint, using the openidconnect crate.

**Client authentication:**

- Prefer a federated credential: the client assertion is the projected Kubernetes service-account token that Entra trusts (Q25).
- Otherwise, use a client secret from `SecretSource`.

**Validation:** issuer, audience, `tid`, nonce, expiry, and signature against cached JWKS. Refresh the JWKS on an unknown key id, with rate limiting.

**Identity:** the user is identified by `tid` plus `oid`. Email and name are stored for display only.

**Verify:** tests against a mock OIDC provider cover a wrong `tid`, a replayed nonce, an expired token, an unknown key id, and both client-auth modes.

### T2: Users and access (S1, S4)

- **First sign-in:** create the user as `pending` with no role, create an access request, publish `AccessRequestCreated`, and call the notifier.
- **Bootstrap admins:** object ids listed in config become active admins at first sign-in.
- **Admin operations:** approve with a role, reject, change role or flag, disable. Each one is audited.
- **Safeguards:** the last active admin can't be demoted, and admins can't change their own role.
- **Sessions:** a role or status change ends every session the user has.
- **Onboarding flag:** `/me` returns `needs_github_token` for Editors and above, asking prompt 06's vault through a port.

**Verify:** integration tests against Postgres for each rule.

### T3: Sessions and CSRF (S2, S3)

**Sessions:**

- Server-side rows, with the session id hashed at rest.
- Cookie `__Host-lk_session`.
- Rotated at sign-in.
- Timeouts of 8 hours idle and 12 hours absolute.

**CSRF:** a double-submit token, plus a check that `Origin` matches the public origin on every non-GET.

**Latency:** a small in-process cache, holding entries for at most 30 seconds, keeps session lookups under 2 ms. Invalidation goes through `EventBus`, so a disabled user is cut off on every replica within 1 second.

**Verify:**

- tests for fixation, expiry, cross-origin POST rejection, and invalidation across replicas (two app instances sharing one database);
- a benchmark of the auth middleware.

### T4: Authorization (S4)

- **Extractors:** `RequireRole<R>` and `ActiveUser`.
- **`#[guarded(role = ...)]`:** a route-registration helper that records the required role.
- **Route-table test:** list every registered route and fail if any lacks a guard or has a role that disagrees with the OpenAPI `x-required-role`.
- **`TokenVerifier::entra_access_token`** for MCP: audience = the hub API, `scp` contains `mcp.access`, `tid` matches, and the user is active.
- **`TokenVerifier::google_id_token`** for agent join: Google issuer, the given audience, and the `email` returned for the caller to match against the swimlane.

**Verify:** the route-table test, plus token tests with forged, expired and wrong-audience tokens.

### T5: Audit chain and checkpoints (S9)

**Recording.** `AuditLog::record(tx, event)`:

1. Take a transaction-scoped advisory lock.
2. Read the head of the chain.
3. Compute `hash = SHA-256(prev_hash || canonical JSON)`, where canonical JSON means sorted keys and no whitespace.
4. Insert the event.

**Copy:** after the commit, emit the event, including its hash, as one structured log line for Cloud Logging.

**Checkpoints:** every hour, a job holding a lease does the following:

1. Sign the head hash with `KmsSigner`.
2. Write `{seq, head, signature}` to the locked GCS bucket.
3. Record it in `audit_checkpoints`.

**Verification:** every night, a leased job recomputes the whole chain and checks it against every checkpoint. On any mismatch, it publishes an alert, raises a metric and calls the notifier.

**Verify:**

- concurrent writers across two app instances produce one linear chain;
- a row tampered with in a test (trigger disabled) is detected;
- a checkpoint signature verifies with the public key;
- a benchmark shows `record` adds under 3 ms to a transaction.

### T6: Threat model

Fill in the identity and audit sections of `docs/threat-model.md`.

## Acceptance (all required)

- [ ] `just verify-03a` passes, with the auth middleware under 2 ms and audit `record` under 3 ms in `bench-check`.
- [ ] The route-table guard test exists. It's exported for the hub binary to run against its full route table.
- [ ] Chain verification catches tampering, and the checkpoint signatures verify.
- [ ] Every rule in S1–S4 has a named test.

## Stop and escalate if

- Entra rejects federated credentials from the GKE issuer in a test tenant (Q25).
- The advisory-lock chaining can't stay under 3 ms when writes come from multiple replicas.

## Out of scope

The agent gateway and Git (03b), and write paths (06).
