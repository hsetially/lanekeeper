// The frames of Feature status: the board frames 1u to 1x of "Lanekeeper Search and Status.dc.html", and the States-index rows of
// Feature status (L06 E06 R06 O05 U03).
import { HIDDEN_NAV, SWITCHER, type FrameDef, type Mask } from "../search/kit";
import { expectSkeleton, failRequests, holdRequests, expectPaused, pauseStream, pausedMasks } from "../search/steps";

const ADMIN = "design-search-admin";

// What the design draws and the contract cannot send, as regions painted over in both images (design/DEVIATIONS.md: "Feature status").
const SUBTITLE: Mask = { x: 248, y: 88, width: 740, height: 22, row: "Feature status (no unresolved flags in the contract: \"2 unresolved\" is not drawn)" };
const TEMPLATE_HINT: Mask = { x: 1030, y: 88, width: 386, height: 22, row: "Feature status (no template version in the contract)" };
const COUNT: Mask = { x: 1240, y: 130, width: 172, height: 22, row: "Feature status (the design counts its two unresolved flags: \"Showing 10 of 10\")" };
const TABLE: Mask = { x: 249, y: 162, width: 1166, height: 690, row: "Feature status (unresolved note and rows, Doc titles, the Docs column, the channel rows and the cell tooltips: the contract has no unresolved flags, no doc title and no file:line per cell)" };
const LEGEND_HINT: Mask = { x: 1100, y: 856, width: 316, height: 24, row: "Feature status (\"Click a value to compare it across swimlanes\": the cells link to Compare, not to the file at a line)" };

export const frames: FrameDef[] = [
  { board: "search", frame: "1u", kind: "tier1", scenario: ADMIN, path: "/status", title: "Admin, first flag expanded", masks: [SWITCHER, SUBTITLE, TEMPLATE_HINT, COUNT, TABLE, LEGEND_HINT] },
  {
    board: "search", frame: "1v", kind: "tier1", scenario: ADMIN, path: "/status", title: "all channels open", masks: [SWITCHER, SUBTITLE, TEMPLATE_HINT, COUNT, TABLE, LEGEND_HINT],
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Show channels" }).click();
    },
  },
  {
    board: "search", frame: "1w", kind: "tier1", scenario: "design-search-editor", path: "/status", title: "Editor, only flags that differ", masks: [SWITCHER, HIDDEN_NAV, SUBTITLE, TEMPLATE_HINT, COUNT, TABLE, { x: 249, y: 852, width: 1166, height: 32, row: "Feature status (the Editor frame draws a one-line pointer at the foot; the app keeps the legend, and has no unresolved flags to point at)" }],
    steps: async ({ page }) => {
      await page.getByRole("switch").click();
    },
  },
  { board: "search", frame: "1x", kind: "tier1", scenario: ADMIN, path: "/status", title: "Admin", masks: [SWITCHER, SUBTITLE, TEMPLATE_HINT, COUNT, TABLE, LEGEND_HINT] },
];

const APP = "the state of the app's own screen, with the request held or failed (States index)";

// The States-index rows of Feature status, light, as app-owned baselines.
export const states: FrameDef[] = [
  {
    board: "states", frame: "L06", kind: "state", scenario: ADMIN, path: "/status", title: "Feature status, loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/features\/status(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "states", frame: "E06", kind: "state", scenario: "design-search-empty", path: "/status", title: "Feature status, no flags", why: APP },
  { board: "states", frame: "R06", kind: "state", scenario: ADMIN, path: "/status", title: "Feature status, error with the request id", why: APP, before: ({ page }) => failRequests(page, /\/api\/v1\/features\/status(\?|$)/) },
  { board: "states", frame: "O05", kind: "state", scenario: ADMIN, path: "/status", title: "Feature status, an agent offline: last known values", why: APP },
  {
    board: "states", frame: "U03", kind: "state", scenario: ADMIN, path: "/status", title: "Feature status, live updates paused", why: APP,
    before: pauseStream,
    steps: ({ page }) => expectPaused(page),
    mask: pausedMasks,
  },
];
