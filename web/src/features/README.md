# Rules for the slice authors of T4 (screens)

Eight slices (S1 to S8) build the screens in parallel, after the shell (S0) is done. This page is the contract between them.
The shell made every file you need; **you replace your own and touch nothing else.** The implementer runs install, code
generation, the build and every Playwright run, resolves anything shared, and makes the one `08/T4` commit. You do not commit.

## 1. What you own

| Slice | Pages you replace (stubs the shell made) | Other paths you own | Design scenarios (`src/mocks/design/<folder>/index.ts`) |
|---|---|---|---|
| S1 access | `features/access/{SignInPage, PendingPage, DisabledPage, OnboardingPage, GithubTokenPage, McpTokensPage}.tsx` | `features/access/**` | `access` |
| S2 swimlanes | `features/swimlanes/{SwimlanesPage, SwimlaneDetailPage, ServicesPage, DriftPage, ActivityPage, SwimlaneFindingsPage, SwimlaneAuditPage}.tsx` | `features/swimlanes/**` | `swimlanes` |
| S3 file view | `features/file/{FilePage, DraftPage}.tsx` | `features/file/{view,diff,edit,history,dialogs,pr,docs-panel}/**` | `file` |
| S4 compare | `features/compare/ComparePage.tsx` | `features/compare/**` | `compare` |
| S5 search, docs, status | `features/search/SearchPage.tsx`, `features/docs/{DocPage, DocViewPage}.tsx`, `features/status/StatusPage.tsx` | `features/{search,docs,status}/**` | `search` (docs and status too) |
| S6a findings | `features/findings/FindingsPage.tsx` | `features/findings/**` | `findings` |
| S6b adoption | `features/adoption/AdoptPage.tsx` | `features/adoption/**` | `adoption` |
| S7 approvals, audit | `features/approvals/{ApprovalsPage, ApprovalPage}.tsx`, `features/audit/AuditPage.tsx` | `features/{approvals,audit}/**` | `approvals` (audit too) |
| S8 admin | `features/admin/{UsersPage, SwimlanesAdminPage, SectionPage}.tsx` | `features/admin/**` | `admin` |
| S0 shell (done) | `features/shell/{NotFoundPage, ErrorPage, ForbiddenPage}.tsx` | `features/shell/**`, `src/keyboard/**`, `src/routes/**`, `src/components/app/{EmptyState, ErrorState, PermissionState, PageSkeleton, PersonalTokens, drift}.*` | `shell` |

Inside your own paths, add files freely (components, hooks, tests, fixtures). Each page is the **default export** of its file and
takes `PageProps<"<route id>">` from `@/routes/props`:

```tsx
import type { PageProps } from "@/routes/props";

export default function FilesPage({ params, search }: PageProps<"files">) {
  // params.id is the swimlane id, already checked against the route table. search.path, search.line, ... are strings or undefined.
}
```

The router passes `{ params, search }`; a page never reads the URL by itself (`useSearch`, `useParams` and `window.location` are
not for pages) and never builds a path by hand: use `linkTo("files", { id }, { path })` from `@/routes/links` for a string, or
`<AppLink route="files" params={{ id }} search={{ path }}>` from `@/routes/AppLink` for a link. A wrong key does not compile.
`src/routes/pages.ts` (shared, not yours) names each page file and checks its props against its route.

**A page renders no `<main>`** (the shell provides the one landmark) and **exactly one `<h1>`**, which is where focus goes
after navigation (`src/features/shell/focus.ts`): render it as soon as the page renders, not after its data arrives, so that a
screen reader hears where the person is. A page that has to show a state (empty, error, no permission) uses the shared kits in
section 3.

## 2. What you must not touch

These are shared and belong to the implementer; if you need a change, say so in your report (section 8) and do not make it:

`package.json`, `pnpm-lock.yaml`, `vite.config.ts` (and the Vitest projects), `tsconfig*.json`, `eslint.config.js`,
`playwright.config.ts`, `orval.config.ts`, `components.json`, `src/router.tsx`, `src/routes/**` (including `pages.ts`,
`table.ts`, `overrides.ts`), `src/keyboard/**`, `src/lib/query/keys.ts`, `src/api/generated/**`, `src/styles/{tokens,tailwind}.css`,
`src/mocks/design/index.ts`, `src/mocks/scenarios.ts`, the Justfile, `design/DEVIATIONS.md`, `perf/budgets.toml`,
`crates/xtask/tests/budgets_registry.rs`, and everything under `src/components/ui/` and `src/components/app/` (the shared
components; a missing option on one is a request, not an edit). Another slice's folder is not yours either.

You never run `pnpm install`, `pnpm build`, `pnpm gen:api` or Playwright, and you add no dependency.

