// The performance pages under the production policy (S12): chunk loading, the editor warm-up's fetches, scrolling and
// virtualised rows (positions are set through CSSOM, never a `style` attribute string), the diff with its keys and its two
// modes, and the grid through the real paging path. Zero violations of any kind: a `securitypolicyviolation` event, a Chromium
// CSP issue, a Trusted Types error, or an uncaught exception (the fixtures watch all four and fail the test on any).
import { expect, pageViolations, test } from "../fixtures/test";

test.use({ dist: "dist-perf", scenario: "perf" });

test("the grid, the diff and the route pages run clean under the production CSP and Trusted Types", async ({ page, harness, csp }) => {
  // The route pages: three lazy chunks, a data table each.
  await page.goto(`${harness.url}/testpages/perf/nav/a`);
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
  for (const [link, title] of [["Page B", "Route B"], ["Page C", "Route C"], ["Page A", "Route A"]] as const) {
    await page.getByRole("link", { name: link }).click();
    await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
  }

  // The diff: its keys, both modes, a fold, and the editor warm-up that fires when it is up.
  await page.getByRole("link", { name: "Open the diff" }).click();
  const diff = page.getByRole("grid", { name: /^Diff of / });
  await expect(diff).toBeVisible();
  await page.waitForFunction(() => window.__perf?.phase === "ready", undefined, { timeout: 30_000 });
  const active = () => diff.getAttribute("aria-activedescendant");
  const start = await active();
  for (let i = 0; i < 5; i++) await page.keyboard.press("n");
  await expect.poll(active, { message: "n moved the active change" }).not.toBe(start);
  await page.keyboard.press("p");
  await page.getByRole("button", { name: "Unified" }).click();
  await expect(page.getByRole("button", { name: "Split" })).toBeVisible();
  await diff.evaluate((el) => {
    el.scrollTop = el.scrollHeight / 2;
  });
  await page.waitForFunction(() => performance.getEntriesByName("lk:editor-warm:done").length > 0, undefined, { timeout: 30_000 });

  // The grid: loaded 500 rows a page, scrolled across and down.
  await page.getByRole("link", { name: "Back to the pages" }).click();
  await page.getByRole("link", { name: "Open the grid" }).click();
  await page.waitForFunction(() => window.__perf?.phase === "ready", undefined, { timeout: 60_000 });
  const grid = page.getByRole("grid", { name: "Comparison grid" });
  await expect(grid).toHaveAttribute("aria-rowcount", "5001");
  await grid.evaluate(async (el) => {
    for (const [x, y] of [[0, 0.5], [3000, 0.5], [el.scrollWidth, 1], [0, 0]] as const) {
      el.scrollTo({ left: x, top: y * (el.scrollHeight - el.clientHeight) });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    }
  });

  expect(await pageViolations(page)).toEqual([]);
  expect(csp.report(), "a CSP or Trusted Types violation").toBe("");
});
