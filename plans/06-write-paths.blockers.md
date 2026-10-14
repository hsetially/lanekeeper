# Blockers for prompt 06 (write paths), mode T1

Status: BLOCKED before any T1 code. Nothing was implemented, and no contract path was touched.

## What blocks

The approved plan (`plans/06-write-paths.md`, section "Extra paths and contract changes") says the contract additions G1 to G12 go through one separate PR, `contract/06-writes` (label `contract-change`), and that it **merges before T1**. The human approved `Contract-change: APPROVED`. That approval allows the contract PR to exist. It does not put its content into this worktree, and the plan sequencing does not allow T1 to edit `crates/domain`, `crates/ports`, `api/openapi.yaml` or `db/migrations/` from the `agent/06-write-paths` branch (only `perf/budgets.toml` stays on this branch, per G12).

## Evidence that the contract PR has not landed

Checked in this worktree (HEAD `8cb7c39`, branch `agent/06-write-paths`) after `git fetch origin`:

- `git ls-remote --heads origin` lists only `main` (74d9b4f) and `claude/clever-franklin-26x6ym`. There is no `contract/06-writes` branch, and `origin/main` is 74d9b4f (PR 10, fact engine), which has none of the additions.
- `grep -rnE "change_eol|PreviewId|ToolWrites|SecretDetected|ProposalStale|preview_effect" crates/domain crates/ports api/openapi.yaml docs/mcp-tools.md` finds nothing. The only hit for `NotifyRequest` is the unrelated OpenAPI schema at `api/openapi.yaml:4663`; there is no domain `NotifyRequest` and no `WriteService::notify`.
- `db/migrations/` holds `0001_init.sql` and `0002_audit_verify_state.sql` only. `0003_write_additions.sql` (G8 notify action in the `proposals` CHECK, G9 `github_credentials.expires_at`, G10 approvals hardening) does not exist.
- `perf/budgets.toml` still has `P8` with `registered = false` and no `P12.hub_writes_queries` (that edit is allowed on this branch, in the T1 commit).

## Which T1 work needs which gap

T1 (the write pipeline) cannot be built as planned without these. Doing it on today's contracts would mean the workarounds the plan rules out.

| Gap | T1 dependency |
|---|---|
| G5 `ToolWrites` port | Step 6, tx 2: "complete the tool-write registration" so the registry attributes the next observation as `tool_write`. The fallback (writing `file_observations` directly) crosses into 05's table and was not approved. |
| G6 `RejectReason::{SecretDetected, ParseError}`, `Rejected{reason, issues}`, `Applied{.., warnings}` | Step 4, validate: parse errors block, duplicate keys are warnings, a secret match reports line numbers and rule ids. `WriteOutcome` cannot carry these today. |
| G7 `AuditEvent.author`, `AuditAction::ProposalStale` | Step 5 and the approval step: the audit event records both author and approver. |
| G4 `WriteService::notify`, `NotifyRequest` | The `WriteHub` implements `WriteService`, and `ports::conformance::write` runs against it. The trait shape must match before the first conformance test. |
| G1, G2, G3, G11 | Request DTOs (`EditRequest.change_eol`, `PreviewId` on upload and delete, PR `target`, reveal `reason`) that the pipeline's request-hash canonicalisation covers. Changing them later changes the idempotency hash layout. |
| G8, G9, G10 (`0003_write_additions.sql`) | The T1 integration rig migrates the real schema. Proposals insert (step 5) must accept `action = 'notify'`. |

## What is needed to unblock

1. A human (or the orchestrator, outside this worktree) opens and merges the `contract/06-writes` PR with G1 to G11, covering the Rust types in `crates/domain`, the `ports` trait and fake and conformance updates, the OpenAPI and `docs/mcp-tools.md` additions, `db/migrations/0003_write_additions.sql`, and the 05, 07 and fake consumers updated in the same PR (per the `lanekeeper-contract-change` skill). The plan lists a fallback per item that a human may choose instead; if any item is struck, say which.
2. Merge `main` into `agent/06-write-paths` (the 03b precedent: `03b/merge: bring in main`) so the worktree sees the new types.
3. Re-run the implementer in mode `T1`.

Alternatively, a human can explicitly approve a plan amendment that moves the contract edits onto this branch (adding `crates/domain`, `crates/ports`, `api/openapi.yaml`, `docs/mcp-tools.md` and `db/migrations/` to the approved paths). The plan is frozen and I did not make that call.

## Open questions from the plan that still need a human before or during T1

A1 (use of `response_body` for the crash intent), A3 (3 MiB limit and OpenAPI wording), A4, A5, A7, A12, A13, A15, A16, A21. The plan marks these as "Needs a human: Yes", and its header says a human approved the plan, so I took them as approved with the stated defaults; none of them is the cause of this block.
