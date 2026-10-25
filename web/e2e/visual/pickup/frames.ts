// The frames of the pickup chips (T6, slice A): whether a service or a file has been picked up by what reads it (D85: Live, Live by a time,
// Needs notify or restart, Needs config-server restart), in every place a chip is drawn: the Services tab, the swimlane list's Pickup column
// and its legend, the head of a swimlane, the file tree (a mark on a file, a rollup on a folder), the head of a file, and the effects on a
// draft. With every tooltip that says, in plain words, when the change takes effect.
//
// None of them is a comparison with the design: the handoff has no frame for a pickup chip in any of these places (plan F8, W6, handoff
// gap), so each frame is the app against its own baseline, which CI generates. The rows of design/DEVIATIONS.md that make it so are named in
// each frame's `why`. The scenarios are the pickup slice's (`design-pickup-*`) on the Swimlane Detail board's swimlane, and, where a frame
// needs a file or a draft, the File page's own scenarios with the one answer that carries the state replaced.
//
// Where a state is a hover (a tooltip), the frame is `hold` (the pointer is left on the chip) and its `check` reads the tooltip's words.
import type { Locator, Page } from "@playwright/test";
import type { FileEntry, PickupState } from "../../../src/api/generated/model/index.ts";
import { expect } from "../../fixtures/test";
import { api, rewrite } from "../access/lib/steps";
import { frame, pair, then, type Frame, type Step } from "./runner";
import { FILE, OVERVIEW, SERVICES, hostileMarkers, openFolder, rowOf, tooltip } from "./steps";

const STATES = "design-pickup-states";
const UNSTATED = "design-pickup-unstated";
const QUIET = "design-pickup-quiet";
const HOSTILE = "design-pickup-hostile";

const SERVICES_WHY = "the Services tab has no handoff frame for the pickup chips (design/DEVIATIONS.md: \"StatusBadge, pickup states\", \"Swimlane detail: Services tab\", \"Notifications flag\")";
const OVERVIEW_WHY = "the Pickup column and its legend are the app's, one chip for each state with its own count (design/DEVIATIONS.md: \"Swimlanes: Restarts column\", \"StatusBadge, pickup states\")";
const HEADER_WHY = "the head of a swimlane draws one chip for each pickup state, with its own count (design/DEVIATIONS.md: \"Swimlane detail: header, pickup chips\", \"StatusBadge, pickup states\")";
const TREE_WHY = "the file tree marks a file and rolls a folder up from the files that are loaded (design/DEVIATIONS.md: \"Swimlane detail: file tree\", \"StatusBadge, pickup states\")";
const FILE_WHY = "the head of a file has a pickup chip at the end of its title row, which the design has no place for (design/DEVIATIONS.md: \"File: header, pickup\", \"StatusBadge, pickup states\")";
const DRAFT_WHY = "the draft page is a screen of its own whose effects are pickup chips (design/DEVIATIONS.md: \"Draft (/drafts/:id)\", \"StatusBadge, pickup states\")";
const UNSTATED_WHY = "the hub fills no pickup state on a file or a folder today, so the tree and the head of a file draw none (design/DEVIATIONS.md: \"Swimlane detail: file tree\", \"File: header, pickup\")";
const HOSTILE_WHY = "names and times the hub could send that are not what they look like are shown as markers and as fixed words (design/DEVIATIONS.md: \"Swimlane detail: Services tab\", \"Swimlane detail: file tree\", \"StatusBadge, pickup states\")";

// ----- steps -----------------------------------------------------------------------------------------------------------------------

/** The sentence of the open tooltip: what the chip says in plain words, read in the page and not only photographed. */
const tipSays = (...words: Array<string | RegExp>): Step => async ({ page }) => {
  for (const word of words) await expect(tooltip(page)).toContainText(word);
};

/** Put the pointer on a chip and wait for its tooltip. */
const hoverOn = (locator: (page: Page) => Locator): Step => async ({ page }) => {
  await locator(page).hover();
  await tooltip(page).waitFor();
};

/** The chip of a service's row on the Services tab. */
const serviceChip = (service: string) => (page: Page): Locator => rowOf(page, service).locator("[data-slot='pickup-chip']");

