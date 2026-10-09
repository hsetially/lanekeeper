# 06: Write paths, approvals, token vault, PRs and notifications

| | |
|---|---|
| **Wave** | 2. Can start against fakes. |
| **Depends on** | 01. Integrates with 03a, 03b, 04 and 05. |
| **You own** | `crates/hub-writes/**`, the write and vault sections of `docs/threat-model.md` |
| **Interfaces** | Implements `WriteService` and the Teams `Notifier`. Consumes `AgentGateway`, `AuditLog`, `Users`, `KmsEnvelope`, `SecretSource`, `EventBus`, `BlobStore`, the engine and the registry's tool-write registration. |
| **Security** | S4, S8, S9, S10, S11, S13, S21, S22 |
| **Performance** | P8, P12 |
| **Gate** | `just verify-06` |

## Objective

Every change is:

- authorised;
- validated;
- idempotent;
- byte-faithful;
- checked against the expected hash;
- audited in the same transaction;
- either applied within 1.5 seconds or routed to approval.

## Tasks

### T1: Write pipeline (P8, D68)

Every `WriteService` method goes through these steps, in order:

1. **Active user and role check:** Editor for file actions and PRs, Operator for restarts.
2. **`Idempotency-Key` lookup.** The same key with the same request hash returns the stored response. The same key with a different request hash returns 422.
3. **Editing lock** (file actions only): the swimlane's baseline must be confirmed. Otherwise return 423.
4. **Validation** with the engine. Parse errors block; duplicate keys are warnings. The value-based secret scan blocks the save and reports line numbers only.
5. **Approval.** If the user has `requires_approval`, create a pending proposal (72-hour expiry) and notify admins.
6. **Execute.** Otherwise, carry out the action, write the audit event in the same transaction, publish events and return the outcome.

**Verify:**

- a test matrix: every action × Viewer, pending user, approval user and normal user;
- idempotency replay tests;
- an end-to-end P8 benchmark through a fake agent.

### T2: Byte faithfulness

- Before writing, convert incoming text to the target file's line-ending style with `apply_eol`, and keep any BOM.
- Change the style only when the request sets `change_eol` explicitly.
- New files take the style most common in their folder.
- Binary content is written untouched.

**Verify:** editing a CRLF file from LF input produces a stored diff and a PR diff containing only the changed lines.

### T3: Edit, upload, delete, revert and restart

**Edit:** register the expected new hash with the registry, then send `WriteFile` with the expected current hash. A conflict returns 409 with the current hash and a diff hint.

**Upload and delete:**

- Both need a valid effect-preview id, less than 10 minutes old and bound to the same user, path and hash.
- Uploads may be binary, within the size limits.
- Deleted content stays in the blob store.

**Revert:** write a stored version, with the current hash as the precondition.

**Restart:** sent through the agent gateway.

- A change to a root file offers "restart all services in this swimlane", with a confirmation listing every service.
- At most one restart per service at a time.

**Verify:** tests for each action, including 409, 423 and preview misuse.

### T4: Approvals

- Only admins approve, and nobody can approve their own proposal.
- On approval, re-check the current hash. If it has changed, mark the proposal stale.
- A leased job marks expired proposals.
- The audit event records both the author and the approver.
- Proposal changes publish `ProposalChanged`, which drives the UI over SSE.

**Verify:** the rule tests.

### T5: GitHub token vault (S8)

**Saving (`PUT /me/github-token`):**

1. Accept only fine-grained tokens (`github_pat_` prefix). Reject classic tokens, with a message that cites the policy.
2. Call GitHub's `GET /user`.
3. Check the token can push to both repos.
4. Link the GitHub login to the user on first save. Later saves must match, unless an admin resets the link.

**Encryption:**

- A fresh data key per token, with AES-256-GCM.
- The user id and credential id are the associated data.
- The data key is wrapped by `KmsEnvelope`.

**Use:** decrypt only inside the owner's request, keep the plaintext in `Secret<String>`, and zeroize it after use.

**Lifecycle:**

