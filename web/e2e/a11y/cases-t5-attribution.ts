// The states of the attribution and severity pieces of T5 (slice A): the chip that says who changed a file, how Lanekeeper knows and how
// sure it is, with the evidence popover it opens, in every place the app draws it (the rows of a file's History, the "Changed by" of the
// file header, a swimlane's Drift tab and Activity tab, the Audit table and the drawer of an event), and the one severity badge and its
// "that level and above" filter. The data is the attribution scenarios: every source of D73, every level of confidence, every severity.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { modal } from "./helpers.ts";
import { BY_SOURCE, hostileVersions } from "../../src/mocks/design/attribution/data.ts";
import { CORE_FILE, itemsOf, patchJson } from "./t5.ts";

const openRow = async (page: Page, grid: string, index: number): Promise<void> => {
  await page.getByRole("grid", { name: grid }).getByRole("row").nth(index).click();
};

/** The History panel of the file page, with the chip of the n-th version pressed: the evidence popover is open. */
async function historyPopover(page: Page, chip?: RegExp): Promise<void> {
  await page.getByRole("button", { name: "History", exact: true }).click();
  const panel = page.getByRole("complementary", { name: "History" });
  await expect(panel).toBeVisible();
  // The chip is the button inside [data-slot=attribution]; the row has other buttons (Copy hash, Revert).
  const chips = panel.locator("[data-slot='attribution'] button");
  await (chip === undefined ? chips : chips.filter({ hasText: chip })).first().click();
  await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
}

const SOURCES = Object.values(BY_SOURCE);
const LEVELS = ["critical", "high", "medium", "low"] as const;

/** A swimlane's drift list where every file has an attribution and a severity: the chip and the badge on each row of the Drift tab. */
async function ratedDrift(page: Page, over: Record<string, unknown> = {}): Promise<void> {
  await patchJson(page, /\/api\/v1\/swimlanes\/sitb\/drift(\?.*)?$/, (list) => ({
    ...list,
    ...over,
    items: itemsOf(list).map((file, i) => ({ ...file, attribution: SOURCES[i % SOURCES.length], severity: LEVELS[i % LEVELS.length] })),
  }));
}

