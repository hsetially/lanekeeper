// The states of the Admin area (S8): users and requests, swimlanes and agents (manage panel, join token), the rule sections with their
// add/edit/delete dialogs, notifications (a webhook field), MCP tokens, a section that does not exist, and the permission state.
// The four sections that T5 added (sentinels, OS Login, severity rules, retention) are in cases-t5-admin.ts.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { fail, hold, modal } from "./helpers.ts";

const section = (id: string, scenario: string, path: string): Case => ({ id, scenario, url: path });
const click = (name: string | RegExp, exact = false) => async ({ page }: { page: Page }) => {
  await page.getByRole("button", { name, exact }).first().click();
  await expect(modal(page).first()).toBeVisible();
};

export const adminCases: Case[] = [
  // ---- Users and access requests (frames 1a to 1e; States L11, E11, P02, C03) ---------------------------------------------------------
  section("admin users / people and a pending request", "design-admin-users", "/admin/users"),
  section("admin users / no request waits", "design-admin-users-empty", "/admin/users"),
  section("admin users / a viewer: needs the Admin role", "design-admin-viewer", "/admin/users"),
  {
    id: "admin users / loading (the people are held)",
    scenario: "design-admin-users",
    url: "/admin/users",
    async before({ page }) {
      await hold(page, /\/api\/v1\/admin\/users/);
    },
  },
  {
    id: "admin users / the people cannot be read (500)",
    scenario: "design-admin-users",
    url: "/admin/users",
    async before({ page }) {
      await fail(page, /\/api\/v1\/admin\/users/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "admin users / the role filter",
    scenario: "design-admin-users",
    url: "/admin/users",
    async open({ page }) {
      await page.getByRole("combobox", { name: "Role", exact: true }).selectOption({ index: 1 });
    },
  },
  {
    id: "admin users / disable a person (a consequence dialog)",
    scenario: "design-admin-users",
    url: "/admin/users",
    open: click("Disable Amir Haddad"),
  },
  {
    id: "admin users / change a role",
    scenario: "design-admin-users",
    url: "/admin/users",
    async open({ page }) {
      await page.getByRole("combobox", { name: "Role for Amir Haddad" }).selectOption({ index: 1 });
      await expect(modal(page)).toBeVisible();
    },
  },
  {
    id: "admin users / the role changed meanwhile (409)",
    scenario: "design-admin-users-conflict",
    url: "/admin/users",
    async open({ page }) {
      await page.getByRole("combobox", { name: "Role for Amir Haddad" }).selectOption({ index: 1 });
      await modal(page).getByRole("button").filter({ hasNotText: /Cancel|Close/ }).last().click();
      await expect(page.getByText(/changed|Keep editing|Reload/i).first()).toBeVisible();
    },
  },

  // ---- Swimlanes and agents (frames 1f, 1g, 1j) ----------------------------------------------------------------------------------------
  section("admin swimlanes / swimlanes and agents", "design-admin-swimlanes", "/admin/swimlanes"),
  section("admin swimlanes / none registered", "design-admin-swimlanes-empty", "/admin/swimlanes"),
  {
    id: "admin swimlanes / manage a swimlane",
    scenario: "design-admin-swimlanes",
    url: "/admin/swimlanes",
    async open({ page }) {
      await page.getByRole("button", { name: "Manage sitb" }).click();
      await expect(page.getByRole("heading", { name: "Settings for sitb" })).toBeVisible();
    },
  },
  {
    id: "admin swimlanes / the join token of a swimlane",
    scenario: "design-admin-swimlanes",
    url: "/admin/swimlanes",
    async open({ page }) {
      await page.getByRole("button", { name: "Manage presitc" }).click();
      await page.getByRole("button", { name: "Create join token" }).click();
      await expect(modal(page).or(page.getByText(/join token/i).nth(1)).first()).toBeVisible();
    },
  },

  // ---- The rule sections (frames 1k to 1ad) -----------------------------------------------------------------------------------------------
  section("admin paths / the rules", "design-admin-paths", "/admin/paths"),
  section("admin paths / no rule", "design-admin-paths-empty", "/admin/paths"),
  { id: "admin paths / add a rule", scenario: "design-admin-paths", url: "/admin/paths", open: click("Add rule") },
  { id: "admin paths / edit a rule", scenario: "design-admin-paths", url: "/admin/paths", open: click("Edit rule default") },
  {
    id: "admin paths / delete a rule",
    scenario: "design-admin-paths",
    url: "/admin/paths",
    async open({ page }) {
      await page.getByRole("button", { name: "Delete rule presit-legacy" }).click();
      await expect(modal(page)).toBeVisible();
    },
  },
  section("admin merge / the rules", "design-admin-merge", "/admin/merge"),
  section("admin merge / no rule", "design-admin-merge-empty", "/admin/merge"),
  { id: "admin merge / add a rule", scenario: "design-admin-merge", url: "/admin/merge", open: click("Add rule") },
  section("admin tiers / the patterns", "design-admin-tiers", "/admin/tiers"),
  section("admin tiers / no pattern", "design-admin-tiers-empty", "/admin/tiers"),
  { id: "admin tiers / add a pattern", scenario: "design-admin-tiers", url: "/admin/tiers", open: click("Add pattern") },
  section("admin folders / the mappings", "design-admin-folders", "/admin/folders"),
  section("admin folders / no mapping", "design-admin-folders-empty", "/admin/folders"),
  { id: "admin folders / add a mapping", scenario: "design-admin-folders", url: "/admin/folders", open: click("Add mapping") },

  // ---- Notifications, MCP tokens, a section that does not exist ---------------------------------------------------------------------------
  section("admin notify / the channels", "design-admin-notify", "/admin/notify"),
  section("admin notify / no channel", "design-admin-notify-empty", "/admin/notify"),
  { id: "admin notify / add a channel (the webhook field)", scenario: "design-admin-notify", url: "/admin/notify", open: click("Add channel") },
  { id: "admin notify / edit a channel", scenario: "design-admin-notify", url: "/admin/notify", open: click("Edit channel certificates") },
  section("admin mcp / the admin's own tokens", "design-admin-mcp", "/admin/mcp"),
  section("admin mcp / none issued", "design-admin-mcp-empty", "/admin/mcp"),
  section("admin mcp / the mode is off", "design-admin-mcp-off", "/admin/mcp"),
  { id: "admin / a section that does not exist", scenario: "design-admin-users", url: "/admin/nothing", noChrome: true },
];
