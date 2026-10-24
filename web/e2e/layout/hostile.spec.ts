// Hostile text in the screens (plan, "Layout"; AGENTS rule 10: paths and names are untrusted): long unbroken words, right-to-left text, bidi
// overrides, zero-width characters, stacked combining marks, emoji with joiners, control characters and deep paths, put into the names,
// paths, messages and values that each screen draws. The answers are the real ones of the scenario with the strings replaced.
//
// What must hold: nothing makes the page scroll sideways or reaches past its box, no text is cut off without an ellipsis or covers other
// text (audit.ts), and every character that must not be shown as itself has a visible marker (SafeText) that is inside the screen.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { settle } from "../a11y/axe.ts";
import { auditLayout, describeReport, hasDefects } from "./audit.ts";
import { HOSTILE, hostileKeys, makeHostile, markers, pick } from "./hostile.ts";
import { auditTables } from "./tables.ts";
import { freezeDate } from "../a11y/helpers.ts";
import { CORE_FILE, itemsOf, patchJson, raisePr, reveal } from "../a11y/t5.ts";
import { API as T6_API, bannerRestart, notifyButton, saveFile, servedUrl } from "../a11y/t6.ts";
import { MARCO } from "../../src/mocks/design/shell/index.ts";

test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1440, height: 900 },
] as const;
const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";

type Screen = {
  id: string;
  scenario: string;
  url: string;
  /** Each pattern's answers have these keys replaced. */
  hostile: ReadonlyArray<[RegExp, readonly string[]]>;
  /** The most markers that the page may draw is not limited; the least is the point. */
  minMarkers: number;
  before?: (page: Page) => Promise<void>;
  /** Runs once the page has drawn its h1: brings it into the state (a dialog, a popover, a revealed value) before the strings are looked at. */
  open?: (page: Page) => Promise<void>;
};

const LIST = /\/api\/v1\/swimlanes(\?.*)?$/;
const DETAIL = /\/api\/v1\/swimlanes\/sitb$/;
const PRS = /\/api\/v1\/prs(\?.*)?$/;
const USERS = /\/api\/v1\/admin\/users/;
const CONTENT = /\/api\/v1\/swimlanes\/sitb\/files\/content/;

/** The names of the people the Admin's user list gives are these hostile strings, the ones with hidden characters first (every name is one). */
async function hostileNames(page: Page): Promise<void> {
  await patchJson(page, USERS, (list) => ({ ...list, items: itemsOf(list).map((u, i) => ({ ...u, display_name: pick(3 + (i % 4)) })) }));
}

/** The masked core file with the hostile lines in front of the real ones: the masks follow the text (their lines and byte offsets move with it). */
async function hostileMaskedFile(page: Page): Promise<void> {
  await patchJson(page, CONTENT, (content, url) => {
    if (url.searchParams.get("source") !== "nfs" || typeof content["text"] !== "string") return content;
    const prefix = `${HOSTILE.map((h, i) => `key${i}: ${h.replace(/[\r\n]/g, " ")}`).join("\n")}\n`;
    const bytes = new TextEncoder().encode(prefix).length;
    const lines = prefix.split("\n").length - 1;
    // (Two routes on one address would not stack: the later one fetches the real answer and the earlier one never runs, so the setting names are made hostile here.)
    const masks = (Array.isArray(content["masks"]) ? (content["masks"] as Array<{ line: number; start: number; end: number; setting?: string | null }>) : []).map((m, i) => ({ ...m, line: m.line + lines, start: m.start + bytes, end: m.end + bytes, ...(typeof m.setting === "string" ? { setting: pick(3 + i) } : {}) }));
    return { ...content, text: `${prefix}${content["text"]}`, masks };
  });
}