export const t5AttributionCases: Case[] = [
  // ---- The History of a file: the chip in a row, the head of the file, the popover ----------------------------------------------------------
  {
    id: "attribution / the file header says who changed it (Changed by)",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await expect(page.getByText("Changed by")).toBeVisible();
    },
  },
  {
    id: "attribution / history rows: every source, level and severity",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await page.getByRole("button", { name: "History", exact: true }).click();
      await expect(page.getByRole("list", { name: "Versions of this file" })).toBeVisible();
    },
  },
  {
    id: "attribution / history: the evidence of an OS login (popover open)",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await historyPopover(page, /^dana\.okoye/);
    },
  },
  {
    id: "attribution / history: unknown author (popover open)",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await historyPopover(page, /^Unknown author/);
    },
  },
  {
    id: "attribution / history: a user the Admin list does not know (popover open)",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await historyPopover(page, /^Lanekeeper user cccccccc/);
    },
  },
  {
    id: "attribution / history: a sync job (popover open)",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await historyPopover(page, /^Sync job /);
    },
  },
  {
    id: "attribution / history: the head of the file (popover open)",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await page.getByRole("button", { name: /^dana\.okoye/ }).first().click();
      await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
    },
  },
  {
    id: "attribution / history: hostile text in logins, jobs and executables",
    scenario: "design-attribution-hostile",
    url: CORE_FILE,
    async open({ page }) {
      await page.getByRole("button", { name: "History", exact: true }).click();
      await expect(page.getByRole("list", { name: "Versions of this file" })).toBeVisible();
    },
  },
  {
    id: "attribution / history: hostile text in the evidence popover",
    scenario: "design-attribution-hostile",
    url: CORE_FILE,
    async open({ page }) {
      await historyPopover(page);
    },
  },

  {
    // Reverting names the version, who changed it and how sure the server is, in words (the dialog reads the same attribution as the chip).
    id: "attribution / history: the Revert dialog says who changed that version",
    scenario: "design-attribution-history",
    url: CORE_FILE,
    async open({ page }) {
      await page.getByRole("button", { name: "History", exact: true }).click();
      await page.getByRole("complementary", { name: "History" }).getByRole("button", { name: /^Revert to / }).nth(4).click();
      await expect(page.getByRole("alertdialog").or(page.getByRole("dialog", { name: /Revert/ })).first()).toBeVisible();
    },
  },

  // ---- A swimlane: the Drift tab (chip, severity badge, the "that level and above" filter) and the Activity tab -----------------------------
  {
    id: "attribution / the Drift tab: a chip and a severity badge on every row",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/drift",
    async before({ page }) {
      await ratedDrift(page);
    },
    async open({ page }) {
      await expect(page.locator("[data-slot='attribution']").first()).toBeVisible();
      await expect(page.locator("[data-slot='severity-badge']").first()).toBeVisible();
    },
  },
  {
    // What an NFS VM can send as a login (a right-to-left override, markup, 256 characters): the chip wraps or truncates, and the row keeps its height.
    id: "attribution / the Drift tab: hostile and very long attribution text",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/drift",
    async before({ page }) {
      const hostile = hostileVersions().map((v) => v.attribution);
      await patchJson(page, /\/api\/v1\/swimlanes\/sitb\/drift(\?.*)?$/, (list) => ({
        ...list,
        items: itemsOf(list).map((file, i) => ({ ...file, attribution: hostile[i % hostile.length], severity: LEVELS[i % LEVELS.length] })),
      }));
    },
    async open({ page }) {
      await expect(page.locator("[data-slot='attribution']").first()).toBeVisible();
    },
  },
  {
    id: "attribution / the Drift tab: the evidence of a row (popover open)",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/drift",
    async before({ page }) {
      await ratedDrift(page);
    },
    async open({ page }) {
      await page.getByRole("tabpanel").locator("[data-slot='attribution'] button").first().click();
      await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
    },
  },
  {
    id: "attribution / the Drift tab: severity filter, High and above (applies to the rows loaded)",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/drift",
    async before({ page }) {
      await ratedDrift(page);
    },
    async open({ page }) {
      await page.getByRole("combobox", { name: "Severity" }).selectOption("high");
      await expect(page.getByRole("combobox", { name: "Severity" })).toHaveValue("high");
    },
  },
  {
    id: "attribution / the Drift tab: severity filter, more rows on the server than loaded",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/drift",
    async before({ page }) {
      // The server has more: the first page names a next one, and that one does not come (it is still on its way).
      await ratedDrift(page, { next: "more" });
      await page.route(/\/api\/v1\/swimlanes\/sitb\/drift\?.*cursor=more/, () => new Promise(() => undefined));
    },
    async open({ page }) {
      await page.getByRole("combobox", { name: "Severity" }).selectOption("critical");
      await expect(page.getByText(/files loaded so far/)).toBeVisible();
    },
  },
  {
    id: "attribution / the Drift tab: the filter keeps nothing",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/drift",
    async open({ page }) {
      await page.getByRole("combobox", { name: "Severity" }).selectOption("critical");
    },
  },
  { id: "attribution / the Activity tab: who made each event (chips)", scenario: "design-attribution-audit", url: "/swimlanes/sitb/activity" },
  {
    id: "attribution / the Activity tab: the evidence of an event (popover open)",
    scenario: "design-attribution-audit",
    url: "/swimlanes/sitb/activity",
    async open({ page }) {
      await page.getByRole("tabpanel").locator("[data-slot='attribution'] button").nth(2).click();
      await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
    },
  },

  // ---- The Audit log: the source and level of each event, and the drawer's evidence -------------------------------------------------------
  { id: "attribution / the Audit table: a source and a level for every event", scenario: "design-attribution-audit", url: "/audit" },
  {
    id: "attribution / the Audit drawer: an OS login with evidence",
    scenario: "design-attribution-audit",
    url: "/audit",
    async open({ page }) {
      await openRow(page, "Audit events", 3);
      await expect(page.getByRole("dialog")).toBeVisible();
      await expect(page.getByRole("dialog").getByText("Login identity")).toBeVisible();
    },
  },
  {
    id: "attribution / the Audit drawer: unknown author",
    scenario: "design-attribution-audit",
    url: "/audit",
    async open({ page }) {
      await openRow(page, "Audit events", 8);
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  {
    id: "attribution / the Audit drawer: an event with no attribution",
    scenario: "design-attribution-audit",
    url: "/audit",
    async open({ page }) {
      await openRow(page, "Audit events", 10);
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  {
    id: "attribution / the Audit drawer: hostile text in the evidence",
    scenario: "design-attribution-audit",
    url: "/audit",
    async open({ page }) {
      await openRow(page, "Audit events", 12);
      await expect(page.getByRole("dialog")).toBeVisible();
      await expect(modal(page)).toBeVisible();
    },
  },

  // ---- The one severity badge ------------------------------------------------------------------------------------------------------------
  {
    id: "severity / the Findings tab of a swimlane and the badge on each row",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/findings",
    async open({ page }) {
      await expect(page.locator("[data-slot='severity-badge']").first()).toBeVisible();
    },
  },
  {
    id: "severity / all four levels on the Findings page",
    scenario: "design-findings-all-kinds",
    url: "/findings",
    async open({ page }) {
      await expect(page.locator("[data-slot='severity-badge'][data-severity='critical']").first()).toBeVisible();
    },
  },
];
