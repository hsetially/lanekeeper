# Plan NN: <title>

Status: DRAFT
Contract-change: none
Extra-paths:
Prompt: prompts/NN-<name>.md
Branch: agent/NN-<name>

<!--
Header rules. A human approves the plan; the orchestrator then flips Status to APPROVED and the plan is frozen.
- Contract-change: none | needed. When "needed", describe it below. A human decides.
- Extra-paths: backticked paths this plan needs beyond the prompt's owned paths, each with its reason below. Empty if none.
-->

## Docs read

List the docs and the sections that shaped this plan.

## Tasks and commits

| Task | Commit message | Files and types created or changed | Verify (from the prompt) |
|---|---|---|---|
| T1 | `NN/T1: ...` | | |

## Traceability

Every acceptance criterion, S# and P# in the prompt, each proved by a named test, benchmark or fuzz target.

| Requirement (acceptance item, S#, P#) | Task | Proof (test, bench or fuzz target name) |
|---|---|---|

## Tests, benchmarks and fuzz targets

Unit, property, golden, integration (real Postgres), fuzz for every parser at a trust boundary, criterion benchmarks registered in `perf/budgets.toml`.

## Extra paths and contract changes

Reasons for each entry in the header. "None" if none.

## Ambiguities and open questions

| Question | Where it came from | Proposed default (configurable) | Needs a human? |
|---|---|---|---|

## Risks

The biggest security and performance risks, and how the plan contains them.
