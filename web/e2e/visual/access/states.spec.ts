// The access screens that the design has no full frame for, and the States index rows of the access flow, in light, as the app's own baseline
// (`toHaveScreenshot`, generated in CI only) with a check in words that runs everywhere.
//  - P01 (a pending user) and the access-disabled screen are frames 1c to 1f of the Access board: tier 1 in pages.spec.ts.
//  - My GitHub token (frames 1s and 1t are tier 1 in token.spec.ts): the expired token (States G), no token, loading, an error, the replace and delete dialogs.
//  - Personal MCP tokens (no frame): the list, the one-time token, and the hub where the mode is off.
//  - The permission state of a Viewer on a screen that needs an Editor, and the request for a role.
import { expect } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { appOwned } from "./lib/matrix";
import { api, fail, hold } from "./lib/steps";

const TOKEN = api("/me/github-token");
const away = async (page: Page): Promise<void> => {
  await page.mouse.move(1439, 899);
};

appOwned([
  {
    name: "G my GitHub token, expired: Save and Raise PR do not work",
    scenario: "design-access-mytoken-expired",
    path: "/me/github-token",
    check: async (page) => {
      await expect(page.getByText(/token (has )?expired|expired/i).first()).toBeVisible();
      await expect(page.getByRole("button", { name: "Replace token" }).first()).toBeVisible();
    },
  },
  {
    name: "my GitHub token, no token stored: the empty state and the form to add one",
    scenario: "design-access-mytoken-none",
    path: "/me/github-token",
    check: async (page) => {
      await expect(page.getByLabel("2 · Paste the token")).toBeVisible();
    },
  },
  {
    name: "L my GitHub token, loading",
    scenario: "design-access-mytoken",
    path: "/me/github-token",
    loading: true,
    before: async (page) => {
      await hold(page, TOKEN);
    },
    check: async (page) => {
      await expect(page.locator("[data-slot='skeleton']").first()).toBeVisible();
    },
  },
  {
    name: "R my GitHub token, the token cannot be read",
    scenario: "design-access-mytoken",
    path: "/me/github-token",
    before: async (page) => {
      await fail(page, TOKEN);
    },
    check: async (page) => {
      await expect(page.getByText(/couldn.t load/i).first()).toBeVisible();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
    },
  },
  {
    name: "my GitHub token, the delete dialog",
    scenario: "design-access-mytoken-ok",
    path: "/me/github-token",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Delete token" }).click();
      await page.getByRole("alertdialog").waitFor();
      await away(page);
    },
    check: async (page) => {
      await expect(page.getByRole("alertdialog")).toContainText(/delete/i);
      await expect(page.getByRole("button", { name: "Cancel" })).toBeFocused();
    },
  },
  {
    name: "my GitHub token, replacing it: the form opens in the page",
    scenario: "design-access-mytoken-ok",
    path: "/me/github-token",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Replace token" }).first().click();
      await page.getByLabel("2 · Paste the token").waitFor();
      await away(page);
    },
    check: async (page) => {
      await expect(page.getByRole("button", { name: "Replace token" }).first()).toHaveAttribute("aria-expanded", "true");
      await expect(page.getByLabel("2 · Paste the token")).toBeEmpty();
    },
  },
  {
    name: "personal MCP tokens: a token that works, one that ended and one that was revoked",
    scenario: "design-access-mcp-tokens",
    path: "/me/tokens",
    check: async (page) => {
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expect(page.getByText("Revoked").first()).toBeVisible();
    },
  },
  {
    name: "personal MCP tokens, the mode is off on this hub: the empty state",
    scenario: "design-access-mcp-off",
    path: "/me/tokens",
    check: async (page) => {
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    },
  },
  {
    name: "P a Viewer where an Editor is needed: the permission state, and a way to ask for the role",
    scenario: "design-access-viewer",
    path: "/access/onboarding",
    check: async (page) => {
      await expect(page.getByRole("button", { name: /request a role change/i })).toBeVisible();
      await expect(page.getByRole("link", { name: /Access|Onboarding/i })).toHaveCount(0);
    },
  },
]);
