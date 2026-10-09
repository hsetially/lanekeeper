# Design track: Claude Design first, then code

Build the whole UI in Claude Design before agents write production frontend code. Claude Design is the source of truth for **how the app looks and behaves**. The OpenAPI spec remains the source of truth for **data**, and `docs/security.md` for **constraints**. Where a design conflicts with security, security wins.

## How to run it

1. **Brief.** In Claude Design, create a project called "Lanekeeper" and paste `00-project-brief.md`. Keep every screen in this one project, so screens share context and can link into clickable prototypes.
2. **Design system.** Paste `01-design-system.md`. Iterate until the "Done when" list is met, then publish it as the organisation's Lanekeeper design system. Every later screen uses it.
3. **Screens.** Paste prompts `02` to `12` in order. Each file contains:
   - the prompt to paste, in a fenced block;
   - follow-up prompts for iterating;
   - a "Done when" checklist to review against.

   Don't move on until the checklist passes.
4. **Sweep.** Paste `13-states-accessibility-prototype.md` to check every state, accessibility, dark mode, and the key user journeys as clickable flows.
5. **Handoff.** Follow `14-handoff.md`. Claude Design packages the design, the components it uses and the design intent into a handoff bundle for Claude Code. The coding agent then implements prompt 08 against that bundle, with visual regression tests.
6. **Keep the two in sync.** Once prompt 08 has built the real components, run `/design-sync` from Claude Code. That brings the coded design system back into Claude Design, so later iterations use the real components.

## Rules for every design prompt

- **Synthetic data only.** Use the sample names in the brief. Never paste real config contents, hostnames or the six files flagged by the secret scan (Q20) into Claude Design.
- **Built to be built.** Every component maps to shadcn/ui and Tailwind. Fonts are self-hosted (the CSP blocks third-party origins). No third-party embeds.
- **Every screen has a light and a dark variant,** plus its loading, empty, error and permission states.
- **Desktop first.** Designed at 1440×900. Must work from 1280 to 2560 wide. Usable at a minimum of 1024 wide. No mobile layout.

## Files

| File | What it designs |
|---|---|
| 00-project-brief.md | Product context, users, sample data and principles |
| 01-design-system.md | Tokens, typography, drift-state palette and component library |
| 02-app-shell.md | Navigation, swimlane switcher, Cmd+K, live indicator |
| 03-access-onboarding.md | Sign-in, access requested, disabled, GitHub token onboarding |
| 04-swimlanes-overview.md | All swimlanes at a glance |
| 05-swimlane-detail.md | File tree, services, pending restarts, agent status |
| 06-file-view-editing.md | NFS, Git and baseline tabs, diff, editor, history, binary files |
| 07-change-dialogs.md | PR dialog, upload and delete previews, restart, conflict, approval |
| 08-compare.md | Cross-swimlane grid and whole-swimlane compare |
| 09-search-docs-features.md | Text search, settings search, docs, feature status |
| 10-findings-adoption.md | Findings, and the adoption wizard |
| 11-approvals-audit.md | Approvals queue and audit log |
| 12-admin.md | Users, access requests, mappings, rules, agents, tokens |
| 13-states-accessibility-prototype.md | State sweep, accessibility, dark mode, clickable journeys |
| 14-handoff.md | Exporting to the coding agents, and what they must do with it |
