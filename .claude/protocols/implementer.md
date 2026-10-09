# Implementer protocol (shared by rust-, web- and ops-implementer)

You receive: the worktree path, the prompt id `NN`, and a mode.

## Always

- Work only in the worktree. Start every shell command with `cd <worktree> &&`.
- Read AGENTS.md "Code rules" and the approved plan `plans/NN-slug.md`. For a task, read only that task's section of `prompts/NN-*.md` and the doc sections the plan lists for it.
- The plan is frozen. If it does not fit reality, an interface does not fit, a decision blocks you, a budget cannot be met, or a security requirement conflicts with a functional one, do not work around it. Write the problem, with measurements, to `plans/NN-slug.blockers.md`, commit it as `NN/blocked: <summary>`, and return `BLOCKED: <one line>`.
- The guard hook blocks edits outside the prompt's owned paths. A block means escalate (above), never try another route to the same file.
- Never skip a gate, a hook or a test. Never weaken a test to make it pass.
- One commit per unit of work, never `--no-verify`.

## Mode `Tk` (one task)

1. Read the task's acceptance rows in the plan's traceability table.
2. Write the failing tests, benchmarks or fuzz target first (test-driven-development). Run them and watch them fail for the right reason.
3. Implement the minimum that passes, following the task text.
4. Run the task's **Verify** command from the prompt. Fix until it passes. Use systematic-debugging for failures; do not guess.
5. Run the formatter and linter for what you touched.
6. Commit everything for this task as `NN/Tk: <summary>`.

## Mode `FINAL`

1. Run the gate for the prompt (`just verify-NN`), then `just verify`, then `cargo xtask bench-check` for Rust prompts. Fix real failures with a `NN/fix:` commit. Never skip.
2. Create `plans/NN-slug.evidence.md` from `plans/EVIDENCE-TEMPLATE.md`. Fill it with real output (command, tail of the output, pass/fail), the ticked acceptance list with proof for each item, the S# implemented, the benchmark numbers against budgets, new dependencies with their reasons and ages, and your assumptions. Do not tick anything you did not prove.
3. Commit as `NN/evidence: <summary>`.

## Mode `FIX`

You receive the reviewer's blocking findings. Apply the receiving-code-review skill: verify each finding against the code first, fix the real ones with tests, and say why for any you reject. Re-run the gates. Commit as `NN/fix: <summary>`. Update the evidence file if numbers changed.

## Return (at most 15 lines)

`DONE` or `BLOCKED`; the commit hash; what you ran and the result; anything the orchestrator or a human must know.
