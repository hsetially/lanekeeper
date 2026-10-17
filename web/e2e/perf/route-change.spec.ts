// P10: a route change with its data cached takes under 100 ms (p95). The pages are three screens that are chunks of their own,
// each a DataTable over the swimlanes (the generated hook, cached), behind the real role gate, in the real router, served by
// the hub's own header code (src/testpages/perf, the perf-mode build). The time is from the click or key to the new route's h1
// painted. Warm: the chunk and the data are already there, which is what the idle prefetch and the cache are for. Cold is
// reported and not budgeted. T4 Phase C switches this to the real routes with the same result file.
import { expect, test } from "../fixtures/test";
import { armRouteChange, environment, longTasks, observeLongTasks, record, round1, routeChangeMs, settleLive, summarize, values } from "./helpers";

test.use({ dist: "dist-perf", scenario: "perf" });

const ORDER = ["a", "b", "c"] as const;
const LABEL = { a: "Page A", b: "Page B", c: "Page C" } as const;
const TITLE = { a: "Route A", b: "Route B", c: "Route C" } as const;
const ROUNDS = 25;
const COLD_RUNS = 5;

test("route change, cached: click and key, p95 under 100 ms", async ({ page, harness, browser, csp }) => {
  await observeLongTasks(page);
  await page.goto(`${harness.url}/testpages/perf/nav/a`);
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
  // The tab's live stream is open and its first refetch is over: nothing but the click is going on while a route change is timed.
  await settleLive(page, harness);

  // Warm everything once: every chunk is loaded and the swimlanes are in the query cache.
  for (const id of ["b", "c", "a"] as const) {
    await armRouteChange(page, TITLE[id]);
    await page.getByRole("link", { name: LABEL[id] }).click();
    await routeChangeMs(page);
  }
  await page.waitForTimeout(500);

  const click: number[] = [];
  const keyboard: number[] = [];
  let at = 0;
  for (let round = 0; round < ROUNDS; round++) {
    at = (at + 1) % ORDER.length;
    const target = ORDER[at] as (typeof ORDER)[number];
    await armRouteChange(page, TITLE[target]);
    await page.getByRole("link", { name: LABEL[target] }).click();
    click.push(await routeChangeMs(page));
    await page.waitForTimeout(60);
  }
  for (let round = 0; round < ROUNDS; round++) {
    at = (at + 1) % ORDER.length;
    const target = ORDER[at] as (typeof ORDER)[number];
    await page.getByRole("link", { name: LABEL[target] }).focus();
    await armRouteChange(page, TITLE[target]);
    await page.keyboard.press("Enter");
    keyboard.push(await routeChangeMs(page));
    await page.waitForTimeout(60);
  }

  // Cold, in fresh contexts: the chunk of the page is not loaded and has not been hovered. Reported, not budgeted.
  const cold: number[] = [];
  for (let run = 0; run < COLD_RUNS; run++) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light", reducedMotion: "reduce" });
    const coldPage = await context.newPage();
    await observeLongTasks(coldPage);
    harness.clearRequests();
    await coldPage.goto(`${harness.url}/testpages/perf/nav/a`);
    await expect(coldPage.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
    await settleLive(coldPage, harness);
    await armRouteChange(coldPage, "Route B");
    await coldPage.getByRole("link", { name: "Page B" }).click();
    cold.push(await routeChangeMs(coldPage));
    await context.close();
  }

  const warm = [...click, ...keyboard];
  const summary = summarize(warm);
  const path = record({
    bench: "web/route_change_cached",
    unit: "ms",
    values: values(summary),
    samples: warm.length,
    series: { click: values(summarize(click)), keyboard: values(summarize(keyboard)), cold: values(summarize(cold)) },
    longTasks: (await longTasks(page)).length,
    note: "click or key to the new route's h1 painted (a task after the frame that shows it); warm chunk and warm data; the pages are src/testpages/perf, in the perf-mode build",
  });
  test.info().annotations.push({ type: "result", description: `${path}: p95 ${round1(summary.p95)} ms of ${warm.length} (click p95 ${round1(summarize(click).p95)}, key p95 ${round1(summarize(keyboard).p95)}, cold p95 ${round1(summarize(cold).p95)}); ${JSON.stringify(environment())}` });

  expect(csp.report(), "a CSP or Trusted Types violation").toBe("");
  // The budget is judged by `cargo xtask bench-check`; this only refuses a run that did not measure what it says.
  expect(warm).toHaveLength(2 * ROUNDS);
  expect(Math.min(...warm)).toBeGreaterThan(0);
});