const SCREENS: Screen[] = [
  { id: "the swimlane list", scenario: "design-swimlanes-six", url: "/swimlanes", hostile: [[LIST, ["display_name", "project", "tenants"]]], minMarkers: 4 },
  { id: "the swimlane switcher and header, with the detail", scenario: "design-swimlanes-six", url: "/swimlanes/sitb/services", hostile: [[LIST, ["display_name", "tenants"]], [DETAIL, ["display_name", "project", "tenants"]]], minMarkers: 1 }, // the header draws the name (no hidden character) and the first tenant (one mark)
  { id: "the file tree of a swimlane", scenario: "design-swimlanes-six", url: "/swimlanes/sitb/services", hostile: [[/\/api\/v1\/swimlanes\/sitb\/tree/, ["path"]]], minMarkers: 2 },
  {
    id: "a file: its lines",
    scenario: "design-file-core",
    url: `/swimlanes/sitb/files?path=${CORE}`,
    hostile: [],
    minMarkers: 4,
    async before(page) {
      await page.route(/\/api\/v1\/swimlanes\/sitb\/files\/content/, async (route) => {
        const response = await route.fetch();
        const body = (await response.json()) as { text?: string | null; masks?: unknown };
        if (typeof body.text === "string") body.text = `${HOSTILE.map((h, i) => `key${i}: ${h}`).join("\r\n")}\r\n${body.text}`;
        delete body.masks;
        await route.fulfill({ response, json: body });
      });
    },
  },
  { id: "findings", scenario: "design-findings-open", url: "/findings", hostile: [[/\/api\/v1\/findings/, ["path", "message"]]], minMarkers: 2 },
  { id: "the compare grid", scenario: "design-compare-default", url: `/compare?file=tx-infinity-api%2Ftx-infinity-core.yml&lanes=sit,sitb,sitc`, hostile: [[/\/api\/v1\/grid/, ["path", "text"]]], minMarkers: 2 },
  { id: "text search hits", scenario: "design-search-admin", url: "/search?type=text&q=enableAccountSorting", hostile: [[/\/api\/v1\/search\/text/, ["path", "text"]]], minMarkers: 2 },
  { id: "the approvals queue", scenario: "design-approvals-pending", url: "/approvals", hostile: [[/\/api\/v1\/proposals\?/, ["path"]], [/\/api\/v1\/admin\/users/, ["display_name"]]], minMarkers: 2 },
  { id: "the audit log", scenario: "design-approvals-audit", url: "/audit", hostile: [[/\/api\/v1\/admin\/audit/, ["target", "action"]]], minMarkers: 2 },
  { id: "the people of the admin area", scenario: "design-admin-users", url: "/admin/users", hostile: [[/\/api\/v1\/admin\/users/, ["display_name", "email", "github_login"]]], minMarkers: 2 },
  { id: "the agents and tenants of the admin area", scenario: "design-admin-swimlanes", url: "/admin/swimlanes", hostile: [[LIST, ["display_name", "project", "tenants"]]], minMarkers: 2 },

  // ---- T5 ----------------------------------------------------------------------------------------------------------------------------------
  { id: "the pull requests of a swimlane", scenario: "design-prs-list", url: "/swimlanes/sitb/pull-requests?state=all", hostile: [[PRS, ["path"]], [USERS, ["display_name"]]], minMarkers: 2 },
  { id: "the pull requests page, links it must not trust", scenario: "design-prs-hostile", url: "/swimlanes/sitb/pull-requests", hostile: [], minMarkers: 1 },
  {
    id: "the pull request that exists already, in the Raise PR dialog",
    scenario: "design-file-pr-exists",
    url: CORE_FILE,
    hostile: [],
    minMarkers: 2,
    async before(page) {
      // Opened by another person, so the card names them (their name is a hostile string), for a file whose path is hostile too.
      await hostileNames(page);
      await patchJson(page, /\/api\/v1\/prs\/preview$/, (preview) => ({ ...preview, open_pr: { ...(preview["open_pr"] as object), author: MARCO, path: pick(3) } }));
    },
    async open(page) {
      await raisePr(page, 1);
      await page.getByRole("button", { name: "Continue" }).click();
      await expect(page.getByRole("dialog").getByText(/already open/i).first()).toBeVisible();
    },
  },
  {
    id: "the pull request job: what the server said and its warnings",
    scenario: "design-file-pr-failed",
    url: CORE_FILE,
    hostile: [],
    minMarkers: 3,
    async before(page) {
      await patchJson(page, /\/api\/v1\/pr-jobs\/\d+$/, (job) => ({ ...job, error: pick(3), warnings: [pick(2), pick(5), pick(8)] }));
    },
    async open(page) {
      await raisePr(page, 3);
      await expect(page.getByRole("dialog").getByText("What the server said")).toBeVisible();
    },
  },
  {
    id: "the evidence of a change: logins, executables and jobs from the VMs (File history)",
    scenario: "design-attribution-hostile",
    url: CORE_FILE,
    hostile: [],
    minMarkers: 4,
    async open(page) {
      await page.getByRole("button", { name: "History", exact: true }).click();
      await page.getByRole("complementary", { name: "History" }).locator("[data-slot='attribution'] button").first().click();
      await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
    },
  },
  { id: "the audit log with hostile attribution", scenario: "design-attribution-audit", url: "/audit", hostile: [[/\/api\/v1\/admin\/audit/, ["login", "executable", "login_identity", "name"]]], minMarkers: 3 },
  {
    id: "the activity of a swimlane with hostile attribution",
    scenario: "design-attribution-audit",
    url: "/swimlanes/sitb/activity",
    hostile: [[/\/api\/v1\/admin\/audit/, ["login", "executable", "login_identity", "name"]]],
    minMarkers: 2,
  },
  {
    id: "a masked file: hostile lines and a hostile setting name, then the Reveal dialog",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    hostile: [],
    minMarkers: 4,
    before: hostileMaskedFile,
    async open(page) {
      await page.getByRole("button", { name: /^Reveal value of / }).first().click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },
  {
    id: "a masked file: a hostile value that was revealed",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    hostile: [],
    minMarkers: 2,
    async before(page) {
      await page.route(/\/api\/v1\/swimlanes\/sitb\/files\/reveal$/, (route) => route.fulfill({ headers: { "cache-control": "no-store" }, json: { value: `${pick(3)}${pick(5)}${pick(6)}` } }));
    },
    async open(page) {
      await reveal(page);
      await expect(page.locator("[data-slot='revealed-value']")).toBeVisible();
    },
  },
  // ---- T6 ----------------------------------------------------------------------------------------------------------------------------------
  {
    id: "the text the config-server serves for a file (As served): its lines, the placeholders left in it and the sources it came from",
    scenario: "design-served-text",
    url: "/swimlanes/sitb/served?application=tx-infinity-core&tenant=sit7&file=tx-infinity-api%2Fconnection-pool.yml",
    hostile: [],
    minMarkers: 4,
    async before(page) {
      // The hostile lines in front of the real ones, the sources and the placeholders hostile too (a placeholder is a name from the file).
      await patchJson(page, /\/api\/v1\/swimlanes\/sitb\/served/, (served) => {
        const sources = Array.isArray(served["sources"]) ? (served["sources"] as Array<Record<string, unknown>>) : [];
        const prefix = `${HOSTILE.map((h, i) => `key${i}: ${h.replace(/[\r\n]/g, " ")}`).join("\n")}\n`;
        return {
          ...served,
          text: typeof served["text"] === "string" ? `${prefix}${served["text"]}` : served["text"],
          masks: undefined,
          sources: sources.map((source, i) => ({ ...source, path: pick(1 + i * 2) })),
          unresolved_placeholders: [pick(3), pick(5), pick(8)],
        };
      });
    },
  },
  {
    id: "the pickup chips: files a service waits on, a tree and times the page must not trust (Services tab)",
    scenario: "design-pickup-hostile",
    url: "/swimlanes/sitb/services",
    hostile: [],
    minMarkers: 3,
    async open(page) {
      const rows = page.getByRole("tree").getByRole("treeitem");
      const before = await rows.count();
      await page.getByRole("button", { name: "Open tx-infinity-api" }).click();
      await expect.poll(() => rows.count()).toBeGreaterThan(before);
      await expect(page.getByRole("tree").locator("[data-slot='pickup-chip']").first()).toBeVisible();
    },
  },
  {
    id: "the pickup column of the swimlane list, in rows whose names the page must not trust",
    scenario: "design-pickup-states",
    url: "/swimlanes",
    hostile: [[LIST, ["display_name", "project", "tenants"]]],
    minMarkers: 4,
    async open(page) {
      await expect(page.locator("[data-slot='pickup-chip']").first()).toBeVisible();
    },
  },
  {
    id: "Notify services: the paths and the services it announces, hostile (the Services tab and the dialog)",
    scenario: "design-notify-services-tab",
    url: "/swimlanes/sitb/services",
    hostile: [[DETAIL, ["name", "pending_files"]]],
    minMarkers: 4,
    async open(page) {
      await notifyButton(page).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },
  {
    id: "the config-server restart: a Deployment and affected services whose names the page must not trust (the dialog types the name)",
    scenario: "design-notify-config-server",
    url: "/swimlanes/sitb/services",
    hostile: [[T6_API.configServerOffer, ["name"]]],
    minMarkers: 3,
    async open(page) {
      await bannerRestart(page).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },
  {
    id: "Restart services of a file: the services the hub lists, hostile (the dialog types the swimlane's id)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    hostile: [],
    minMarkers: 3,
    async before(page) {
      // Names with hidden characters in them (the first strings of the list have none, so `makeHostile` would not do).
      await patchJson(page, T6_API.restartOffer, (offer) => ({ ...offer, services: [3, 4, 5, 8].map((i) => ({ namespace: "sitb", name: pick(i) })) }));
    },
    async open(page) {
      await saveFile(page);
      await page.getByRole("button", { name: "Restart services" }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },
  {
    id: "the services of a draft, with names the page must not trust, and the chips beside them",
    scenario: "design-file-draft",
    url: "/drafts/7",
    hostile: [],
    minMarkers: 4,
    async before(page) {
      await patchJson(page, /\/api\/v1\/drafts\/7$/, (draft) => ({
        ...draft,
        effects: [3, 4, 5, 6, 9].map((i, n) => [{ namespace: pick(i + 1), name: pick(i) }, ["needs_notify_or_restart", "needs_config_server_restart", "live_within_ttl", "live", "needs_notify_or_restart"][n] ?? "live"]),
      }));
    },
  },
  {
    id: "As served: what the answer names (the application, the tenant, the channel and the file), hostile",
    scenario: "design-served-text",
    url: servedUrl(),
    hostile: [],
    minMarkers: 3,
    async before(page) {
      await patchJson(page, T6_API.served, (served) => ({ ...served, application: pick(3), tenant: pick(4), channel: pick(5), file: pick(9) }));
    },
  },
  {
    id: "As served: an address whose every value is a string the page must not trust (the form says which fields are wrong)",
    scenario: "design-served-text",
    url: `/swimlanes/sitb/served?${new URLSearchParams({ application: pick(3), tenant: pick(4), channel: pick(5), file: pick(2) }).toString()}`,
    hostile: [],
    minMarkers: 0,
  },
  {
    id: "As served: an address whose names are long unbroken words (a valid request, and its answer)",
    scenario: "design-served-text",
    url: servedUrl({ application: "a".repeat(120), channel: "b".repeat(120), file: `${"d".repeat(60)}/${"e".repeat(60)}/${"f".repeat(60)}.yml` }),
    hostile: [],
    minMarkers: 0,
  },
  { id: "the sentinels of the admin area", scenario: "design-admin-t5-sentinels", url: "/admin/sentinels", hostile: [[/\/api\/v1\/admin\/sentinels/, ["host", "version"]]], minMarkers: 3 },
  { id: "the OS Login mappings of the admin area", scenario: "design-admin-t5-oslogin", url: "/admin/oslogin", hostile: [[/\/api\/v1\/admin\/os-login-users$/, ["os_login"]], [USERS, ["display_name", "email"]]], minMarkers: 3 },
  {
    id: "the severity rules of the admin area",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    hostile: [[/\/api\/v1\/admin\/severity-rules$/, ["name", "path_glob", "setting_glob"]]],
    minMarkers: 3,
  },
  {
    id: "the severity rule dialog, with a hostile name and glob",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    hostile: [[/\/api\/v1\/admin\/severity-rules$/, ["name", "path_glob"]]],
    minMarkers: 2,
    async open(page) {
      await page.getByRole("button", { name: /^Edit rule security-high/ }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },
  { id: "the retention policy: who changed it last", scenario: "design-admin-t5-retention", url: "/admin/retention", hostile: [], minMarkers: 1, before: hostileNames },
];

for (const screen of SCREENS) {
  test.describe(screen.id, () => {
    test.use({ scenario: screen.scenario });

    test("hostile text: no sideways scroll, nothing cut off or covered, and the hidden characters are marked", async ({ page, harness, csp }) => {
      test.setTimeout(120_000);
      await freezeDate(page);
      for (const [pattern, keys] of screen.hostile) await makeHostile(page, pattern, keys);
      await screen.before?.(page);
      await page.goto(`${harness.url}${screen.url}`);
      await expect(page.locator("h1").first()).toBeVisible();
      await screen.open?.(page);
      await page.waitForTimeout(800);
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await settle(page, 250);
        const label = `${screen.id} @${size.width}x${size.height}`;
        const report = await auditLayout(page, { skip: ".monaco-editor" });
        expect.soft(hasDefects(report), describeReport(label, report)).toBe(false);
        const tables = await auditTables(page);
        expect.soft(tables.defects, `${label}: ${JSON.stringify(tables.defects)}`).toEqual([]);
        const seen = await markers(page);
        expect.soft(seen.length, `${label}: the page drew ${seen.length} markers for hidden characters`).toBeGreaterThanOrEqual(screen.minMarkers);
        expect.soft(seen.filter((m) => !m.visible).map((m) => m.label), `${label}: markers that are not visible`).toEqual([]);
        // The strings were really on the page (a test that replaced nothing proves nothing).
        const text = await page.evaluate(() => document.body.innerText);
        expect.soft(text.length, `${label}: something was drawn`).toBeGreaterThan(200);
      }
      expect(csp.report()).toBe("");
    });
  });
}

test.describe("the strings themselves", () => {
  test("every hostile string that holds a hidden character is split into a marker by SafeText, and the others are shown whole", () => {
    const hidden = HOSTILE.filter((s) => /[\p{Cc}\p{Cf}]/u.test(s));
    expect(hidden.length).toBeGreaterThanOrEqual(5);
    expect(pick(0)).toBe(HOSTILE[0]);
    expect(pick(HOSTILE.length)).toBe(HOSTILE[0]);
    expect(hostileKeys({ a: { display_name: "x", tenants: ["a", "b"], n: 1 } }, ["display_name", "tenants"])).toEqual({ a: { display_name: pick(0), tenants: [pick(1), pick(2)], n: 1 } });
  });
});
