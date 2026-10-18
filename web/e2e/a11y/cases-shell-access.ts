// The states of the shell, the public pages, the error screens and the access screens (S0, S1), one entry each. See cases.ts for
// how an entry is used. Frames are the handoff's ids (Shell 1a..; Access 1a..1t; States P01, G, L, E, R).
import { expect } from "@playwright/test";
import type { Case, Ctx } from "./helpers.ts";
import { SIDEBAR, fail, hold, modal } from "./helpers.ts";

const frame = (id: number, event: Record<string, unknown>): string => `id: ${id}\nevent: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;

const pasteToken = async ({ page }: Ctx, token = "github_pat_11A0000000_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"): Promise<void> => {
  await page.getByRole("textbox", { name: "2 · Paste the token" }).fill(token);
  await page.getByRole("button", { name: "Verify and save" }).click();
};

export const shellAccessCases: Case[] = [
  // ---- Public pages (Access Screen 1a to 1f; no chrome) -----------------------------------------------------------------
  { id: "access / sign in", scenario: "signed-out", url: "/signin", noChrome: true, h1: "Lanekeeper" },
  { id: "access / sign in with a return address", scenario: "signed-out", url: "/signin?next=%2Fswimlanes%2Fsitb", noChrome: true, h1: "Lanekeeper" },
  { id: "access / access requested", scenario: "design-access-public", url: "/access-requested", noChrome: true, h1: "Access requested" },
  { id: "access / access disabled", scenario: "design-access-public", url: "/access-disabled", noChrome: true, h1: "Your access is disabled" },

  // ---- Error screens (States, "404", "error", "403") ---------------------------------------------------------------------
  { id: "shell / not found", scenario: "design-shell-admin", url: "/404", noChrome: true, h1: "Page not found" },
  { id: "shell / not found, a path that is not a route", scenario: "design-shell-admin", url: "/swimlanes/sitb/nothing-here", noChrome: true },
  { id: "shell / error with a request id", scenario: "design-shell-admin", url: "/error?req=req-abc123", noChrome: true, h1: "Something went wrong" },
  { id: "shell / forbidden with a request id", scenario: "design-shell-admin", url: "/403?req=req-abc123", noChrome: true, h1: "You do not have access" },

  // ---- Chrome states (Shell 1a to 1l) ------------------------------------------------------------------------------------
  {
    id: "shell / swimlane switcher open",
    scenario: "design-shell-admin",
    url: "/swimlanes/sitb",
    async open({ page }) {
      await page.getByRole("button", { name: /^Swimlane: sitb/ }).click();
      await expect(page.getByRole("dialog").getByRole("option").first()).toBeVisible();
    },
  },
  {
    id: "shell / swimlane switcher, filtered to nothing",
    scenario: "design-shell-admin",
    url: "/swimlanes/sitb",
    async open({ page }) {
      await page.getByRole("button", { name: /^Swimlane: sitb/ }).click();
      // The list is a chunk of its own: its filter field takes the keyboard when it arrives, and keys typed before that are lost.
      await expect(page.getByRole("dialog").getByRole("combobox")).toBeFocused();
      await page.keyboard.type("zzzz");
      await expect(page.getByRole("dialog").getByRole("option")).toHaveCount(0);
    },
  },
  {
    id: "shell / account menu open",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await SIDEBAR(page).getByRole("button", { name: /Priya Shah/ }).click();
      await expect(page.getByRole("menu")).toBeVisible();
    },
  },
  {
    id: "shell / command palette open",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("ControlOrMeta+k");
      await expect(page.getByRole("dialog", { name: "Command palette" })).toBeVisible();
      await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("option").first()).toBeVisible();
    },
  },
  {
    id: "shell / command palette with a query",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("ControlOrMeta+k");
      // The palette is a chunk of its own: its field takes the keyboard when it arrives, and keys typed before that are lost.
      await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toBeFocused();
      await page.keyboard.type("sit");
      await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toHaveValue("sit");
      await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("option").first()).toBeVisible();
    },
  },
  {
    id: "shell / command palette, no result",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("ControlOrMeta+k");
      await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toBeFocused();
      await page.keyboard.type("qqqqzzzz");
      await expect(page.getByRole("dialog", { name: "Command palette" }).getByText(/No results for/)).toBeVisible();
    },
  },
  {
    id: "shell / keyboard shortcuts sheet",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("?");
      await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
    },
  },
  {
    id: "shell / keyboard shortcuts sheet for a viewer",
    scenario: "design-shell-viewer",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("?");
      await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
    },
  },
  {
    id: "shell / sidebar collapsed",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("ControlOrMeta+b");
      await expect(SIDEBAR(page)).toHaveAttribute("data-collapsed", "true");
    },
  },
  {
    // The hidden role=tooltip copy of an open tooltip sits in a portal under <body>: the known `region` finding of C1b.
    id: "shell / collapsed sidebar, a tooltip open on an icon",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page }) {
      await page.keyboard.press("ControlOrMeta+b");
      await expect(SIDEBAR(page)).toHaveAttribute("data-collapsed", "true");
      await SIDEBAR(page).getByRole("link", { name: /^Compare/ }).focus();
      await expect(page.getByRole("tooltip")).toBeVisible();
    },
  },
  {
    id: "shell / sidebar for a viewer",
    scenario: "design-shell-viewer",
    url: "/swimlanes",
  },
  {
    id: "shell / a toast: a file changed on NFS",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      harness.emit(frame(500, { type: "file_observed", swimlane: "sitb", path: "tx-infinity-api/tx-infinity-core-sit7.yml", hash: "a".repeat(64), source: "scan" }));
      await expect(page.getByText(/changed on NFS/)).toBeVisible();
    },
  },
  {
    id: "shell / live updates reconnecting",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      harness.refuseStreams(503, 50);
      harness.closeStreams();
      await expect(page.locator("[data-slot='live-indicator']")).not.toHaveAttribute("data-status", "live", { timeout: 15_000 });
    },
  },
  {
    id: "shell / live updates paused banner",
    scenario: "design-shell-admin",
    url: "/swimlanes",
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      harness.refuseStreams(503, 50);
      harness.closeStreams();
      await expect(page.getByText(/Live updates paused/i).first()).toBeVisible({ timeout: 40_000 });
    },
  },

  // ---- GitHub token onboarding (Access 1g to 1r) ---------------------------------------------------------------------------
  { id: "access / onboarding, the token step", scenario: "design-access-onboarding", url: "/access/onboarding" },
  {
    id: "access / onboarding, verifying (the answer is held)",
    scenario: "design-access-onboarding",
    url: "/access/onboarding",
    async before({ page }) {
      await hold(page, "**/api/v1/me/github-token");
    },
    async open(ctx) {
      await pasteToken(ctx);
      await expect(ctx.page.getByText(/Verifying/i).first()).toBeVisible();
    },
  },
  {
    id: "access / onboarding, saved",
    scenario: "design-access-onboarding",
    url: "/access/onboarding",
    async open(ctx) {
      await pasteToken(ctx);
      await expect(ctx.page.getByRole("status", { name: "Token verified and saved" })).toBeVisible();
    },
  },
  {
    id: "access / onboarding, a classic token (refused by policy)",
    scenario: "design-access-onboarding-classic",
    url: "/access/onboarding",
    async open(ctx) {
      await pasteToken(ctx, "ghp_classic0000000000000000000000000000");
      await expect(ctx.page.getByRole("alert").or(ctx.page.getByText(/classic/i)).first()).toBeVisible();
    },
  },
  {
    id: "access / onboarding, no access to the tenant repository",
    scenario: "design-access-onboarding-repo",
    url: "/access/onboarding",
    async open(ctx) {
      await pasteToken(ctx);
      await expect(ctx.page.getByText(/tenant/i).first()).toBeVisible();
    },
  },
  {
    id: "access / onboarding, the token belongs to another account",
    scenario: "design-access-onboarding-mismatch",
    url: "/access/onboarding",
    async open(ctx) {
      await pasteToken(ctx);
      await expect(ctx.page.getByText(/another|different|belongs/i).first()).toBeVisible();
    },
  },
  {
    id: "access / onboarding, the hub is unavailable",
    scenario: "design-access-onboarding",
    url: "/access/onboarding",
    async before({ page }) {
      await fail(page, "**/api/v1/me/github-token", 503, "The hub could not check the token");
    },
    async open(ctx) {
      await pasteToken(ctx);
      await expect(ctx.page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  { id: "access / onboarding as a viewer: permission state", scenario: "design-access-viewer", url: "/access/onboarding", h1: /needs the Editor role/ },
  {
    id: "access / request a role change dialog",
    scenario: "design-access-viewer",
    url: "/access/onboarding",
    h1: /needs the Editor role/,
    async open({ page }) {
      await page.getByRole("button", { name: "Request a role change" }).click();
      await expect(modal(page)).toBeVisible();
    },
  },
  {
    id: "access / request a role change, one already waits (409)",
    scenario: "design-access-viewer-open-request",
    url: "/access/onboarding",
    h1: /needs the Editor role/,
    async open({ page }) {
      await page.getByRole("button", { name: "Request a role change" }).click();
      await modal(page).getByRole("textbox", { name: "Why you need it" }).fill("I need to edit the sit7 files.");
      await modal(page).getByRole("button", { name: "Send request" }).click();
      await expect(modal(page).getByRole("alert")).toBeVisible();
    },
  },

  // ---- My GitHub token (Access 1s, 1t; States G) ---------------------------------------------------------------------------
  { id: "access / my GitHub token, ends in 5 days", scenario: "design-access-mytoken", url: "/me/github-token" },
  { id: "access / my GitHub token, no warning", scenario: "design-access-mytoken-ok", url: "/me/github-token" },
  { id: "access / my GitHub token, ended yesterday", scenario: "design-access-mytoken-expired", url: "/me/github-token" },
  { id: "access / my GitHub token, none stored", scenario: "design-access-mytoken-none", url: "/me/github-token" },
  {
    id: "access / my GitHub token, loading",
    scenario: "design-access-mytoken",
    url: "/me/github-token",
    async before({ page }) {
      await hold(page, "**/api/v1/me/github-token");
    },
  },
  {
    id: "access / my GitHub token, the hub fails",
    scenario: "design-access-mytoken",
    url: "/me/github-token",
    async before({ page }) {
      await fail(page, "**/api/v1/me/github-token");
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "access / replace token dialog",
    scenario: "design-access-mytoken",
    url: "/me/github-token",
    async open({ page }) {
      await page.getByRole("button", { name: "Replace token" }).last().click();
      await expect(page.getByRole("textbox", { name: "2 · Paste the token" })).toBeVisible();
    },
  },
  {
    id: "access / delete token dialog",
    scenario: "design-access-mytoken",
    url: "/me/github-token",
    async open({ page }) {
      await page.getByRole("button", { name: "Delete token" }).click();
      await expect(modal(page)).toBeVisible();
    },
  },

  // ---- Personal MCP tokens ----------------------------------------------------------------------------------------------------
  { id: "access / MCP tokens, list", scenario: "design-access-mcp-tokens", url: "/me/tokens" },
  {
    id: "access / MCP tokens, a new token shown once",
    scenario: "design-access-mcp-token-issued",
    url: "/me/tokens",
    async open({ page }) {
      await page.getByRole("textbox", { name: "Label" }).fill("laptop");
      await page.getByRole("button", { name: "Create token" }).click();
      await expect(page.getByRole("button", { name: /Copy token/ })).toBeVisible();
    },
  },
  { id: "access / MCP tokens, mode off (404): empty state", scenario: "design-access-mcp-off", url: "/me/tokens" },
];
