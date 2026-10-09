# 09: Search, docs and feature status

## Prompt

```
Using the Lanekeeper design system, design the Search area and Feature status.

1. TEXT SEARCH
   - Query field (literal by default, a regex toggle with validation errors shown inline), case toggle, swimlane multi-select, path glob filter.
   - Results grouped by swimlane, then file, with line number, a highlighted match and a one-line context. "1,000+ matches, results truncated" notice. Each hit opens the file at that line.

2. SETTINGS SEARCH
   Query "enableAccountSorting" → rows of swimlane, file, setting path, value and line, with a quick "Open in grid" action.

3. DOCS
   - Search results show the doc title, heading path and snippet (for example "Account sorting > Feature Flags").
   - Doc reader: rendered Markdown with tables, code and a table of contents; "Related files" chips that open the files; a source label ("templates/common-docs" or "Uploaded by Priya Shah"). Git docs are read-only.
   - Upload and replace (Editor+): a drop zone (Markdown only, up to 2 MB), validation errors, and version history.

4. FEATURE STATUS
   - Rows: documented flags (enableAccountSorting…), with the doc link and the template default.
   - Columns: swimlanes, with a per-channel breakdown (remote-itm-teller, atm-iso).
   - Cells: the actual value, highlighted when it differs from the template default.
   - An "unresolved flags" note for admins.
```

## Follow-ups

- "Put all three searches behind one Search page with tabs, and make '/' focus the query field."
- "Show a no-results state with suggestions (switch to regex, widen swimlanes)."

## Done when

- [ ] All four areas are designed, including the truncated, invalid-regex, no-results and read-only states.
- [ ] Every result links to the exact file and line, or the exact doc section.
