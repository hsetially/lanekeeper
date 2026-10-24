// The frames of Search (text, settings, docs list): the board frames 1a to 1j and 1n of "Lanekeeper Search and Status.dc.html", and the
// States-index rows of Search (L05 E05 R05 T01).
import { SWITCHER, type FrameDef, type Mask } from "./kit";
import { expectSkeleton, holdRequests } from "./steps";

const TEXT = "/search?type=text&q=enableAccountSorting";
const ADMIN = "design-search-admin";

// What the design draws and the app does not, as regions that are painted over in both images. Each names the row of
// design/DEVIATIONS.md that explains it ("Screen / component" column).
const TREE_ROWS: Mask = { x: 249, y: 238, width: 1166, height: 614, row: "Search: text results (the shared tree: chevrons, indents, no \"Open at line N\" hint, tenant and base chips at the right)" };
const TREE_FOOTER: Mask = { x: 249, y: 852, width: 1166, height: 32, row: "Search: text results (no \"Cmd+Enter new tab\" footer hint)" };
const BELOW_TOOLBAR: Mask = { x: 249, y: 206, width: 1166, height: 678, row: "Search: text / settings / docs (everything below the toolbar differs by the rows named on the frame)" };
const SETTINGS_SUBTITLE: Mask = { x: 248, y: 88, width: 420, height: 22, row: "Search: settings (matches the setting path only: \"by its path\", not \"by name or value\")" };
const SETTINGS_NOTE: Mask = { x: 420, y: 208, width: 300, height: 28, row: "Search: settings (\"Matches setting paths\", not \"path and value\")" };
const SETTINGS_ROWS: Mask = { x: 249, y: 270, width: 1166, height: 614, row: "Search: settings (no strike-through of overridden base values, the shared table's cell padding, a \"Showing N\" footer)" };
const DOCS_NOTE: Mask = { x: 385, y: 208, width: 340, height: 28, row: "Search: docs (\"Best matches first, from titles, headings and text\")" };
const DOCS_ROWS: Mask = { x: 249, y: 238, width: 1166, height: 646, row: "Search: docs (the source chip says Git or Uploaded without the repository or the person, the anchors carry the hub's doc- prefix, the snippet marks come from the hub)" };

export const frames: FrameDef[] = [
  { board: "search", frame: "1a", kind: "tier1", scenario: ADMIN, path: TEXT, title: "text search", masks: [SWITCHER, TREE_ROWS, TREE_FOOTER] },
  {
    board: "search", frame: "1b", kind: "app", scenario: ADMIN, path: TEXT, title: "swimlane picker",
    why: "the picker is the app's own checkbox list with a filter field (design/DEVIATIONS.md: \"Search: swimlane picker\")",
    steps: async ({ page }) => {
      await page.getByRole("button", { name: /^Swimlanes/ }).click();
      await page.getByRole("dialog", { name: "Choose swimlanes" }).waitFor();
    },
  },
  { board: "search", frame: "1c", kind: "tier1", scenario: "design-search-truncated", path: "/search?type=text&q=timeout", title: "truncated", masks: [SWITCHER, BELOW_TOOLBAR] },
  { board: "search", frame: "1d", kind: "tier1", scenario: "design-search-invalid", path: "/search?type=text&q=enableAccount(Sort&rx=1", title: "invalid regex", masks: [SWITCHER, { ...BELOW_TOOLBAR, y: 196, height: 688 }] },
  { board: "search", frame: "1e", kind: "tier1", scenario: ADMIN, path: "/search?type=text&q=enable.*Sorting&lanes=presita,presitb", title: "no results", masks: [SWITCHER] },
  { board: "search", frame: "1f", kind: "tier1", scenario: ADMIN, path: TEXT, title: "results", masks: [SWITCHER, TREE_ROWS, TREE_FOOTER] },
  { board: "search", frame: "1g", kind: "tier1", scenario: ADMIN, path: "/search?type=settings&q=enableAccountSorting", title: "settings", masks: [SWITCHER, SETTINGS_SUBTITLE, SETTINGS_NOTE, SETTINGS_ROWS] },
  { board: "search", frame: "1h", kind: "tier1", scenario: ADMIN, path: "/search?type=settings&q=enableAccountSortng", title: "settings no results", masks: [SWITCHER, SETTINGS_SUBTITLE, SETTINGS_NOTE, { ...BELOW_TOOLBAR, y: 238, height: 646 }] },
  { board: "search", frame: "1i", kind: "tier1", scenario: ADMIN, path: "/search?type=settings&q=enableAccountSorting", title: "settings", masks: [SWITCHER, SETTINGS_SUBTITLE, SETTINGS_NOTE, SETTINGS_ROWS] },
  { board: "search", frame: "1j", kind: "tier1", scenario: ADMIN, path: "/search?type=docs&q=account%20sorting", title: "docs results", masks: [SWITCHER, DOCS_NOTE, DOCS_ROWS] },
  { board: "search", frame: "1n", kind: "tier1", scenario: ADMIN, path: "/search?type=docs&q=ledger%20sorting", title: "docs no results", masks: [SWITCHER, DOCS_NOTE, { ...DOCS_ROWS, y: 238 }] },
];

const APP = "the state of the app's own screen, with the request held or failed (States index)";

// The States-index rows of Search, light, as app-owned baselines.
export const states: FrameDef[] = [
  {
    board: "states", frame: "L05", kind: "state", scenario: ADMIN, path: TEXT, title: "Search, loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/search\/text(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "states", frame: "E05", kind: "state", scenario: "design-search-empty", path: TEXT, title: "Search, no results", why: APP },
  { board: "states", frame: "R05", kind: "state", scenario: "design-search-error", path: TEXT, title: "Search, the search failed (request id)", why: APP },
  { board: "states", frame: "T01", kind: "state", scenario: "design-search-truncated", path: "/search?type=text&q=timeout", title: "Search, cut off at the cap", why: APP },
];
