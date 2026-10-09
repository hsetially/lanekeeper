# 14: Handoff to the coding agents

## Before you export

- **Finish the reviews.** Every "Done when" list in design prompts 01–13 must be complete.
- **Pass the design to engineering:**
  1. Produce a final "Engineering handoff" page in Claude Design, using the prompt below.
  2. Export.
- **Ask in Claude Design:**

```
Create an "Engineering handoff" page:
- the token list (light and dark), with names matching Tailwind and CSS variables;
- the component inventory mapped to shadcn/ui, listing every variant and state;
- the screen inventory with routes (for example /swimlanes/:id/files?path=…&diff=…);
- the keyboard map per screen;
- interaction notes (live updates, toasts, optimistic UI is NOT used for writes);
- fixed row heights for virtualised lists.
Flag anything that needs inline scripts, external fonts or third-party assets; those are not allowed by our CSP and must be redesigned.
```

## Export

- **For Claude Code (recommended for prompt 08):** Export, then send to Claude Code, with the local agent in the monorepo. Save the bundle under `design/handoff/` in the repo. That path is read-only for agents.
- **For Cursor agents:** export as a `.zip` or as standalone HTML into `design/exports/`, and point the agent at it.
- Commit the handoff in its own PR, titled `design: handoff vN`.

## Instructions the coding agent follows

Prompt 08 has these as task T0:

1. **Read the handoff bundle and the Engineering handoff page.**
2. **Tokens.**
   - Map the tokens into `web/src/styles/tokens.css` and the Tailwind config.
   - Map the components onto shadcn/ui.
3. **No exported code in the app.** Never paste exported HTML, inline scripts or styles into the app. Re-implement it, to satisfy the CSP and Trusted Types (S12).
4. **Self-hosted assets.** Fonts and icons are self-hosted, with nothing loaded from external origins.
5. **Visual regression.** Playwright screenshot tests at 1440×900 in light and dark, for every screen and state in the States index, compared against reference images from the design. Differences must stay within a small threshold.
6. **Deviations.** Record every intentional deviation in `design/DEVIATIONS.md`, with the reason (performance, security, accessibility). The design owner approves it.

## After the build

Run `/design-sync` from Claude Code so Claude Design picks up the real coded components. From then on, design changes start from the real components, and each new handoff bumps the version (`design: handoff v2`).
