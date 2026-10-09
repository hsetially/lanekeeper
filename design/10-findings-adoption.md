# 10: Findings and adoption

## Prompt

```
Using the Lanekeeper design system, design:

1. FINDINGS
   - A table filterable by swimlane, check (C1 to C8) and severity.
   - Each row: check name in plain words, swimlane, file, a one-line explanation, line range link.
   - Examples:
     - C1 "sitc's tenant copy of entryGroupConfig.yml has no atm entry that base has" (shows the base commits since the tenant copy);
     - C2 "account-sorting-config-sit7.yml is identical to base and blocks future base changes";
     - C6 "presita references a dev host";
     - C7 "A sync overwrote an NFS change" (a "Restore lost version" action);
     - C8 "Duplicate keys in security-roles.yml at lines 5171, 5643 and 6115".
   - A detail drawer with the evidence and recommended next steps. Actions: open file, open diff, mark intentional (where allowed).

2. ADOPTION WIZARD for swimlane presitb (Admin; no baseline yet)
   - Step 1, branch: ranked candidates (sit12 94% identical, sit11 81%, sit9 60%), with env and release hints beside them.
   - Step 2, base version: best-matching commit/tag ("v2.3.1 + 4 commits").
   - Step 3, differing files: a table of each file and its difference, with choices "NFS is right", "Git is right" or "Intentional (reason)"; bulk actions; a diff preview.
   - Step 4, secret scan results: files with suspicious values (line numbers and lengths only).
   - Step 5, confirm: summary, and "Confirm baseline and enable editing".
   - Progress persists; the wizard can be resumed.
```

## Follow-ups

- "Make findings explanations readable by someone new to the system: one sentence, then the evidence."
- "Show the wizard after confirmation: success state and what changed (editing unlocked)."

## Done when

- [ ] Every finding type has an example and a detail view.
- [ ] Every wizard step is designed, including resume, bulk actions and success.
