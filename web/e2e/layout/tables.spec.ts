// The DataTable's column widths in a real browser (the item S2 flagged as unchecked: its columns carry a `minWidth`, and the table
// scrolls sideways through them instead of squeezing): the screens that use it, at the four widths, say whether the table scrolls
// INSIDE its container (never the page), whether every cell stays under its header, whether the header stays put, and, for the grid of
// 40 swimlanes, whether the first column stays at the left while the others (drawn only when they are in view) scroll past.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { settle } from "../a11y/axe.ts";
import { auditLayout, describeReport, hasDefects } from "./audit.ts";
import { auditTables } from "./tables.ts";
import { freezeDate } from "../a11y/helpers.ts";

test.use({ dist: process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
  { width: 2560, height: 1440 },
] as const;

const LANES_40 = Array.from({ length: 40 }, (_, i) => `swimlane-${String(i + 1).padStart(2, "0")}`).join(",");

// The DataTable grids. (The services list of a swimlane is a `role=table` of its own, not a DataTable; the layout matrix covers it.)
const TABLES = [
  ["swimlanes (11 columns)", "design-swimlanes-six", "/swimlanes", "Swimlanes"],
  ["swimlanes, forty rows", "design-swimlanes-forty", "/swimlanes", "Swimlanes"],
  ["findings", "design-findings-open", "/findings", "Findings"],
  ["approvals queue", "design-approvals-pending", "/approvals", "Proposals"],
  ["audit log", "design-approvals-audit", "/audit", "Audit events"],
  ["feature status", "design-search-admin", "/status", "Feature flags"],
  ["admin: swimlanes", "design-admin-swimlanes", "/admin/swimlanes", "Swimlanes"],
  ["admin: users", "design-admin-users", "/admin/users", "Users"],
] as const;

async function gridBox(page: Page, name: string): Promise<{ scrollWidth: number; clientWidth: number; scrollHeight: number; clientHeight: number } | null> {
  return page.evaluate((label) => {
    const grid = [...document.querySelectorAll("[role='grid']")].find((g) => (g.getAttribute("aria-label") ?? "").includes(label));
    return grid ? { scrollWidth: grid.scrollWidth, clientWidth: grid.clientWidth, scrollHeight: grid.scrollHeight, clientHeight: grid.clientHeight } : null;
  }, name);
}

for (const [id, scenario, url, grid] of TABLES) {
  test.describe(id, () => {
    test.use({ scenario });

    test("scrolls inside its container when the columns need more than the room, and never scrolls the page", async ({ page, harness }, info) => {
      test.setTimeout(120_000);
      await freezeDate(page);
      await page.goto(`${harness.url}${url}`);
      await expect(page.locator("h1").first()).toBeVisible();
      await expect(page.getByRole("grid", { name: new RegExp(grid) }).first()).toBeVisible();
      const widths: Record<number, string> = {};
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await settle(page, 250);
        const label = `${id} @${size.width}x${size.height}`;
        const box = await gridBox(page, grid);
        expect(box, `${label}: the table is on the page`).not.toBeNull();
        if (box) widths[size.width] = `${box.scrollWidth} of ${box.clientWidth}`;
        const page_ = await page.evaluate(() => ({ doc: document.documentElement.scrollWidth - document.documentElement.clientWidth, main: (document.querySelector("main")?.scrollWidth ?? 0) - (document.querySelector("main")?.clientWidth ?? 0) }));
        expect.soft(page_.doc, `${label}: the document scrolls sideways`).toBeLessThanOrEqual(1);
        expect.soft(page_.main, `${label}: the main region scrolls sideways`).toBeLessThanOrEqual(1);
        const tables = await auditTables(page);
        expect.soft(tables.tables, `${label}: the table audit found a table`).toBeGreaterThan(0);
        expect.soft(tables.defects, `${label}: ${JSON.stringify(tables.defects)}`).toEqual([]);
        const report = await auditLayout(page);
        expect.soft(hasDefects(report), describeReport(label, report)).toBe(false);
      }
      info.annotations.push({ type: "table width (scroll of client)", description: JSON.stringify(widths) });
    });
  });
}

