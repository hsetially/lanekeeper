// The states of the file view (S3): view (core with a masked value, viewer, line endings, large, binary, denied, agent offline), its
// tabs, panels, ref menu, edit (the editor, a change, the save dialog and its four answers), Raise PR, upload, delete, and the draft page.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case, Ctx } from "./helpers.ts";
import { fail, hold, modal } from "./helpers.ts";

const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const file = (path = CORE, extra = ""): string => `/swimlanes/sitb/files?path=${path}${extra}`;

async function startEditing(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(page.locator(".monaco-editor").first()).toBeVisible({ timeout: 20_000 });
  await expect(page.getByRole("button", { name: "Cancel" })).toBeVisible();
}

/** The button that sends the change: "Save to sitb NFS", or "Submit for approval" for a person whose writes need it. */
const sendButton = (page: Page) => page.getByRole("button", { name: /^(Save to sitb NFS|Submit for approval)$/ });

async function changeEditor(page: Page): Promise<void> {
  await startEditing(page);
  const box = await page.locator(".monaco-editor").first().boundingBox();
  if (!box) throw new Error("no editor box");
  await page.mouse.click(box.x + 200, box.y + 60);
  await page.keyboard.press("Control+End");
  await page.keyboard.type("# note");
  await expect(sendButton(page)).toBeEnabled();
}

async function openSaveDialog(page: Page): Promise<void> {
  await changeEditor(page);
  await sendButton(page).click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
}

async function confirmSave(page: Page): Promise<void> {
  await openSaveDialog(page);
  await page.getByRole("alertdialog").getByRole("button").last().click();
}