## 3. What the shell gives you

| Need | Use |
|---|---|
| A link | `AppLink`, `linkTo`, `hrefFor` (`@/routes/AppLink`, `@/routes/links`). A server-given path must pass the route table; a value it refuses makes no link. |
| Empty, error, permission, loading | `EmptyState`, `ErrorState`, `PermissionState`, `PageSkeleton` (`@/components/app/...`). Query states: `QueryBoundary` (below). |
| A toast | `toast({ kind, title, action })` from `@/features/shell/toast/store`. Only for what the person did NOT do themselves; the result of their own write is shown in the dialog that made it (H-3). |
| A key | `useShortcut(rowId, handler)` from `@/keyboard`, section 5. |
| Roles | `GatedButton` and `useWriteGate(minRole)` (disabled with a tooltip naming the role, never hidden). The server enforces roles whatever the page shows. |
| Tier chip, drift badge | `TierChip` (`@/components/app/TierBadge`), `DRIFT_VARIANT` and `driftSummary` (`@/components/app/drift`), `StatusBadge`. |
| Tenants | `TenantSet` (`@/features/shell/Tenants`): the first tenant and a `+N` chip with the whole set in a tooltip. A swimlane has a SET of tenants; `tenants` may be empty for a swimlane that is not adopted, and the page must cope. |
| The personal MCP tokens | `PersonalTokens` (`@/components/app/PersonalTokens`). S1 and S8 only mount it. |
| Copying | `CopyButton` (a request ID, a path, a hash: `what` names it, `source` is the text to select when the clipboard refuses), `CopyValue` (a value that shows its button on hover and focus: for a drawer, a detail line or a header, never in a DataTable or tree row), `CopyLink` (a permalink made from `linkTo`, never by hand), `RequestId` (with `details` from `supportDetails` for an error that had a response). A one-time secret has `SecretCopyButton`, and only `PersonalTokens` and `JoinTokenCard` may import it (lint). Nothing else is copied: no GitHub token a person typed, no revealed value, no masked value. |
| Untrusted text (paths, names, lines, values) | `SafeText` (`@/components/app/SafeText`). Never `dangerouslySetInnerHTML` (the lint refuses it). |
| Times | `formatTime`, `formatClock`, `formatRelative` (`@/lib/time`): UTC, spelled out. |
| Icons | `@/icons` only, which are the handoff's 68. Another icon is a request. |
| A choice that must be remembered | `@/lib/prefs` only (layout choices, no content). Nothing else touches web storage. |

## 4. Server data

Every query and mutation already has a colocated module, `features/<domain>/queries.ts` and `mutations.ts` (T3), built from the
generated operations and the key factories of `@/lib/query/keys`. **Import them; do not write another.** The rules the lint and
the tests enforce:

- no literal query key, no generated `useXxx` hook, no `useQuery` with a key you invented;
- paged lists go through `useCursorPages({ ...someQuery(), ... })`; keep the cursor out of the key; ask for 50 rows first on a
  large grid;
- show a query with `QueryBoundary` (loading skeleton, error with the problem's title and request id, empty state, and the
  refreshing and stale overlays) or with `useQueryState` when the screen combines several;
- a write is a `writeMutation` from `mutations.ts` or a secret-bearing one through `useSecretMutation`: never optimistic (the one
  exception is `adminPutAutoNotify`), never `setQueryData`, never a retry; show the server's answer in the dialog that made it,
  including 409, 423 and `proposal_created`;
- a secret (a token, a revealed value) lives in component state only and is cleared on every outcome and on unmount;
- the web never masks. The server sends `masks`; absent `masks` means none supplied (M1).

```tsx
const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
return (
  <QueryBoundary query={cursorPagesSource(pages)} noun="swimlanes" isEmpty={(items) => items.length === 0} empty={<EmptyState title="No swimlanes registered yet" />}>
    {(swimlanes) => <SwimlaneTable rows={swimlanes} />}
  </QueryBoundary>
);
```

(`useQueryState(cursorPagesSource(pages))` and `QueryBoundary` take a cursor list as a source; check `QueryBoundary.tsx` for the
exact props.) The sidebar's Findings count and the header's swimlane switcher use `swimlaneListQuery()`, so a list screen that
uses it shares one cache entry with them and with the live events.

## 5. Keys

`useShortcut(rowId, handler, options?)` binds a handler to a ROW of the keymap, not to a key. The rows are the handoff's
(`src/handoff/keymap.generated.ts`) plus the reviewed screen-local ones (`src/keyboard/rows.ts`); `src/keyboard/bindings.ts`
says who executes each row. A key that is in neither list does not exist: **ask for a row to be added, do not invent a key.**

