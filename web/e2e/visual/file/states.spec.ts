// The States index rows of the file view, one at a time, in light, each as the app's own baseline (`toHaveScreenshot`, generated in CI only) with a
// check in words that runs everywhere. The dialogs of the Raise PR flow and the delete confirmation are in dialogs.spec.ts (tier 2).
//   L03 loading, E03 empty file, R03 error, P03 Viewer, O03 agent offline, C01 409 on save, K02 423 on save, A01 202 on save, G01 token expired on
//   Raise PR, and T05 (a truncated diff), which the contract has no field for: asserted absent, as DEVIATIONS "File: Diff and large files" says.
// And the draft page (a screen of its own, CG17): a draft, one that was applied, one that is not there, and one that has gone stale (409).
import { expect, test } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { pin, settle } from "../access/lib/app";
import { appOwned } from "../access/lib/matrix";
import { d } from "../access/lib/why";
import { api, fail, hold, rewrite } from "../access/lib/steps";

const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const FILE = `/swimlanes/sitb/files?path=${CORE}`;
const CONTENT = api("/swimlanes/sitb/files/content");

/** Edit the core file a little, and ask to save it. */
async function editAndSave(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await page.locator(".monaco-editor .view-lines").waitFor();
  await page.locator(".monaco-editor .view-lines").click({ position: { x: 300, y: 100 } });
  await page.keyboard.press("Control+End");
  await page.keyboard.type("\nextra: 1");
  await page.getByRole("button", { name: /^(Save|Submit)/ }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: /^(Save|Submit)/ }).click();
}

