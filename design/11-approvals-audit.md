# 11: Approvals and audit

## Prompt

```
Using the Lanekeeper design system, design:

1. APPROVALS (Admin)
   - A queue with tabs Pending, Stale, Expired, Decided. Each row: author (Ana Ruiz), action (edit, upload, delete, restart, PR, AI-made change via Cursor), swimlane, file, age, expires in.
   - Detail: the diff or action parameters, the consequence summary, "Approve" and "Reject (reason required)".
   - "You can't approve your own proposal" state. A stale proposal: "The file changed since this was proposed", with "Ask author to update".
   - Updates arrive live.

2. AUDIT LOG
   - A filterable table: time, user (or "unknown author" for out-of-band changes), via (UI, Cursor, sync, detected), action, swimlane, file, hashes before and after, approval link.
   - A detail drawer with the diff and the event's chain hash.
   - An integrity panel: "Chain verified at 02:00, last checkpoint 14:00, signed", plus the alert state "Chain verification failed at event 18,442".
```

## Follow-ups

- "Make 'via Cursor' and 'unknown author' visually distinct and easy to filter."

## Done when

- [ ] The queue tabs, approval detail, self-approval block and stale state are designed.
- [ ] The audit filters, detail drawer and integrity states, including failure, are designed.

## Additional prompt (attribution in audit)

```
Update the audit log:
- An attribution column with a source filter (via Lanekeeper, edited on the NFS VM, sync job, NFS client, unknown) and a minimum confidence filter.
- The detail drawer shows the evidence: the OS Login user, the command (exe), the time, and the correlation window.
- "Attribution upgraded" events (unknown → named user) appear as linked entries.
- Reveal actions appear as audit entries, with the user, file, line and reason.
```