- The handler gets `{ id, binding, action, event }` (`binding` is which of the row's keys fired) and may return `false` to decline.
- A screen's row beats the shell's (`/` on the Search screen focuses the query; elsewhere the shell sends you to Search).
- A single key and a sequence do not fire while a field or the editor has focus or a dialog is open; a chord (Control or
  Command plus a key) fires in a field and in the editor. `options.allow` widens it, `options.enabled` binds conditionally.
- The registry is mounted by the shell, so `useShortcut` binds nothing on a screen before sign-in, and nothing in a component
  test without a `KeyboardProvider` (wrap with `<KeyboardProvider>` from `@/keyboard` to test a binding).
- Rows owned by `component` (lists, tree, diff, dialogs, palette, split pane) are already handled by those components; bind
  them only for what they say a screen binds (for example `global.close-dialog-menu-or-palette` to cancel an inline edit).
- Every shortcut needs a pointer path too (a button, a menu item).

## 6. Mocks and scenarios

Your design-parity data lives in `src/mocks/design/<folder>/index.ts` (the folder in the table above), which exports
`scenarios: readonly Scenario[]`. The loader (`src/mocks/design/index.ts`) already lists every folder, so you never edit it.

- A scenario id is `design-<folder>-<name>`; a collision or a wrong prefix fails `src/mocks/design/design.test.ts`.
- A scenario is DATA (`src/mocks/scenario.ts`): for each operation, the status and the body. `src/mocks/mocks.test.ts` checks
  every response against the OpenAPI schema (patterns and lengths included), so a mock cannot give an answer the server could
  not. Give a function body a `sample` (and `samples` for the pages of a list).
- It reproduces the content of the handoff's page (lane names, counts, hashes, "4 min ago"), derives relative times from the
  frozen clock the visual runs use, and is what the Playwright harness serves when a test calls `harness.setScenario(id)`.
- Reuse `src/mocks/design/shell` (`SHELL_LANES`) for the six swimlanes of the design instead of making another copy. They carry the
  Shell page's data (every drift count is `nfs_ahead`: 4, 4, none, 3, none, 1, which is what the Shell's switcher draws); the
  Swimlanes page draws a different mix for the same lanes, so S2 spreads `SHELL_LANES` and sets its own `drift_counts`.
- Loaded by Node as well as Vite: relative imports with the `.ts` extension, `import type`, nothing that cannot be erased.

## 7. Tests you run

Test first. Run only your own folder:

```
pnpm exec vitest run src/features/<your folder>
pnpm exec eslint src/features/<your folder>
```

jsdom has no layout: component tests use the stubs in `src/test/setup.ts`. `src/test/app.tsx` has `renderApp(url, api, options)`
for a test that needs the real router, session and shell on a memory history with `fetch` answered by a function. Monaco does
not run in jsdom; use a fake editor surface. Playwright specs (visual, keyboard, journeys, axe) are written by the implementer.

## 8. What to report back

Your return is short: the files you created or replaced, the routes and keymap rows you covered, the scenario ids you added,
and a **Deviations** list for the implementer to put in `design/DEVIATIONS.md`, one line each in the form
`<screen or component> | <what differs from the design> | <reason: security, performance, accessibility; or contract or handoff gap>`.
Also list anything you needed from a shared file, and any contract gap you hit (do not work around one: say what you needed).

## Facts found while building the shell

- The contract's `AgentStatus` is `connected`, `disconnected`, `never_seen`; the design's dots are online, degraded, offline.
  The mapping is the swimlane slice's to write (S2), once, and shared.
- The Shell page of the design counts 11 findings in the sidebar, but its own swimlane rows add up to 5. The sidebar sums
  `finding_count` over the list (`useNavBadges`), so a screen's scenario that wants 11 must give the lanes 11 in total.
- The design shows a notifications bell; the contract has nothing to put behind it, so the header has none.
- The permission state says "Ask an Admin": the contract cannot name the admins to a non-admin (CG14).
- The chrome (sidebar and header) is compared with the Shell frames 1a and 1b at 1440x900 by `e2e/visual/chrome.spec.ts`
  (0.03% and 0.02% of the pixels differ at the threshold of 0.2). It needs `LK_REACT18_DIR` (the two React 18.3.1 UMD files) and is
  skipped with "NOT RUN" without it. A slice that changes the header or the sidebar re-runs it; a slice never does.
- `DestructiveConfirm` let Enter on its focused verb button confirm (a button's own Enter is a click), against its contract and the
  keymap's "never in DestructiveConfirm"; the shell fixed it (`dialogs.test.tsx`, `e2e/keyboard/keymap.spec.ts`).
