// P10: a route change with its data cached takes under 100 ms (p95). The pages are four screens of the app, each a chunk of its own,
// reached from the sidebar as a person reaches them: Swimlanes (forty of them), Compare, Findings and the Approvals queue, behind
// the real role gate, in the real router, in the production build, served by the hub's own header code (scenario `perf-routes`,
// src/mocks/perf/scenarios.ts, which is the design scenarios of those screens in one). The time is from the click or key to the new
// route's h1 painted. Warm: the chunk and the data are already there, which is what the idle prefetch and the cache are for. Cold is
// reported and not budgeted.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { armRouteChange, environment, longTasks, observeLongTasks, record, round1, routeChangeMs, settleLive, summarize, values } from "./helpers";

test.use({ dist: "dist", scenario: "perf-routes" });

/** The sidebar's entries in the order they are walked, the name of their link (a count may follow it) and the h1 of their screen. */
const ORDER = ["swimlanes", "compare", "findings", "approvals"] as const;
const LABEL = { swimlanes: /^Swimlanes/, compare: /^Compare/, findings: /^Findings/, approvals: /^Approvals/ } as const;
const TITLE = { swimlanes: "Swimlanes", compare: "Compare", findings: "Findings", approvals: "Approvals" } as const;
const START = "/swimlanes";
const sidebarLink = (page: Page, id: (typeof ORDER)[number]) => page.getByRole("complementary", { name: "Primary" }).getByRole("link", { name: LABEL[id] });
const ROUNDS = 25;
const COLD_RUNS = 5;

test("route change, cached: click and key, p95 under 100 ms", async ({ page, harness, browser, csp }) => {
  await observeLongTasks(page);
  await page.goto(`${harness.url}${START}`);
  await expect(page.getByRole("heading", { level: 1, name: TITLE.swimlanes })).toBeVisible();
  // The tab's live stream is open and its first refetch is over: nothing but the click is going on while a route change is timed.
  await settleLive(page, harness);

  // Warm everything once: every chunk is loaded and each screen's data is in the query cache.
  for (const id of [...ORDER.slice(1), ORDER[0]]) {
    await armRouteChange(page, TITLE[id]);
    await sidebarLink(page, id).click();
    await routeChangeMs(page);
  }
  await page.waitForTimeout(500);

  const click: number[] = [];
  const keyboard: number[] = [];
  // Which screen each sample went to, so that a slow one can be named (the result carries each screen's own figures).
  const clickTo: Array<(typeof ORDER)[number]> = [];
  const keyboardTo: Array<(typeof ORDER)[number]> = [];
  let at = 0;
  for (let round = 0; round < ROUNDS; round++) {
    at = (at + 1) % ORDER.length;
    const target = ORDER[at] as (typeof ORDER)[number];
    await armRouteChange(page, TITLE[target]);
    await sidebarLink(page, target).click();
    click.push(await routeChangeMs(page));
    clickTo.push(target);
    await page.waitForTimeout(60);
  }
  for (let round = 0; round < ROUNDS; round++) {
    at = (at + 1) % ORDER.length;
    const target = ORDER[at] as (typeof ORDER)[number];
    await sidebarLink(page, target).focus();
    await armRouteChange(page, TITLE[target]);
    await page.keyboard.press("Enter");
    keyboard.push(await routeChangeMs(page));
    keyboardTo.push(target);
    await page.waitForTimeout(60);
  }

  // Cold, in fresh contexts: the chunk of the page is not loaded and has not been hovered. Reported, not budgeted.
  const cold: number[] = [];
  for (let run = 0; run < COLD_RUNS; run++) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light", reducedMotion: "reduce" });
    const coldPage = await context.newPage();
    await observeLongTasks(coldPage);
    harness.clearRequests();
    await coldPage.goto(`${harness.url}${START}`);
    await expect(coldPage.getByRole("heading", { level: 1, name: TITLE.swimlanes })).toBeVisible();
    await settleLive(coldPage, harness);
    await armRouteChange(coldPage, TITLE.compare);
    await sidebarLink(coldPage, "compare").click();
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
    byScreen: Object.fromEntries(ORDER.map((id) => [id, values(summarize([...click.filter((_, i) => clickTo[i] === id), ...keyboard.filter((_, i) => keyboardTo[i] === id)]))])),
    longTasks: (await longTasks(page)).length,
    note: "click or key to the new route's h1 painted (a task after the frame that shows it); warm chunk and warm data; the screens are Swimlanes (forty), Compare, Findings and Approvals, reached from the sidebar of the production build",
  });
  test.info().annotations.push({ type: "result", description: `${path}: p95 ${round1(summary.p95)} ms of ${warm.length} (click p95 ${round1(summarize(click).p95)}, key p95 ${round1(summarize(keyboard).p95)}, cold p95 ${round1(summarize(cold).p95)}); ${JSON.stringify(environment())}` });

  expect(csp.report(), "a CSP or Trusted Types violation").toBe("");
  // The budget is judged by `cargo xtask bench-check`; this only refuses a run that did not measure what it says.
  expect(warm).toHaveLength(2 * ROUNDS);
  expect(Math.min(...warm)).toBeGreaterThan(0);
});
