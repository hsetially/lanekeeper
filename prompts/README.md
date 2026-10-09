# Running the prompts with coding agents

There are 18 prompts. Each one is a self-contained work order for one coding agent (Cursor or Claude Code), on one branch, producing one PR. Every prompt uses the same template:

- a metadata table: wave, dependencies, owned paths, interfaces, S# and P# requirements, and the gate command;
- the objective;
- ordered tasks, each with a `Verify` command;
- acceptance criteria;
- stop conditions;
- what's out of scope.

Agents follow the protocol in AGENTS.md: plan, wait for approval, implement task by task, pass the gate, and then the reviewer agent checks the PR.

## Design track (before and alongside wave 1)

Build the UI in Claude Design first, using `design/README.md` and prompts 00–14. The handoff bundle lands in `design/handoff/` through its own PR.

Prompt 08 can start T1–T3 (foundation, performance and live updates) before the handoff exists. The screens (T4) are built only from the handoff.

## Waves

Agents within a wave run in parallel. Each depends only on contracts and fakes, so none has to wait for another's code.

| Wave | Prompt | Depends on |
|---|---|---|
| 0 | 01 Contracts, skeleton, ports, gates, fixtures | Nothing. One agent; a human reviews closely. |
| 1 | 02 Agent | 01 |
| 1 | 03a Hub identity and audit | 01 |
| 1 | 03b Hub platform | 01 |
| 1 | 04 Fact engine | 01 |
| 1 | 08 Web app (against mocks) | 01 |
| 1 | 09 Deployment, CI and supply chain | 01 |
| 1 | 11 Cursor skill (draft) | 01 |
| 2 | 05 Registry, indexes and read API | 01, using fakes. Integrates with 03b and 04. |
| 2 | 06 Write paths | 01, using fakes. Integrates with 03a, 03b and 04. |
| 2 | 07 MCP server | 01, using fakes. Integrates with 05, 06 and 14. |
| 2 | 14 Docs search | 01, using fakes. Integrates with 04 and 05. |
| 2 | 17 NFS VM sentinel | 01, using fakes. Integrates with 03b and 05. Install needs Q30. |
| 3 | 15 Security verification | Waves 1 and 2 merged |
| 3 | 16 Performance verification | Waves 1 and 2 merged; staging deployed |
| Investigation | 10 Sync job reporting | Any time. Implementation waits for a decision. |
| v2 | 12 GLiNER, 13 v2 features | v1 |

## Human gates

- **Approve each plan** in `plans/` before the agent starts coding.
- **Review every PR** after the reviewer agent has passed it.
- **Approve every `contract-change` PR.**
- **Approve each design handoff,** and every entry in `design/DEVIATIONS.md`.
- **Sign off the threat model** before go-live (S24).
- **Answer the open questions.** Q1, Q2, Q4, Q11, Q17, Q24 and Q26 block the pilot.

## Starting an agent

Use this as the first message, in Cursor's Agent chat or in Claude Code:

> Follow the protocol in AGENTS.md for prompts/NN-name.md. Read the required docs in order, write plans/NN-name.md, then stop and wait for my approval.

After approving the plan:

> Plan approved. Implement it task by task, running each task's Verify command, then run `just verify-NN` and `just verify` and open the PR with the evidence AGENTS.md requires.

Then start a reviewer agent on the PR:

> Run prompts/REVIEW.md against PR #N.

## Orchestration (optional)

- **Vibe Kanban:** make one card per prompt, ordered by wave. Each card runs its agent in its own workspace and returns a PR. Check that it supports the agent setups your engineers use.
- **rtk:** shrinks the build and test output agents read, through hooks for Claude Code and Cursor. Install it with Homebrew, winget or cargo. Telemetry stays off unless someone opts in.

## Milestones

- **M1:** the agent in a pilot SIT cluster joins through Workload Identity, and heartbeats with Merkle roots arrive on any hub replica.
- **M2:** adoption is done for two pilot swimlanes, and drift, grid, whole-swimlane compare, text search and docs search all meet their P7 budgets.
- **M3:** every write path, approvals and user management work end to end, the audit chain is checkpointed, and SSE live updates work.
- **M4:** Cursor MCP is installed from the team marketplace, and elicitation is verified on Windows and macOS.
- **M5:** prompts 15 and 16 pass, the threat model is signed off, and agents are rolled out to every swimlane.
