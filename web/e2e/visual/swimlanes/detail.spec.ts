// Tier 1, the swimlane detail page: board "Swimlane Detail" frames 1a to 1n (Services with the tree physical and logical, Drift,
// Findings, Activity, and the states: locked, agent offline, a Viewer, a sync in progress) against the app on the
// `design-swimlanes-*` scenarios, 1440x900.
//
// What is masked, and why (each a row of design/DEVIATIONS.md, named here by its first column):
//  - the header: "Swimlane detail: header" (no file count, no "/" hint on Search in), "Swimlanes: Branch column" and "Swimlane
//    switcher and swimlane headers" (one badge, the colour of the most urgent state), "Swimlane detail: header, pickup chips" (one chip for
//    each pickup state with its own count, where the design has one chip: D85), "Copy affordances the design does not draw" (a Copy beside
//    the swimlane name).
//  - the breadcrumb's third crumb names the tab in the app; the design stops at the swimlane (candidate row).
//  - the tree: "Swimlane detail: file tree" (lazy folders: the app's tree is closed until a folder is opened, no drift counts, and the
//    pickup marks on files and folders that the design does not draw).
//  - the tab bar's counts and the panel under it: "Swimlane detail: Services tab" (the pickup chip, the Notify services button, the
//    auto-notify line and the config-server banner), "Swimlane detail: Drift tab", "Swimlane detail: Findings tab", "Swimlane detail:
//    Activity and Audit tabs".
// What stays compared: the sidebar, the top bar (but for the switcher's badge and the third crumb), the tree's filter strip and the
// frame around the two panes.
import { tier1, type FrameCase } from "../access/lib/matrix";
import type { Mask } from "../access/lib/compare";

const BOARD = "Lanekeeper Swimlane Detail.dc.html";

const BADGE: Mask = { x: 1004, y: 8, width: 74, height: 28, why: "Swimlane switcher and swimlane headers (one badge, the colour of the most urgent state)" };
const CRUMB: Mask = { x: 330, y: 8, width: 140, height: 28, why: "candidate row: the breadcrumb's third crumb names the tab" };
const HEADER: Mask = { x: 245, y: 56, width: 745, height: 62, why: "Swimlane detail: header; Swimlane detail: header, pickup chips; Copy affordances the design does not draw" };
const ACTIONS: Mask = { x: 980, y: 56, width: 440, height: 38, why: "Swimlane detail: header (no / hint on Search in)" };
const TABS: Mask = { x: 691, y: 126, width: 749, height: 34, why: "Swimlane detail: Drift / Findings tabs (counts: the contract has no Untracked state; the Activity dot is a live feed)" };
const TREE: Mask = { x: 224, y: 203, width: 462, height: 697, why: "Swimlane detail: file tree" };
const PANEL: Mask = { x: 691, y: 161, width: 749, height: 739, why: "the tab's panel (Services, Drift, Findings, Activity rows)" };
const SWITCH: Mask = { x: 538, y: 170, width: 36, height: 22, why: "Switch (a 28x16 track, as the Style Guide draws it; the handoff table says 28x14)" };
const COMMON = [BADGE, CRUMB, HEADER, ACTIONS, TABS, TREE, PANEL, SWITCH];
// A banner above the header (locked, agent offline, sync in progress) pushes the page under it down by its own height, which is the design's
// 40 px and the app's 42 (its border is outside the 24 px button and 8 px of padding): what is under the top bar is the banner's text ("since
// 14:05" against "since 08:39 UTC": Times) and the page it moved, so under a banner only the chrome is compared, and the banner's layout is
// asserted in a test of its own below.
const UNDER_BAR: Mask = { x: 224, y: 44, width: 1216, height: 856, why: "a banner above the header: its words (Times) and the page under it, moved by its height" };
const WITH_BANNER = [BADGE, CRUMB, UNDER_BAR];

const cases: FrameCase[] = [
  { name: "Services tab, physical tree", board: BOARD, ids: ["1a", "1c"], scenario: "design-swimlanes-six", path: "/swimlanes/sitb/services", masks: COMMON },
  {
    name: "Services tab, logical files on",
    board: BOARD,
    ids: ["1b", "1d"],
    scenario: "design-swimlanes-six",
    path: "/swimlanes/sitb/services",
    prepare: async (page) => {
      await page.getByRole("switch", { name: "Show logical files" }).click();
      await page.mouse.move(1439, 899);
    },
    masks: COMMON,
  },
  { name: "Drift tab", board: BOARD, ids: ["1e", "1f"], scenario: "design-swimlanes-six", path: "/swimlanes/sitb/drift", masks: COMMON },
  { name: "Findings tab", board: BOARD, ids: ["1g", "1h"], scenario: "design-swimlanes-six", path: "/swimlanes/sitb/findings", masks: COMMON },
  { name: "Activity tab", board: BOARD, ids: ["1i", "1j"], scenario: "design-swimlanes-six", path: "/swimlanes/sitb/activity", masks: COMMON },
  {
    name: "Editing locked on presitc, unknown drift, an Admin can start adoption",
    board: BOARD,
    ids: ["1k"],
    scenario: "design-swimlanes-six",
    path: "/swimlanes/presitc/drift",
    masks: WITH_BANNER,
  },
  {
    name: "agent offline, data as of the last scan, restart off with its reason",
    board: BOARD,
    ids: ["1l"],
    scenario: "design-swimlanes-agent-off",
    path: "/swimlanes/sitb/services",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).hover();
      await page.getByRole("tooltip").first().waitFor();
    },
    masks: WITH_BANNER,
  },
  {
    name: "a Viewer: Restart disabled with a tooltip",
    board: BOARD,
    ids: ["1m"],
    scenario: "design-swimlanes-six-viewer",
    path: "/swimlanes/sitb/services",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).hover();
      await page.getByRole("tooltip").first().waitFor();
    },
    masks: COMMON,
  },
  { name: "sync in progress, alerts held", board: BOARD, ids: ["1n"], scenario: "design-swimlanes-syncing", path: "/swimlanes/sitb/drift", masks: WITH_BANNER },
];
tier1(cases);
