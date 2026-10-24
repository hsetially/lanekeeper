// The States index rows of the Swimlanes screens, one at a time, in light, each as the app's own baseline (`toHaveScreenshot`; baselines are
// generated in CI only) with a check in words that runs everywhere, so that the state is what it says it is whether or not there is a baseline.
//  L01, E01 and O01 (the offline row) are frames of the Swimlanes board and are compared with the design in overview.spec.ts (1g, 1i, 1a);
//  L02 (the services panel), O02 and U02, R02, E02, P04, K01 and A02 are here; so are R01 and U01.
// G03, the token-expired state of the whole app, has no screen to show: the shared expired-token flag is page-local (T5), so the Swimlanes page
// says nothing about a token. The test says that, so that the day it does, the state gets a baseline.
import { expect } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { appOwned } from "../access/lib/matrix";
import { d } from "../access/lib/why";
import { api, fail, hold, pushEvents, signedInAs } from "../access/lib/steps";

const stream = async (page: Page, harness: { refuseStreams(status: number, count: number): void; closeStreams(): void }): Promise<void> => {
  // The connection drops and the hub refuses to take it back: the pill says so and the shell's banner says "Live updates paused".
  harness.refuseStreams(503, 50);
  harness.closeStreams();
  await page.getByText(/live updates paused/i).first().waitFor();
};

