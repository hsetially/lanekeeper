// Conditional requests in a real browser (P10, plan "Query keys, retry and SSE invalidation"): the app keeps the answer of a
// content read with its ETag and asks again with `If-None-Match`; a 304 costs no body and the screen shows the kept answer.
// The pages are the diff of the perf build, behind the harness, which answers 304 exactly as the hub's content operations do
// (src/mocks/scenario.ts). What each case proves: the header is sent and is the ETag that was given, a 304 gives the same
// screen, a request after the ETag is gone is unconditional, and an answer that changed replaces the kept one.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";

test.use({ dist: "dist-perf", scenario: "perf" });

const REALISTIC = '"perf-diff-6000-realistic-1"';
const FULL = '"perf-diff-6000-full-1"';

const compare = (harness: { requests(): Array<{ path: string; status: number; ifNoneMatch?: string | null }> }) => harness.requests().filter((r) => r.path.startsWith("/api/v1/compare"));

/** Open the diff from the pages, and answer with the number of rows it drew (ARIA's row count, which counts them all). */
async function openDiff(page: Page): Promise<number> {
  await page.getByRole("link", { name: "Open the diff" }).click();
  const grid = page.getByRole("grid", { name: /^Diff of / });
  await expect(grid).toBeVisible();
  await page.waitForFunction(() => window.__perf?.phase === "ready", undefined, { timeout: 30_000 });
  return Number(await grid.getAttribute("aria-rowcount"));
}

async function back(page: Page): Promise<void> {
  await page.getByRole("link", { name: "Back to the pages" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
}

async function start(page: Page, base: string): Promise<void> {
  await page.goto(`${base}/testpages/perf/nav/a`);
  await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
}

test("a second read sends the ETag it was given, is answered 304, and the screen is the same", async ({ page, harness }) => {
  await start(page, harness.url);
  const first = await openDiff(page);
  expect(first, "the diff has its rows (6,000 lines, a change every ten, the unchanged runs folded)").toBeGreaterThan(5000);
  await back(page);

  // The query cache forgets the answer, the ETag cache keeps it: the next read is a revalidation.
  await page.evaluate(() => window.__perfControl?.dropCompare(false));
  const second = await openDiff(page);

  const requests = compare(harness);
  expect(requests, "two reads").toHaveLength(2);
  expect(requests[0]).toMatchObject({ status: 200, ifNoneMatch: null });
  expect(requests[1]).toMatchObject({ status: 304, ifNoneMatch: REALISTIC });
  expect(second, "a 304 shows the kept answer, whole").toBe(first);
});

test("once the ETag is gone the read is unconditional and the answer is whole", async ({ page, harness }) => {
  await start(page, harness.url);
  const first = await openDiff(page);
  await back(page);
  await page.evaluate(() => window.__perfControl?.dropCompare(true));
  const again = await openDiff(page);

  const requests = compare(harness);
  expect(requests).toHaveLength(2);
  expect(requests[1]).toMatchObject({ status: 200, ifNoneMatch: null });
  expect(again).toBe(first);
});

test("an answer that changed replaces the kept one, and the new ETag is what is sent next", async ({ page, harness }) => {
  await start(page, harness.url);
  const before = await openDiff(page);
  await back(page);

  // The file changed on the server: another ETag, another body (every line changed, so twice the rows).
  harness.setScenario("perf-full");
  await page.evaluate(() => window.__perfControl?.dropCompare(false));
  const after = await openDiff(page);
  expect(after, "the new answer, not the kept one").toBeGreaterThan(before);
  await back(page);

  await page.evaluate(() => window.__perfControl?.dropCompare(false));
  const again = await openDiff(page);

  const requests = compare(harness);
  expect(requests).toHaveLength(3);
  expect(requests[1]).toMatchObject({ status: 200, ifNoneMatch: REALISTIC });
  expect(requests[2]).toMatchObject({ status: 304, ifNoneMatch: FULL });
  expect(again).toBe(after);
});
