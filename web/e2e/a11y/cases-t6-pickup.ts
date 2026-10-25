// The states of the pickup chips of T6 (slice A): when the services that read a file get its new version, in the four states of D85
// (Live, Live by a time (the client cache), Needs notify or restart, Needs config-server restart), drawn in the Pickup column of the
// swimlane list and its legend, in the chips under a swimlane's name, in a service's row on the Services tab (the chip, the time
// and the tooltip), on the rows of the file tree and on the folders' rollups, at the end of the title row of a file, and in the
// list of a draft's services. The data is the scenarios `design-pickup-*` (the services and files in every state, no state stated
// by the hub, nothing waiting, and text the page must not trust) and, for the file page, `design-notify-file-*`.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { API, CORE_FILE, SERVICES_URL, chips, itemsOf, patchJson, showTip } from "./t6.ts";

const STATES = "design-pickup-states";
const UNSTATED = "design-pickup-unstated";
const QUIET = "design-pickup-quiet";
const HOSTILE = "design-pickup-hostile";

/** The chips of the file tree (not the header's, not the Services tab's). */
const treeChips = (page: Page) => page.getByRole("tree").locator("[data-slot='pickup-chip']");

/** Open a folder of the file tree by its chevron and wait until the tree has more rows than before. */
async function openFolder(page: Page, name: string): Promise<void> {
  const rows = page.getByRole("tree").getByRole("treeitem");
  const before = await rows.count();
  await page.getByRole("button", { name: `Open ${name}` }).click();
  await expect.poll(() => rows.count(), { timeout: 10_000 }).toBeGreaterThan(before);
}

/** One effect of a draft: a service and the state it will be in. */
const SERVICE = (name: string, pickup: string): [{ namespace: string; name: string }, string] => [{ namespace: "csp", name }, pickup];