/** The tree row of a file or a folder, by what it is called (the row's name starts with the file or folder's name). */
const treeRow = (page: Page, name: string | RegExp): Locator => page.getByRole("treeitem", { name });
/** A folder's row is named "Open <folder> <folder>/" (or "Close", once it is open) and its mark. */
const folderRow = (page: Page, folder: string): Locator => page.getByRole("treeitem", { name: new RegExp(`^(Open|Close) ${folder}\\b`) });

/** The chip of a header: the head of a swimlane says each state with its own count. */
const headerChip = (name: string) => (page: Page): Locator => page.getByRole("main").locator("[data-slot='pickup-chip']").filter({ hasText: name }).first();

/** The files of sitb's tree with the state the hub gives the core tenant file (the scenario's own answer is "Needs notify or restart"). */
const CORE_PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const coreIs = (state: PickupState | null): Step => async ({ page }) => {
  await rewrite(page, api("/swimlanes/sitb/tree"), (body) => ({
    ...body,
    items: (body["items"] as FileEntry[]).map((entry) => {
      if (entry.path !== CORE_PATH) return entry;
      const { pickup_state: _dropped, ...rest } = entry;
      return state === null ? rest : { ...rest, pickup_state: state };
    }),
  }));
};

/** The effects of a draft, as the hub works them out (a service and when it would pick the change up). */
type Effect = [{ namespace: string; name: string }, PickupState];
const effectsAre = (effects: readonly Effect[]): Step => async ({ page }) => {
  await rewrite(page, api("/drafts/7"), (body) => ({ ...body, effects }));
};
const svc = (name: string): { namespace: string; name: string } => ({ namespace: "csp", name });

const chipsIn = (scope: (page: Page) => Locator, n: number): Step => async ({ page }) => {
  await expect(scope(page).locator("[data-slot='pickup-chip']")).toHaveCount(n);
};

/** No text of the page, and no title attribute of what the hub named, holds a bidirectional override or a zero-width character as itself. */
const NO_RAW_CONTROLS: Step = async ({ page }) => {
  expect(await hostileMarkers(page)).toEqual({ raw: [], marked: true });
};

const NARROW = { width: 1024, height: 768 } as const;

// ----- the Services tab ------------------------------------------------------------------------------------------------------------

/** The seven rows of `design-pickup-states`, the words each chip's tooltip must say, and the chip's name. */
const ROWS: ReadonlyArray<{ code: string; service: string; title: string; says: ReadonlyArray<string | RegExp> }> = [
  { code: "V03", service: "tx-infinity-core", title: "Needs notify or restart, notifications on", says: ["A property file changed", "notified or restarted", "Notifications: on (as reported by the hub)"] },
  { code: "V04", service: "ui-service", title: "Needs notify or restart, notifications off", says: ["notified or restarted", "Notifications: off (as reported by the hub)"] },
  { code: "V05", service: "config-server", title: "Needs config-server restart", says: ["A new folder or a change to channels.yml", "does not serve it until the config-server restarts"] },
  { code: "V06", service: "remote-itm-teller", title: "Live by 09:15 UTC (client cache), a time to come", says: ["client cache expires, by 2025-10-09 09:15:00 UTC", "Client cache lifetime: 20 min"] },
  { code: "V07", service: "document-service", title: "Live by 08:40 UTC (time passed)", says: ["should have expired by 2025-10-09 08:40:00 UTC", "has not reported this service as live yet"] },
  { code: "V08", service: "holds-service", title: "Live within client cache: the hub gave no time", says: ["The hub did not say when", "Client cache lifetime: 5 min"] },
  { code: "V09", service: "limits-service", title: "Live", says: ["This service already has the current config"] },
];

