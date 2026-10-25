// Tier 1, Compare, one file across swimlanes: board "Compare" frames 1a to 1k against the app on the `design-compare-*` scenarios,
// 1440x900. The grid is the screen where the app and the design differ most, each difference a row of design/DEVIATIONS.md ("Compare
// grid: header", "columns", "counts", "search", "channel filter", "choosing two cells", "cell drawer", "hidden values"), so what is
// compared is the frame: the sidebar, the top bar, the title and its sentence. Everything under the title is the app's own baseline.
//  - the header's switcher: the design names the current swimlane on every page, the app only on a swimlane's own routes (candidate row).
//  - the mode toggle: the design's page is wider than its frame and cuts the toggle at the frame's edge, the app's fits.
//  - the toolbar, the grid and its footer: the rows listed above.
import { expect } from "../../fixtures/test";
import { tier1, type FrameCase } from "../access/lib/matrix";
import type { Mask } from "../access/lib/compare";
import { api, hold } from "../access/lib/steps";
import type { Page } from "@playwright/test";

const BOARD = "Lanekeeper Compare.dc.html";
const LANES = "sit,sitb,sitc,presita,presitb,presitc";
const CORE = "tx-infinity-api%2Ftx-infinity-core.yml";
const grid = (query = ""): string => `/compare?lanes=${LANES}&file=${CORE}${query}`;

const SWITCHER: Mask = { x: 820, y: 4, width: 290, height: 36, why: "candidate row: the header names the current swimlane only on a swimlane's own routes" };
const MODE_TOGGLE: Mask = { x: 1060, y: 66, width: 380, height: 50, why: "the design's page is wider than its frame and cuts the mode toggle; the app's fits" };
const BODY: Mask = { x: 224, y: 116, width: 1216, height: 784, why: "Compare grid: header, columns, counts, search, channel filter, choosing two cells, cell drawer" };
const PAGE = [SWITCHER, MODE_TOGGLE, BODY];

async function pickTwo(page: Page, row: RegExp, a: RegExp | string, b: RegExp | string): Promise<void> {
  const line = page.getByRole("row", { name: row });
  await line.getByRole("button", { name: a, exact: typeof a === "string" }).first().click();
  await line.getByRole("button", { name: b, exact: typeof b === "string" }).first().click();
  await page.getByRole("dialog", { name: "Diff at setting" }).waitFor();
  await page.mouse.move(1439, 899);
}

const cases: FrameCase[] = [
  { name: "grid, only differing rows, six agents online", board: BOARD, ids: ["1a", "1b"], scenario: "design-compare-default", path: grid(), masks: PAGE },
  {
    name: "two cells picked (A sit, B sitb): the drawer shows value against value",
    board: BOARD,
    ids: ["1c", "1f"],
    scenario: "design-compare-default",
    path: grid(),
    prepare: (page) => pickTwo(page, /core\.pool\.host-conn\. max-size/, "100", /^50/),
    masks: PAGE,
  },
  {
    name: "the drawer, absent against present",
    board: BOARD,
    ids: ["1d"],
    scenario: "design-compare-default",
    path: grid(),
    prepare: (page) => pickTwo(page, /core\.pool\.db\. validate/, "true", /^absent/),
    masks: PAGE,
  },
  {
    name: "only differing rows off: every row",
    board: BOARD,
    ids: ["1e"],
    scenario: "design-compare-default",
    path: grid("&only=all"),
    masks: PAGE,
  },
  {
    name: "loading: the column names are known, the values are pending",
    board: BOARD,
    ids: ["1g"],
    scenario: "design-compare-default",
    path: grid(),
    loading: true,
    before: async (page) => {
      await hold(page, api("/grid"));
    },
    masks: PAGE,
  },
  {
    name: "too many swimlanes: 43 selected, the limit is 40",
    board: BOARD,
    ids: ["1h"],
    scenario: "design-compare-toomany",
    path: `/compare?lanes=${["sit", ...Array.from({ length: 25 }, (_, i) => `sit${String.fromCharCode(98 + i)}`), ...Array.from({ length: 17 }, (_, i) => `presit${String.fromCharCode(97 + i)}`)].join(",")}&file=${CORE}`,
    masks: PAGE,
  },
  { name: "presitb agent offline: a dashed column, as of 10:28, a banner with retry", board: BOARD, ids: ["1i", "1j"], scenario: "design-compare-offline", path: grid(), masks: PAGE },
  {
    name: "search with no match",
    board: BOARD,
    ids: ["1k"],
    scenario: "design-compare-default",
    path: grid(),
    prepare: async (page) => {
      await page.getByRole("searchbox", { name: "Search settings" }).fill("zzz.retry");
      await expect(page.getByText(/no (settings|rows)/i).first()).toBeVisible();
    },
    masks: PAGE,
  },
];
tier1(cases);
