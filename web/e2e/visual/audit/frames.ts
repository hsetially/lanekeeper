// The frames of Audit: the board frames 1v to 1ap of "Lanekeeper Approvals and Audit.dc.html", and the States-index rows of Audit
// (L10 E10 R10 U06 T02).
import type { Excluded, FrameDef, Step } from "../search/kit";
import { auditEvent, PRIYA } from "../../../src/mocks/design/approvals/fixtures.ts";
import { expectSkeleton, failRequests, holdRequests, expectPaused, pauseStream, pausedMasks, rowWith } from "../search/steps";

const AUDIT = "design-approvals-audit";
const APP = "the table, filters and drawer differ from the design by documented rows (design/DEVIATIONS.md, \"Audit: ...\")";

const eventDrawer = (seq: string): Step => async ({ page }) => {
  await rowWith(page, new RegExp(`#${seq}`)).click();
  await page.getByRole("button", { name: "Close" }).first().waitFor();
};

export const frames: FrameDef[] = [
  { board: "approvals", frame: "1v", kind: "app", scenario: AUDIT, path: "/audit", title: "default: the chain, quick filters, attribution column", why: APP },
  { board: "approvals", frame: "1x", kind: "app", scenario: "design-approvals-audit-broken", path: "/audit", title: "integrity: a link that does not follow", why: APP },
  { board: "approvals", frame: "1y", kind: "app", scenario: "design-approvals-audit-broken", path: "/audit", title: "integrity: the drawer of the event after the cut", why: APP, steps: eventDrawer("18,456") },
  {
    board: "approvals", frame: "1aa", kind: "app", scenario: AUDIT, path: "/audit", title: "quick filter: via Cursor", why: APP,
    steps: async ({ page }) => {
      await page.getByRole("group", { name: "Quick filter" }).getByRole("button", { name: /^Via Cursor/ }).click();
    },
  },
  {
    board: "approvals", frame: "1ab", kind: "app", scenario: AUDIT, path: "/audit", title: "quick filter: unknown author", why: APP,
    steps: async ({ page }) => {
      await page.getByRole("group", { name: "Quick filter" }).getByRole("button", { name: /^Unknown author/ }).click();
    },
  },
  { board: "approvals", frame: "1ag", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: unknown author", why: APP, steps: eventDrawer("18,459") },
  { board: "approvals", frame: "1ah", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: before the upgrade", why: APP, steps: eventDrawer("18,453") },
  { board: "approvals", frame: "1ai", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: attribution upgraded", why: APP, steps: eventDrawer("18,454") },
  { board: "approvals", frame: "1aj", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: NFS client at medium", why: APP, steps: eventDrawer("18,452") },
  { board: "approvals", frame: "1ak", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: reveal", why: APP, steps: eventDrawer("18,463") },
  { board: "approvals", frame: "1al", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: via Cursor", why: APP, steps: eventDrawer("18,460") },
  { board: "approvals", frame: "1am", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: sync", why: APP, steps: eventDrawer("18,451") },
  { board: "approvals", frame: "1an", kind: "app", scenario: AUDIT, path: "/audit", title: "drawer: unknown author", why: APP, steps: eventDrawer("18,459") },
  { board: "approvals", frame: "1ao", kind: "app", scenario: "design-approvals-audit-broken", path: "/audit", title: "integrity: a link that does not follow", why: APP },
  {
    board: "approvals", frame: "1ap", kind: "app", scenario: AUDIT, path: "/audit", title: "loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/admin\/audit(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
];

const NO_FRAME = (what: string): string => `the app has no ${what} (design/DEVIATIONS.md: "Audit: Integrity", "Audit: filters": the contract has no verification operation and no via, source or confidence filter)`;

export const excluded: Excluded[] = [
  { board: "approvals", frame: "1w", why: NO_FRAME("integrity panel with a verification, a checkpoint, a signature and a next run") },
  { board: "approvals", frame: "1z", why: NO_FRAME("\"Verify now\": there is no verification in progress to draw") },
  { board: "approvals", frame: "1ac", why: NO_FRAME("Via filter") },
  { board: "approvals", frame: "1ad", why: NO_FRAME("attribution-source filter with counts") },
  { board: "approvals", frame: "1ae", why: NO_FRAME("minimum-confidence filter") },
  { board: "approvals", frame: "1af", why: NO_FRAME("Via and confidence filters, so \"detected edits only, 70%+\" cannot be asked") },
];

/** The hub's audit read with a next page for ever: three events a page, so that the app reaches its cap of 50 pages (State T02). */
const endlessLog: Step = async ({ page }) => {
  await page.route(/\/api\/v1\/admin\/audit(\?|$)/, async (route) => {
    const n = Number(new URL(route.request().url()).searchParams.get("cursor") ?? "0");
    const items = [0, 1, 2].map((i) =>
      auditEvent({ seq: 90_000 - n * 3 - i, at: 1_759_999_000_000 - (n * 3 + i) * 60_000, action: "file_edited", actor: PRIYA, via: "ui", target: `tx-infinity-api/file-${(n * 3 + i) % 97}.yml` }),
    );
    await route.fulfill({ json: { items, next: String(n + 1) } });
  });
};

// The States-index rows of Audit, light, as app-owned baselines.
export const states: FrameDef[] = [
  {
    board: "states", frame: "L10", kind: "state", scenario: AUDIT, path: "/audit", title: "Audit, loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/admin\/audit(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "states", frame: "E10", kind: "state", scenario: "design-approvals-audit-empty", path: "/audit", title: "Audit, empty", why: APP },
  { board: "states", frame: "R10", kind: "state", scenario: AUDIT, path: "/audit", title: "Audit, error with the request id", why: APP, before: ({ page }) => failRequests(page, /\/api\/v1\/admin\/audit(\?|$)/) },
  {
    board: "states", frame: "U06", kind: "state", scenario: AUDIT, path: "/audit", title: "Audit, live updates paused", why: APP,
    before: pauseStream,
    steps: ({ page }) => expectPaused(page),
    mask: pausedMasks,
  },
  {
    board: "states", frame: "T02", kind: "state", scenario: AUDIT, path: "/audit", title: "Audit, cut off after 50 pages", why: APP,
    before: endlessLog,
    steps: async ({ page }) => {
      for (let i = 0; i < 60; i += 1) {
        const more = page.getByRole("button", { name: "Load more" });
        if ((await more.count()) === 0) break;
        await more.click();
        await page.getByRole("button", { name: "Load more" }).or(page.getByText(/cut off|Showing the first/)).first().waitFor();
      }
      await page.getByText(/cut off|Showing the first/).first().waitFor();
    },
  },
];