const servicesTab: Frame[] = [
  ...pair("V01", {
    scenario: STATES,
    path: SERVICES,
    title: "Services tab: a service in each of the seven states, the tree marked, the head with its two chips",
    why: SERVICES_WHY,
    check: async ({ page }) => {
      await expect(page.getByRole("row").filter({ has: page.getByRole("cell", { name: /^(tx-infinity-core|ui-service|config-server|remote-itm-teller|document-service|holds-service|limits-service)$/ }) })).toHaveCount(7);
      await expect(page.getByRole("main").getByText("Live by 09:15 UTC")).toBeVisible();
      await expect(page.getByRole("main").getByText("Live by 08:40 UTC")).toBeVisible();
      await expect(page.getByRole("main").getByText("Live within client cache", { exact: true })).toBeVisible();
    },
  }),
  ...ROWS.flatMap(({ code, service, title, says }) =>
    pair(code, {
      scenario: STATES,
      path: SERVICES,
      title: `Services tab: the tooltip of ${service}: ${title}`,
      why: SERVICES_WHY,
      hold: true,
      steps: then(hoverOn(serviceChip(service)), tipSays(...says)),
    }),
  ),
  // The row keeps its two lines at a narrow window, and the tree's marks keep their place.
  frame("V10", { scenario: STATES, path: SERVICES, size: NARROW, title: "Services tab at 1024 px wide: the chips and \"(client cache)\" in the 360 px pane", why: SERVICES_WHY }),
];

// ----- the swimlane list ------------------------------------------------------------------------------------------------------------

const overview: Frame[] = [
  ...pair("V20", {
    scenario: STATES,
    path: OVERVIEW,
    title: "Swimlanes: the Pickup column with a chip for each state that has services, and the legend that names both",
    why: OVERVIEW_WHY,
    check: async ({ page }) => {
      const row = page.getByRole("row", { name: /^Select sitb / });
      await expect(row.getByRole("img", { name: "1 Needs config-server restart" })).toBeVisible();
      await expect(row.getByRole("img", { name: "2 Needs notify or restart" })).toBeVisible();
      // A swimlane with nothing waiting says 0, in words for a screen reader.
      await expect(page.getByRole("row", { name: /^Select presita / }).locator("[data-slot='pickup-counts'][data-empty='true']")).toHaveText("0");
      const legend = page.getByText("Needs notify or restart", { exact: true }).last();
      await expect(legend).toBeVisible();
      await expect(page.getByText("Needs config-server restart", { exact: true }).last()).toBeVisible();
    },
  }),
  ...pair("V21", {
    scenario: STATES,
    path: OVERVIEW,
    title: "Swimlanes: the tooltip of the config-server chip names its state and how it takes effect",
    why: OVERVIEW_WHY,
    hold: true,
    steps: then(hoverOn((page) => page.getByRole("row", { name: /^Select sitb / }).getByRole("img", { name: "1 Needs config-server restart" })), tipSays("config-server", "restart")),
  }),
  ...pair("V22", {
    scenario: STATES,
    path: OVERVIEW,
    title: "Swimlanes: the tooltip of the notify chip",
    why: OVERVIEW_WHY,
    hold: true,
    steps: then(hoverOn((page) => page.getByRole("row", { name: /^Select sitb / }).getByRole("img", { name: "2 Needs notify or restart" })), tipSays("2 services need a notify or a restart")),
  }),
  frame("V23", { scenario: STATES, path: OVERVIEW, size: NARROW, title: "Swimlanes at 1024 px wide: the Pickup column is 88 px and two chips fit", why: OVERVIEW_WHY }),
];

// ----- the head of a swimlane ------------------------------------------------------------------------------------------------------

const header: Frame[] = [
  ...pair("V30", {
    scenario: STATES,
    path: SERVICES,
    title: "Swimlane head: the tooltip of \"1 Needs config-server restart\"",
    why: HEADER_WHY,
    hold: true,
    steps: then(hoverOn(headerChip("Needs config-server restart")), tipSays("config-server")),
  }),
  ...pair("V31", {
    scenario: STATES,
    path: SERVICES,
    title: "Swimlane head: the tooltip of \"2 Needs notify or restart\"",
    why: HEADER_WHY,
    hold: true,
    steps: then(hoverOn(headerChip("Needs notify or restart")), tipSays("2 services need a notify or a restart")),
    check: async ({ page }) => {
      // One chip for each state with its own count: the total of two states is never put under the name of one.
      const head = page.getByRole("main").locator("[data-slot='pickup-chip']");
      await expect(head.filter({ hasText: /1\s*Needs config-server restart/ }).first()).toBeVisible();
      await expect(head.filter({ hasText: /2\s*Needs notify or restart/ }).first()).toBeVisible();
      await expect(page.getByRole("main").getByText("3 Needs config-server restart")).toHaveCount(0);
    },
  }),
];

