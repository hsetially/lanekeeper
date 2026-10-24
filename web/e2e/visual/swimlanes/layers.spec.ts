// What the browser does with the stacking and the layout of two shared pieces that only a browser can say: a tooltip on a button inside a
// table (it was painted under the table's sticky header: the overlay had no layer, the header has one), and a banner (its button was
// laid out UNDER its words, because the Alert it is made of is a column; the design's banner is one line with its button at the right).
// The unit tests say which classes are there (components/ui/overlays.test.tsx, components/app/Banner.test.tsx); these say what is on screen.
import { expect, test } from "../../fixtures/test";
import { pin, settle } from "../access/lib/app";

const GEOMETRY = async (page: import("@playwright/test").Page, selector: string) =>
  page.locator(selector).first().evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height, right: r.right, bottom: r.bottom };
  });

test.describe("layers and banners, in a real browser", () => {
  test("a tooltip on a button in the services table is the topmost thing at its own centre, above the table's sticky header", async ({ page, harness }) => {
    harness.setScenario("design-swimlanes-agent-off");
    await pin(page, "light", { width: 1440, height: 900 });
    await page.goto(`${harness.url}/swimlanes/sitb/services`);
    await settle(page);
    await page.getByRole("button", { name: "Restart tx-infinity-core" }).hover();
    const tip = page.getByRole("tooltip").first();
    await tip.waitFor();
    const box = await GEOMETRY(page, "[data-slot='tooltip-content']");
    // The tooltip sits over the table's header row: the point at its centre must belong to the tooltip, and not to the header.
    const top = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest("[data-slot]")?.getAttribute("data-slot") ?? null, { x: box.x + box.width / 2, y: box.y + box.height / 2 });
    expect(top).toBe("tooltip-content");
  });

  test("the agent-offline banner is one line, with its action at the right of the words", async ({ page, harness }) => {
    harness.setScenario("design-swimlanes-agent-off");
    await pin(page, "light", { width: 1440, height: 900 });
    await page.goto(`${harness.url}/swimlanes/sitb/services`);
    await settle(page);
    const row = await GEOMETRY(page, "[data-slot='banner-row']");
    const words = await GEOMETRY(page, "[data-slot='banner-row'] [data-slot='alert-title']");
    const action = await GEOMETRY(page, "[data-slot='banner-row'] button");
    expect(row.height, "one line of words and a 24 px button").toBeLessThan(36);
    expect(action.y, "the action is level with the words, not under them").toBeLessThan(words.bottom);
    expect(action.x, "and to the right of them").toBeGreaterThan(words.right);
    expect(row.right - action.right, "at the right edge of the banner").toBeLessThan(8);
  });

  test("the locked banner says 'Start adoption' at the right, on the same line as its words", async ({ page, harness }) => {
    harness.setScenario("design-swimlanes-six");
    await pin(page, "light", { width: 1440, height: 900 });
    await page.goto(`${harness.url}/swimlanes/presitc/drift`);
    await settle(page);
    const words = await GEOMETRY(page, "[data-slot='banner-row'] [data-slot='alert-title']");
    const action = await GEOMETRY(page, "[data-slot='banner-row'] a");
    expect(action.y).toBeLessThan(words.bottom);
    expect(action.x).toBeGreaterThan(words.right);
  });
});
