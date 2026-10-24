// The states of the Swimlanes screen and the Swimlane detail (S2): list (six, forty, empty, viewer), its menus and selection,
// detail tabs, agent offline, syncing, restart dialogs (applied, 202, 423), and the adoption screen (a stub until S6b/CG6).
import { expect } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { fail, hold, modal } from "./helpers.ts";

const LIST = ["design-swimlanes-six", "/swimlanes"] as const;
const frame = (id: number, event: Record<string, unknown>): string => `id: ${id}\nevent: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;
const SITB = (tab = "") => `/swimlanes/sitb${tab}`;

export const swimlanesCases: Case[] = [
  // ---- Swimlanes list (boards 1a to 1j; States L01, L02, E01, E02, R01, R02, P04, O01) ----------------------------------------
  { id: "swimlanes / six swimlanes", scenario: LIST[0], url: LIST[1] },
  { id: "swimlanes / six swimlanes, a viewer", scenario: "design-swimlanes-six-viewer", url: "/swimlanes" },
  { id: "swimlanes / forty swimlanes", scenario: "design-swimlanes-forty", url: "/swimlanes" },
  { id: "swimlanes / none registered, an admin", scenario: "design-swimlanes-empty", url: "/swimlanes" },
  { id: "swimlanes / none registered, a viewer", scenario: "design-swimlanes-empty-viewer", url: "/swimlanes" },
  {
    id: "swimlanes / loading (the list is held)",
    scenario: LIST[0],
    url: "/swimlanes",
    async before({ page }) {
      await hold(page, "**/api/v1/swimlanes");
    },
    async open({ page }) {
      await expect(page.getByText("Loading swimlanes…").first()).toBeVisible();
    },
  },
  {
    id: "swimlanes / the list fails (500)",
    scenario: LIST[0],
    url: "/swimlanes",
    async before({ page }) {
      await fail(page, "**/api/v1/swimlanes");
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    // Stale (the data may be out of date): the list is on screen, a live event asks for it again, and the hub cannot answer.
    id: "swimlanes / stale: the list could not be refreshed",
    scenario: LIST[0],
    url: "/swimlanes",
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      await fail(page, /\/api\/v1\/swimlanes(\?.*)?$/);
      harness.emit(frame(700, { type: "swimlane_changed", swimlane: "sitb" }));
      await expect(page.getByText("Couldn't refresh").first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "swimlanes / the list is filtered to nothing",
    scenario: LIST[0],
    url: "/swimlanes",
    async open({ page }) {
      await page.getByRole("searchbox", { name: "Filter swimlanes" }).fill("zzzzzz");
      await expect(page.getByText("No swimlane matches")).toBeVisible();
    },
  },
  ...["Tier", "Agent status", "Columns"].map(
    (name): Case => ({
      id: `swimlanes / the ${name} menu open`,
      scenario: LIST[0],
      url: "/swimlanes",
      async open({ page }) {
        await page.getByRole("button", { name, exact: true }).click();
        await expect(page.getByRole("menu").or(page.getByRole("dialog")).or(page.getByRole("listbox")).first()).toBeVisible();
      },
    }),
  ),
  {
    id: "swimlanes / the Has drift filter on",
    scenario: LIST[0],
    url: "/swimlanes",
    async open({ page }) {
      await page.getByRole("button", { name: "Has drift", exact: true }).click();
      await expect(page.getByRole("button", { name: "Has drift", exact: true })).toHaveAttribute("aria-pressed", "true");
    },
  },
  {
    id: "swimlanes / two rows selected: Compare selected is on",
    scenario: LIST[0],
    url: "/swimlanes",
    async open({ page }) {
      await page.getByRole("checkbox", { name: "Select sit", exact: true }).check();
      await page.getByRole("checkbox", { name: "Select sitb", exact: true }).check();
      await expect(page.getByRole("link", { name: "Compare selected" })).toBeVisible();
    },
  },

  // ---- Swimlane detail (boards 1a to 1n) -----------------------------------------------------------------------------------------
  { id: "swimlane detail / Services tab", scenario: LIST[0], url: SITB("/services") },
  { id: "swimlane detail / Drift tab", scenario: LIST[0], url: SITB("/drift") },
  { id: "swimlane detail / Findings tab", scenario: LIST[0], url: SITB("/findings") },
  { id: "swimlane detail / Activity tab (admin)", scenario: LIST[0], url: SITB("/activity") },
  { id: "swimlane detail / Audit (admin)", scenario: LIST[0], url: SITB("/audit") },
  { id: "swimlane detail / Activity needs the Admin role (viewer)", scenario: "design-swimlanes-six-viewer", url: SITB("/activity"), h1: /needs the Admin role/ },
  { id: "swimlane detail / Services tab, a viewer", scenario: "design-swimlanes-six-viewer", url: SITB("/services") },
  { id: "swimlane detail / agent offline", scenario: "design-swimlanes-agent-off", url: SITB("/services") },
  { id: "swimlane detail / sync in progress", scenario: "design-swimlanes-syncing", url: SITB("/services") },
  { id: "swimlane detail / not adopted (presitc)", scenario: LIST[0], url: "/swimlanes/presitc/services" },
  { id: "swimlane detail / no files, no services (a swimlane the hub knows nothing of)", scenario: LIST[0], url: "/swimlanes/zzz" },
  {
    id: "swimlane detail / the Compare with menu",
    scenario: LIST[0],
    url: SITB("/services"),
    async open({ page }) {
      await page.getByRole("button", { name: "Compare with…" }).click();
      await expect(page.getByRole("menu")).toBeVisible();
    },
  },
  {
    id: "swimlane detail / the drift state filter menu",
    scenario: LIST[0],
    url: SITB("/drift"),
    async open({ page }) {
      await page.getByRole("button", { name: "Drift state" }).click();
      await expect(page.getByRole("menu").or(page.getByRole("dialog")).first()).toBeVisible();
    },
  },
  {
    id: "swimlane detail / services loading (the services are held)",
    scenario: LIST[0],
    url: SITB("/services"),
    async before({ page }) {
      await hold(page, /\/api\/v1\/swimlanes\/sitb\/services/);
    },
  },
  {
    id: "swimlane detail / the detail fails (500)",
    scenario: LIST[0],
    url: SITB("/services"),
    async before({ page }) {
      await fail(page, /\/api\/v1\/swimlanes\/sitb$/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "swimlane detail / collapsed file tree filter: no match",
    scenario: LIST[0],
    url: SITB("/services"),
    async open({ page }) {
      await page.getByRole("searchbox", { name: "Filter files" }).fill("zzzzzz");
    },
  },

  // ---- Restart a service (Consequence dialog; States A02, K01) ---------------------------------------------------------
  {
    id: "restart / the dialog",
    scenario: LIST[0],
    url: SITB("/services"),
    async open({ page }) {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await expect(page.getByRole("alertdialog", { name: /Restart tx-infinity-core in sitb\?/ })).toBeVisible();
    },
  },
  {
    id: "restart / applied",
    scenario: LIST[0],
    url: SITB("/services"),
    async open({ page }) {
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await modal(page).getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await expect(page.getByText(/accepted|restart/i).first()).toBeVisible();
    },
  },
  {
    id: "restart / sent for approval (202)",
    scenario: "design-swimlanes-restart-approval",
    url: SITB("/services"),
    async open({ page }) {
      await page.getByRole("button", { name: /^Restart / }).first().click();
      await modal(page).getByRole("button", { name: /^Restart / }).click();
      await expect(page.getByText(/Admin approves|approval/i).first()).toBeVisible();
    },
  },
  {
    id: "restart / locked (423)",
    scenario: "design-swimlanes-restart-locked",
    url: SITB("/services"),
    async open({ page }) {
      await page.getByRole("button", { name: /^Restart / }).first().click();
      await modal(page).getByRole("button", { name: /^Restart / }).click();
      await expect(page.getByText(/locked|Another operation/i).first()).toBeVisible();
    },
  },

  // ---- Adopt (stub until the adoption contract lands) ---------------------------------------------------------------------
  { id: "adopt / the screen that is built in a later task", scenario: LIST[0], url: "/swimlanes/presitc/adopt" },
];