export const t6PickupCases: Case[] = [
  // ---- The swimlane list: the Pickup column and the legend ----------------------------------------------------------------------------
  {
    id: "pickup / swimlane list: the Pickup column (a chip for each state, with its count) and the legend",
    scenario: STATES,
    url: "/swimlanes",
    async open({ page }) {
      await expect(page.getByRole("img", { name: "1 Needs config-server restart" }).first()).toBeVisible();
      await expect(page.getByRole("img", { name: "2 Needs notify or restart" }).first()).toBeVisible();
      await expect(page.getByText("Needs config-server restart", { exact: true }).first()).toBeVisible();
    },
  },
  {
    id: "pickup / swimlane list: a chip's tooltip open (the sentence says when the change takes effect)",
    scenario: STATES,
    url: "/swimlanes",
    async open({ page }) {
      await showTip(page, page.getByRole("img", { name: "1 Needs config-server restart" }).first(), /waits for the config-server to restart/);
    },
  },
  {
    id: "pickup / swimlane list: nothing is waiting (the zero, and no chip)",
    scenario: QUIET,
    url: "/swimlanes",
    async open({ page }) {
      await expect(page.locator("[data-slot='pickup-counts'][data-empty='true']").first()).toBeVisible();
    },
  },
  { id: "pickup / swimlane list: counts the hub gave for text the page must not trust", scenario: HOSTILE, url: "/swimlanes" },

  // ---- The head of a swimlane and the Services tab --------------------------------------------------------------------------------------
  {
    id: "pickup / swimlane detail: a chip for each state under the name, and a service in every state on the Services tab",
    scenario: STATES,
    url: SERVICES_URL,
    async open({ page }) {
      await expect(page.getByRole("table", { name: "Services" }).getByRole("row")).toHaveCount(8);
      await expect(page.locator("[data-slot='pickup-chip'][data-pickup='live_within_ttl']").first()).toContainText("Live by 09:15 UTC");
      await expect(page.locator("[data-slot='pickup-chip'][data-pickup='live_within_ttl']").nth(1)).toContainText("(time passed)");
    },
  },
  {
    id: "pickup / services tab: the tooltip of a chip that waits on the client cache (its lifetime and the notifications flag)",
    scenario: STATES,
    url: SERVICES_URL,
    async open({ page }) {
      await showTip(page, page.getByRole("table", { name: "Services" }).locator("[data-slot='pickup-chip'][data-pickup='live_within_ttl']").first(), /Client cache lifetime: 20 min/);
    },
  },
  {
    id: "pickup / swimlane detail: the tooltip of a chip under the name",
    scenario: STATES,
    url: SERVICES_URL,
    async open({ page }) {
      await showTip(page, chips(page).first(), /config-server/);
    },
  },
  {
    id: "pickup / services tab: the hub states nothing about files (the services still say what they wait on)",
    scenario: UNSTATED,
    url: SERVICES_URL,
    async open({ page }) {
      await expect(page.getByRole("table", { name: "Services" }).locator("[data-slot='pickup-chip']").first()).toBeVisible();
      await expect(treeChips(page)).toHaveCount(0);
    },
  },
  { id: "pickup / services tab: nothing is waiting (every service is Live)", scenario: QUIET, url: SERVICES_URL },
  { id: "pickup / services tab: files and times the page must not trust (hostile)", scenario: HOSTILE, url: SERVICES_URL },

  // ---- The file tree: the mark of a file and the rollup of a folder ---------------------------------------------------------------------
  {
    id: "pickup / file tree: marks on files, rollups on folders (a count of the files loaded), a folder opened",
    scenario: STATES,
    url: SERVICES_URL,
    async open({ page }) {
      await openFolder(page, "tx-infinity-api");
      await expect(treeChips(page).first()).toBeVisible();
      for (const chip of await treeChips(page).all()) await expect(chip).toHaveAttribute("role", "img");
    },
  },
  {
    id: "pickup / file tree: the new folder opened (its file says Needs config-server restart)",
    scenario: STATES,
    url: SERVICES_URL,
    async open({ page }) {
      await openFolder(page, "new-folder");
      await expect(page.getByRole("tree").getByRole("img", { name: "Needs config-server restart" })).toHaveCount(3);
    },
  },
  {
    id: "pickup / file tree: the tooltip of the mark of a file",
    scenario: STATES,
    url: SERVICES_URL,
    async open({ page }) {
      await showTip(page, page.getByRole("tree").getByRole("img", { name: "Needs config-server restart" }).last(), /does not serve it until it restarts/);
    },
  },
  {
    id: "pickup / file tree: the hub states no pickup, so no file or folder is marked (never Live)",
    scenario: UNSTATED,
    url: SERVICES_URL,
    async open({ page }) {
      await openFolder(page, "tx-infinity-api");
      await expect(treeChips(page)).toHaveCount(0);
    },
  },
  {
    id: "pickup / file tree: names the page must not trust, marked",
    scenario: HOSTILE,
    url: SERVICES_URL,
    async open({ page }) {
      await openFolder(page, "tx-infinity-api");
      await expect(treeChips(page).first()).toBeVisible();
    },
  },

  // ---- The head of a file -------------------------------------------------------------------------------------------------------------------
  {
    id: "pickup / file header: Needs notify or restart, at the end of the title row",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await expect(page.locator("[data-slot='pickup-chip'][data-pickup='needs_notify_or_restart']")).toHaveCount(1);
    },
  },
  {
    id: "pickup / file header: Needs config-server restart",
    scenario: "design-notify-file-config-server",
    url: CORE_FILE,
    async open({ page }) {
      await expect(page.locator("[data-slot='pickup-chip'][data-pickup='needs_config_server_restart']")).toHaveCount(1);
    },
  },
  {
    id: "pickup / file header: Live",
    scenario: "design-notify-file-live",
    url: CORE_FILE,
    async open({ page }) {
      await expect(page.locator("[data-slot='pickup-chip'][data-pickup='live']")).toHaveCount(1);
    },
  },
  {
    id: "pickup / file header: the hub states no pickup (no chip, never Live)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, API.tree, (list) => ({ ...list, items: itemsOf(list).map((entry) => ({ ...entry, pickup_state: null })) }));
    },
    async open({ page }) {
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expect(page.locator("[data-slot='pickup-chip']")).toHaveCount(0);
    },
  },
  {
    id: "pickup / file header: the tooltip of the chip",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await showTip(page, chips(page).first(), /A property file: services take the new version after they are notified or restarted/);
    },
  },

  // ---- The services of a draft -------------------------------------------------------------------------------------------------------------
  {
    id: "pickup / draft: the services that pick the change up, in every state (and one the app does not know)",
    scenario: "design-file-draft",
    url: "/drafts/7",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/drafts\/7$/, (draft) => ({
        ...draft,
        effects: [
          SERVICE("tx-infinity-core", "needs_notify_or_restart"),
          SERVICE("config-server", "needs_config_server_restart"),
          SERVICE("remote-itm-teller", "live_within_ttl"),
          SERVICE("tx-infinity-gateway", "live"),
          SERVICE("holds-service", "a_state_of_the_future"),
        ],
      }));
    },
    async open({ page }) {
      await expect(page.getByRole("list", { name: "Services" }).getByRole("listitem")).toHaveCount(5);
      await expect(page.getByText("Pickup unknown")).toBeVisible();
      await expect(page.getByText("a_state_of_the_future")).toHaveCount(0);
    },
  },
  {
    id: "pickup / draft: more services than the page lists (and names the page must not trust)",
    scenario: "design-file-draft",
    url: "/drafts/7",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/drafts\/7$/, (draft) => ({
        ...draft,
        effects: Array.from({ length: 56 }, (_, i) => SERVICE(i % 7 === 0 ? `inv‮oice-${i}` : `service-${i}-with-a-long-name-that-goes-on-and-on-and-on-and-on-and-on-and-on-and-on`, ["needs_notify_or_restart", "live_within_ttl", "live", "needs_config_server_restart"][i % 4] ?? "live")),
      }));
    },
    async open({ page }) {
      await expect(page.getByText("and 6 more services.")).toBeVisible();
    },
  },
];
