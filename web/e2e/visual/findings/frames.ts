// The frames of Findings: the board frames of "Lanekeeper Findings and Adoption.dc.html" that belong to the Findings screen (1a to
// 1t; the adoption wizard's frames 1u to 1am are slice S6b's), and the States-index rows of Findings (L07 E07 R07 O06 U04 T03).
//
// No Findings frame is tier 1: the screen differs from the design by documented rows that change what every row, the toolbar and
// the drawer draw (design/DEVIATIONS.md: "Findings: tabs and actions" (no Open and Intentional tabs), "Findings: rows" (32 px one
// line, not 44 px two lines), "Findings: counts and order", "Findings: checks" (C1 to C12 names and copy), "Findings: severity",
// "Findings: drawer evidence" (no commits, hashes or timeline), "Findings: truncated list"). A pixel comparison would say that those
// rows exist and nothing else; the screens are guarded by app-owned baselines, which are generated in CI.
import type { Excluded, FrameDef, Step } from "../search/kit";
import { expectSkeleton, failRequests, holdRequests, expectPaused, pauseStream, pausedMasks, rowWith } from "../search/steps";
import { frame } from "../search/sse";

const LIST = "/findings";
const APP = "the app's own screen: the rows, tabs and drawer differ from the design by documented rows (design/DEVIATIONS.md, \"Findings: ...\")";

const drawerOf = (path: RegExp): Step => async ({ page }) => {
  await rowWith(page, path).click();
  await page.getByRole("button", { name: "Close" }).first().waitFor();
};

export const frames: FrameDef[] = [
  { board: "findings", frame: "1a", kind: "app", scenario: "design-findings-open", path: LIST, title: "open findings, Admin", why: APP },
  {
    board: "findings", frame: "1b", kind: "app", scenario: "design-findings-open", path: "/findings?code=C1,C2", title: "swimlane menu open", why: APP,
    steps: async ({ page }) => {
      await page.getByRole("group", { name: "Filters" }).getByRole("button", { name: /Swimlane/ }).click();
      await page.getByRole("menu").waitFor();
    },
  },
  { board: "findings", frame: "1c", kind: "app", scenario: "design-findings-open", path: "/findings?lanes=sitb,sitc&severity=medium", title: "filters applied", why: APP },
  { board: "findings", frame: "1d", kind: "app", scenario: "design-findings-open", path: "/findings?lanes=sit&code=C8", title: "no match", why: APP },
  {
    board: "findings", frame: "1f", kind: "app", scenario: "design-findings-open", path: LIST, title: "loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/findings(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "findings", frame: "1g", kind: "app", scenario: "design-findings-empty", path: LIST, title: "empty", why: APP },
  { board: "findings", frame: "1h", kind: "app", scenario: "design-findings-open", path: LIST, title: "C1 drawer", why: APP, steps: drawerOf(/entryGroupConfig-sit9/) },
  { board: "findings", frame: "1i", kind: "app", scenario: "design-findings-open", path: LIST, title: "C2 drawer", why: APP, steps: drawerOf(/account-sorting-config-sit7/) },
  { board: "findings", frame: "1j", kind: "app", scenario: "design-findings-open", path: LIST, title: "C3 drawer", why: APP, steps: drawerOf(/legacy-batch-sit1/) },
  { board: "findings", frame: "1k", kind: "app", scenario: "design-findings-open", path: LIST, title: "C4 drawer", why: APP, steps: drawerOf(/tmp-hotfix/) },
  { board: "findings", frame: "1l", kind: "app", scenario: "design-findings-open", path: LIST, title: "C5 drawer", why: APP, steps: drawerOf(/gateway-routes-sit9/) },
  { board: "findings", frame: "1m", kind: "app", scenario: "design-findings-open", path: LIST, title: "C6 drawer, presita", why: APP, steps: drawerOf(/tx-infinity-core-sit11/) },
  { board: "findings", frame: "1n", kind: "app", scenario: "design-findings-open", path: LIST, title: "C6 drawer, presitb", why: APP, steps: drawerOf(/tx-infinity-payments-sit12/) },
  { board: "findings", frame: "1o", kind: "app", scenario: "design-findings-open", path: LIST, title: "C7 drawer", why: APP, steps: drawerOf(/tx-infinity-gateway-sit7/) },
  { board: "findings", frame: "1p", kind: "app", scenario: "design-findings-open", path: LIST, title: "C8 drawer", why: APP, steps: drawerOf(/security-roles/) },
  { board: "findings", frame: "1s", kind: "app", scenario: "design-findings-viewer", path: LIST, title: "Viewer, drawer", why: APP, steps: drawerOf(/account-sorting-config-sit7/) },
  { board: "findings", frame: "1t", kind: "app", scenario: "design-findings-open", path: LIST, title: "C7 drawer", why: APP, steps: drawerOf(/tx-infinity-gateway-sit7/) },
];

const CG6 = (what: string): string => `the app has no ${what}: the contract has no operation that marks a finding intentional and no \`lost_version\` on a finding (plan CG6; design/DEVIATIONS.md: "Findings: tabs and actions")`;

export const excluded: Excluded[] = [
  { board: "findings", frame: "1e", why: CG6("Intentional tab, so no drawer shows who marked a finding intentional, why and when") },
  { board: "findings", frame: "1q", why: CG6("\"Mark intentional\" action and its reason dialog") },
  { board: "findings", frame: "1r", why: CG6("\"Restore lost version\" action and its consequence dialog") },
];

// The States-index rows of Findings, light, as app-owned baselines (held or failed requests with `page.route`).
export const states: FrameDef[] = [
  {
    board: "states", frame: "L07", kind: "state", scenario: "design-findings-open", path: LIST, title: "Findings, loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/findings(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "states", frame: "E07", kind: "state", scenario: "design-findings-empty", path: LIST, title: "Findings, empty", why: APP },
  {
    board: "states", frame: "R07", kind: "state", scenario: "design-findings-open", path: LIST, title: "Findings, error with the request id", why: APP,
    before: ({ page }) => failRequests(page, /\/api\/v1\/findings(\?|$)/),
  },
  { board: "states", frame: "O06", kind: "state", scenario: "design-findings-offline", path: LIST, title: "Findings, offline agent (as of)", why: APP },
  { board: "states", frame: "T03", kind: "state", scenario: "design-findings-truncated", path: LIST, title: "Findings, cut off", why: APP },
  {
    board: "states", frame: "U04", kind: "state", scenario: "design-findings-open", path: LIST, title: "Findings, live updates paused", why: APP,
    before: pauseStream,
    steps: ({ page }) => expectPaused(page),
    mask: pausedMasks,
  },
  { board: "states", frame: "R07-403", kind: "state", scenario: "design-findings-open", path: LIST, title: "Findings, the hub refuses with 403", why: APP, before: ({ page }) => failRequests(page, /\/api\/v1\/findings(\?|$)/, 403, "Forbidden") },
  {
    board: "states", frame: "R07-refetch", kind: "state", scenario: "design-findings-open", path: LIST, title: "Findings, a refetch failed with the rows on screen", why: APP,
    steps: async ({ page, harness }) => {
      await failRequests(page, /\/api\/v1\/findings(\?|$)/);
      harness.emit(frame(700, { type: "findings_changed", swimlane: "sitb" }));
      await page.getByText(/Couldn.t refresh/).first().waitFor();
    },
  },
];
