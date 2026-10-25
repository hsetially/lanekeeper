// P10: the diff of a 6,000-line file is interactive within 500 ms (p95). This is the file view of the app (the Diff tab of
// /swimlanes/sitb/files, scenario `design-file-diff-large`: a file of 6,140 lines whose comparison is 6,000 lines of hunks, a change
// every ten lines, tagged with an ETag as the hub tags it). The hunks come from the generated `compareFile` (the server computes
// them; the browser never diffs two texts), DiffView draws them, and "interactive" is the first `n` key that moves the diff: the
// toolbar then reads "Change 1 of N". The time is from the click on the Diff tab to the moment that text appeared, both read from
// the page's clock; the spec presses `n` every few milliseconds, as an impatient person would, until one works.
//
// Two series: COLD (a fresh browser context, the production build: the chunk, the data and the HTTP cache are all empty when the
// file opens, and the Diff tab is clicked once the file is on screen and the page has settled) and WARM (the same tab left for the
// file's own tab and clicked again, the chunk and the comparison are in memory). The budget is the cold series, the slower of the two.
// The file view asks for the comparison as soon as the file is open (its tab shows the count of changes), which is how the real
// screen is; the click then finds the answer and draws it. The Editor role is the one the editor warm-up runs for: no long task may
// fall in the warm-up's window.
import type { Browser, Page } from "@playwright/test";
import type { Harness } from "../harness/serve.ts";
import { expect, pageViolations, test } from "../fixtures/test";
import { environment, longTasks, observeLongTasks, record, round1, settleLive, summarize, values } from "./helpers";

test.use({ dist: "dist", scenario: "design-file-diff-large" });

const COLD = 20;
const WARM = 20;
const FILE = `/swimlanes/sitb/files?path=${encodeURIComponent("security/security-roles.yml")}`;
const DIFF_TAB = /^Diff/;

// The page is started signed in, so it has opened the tab's live stream; its first connection refetches what is on screen once.
// A sample starts after that, so the diff is timed with nothing else going on (the log is cleared first: it is how the stream is told).
async function startPage(page: Page, harness: Harness): Promise<void> {
  await observeLongTasks(page);
  await page.addInitScript(() => {
    const w = window as unknown as { __lkClickAt?: number; __lkFirstChange?: number };
    // The time of the click itself, kept: the `n` presses that follow overwrite __lkInputAt.
    addEventListener("click", (e) => (w.__lkClickAt = e.timeStamp), true);
    // The first moment the toolbar says which change the diff is on: the diff took an `n`. (Reset by the spec before each sample.)
    new MutationObserver(() => {
      if (w.__lkFirstChange !== undefined) return;
      if (/^Change \d+ of \d+$/.test(document.querySelector("[data-slot='diff-position']")?.textContent ?? "")) w.__lkFirstChange = performance.now();
    }).observe(document, { subtree: true, childList: true, characterData: true });
  });
  harness.clearRequests();
  await page.goto(`${harness.url}${FILE}`);
  await expect(page.getByRole("heading", { level: 1, name: "security-roles.yml" })).toBeVisible();
  await expect(page.getByRole("tab", { name: DIFF_TAB })).toBeVisible();
  await settleLive(page, harness);
}

/** Click the Diff tab and press `n` until one works. Milliseconds from the click to the toolbar reading "Change 1 of N", by the page's own clock. */
async function sample(page: Page): Promise<number> {
  await page.evaluate(() => {
    const w = window as unknown as { __lkClickAt?: number; __lkFirstChange?: number };
    delete w.__lkClickAt;
    delete w.__lkFirstChange;
  });
  await page.getByRole("tab", { name: DIFF_TAB }).click();
  const deadline = Date.now() + 15_000;
  for (;;) {
    await page.keyboard.press("n");
    const ms = await page.evaluate(() => {
      const w = window as unknown as { __lkFirstChange?: number; __lkClickAt?: number };
      return w.__lkFirstChange === undefined || w.__lkClickAt === undefined ? null : w.__lkFirstChange - w.__lkClickAt;
    });
    if (ms !== null) return ms;
    if (Date.now() > deadline) throw new Error("no `n` key ever worked");
    await page.waitForTimeout(4);
  }
}

/** Back to the file's own tab (the tab row is the route). */
async function back(page: Page): Promise<void> {
  await page.getByRole("tab", { name: /^NFS/ }).click();
  await expect(page.getByRole("tab", { name: /^NFS/ })).toHaveAttribute("aria-selected", "true");
}

async function coldSeries(browser: Browser, harness: Harness, runs: number): Promise<number[]> {
  const out: number[] = [];
  for (let run = 0; run < runs; run++) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light", reducedMotion: "reduce" });
    const page = await context.newPage();
    await startPage(page, harness);
    await page.waitForTimeout(250);
    out.push(await sample(page));
    await context.close();
  }
  return out;
}

test("the file view, the Diff tab of 6,000 lines: interactive within 500 ms, p95 (cold; warm reported)", async ({ page, harness, browser, csp }) => {
  // Cold: the slowest honest case, and the budgeted series.
  const cold = await coldSeries(browser, harness, COLD);

  // Warm, in one page: the chunk of the diff and its answer are in memory after the first visit.
  await startPage(page, harness);
  // The editor warm-up fires once the file is interactive (an Editor). It must cost the main thread nothing worth a long task.
  await page.waitForFunction(() => performance.getEntriesByName("lk:editor-warm:done").length > 0, undefined, { timeout: 30_000 });
  const window_ = await page.evaluate(() => ({ start: performance.getEntriesByName("lk:editor-warm:start")[0]?.startTime ?? -1, done: performance.getEntriesByName("lk:editor-warm:done")[0]?.startTime ?? -1 }));
  const during = (await longTasks(page)).filter((t) => t.start + t.duration >= window_.start && t.start <= window_.done);
  const warmUp = { warmed: window_.start >= 0 && window_.done >= window_.start, longTasksDuring: during.length };
  expect(warmUp.warmed, "the editor warm-up fired").toBe(true);
  expect(warmUp.longTasksDuring, `long tasks while the editor was warmed: ${JSON.stringify(during)}`).toBe(0);
  await sample(page);
  await back(page);
  const warm: number[] = [];
  for (let i = 0; i < WARM; i++) {
    warm.push(await sample(page));
    await back(page);
  }

  const summary = summarize(cold);
  const path = record({
    bench: "web/diff_6000_lines_interactive",
    unit: "ms",
    values: values(summary),
    samples: cold.length,
    series: { cold: values(summary), warm: values(summarize(warm)) },
    editorWarmUp: warmUp,
    longTasks: (await longTasks(page)).length,
    note: "the budgeted series is COLD (a fresh context: chunk, data and HTTP cache empty when the file opens); the click on the Diff tab of the file view to the toolbar reading Change 1 of N after the first working `n` key; the mock answers at once; 6,000 lines of hunks, a change every ten lines; the production build behind the hub's headers",
  });
  test.info().annotations.push({
    type: "result",
    description: `${path}: cold p95 ${round1(summary.p95)} ms of ${cold.length}; warm p95 ${round1(summarize(warm).p95)}; ${JSON.stringify(environment())}`,
  });

  expect(await pageViolations(page)).toEqual([]);
  expect(csp.report(), "a CSP or Trusted Types violation").toBe("");
  expect(cold).toHaveLength(COLD);
});
