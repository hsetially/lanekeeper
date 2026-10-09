# 15: Security verification

| | |
|---|---|
| **Wave** | 3. Waves 1 and 2 merged, and staging deployed. |
| **Depends on** | Everything in v1 |
| **You own** | `fuzz/**` (corpora and new targets), `.github/workflows/security-*.yml`, `docs/threat-model.md` (consolidation), `docs/security-report.md` |
| **Security** | S1–S24 (verification) |
| **Gate** | `just verify-15` |

## Objective

Prove the baseline holds across the integrated system, not just within each crate. Leave behind automation that keeps proving it on every release.

## Tasks

### T1: Requirement traceability

Write `docs/security-report.md` as a table. For each S#, list:

- where it's implemented, as file paths;
- the test that proves it, by test name;
- its status.

Any S# without an automated test either gets one now or is escalated.

### T2: Integrated authorization tests

**Route matrix.** Generate a matrix of every route and every MCP tool against every user state: anonymous, pending, disabled, Viewer, Editor, Operator, Admin, and a user with `requires_approval`. Assert the expected status code for each pair.

**Agents.** Also test as agents: a valid certificate for a different swimlane, an expired certificate, and a revoked join token.

**Verify:** the matrix test passes in CI against the integrated hub.

### T3: Fuzzing at depth

- Extend the targets to webhook payloads, `AgentMessage` decoding, path mapping, every engine parser, the docs chunker, and regex options.
- Grow corpora from the synthetic fixtures.
- Nightly runs: 10 minutes per target.
- Every crash becomes a regression test.

### T4: Dynamic and infrastructure checks

**Dynamic:**

- the OWASP ZAP baseline against staging, authenticated as a Viewer and as an Admin, with no high-severity findings;
- CSP and header checks on every page.

**Infrastructure:**

- conftest and kube-linter on the rendered charts;
- a check that NetworkPolicies block unexpected egress, tested from inside pods.

### T5: Secrets and audit drills

**Log leak test.** Run the full end-to-end suite with log capture, then fail if any captured output matches:

- a token pattern;
- a sample file's contents;
- a session id.

**Audit tamper drill.** Alter a row as a database superuser in staging, and confirm that the nightly verification detects it and the alert fires.

**Key drill.** Confirm the CA private key can't be exported from KMS, and that the token vault's associated-data binding holds.

### T6: Threat model consolidation

Merge every component section, resolve gaps, list residual risks, and prepare the document for human sign-off (S24).

## Acceptance (all required)

- [ ] Every S# has an automated test, or a documented, human-approved exception.
- [ ] The route matrix, ZAP, leak test and tamper drill all pass.
- [ ] The threat model is ready for sign-off.

## Stop and escalate if

- Any high-severity finding requires a change to a decision or a contract.
