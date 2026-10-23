// Tier 1, the Swimlanes overview: boards "Swimlanes" frames 1a to 1j (six swimlanes, the Viewer, the column chooser, forty swimlanes,
// loading and empty) against the app on the `design-swimlanes-*` scenarios, 1440x900, light and dark.
//
// What is masked, and why (each a row of design/DEVIATIONS.md, named here by its first column):
//  - the header's swimlane switcher: the design names the current swimlane on every page ("SIT sitb sit7, 4 drift"); the app names it
//    on a swimlane's own routes only and says "Choose a swimlane" on /swimlanes. Not a row yet: a candidate for the design owner.
//  - the table (header and rows): "Swimlanes: Agent column" (no Degraded), "Swimlanes: Branch column", "Swimlanes: Restarts column"
//    (Pickup) and "Swimlanes: Last change column" (Last scan) change what four of its columns say, and the rows are in the app's
//    order (by name) where the design lists its own. The chrome, the title, the toolbar and the footer are compared.
//  - the subtitle's counts ("4 of 6 agents online" has a Degraded agent that the contract does not have: "Swimlanes: Agent column").
//  - the footer's legend ends with "Pending restart" in the design and "Needs notify or restart" and "Needs config-server restart" in the app
//    ("Swimlanes: Restarts column").
//  - the toast: "Toasts" (the app's own toaster, "View swimlane" where the design says "View diff").
import type { Page } from "@playwright/test";
import { pushEvents, hold, api } from "../access/lib/steps";
import { maskOf, tier1, type FrameCase } from "../access/lib/matrix";
import type { Mask } from "../access/lib/compare";

const BOARD = "Lanekeeper Swimlanes.dc.html";

const SWITCHER: Mask = { x: 820, y: 4, width: 290, height: 36, why: "candidate row: the header names the current swimlane only on a swimlane's own routes" };
const SUBTITLE: Mask = { x: 246, y: 90, width: 420, height: 20, why: "Swimlanes: Agent column (no Degraded, so 5 of 6 are online)" };
const TABLE_SIX: Mask = { x: 249, y: 162, width: 1168, height: 204, why: "Swimlanes: Branch, Agent, Restarts and Last change columns; row order" };
const TABLE_FORTY: Mask = { x: 249, y: 162, width: 1168, height: 696, why: "Swimlanes: Branch, Agent, Restarts and Last change columns; row order" };
const LEGEND: Mask = { x: 700, y: 860, width: 710, height: 22, why: "Swimlanes: Restarts column (the legend's last entries are the two pickup states that need action)" };
const FINDINGS_BADGE: Mask = { x: 172, y: 268, width: 40, height: 24, why: "dataset: the sidebar sums the findings of the swimlanes the scenario lists; the Shell page's 11 is typed in" };
const VIEWER_NAV: Mask = { x: 8, y: 296, width: 208, height: 70, why: "Sidebar: Audit and Approvals (hidden from a Viewer; the design's Viewer frame shows Audit)" };
const LOADING_LINE = maskOf("[data-slot='query-loading'] [role='status']", "candidate row: a status line, 'Loading swimlanes…', under the skeleton (shared QueryBoundary)", 4);
const TOAST: Mask = { x: 1020, y: 800, width: 410, height: 90, why: "Toasts" };

async function selectTwo(page: Page): Promise<void> {
  await page.getByRole("checkbox", { name: "Select sit", exact: true }).check();
  await page.getByRole("checkbox", { name: "Select sitc", exact: true }).check();
}

const cases: FrameCase[] = [
  {
    name: "six swimlanes, two selected, sitb just changed",
    board: BOARD,
    ids: ["1a", "1b"],
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    prepare: async (page, harness) => {
      await selectTwo(page);
      // A scan saw sitb's core file change on NFS: the row is highlighted and the toast says so.
      await pushEvents(page, harness, [100, { type: "file_observed", swimlane: "sitb", path: "tx-infinity/tx-infinity-core-sit1.yml", hash: "a".repeat(64), source: "scan" }]);
    },
    masks: [SWITCHER, SUBTITLE, TABLE_SIX, LEGEND, TOAST],
    // A toast leaves by itself after a while: the app's own baseline does not photograph it.
    moving: (page) => [page.locator("[data-slot='toaster']")],
  },
  {
    name: "a Viewer: no Add a swimlane, Compare disabled",
    board: BOARD,
    ids: ["1c"],
    scenario: "design-swimlanes-six-viewer",
    path: "/swimlanes",
    masks: [SWITCHER, SUBTITLE, TABLE_SIX, LEGEND, VIEWER_NAV],
  },
  {
    name: "the column chooser open",
    board: BOARD,
    ids: ["1d"],
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Columns" }).click();
      await page.getByRole("menu", { name: "Columns" }).waitFor();
    },
    masks: [SWITCHER, SUBTITLE, TABLE_SIX, LEGEND],
  },
  {
    name: "forty swimlanes, the rows scroll inside the table",
    board: BOARD,
    ids: ["1e", "1f"],
    scenario: "design-swimlanes-forty",
    path: "/swimlanes",
    masks: [SWITCHER, SUBTITLE, TABLE_FORTY, LEGEND, FINDINGS_BADGE],
  },
  {
    name: "loading skeleton",
    board: BOARD,
    ids: ["1g", "1h"],
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    loading: true,
    before: async (page) => {
      await hold(page, api("/swimlanes"));
    },
    masks: [SWITCHER, FINDINGS_BADGE, LOADING_LINE],
  },
  { name: "empty, an Admin is offered Add a swimlane", board: BOARD, ids: ["1i"], scenario: "design-swimlanes-empty", path: "/swimlanes", masks: [SWITCHER, FINDINGS_BADGE] },
  { name: "empty, a Viewer is told to ask an admin", board: BOARD, ids: ["1j"], scenario: "design-swimlanes-empty-viewer", path: "/swimlanes", masks: [SWITCHER, FINDINGS_BADGE, VIEWER_NAV] },
];

tier1(cases);
