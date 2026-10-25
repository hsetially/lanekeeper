// The States index rows of Compare, in light, as the app's own baseline (`toHaveScreenshot`, generated in CI only) with a check in words that runs
// everywhere. L04 (loading) and O04 (an agent offline) are frames of the Compare board and are compared with the design in grid.spec.ts (1g, 1i) and
// tree.spec.ts (2h, 2i); E04, R04 and T04 are here.
import { expect } from "../../fixtures/test";
import { appOwned } from "../access/lib/matrix";
import { d } from "../access/lib/why";
import { api, hold, rewrite } from "../access/lib/steps";

const LANES = "sit,sitb,sitc,presita,presitb,presitc";
const CORE = "tx-infinity-api%2Ftx-infinity-core.yml";
const grid = `/compare?lanes=${LANES}&file=${CORE}`;
const tree = `/compare?left=${encodeURIComponent("effective:sitb")}&right=${encodeURIComponent("effective:sitc")}`;

appOwned([
  {
    name: "E04 compare, the swimlanes agree: no row",
    scenario: "design-compare-empty",
    path: grid,
    design: { row: "E04", specimen: "s-empty-compare", key: ["Choose two swimlanes to compare", "Pick a left and a right swimlane. Files that differ are listed here.", "Choose swimlanes"], differences: [...d("chooseTwo", "Choose two swimlanes to compare", "Pick a left and a right swimlane. Files that differ are listed here.", "Choose swimlanes")] },
    check: async (page) => {
      await expect(page.getByText("All 6 swimlanes have the same values")).toBeVisible();
    },
  },
  {
    name: "E04 compare whole swimlane, identical: nothing to review",
    scenario: "design-compare-empty",
    path: tree,
    check: async (page) => {
      await expect(page.getByText("sitb and sitc are identical")).toBeVisible();
      await expect(page.getByText("Nothing to review.", { exact: false })).toBeVisible();
    },
  },
  {
    name: "R04 compare, the comparison cannot be read",
    scenario: "design-compare-error",
    path: grid,
    check: async (page) => {
      await expect(page.getByRole("heading", { name: "Couldn't load comparison" })).toBeVisible();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
    },
  },
  {
    name: "R04 compare whole swimlane, the comparison cannot be read",
    scenario: "design-compare-error",
    path: tree,
    design: { row: "R04", specimen: "s-error-compare", key: ["Couldn't load comparison", "The server returned an error while reading both swimlanes. Retry, or send the request ID to the platform team.", "Request ID", "Retry"] },
    check: async (page) => {
      await expect(page.getByText(/couldn.t load/i).first()).toBeVisible();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
    },
  },
  {
    name: "L04 compare whole swimlane, loading the comparison",
    scenario: "design-compare-default",
    path: tree,
    loading: true,
    before: async (page) => {
      await hold(page, api("/compare/tree"));
    },
    design: { row: "L04", specimen: "s-loading-compare", key: ["Loading comparison…"] },
    check: async (page) => {
      await expect(page.locator("[data-slot='skeleton'], [role='status']").first()).toBeVisible();
    },
  },
  {
    name: "O04 compare, an agent is offline: its column and the banner",
    scenario: "design-compare-offline",
    path: grid,
    design: { row: "O04", specimen: "s-offline-compare", key: ["Agent for sitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on sitb are off.", "Check agent"], differences: [...d("offline", "Agent for sitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on sitb are off.", "Check agent")] },
    check: async (page) => {
      await expect(page.getByText(/presitb/).first()).toBeVisible();
    },
  },
  {
    name: "T04 compare whole swimlane, the list of files is cut off",
    scenario: "design-compare-default",
    path: tree,
    before: async (page) => {
      // The hub always has one more page: the app asks for ten and says so (the cap of the whole list), and never for an eleventh.
      await rewrite(page, api("/compare/tree"), (body) => ({ ...body, entries: { ...(body["entries"] as Record<string, unknown>), next: "again" } }));
    },
    design: { row: "T04", specimen: "s-truncated-compare", key: ["Results truncated", "Showing the first 1,000 of 4,812 files that differ. Turn on \"Only differences\" or pick a folder.", "Pick a folder", "Narrow results"], differences: [...d("truncation", "Results truncated", "Showing the first 1,000 of 4,812 files that differ. Turn on \"Only differences\" or pick a folder.", "Pick a folder", "Narrow results")] },
    check: async (page) => {
      await expect(page.getByText(/results cut off|showing the first/i).first()).toBeVisible();
    },
  },
]);