// ----- the file tree ---------------------------------------------------------------------------------------------------------------

const openApi: Step = async (env) => {
  await openFolder(env.page, "tx-infinity-api");
  await expect(treeRow(env.page, /^tx-infinity-core-sit7\.yml/)).toBeVisible();
};

const tree: Frame[] = [
  ...pair("V40", {
    scenario: STATES,
    path: SERVICES,
    title: "Tree: marks on the files that wait, rollups on the folders (new-folder), tx-infinity-api opened with a tenant file marked and the live ones not",
    why: TREE_WHY,
    steps: openApi,
    check: async ({ page }) => {
      await expect(treeRow(page, /^application\.properties/).getByRole("img", { name: "Needs notify or restart" })).toBeVisible();
      await expect(treeRow(page, /^channels\.yml/).getByRole("img", { name: "Needs config-server restart" })).toBeVisible();
      await expect(folderRow(page, "new-folder").getByRole("img", { name: "Needs config-server restart" })).toBeVisible();
      await expect(treeRow(page, /^tx-infinity-core-sit7\.yml/).getByRole("img", { name: "Needs notify or restart" })).toBeVisible();
      // A live file has no mark, and a folder is never called live.
      await expect(treeRow(page, /^tx-infinity-gateway\.yml/).locator("[data-slot='pickup-chip']")).toHaveCount(0);
      await expect(page.getByRole("tree").getByRole("img", { name: /^Live/ })).toHaveCount(0);
    },
  }),
  ...pair("V41", {
    scenario: STATES,
    path: SERVICES,
    title: "Tree: the tooltip of a file's mark (application.properties)",
    why: TREE_WHY,
    hold: true,
    steps: then(hoverOn((page) => treeRow(page, /^application\.properties/).getByRole("img", { name: "Needs notify or restart" })), tipSays("notified or restarted")),
  }),
  ...pair("V42", {
    scenario: STATES,
    path: SERVICES,
    title: "Tree: the tooltip of a folder the hub itself marks (new-folder), which is not opened",
    why: TREE_WHY,
    hold: true,
    steps: then(hoverOn((page) => folderRow(page, "new-folder").getByRole("img", { name: "Needs config-server restart" })), tipSays("The hub reports the folder itself as waiting for the config-server to restart")),
  }),
  ...pair("V44", {
    scenario: STATES,
    path: SERVICES,
    title: "Tree: the tooltip of an opened folder says how many of the files that are loaded wait",
    why: TREE_WHY,
    hold: true,
    steps: then(openApi, hoverOn((page) => folderRow(page, "tx-infinity-api").getByRole("img", { name: /Needs notify or restart/ })), tipSays("1 of the 3 files loaded in this folder")),
  }),
  frame("V45", { scenario: STATES, path: SERVICES, size: NARROW, title: "Tree at 1024 px wide: the marks keep the end of the row", why: TREE_WHY, steps: openApi }),
];

// ----- the head of a file ----------------------------------------------------------------------------------------------------------

/** The head of the core file (the File page's own scenario), with the state the hub gives it. */
const FILE_STATES: ReadonlyArray<{ code: string; state: PickupState | null; title: string; chip: string | null }> = [
  { code: "V50", state: "needs_notify_or_restart", title: "Needs notify or restart, at the end of the title row", chip: "Needs notify or restart" },
  { code: "V51", state: "needs_config_server_restart", title: "Needs config-server restart", chip: "Needs config-server restart" },
  { code: "V52", state: "live", title: "Live", chip: "Live" },
  { code: "V53", state: "live_within_ttl", title: "Live within client cache (no time on a file)", chip: "Live within client cache" },
  { code: "V54", state: null, title: "no state from the hub: no chip, and not Live", chip: null },
];