async function raisePr({ page }: Ctx, toStep = 1): Promise<void> {
  await page.getByRole("button", { name: "Raise PR", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Raise pull request" })).toBeVisible();
  if (toStep >= 2) {
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page.getByRole("textbox", { name: "Title" })).toBeVisible();
  }
  if (toStep >= 3) await page.getByRole("button", { name: "Create PR" }).click();
}

export const fileCases: Case[] = [
  // ---- Read-only views ------------------------------------------------------------------------------------------------------------
  { id: "file / core file, NFS tab, a masked value", scenario: "design-file-core", url: file() },
  { id: "file / core file, a viewer (the write buttons say which role)", scenario: "design-file-viewer", url: file() },
  { id: "file / line endings only (CRLF against LF)", scenario: "design-file-lineending", url: file("tx-infinity-api%2Ftx-infinity-gateway.yml") },
  { id: "file / large file (read-only, enable editing on request)", scenario: "design-file-large", url: file("security%2Fsecurity-roles.yml") },
  { id: "file / binary file (a picture)", scenario: "design-file-binary", url: file("document-service%2Fresources%2Freceipt-logo.jpeg") },
  { id: "file / denied file (content withheld)", scenario: "design-file-denied", url: file("security%2Fkeystore.jks") },
  { id: "file / agent offline: the last known file", scenario: "design-file-agent-offline", url: file() },
  { id: "file / choose a file (no path)", scenario: "design-file-core", url: "/swimlanes/sitb/files" },
  {
    id: "file / loading (the file is held)",
    scenario: "design-file-core",
    url: file(),
    async before({ page }) {
      await hold(page, /\/api\/v1\/swimlanes\/sitb\/files\/content/);
    },
  },
  {
    id: "file / the file cannot be read (500)",
    scenario: "design-file-core",
    url: file(),
    async before({ page }) {
      await fail(page, /\/api\/v1\/swimlanes\/sitb\/files\/content/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "file / the file is not there (404)",
    scenario: "design-file-core",
    url: file(),
    async before({ page }) {
      await fail(page, /\/api\/v1\/swimlanes\/sitb\/files\/content/, 404, "File not found");
    },
    async open({ page }) {
      await expect(page.getByText("File not found").first()).toBeVisible();
    },
  },

  // ---- Tabs and panels ------------------------------------------------------------------------------------------------------------
  {
    id: "file / the Git tab",
    scenario: "design-file-core",
    url: file(),
    async open({ page }) {
      await page.getByRole("tab", { name: /^Git/ }).click();
      await expect(page.getByRole("tab", { name: /^Git/ })).toHaveAttribute("aria-selected", "true");
    },
  },
  {
    id: "file / the Baseline tab",
    scenario: "design-file-core",
    url: file(),
    async open({ page }) {
      await page.getByRole("tab", { name: "Baseline" }).click();
      await expect(page.getByRole("tab", { name: "Baseline" })).toHaveAttribute("aria-selected", "true");
    },
  },
  {
    id: "file / the Diff tab, split",
    scenario: "design-file-diff",
    url: file(),
    async open({ page }) {
      await page.getByRole("tab", { name: /^Diff/ }).click();
      await expect(page.getByRole("grid", { name: /^Diff of/ })).toBeVisible();
    },
  },
  {
    id: "file / the Diff tab, unified",
    scenario: "design-file-diff",
    url: file(),
    async open({ page }) {
      await page.getByRole("tab", { name: /^Diff/ }).click();
      await page.getByRole("radio", { name: /Unified/ }).click();
      await expect(page.getByRole("grid", { name: /^Diff of/ })).toBeVisible();
    },
  },
  {
    id: "file / the Diff tab, line endings only",
    scenario: "design-file-lineending",
    url: file("tx-infinity-api%2Ftx-infinity-gateway.yml"),
    async open({ page }) {
      await page.getByRole("tab", { name: /^Diff/ }).click();
      await expect(page.getByRole("tabpanel")).toBeVisible();
    },
  },
  {
    id: "file / the Diff tab, a binary file",
    scenario: "design-file-binary",
    url: file("document-service%2Fresources%2Freceipt-logo.jpeg"),
    async open({ page }) {
      await page.getByRole("tab", { name: /^Diff/ }).click();
      await expect(page.getByRole("tabpanel")).toBeVisible();
    },
  },
  {
    id: "file / the ref menu (what to compare with)",
    scenario: "design-file-diff",
    url: file(),
    async open({ page }) {
      await page.getByRole("tab", { name: /^Diff/ }).click();
      await page.getByRole("button", { name: /^Left / }).click();
      await expect(page.getByRole("menu").or(page.getByRole("dialog")).or(page.getByRole("listbox")).first()).toBeVisible();
    },
  },
  {
    id: "file / the history panel",
    scenario: "design-file-core",
    url: file(),
    async open({ page }) {
      await page.getByRole("button", { name: "History", exact: true }).click();
      await expect(page.getByRole("complementary", { name: "History" })).toBeVisible();
    },
  },
  {
    id: "file / the docs panel",
    scenario: "design-file-core",
    url: file(),
    async open({ page }) {
      await page.getByRole("button", { name: "Docs for this file", exact: true }).click();
      await expect(page.getByRole("complementary", { name: "Docs for this file" })).toBeVisible();
    },
  },

  // ---- Edit (Monaco), save, and the answers (409, 423, 202, 422) -------------------------------------------------------------------
  {
    id: "file edit / the editor, no change yet",
    scenario: "design-file-edit",
    url: file(),
    async open({ page }) {
      await startEditing(page);
    },
  },
  {
    id: "file edit / the editor, a change made",
    scenario: "design-file-edit",
    url: file(),
    async open({ page }) {
      await changeEditor(page);
    },
  },
  {
    id: "file edit / the save dialog",
    scenario: "design-file-edit",
    url: file(),
    async open({ page }) {
      await openSaveDialog(page);
    },
  },
  {
    id: "file edit / saved",
    scenario: "design-file-edit",
    url: file(),
    async open({ page }) {
      await confirmSave(page);
      await expect(page.getByRole("button", { name: "Edit", exact: true })).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "file edit / someone else changed the file (409)",
    scenario: "design-file-edit-conflict",
    url: file(),
    async open({ page }) {
      await confirmSave(page);
      await expect(page.getByRole("dialog").or(page.getByRole("alertdialog")).filter({ hasText: /changed/i }).first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "file edit / the swimlane is locked (423)",
    scenario: "design-file-edit-locked",
    url: file(),
    async open({ page }) {
      await confirmSave(page);
      await expect(page.getByRole("dialog").or(page.getByRole("alertdialog")).filter({ hasText: /locked/i }).first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "file edit / sent for approval (202)",
    scenario: "design-file-edit-approval",
    url: file(),
    async open({ page }) {
      await confirmSave(page);
      await expect(page.getByRole("dialog").or(page.getByRole("alertdialog")).filter({ hasText: /approval/i }).first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "file edit / the content holds a secret (422)",
    scenario: "design-file-edit-secret",
    url: file(),
    async open({ page }) {
      await confirmSave(page);
      await expect(page.getByText(/secret/i).first()).toBeVisible({ timeout: 15_000 });
    },
  },

  // ---- Raise PR (1b to 1k), upload, delete (2a, 2b) -------------------------------------------------------------------------------
  { id: "file PR / step 1, the target", scenario: "design-file-pr", url: file(), open: (ctx) => raisePr(ctx, 1) },
  { id: "file PR / step 2, the title and description", scenario: "design-file-pr", url: file(), open: (ctx) => raisePr(ctx, 2) },
  {
    id: "file PR / the job runs and the PR is made",
    scenario: "design-file-pr",
    url: file(),
    async open(ctx) {
      await raisePr(ctx, 3);
      await expect(ctx.page.getByRole("dialog").getByText(/pull request|#41/i).first()).toBeVisible();
    },
  },
  {
    id: "file PR / a pull request is already open (409)",
    scenario: "design-file-pr-exists",
    url: file(),
    async open(ctx) {
      // The preview already says a PR is open for this change: after Continue the dialog shows that PR and offers no second one.
      await raisePr(ctx, 1);
      await ctx.page.getByRole("button", { name: "Continue" }).click();
      await expect(ctx.page.getByRole("dialog").getByText(/already open/i).first()).toBeVisible();
    },
  },
  {
    id: "file PR / the GitHub token has expired",
    scenario: "design-file-pr-token-expired",
    url: file(),
    async open(ctx) {
      await raisePr(ctx, 3);
      await expect(ctx.page.getByRole("dialog").getByText(/expired|token/i).first()).toBeVisible();
    },
  },
  {
    id: "file PR / the token cannot push to the tenant repository",
    scenario: "design-file-pr-no-access",
    url: file(),
    async open(ctx) {
      await raisePr(ctx, 3);
      await expect(ctx.page.getByRole("dialog").getByText(/access|push/i).first()).toBeVisible();
    },
  },
  {
    id: "file PR / the job fails",
    scenario: "design-file-pr-failed",
    url: file(),
    async open(ctx) {
      await raisePr(ctx, 3);
      await expect(ctx.page.getByRole("dialog").getByText(/fail|error|could not/i).first()).toBeVisible();
    },
  },
  {
    id: "file / the upload dialog",
    scenario: "design-file-core",
    url: file(),
    async open({ page }) {
      await page.getByRole("button", { name: "Upload", exact: true }).click();
      await expect(modal(page)).toBeVisible();
    },
  },
  {
    id: "file / the delete dialog (type the name to confirm)",
    scenario: "design-file-core",
    url: file(),
    async open({ page }) {
      await page.getByRole("button", { name: "Delete", exact: true }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },

  // ---- Draft (CG17; no handoff frame) ---------------------------------------------------------------------------------------------
  { id: "draft / ready to review and apply", scenario: "design-file-draft", url: "/drafts/7" },
  {
    id: "draft / applying it",
    scenario: "design-file-draft",
    url: "/drafts/7",
    async open({ page }) {
      await page.getByRole("button", { name: /^Apply/ }).first().click();
      await expect(modal(page).or(page.getByText(/applied/i)).first()).toBeVisible();
    },
  },
  { id: "draft / the file changed since (409)", scenario: "design-file-draft-conflict", url: "/drafts/7" },
  { id: "draft / already applied", scenario: "design-file-draft-applied", url: "/drafts/7" },
  {
    id: "draft / not found (404)",
    scenario: "design-file-draft",
    url: "/drafts/7",
    async before({ page }) {
      await fail(page, /\/api\/v1\/drafts\/7$/, 404, "Draft not found");
    },
  },
];
