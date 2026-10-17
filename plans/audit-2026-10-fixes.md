# Plan audit-2026-10: fix the findings of the cross-directory audit

Status: DRAFT
Contract-change: needed (WP-C1 to WP-C4, WP-C4's migration, the `perf/budgets.toml` rows of WP-R1 and WP-R6; this plan itself edits nothing outside `plans/`)
Extra-paths: none for this plan. Each work package's own plan lists its own (see "Extra paths and contract changes")
Prompt: none. The input is the audit of 16 Oct 2026 ("Lanekeeper: cross-directory audit and improvements"), committed verbatim as `plans/audit-2026-10.md`. WP-P3 creates prompts 18 and 19
Branch: `claude/beautiful-pasteur-52zl4n` (this plan). Every work package has a branch of its own, named in its block

Plan only. Nothing outside `plans/` was edited, and no work package starts until a human approves this plan and answers the questions in "Ambiguities and open questions". This plan is an umbrella, as `plans/contract-08-web.md` was: each work package still writes its own `plans/<name>.md` from `plans/TEMPLATE.md` and waits for its own approval (AGENTS.md, protocol step 2). The audit text (`plans/audit-2026-10.md`, a snapshot at `db6ea42`, not updated for the merges described below) is the specification for each item; the fixes quoted in its §3 are the starting point, and where this plan departs from one it says so and why. When the two disagree, this plan wins for the item, because it was checked against `20155b4`.

Base commit `20155b4` (`origin/main`, which holds prompts 01 to 07, `contract/08-web` and prompt 05). The audit was taken at `db6ea42`, on 05's open branch. Facts below were checked at `20155b4`; "Not verified" at the end says what was not.

## Summary for approval

**What the audit found, in one line.** The contracts are rigorous, but the system cannot reach its pilot: composition and CSRF (C-1), the edit lock (C-2), registration of swimlanes, agents and sentinels (C-3) and masking of raw text (C-4) have no working owner; 23 High and 25 Medium findings sit around them.

**What changed since the audit.** Two merges (PR #19 prompt 05, PR #20 `contract/08-web`) closed or narrowed seven findings (H-1, H-4, H-7, H-8, H-9, H-12, M-8) and changed the fix for four more (H-2, C-2, C-3, C-4; table "Re-baseline"). Two examples: the six `allOf` schemas (H-1) are fixed in the spec, and masking (C-4) is no longer "no contract" but "contract merged, nobody built it" (D128). The audit's snippets for H-2, H-8, C-2 and C-4 would now duplicate or contradict what is on `main`, so this plan replaces them.

**How the work is cut.** 18 packages in five waves (wave D resumes existing prompts and adds none):

| Wave | Packages | Kind | Gate |
|---|---|---|---|
| 0 | this plan, decisions A1 to A19 (A1 to A5 answered on 17 Oct 2026, see "Ambiguities and open questions") | human | approve and answer the rest |
| A | P1 agent protocol and guard script; P2 amended prompts; P3 new prompts 18 and 19, README rows for 18 to 20 (prompt 20 is already written, see A4); P4 records | Markdown and one script, no contract path, no Rust | one human review each |
| B | C1 errors and edges; C2 adoption (`contract/08-adoption`); C3 admin registration; C4 shapes and the MCP document | contract PRs, merged in this order | each: own plan, human approval, label `contract-change` |
| C | R0 03b fix (CSRF in the guard, B16, idempotency helper), then R1 prompt 18 hub integration; R2 03a fix; R3 06 fix; R4 07 fix; R5 05 fix (adoption routes, effective hash, served limit); R6 prompt 20 read-response masking (built later, see A4); R7 prompt 19 admin API; R8 prompt 21 first admin and break-glass (proposed, A19) | code, each against its merged contract | each: plan, gate `just verify-NN`, `just verify`, bench-check, reviewer |
| D | resume prompts 08, 09, 14, 17, then 15, 16 (and 10, 11) with the amended text | existing prompts | as in `prompts/README.md` |
| E | P5 docs restructure (split `threat-model.md` and `decisions.md`) | optional, last | only if A14 says yes |

**Answered on 17 Oct 2026:** A1 the full wizard; A3 a 03b fix first (WP-R0); A4 a separate prompt 20 that builds masking later; A5 no problem-type rename; A2 (only admins add admins and register swimlanes, agents and sentinels; and the agent's service account already has a column, so no stop is needed). **Proposed for your decision:** A19, how the first admin is created: replace the configured list with an operator command that has no HTTP surface (WP-R8, option 4 of six), because the list as built is a standing, silent, never-expiring way to mint an admin and nothing deploys it. Then A6 to A18 and A20, each with a default, so nothing is blocked on silence.

**Done means** every audit ID in "Traceability" is `closed` with the named proof, or `human exception` with a D# in `docs/decisions.md`; `just verify` and `cargo xtask bench-check --strict` pass on `main`; the composed route table equals `api/openapi.yaml` except for a named list that is empty before the pilot.

## Re-baseline: what `main` already has

Method. `git diff --name-only db6ea42 HEAD` lists 82 files. Untouched since the audit: `prompts/`, `Justfile`, `scripts/`, `skills/`, `db/migrations/`, `crates/hub`, `crates/hub-identity`, `docs/security.md`, `docs/performance.md`; in `crates/hub-platform` only `events/sse.rs`; in `crates/hub-writes` only `fidelity.rs` and tests; in `crates/hub-mcp` only `schemas/tools.json` and tests. So every finding that cites only untouched files stands as audited (spot-checked: `crates/hub/src/main.rs:34-36` still exits 1; `crates/hub/tests/guard.rs:41-50` still asserts that an editor's POST with no token returns 200; `crates/hub-platform/src/routes.rs:147-148` still folds pending, disabled and low role into one `Forbidden`; `served.rs:230` still uses `try_acquire`). Findings that cite a changed file (`api/openapi.yaml`, `crates/domain`, `crates/ports`, `crates/hub-registry`, `docs/mcp-tools.md`, `docs/decisions.md`, `docs/interfaces.md`, `docs/threat-model.md`) were re-read:

| ID | Audit said | At `20155b4` | Effect on this plan |
|---|---|---|---|
| H-1 | Six `allOf` schemas can never validate | **Closed in the spec.** `Me`, `SeverityRule`, `RetentionSettings` and `SwimlaneDetail` are one flat closed object each; `ConflictProblem`, `LockedProblem` and `TokenMissingAccessProblem` extend the open `Problem` (`additionalProperties: true`). Re-run with `jsonschema` 4.26 (2020-12): a real `ConflictProblem` body and a real `LockedProblem` body are VALID. `crates/ports/tests/web_contract_allof.rs` scans the whole document for a closed `allOf` member, so the audit's proposed guard test exists. | Residual only: a test that validates sample bodies with a real 2020-12 validator (`jsonschema =0.58.3` is already a workspace dependency, used by hub-mcp), because the registry's hand-written walker still merges `allOf`. In C1. |
| H-2 | Nothing refuses a marker in written content; fix in hub-mcp | **Contract merged.** `redaction_marker` is an `IssueKind` and is in `docs/mcp-tools.md`; `editFile` says a write that holds a marker is refused; `docs/interfaces.md` gives 06 "Refuse a write that holds a marker (CG7)" and says it reaches MCP through 06's pipeline. Not built: `crates/hub-writes/src/pipeline/validate.rs` has no check. | The fix moves to one enforcement point in hub-writes (R3), shared by REST and MCP, answering `redaction_marker`. The audit's `refuse_markers` in hub-mcp (second check, different error) is dropped. Skill text and the planted round-trip test remain. |
| H-4 | No `GET /drafts/{id}` | `getDraft` exists (CG17). `listMyDrafts` does not. `draft_mode.rs:28` is unchanged. | REST half done; `listMyDrafts` in C4; MCP wording in R4; mounting `getDraft` is 06's promise from PR #20 (R3 depends on it). |
| H-7 | No 429, 503, 504 or `Retry-After` | Components `TooManyRequests`, `ServiceUnavailable` and header `RetryAfter` exist. 429 is on 3 of 99 operations, 503 on 2, 504 on none; no `GatewayTimeout`. `agent_unavailable` is still a 422 in code. | C1 applies them to every operation, adds `GatewayTimeout`; R3 remaps the code. |
| H-8 | Hash-conflict 409 reused for state conflicts | `ConflictState` (a plain state conflict, decided in PR #20, Q10) is on `adminApproveAccessRequest`, `adminRejectAccessRequest`, `adminUpdateUser`. `createPr` documents its 200 (`proposal_created`) and its 409. `rejectProposal`, `adminConfirmAdoption`, the access-request 409 and 429, the duplicate-PR discriminator and 422 on key reuse are still open. | C1 reuses `ConflictState` and adds the `conflict` discriminator additively; the audit's new `StateConflict` response is not created. |
| H-9 | Spec defines no `Problem.type` values; guard loses B16 | Spec half closed: `Problem.type` lists both spellings (snake_case from the hub layers, kebab-case from identity) and states that nothing is renamed. Code half open (guard unchanged). | The audit's "normalise to underscores" contradicts a merged statement: decision A5. Default: no rename; restore the B16 types on platform routes using identity's spellings. |
| H-12 | `Proposal` has no `action` or `paths` | `Proposal` has `action`, `via`, `paths`, `new_hash`, `decided_at`; `getProposal` exists (CG12). Missing: `params` (the target of a restart, notify or PR) and `WriteApplied.pr_job_id`. | Residual in C4; the workaround in `proposals/mod.rs:33-49` goes in R3. |
| M-8 | SSE stream untyped | `DomainEvent` is a `oneOf` with one schema per kind and is linked from `streamEvents`; frames, `Last-Event-ID`, heartbeat, 30-minute limit and 429 are described. | Residual: say in `EventKind` that `pending_restart_changed` is no longer published (no note exists). One line in C1. |
| C-4 | No contract support for masking | **Contract merged (M1).** `Mask {line, start, end, setting?}`; `masks` on `FileContent` and `ServedFile`; markers recognised by form on `TextHit` and `DiffLine`; the `getBlob` rule as a requirement; the search and compare oracles written down as open items 1 and 3 in `docs/interfaces.md`. **Not built, not owned:** D128 says 05 did not build the masking, the scan cache, the benchmark or the blob refusal, and reported it to the human. | C-4 becomes "name an owner and build M1" (R6) plus A4. The audit's `FlaggedSpan` schema and its Q39 are superseded by `Mask` and M1 and are not added. |
| C-2 | No adoption or divergence operation | Still none. PR #20 deferred CG6 (adoption wizard) to a PR named `contract/08-adoption` (Q13) with fixed rules: tag `adoption`, the five database steps, confirm is Admin, marks only for drift states, audit actions `BaselineAdopted`, `DriftMarkedIntentional`, `DriftMarkCleared`. `SwimlaneSummary.baseline` (CG4a) now says whether editing is unlocked. | C2 is that PR; the audit's three operations become a subset of CG6. The audit's `editing_locked` boolean is dropped as redundant. The `lock` cause on `LockedProblem` is CG4b (deferred): A17. |
| C-3 | No way to create swimlanes, agents, sentinels | Still none (CG15, deferred in PR #20, Q4). | C3. |
| numbers | 91 operations, 23 decisions to confirm, D124, Q39 | 99 operations; 26 decision lines say "to be confirmed at review"; last decision D128; last question Q37. | New records take D129 and Q38 onward (the audit's Q39 and Q41 are renumbered). The 413 gap is 9 of 32 body operations (audit: 8 of 31). |
| M-22 | Review 05 at `db6ea42`, PR open | PR #19 merged. | A retrospective review of the merged head instead (P4). |

## Tasks and commits

Commit subjects follow the repository: `audit/Pn/Tk: ...` for documentation packages, `contract(<name>): ...` for contract PRs, `NN/Tk: ...` for a prompt run and `NN/fix: ...` for a fix of a merged prompt. Sizes are estimates (S under 300 changed lines, M under 1,500, L above).

### Wave A: Markdown and script, no contract path

These packages edit `.claude/`, `AGENTS.md` and `prompts/`, which the guard protects inside `agent/*` worktrees, so they are written on ordinary `docs/*` branches by a person or an agent outside a worktree. The hook does not apply there: the reviewer and CI are the backstop, as AGENTS.md says.

**WP-P1 agent protocol and the guard script** (`docs/audit-agent-protocol`, S). Closes H-15, H-14 (protocol half), M-17 (script half), M-20 (protocol half), H-13 (protocol line).

| Task | Commit | Change |
|---|---|---|
| T1 | `audit/P1/T1: one machine-read reviewer verdict` | `prompts/REVIEW.md`: the Verdict section of the audit's H-15 (line 1 exactly `VERDICT: APPROVE` or `VERDICT: CHANGES REQUESTED`; a conditional approval is changes requested) and sections 6 to 8 (spec chain and shared files; decisions, requirements and budgets; untrusted content). `.claude/agents/reviewer.md` and `.claude/skills/lanekeeper-pr-review/SKILL.md` point to it and stop defining their own. New `node scripts/lk.mjs verdict <file>` reads line 1 and prints JSON; `.claude/skills/run-prompt/SKILL.md` calls it instead of matching text. Verdicts written before this change (`plans/02-agent.review.md` has `**VERDICT: APPROVE**`) are history and are not re-parsed. |
| T2 | `audit/P1/T2: human-only steps and the threat-model task` | `AGENTS.md`: the "Stop and escalate" bullet for a real environment or credential (H-14) and, in protocol step 3, "every prompt updates its component's threat-model section in its last task" (M-20). |
| T3 | `audit/P1/T3: the implementer reads Amended-by` | `.claude/protocols/implementer.md:8` (H-13); `.claude/skills/lanekeeper-rust-conventions/SKILL.md:49`, the `query!` advice replaced by runtime `sqlx::query` with EXPLAIN snapshots (D112 A39). |
| T4 | `audit/P1/T4: the path guard enforces what ownership cells mean` | `scripts/lk-lib.mjs`: the PROTECTED check runs before the owns check (line 251), so `@root-files` can never grant `AGENTS.md` or `CLAUDE.md` (today `@root-files` matches any path without a slash, so it also matches them); a test pins that only backticked paths count as ownership; the wave parser (line 62, `/^(\d+)/`) reads `2.5` as `2`, which would pull prompts 18 and 19 into `wave:2` runs, so it accepts decimals and sorts `2.5` between 2 and 3. The same file keeps `p.wave <= 2` when a prompt depends on "Everything in v1" (line 111, prompts 15 and 16): today that list includes 18 to 20 only because 2.5 reads as 2, and once decimals parse it would drop them and `deps 15` would say ok before they merge, so that comparison becomes `< 3`, with a test (found by simulating a wave-2.5 prompt in a scratch copy). |
| T5 (optional, A15) | `audit/P1/T5: lk worktree --slug for fix rounds` | `node scripts/lk.mjs worktree NN --slug fix-name`, so a fix of a merged prompt gets its own branch `agent/NN-fix-name`. |

Verify: `just lk-test` with new tests `a_verdict_is_line_one_and_nothing_else`, `a_conditional_approval_is_not_an_approval`, `root_files_never_grant_protected_paths`, `a_decimal_wave_sorts_between_its_neighbours`; `node scripts/lk.mjs list` prints the same prompts as before.

**WP-P2 the existing prompts, amended** (`docs/audit-prompts`, M). Closes H-13, H-14 (prompt half), H-16 (text), H-17 (text), H-18 (text), H-23 (text), H-5 (acceptance text), M-17 (text half), M-18, M-19, M-20, M-21, M-12 (prompt 08 screen), and the prompt items of the Low batch.

| Task | Commit | Change |
|---|---|---|
| T1 | `audit/P2/T1: superseded task text and Amended-by rows` | The six replacements of H-13 (`02:71` agent walker, not `ignore::WalkParallel`; `05:41`; `06:97`; `09:45`; `11:33`; `13:53-55`) and an **Amended by** row in the metadata table of every prompt that has one (for example 02: D89 to T4, D74 to D79). |
| T2 | `audit/P2/T2: credential and real-environment steps are human-only` | The Stop sections of 03b, 07, 09, 10, 15, 16; `09:162` and `10:20-39` as in H-14 (a human runs `helm list -A`, because Helm 3 stores releases in Secrets). |
| T3 | `audit/P2/T3: the sentinel wire contract` | `prompts/17-nfs-sentinel.md`: line 42 fixed (relative paths), the D123 contract under T3 (`seq` starts at 1, relative path, empty login for an unset uid, 500 records and 2 MiB per batch), and the note that the hub half of "heartbeat gap raises a finding" belongs to prompt 18 (A12). |
| T4 | `audit/P2/T4: prompt 09 takes its hand-offs` | The T5 database, T3 network and users, and T2 CI additions of H-17, and the egress line that lets the agent reach the config-server on TCP 8888. If A19 is approved: the disabled `admin-job.yaml`, `deploy/RUNBOOK-first-admin.md`, and the RBAC and Cloud Audit Logs alert on `create job` and `pods/exec` (WP-R8, item 4). Acceptance for H-5: `just mcp-t0-status --strict` exits 0 or a human exception names the shipped modes (also in 15). |
| T5 | `audit/P2/T5: verification prompts cover the current requirement set` | 15: `S1–S25`; 16: `P1–P15`, plus the P14, P15, P7 text-search steady state and cold start, P6 history and `bench-check --strict` items; acceptance for S14b (no flagged value in any REST response except `POST /files/reveal`, over every read operation) and for each human-owned residual closed or excepted (M-23); H-23: 16 T1 writes and runs `crates/agent/tests/nfs_real_mount.rs`, with Extra-path `crates/agent/tests/**`. |
| T6 | `audit/P2/T6: template conformance` | A Verify for each of the 32 tasks without one (threat-model tasks use `cargo test -p <crate> --test threat_model`); prompt 12 expanded or marked "none until expanded"; each stubbed Gate row says it is an `exit 1` stub and what T1 replaces it with; `verify-12` stub added to `Justfile`; a threat-model task added to 01, 04, 05, 08, 10, 11, 16. |
| T7 | `audit/P2/T7: ownership, blocking questions and the README tables` | Ownership cells list exact backticked paths (01's "workspace root files", 09's "Dockerfiles", the threat-model sections); a **Blocking questions** row from the "Blocks:" lines of `docs/open-questions.md` (02: Q1, Q10; 03a: Q21; 03b: Q26, Q28; 05: Q3, Q17, Q19, Q33; 06: Q10, Q35, Q37; 07: Q20, Q21; 08: Q35; 09: Q8; 10: Q36; 14: Q17, Q23; check each against the file first); `prompts/README.md`: the shared-and-unowned table and the rule that only backticked paths are enforced (M-18), the human gates for the real NFS mount test and `just engine-real-snapshot` (H-23, M-22); `16:135` replaced. |
| T8 | `audit/P2/T8: prompt 11 and the Cursor skill` | `prompts/11-cursor-skill.md`: the T4 rules, Verify, Stop and Out-of-scope of M-21; `skills/lanekeeper/SKILL.md`: C1 to C12, `notify_services` may be off, the truncation rule, and "never put a `[REDACTED: ...]` marker into content" (H-2); new scenarios in `skills/lanekeeper/evals/evals.json` (injection text in a file, a redacted value, `locked`, draft mode); prompt 08's merge-mode screen removed (M-12); "previous bundle" replaced in `01:148, 187` and `05:86`. |

Verify: `just lk-test`; `node scripts/lk.mjs info NN` for every prompt still returns wave, owns and stack; a check that no prompt still contains `WalkParallel`, `sqlx prepare`, an instruction to publish `PendingRestartChanged`, "multi-replica" under v2 or "previous bundle" (a grep, its output pasted in the PR).

**WP-P3 new prompts 18 and 19, and the README rows for 18 to 20** (`docs/audit-new-prompts`, S). Closes the prompt half of C-1 and C-3. Prompt 20 (`prompts/20-read-masking.md`, A4) was written on 17 Oct 2026 and parses (`node scripts/lk.mjs info 20`); P3 only gives it its README row and rereads it with the other two. Its gate recipe `verify-20` is already in the Justfile as an `exit 1` stub, as prompt 01 did for the others; P3 T3 adds the stubs for 18, 19 and 21.

| Task | Commit | Change |
|---|---|---|
| T1 | `audit/P3/T1: prompt 18, hub integration` | The audit's C-1 prompt, adjusted: the CSRF step, the B16 types and the idempotency helper are not here (R0 lands them first, A3), so 18 owns the composition: the production `SessionAuth` over hub-identity, the route-table test (H-10), the leased jobs, the migration runner, `retention_dry_run = true` (A7), request ids and the duplicate-route `BuildError`; it takes over `crates/hub/**` from 03b; **Amended by** D109, D112, D116, D118; the S3 test covers every non-GET composed route; P7 auth and P11 measured with every module. |
| T2 | `audit/P3/T2: prompt 19, admin API` | Owns `crates/hub-admin/**`; T1 registration (C-3), T2 every other `admin*` operation not built by 03a (explicit list, `adminListAudit` among them, with the D38 fields of H-11), T3 schema alignment (H-22); each write audits in its own transaction (rule 8) and honours `Idempotency-Key`; join tokens are never replayed (H-6). |
| T3 | `audit/P3/T3: README rows and the owner of D112 A7` | Wave 2.5 rows for 18, 19 and 20 in `prompts/README.md`, `verify-18`, `verify-19` and `verify-21` stubs in the Justfile, the shared-work table, and the note that 18 takes over `crates/hub/**` composition from 03b and 20 takes the registry's read edge for masking. Needs P1 T4 first: until the wave parser accepts decimals, `2.5` reads as wave 2. |

Verify: `node scripts/lk.mjs info 18`, `info 19`, `info 20` (wave prints as 2.5), `deps 18`, `list --wave 2.5`, and `list --wave 2` does not list them.

**WP-P4 records** (`docs/audit-records`, S). Closes M-22, M-23, the record items of the Low batch, H-18's decision amendment and the `CHANGES.md` half of M-12.

| Task | Commit | Change |
|---|---|---|
| T1 | `audit/P4/T1: open questions for what only a human can close` | `docs/open-questions.md`, from Q38: D117 (windows that never close), the six "Gaps against requirements" of the threat model, D116 and D118 (retention), the effective hash and `application-<tenant>.*` (H-21, A11), the sentinel finding (A12), the search and compare oracles (open items 1 and 3 of `docs/interfaces.md`, A4), the REST `ty` rename (A13), the uniqueness of service accounts (A20). Each with a "Blocks:" line. Q21 is moved to "Answered" (assignment required: yes) if A19 is approved. |
| T2 | `audit/P4/T2: decisions recorded, amendments dated` | New D129 onward recording the answers to A1 to A18; one table of the 26 "to be confirmed at review" decisions (D#, confirmed by, date); dated pointers (not silent rewrites) on D70 (S1 to S25, P1 to P15), D73 (supersedes D39), the D43 references (D48), D106 (collation superseded by D124), D14 (superseded by D82); `docs/interfaces.md` port methods (`effective_for`, `compare_in`, `compare_tree_under`, `drift`, `auto_notify`, `served`, `related_to`, `grep_in`); `CHANGES.md`. |
| T3 | `audit/P4/T3: the review trail` | `plans/03a-hub-identity-audit.review.md` and `plans/06-write-paths.review.md` quoting the verdicts of PR #9 and PR #15 (read with the GitHub tools, not from memory); a reviewer run on merged `20155b4` for the commits after round 2 of 05, appended to `plans/05-registry-read-api.review.md`; the `just verify-NN` end-to-end results for every merged prompt as one table in `plans/audit-2026-10-fixes.evidence.md`. |

Verify: `cargo test -p hub-mcp --test t0_status --test cursor_admin_doc --test threat_model`, `cargo test -p xtask --test budgets_registry`, `just lk-test` (these read `docs/decisions.md`); the tests that read `docs/threat-model.md` pass unchanged.

### Wave B: contract PRs

Common process (skill `lanekeeper-contract-change`). Each package: a planner writes `plans/<branch>.md` with `Contract-change: needed`; a human approves; tests first; update `ports` fakes, `ports::conformance` and the "What each owner must implement" table of `docs/interfaces.md`; run `just verify-01`, then the gate of every merged consumer (`verify-03a`, `verify-03b`, `verify-04`, `verify-05`, `verify-06`, `verify-07`); regenerate the client when `web/` has `pnpm gen:api`; label `contract-change`; the PR text names the prompts affected. Write them in parallel, merge in the order C1, C2, C3, C4 (all edit `api/openapi.yaml`, in different places). C1 goes first so that prompt 08 generates its client against one error surface.

**WP-C1 errors and edges** (`contract/audit-errors`, L). Closes H-6 (spec), H-7 (spec), H-8 (spec), H-9 (spec), H-1 (residual), M-4, M-5, M-6, M-8 (residual), M-9, M-10 and the `api/openapi.yaml` items of the Low batch.

- `IdempotencyKeyNoReplay` on `revealValue`, `createMcpToken`, `adminIssueJoinToken`, with a convention test that every operation whose 2xx carries `Cache-Control: no-store` uses it (H-6).
- `TooManyRequests` and `ServiceUnavailable` on every operation, `GatewayTimeout` and 504 on `getServedFile`, `restartService`, `notifyServices`, the file writes, `approveProposal` and `putGithubToken`; `RetryAfter` on every 429 and 503; `.redocly.yaml` rule extended (H-7). 422 on the operations that take `Idempotency-Key` and list none (H-8).
- `ConflictState` on `rejectProposal`, `createPr` (409 with `conflict: duplicate_pr`, never `reason`) and, in C2, `adminConfirmAdoption`; the access-request 409 and 429 (H-8).
- `Problem.type`: add the B16 codes to the documented list; no rename (H-9, A5).
- `return_to` pattern `^/(?:[^/\\\x00-\x1f\x7f][^\\\x00-\x1f\x7f]*)?$` (M-4); `NfsPath` and `GitRef` patterns that mirror `crates/domain/src/paths.rs` and `compare_ref.rs`, and the blob `{hash}` as lowercase `ContentHash` (M-5; the convention test that pins `[0-9a-fA-F]` is updated). These tighten request validation to what the server already accepts: A18 asks a human to approve them, and a differential test runs each pattern against the parser it mirrors.
- `413` on all 32 body operations, the 4 MiB and nesting-depth sentence in `info.description`, the `!SPEC.contains("4 MiB")` assertion narrowed, multipart `encoding` (M-6).
- `LanekeeperTenants` header on `compareFile`, `compareTree`, `getGrid`, `getEffectiveConfig`; `Content-Disposition: attachment` and `X-Content-Type-Options: nosniff` on `getBlob`; `SwimlaneSummary.tenants` `minItems: 0` (M-9).
- Sign-in documentation: callback `error`, `error_description`, `session_state`; the two `Set-Cookie` headers and that `__Host-lk_csrf` is readable by script; `Location` targets; the logout cookie; when a 401 follows an idle or absolute expiry (M-10).
- `EventKind`: `pending_restart_changed` is no longer published (M-8).
- Hygiene: S8 not S7 (`:1994`, `:5415`), D37 for "D-approvals" (`:2621`), S18 and D64 (`:3335`), S10 (`:1777`), `/events` `explode: false`, `FileContent.format` nullable, "bytes, enforced by 413" in `DocCreateRequest` and `DocUpdateRequest`, the `Locked` description ("the file or the swimlane is locked", not only a sync window).
- A test that validates one real body of each `allOf` composite with `jsonschema =0.58.3` (2020-12) (H-1).

Verify: `just verify-01`; `cargo test -p ports --test openapi_conventions --test web_contract_allof --test web_contract_problems --test web_contract_global`; new tests `every_no_store_operation_uses_the_no_replay_key`, `every_operation_declares_429_and_503`, `every_body_operation_declares_413`, `path_and_ref_patterns_agree_with_their_parsers` (differential, with the proptest corpus of `domain`), `allof_composites_validate_real_bodies`.

**WP-C2 adoption and intentional divergence** (`contract/08-adoption`, M; A1 decided: the full wizard). Closes the contract half of C-2.

- Operations, tag `adoption`, every non-GET with `X-CSRF-Token`, `Idempotency-Key` and `x-required-role`: read the open pass or tenant re-match review, the wizard steps CG6 lists (choose branch, choose base, resolve, cancel), `adminConfirmAdoption` (Admin; its 200 says `editing_unlocked`; 409 `ConflictState` when the pass changed), `markIntentional` and `clearIntentional` (Operator; 409 `ConflictState` or `Conflict` when a hash moved).
- Schemas `AdoptionProposal`, `AdoptionConfirmRequest`, `AdoptionResult`, `IntentionalDivergenceRequest`, every string and array bounded, derived from the typed inputs of `crates/hub-registry/src/adoption/confirm.rs` and from migration `0003_registry.sql:196-243` (the five steps win over the three vocabularies of the handoff).
- `lost_version` on `Finding` is included: the S6b screen's "restore a lost version" needs the lost hash, and the restore itself is the existing `revertFile`. That is my reading of "full wizard" (CG6 lists it with the wizard); it is one optional field and one commit, so say so if you want it out. The `lock` cause (CG4b) only if A17 says so.
- `crates/ports/tests/web_contract_global.rs`: rows in `new_operations_name_their_audit_action_or_say_why_not` for each operation (confirm: `BaselineAdopted`; marks: `DriftMarkedIntentional`, `DriftMarkCleared`; wizard steps: none, stated).

Verify: as the common process, plus `cargo test -p ports --test web_contract_global` and the registry conformance hook `RegistryScenario::set_open_adoption`.

**WP-C3 admin registration** (`contract/admin-registration`, M; A2 decided: every registration is Admin-only). Closes the contract half of C-3, H-22 and M-12.

- `adminPutProject`, `adminPutSwimlane`, `adminPutSentinel`, `adminDeleteSentinel` (all `x-required-role: admin`), schemas `ProjectInput`, `SwimlaneInput`, `SentinelInput` with the fields of `db/migrations/0001_init.sql:110-139, 737-748`. Creating a row never sets `editing_unlocked` (C-2). Each names its audit action and publishes `swimlane_changed` in its transaction. `crates/ports/src/audit.rs` has no action for registering an account or issuing a join token: registrations reuse `SettingsChanged` (as CG15 proposed), and a new `JoinTokenIssued` is added with the operation that needs it (a ports change, so part of this contract PR). `SentinelEnrolled` exists and is unused; use it for a sentinel's creation. If A19 is approved, `AuditAction::AdminBootstrapped` and its wire-name snapshot (`crates/ports/tests/snapshots/contract_shape__audit_action_wire_names.snap`) go in here too; no migration, because `audit_events.action` has only a length check.
- **Where the service account lives (checked 17 Oct 2026, answers the second half of A2).** The agent's account is `swimlanes.agent_service_account` and the sentinel's is `sentinels.service_account` (`0001_init.sql:126-127`, `:743`), and the join already reads them (`crates/hub-platform/src/join/google.rs:12-33`, compared in constant time with the token's email). No column is missing and no stop is needed. Nothing writes either column in production (only test seeds do). The spec already has `adminPutAgent` (`PUT /api/v1/admin/agents/{swimlane}`, body `AgentRegistrationInput`) and `adminIssueJoinToken`, which nobody implements: C3 adds only the project, swimlane and sentinel operations, makes `adminPutAgent`'s 404 mean "no such swimlane", and adds `service_account`, `export_root` and `swimlane` to `SentinelInfo` so the admin can see what is registered. R7 implements `adminPutAgent` and `adminIssueJoinToken` (the token stores only its SHA-256, with the issuing admin's `(tid, oid)` and an expiry at most 24 h away; the plaintext is returned once with `Cache-Control: no-store`).
- Optional, my recommendation (no document requires it): a new migration `0009_*.sql` with partial `UNIQUE` indexes on `lower(agent_service_account)` and `lower(service_account)` where not null, and a shape check (at most 256 characters, one `@`), because the schema today lets one account join as two swimlanes, or as an agent and a sentinel. Uniqueness across the two tables cannot be an index, so the admin operation refuses it with 422 inside its transaction. Raised as question A20.
- H-22: `FolderMapping` becomes `{id, swimlane|null, folder, deployment, suggested_score, confirmed}` and gains `POST /api/v1/admin/folder-mappings/{id}/confirm`; the same alignment for path mappings, severity rules, notification settings, the order of host tiers and `OsLoginUser.os_login` (`{minLength: 1, maxLength: 128}`).
- M-12: the three merge-mode operations get `deprecated: true` ("no effect since D82").

Verify: as the common process; `crates/ports/tests/openapi_conventions.rs` gains `admin_schemas_name_the_columns_of_their_tables` (a check of property names against the migration, written as an ad-hoc script first and committed as a test only if it holds up).

**WP-C4 shapes and the MCP document** (`contract/audit-shapes`, L; after A10, A12, A13). Closes H-4 (REST), H-11, H-12 (residual), H-3 (lasting), M-7, M-11, M-13, M-14, M-15, M-16, the contract half of H-16, and the document lines of H-2 and H-4.

- `listMyDrafts` (paged) next to `getDraft` (H-4).
- `AuditEvent`: `via` gains `sync` and `agent_detected`; `actor` covers agent, sentinel and system; `hash_before`, `hash_after`, `github_login`, `approval`, `path`, and `chain {prev_hash, hash}` so S9 can be re-checked from the API (H-11).
- `Proposal.params` and `WriteApplied.pr_job_id` (H-12).
- Output bounds: `maxLength: 4096` and `text_truncated` on `TextHit.text`, `DocGrepHit`, `DocHit.snippet`, `DiffLine.text`; paging for `/features/status`; `truncated` on `SwimlaneDetail.services` and `EffectiveConfig.settings`; a convention test that any response array with `maxItems` over 500 is paged or paired with `truncated` (M-7).
- `Confidence.none` in `crates/domain`, `api/openapi.yaml`, a new migration widening the CHECK (a new file, never an edit) and `docs/mcp-tools.md`, with `Attribution::unknown()` returning it; or an amendment of D73, per A10 (M-11).
- `redacted: bool` on `domain::SettingValue` and on REST `SettingValue` (H-3); `FindingKind::sentinel_silent` with its migration and the audit action, if A12 says yes (H-16).
- The drift order stated in `listDrift` and in `get_drift` (M-13; the order itself is A6).
- `docs/mcp-tools.md`: the actual REST-to-MCP naming map replacing "names and values match" (`ty` and `type`, actor shape, `proposal_id` and `id`, `returned_lines` and `lines`), `truncated` on `pending_files` (50), `tenants` (64) and the `rule_ids` and `locations` lists (20), the S15 labelling claim as it is (four tools carry no `untrusted` field; delimited blocks only in `data_block` mode), the Errors paragraph of M-16, the draft-mode paragraph of H-4 (only `apply_change` has a recorded draft), "markers are output-only" under `propose_change` and `upload_file` (H-2). The `ty` to `type` rename in REST only if A13 says yes.

Verify: as the common process; `cargo test -p xtask --test mcp_tools_doc`, `just mcp-doc-verify`, `cargo test -p hub-mcp --test mcp_auth`; snapshots (insta) regenerated and reviewed.

### Wave C: code against the merged contracts

Each runs the protocol of AGENTS.md: a plan in `plans/`, approval, one commit per task, `just verify-NN`, `just verify`, `cargo xtask bench-check`, evidence, reviewer. The owner crates are merged prompts, so each is a fix round (A15, A16): its plan lists the owner's paths as owned and names no other crate except where "Extra-paths" says so.

**WP-R0 03b fix, the guard first** (`agent/03b-guard-csrf`, S to M; after C1, before R1). A3 decided: the 03b fix goes first. 03b owns both `crates/hub-platform/**` and `crates/hub/**`, so this package needs no Extra-paths. Closes the guard half of C-1, H-9 (code), H-6 (helper) and, if A12 says yes, the heartbeat stamp of H-16.

| Task | Commit | Change |
|---|---|---|
| T1 | `03b/fix: S3 in the one guard` | `SessionAuth::verify_csrf`, default-deny, called in `guard` right after authentication; new `ApiError::CsrfRejected` (403, problem type `csrf`, the spelling identity already uses); every fake and test that implements `SessionAuth` is updated in the same commit; `crates/hub/tests/guard.rs:41-50` stops asserting 200 and asserts 403 without the double-submit token or with another `Origin`, and 200 with both (a `status_with_csrf` helper). `/mcp` stays self-authenticating (D109 D-1); the webhook is HMAC (S18). |
| T2 | `03b/fix: B16 on platform routes` | `AccessPending`, `AccessDisabled`, `InsufficientRole` in `guard` and `auth_failure`, with identity's spellings (`access-pending`, `access-disabled`, `insufficient-role`), because A5 decided there is no rename; test: a pending, a disabled and a low-role user get three different problems on one platform route. |
| T3 | `03b/fix: one Idempotency-Key check` | `require_idempotency_key` in `crates/hub-platform/src/routes.rs` (D68): exactly one header, matching the contract's pattern, else 400. |
| T4 (A12) | `03b/fix: the sentinel heartbeat is stamped` | `SentinelHeartbeat` stamps `sentinels.last_seen_at` (`sentinel_service.rs:333` drops it today). |

Verify: `just verify-03b`; `cargo test -p hub-platform`; `cargo test -p hub --test guard`; P7 auth re-measured with the CSRF step (`cargo xtask bench-check`). The platform threat-model rows for S3 and S4 are updated in the last commit.

**WP-R1 prompt 18, hub integration** (`agent/18-hub-integration`, L; after P3, C1, R0). Closes the composition half of C-1, H-10, M-1, M-2, M-3, the hub half of H-16 (A12) and the code items of the Low batch.

| Task | Commit | Change |
|---|---|---|
| T1 | `18/T1: the production SessionAuth` | `crates/hub/src/identity.rs` implements `verify_csrf` over hub-identity's `csrf::check` and `AuthContext`; 03a's ten routes are re-registered through an `IdentityModule` that keeps B16's types, and hub-identity's own router is deleted or test-gated. `cargo test -p hub --test csrf`: every non-GET route of the composed table answers 403 without the token or with another `Origin`, `/api/v1/me/mcp-tokens` included (D109 D-2). |
| T2 | `18/T2: mount every module and test the whole table` | Registry, writes, MCP, events, blobs and git mounted through `Module`; `RegistryBlobPolicy` on `/api/v1/blobs/{hash}`; `crates/hub/tests/route_table_full.rs` compares the composed table with `api/openapi.yaml`, with `NOT_YET_BUILT` and `NOT_IN_SPEC` as explicit `const` lists naming an owner and a prompt, and fails on a route registered twice (H-10); a duplicate route is a `BuildError`, not an axum panic. |
| T3 | `18/T3: edge hardening` | Streaming JSON depth limit replacing the buffer-then-scan (`limits.rs:216-236`) so nothing is buffered before authentication (M-1); `catch_panic` inside `normalize_errors` and `unreachable = "deny"`, `indexing_slicing = "warn"` in `[workspace.lints.clippy]` (M-2); authenticate before a 404 under `/api` (M-3, after T1's B16 types); one `RequestId` generator read by writes, MCP and identity. |
| T4 | `18/T4: background work and start-up` | Every leased job that 05 and 06 export registered (a test fails if one is not), `spawn_warmup`, the gateway given the registry's `ReportSink` and `SentinelSink`, 06 given the registry's `ToolWrites`; retention starts with `retention_dry_run = true` (A7); migrations run as `lanekeeper_migrator` before the app pool opens; `main` starts with every module; `just dev` starts the hub; if A12 says yes, a leased job raises `sentinel_silent` after N missed beats (the stamp it reads is R0 T4). |
| T5 | `18/T5: threat model, P7 and P11` | The "Hub composition (18)" section of `docs/threat-model.md`; P7 auth measured with the CSRF step, P11 with every module mounted. |

Extra-paths: `Cargo.toml` (lints); `Justfile` (`verify-18`, `just dev`); `perf/budgets.toml` (a row for the streaming depth check, a contract path). Nothing in `crates/hub-platform`: R0 did that (A3). Verify: `cargo test -p hub --test csrf --test route_table_full --test jobs --test guard`; `just verify-18`; `cargo xtask bench-check`. New fuzz target `crates/hub/fuzz/depth_scanner` (the depth scanner parses untrusted bytes, rule 13).

**WP-R2 03a fix** (`agent/03a-audit-verify-state`, S; after C1). Closes H-20; H-6 (the four hub-identity writes); H-8 (the identity 409s and 429s).

- `crates/hub-identity/src/audit/verify.rs`: `unfinished_in_a_row` is read and written in `audit_verify_state` under the verifier's lease; the column names are checked against `db/migrations/0002_audit_verify_state.sql` first. Test `a_restart_between_two_unfinished_nights_still_alerts`.
- The four writes call `require_idempotency_key` (R0 T3).
- A5 decided no rename, so identity's problem types stay as they are and nothing else changes here.

**WP-R3 06 fix** (`agent/06-write-fixes`, M; after C1, C4). Closes H-2 (server), H-19, H-6 (reveal, tokens), H-7 (code), H-8 (code), H-12 (workaround), M-15 (the S14b form).

- `pipeline/validate.rs`: a content that holds `[REDACTED: ` is refused with `redaction_marker` for `editFile`, `uploadFile`, doc writes and every MCP write. Test `a_marker_read_through_read_file_and_written_back_is_refused` (plants a flagged value, reads it through the MCP tool, writes the result back).
- `pipeline/execute.rs`: the delete calls `register_delete` inside the intent transaction, in a savepoint as `docs/interfaces.md` recommends. Test `a_delete_through_the_tool_is_attributed_tool_write_and_raises_no_file_change`.
- `routes/problem.rs`: `agent_unavailable` becomes 503 with `Retry-After`, the duplicate-PR conflict sends `conflict: duplicate_pr`; `revealValue` and the token writes validate the key and never store a response.
- `proposals/mod.rs:33-49`: the zero hash and fake path workaround is deleted once the `Proposal` fields and `getProposal` are mounted (PR #20's promise to 06, which this package depends on); `proposals/view.rs:22` and `preview/effect.rs:207, 403` emit `[REDACTED: N chars, unscanned]`.

**WP-R4 07 fix** (`agent/07-mcp-fixes`, M; after C4, R3). Closes H-3, H-4 (MCP), H-5 (defaults), M-14 (code), M-15, M-16 (code).

- `tools/dto.rs`: `masked_by_port` so a value that a port already masked counts as redacted; then read `redacted` from `SettingValue` once C4 lands. Test with a seeded `[REDACTED: 40 chars, github_token]` asserting `redacted == true` and the summary counting it.
- `confirm/draft_mode.rs`: the two summaries of H-4; `apply_change` is the only tool with a recorded draft.
- `truncated` set on every list that is cut (C4's flags); the marker round-trip test of H-2 on the MCP side.
- Until `O.error_vs_output_schema` is filled by a T0 run, `advertise_output_schemas` defaults to `false` and `LK_MCP_CONFIRM` to `draft` (A9); `just verify-07` keeps `mcp-t0-status` (the `justfile_gate` test of hub-mcp must stay green) and the go-live check uses `--strict`.

**WP-R5 05 fix, adoption, effective hash and served limit** (`agent/05-adoption-routes`, M; after C2, A11). Closes the code half of C-2, H-21, M-24.

- Mount the C2 operations from `hub-registry` (the service exists: `adoption/confirm.rs`); the route-table test of R1 loses the entries it had for them.
- H-21: per A11, the engine adds `application-<tenant>.*` to `Layers` and the batch recomputes through `property_view`; regression test `an_edit_to_application_sit1_moves_the_effective_hash_and_the_drift`.
- M-24: a per-user `RateLimiter<UserId>` in front of `served_slots.try_acquire()` (429 with `Retry-After`), and 4 slots reserved for the nightly verify job; test `one_viewer_cannot_take_every_served_slot`; `served` benchmark unchanged in `bench-check`.

**WP-R6 prompt 20, read-response masking** (`agent/20-read-masking`, L; built later, when the human schedules it, and before prompts 15 and 08 T5; A4 decided). Closes C-4 (build) and the blob half of S14b. Its work order is `prompts/20-read-masking.md` (written 17 Oct 2026 from a read-only investigation of the registry, the engine's scan, the contract and the budgets; three independent reviews followed: facts, security and performance, and feasibility under the path guard. They returned 62 findings, 4 of them blocking, which are two defects: scan results and the cached diff were keyed by a hash that does not identify the scanned bytes (found independently by two reviewers; a token in a comment could reach a Viewer through another file with equal settings), and an acceptance item that could not pass. All 62 are folded into the prompt, and its length (about 5,400 words, against 1,300 for prompt 05) is the cost: the design decisions are in the prompt so that the planner does not have to rediscover them).

- What it builds, in ten tasks: one scan source and a scan-version fingerprint for the registry (T1); a pure, linear, total masker with byte-exact `Mask` offsets, a type that makes unmasked text unrepresentable, and a fuzz target (T2); a bounded scan cache with its own pool, permits and per-user cap (T3); masked file content with a scan-version ETag (T4a) and masked served text (T4b); masked compare hunks and inline ranges at the REST edge (T5); masked search hits that scan the whole blob first and drop a hit that would act as an oracle (T6); the blob policy that refuses a flagged text blob (T7); a planted-key test complete by construction over every GET operation (T8); the threat model, decisions and interfaces (T9). Four new budget rows (`P7.mask_scan`, `P7.file_content_scan_worst`, `P7.compare_api_masked`, `P7.blob_policy_scan`), each with its own bench file and a same-commit edit of `crates/xtask/tests/budgets_registry.rs`.
- The ports stay raw: hub-writes reads raw bytes through `RegistryRead::content` for edits, PRs, previews and reveal, and hub-mcp masks its own copy.
- Fourteen defaults (the "Defaults the plan confirms" section of the prompt) answer the open questions of the investigation and of the review, among them: scan results are keyed by the hash of the exact bytes scanned, never by an effective hash (two files with equal settings and a token in a comment would otherwise share a verdict); no scan version existed (it is defined in the registry from the thresholds, the input limit and what the engine flags over a run-time probe); the contract's `Mask.line` counts line feeds only while the scan also ends a line at a lone CR (converted on the server, no contract change); the search extraction oracle of item 1 is closed at the REST edge without a port change, while compare equality (item 3) is recorded, not closed; only deterministic scan failures are cached; the scan has its own pool and bounds. The human confirms them when approving the prompt's plan.
- Extra-paths its plan will list: `docs/decisions.md`, `docs/interfaces.md`, `crates/xtask/tests/budgets_registry.rs`; contract change: the new `perf/budgets.toml` rows (`registered = false` first, as 05, 06 and 07 did). No migration, no OpenAPI change, no engine edit, no new dependency.
- Prompt 08 T5's masking test and edit-mode switch-off depend on this package.

Verify: `just verify-20`, `just verify`, `cargo xtask bench-check --strict`, the registry threat-model rows for S13 and S14b updated and D128's residual closed.

**WP-R7 prompt 19, admin API** (`agent/19-admin-api`, L; after C3, P3). Closes C-3 (build), H-22 (build), H-11 (build).

- T1 registration of projects, swimlanes, sentinels, agents (`adminPutAgent` writes `swimlanes.agent_service_account`) and join tokens (`adminIssueJoinToken`); every one is Admin-only and audited; T2 every `admin*` operation not built by 03a, `adminListAudit` with the D38 fields; T3 the config routes against the aligned schemas. Each write audits in its own transaction and publishes `swimlane_changed`. The composed route table has no unimplemented operation apart from prompt 14's. Verify: `just verify-19`, `just verify`.

**WP-R8 prompt 21, the first admin and break-glass** (`agent/21-first-admin`, M; proposed, needs A19; after R1 and before the pilot). Not an audit finding: the audit assumed the first admin exists, and the investigation of 17 Oct 2026 found that the way the repository means to create one has gaps. The owner's rule is that only admins add admins.

What exists today (checked 17 Oct 2026, with file and line):
- `IdentityConfig.bootstrap_admins` (at most 32 Entra object ids, `crates/hub-identity/src/config.rs:16-17, 178-201`) makes a listed oid an Active Admin at its first sign-in only (`users/mod.rs:357-362`). The event is audited (`bootstrap_event`, `users/mod.rs:511-529`) but nothing notifies the admins.
- Nothing loads the list: `crates/hub` does not link hub-identity (`main.rs:33-35` refuses to start), the chart is a stub, and prompt 09 says only "Helm values".
- Recovery after a lockout is "a database intervention" (B13, `docs/threat-model.md:296`): a hand-made `UPDATE users`, which leaves no audit event, no log line and no notification, and is indistinguishable from an attacker with database write.
- The rule "only admins add admins" holds at the routes: no self-approval, no change of one's own role, status or flag, and a last-admin guard under a lock, each with a test (`tests/users.rs`, `users/rules.rs`). The service layer does not re-check the actor, and no test pins that `users.role` is written only by hub-identity.

Gaps found, high first: (1) the list is a standing admin-minting channel for anyone who can change configuration (Helm, ArgoCD, a deploy-repo PR) while real admins exist; (2) creating a bootstrap admin is silent; (3) a listed id that never signs in is a pre-authorised seat that never expires, and one mistyped hex digit hands it to another person; (4) ordering trap: the list helps only if deployed before the person's first sign-in, else the user is pending and only SQL helps; (5) medium: a deleted user row re-arms the id, no step-up for admin grants, delivery of the list undefined, the service layer trusts the actor; (6) low: the bootstrap decision ignores the tenant (safe only through the current call path), and the event is labelled `via: ui`.

**Proposal (option 4 of six; the one I recommend).** The first admin, and any recovery, is created by an operator command with no HTTP surface, and the `bootstrap_admins` list is deleted. Nothing is deployed with the list, so removing it costs nothing. After the first admin exists, only an admin can add an admin; the only other path needs cluster access, database credentials and leaves a chained audit event.

1. `docs/decisions.md`: a new D# (dated pointer, not a silent rewrite) replaces the last sentence of D35 ("The first admins come from configuration") and answers B13's open break-glass question; Q21 is answered "Entra assignment required: yes", so only assigned users can reach the hub and create a pending row.
2. `crates/hub-identity`: delete the list, `MAX_BOOTSTRAP_ADMINS`, the error and the bootstrap branches of `sign_in_tx`, so every first sign-in is pending with a request and a notification (`deny_unknown_fields` makes a stale `bootstrap_admins` value stop the hub at start). New `users/first_admin.rs`: `FirstAdmin::promote` in one transaction, in the existing lock order (admin-set lock, user row lock, chain): the user must already exist (the command never inserts), the expected email must match (a typo guard, not an authority), it counts active admins and refuses unless there are none or `--break-glass` is given with a reason, sets Active and Admin, ends the user's sessions, closes the open access request, and writes one chained audit event as the last statement (actor `System` with component `first-admin` or `break-glass`, `via: system`, the before and after of role and status, `active_admins_before`, the operator's pod or job name, the reason). A break-glass also sends a Critical notification and moves a counter. `approve` and `update_user` re-read the actor's standing inside the transaction.
3. `crates/hub` (prompt 18 now owns it): `hub admin list-pending` (read-only, at most 50 rows) and `hub admin first-admin --oid --expect-email --reason [--break-glass]`; arguments parsed with `std` only (no new dependency, rule 15), dispatched before the server and telemetry start, no listener, every call bounded by a timeout, a non-zero exit on every refusal, output limited to ids and the outcome.
4. Deployment (prompt 09, text added by P2): `admin-job.yaml`, disabled by default and never synced by ArgoCD, same image and S16 security context, no service-account token, the CloudNativePG app credentials, labels that let the S18 egress policy reach Postgres and nothing else; `deploy/RUNBOOK-first-admin.md` (sign in once, list pending, run the command with `--expect-email`, check `adminListAudit`, and the break-glass procedure); `create job` and `pods/exec` in the hub namespace restricted to a platform-owner group and alerted on in Cloud Audit Logs, which is where the human principal is recorded.
5. Contract (in C3): `AuditAction::AdminBootstrapped` in `crates/ports/src/audit.rs` and its wire-name snapshot; no migration (`audit_events.action` has only a length check), no OpenAPI or proto change, because there is no route. If you prefer no contract change, the event reuses `RoleChanged` with a diff that starts `first-admin;` or `break-glass;`, and is not typed.
6. Documents: `docs/threat-model.md` lines 217, 278 and 296, `docs/domain-model.md:220`, and one sentence in S4 (an admin is created only by an admin or by the operator command; this strengthens S4 and relaxes nothing).

Ownership: the prompt owns only the new files (`users/first_admin.rs`, the hub subcommand module, their tests); its plan lists as Extra-paths `crates/hub-identity/src/{config.rs,lib.rs,users/mod.rs,users/admin.rs}` and `crates/hub/src/main.rs`.

Tests, each against real Postgres as `lanekeeper_app` unless stated: `first_admin_promotes_the_pending_user_when_no_active_admin_exists` (one audit event as the last statement, sessions ended, request closed); `first_admin_refuses_when_an_active_admin_exists_and_writes_nothing`; `first_admin_never_creates_a_row_for_an_unknown_oid`; `first_admin_refuses_when_the_expected_email_differs`; `first_admin_refuses_a_user_of_another_tid`; `break_glass_writes_a_distinct_event_and_notifies_critical`; `break_glass_without_a_reason_is_refused`; `concurrent_first_admin_commands_and_approvals_leave_the_rules_state`; `a_failed_audit_write_rolls_back_first_admin`; `first_admin_event_verifies_clean_in_the_nightly_chain`; `every_first_sign_in_is_pending_whatever_the_oid` and `identity_config_refuses_bootstrap_admins` (replacing the three bootstrap tests); `user_admin_refuses_a_non_admin_actor`; `a_request_never_changes_the_requesters_role`; a source-scan test that only hub-identity writes `users.role` and `users.status`; in `crates/hub`, `admin_command_opens_no_listener` and an exit-code test for every refusal; in prompt 09, `helm template` with the Job disabled renders no Job and with it enabled passes the S16 and S18 policies.

If you decline the command, the fallback is option 2: the list is honoured only while no active admin exists (about ten lines, under the admin-set lock), a Critical notification is added to its creation, and prompt 18 adds a loader whose source is access-controlled (my suggestion: not a Helm value that anyone who can merge to the deploy repo can change). It closes the standing channel after the first admin but not the typo risk, the ordering trap or the undocumented recovery, so it is a weaker answer.

### Wave D: the existing prompts, resumed

Run with the amended text of P2: **08** (after C1 to C4 for one client generation, R6 for T5), **09** (H-17: collation, database owner not the app role, `Mcp-Session-Id` affinity, migration Job, `LK_REQUIRE_DOCKER=1`, crate fuzz targets, `/auth/*` rate limit, config-server egress, `osv-scanner` over every lockfile, a gates job for every merged prompt), **14**, **17** (D123 contract; the install is human-only), then **15** (S1 to S25 with S14b; the human-owned residuals) and **16** (P1 to P15, the real NFS mount test, `bench-check --strict`), and 10 and 11 as their gates allow.

### Wave E: optional docs restructure

**WP-P5** (`docs/audit-restructure`, M; only if A14 says yes, after everything above). M-17 (split `docs/threat-model.md` into `docs/threat-model/NN-<component>.md` so section ownership is a path) and M-25 (move the bodies of D89 and later to `docs/decisions/Dnnn.md`, keep a one-line index entry each, and cut the mandatory reading). Both files have readers outside the documents. The threat model is read by `crates/{agent,engine,hub-identity,hub-mcp,hub-platform,hub-registry,hub-writes}/tests/threat_model.rs`, by `crates/agent/HARDENING.md` and `crates/hub/HARDENING.md`, and by the `lanekeeper-pr-review` skill. The decisions are read by `crates/hub-mcp/tests/{t0_status,threat_model,cursor_admin_doc}.rs`, `crates/hub-mcp/examples/{t0_status,t0_stub}.rs`, `crates/hub-mcp/{plugin,t0}/README.md`, `crates/xtask/tests/budgets_registry.rs` and `scripts/lk.test.mjs`. (Found with `grep -rl`; P5's plan repeats the search at its own base.) The plan lists the test and example files as Extra-paths and changes only their path constants; the old paths stay as index files so a stale reader fails loudly. Verify: every crate's `threat_model` test, `cargo test -p xtask`, `just lk-test`, and the section and decision counts equal before and after.

## Order and dependencies

```
Gate 0 ──► P1 ──┐
           P2 ──┤ (P3 needs A2, A3)
           P3 ──┤
           P4 ──┘
Gate 0 ──► C1 ─► C2 ─► C3 ─► C4          (merge order; written in parallel)
C1 ─► R1 (also needs P3)    C1 ─► R2    C1, C4 ─► R3 ─► R4    C2 (+A11) ─► R5
A4 ─► R6    C3, P3 ─► R7
R0 ─► R1  (R0 first, A3)              R1, C3, A19 ─► R8 (before the pilot)
then: 08, 09, 14, 17 ─► 15, 16        P5 last, if at all
R6 (prompt 20) is scheduled by the human; it must merge before 15 and 08 T5, or go-live needs an exception (see Risks)
```

Files that every package may touch and so need append-only care and a rebase before each merge: `api/openapi.yaml` (C1 to C4), `docs/decisions.md` and `docs/open-questions.md` (every package that records a D# or a Q#, taking the next free number at merge time), `prompts/README.md` (P2, P3), `Justfile` and `Cargo.toml` (P2, R1).

## Traceability

Every audit ID, its status at `20155b4`, the package that closes it and the proof. "New" names a test or check this plan asks the package to write.

| ID | Status at `20155b4` | Package | Proof |
|---|---|---|---|
| C-1 | open | R0 (guard), P3 (prompt), R1 (composition) | `guard.rs` asserts 403 without token or `Origin` (R0); `route_table_full`; `csrf` over every non-GET composed route (R1) |
| C-2 | open; CG6 deferred to `contract/08-adoption` | C2, R5 | `new_operations_name_their_audit_action_or_say_why_not` rows; registry conformance with `set_open_adoption`; a confirm unlocks writes end to end (new) |
| C-3 | open; CG15 deferred | C3, P3, R7 | no unimplemented spec operation in `route_table_full` |
| C-4 | contract merged, build missing (D128) | R6, A4 | planted-key test over every read operation; blob policy test; bench row |
| H-1 | closed in spec | C1 (residual) | `allof_composites_validate_real_bodies` |
| H-2 | contract merged, build missing | R3, R4, P2 (skill), C4 (doc) | `a_marker_read_through_read_file_and_written_back_is_refused`; skill evals |
| H-3 | open | R4, C4 | seeded-marker test in `read_tools.rs` |
| H-4 | REST half closed | C4, R3 (mount), R4 | `listMyDrafts` in the route table; draft-mode summaries test |
| H-5 | open | P2 (text), R4 (defaults) | acceptance rows in 09 and 15; `mcp-t0-status --strict` at go-live |
| H-6 | open | C1, R0, R2, R3 | `every_no_store_operation_uses_the_no_replay_key`; a write without the header answers 400 on every write route (new, in `route_table_full`) |
| H-7 | partly closed | C1, R3 | `every_operation_declares_429_and_503`; `agent_unavailable` is 503 (new) |
| H-8 | partly closed | C1, R2, R3 | conflicts answer `ConflictState` with `conflict` (new) |
| H-9 | spec half closed | C1, R0 (A5: no rename) | pending, disabled and low role give three distinct problems on a platform route (new) |
| H-10 | open | R1 | `composed_hub_route_table_matches_openapi` |
| H-11 | open | C4, R7 | audit read schema test with `sync`, `agent_detected` and `chain` (new) |
| H-12 | mostly closed | C4, R3 | workaround deleted; `params` and `pr_job_id` in the conformance |
| H-13 | open | P2, P1 (protocol) | the grep check of P2 |
| H-14 | open | P1, P2 | review of the Stop sections |
| H-15 | open | P1 | `a_verdict_is_line_one_and_nothing_else` |
| H-16 | open | P2 (text), C4, R0 (stamp) and R1 (job), A12 | fake-hub tests in prompt 17; heartbeat stamp and job test (new) |
| H-17 | open | P2 (text), Wave D 09 | prompt 09's acceptance |
| H-18 | open | P2, P4 (D70), Wave D 15 and 16 | acceptance rows |
| H-19 | open | R3 | `a_delete_through_the_tool_is_attributed_tool_write_and_raises_no_file_change` |
| H-20 | open | R2 | `a_restart_between_two_unfinished_nights_still_alerts` |
| H-21 | open | R5, A11 | `an_edit_to_application_sit1_moves_the_effective_hash_and_the_drift` |
| H-22 | open | C3, R7 | `admin_schemas_name_the_columns_of_their_tables` |
| H-23 | open | P2 (text), Wave D 16 | `crates/agent/tests/nfs_real_mount.rs` run against a pilot mount |
| M-1 | open | R1 | streaming depth test and fuzz target `depth_scanner` |
| M-2 | open | R1 | a panicking handler answers a 500 problem (new) |
| M-3 | open | R1 (after R0's B16 types) | an anonymous caller gets 401 and a pending user 403 under `/api` (new) |
| M-4 | open | C1 | `return_to` pattern test with `/\t/evil.example` (new) |
| M-5 | open | C1 | `path_and_ref_patterns_agree_with_their_parsers` |
| M-6 | open | C1 | `every_body_operation_declares_413` |
| M-7 | open | C4 | the paging convention test |
| M-8 | mostly closed | C1 | `EventKind` note; `web_contract_events` stays green |
| M-9 | open | C1 | header declarations in the conformance |
| M-10 | open | C1 | `redocly lint` and the documented flow |
| M-11 | open | C4, A10 | `Attribution::unknown()` is `none`; migration test |
| M-12 | open | C3, P2, P4 | the three operations are `deprecated` |
| M-13 | open | C4, A6 | the order is stated in both contracts |
| M-14 | open | C4, R4 | `mcp_tools_doc` test with the naming map |
| M-15 | open | C4, R3, R4 | one marker pattern across hub-writes and MCP |
| M-16 | open | C4, R4 | the Errors paragraph; hub-mcp tests |
| M-17 | open | P1 (script), P2 (text), P5 (split, A14) | `root_files_never_grant_protected_paths` |
| M-18 | open | P2 | README table |
| M-19 | open | P2 | the Blocking questions rows against `open-questions.md` |
| M-20 | open | P1, P2 | every task has a Verify (a script check, new) |
| M-21 | open | P2 | skill evals including the injection scenario |
| M-22 | open | P4 | review files exist for 03a, 05 (retrospective), 06 |
| M-23 | open | P4, Wave D 15 | open questions and the confirmation table exist |
| M-24 | open | R5 | `one_viewer_cannot_take_every_served_slot` |
| M-25 | open | P5 (A14) | counts equal before and after |
| Low batch | open | C1 (openapi), P2 (prompts), P4 (records), R1 (code) | diff review |

The hand-off ledger of the audit (§4) maps through its "Fix" column to the IDs above; its two rows marked "—" were already `Yes`.

## Tests, benchmarks and fuzz targets

New guards this plan adds, besides each package's own tests:

- Contract: `every_no_store_operation_uses_the_no_replay_key`, `every_operation_declares_429_and_503`, `every_body_operation_declares_413`, `path_and_ref_patterns_agree_with_their_parsers`, `allof_composites_validate_real_bodies`, the response-array paging test, the audit-action rows for every new operation.
- Composition: `composed_hub_route_table_matches_openapi` (explicit allowance lists only), the CSRF test over every non-GET composed route, the jobs test (a job exported by 05 or 06 and not registered fails).
- Behaviour: marker round trip, delete attribution, restart between unfinished nights, one viewer and the served slots, the effective hash of `application-<tenant>.*`, the planted-key test over every read operation.
- Agent protocol: the verdict parser and the PROTECTED-first guard tests in `scripts/lk.test.mjs`.
- Fuzz: `crates/hub/fuzz/depth_scanner` (R1); nothing else parses a new untrusted format.
- Benchmarks: a row for the depth check (R1) and one for the masking scan (R6) in `perf/budgets.toml`; P7 auth re-measured with the CSRF step; `cargo xtask bench-check --strict` after every Wave C package.
- Integration tests run against real Postgres as everywhere else; `LK_REQUIRE_DOCKER=1` in CI (09).

## Extra paths and contract changes

- **Contract changes (a human approves each, label `contract-change`).** C1 to C4 (`api/openapi.yaml`, `docs/mcp-tools.md`, `crates/domain`, `crates/ports`, `db/migrations` for `none` confidence and `sentinel_silent`); `perf/budgets.toml` rows in R1 and R6.
- **Extra-paths.** R1: the four hub-platform guard files and their tests, `Cargo.toml`, `Justfile`. P5: the test, example and `HARDENING.md` files that read the two split documents (listed under WP-P5). Wave D 16: `crates/agent/tests/**`. Every other package owns its paths and its tests, benches and fixtures.
- **Protected paths.** P1 to P3 edit `.claude/`, `AGENTS.md` and `prompts/`; they run on ordinary branches, outside `agent/*` worktrees, and a human reviews them as the only backstop.
- **No new dependency.** `jsonschema =0.58.3` is already in `Cargo.toml` (hub-mcp). C1's validator test adds it as a dev-dependency of `ports`, with the same version and no new features; the PR says so (rule 15).

## Ambiguities and open questions

Every row has a default, so a silent approval means the default. "Yes" means a human has to choose because a decision, a requirement or an interface is involved.

| Id | Question | Where it came from | Proposed default (configurable) | Needs a human? |
|---|---|---|---|---|
| A1 | **Adoption scope.** The audit asks for three operations; `contract/08-web` Q2 and Q13 foresaw the whole wizard (CG6). Does C2 carry the wizard, `lost_version` on `Finding`, and the mark and clear operations? | C-2; `plans/contract-08-web.md` Q2, Q13 | **Decided 17 Oct 2026: the full wizard.** The wizard, the marks and `lost_version` (my reading of "full"; see WP-C2) | Decided |
| A2 | **Who owns admin registration, and where does an agent's service account live (S5)?** | C-3; D112 A7; CG15 named 05 | **Decided 17 Oct 2026:** only admins can add admins and register swimlanes, agents and sentinels (`x-required-role: admin`, no self-promotion, last-admin guard; these hold at the routes today, with tests). The service account is `swimlanes.agent_service_account` and `sentinels.service_account`; both columns exist and the join reads them, so no stop is needed (WP-C3). Owner of the operations: prompt 19 and `crates/hub-admin`. The first admin is A19 | Decided |
| A3 | **How does hub composition touch `hub-platform`?** The CSRF step and the B16 types live in `crates/hub-platform`, which prompt 03b owns. | C-1; AGENTS.md ownership | **Decided 17 Oct 2026: a 03b fix first** (WP-R0, no Extra-paths: 03b owns `crates/hub-platform/**` and `crates/hub/**`) | Decided |
| A4 | **Who builds M1 masking, and what happens to the search and compare oracles?** D128 reported the gap; the contract names 05. | C-4; D128; `docs/interfaces.md` open items 1 and 3 | **Decided 17 Oct 2026: a separate prompt (20) that builds it later** (R6). The two oracles are recorded as open questions with "accept for the pilot only, with a D#" until 15 decides. Not on the build path of anything but prompt 08 T5 and prompt 15's S14b check, but still a go-live gate: see Risks | Decided |
| A5 | **Problem type spelling.** The audit normalises to snake_case; the merged contract says "nothing is renamed". | H-9; `Problem.type` description | **Decided 17 Oct 2026: no rename.** Platform routes send identity's B16 types (`access-pending`, `access-disabled`, `insufficient-role`) and C1 lists them | Decided |
| A6 | **Drift order**, most serious first (REST) or by path (port and MCP). | M-13; D103, D108 | Both are stated where they apply now; a human picks one for both | **Yes** |
| A7 | **Retention (D116, D118).** | audit §5; D116, D118 | `retention_dry_run = true` until a human confirms | **Yes** |
| A8 | **The 26 decisions "to be confirmed at review".** | M-23 | A table in P4; none counts as confirmed until a human signs a line | **Yes** |
| A9 | **D109 D-1 and the Cursor T0 test.** | H-5; D91 | Ship `advertise_output_schemas: false` and `LK_MCP_CONFIRM=draft` until `mcp-t0-status --strict` passes | **Yes** |
| A10 | **`none` confidence**: add the value (migration) or amend D73. | M-11 | Add it | **Yes** |
| A11 | **Effective hash for `application-<tenant>.*`**: the engine adds the layer, or 05 recomputes through `property_view`. | H-21; 04 review note 6 | The engine adds the layer to `Layers`; 05 recomputes | **Yes** |
| A12 | **Sentinel silence**: add `FindingKind::sentinel_silent`, stamp `last_seen_at`, add a leased job. | H-16; D123 | Yes, with R1 owning the hub half | **Yes** |
| A13 | **REST `ty` to `type`**: a rename of a field in a contract no client has generated against yet. | M-14 | Document the mapping only; rename only if 08 has not generated its client when C4 merges | **Yes** |
| A14 | **Split `threat-model.md` and `decisions.md`** (P5). Both are read by tests. | M-17, M-25 | Do it last, with the test constants changed, or drop it; the other M-17 and M-25 effects (exact paths, PROTECTED first, citing D# instead of paraphrasing) do not need it | **Yes** |
| A15 | **How a fix round of a merged prompt runs in the harness.** `lk worktree NN` makes `agent/NN-slug`, the name the merged branch used. | Wave C | By hand per AGENTS.md, branch `agent/NN-fix-name`; optionally P1 T5 adds `--slug` | No |
| A16 | **Where the owner-crate fixes of H-19, H-20, M-24 go.** The audit puts them in prompt 18. | audit §5 | Fix rounds in the owner crates (R2, R3, R5), so the owner's reviewer sees them; prompt 18 stays about composition | **Yes** |
| A17 | **`lock` cause on `LockedProblem`** (CG4b, deferred; 6 producers in hub-writes). | C-2; `contract-08-web.md` | Defer; the web derives the cause from `baseline`; only the description changes (C1) | **Yes** |
| A18 | **Tightening `return_to`, `NfsPath`, `GitRef` and the blob hash.** The contract skill says additive only within a version. | M-4, M-5 | Approve them, because they only match what the server accepts already, with the differential test of C1 | **Yes** |
| A19 | **How the first admin is created, and whether D35 and B13 need hardening.** Today configured Entra object ids become admin at first sign-in only, and recovery after a lockout is a database intervention. The owner's rule is that only admins add admins. | A2; D35; B13; `docs/threat-model.md` "Bootstrap admins and recovery" | **Proposal (WP-R8, option 4):** delete the configured list; the first admin, and any break-glass recovery, is created by an operator command with no HTTP surface, that promotes only a user who has already signed in, refuses while an active admin exists unless `--break-glass` is given with a reason, and writes one chained audit event. Amends D35 (last sentence) and B13; answers Q21 "assignment required: yes". Fallback if declined: the list is honoured only while no active admin exists, plus a Critical notification. Until you decide, D35 and B13 stay as built | **Yes** |
| A20 | **Uniqueness of service accounts.** Nothing stops two swimlanes, or a swimlane and a sentinel, holding the same account, so one account could join as both. | C3 research of 17 Oct 2026 | Add partial `UNIQUE` indexes on `lower(...)` of both columns and a shape check in a new migration `0009`, and refuse a cross-table duplicate with 422 in the admin transaction | **Yes** |

## Risks

- **`api/openapi.yaml` is a merge hot spot.** Four contract PRs and several prompts edit it. Merge in order, rebase before each, and regenerate snapshots after each rebase, never by hand.
- **A default-deny `verify_csrf` breaks every existing `SessionAuth` implementation and test** (platform fakes, hub tests). R0 updates them in the same commit and the 403 test replaces the 200 assertion, so a lost step shows as a red test and not as an open route.
- **Applying 429, 503 and 504 to 99 operations makes a large generated-client diff.** C1 lands before prompt 08 generates its client, so it is generated once.
- **Tightened patterns could reject input that a client sends today.** They are copied from the parsers, and the differential test fails on any drift; A18 makes the tightening a human decision.
- **A required `Idempotency-Key` on six writes that ignore it today** breaks callers that never sent one. Both the contract and prompt 08 already require it; R2 and R3 add tests for the 400.
- **Masking at the REST edge costs a scan per content hash and a cache.** R6 measures it against a new budget before it is merged and keeps the cache bounded (rule 5); if the budget cannot be met the package stops and escalates with measurements (AGENTS.md).
- **Splitting the two big documents breaks readers silently.** P5 is last, optional, and keeps the old paths as index files.
- **Masking is deferred (A4), and that is a go-live risk, not only a schedule choice.** Until prompt 20 merges, any signed-in Viewer, and the MCP tools that sit on the same ports, receive flagged values in clear in file text, diffs, search lines and blobs. D18 says config holds no secrets, but Q20 names six files whose long values nobody has confirmed are public keys or ids, and the scan that flagged them looked at lengths, not values. S14b is a security requirement and the threat model calls the gap "not an accepted risk". So go-live needs either prompt 20 merged or a human exception in `docs/decisions.md` with conditions. A cheap stopgap for an exception: put the Q20 files, and any other file the owners know to hold values, under the hub's deny globs, so the registry never serves their text (`is_denied` withholds it from every read path). Nothing else in this plan waits for prompt 20 except prompt 08 T5 and prompt 15's S14b check.
- **The first admin is on the path to the pilot.** Without one nobody can approve users or register swimlanes and agents. As built, the configured list that would create one is never loaded by the hub, so today there is no working way to make an admin except SQL. A19 and WP-R8 close that; if A19 is not decided, prompt 18 must at least load the list, or the hub starts with no admin.
- **Human-owned items can stay open.** A4, A6, A7, A8, A9 and A12 end in a decision or a recorded exception; P4 turns each into an open question, and prompt 15's acceptance fails while one is neither closed nor excepted, so none is hidden.
- **The protected-path edits of Wave A have no hook.** Reviewers see the diff; the tests of P1 and P2 check that `lk.mjs` still parses every prompt.

## Not verified

- No `cargo`, `just`, `redocly` or `pnpm` command was run. The only executions were `git`, `grep`, a Python read of `api/openapi.yaml`, and `jsonschema` 4.26 (Draft 2020-12) on two bodies.
- I did not re-read the code behind every Medium finding. For findings that cite only files untouched since `db6ea42`, I relied on `git diff` to say they stand, and spot-checked the four listed in "Re-baseline".
- The audit's line numbers into `docs/mcp-tools.md` and `api/openapi.yaml` may have shifted after PR #20; each package re-reads before editing.
- `plans/08-frontend.md` (the approved plan that `contract-08-web.md` cites) is not in this clone, so what prompt 08 has built is unknown here; the order of C1 before 08's client generation assumes it has not generated one.
- The ledger rows for P11 and Q20 (marked `Yes` in the audit) were not re-checked.

## Docs read

- `AGENTS.md`, `CLAUDE.md`, `prompts/README.md`, `plans/TEMPLATE.md`, `plans/contract-08-web.md` (decisions, CG6, CG15, "What each owner must implement"), `docs/decisions.md` (D125 to D128 in full, the numbering), `docs/open-questions.md` (format, last number), `docs/interfaces.md` ("What each owner must implement", open items 1 to 6), `docs/threat-model.md` (the PR #20 and PR #19 diff), `.claude/skills/lanekeeper-contract-change/SKILL.md`, `.claude/skills/run-prompt/SKILL.md`, `.claude/protocols/implementer.md`, `scripts/lk-lib.mjs` (the PROTECTED check), `Justfile` (recipe names), `api/openapi.yaml` (parsed), and the code lines named in "Re-baseline".
- The audit's findings in `docs/security.md`, `docs/performance.md`, `docs/domain-model.md` and `docs/mcp-tools.md` were taken from its citations, not re-read; each package's planner reads the full list that AGENTS.md prescribes.