const fileHead: Frame[] = [
  ...FILE_STATES.flatMap(({ code, state, title, chip }) =>
    pair(code, {
      scenario: "design-file-core",
      path: FILE,
      title: `File head: ${title}`,
      why: FILE_WHY,
      before: coreIs(state),
      check: async ({ page }) => {
        const head = page.getByRole("region", { name: "File" });
        const chips = head.locator("[data-slot='pickup-chip']");
        if (chip === null) {
          await expect(chips).toHaveCount(0);
          await expect(head.getByText("Live", { exact: true })).toHaveCount(0);
          return;
        }
        await expect(chips).toHaveCount(1);
        await expect(chips.first()).toContainText(chip);
        // At the end of the title row (after the drift state), never in the row of the facts, where it made "Changed by" wrap.
        const heading = await head.getByRole("heading", { level: 1 }).boundingBox();
        const mark = await chips.first().boundingBox();
        const changedBy = await head.getByText("Changed by").boundingBox();
        expect(heading, "the title").not.toBeNull();
        expect(mark, "the chip").not.toBeNull();
        expect(changedBy, "the Changed by line").not.toBeNull();
        if (heading === null || mark === null || changedBy === null) return;
        expect(mark.y + mark.height / 2, "the chip is on the title's row").toBeLessThan(heading.y + heading.height);
        expect(mark.y + mark.height, "the facts row stays below the title row").toBeLessThan(changedBy.y);
      },
    }),
  ),
  ...pair("V55", {
    scenario: "design-file-core",
    path: FILE,
    title: "File head: the tooltip of the chip says when the services that read the file get its new version",
    why: FILE_WHY,
    hold: true,
    steps: then(hoverOn((page) => page.getByRole("region", { name: "File" }).locator("[data-slot='pickup-chip']").first()), tipSays("services take the new version after they are notified or restarted")),
  }),
  frame("V56", { scenario: "design-file-core", path: FILE, size: NARROW, title: "File head at 1024 px wide: the chip stays on the title row", why: FILE_WHY }),
];

// ----- the draft's effects ---------------------------------------------------------------------------------------------------------

const ALL_STATES: Effect[] = [
  [svc("tx-infinity-core"), "needs_notify_or_restart"],
  [svc("config-server"), "needs_config_server_restart"],
  [svc("tx-infinity-gateway"), "live_within_ttl"],
  [svc("limits-service"), "live"],
];
const MANY: Effect[] = Array.from({ length: 53 }, (_, i): Effect => [svc(`service-${String(i + 1).padStart(2, "0")}`), i % 2 === 0 ? "needs_notify_or_restart" : "live_within_ttl"]);

const services = (page: Page): Locator => page.getByRole("list", { name: "Services" });

const draft: Frame[] = [
  ...pair("V60", {
    scenario: "design-file-draft",
    path: "/drafts/7",
    title: "Draft: when services pick this up, as the scenario has it (Needs notify or restart, Live within client cache)",
    why: DRAFT_WHY,
    check: chipsIn(services, 2),
  }),
  ...pair("V61", {
    scenario: "design-file-draft",
    path: "/drafts/7",
    title: "Draft: a service in each of the four states",
    why: DRAFT_WHY,
    before: effectsAre(ALL_STATES),
    check: async (env) => {
      await chipsIn(services, 4)(env);
      await expect(services(env.page)).toContainText("Needs config-server restart");
      await expect(services(env.page)).toContainText("Live within client cache");
    },
  }),
  ...pair("V62", {
    scenario: "design-file-draft",
    path: "/drafts/7",
    title: "Draft: 53 services, the list holds 50 and says how many more",
    why: DRAFT_WHY,
    before: effectsAre(MANY),
    // The list shows the first 50 (MAX_EFFECTS) and counts the rest at its foot, which the frame scrolls to.
    steps: async ({ page }) => {
      await page.getByText("and 3 more services.").scrollIntoViewIfNeeded();
    },
    check: async ({ page }) => {
      await expect(page.getByText("and 3 more services.")).toBeVisible();
      await expect(services(page).locator("[data-slot='pickup-chip']")).toHaveCount(50);
    },
  }),
];