- Track the expiry and warn when it's under 7 days away.
- A leased nightly job deletes credentials of users not seen for 30 days.

**Verify:**

- a test that swapping the associated data fails decryption;
- the token never appears in logs, errors, audit events or responses (log capture test);
- classic tokens are rejected;
- a mismatched login is refused.

### T6: PRs

**Reverse mapping.** Map the NFS path back with the engine. For example, `tx-infinity-api/tx-infinity-core-sit1.yml` becomes `data/config/tx-infinity-api/tx-infinity-core.yml` on branch `sit1`.

**Explicit target:**

- **Tenant:** the mapped path on the chosen branch, defaulting to the deployed branch. If the file is new on that branch, include the fork warning.
- **Base:** `config/<rel>` on the default branch, with the note that every swimlane picks it up. If the NFS file is a tenant file, add an extra warning: this copies one tenant's content into base.

**Creating the PR:**

1. Create the branch `lanekeeper/<swimlane>/<short id>`.
2. Commit with the Git Data API, keeping the repo file's line endings.
3. Open the PR, with the swimlane, hashes, user and audit id in its body.
4. Record the PR URL in the audit event.

All GitHub calls have timeouts and handle rate-limit backoff.

**Verify:** tests against a mocked GitHub API for a tenant target with an existing file, a tenant target with a new file, and a base target.

### T7: Teams notifier

- Uses Workflows webhooks (Q9), with the URLs read through `SecretSource`.
- Adaptive Cards for access requests, proposals, drift alerts, overwritten changes and audit-chain alerts.
- Retries with backoff and a circuit breaker.
- A failure never fails the action that triggered it.

**Verify:** tests with a fake webhook server for retries and the breaker.

### T8: Threat model

Fill in the write and vault sections of `docs/threat-model.md`.

### T9: PR links and PR jobs (D77)

- **Before creating a PR:** if an open PR already exists for the same (swimlane, path, NFS hash), return it, with a 409 and its link.
- **Creation** runs as a `pr_jobs` state machine: `pending → preparing → committing → opening → completed | failed`.
  - It's resumable after a crash, and idempotent through the job id.
  - Progress is published with `PrJobProgress`.
  - The API returns 202 with the job id.
- **Link state** is updated from `PrStateChanged`. A merged PR marks the related drift as "fix merged; awaiting sync".

**Verify:** duplicate-PR blocking, a crash resumed mid-job, and webhook-driven state changes.

### T10: Masked values and the reveal action (D79)

- `POST .../files/reveal` returns the unmasked value for one flagged line. It's allowed for Editors and above, rate-limited, and writes an audit event with the user, file, line and reason.
- Writes to denied paths are refused.

**Verify:** a Viewer gets 403, and every reveal is audited.

### T11: Notify services, and config-server restarts (D85, D87)

**`WriteService::notify(paths)`:**

- Requires Operator, and goes through approval for users with `requires_approval`.
- Sends `NotifyConfigServer` through the agent and writes an audit event.
- Updates pickup states, so that resource files become `live` once notified and property sources become `live` for clients with notifications enabled.

**After each write,** the response offers "Notify services". If auto-notify is on for the swimlane, the notification is sent automatically after the write commits, batched within 2 seconds.

**Config-server restarts.** When check C11 is open, Operators get a "Restart config-server" action. It uses the existing restart path, and its confirmation names the services affected while the config-server is unavailable.

**Verify:**

- notify sends exactly the written paths, relative to the config root;
- auto-notify batches the paths from a multi-file write into one call;
- notify is audited and requires Operator.

## Acceptance (all required)

- [ ] `just verify-06` passes, and P8 is within budget.
- [ ] The full action × user matrix passes, and idempotency, 409 and 423 are tested.
- [ ] The vault's associated-data binding and zeroization are tested, and the leak test passes.
- [ ] Every write produces exactly one chained audit event.

## Stop and escalate if

- Write latency through the gateway can't meet P8 in the multi-replica test.
- GitHub's API can't preserve CRLF in commits made through the Git Data API.

## Out of scope

Reads (05) and MCP formatting (07).