appOwned([
  {
    name: "L03 file, loading",
    scenario: "design-file-core",
    path: FILE,
    loading: true,
    before: async (page) => {
      await hold(page, CONTENT);
    },
    design: { row: "L03", specimen: "s-loading-file", key: ["Loading file…"] },
    check: async (page) => {
      await expect(page.locator("[data-slot='skeleton']").first()).toBeVisible();
    },
  },
  {
    name: "E03 file, empty: 0 bytes",
    scenario: "design-file-core",
    path: FILE,
    before: async (page) => {
      await rewrite(page, CONTENT, (body) => ({ ...body, text: "", size: 0, masks: undefined }));
    },
    design: { row: "E03", specimen: "s-empty-file", key: ["The file exists but has 0 bytes. Compare it with Git, or restore the Git version.", "Compare with Git"] },
    check: async (page) => {
      await expect(page.getByText(/is empty on NFS/)).toBeVisible();
      await expect(page.getByText("The file exists but has 0 bytes.", { exact: false })).toBeVisible();
    },
  },
  {
    name: "R03 file, the file cannot be read",
    scenario: "design-file-core",
    path: FILE,
    before: async (page) => {
      await fail(page, CONTENT);
    },
    design: { row: "R03", specimen: "s-error-file", key: ["Couldn't load file", "The server returned an error while reading the file from NFS. Retry, or send the request ID to the platform team.", "Request ID", "Retry"] },
    check: async (page) => {
      await expect(page.getByText(/couldn.t load/i).first()).toBeVisible();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
    },
  },
  {
    name: "P03 file, a Viewer: the write buttons are off with the role they need",
    scenario: "design-file-viewer",
    path: FILE,
    prepare: async (page) => {
      await page.getByRole("button", { name: "Raise PR" }).hover();
      await page.getByRole("tooltip").first().waitFor();
    },
    design: { row: "P03", specimen: "s-permission-file", key: ["Read-only", "Viewers can compare and download. Saving and raising PRs need Editor.", "Requires Editor · Priya Shah can grant"], differences: [...d("permission", "Requires Editor \u00b7 Priya Shah can grant")] },
    check: async (page) => {
      for (const name of ["Edit", "Raise PR", "Upload", "Delete"]) await expect(page.getByRole("button", { name, exact: true }), name).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByRole("tooltip").first()).toContainText(/editor/i);
    },
  },
  {
    name: "O03 file, the agent is offline: the last known file, saving is off",
    scenario: "design-file-agent-offline",
    path: FILE,
    design: { row: "O03", specimen: "s-offline-file", key: ["Agent for sitb is offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on sitb are off.", "Check agent", "Unavailable while the agent is offline"], differences: [...d("offline", "Showing data as of 10:28 (14 min ago). Saving and restarts on sitb are off.")] },
    check: async (page) => {
      await expect(page.getByText("Agent for sitb is offline")).toBeVisible();
      await expect(page.getByRole("button", { name: "Edit", exact: true })).toBeDisabled();
    },
  },
  {
    name: "C01 file, saving: someone else changed it first (409)",
    scenario: "design-file-edit-conflict",
    path: FILE,
    clock: "date",
    prepare: async (page) => {
      await editAndSave(page);
      await page.getByRole("dialog").getByText("File changed on NFS while you were editing").waitFor();
      await page.mouse.move(1439, 899);
    },
    design: { row: "C01", specimen: "s-c409-file", key: ["File changed on NFS while you were editing", "Your save was not applied. Nothing on NFS changed.", "Request ID", "Discard my changes", "View diff", "Reload and re-apply"], differences: [...d("conflict", "View diff")] },
    check: async (page) => {
      await expect(page.getByRole("dialog")).toContainText("Your save was not applied. Nothing on NFS changed.");
      await expect(page.getByRole("button", { name: "Reload and re-apply" })).toBeVisible();
    },
  },
  {
    name: "K02 file, saving: the swimlane is locked (423)",
    scenario: "design-file-edit-locked",
    path: FILE,
    clock: "date",
    prepare: async (page) => {
      await editAndSave(page);
      await page.getByRole("dialog").getByText("sitb is locked until it is adopted").waitFor();
      await page.mouse.move(1439, 899);
    },
    design: { row: "K02", specimen: "s-c423-file", key: ["presitc is locked until it is adopted", "Its baseline is unconfirmed, so edits are blocked. An admin needs to adopt 412 unknown files first.", "Request ID", "Close", "Start adoption"], differences: [...d("dataset", "presitc is locked until it is adopted")] },
    check: async (page) => {
      await expect(page.getByRole("dialog")).toContainText("Request ID");
      await expect(page.getByRole("link", { name: "Start adoption" })).toBeVisible();
    },
  },
  {
    name: "A01 file, saving: sent for approval (202)",
    scenario: "design-file-edit-approval",
    path: FILE,
    clock: "date",
    prepare: async (page) => {
      await editAndSave(page);
      await page.getByRole("dialog").getByText("Sent for approval").waitFor();
      await page.mouse.move(1439, 899);
    },
    design: { row: "A01", specimen: "s-c202-file", key: ["Sent for approval", "Saving to PRESIT needs an Admin. Nothing has changed on NFS yet.", "Request", "Waiting on", "Stay on file", "View request"], differences: [...d("approval", "Saving to PRESIT needs an Admin. Nothing has changed on NFS yet.")] },
    check: async (page) => {
      await expect(page.getByRole("dialog")).toContainText("Your change needs an Admin to approve it. Nothing has changed on NFS yet.");
    },
  },
  {
    name: "G01 file, Raise PR with an expired GitHub token",
    scenario: "design-file-pr-token-expired",
    path: FILE,
    prepare: async (page) => {
      await page.getByRole("button", { name: "Raise PR" }).click();
      await page.getByRole("button", { name: "Continue" }).click();
      await page.getByRole("button", { name: "Create PR" }).click();
      await page.getByText("Pull request not created").waitFor();
      await page.mouse.move(1439, 899);
    },
    design: { row: "G01", specimen: "s-token-file", key: ["Your GitHub token expired", "It expired on 8 Oct 2026. Raising a PR needs a valid token. Saving to NFS still works.", "Request ID", "Not now", "Update token"], differences: [...d("token", "It expired on 8 Oct 2026. Raising a PR needs a valid token. Saving to NFS still works.", "Not now", "Update token")] },
    check: async (page) => {
      await expect(page.getByRole("dialog")).toContainText("Your GitHub token expired");
      await expect(page.getByRole("link", { name: "Go to My token" })).toBeVisible();
    },
  },
  {
    name: "a draft saved by a tool, ready to review and apply",
    scenario: "design-file-draft",
    path: "/drafts/7",
    check: async (page) => {
      await expect(page.getByRole("heading", { level: 1, name: "Review draft" })).toBeVisible();
      await expect(page.getByRole("button", { name: /apply/i }).first()).toBeEnabled();
    },
  },
  {
    name: "a draft that was already applied: Apply is off",
    scenario: "design-file-draft-applied",
    path: "/drafts/7",
    check: async (page) => {
      await expect(page.getByText(/applied/i).first()).toBeVisible();
      await expect(page.getByRole("button", { name: /^apply/i }).first()).toBeDisabled();
    },
  },
  {
    name: "a draft that is not there, or is someone else's: the same words",
    scenario: "design-file-draft",
    path: "/drafts/404",
    before: async (page) => {
      await fail(page, api("/drafts/404"), 404, "Not found");
    },
    check: async (page) => {
      await expect(page.getByText("Draft not found")).toBeVisible();
    },
  },
  {
    name: "a draft that is stale: applying it is refused (409)",
    scenario: "design-file-draft-conflict",
    path: "/drafts/7",
    prepare: async (page) => {
      await page.getByRole("button", { name: /^apply/i }).first().click();
      const confirmBtn = page.getByRole("alertdialog").getByRole("button", { name: /apply/i });
      if (await confirmBtn.count()) await confirmBtn.click();
      await page.getByText(/changed|stale|someone else/i).first().waitFor();
      await page.mouse.move(1439, 899);
    },
    check: async (page) => {
      await expect(page.getByText(/changed since|has changed|someone else/i).first()).toBeVisible();
    },
  },
]);

test.describe("states the app does not draw, by the DEVIATIONS rows", () => {
  test("T05, a truncated diff: the comparison has no field that says so, and the Diff tab says nothing of it (File: Diff and large files)", async ({ page, harness }) => {
    harness.setScenario("design-file-diff");
    await pin(page, "light", { width: 1440, height: 900 });
    await page.goto(`${harness.url}${FILE}&tab=diff`);
    await settle(page);
    await expect(page.getByRole("tab", { name: /^Diff/ })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByText(/truncated|too large to diff|diff is cut/i)).toHaveCount(0);
  });
});
