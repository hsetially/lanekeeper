# 08: Web app

| | |
|---|---|
| **Wave** | 1, built against MSW mocks. Integrate as 03a, 05, 06 and 14 land. |
| **Depends on** | 01, plus the design handoff in `design/handoff/` (see `design/14-handoff.md`). T1–T3 can start before the handoff lands. |
| **You own** | `web/**`, `design/DEVIATIONS.md`. `design/handoff/**` is read-only. |
| **Interfaces** | `api/openapi.yaml` (client generated with orval), and SSE at `/api/v1/events` |
| **Security** | S2, S3, S12, S21 (web lint equivalents) |
| **Performance** | P10, P13 |
| **Gate** | `just verify-08` |

## Objective

A fast, keyboard-first operations console. It must:

- stay inside a strict CSP;
- update live over SSE;
- remain responsive on 6,000-line diffs and 5,000-row grids;
- contain no AI chat.

## Tasks

### T0: Import the design handoff

**Read** the reference screens (`design/handoff/screens/*.dc.html`, using the `lanekeeper-ui-from-design` skill) and the "Engineering handoff" page:

- tokens;
- the component-to-shadcn map;
- the screen inventory and routes;
- the keyboard maps;
- the States index.

**Map:**

- tokens to `web/src/styles/tokens.css` and the Tailwind theme, in light and dark;
- components to shadcn/ui variants.

**Fonts and icons** are self-hosted.

**Never paste exported HTML, inline scripts or inline styles into the app.** Re-implement them, so the code stays compliant with the CSP and Trusted Types (S12). If a design can't be built without breaking S12 or a P10 budget, record the deviation in `design/DEVIATIONS.md` and escalate.

**Verify:** a token parity test, comparing the generated CSS variables with the handoff token list.

### T1: Foundation and security (S12)

**Stack:** Vite, React and TypeScript in strict mode, TanStack Router, Query and Virtual, shadcn/ui and Tailwind.

**API client:** generated with orval, together with MSW mocks.

**CSP compliance:**

- no inline scripts;
- Trusted Types policies;
- a Monaco Trusted Types policy;
- Monaco and fonts self-hosted;
- no third-party origins.

**Rendering:** `dangerouslySetInnerHTML` is banned by a lint rule. The only exception is a component that renders server-sanitised docs HTML.

**Requests:** the CSRF header is sent on every non-GET.

**Verify:**

- the lint ban works;
- a CSP violation test in Playwright, which runs the app under the production CSP and fails on any violation report.

### T2: Performance (P10)

**Loading:**

- route-level code splitting;
- Monaco lazy-loaded only on file and diff views.

**Bundle budget:** the initial JavaScript is under 250 KB gzipped, enforced in CI.

**Data:**

- TanStack Query cache keys include content hashes, so cached data is safe to reuse.
- Requests use ETags.

**Rendering:**

- diffs are rendered from server hunks, virtualised;
- grids and trees are virtualised.

**Verify:**

- Lighthouse CI scores performance at 90 or above;
- a Playwright performance test: the 6,000-line diff is interactive within 500 ms, and a 5,000-row grid scroll holds 60 fps;
- the bundle-size gate.

### T3: Live updates (P13)

- One SSE connection per tab, with backoff reconnect and `Last-Event-ID`.
- Events invalidate the precise query keys they affect.
- `Resync` triggers a refetch of the visible views.

**Verify:** a test with a mocked event stream.

### T4: Screens

**Access:**

- an "access requested" page for pending users;
- onboarding for the fine-grained GitHub token, for Editors and above.

**Swimlanes:**

- overview: tier, branch, base version label, agent status, drift counts, pending restarts and baseline status, all updating live;
- detail: a file tree with drift badges and file-class icons, plus services with restart actions.

**File view:**

- tabs for NFS, Git and baseline;
- diff between any two;
- line-ending-only differences flagged, and a line-ending indicator;
- binary files shown as side-by-side image previews;
- editing in Monaco, with the line-ending setting matching the file, sending the expected hash and an `Idempotency-Key`;
- editing locked, with the reason, until the baseline is confirmed;
- history and revert;
- upload and delete with an effect preview;
- raise a PR, with the explicit base-or-tenant choice and the repo paths shown;
- "docs for this file".

**Compare:**

- a grid for one logical file across swimlanes, with a channel filter and differing rows only by default;
- **whole-swimlane compare** (D67): summary counts, then the differing logical files, then the per-file diff.

**Search:**

- text search: literal or regex, a path filter, results by swimlane and file, jump to line;
- settings search;
- docs search, browse and upload or replace (Editors and above);
- feature status.

**Findings, adoption and approvals:**

- findings, each jumping to its exact lines;
- the adoption wizard;
- approvals (admin), updating live.

**Admin:**

- users and access requests;
- path mappings, merge modes, host tiers and folder mappings;
- notifications;
- agents;
- personal MCP tokens, when that mode is active.

**Audit log.**

**Navigation:**

- a Cmd+K palette for every entity;
- keyboard shortcuts in diffs;
- a URL for every view.

**Responses:**

- 409: show what changed, and let the user re-apply;
- 202: "sent for approval", with a link;
- 423: explain the editing lock.

**Verify:**

- component tests for the PR dialog, the 409 flow, effect previews, onboarding, access approval, adoption and CRLF saving;
- **visual regression:** Playwright screenshot tests at 1440×900, in light and dark, for every screen and state in the design's States index. Compare against the reference images exported from Claude Design, within an agreed small pixel threshold;
- the keyboard maps from the handoff, run as Playwright keyboard-only tests;
- a Playwright end-to-end smoke test against the mocks;
- axe checks pass.

### T5: Attribution, severity, PR links, masking and sentinels

- **Attribution:** an attribution chip (source, confidence, actor) on history entries, the activity feed, audit rows and file headers.
- **Severity:** severity badges, with sorting and filtering by severity.
- **PRs:**
  - "PR open" and "fix merged, awaiting sync" badges on drifted files;
  - duplicate-PR feedback;
  - a PR job progress view, updated over SSE.
- **Masking:** flagged values are masked in the editor and diff, with an audited "Reveal" action for Editors and above. Denied files show "content withheld".
- **Admin:**
  - sentinel status (VM, last record, version);
  - the OS Login user mapping;
  - severity rules;
  - retention settings with a dry-run preview.

**Verify:**

- component and visual tests for each item;
- a test that masked values never reach the DOM before a reveal (assert on the DOM).

### T6: Pickup states, notify and "As served"

- **Pickup chips replace "pending restart":**
  - "Live";
  - "Live by 14:32 (client cache)";
  - "Needs notify or restart";
  - "Needs config-server restart".
- **Actions:**
  - "Notify services" after a write, plus per-swimlane auto-notify in admin settings;
  - "Restart config-server" when check C11 is open.
- **File view:** an "As served" tab. Pick an application, tenant and channel, then compare the config-server's response with the rendered view and with NFS.

**Verify:** component and visual tests for each item.

## Acceptance (all required)

- [ ] `just verify-08` passes, and every P10 gate (bundle size, Lighthouse, the diff and grid performance tests) passes.
- [ ] The CSP violation test passes.
- [ ] No API type is written by hand.
- [ ] Visual regression and keyboard-map tests pass against the design handoff. Every deviation is listed in `design/DEVIATIONS.md` and approved by the design owner.

## Stop and escalate if

- Monaco can't run under Trusted Types and the CSP without unsafe exceptions.
- The bundle budget can't be met with Monaco lazy-loaded.
- A design relies on inline scripts, external fonts or third-party assets, or a layout can't be virtualised.

## Out of scope

Server logic.
