# 13: States sweep, accessibility, dark mode and prototypes

## Prompt 1: States sweep

```
Review every screen in this project and add any missing states, in light and dark:
- loading (skeletons matching the final layout);
- empty;
- error (problem title, a one-line explanation, and a request id with copy);
- permission (pending user, Viewer, Editor without Operator);
- offline agent ("data as of" time);
- live-updates paused;
- truncated results;
- 409 conflict, 423 locked and 202 sent for approval;
- expired GitHub token.

Produce a "States index" page linking each state to its screen.
```

## Prompt 2: Accessibility pass

```
Audit every screen for WCAG 2.2 AA:
- contrast of text and status colours in both themes;
- visible focus on every interactive element;
- a logical Tab order;
- keyboard-only operation (tree, grid cells, diff navigation, dialogs);
- labels for icon buttons;
- status never conveyed by colour alone;
- target sizes of at least 24×24px.

List the issues found, fix them, and add an "Accessibility notes" page describing keyboard maps per screen.
```

## Prompt 3: Clickable journeys

```
Link screens into clickable prototypes for these journeys:
1. Out-of-band change: live toast → swimlane detail → file shows "NFS ahead" → diff vs Git → Raise PR to tenant sit7 → PR created.
2. Cross-swimlane check: Compare → grid for tx-infinity-core.yml → only differing rows → open the diff of sitb vs presita.
3. Safe edit with approval: Ana Ruiz edits → "Submit for approval" → Priya approves in the queue → audit entry.
4. Conflict: edit → 409 → re-apply on top → saved → pending restart → restart.
5. New user: Sam signs in → access requested → Priya approves as Editor → Sam completes GitHub token onboarding.
6. Adoption: Priya runs the wizard for presitb → editing unlocked.
7. Feature question: Feature status → enableAccountSorting differs in presita → open doc → open the file at the line.
```

## Done when

- [ ] The States index covers every screen. Every accessibility issue is fixed, and keyboard maps are documented.
- [ ] All 7 journeys are clickable end to end, in light and dark.