// ----- the hub does not state it, nothing is waiting, and text it could send that is not what it looks like ------------------------------

const unstated: Frame[] = [
  ...pair("V70", {
    scenario: UNSTATED,
    path: SERVICES,
    title: "Not stated: the services say what they wait on and the tree draws no mark and no rollup, even with a folder opened",
    why: UNSTATED_WHY,
    steps: openApi,
    check: async ({ page }) => {
      await expect(page.getByRole("tree").locator("[data-slot='pickup-chip']")).toHaveCount(0);
      await expect(page.getByRole("tree").getByRole("img")).toHaveCount(0);
      await expect(serviceChip("tx-infinity-core")(page)).toBeVisible();
    },
  }),
  ...pair("V71", {
    scenario: UNSTATED,
    path: OVERVIEW,
    title: "Not stated: the Pickup column still counts what the services say",
    why: UNSTATED_WHY,
  }),
  frame("V72", {
    scenario: "design-file-core",
    path: FILE,
    title: "Not stated: the head of a file has no chip, and a file is not called Live for want of one",
    why: UNSTATED_WHY,
    before: coreIs(null),
    check: async ({ page }) => {
      await expect(page.getByRole("region", { name: "File" }).locator("[data-slot='pickup-chip']")).toHaveCount(0);
    },
  }),
];

const quiet: Frame[] = [
  ...pair("V80", {
    scenario: QUIET,
    path: OVERVIEW,
    title: "Quiet: nothing waits on sitb, the Pickup column says 0 and the hub sends no counts",
    why: OVERVIEW_WHY,
    check: async ({ page }) => {
      await expect(page.getByRole("row", { name: /^Select sitb / }).locator("[data-slot='pickup-counts'][data-empty='true']")).toHaveText("0");
      await expect(page.getByRole("row", { name: /^Select sitb / }).locator("[data-slot='pickup-chip']")).toHaveCount(0);
    },
  }),
  ...pair("V81", {
    scenario: QUIET,
    path: SERVICES,
    title: "Quiet: every service is Live, the head has no pickup chip and the tree no mark",
    why: SERVICES_WHY,
    check: async ({ page }) => {
      await expect(page.getByRole("row").filter({ has: page.getByRole("cell", { name: /^(tx-infinity-core|ui-service|limits-service)$/ }) })).toHaveCount(3);
      await expect(page.getByRole("tree").locator("[data-slot='pickup-chip']")).toHaveCount(0);
      await expect(page.getByRole("banner").getByText("Config-server restart needed")).toHaveCount(0);
    },
  }),
];

const hostile: Frame[] = [
  ...pair("V90", {
    scenario: HOSTILE,
    path: SERVICES,
    title: "Hostile: files that hold an override, markup and a zero-width space, and times that are not times of day",
    why: HOSTILE_WHY,
    check: NO_RAW_CONTROLS,
  }),
  ...pair("V91", {
    scenario: HOSTILE,
    path: SERVICES,
    title: "Hostile: the tree's rows with the same names, marked, and no raw control character in any of them",
    why: HOSTILE_WHY,
    steps: async (env) => {
      await openFolder(env.page, "tx-infinity-api");
      await expect(env.page.getByRole("tree").getByRole("treeitem")).toHaveCount(1 + 3);
    },
    check: NO_RAW_CONTROLS,
  }),
  ...pair("V92", {
    scenario: HOSTILE,
    path: SERVICES,
    title: "Hostile: a live_by after the year 9999 says \"Live within client cache\" and never repeats the number",
    why: HOSTILE_WHY,
    hold: true,
    steps: then(hoverOn(serviceChip("ui-service")), tipSays("The hub did not say when")),
    check: async ({ page }) => {
      await expect(rowOf(page, "ui-service")).toContainText("Live within client cache");
      await expect(rowOf(page, "ui-service")).not.toContainText("9000000000000000");
      await expect(rowOf(page, "holds-service")).toContainText("Live within client cache");
    },
  }),
];

export const frames: Frame[] = [...servicesTab, ...overview, ...header, ...tree, ...fileHead, ...draft, ...unstated, ...quiet, ...hostile];
