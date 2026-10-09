# 05: Swimlane detail

## Prompt

```
Using the Lanekeeper design system, design the swimlane detail page for sitb (tier SIT, branch sit7, base v2.3.1 + 4, agent online, baseline confirmed).

HEADER
Swimlane name, tier, branch chip, version label, agent status, "Compare with…" and "Search in sitb" actions.

LEFT: FILE TREE (resizable split pane)
- Folders tx-infinity-api, ui/remote-itm-teller, limits, security, document-service/resources, ej-templates, plus root files application.properties and channels.yml.
- Show base and tenant files side by side (tx-infinity-core.yml and tx-infinity-core-sit7.yml), with a toggle "Show logical files" that merges them into one row with an "uses tenant file" marker.
- Each row: file-class icon, name, drift badge, pending-restart marker for files that affect running services.
- Filter chips: drift state, file class; a text filter.

RIGHT: TABS
1. Services: table of services (tx-infinity-core, ui-service…) with pods ready, started time, "Pending restart since 14:02 (tx-infinity-core-sit7.yml changed)", and a Restart action (Operator+). Include a row with "affected by application.properties (all services)".
2. Drift: list of files not in sync, grouped by state, each with a one-line reason and links.
3. Findings: findings for this swimlane (C2 redundant tenant file account-sorting-config-sit7.yml, C8 duplicate keys in security-roles.yml), each linking to the exact lines.
4. Activity: live feed of recent changes and audit events for this swimlane.

STATES
- Editing locked banner when the baseline isn't confirmed (with "Start adoption" for Admins).
- Agent offline banner with last seen time; data shown as "as of 14:05".
- Viewer: restart actions disabled with a tooltip.
```

## Follow-ups

- "Make the tree keyboard navigable and show the focused row clearly."
- "Show the tree with 2,000 files and a filter applied, and make sure performance-critical rows are fixed height."

## Done when

- [ ] The logical-file toggle is shown in both modes.
- [ ] The pending-restart cause is visible, including the root-file case.
- [ ] The locked, offline and Viewer states are all designed.

## Additional prompt (attribution, severity, sync windows)

```
Update the swimlane detail page:
- Activity feed and drift list: every change shows an attribution chip:
  - "Marco Diaz · via Lanekeeper" (certain);
  - "lkowalski · edited on NFS VM via sudo vi" (high);
  - "Sync job csp-tenant-data-sit7 · run 42" (high);
  - "NFS client (no sync running)" (medium);
  - "Unknown".
  Make confidence readable without colour.
- Severity badges (critical, high, medium, low) on changes and findings, sortable.
- A "Sync in progress" banner while the sync job runs: "Alerts held until sync completes; 412 files updated so far."
- The services tab: a sentinel status line ("NFS VM sentinel: reporting, last record 14:05").
```

## Additional prompt (tenants, pickup, config-server)

```
Update the swimlane detail page:
- The header shows the tenant set (for example "Tenants: sit7, sit9") instead of a single branch.
- The services tab replaces "pending restart" with pickup chips:
  - "Live";
  - "Live by 14:32";
  - "Needs notify or restart";
  - "Needs config-server restart".
  Bulk actions: "Notify services" and "Restart config-server".
- Findings include C9 "Ambiguous file name: authentication-config.yaml exists in 12 folders", C10 "Unresolved placeholder" and C11 "Config-server restart required".
```