appOwned([
  // ---- the list ----
  {
    name: "R01 swimlanes, the list cannot be read",
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    before: async (page) => {
      await fail(page, api("/swimlanes"));
    },
    design: { row: "R01", specimen: "s-error-swimlanes", key: ["Couldn't load swimlanes", "The server returned an error while reading agent state. Retry, or send the request ID to the platform team.", "Request ID", "Retry"] },
    check: async (page) => {
      await expect(page.getByText(/couldn.t load swimlanes/i).first()).toBeVisible();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
      await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
    },
  },
  {
    name: "U01 swimlanes, live updates paused",
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    streamDown: true,
    prepare: stream,
    design: { row: "U01", specimen: "s-paused-swimlanes", key: ["Live updates paused, retrying", "Showing data from 10:42. Saving and restarts are off until the connection returns.", "Retry now"], differences: [...d("paused", "Showing data from 10:42. Saving and restarts are off until the connection returns.")] },
    check: async (page) => {
      await expect(page.getByText(/live updates paused/i).first()).toBeVisible();
      await expect(page.getByRole("link", { name: "sitb", exact: true })).toBeVisible();
    },
  },
  {
    name: "O01 swimlanes, an agent is offline: the row says so",
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    design: { row: "O01", specimen: "s-offline-swimlanes", key: ["Agent for presitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on presitb are off.", "Check agent"], differences: [...d("banner", "Agent for presitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on presitb are off.", "Check agent")] },
    check: async (page) => {
      await expect(page.getByText(/Offline · 14 min ago/)).toBeVisible();
      await expect(page.getByText(/Stale · last known 3 NFS ahead/)).toBeVisible();
    },
  },
  {
    name: "stale swimlanes, a refresh failed with the list on screen: the last data, and when it is from",
    scenario: "design-swimlanes-six",
    path: "/swimlanes",
    prepare: async (page, harness) => {
      // The list is on screen, and the next read of it fails: an event that names it makes the app ask again.
      await fail(page, api("/swimlanes"));
      await pushEvents(page, harness, [200, { type: "findings_changed", swimlane: "sitb" }]);
      await page.getByText("Couldn't refresh").waitFor();
    },
    check: async (page) => {
      await expect(page.getByText(/Showing the last data that loaded/)).toBeVisible();
      await expect(page.getByRole("button", { name: "Refresh" })).toBeVisible();
      await expect(page.getByRole("link", { name: "sitb", exact: true })).toBeVisible();
    },
  },
  // ---- one swimlane ----
  {
    name: "L02 swimlane detail, loading",
    scenario: "design-swimlanes-six",
    path: "/swimlanes/sitb/services",
    loading: true,
    before: async (page) => {
      await hold(page, api("/swimlanes/sitb"));
    },
    design: { row: "L02", specimen: "s-loading-detail", key: ["Loading services…"] },
    check: async (page) => {
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expect(page.locator("[data-slot='skeleton']").first()).toBeVisible();
    },
  },
  {
    name: "R02 swimlane detail, the swimlane cannot be read",
    scenario: "design-swimlanes-six",
    path: "/swimlanes/sitb/services",
    before: async (page) => {
      await fail(page, api("/swimlanes/sitb"));
    },
    design: { row: "R02", specimen: "s-error-detail", key: ["Couldn't load services", "The server returned an error while reading sitb services. Retry, or send the request ID to the platform team.", "Request ID", "Retry"] },
    check: async (page) => {
      await expect(page.getByText(/couldn.t load/i).first()).toBeVisible();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
    },
  },
  {
    name: "E02 swimlane detail, no files and no services yet",
    scenario: "design-swimlanes-six",
    path: "/swimlanes/presitc/services",
    design: { row: "E02", specimen: "s-empty-detail", key: ["No services reported", "The sitb agent has not reported any services yet. Check that it is running in the sit7 namespace."], differences: [...d("empty", "The sitb agent has not reported any services yet. Check that it is running in the sit7 namespace.")] },
    check: async (page) => {
      await expect(page.getByText("No files", { exact: true })).toBeVisible();
    },
  },
  {
    name: "O02 swimlane detail, the agent is offline: data as of its last scan, restarts off",
    scenario: "design-swimlanes-agent-off",
    path: "/swimlanes/sitb/services",
    design: { row: "O02", specimen: "s-offline-detail", key: ["Agent for sitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on sitb are off.", "Check agent", "Unavailable while the agent is offline"], differences: [...d("offline", "Agent for sitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on sitb are off.")] },
    check: async (page) => {
      await expect(page.getByText(/Agent offline since/)).toBeVisible();
      await expect(page.getByRole("button", { name: "Restart tx-infinity-core" })).toHaveAttribute("aria-disabled", "true");
    },
  },
  {
    name: "U02 swimlane detail, live updates paused",
    scenario: "design-swimlanes-six",
    path: "/swimlanes/sitb/services",
    streamDown: true,
    prepare: stream,
    design: { row: "U02", specimen: "s-paused-detail", key: ["Live updates paused, retrying", "Showing data from 10:42. Saving and restarts are off until the connection returns.", "Retry now", "Unavailable while updates are paused"], differences: [...d("paused", "Showing data from 10:42. Saving and restarts are off until the connection returns."), ...d("pausedRestart", "Unavailable while updates are paused")] },
    check: async (page) => {
      await expect(page.getByText(/live updates paused/i).first()).toBeVisible();
      await expect(page.getByRole("tab", { name: /^Services/ })).toBeVisible();
    },
  },
  {
    name: "P04 swimlane detail, an Editor without the Operator role: Restart is off with its reason",
    scenario: "design-swimlanes-six",
    path: "/swimlanes/sitb/services",
    before: async (page) => {
      await signedInAs(page, "editor");
    },
    prepare: async (page) => {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).hover();
      await page.getByRole("tooltip").first().waitFor();
    },
    design: { row: "P04", specimen: "s-permission-detail", key: ["Restart needs Operator", "You can edit files here, but restarting services needs the Operator permission.", "Requires Operator · Priya Shah can grant"], differences: [...d("permission", "Requires Operator \u00b7 Priya Shah can grant")] },
    check: async (page) => {
      await expect(page.getByRole("button", { name: "Restart tx-infinity-core" })).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByRole("tooltip").first()).toContainText(/operator/i);
    },
  },
  {
    name: "K01 swimlane detail, a restart refused: the swimlane is locked",
    scenario: "design-swimlanes-restart-locked",
    path: "/swimlanes/sitb/services",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await page.getByRole("alertdialog").getByText(/sitb is locked/i).waitFor();
      await page.mouse.move(1439, 899);
    },
    design: { row: "K01", specimen: "s-c423-detail", key: ["sitb is locked", "Marco Diaz started a restart at 10:41. Another restart can't begin until it finishes.", "Request ID", "Close", "Watch progress"], differences: [...d("holder", "Marco Diaz started a restart at 10:41. Another restart can't begin until it finishes."), ...d("stays", "Close"), ...d("progress", "Watch progress")] },
    check: async (page) => {
      await expect(page.getByRole("alertdialog").getByRole("alert")).toContainText("req-mock-0001");
    },
  },
  {
    name: "A02 swimlane detail, a restart sent for approval",
    scenario: "design-swimlanes-restart-approval",
    path: "/swimlanes/sitb/services",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await page.getByText(/Restart sent for approval/).waitFor();
      await page.mouse.move(1439, 899);
    },
    design: { row: "A02", specimen: "s-c202-detail", key: ["Restart sent for approval", "AP-2042 · presita · 2 services. Nothing restarts until an Admin approves.", "View request"], differences: [...d("approval", "AP-2042 \u00b7 presita \u00b7 2 services. Nothing restarts until an Admin approves.")] },
    check: async (page) => {
      await expect(page.getByText(/Nothing restarts until an Admin approves request AP-2042/)).toBeVisible();
    },
  },
  {
    name: "a restart that is accepted, asked of an Admin on the six swimlanes",
    scenario: "design-swimlanes-six",
    path: "/swimlanes/sitb/services",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Restart document-service" }).click();
      await page.getByRole("alertdialog").waitFor();
      await page.mouse.move(1439, 899);
    },
    check: async (page) => {
      await expect(page.getByRole("alertdialog", { name: /Restart document-service in sitb\?/ })).toBeVisible();
      await expect(page.getByRole("button", { name: "Cancel" })).toBeFocused();
    },
  },
]);