test.describe("the grid of forty swimlanes", () => {
  test.use({ scenario: "design-compare-large" });

  const url = (h: string) => `${h}/compare?file=services%2Fpayments%2Fapplication.yml&lanes=${LANES_40}&only=all`;

  test("draws only the columns in view, keeps the first column at the left, and scrolls sideways inside its container", async ({ page, harness }, info) => {
    test.setTimeout(180_000);
    await freezeDate(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(url(harness.url));
    const scroller = page.getByTestId("grid-scroller");
    await expect(scroller).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole("grid", { name: /across swimlanes/ }).getByRole("row").nth(2)).toBeVisible({ timeout: 30_000 });
    await settle(page, 400);

    const read = () =>
      page.evaluate(() => {
        const grid = document.querySelector<HTMLElement>("[data-testid='grid-scroller']");
        if (grid === null) return null;
        const header = grid.querySelector("[role='row'][aria-rowindex='1']");
        const headers = [...(header?.querySelectorAll("[role='columnheader']") ?? [])].map((h) => ({ col: Number(h.getAttribute("aria-colindex")), left: h.getBoundingClientRect().left, right: h.getBoundingClientRect().right, text: (h.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 30) }));
        const g = grid.getBoundingClientRect();
        return { left: g.left, right: g.right, scrollLeft: grid.scrollLeft, scrollWidth: grid.scrollWidth, clientWidth: grid.clientWidth, headers };
      });

    const start = await read();
    expect(start).not.toBeNull();
    if (start === null) return;
    expect(start.headers.length, "the headers drawn").toBeGreaterThan(2);
    expect(start.headers.length, `not all 41 columns are drawn at once (${start.headers.length})`).toBeLessThan(41);
    expect(start.scrollWidth, "forty columns do not fit in 1440 px: the grid scrolls inside itself").toBeGreaterThan(start.clientWidth + 200);
    const first = start.headers[0];
    expect(first?.col, "the first column drawn is column 1 (the setting path)").toBe(1);
    // The page does not scroll sideways because the grid does.
    expect(await page.evaluate(() => (document.querySelector("main")?.scrollWidth ?? 0) - (document.querySelector("main")?.clientWidth ?? 0))).toBeLessThanOrEqual(1);

    // Scroll to the middle, then to the end: column 1 stays at the left edge, other columns replace those that left, and the last is reachable.
    for (const target of [0.5, 1]) {
      await scroller.evaluate((el, t) => {
        (el as HTMLElement).scrollLeft = ((el as HTMLElement).scrollWidth - (el as HTMLElement).clientWidth) * t;
      }, target);
      await settle(page, 300);
      const now = await read();
      expect(now).not.toBeNull();
      if (now === null) continue;
      const pinned = now.headers.find((h) => h.col === 1);
      expect(pinned, `the first column is still drawn at ${target * 100}%`).toBeDefined();
      expect(Math.abs((pinned?.left ?? -999) - now.left), `the first column stays at the left edge at ${target * 100}%`).toBeLessThanOrEqual(2);
      expect(now.headers.some((h) => h.col > 20), `columns far to the right are drawn at ${target * 100}%`).toBe(true);
      const rows = await auditTables(page);
      expect(rows.defects, JSON.stringify(rows.defects)).toEqual([]);
    }
    // (The table audit above puts the scroll back to the start when it is done, so go to the end again.)
    await scroller.evaluate((el) => {
      (el as HTMLElement).scrollLeft = (el as HTMLElement).scrollWidth;
    });
    await settle(page, 300);
    const end = await read();
    const last = end?.headers.at(-1);
    expect(last?.col, "the last swimlane's column is drawn at the end").toBe(41);
    expect(last?.right ?? 9999, "and it is inside the container").toBeLessThanOrEqual((end?.right ?? 0) + 2);
    info.annotations.push({ type: "grid", description: `scrollWidth ${end?.scrollWidth} of ${end?.clientWidth}; ${end?.headers.length} headers drawn at the end` });
  });

  for (const size of SIZES) {
    test(`at ${size.width} wide: the page does not scroll, the header holds ${size.width < 1300 ? "narrower" : "wider"} columns, and text is not covered`, async ({ page, harness }) => {
      test.setTimeout(180_000);
      await freezeDate(page);
      await page.setViewportSize(size);
      await page.goto(url(harness.url));
      await expect(page.getByTestId("grid-scroller")).toBeVisible({ timeout: 30_000 });
      await expect(page.getByRole("grid", { name: /across swimlanes/ }).getByRole("row").nth(2)).toBeVisible({ timeout: 30_000 });
      await settle(page, 400);
      const report = await auditLayout(page);
      expect(hasDefects(report), describeReport(`40 swimlanes @${size.width}`, report)).toBe(false);
      const tables = await auditTables(page);
      expect(tables.defects, JSON.stringify(tables.defects)).toEqual([]);
    });
  }
});
