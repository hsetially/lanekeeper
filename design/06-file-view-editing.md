# 06: File view and editing

## Prompt

```
Using the Lanekeeper design system, design the file view for sitb / tx-infinity-api/tx-infinity-core-sit7.yml.

HEADER
- Path chip.
- Logical file name.
- "Effective file for tx-infinity-core" marker.
- Drift badge (NFS ahead).
- File facts: CRLF, UTF-8, 412 lines, hash chip.
- Actions: Edit, Raise PR, Upload, Delete, History, Docs for this file.

TABS: NFS (current) | Git (sit7 head) | Baseline | Diff
- Diff tab: choose left and right refs (NFS, Git, Baseline, another swimlane, a tag). Side-by-side and unified views. A change list sidebar with "3 changes". Previous/next with n and p. A semantic summary on top: "2 settings changed, 1 added", listing setting paths.
- "Line endings only" flag example: a file where the only difference is CRLF vs LF, shown as a notice with the diff hidden by default.

EDITOR (Edit mode)
- Monaco-style editor with line numbers and a status bar showing CRLF, UTF-8, cursor position, "Validates as YAML".
- Inline validation: an error at line 88 ("mapping values are not allowed here"), and a warning for duplicate keys.
- Secret detection block: "Save blocked: line 120 looks like a private key", showing the line number only.
- Save bar: "Save to sitb NFS" (primary), Cancel, "Changes: 2 lines". For a user who requires approval, the button reads "Submit for approval".

HISTORY PANEL
Versions with time, source (tool write by Marco Diaz, scan "unknown author", sync), short hash, and Revert (with a confirmation).

LARGE AND BINARY FILES
- security/security-roles.yml (6,100 lines): show the editor with an outline/minimap of top-level keys, and a duplicate-key finding linking to lines 5171, 5643 and 6115.
- document-service/resources/receipt-logo.jpeg: binary preview, with a side-by-side image compare against Git and both hashes.

DOCS PANEL
A right-side drawer "Docs for this file", listing the Account sorting doc and its Feature Flags table for this file.
```

## Follow-ups

- "Make the diff work at 6,000 lines: show the virtualised scroll, change markers in the scrollbar, and sticky hunk headers."
- "Design the editing-locked state, and the read-only state for Viewers."

## Done when

- [ ] The diff, editor, history, large-file, binary and docs-panel states are all designed.
- [ ] The save button wording changes for users who require approval. The secret block shows line numbers only.
- [ ] The line-ending and encoding facts are always visible.

## Additional prompt (attribution, masking, denied files, inline highlights)

```
Update the file view:
- History panel: each version shows its attribution chip and severity. Include a version recovered after a hub outage, marked "recovered from agent spool".
- Diff: word-level highlights inside changed lines.
- Masked values: a flagged value shows as ••••• with a "Reveal" button (Editor+). Revealing asks for a reason, says "This will be recorded in the audit log", then shows the value.
- Denied files (for example keystore.jks): the file header shows "Content withheld: sensitive file type". Only name, size, hash and attribution are shown, and there are no edit actions.
```

## Additional prompt (rendered view, "As served", pickup)

```
Update the file view:
- A "Rendered" toggle on resource files. It shows the file with ${KEY} values filled from the property view, and marks each substituted value with its source file (for example application-sit7.properties). Placeholders left for the service's environment are shown as written, with a "resolved at runtime" hint.
- An "As served" tab: pick an application, tenant (sit7) and channel (remote-itm-teller or none), then show what the config-server returns. Badge it "matches rendered view" or "differs" (with a diff).
- After a save, an inline result panel shows the pickup state per affected service:
  - "Live by 14:32 (client cache)";
  - "Needs notify or restart";
  - "Needs config-server restart (new folder)";
  with a "Notify services" button (Operator).
```
