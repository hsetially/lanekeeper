// P10: the diff of a 6,000-line file is interactive within 500 ms (p95). The hunks come from the generated `compareFile` (the
// server computes them; the browser never diffs two texts), DiffView draws them, and "interactive" is the first `n` key that
// moves the diff. The time is from the click on the link that opens the diff, to that key's effect, both read from the page's
// clock; the spec presses `n` every few milliseconds, as an impatient person would, until one works.
//
// Three series: COLD (a fresh browser context: the chunk, the data and the HTTP cache are all empty), WARM (the chunk is loaded
// and the answer is fetched again in full) and REVALIDATED (the chunk is loaded and the answer is a 304 from the ETag the app
// sent). The budget is the cold series, the slowest of the three. The worst case, every line changed, is measured too.
import type { Browser, Page } from "@playwright/test";
import { expect, pageViolations, test } from "../fixtures/test";
import { environment, longTasks, observeLongTasks, record, round1, summarize, values } from "./helpers";

test.use({ dist: "dist-perf", scenario: "perf" });

const COLD = 20;
const WARM = 20;
const REVALIDATED = 20;

async function startPage(page: Page, url: string): Promise<void> {
  await observeLongTasks(page);
  // The time of the click itself, kept: the `n` presses that follow overwrite __lkInputAt.
  await page.addInitScript(() => {
    addEventListener(
      "click",
      (e) => {
        (window as unknown as { __lkClickAt: number }).__lkClickAt = e.timeStamp;
      },
      true,
    );
  });
  await page.goto(url);
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
}

/** Click "Open the diff" and press `n` until one works. Milliseconds from the click to that key's effect, by the page's own clock. */
async function sample(page: Page): Promise<number> {
  await page.evaluate(() => {
    delete (window as unknown as { __lkClickAt?: number }).__lkClickAt;
  });
  await page.getByRole("link", { name: "Open the diff" }).click();
  const deadline = Date.now() + 15_000;
  for (;;) {
    await page.keyboard.press("n");
    const ms = await page.evaluate(() => {
      const w = window as unknown as { __perf?: { firstN?: number }; __lkClickAt?: number };
      return w.__perf?.firstN === undefined || w.__lkClickAt === undefined ? null : w.__perf.firstN - w.__lkClickAt;
    });
    if (ms !== null) return ms;
    if (Date.now() > deadline) throw new Error("no `n` key ever worked");
    await page.waitForTimeout(4);
  }
}

async function back(page: Page): Promise<void> {
  await page.getByRole("link", { name: "Back to the pages" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
}

async function coldSeries(browser: Browser, url: string, runs: number): Promise<number[]> {
  const out: number[] = [];
  for (let run = 0; run < runs; run++) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light", reducedMotion: "reduce" });
    const page = await context.newPage();
    await startPage(page, url);
    await page.waitForTimeout(250);
    out.push(await sample(page));
    await context.close();
  }
  return out;
}

test("diff of 6,000 lines: interactive within 500 ms, p95 (cold; warm and revalidated reported)", async ({ page, harness, browser, csp }) => {
  const url = `${harness.url}/testpages/perf/nav/a`;

  // Cold: the slowest honest case, and the budgeted series.
  const cold = await coldSeries(browser, url, COLD);

  // Warm and revalidated, in one page: the chunk of the diff is loaded after the first visit.
  await startPage(page, url);
  await sample(page);
  await back(page);
  const warm: number[] = [];
  for (let i = 0; i < WARM; i++) {
    await page.evaluate(() => window.__perfControl?.dropCompare(true));
    warm.push(await sample(page));
    await back(page);
  }
  const revalidated: number[] = [];
  for (let i = 0; i < REVALIDATED; i++) {
    await page.evaluate(() => window.__perfControl?.dropCompare(false));
    revalidated.push(await sample(page));
    await back(page);
  }
  const revalidatedRequests = harness.requests().filter((r) => r.path.startsWith("/api/v1/compare"));
  expect(revalidatedRequests.some((r) => r.status === 304), "the revalidated series is answered 304").toBe(true);

  // The worst case, reported: every line changed, 12,000 lines in one hunk.
  harness.setScenario("perf-full");
  await page.evaluate(() => window.__perfControl?.dropCompare(true));
  await page.reload();
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
  const full: number[] = [];
  for (let i = 0; i < 5; i++) {
    await page.evaluate(() => window.__perfControl?.dropCompare(true));
    full.push(await sample(page));
    await back(page);
  }

  const summary = summarize(cold);
  const path = record({
    bench: "web/diff_6000_lines_interactive",
    unit: "ms",
    values: values(summary),
    samples: cold.length,
    series: { cold: values(summary), warm: values(summarize(warm)), revalidated: values(summarize(revalidated)), fullRewriteWarm: values(summarize(full)) },
    longTasks: (await longTasks(page)).length,
    note: "the budgeted series is COLD (a fresh context: chunk, data and HTTP cache empty); route activation to the first working `n` key; the mock answers at once; 6,000 lines, a change every ten lines",
  });
  test.info().annotations.push({
    type: "result",
    description: `${path}: cold p95 ${round1(summary.p95)} ms of ${cold.length}; warm p95 ${round1(summarize(warm).p95)}; revalidated p95 ${round1(summarize(revalidated).p95)}; full-rewrite p95 ${round1(summarize(full).p95)}; ${JSON.stringify(environment())}`,
  });

  expect(await pageViolations(page)).toEqual([]);
  expect(csp.report(), "a CSP or Trusted Types violation").toBe("");
  expect(cold).toHaveLength(COLD);
});
