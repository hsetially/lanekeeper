# 02: App shell and navigation

## Prompt

```
Using the Lanekeeper design system, design the app shell at 1440×900, in light and dark.

LAYOUT
- Left sidebar (collapsible to icons):
  - Swimlanes
  - Compare
  - Search (text, settings, docs)
  - Feature status
  - Findings (count badge)
  - Approvals (Admin; count badge)
  - Audit
  - Admin (Admin only)
  - At the bottom: user menu with name, role badge, "My GitHub token" and sign out.
- Top bar:
  - breadcrumbs (for example Swimlanes / sitb / tx-infinity-api / tx-infinity-core-sit1.yml);
  - a swimlane switcher (combobox showing tier badge, deployed branch and drift count);
  - a Cmd+K search trigger;
  - the live indicator (connected);
  - a notifications bell.
- Main area with a page header (title, key facts, primary actions) and content.

COMMAND PALETTE
Show it open with the query "core": grouped results for files (tx-infinity-core.yml in sitb, sitc), services (tx-infinity-core), docs (Account sorting), and actions ("Compare sitb with sitc", "Search text…"). Each has a keyboard hint.

SHORTCUTS SHEET
Show the "?" overlay listing global shortcuts: Cmd+K, g s (swimlanes), g c (compare), / (search), and diff navigation (n / p).

STATES
- Live indicator reconnecting, and offline (with a banner "Live updates paused, retrying").
- Sidebar items hidden for Viewers versus disabled with a tooltip for actions (show both patterns and recommend one).
```

## Follow-ups

- "Show the shell at 1280 and 2560 wide. Keep content readable on wide screens with a max width where it helps."
- "Make the swimlane switcher show agent status and 'editing locked' for swimlanes without a confirmed baseline."

## Done when

- [ ] Light and dark variants are designed, at 1280, 1440 and 2560 wide.
- [ ] The palette, shortcuts sheet and live-indicator states are all shown.
- [ ] Role-based navigation behaviour is decided and shown.
