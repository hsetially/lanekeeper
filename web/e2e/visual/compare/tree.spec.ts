// Tier 1, Compare, whole swimlane: board "Compare" frames 2a to 2i against the app on the `design-compare-*` scenarios, 1440x900.
// What differs is the row set of "Compare whole swimlane: tree" (no per-file counts, a status line of its own) and "Compare whole
// swimlane: side headers" (they name where each side is read), so the frame is compared above the panels: the sidebar, the top bar,
// the title and its sentence. The panels are the app's own baseline.
import { tier1, type FrameCase } from "../access/lib/matrix";
import type { Mask } from "../access/lib/compare";
import { api, hold } from "../access/lib/steps";

const BOARD = "Lanekeeper Compare.dc.html";
const SWITCHER: Mask = { x: 820, y: 4, width: 290, height: 36, why: "candidate row: the header names the current swimlane only on a swimlane's own routes" };
const MODE_TOGGLE: Mask = { x: 1060, y: 66, width: 380, height: 50, why: "the design's page is wider than its frame and cuts the mode toggle; the app's fits" };
const BODY: Mask = { x: 224, y: 116, width: 1216, height: 784, why: "Compare whole swimlane: tree and side headers" };
const PAGE = [SWITCHER, MODE_TOGGLE, BODY];

const tree = (left: string, right: string, file?: string): string => `/compare?left=${encodeURIComponent(left)}&right=${encodeURIComponent(right)}${file ? `&file=${encodeURIComponent(file)}` : ""}`;

const cases: FrameCase[] = [
  { name: "sitb against sitc, 38 identical directories skipped, the core file selected", board: BOARD, ids: ["2a", "2b"], scenario: "design-compare-default", path: tree("effective:sitb", "effective:sitc", "tx-infinity-api/tx-infinity-core.yml"), masks: PAGE },
  { name: "sitb against Git branch sit7", board: BOARD, ids: ["2c"], scenario: "design-compare-default", path: tree("nfs:sitb", "git:tenant@sit7", "tx-infinity-api/tx-infinity-core.yml"), masks: PAGE },
  { name: "sitb against tag v2.3.1, a file only in the tag is selected", board: BOARD, ids: ["2d"], scenario: "design-compare-default", path: tree("effective:sitb", "git:base@v2.3.1", "resources/receipt-footer.txt"), masks: PAGE },
  { name: "a binary file selected: a hash comparison", board: BOARD, ids: ["2e"], scenario: "design-compare-default", path: tree("effective:sitb", "effective:sitc", "resources/receipt-logo.jpeg"), masks: PAGE },
  { name: "identical: calm success, the summary reads 0", board: BOARD, ids: ["2f", "2g"], scenario: "design-compare-identical", path: tree("effective:sitb", "effective:sitc"), masks: PAGE },
  {
    name: "loading",
    board: BOARD,
    ids: ["2h"],
    scenario: "design-compare-default",
    path: tree("effective:sitb", "effective:sitc"),
    loading: true,
    before: async (page) => {
      await hold(page, api("/compare/tree"));
    },
    masks: PAGE,
  },
  { name: "right side offline (presitb), data as of 10:28", board: BOARD, ids: ["2i"], scenario: "design-compare-offline", path: tree("effective:sitb", "effective:presitb"), masks: PAGE },
];
tier1(cases);
