# 04: Swimlanes overview

## Prompt

```
Using the Lanekeeper design system, design the Swimlanes overview, which is the home page.

TABLE (one row per swimlane; 6 sample rows, then show 40 rows to test density)
Columns:
- Swimlane (sitb) with tier badge.
- Project.
- Deployed branch (sit7).
- Base version ("v2.3.1 + 4").
- Agent status.
- Drift summary: compact counts by state, for example "3 NFS ahead · 1 conflict · 12 Git ahead".
- Pending restarts (count).
- Findings (count).
- Baseline status: Confirmed, or "Adoption needed" (with a button for Admins).
- Last change (relative time).

Toolbar: filter by tier, by agent status and by "has drift"; text filter; column chooser; "Compare selected" when two rows are selected.

LIVE BEHAVIOUR
Show a row that just changed (a brief highlight) and a toast "sitb: tx-infinity-core-sit1.yml changed on NFS (unknown author)".

STATES
- Loading skeleton.
- Empty ("No swimlanes registered yet", with an Admin action "Add a swimlane").
- One agent offline (row shows offline, with last seen time).
- A swimlane with "Adoption needed".
```

## Follow-ups

- "Add a compact header strip with totals: swimlanes, agents online, files in conflict, pending approvals."
- "Show how the table looks with 40 swimlanes at 1440×900 without horizontal scrolling."

## Done when

- [ ] 40 rows fit cleanly, with no horizontal scroll at 1440 wide.
- [ ] Live-change, offline, adoption-needed, loading and empty states are all shown.
