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

test.use({ dist: process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

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
};

const LIST = /\/api\/v1\/swimlanes(\?.*)?$/;
const DETAIL = /\/api\/v1\/swimlanes\/sitb$/;

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
