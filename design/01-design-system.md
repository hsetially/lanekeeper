# 01: Design system

Build this first, and publish it as the organisation's Lanekeeper design system. Every screen prompt after this one says "use the Lanekeeper design system".

## Prompt

```
Create the Lanekeeper design system. Show it as a living style guide page with every token and component, in light and dark.

FOUNDATIONS
- Fonts (self-hosted, open licence): Inter for UI, JetBrains Mono for paths, setting keys, values, hashes and code. Use tabular numbers in tables.
- Type scale for density: 12 / 13 (default body) / 14 / 16 / 20 / 24. Line heights tuned for 13px body.
- Spacing on a 4px grid. Row heights: 28px compact and 32px default, both fixed so lists can be virtualised.
- Radius: small (4px) for controls, medium (6px) for cards. Minimal shadows; use borders for separation.
- Neutral palette plus one brand accent (calm blue). Define semantic tokens (background, surface, border, text, muted, accent, focus ring, danger, warning, success, info) for light and dark, all meeting WCAG 2.2 AA contrast.

DRIFT-STATE PALETTE (every one is colour + icon + label; colour-blind safe)
- In sync: neutral or green, check icon.
- Git ahead: blue, arrow-down-into icon ("new in Git").
- NFS ahead: amber, arrow-up-out icon ("changed on NFS").
- Conflict: red, split icon.
- Intentional: violet, pin icon.
- Unknown: grey, question icon ("no baseline yet").
- Untracked: slate, dashed-circle icon.
Also: Pending restart (orange, clock icon), Locked (grey, lock icon), Pending approval (violet, hourglass icon), Agent online, degraded and offline (dot and label).

COMPONENTS (each with default, hover, focus, active, disabled and loading states)
- Buttons: primary, secondary, ghost, destructive; icon buttons with tooltips and keyboard hints.
- Inputs: text, search with keyboard hint, select, combobox, multi-select chips, toggle, checkbox, radio, segmented control.
- Status badge (drift states and the others above), role badge (Viewer, Editor, Operator, Admin), tier badge (SIT, PRESIT).
- Monospace chips: file path (truncates in the middle, full path on hover, copy button), setting path, short hash (7 chars, copy), branch chip, version label ("v2.3.1 + 4").
- Data table: dense, sticky header, sortable columns, row selection, inline row actions on hover, a "load more" cursor footer, empty and loading-skeleton states.
- Tree view: folders and files, file-class icons (structured, text, binary), drift badge per row, keyboard navigation.
- Tabs, breadcrumbs, split pane with a resizable divider.
- Diff chrome: header with left and right refs, change counts, "line endings only differ" flag, previous/next change buttons, view toggle (side by side or unified). The diff body uses monospace with added, removed and modified line colours that work in both themes.
- Command palette (Cmd+K): grouped results (swimlanes, files, services, docs, users, actions) with keyboard hints.
- Dialogs: a standard one, a "consequence" confirmation (shows effects before the confirm button), and a destructive confirmation that needs typing the name.
- Toast notifications, inline alerts (info, warning, error), banners (page-level, such as "Editing locked").
- Live indicator: connected, reconnecting, or offline.
- Empty state, error state (with a problem title and a request id), permission state ("You need Editor access").
- Keyboard shortcut hint (kbd) and a shortcuts help sheet.

Use names that map one-to-one to shadcn/ui components and Tailwind tokens, and list that mapping on the style guide page.
```

## Follow-ups

- "Show every drift-state badge in a table row, a tree row and the diff header, in both themes. Check that each still reads correctly in greyscale."
- "Tighten the density. Make the default table show 25 rows at 1440×900 with a header and toolbar visible."
- "Add a component-to-shadcn mapping table and a token list I can hand to engineers."

## Done when

- [ ] Every token has light and dark values that pass AA contrast. The focus ring is visible on every component.
- [ ] Every status is shown as colour plus icon plus label, and is distinguishable in greyscale.
- [ ] Every component shows all its states, including loading and disabled.
- [ ] The component list maps to shadcn/ui names. Fonts are self-hosted, with no external assets.
- [ ] The design system is published as the organisation default for this project.
