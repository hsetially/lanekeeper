// A screen does not move while it fills. More rows arriving must not change the height of the bar above them, even when the words are 40%
// longer and the bar has to wrap (the count at its right grows from "50+" to "550+" as the next page lands).
import { expect, test } from "../fixtures/test";
import { requestsQuiet } from "./audit.ts";
import { pseudoLocalise } from "./pseudo.ts";

test.use({
  dist: process.env["LK_DIST"] ?? "dist",
  colorScheme: "light",
  scenario: "design-compare-default",
});

const GRID = `/compare?file=${encodeURIComponent("tx-infinity-api/tx-infinity-core.yml")}&lanes=sit,sitb,sitc`;

test("compare / the grid options bar keeps its height while the next page arrives", async ({ page, harness }) => {
  test.setTimeout(60_000);
  await page.goto(`${harness.url}${GRID}`);
  await expect(page.locator("h1").first()).toBeVisible();
  await requestsQuiet(page, harness);
  await pseudoLocalise(page);
  await page.setViewportSize({ width: 1024, height: 768 });
  const bar = page.getByRole("group", { name: "Grid options" });
  const grid = page.getByRole("grid");
  await expect(grid).toBeVisible();
  const before = await bar.boundingBox();
  const gridTop = (await grid.boundingBox())?.y;
  expect(before).not.toBeNull();
  // Reading the rows towards the end of the first page asks for the next one.
  await grid.evaluate((el) => {
    el.scrollTop = 160;
  });
  await expect(bar).toContainText("550+");
  await requestsQuiet(page, harness);
  const after = await bar.boundingBox();
  expect(after?.height, "the bar wrapped to another line when the count grew").toBe(before?.height);
  expect((await grid.boundingBox())?.y, "the grid moved down").toBe(gridTop);
});

// The same, for a page that is first drawn as a placeholder and then as the file: the file view of a large file, where a banner
// ("Large file · opened read-only") is above the header once the file has come. What is measured is what Lighthouse scores as
// Cumulative Layout Shift (P10): the sum of the browser's own layout-shift entries, from the first paint to a quiet page.
test.describe("the file view of a large file", () => {
  test.use({ scenario: "design-file-diff-large" });

  test("the file arriving shifts nothing that was on the page", async ({ page, harness }) => {
    test.setTimeout(60_000);
    await page.addInitScript(() => {
      const shifts: { value: number; sources: string[] }[] = [];
      (window as unknown as { __shifts: typeof shifts }).__shifts = shifts;
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          const shift = entry as PerformanceEntry & {
            value: number;
            hadRecentInput: boolean;
            sources?: { node?: Element | null }[];
          };
          if (shift.hadRecentInput) continue;
          shifts.push({
            value: shift.value,
            sources: (shift.sources ?? []).map((source) => `${source.node?.tagName.toLowerCase() ?? "?"}.${(source.node?.getAttribute("class") ?? "").split(" ").slice(0, 3).join(".")}`),
          });
        }
      }).observe({ type: "layout-shift", buffered: true });
    });
    await page.goto(`${harness.url}/swimlanes/sitb/files?path=security%2Fsecurity-roles.yml&tab=diff`);
    await expect(page.getByText("Large file")).toBeVisible();
    await expect(page.getByText("600 changes")).toBeVisible();
    await requestsQuiet(page, harness);
    const shifts = await page.evaluate(
      () =>
        (
          window as unknown as {
            __shifts: { value: number; sources: string[] }[];
          }
        ).__shifts,
    );
    const total = shifts.reduce((sum, shift) => sum + shift.value, 0);
    expect(total, `Cumulative Layout Shift ${total.toFixed(4)}: ${JSON.stringify(shifts)}`).toBeLessThan(0.03);
  });
});
