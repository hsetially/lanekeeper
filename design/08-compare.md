# 08: Compare

## Prompt

```
Using the Lanekeeper design system, design the Compare section with two modes.

MODE 1: GRID (one logical file across swimlanes)
- Pick a logical file (tx-infinity-api/tx-infinity-core.yml) and swimlanes (sit, sitb, sitc, presita, presitb, presitc).
- Rows: setting paths (monospace). Columns: swimlanes, each column header showing which file is effective there (tenant file sit7, or base).
- Cells: values, with differing cells highlighted and "absent" shown distinctly.
- Toggles: "Only differing rows" (on by default), channel filter (remote-itm-teller, atm-iso, atm), a search box within settings.
- Clicking two cells opens a side-by-side diff drawer for those two files at that setting's line range.
- Show 5,000 rows virtualised, with a sticky first column and sticky headers.

MODE 2: WHOLE-SWIMLANE COMPARE (sitb vs sitc, or sitb vs Git branch sit7, or vs tag v2.3.1)
- Summary: identical directories (skipped), differing logical files, only-in-left, only-in-right.
- A tree of differing directories and files, with counts.
- Selecting a file opens its diff on the right.
- Show the "identical" state as a big calm success message.

STATES
Loading, too many swimlanes selected (limit 40), a swimlane whose agent is offline (data "as of" time).
```

## Follow-ups

- "Add an 'export this comparison as a link' action. Every compare state must be a shareable URL."
- "Make the grid keyboard navigable: arrow keys between cells, Enter to open the diff."

## Done when

- [ ] Both modes are designed, with realistic data. The 5,000-row grid stays readable.
- [ ] The effective-file indicator appears in each column header. "Absent" is distinct from "empty".
