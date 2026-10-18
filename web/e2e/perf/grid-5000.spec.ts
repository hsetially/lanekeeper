// P10: a 5,000-row grid scrolls at 60 frames per second. This is the Compare screen of the app (/compare, scenario
// `design-compare-large`): 40 swimlane columns, 5,000 rows loaded through the real paging path (the generated `getGrid`, a keyset
// cursor, the first page of 50 rows and then 500 a page, each asked for by the button under the rows, eleven requests), drawn by
// DataTable over the one virtualiser, scrolled over its whole extent. The production build (`dist`), with the screen's toolbar,
// header and live layer around the grid, as a person has it.
//
// HOW THE GATE IS JUDGED (W20, decided by the human on 2026-10-17: "capacity plus control"; plans/08-frontend.blockers.md):
//
//  1. CAPACITY, the number `cargo xtask bench-check` reads (`values.min`, budget floor 60). Five runs of 300 frames in a Chromium
//     whose frame-rate limit is off (`--disable-frame-rate-limit --disable-gpu-vsync`). Every frame scrolls the same distance (1/300
//     of the extent), so a frame costs what the grid's work costs; the number is frames a second the page can make, from the mean
//     frame interval, NOT capped at 60 (scripts/perf/frame-rule.mjs, `uncappedFps`). The minimum of the five runs is the budget's.
//
//  2. CONTROL, an assertion of this spec. Five runs of the same 5 s programmatic scroll as a person's display would see it (the
//     normal, frame-rate-limited Chromium), alternated with five runs of a hand-written list that has no application code in it
//     (helpers.ts, CONTROL_LIST_HTML), in the same browser in the same execution. The grid's late frames (an interval over 25 ms)
//     must be no more than the control list's plus DROPPED_FRAME_MARGIN. This is what keeps the gate from being blind: capacity
//     cannot see a frame the compositor drops, but a grid that made the compositor slower would drop many more than a list does.
//
// The original W20 metric (min of five 5 s runs of min(60, 1000 / mean frame interval)) is still measured and written to the
// result file as `vsyncLocked`, for information, with `headroom`, `control` and `controlRule` beside it. It is not a gate: on
// the machine this was developed on, a list with no application code misses it too (see the numbers below).
//
// THE MARGIN. A frame in a late-frame count is late because of the machine as much as the page: Chromium's compositor thread
// stalls 10 to 30 ms at the start of a scroll and now and then after it (TileManager::AssignGpuMemoryToTiles), in a hand-written
// list as in the grid. The margin was chosen from paired runs on this machine (Chromium 141.0.7390.37, 4 cores, a virtual
// machine), the grid and the control list alternating on fresh pages, 5 s each:
//
//   60 pairs, 1,500 frames to a block of five: the grid had a late frame in 26 of 60 runs (35 late frames in all, at most 4 in
//   a run) and the control list in 30 of 60 (40 late frames, at most 5 in a run), so the grid is no worse than the least a
//   list can ask of the machine. In the 12 blocks of five consecutive pairs, the grid's late frames minus the control's were
//   -5 -4 -3 -2 -1 -1 -1 0 1 2 3 6 (grid 3 3 0 2 7 0 4 1 3 1 6 5 against control 1 0 5 4 8 3 5 5 4 1 0 4). Drawing five grid
//   runs and five control runs at random from the 60 each, 200,000 times, the grid minus the control was more than 5 in 1.4%
//   of draws, more than 6 in 0.6%, more than 7 in 0.24%, more than 8 in 0.08% and more than 9 in 0.03%.
//
// A margin of 8 late frames in 1,500 is 0.5% of the frames: one extra hitch of 25 ms or more every 3 s of continuous scrolling.
// It is wide enough that the same code fails by luck about once in 1,200 executions. How narrow it is was measured, by adding a
// 30 ms main-thread hitch to the grid's frame loop (late frames in five runs, 1,500 frames, against the control's allowance of
// about 11, which is its mean of 3 plus 8): a hitch every 240 frames (every 4 s) gave 13, every 120 frames (every 2 s) gave 19,
// every 60 frames (every 1 s) gave 27. So a 30 ms hitch at least every 2 s of scrolling fails the gate (it passes only if the
// control list has 11 or more by chance, about 1 execution in 300), and one every 4 s fails about four times in five (it passes
// when the control list has 5 or more, 22% of executions). What the rule
// does not see: a cost that stays under 16.7 ms a frame does not make a frame late at 60 Hz on this machine, whatever it is
// (a box-shadow on every row, a layer for every row, a layer for each of 840 cells gave 1 to 13 late frames), which is what
// the capacity number is for: its floor of 60 against the 180 or more measured is room for three times today's cost of a frame.
// Raising the margin needs the same kind of data written next to it.
import { chromium, type Page } from "@playwright/test";
import { controlVerdict, uncappedFps } from "../../scripts/perf/frame-rule.mjs";
import { GRID_LANES, GRID_ROWS } from "../../src/mocks/perf/grid.ts";
import { chromiumPath, launchArgs } from "../harness/browser.ts";
import type { Harness } from "../harness/serve.ts";
import { CspWatch, expect, pageViolations, test, watchCsp } from "../fixtures/test";
import { CONTROL_LIST_HTML, environment, framesPerSecond, observeLongTasks, record, round1, scrollRun, settleLive, summarize, values } from "./helpers";

test.use({ dist: "dist", scenario: "design-compare-large" });

const RUNS = 5;
const SCROLL_MS = 5000;
const CAPACITY_FRAMES = 300;
const GRID = "[data-testid='grid-scroller']";
/** The screen's address: the file, the 40 swimlanes of the scenario, every row (not only the differing ones). */
const ROUTE = `/compare?file=${encodeURIComponent("services/payments/application.yml")}&lanes=${GRID_LANES.join(",")}&only=all`;
/** Late frames the grid may have beyond the control list's, over the five runs (1,500 frames). See THE MARGIN above. */
const DROPPED_FRAME_MARGIN = 8;
/** The floor of the budget, repeated here so the result says what the number is judged against. */
const CAPACITY_FLOOR_FPS = 60;

/**
 * Open the screen and load all 5,000 rows the way a person does: the first 50 arrive with the page, and the button under the rows
 * asks for 500 more, ten times. Then the tab's live stream is open and its first refetch is over, so that nothing but the scroll is
 * going on while frames are counted. (The rows the person asked for are kept by a refresh, src/features/compare/grid/useGridPages.ts.)
 */
async function openLargeGrid(page: Page, harness: Harness): Promise<void> {
  harness.clearRequests();
  await page.goto(`${harness.url}${ROUTE}`);
  const grid = page.getByRole("grid", { name: "application.yml across swimlanes" });
  await expect(grid).toBeVisible();
  await expect(grid.getByRole("row").nth(1)).toBeVisible();
  await settleLive(page, harness);
  const shown = async (): Promise<number> => Number(((await page.getByText(/^Showing \d+$/).first().textContent()) ?? "").replace(/\D/g, ""));
  let loaded = await shown();
  for (let guard = 0; loaded < GRID_ROWS && guard < 20; guard++) {
    await page.getByRole("button", { name: /^Load \d+ more$/ }).click();
    await expect.poll(shown, { timeout: 60_000 }).toBeGreaterThan(loaded);
    loaded = await shown();
  }
  await expect(grid).toHaveAttribute("aria-rowcount", String(GRID_ROWS + 1));
  await settleLive(page, harness, 800);
}

const CONTEXT = { viewport: { width: 1440, height: 900 }, timezoneId: "UTC", locale: "en-GB", colorScheme: "light", reducedMotion: "reduce" } as const;

test("the Compare screen, 5,000 rows by 40 swimlanes: capacity (the budget), late frames against a control list (the assertion)", async ({ page, harness, browser, csp }) => {
  const vsync: Array<ReturnType<typeof framesPerSecond> & { longTasks: number; heapMb: number | null }> = [];
  const control: Array<ReturnType<typeof framesPerSecond>> = [];

  // The control list lives in a context of its own, with the page's policy off (it has an inline script and style).
  const controlContext = await browser.newContext({ ...CONTEXT, bypassCSP: true });
  const bare = await controlContext.newPage();

  // Alternate the grid and the control list, so that a change in the machine between the first run and the last falls on both.
  for (let run = 0; run < RUNS; run++) {
    await observeLongTasks(page);
    await openLargeGrid(page, harness);

    const result = await scrollRun(page, GRID, { by: "time", durationMs: SCROLL_MS });
    // The run went through the whole list, not a prefix of it.
    expect(result.lastIndex).toBeGreaterThanOrEqual(4990);
    vsync.push({ ...framesPerSecond(result), longTasks: result.longTasks, heapMb: result.heapMb });

    await bare.setContent(CONTROL_LIST_HTML);
    await bare.waitForTimeout(1500);
    control.push(framesPerSecond(await scrollRun(bare, "#s", { by: "time", durationMs: SCROLL_MS })));
  }
  await controlContext.close();

  expect(await pageViolations(page)).toEqual([]);
  expect(csp.report(), "a CSP or Trusted Types violation").toBe("");

  // Capacity: the same grid in a browser that makes frames as fast as it can, the same step in every frame.
  const uncapped = await chromium.launch({ executablePath: chromiumPath(), args: [...launchArgs(), "--disable-frame-rate-limit", "--disable-gpu-vsync"] });
  const capacity: Array<{ fps: number; frames: number; worstMs: number; longTasks: number; heapMb: number | null }> = [];
  try {
    const context = await uncapped.newContext(CONTEXT);
    const lab = await context.newPage();
    const labWatch = new CspWatch();
    await watchCsp(lab, labWatch);
    for (let run = 0; run < RUNS; run++) {
      await openLargeGrid(lab, harness);
      const result = await scrollRun(lab, GRID, { by: "frames", frames: CAPACITY_FRAMES });
      expect(result.lastIndex).toBeGreaterThanOrEqual(4990);
      capacity.push({ fps: round1(uncappedFps(result.stamps)), frames: result.stamps.length, worstMs: framesPerSecond(result).worstMs, longTasks: result.longTasks, heapMb: result.heapMb });
    }
    expect(labWatch.report(), "a CSP or Trusted Types violation in the uncapped browser").toBe("");
  } finally {
    await uncapped.close();
  }

  const capacitySummary = summarize(capacity.map((c) => c.fps));
  const vsyncSummary = summarize(vsync.map((r) => r.fps));
  const gridDropped = vsync.reduce((n, r) => n + r.dropped, 0);
  const controlDropped = control.reduce((n, r) => n + r.dropped, 0);
  const verdict = controlVerdict({ gridDropped, controlDropped, margin: DROPPED_FRAME_MARGIN });

  const path = record({
    bench: "web/grid_5000_rows_scroll_fps",
    unit: "fps",
    // THE BUDGETED NUMBER: capacity, in frames a second, uncapped. `values.min` is the minimum of the five runs.
    values: values(capacitySummary),
    samples: capacity.length,
    capacity: { note: `${CAPACITY_FRAMES} frames a run, the same scroll step in every frame, frame-rate limit off; frames a second the page can make, not capped at 60`, runs: capacity },
    headroom: { floorFps: CAPACITY_FLOOR_FPS, minFps: round1(capacitySummary.min), factor: round1(capacitySummary.min / CAPACITY_FLOOR_FPS) },
    controlRule: { passed: verdict.passed, gridDropped, controlDropped, margin: DROPPED_FRAME_MARGIN, allowed: verdict.allowed, message: verdict.message },
    control: {
      note: "a hand-written list with no application code (40 rows moved and rewritten each frame over 140,000 px), the same 5 s runs, alternated with the grid's in the same browser",
      droppedFrames: controlDropped,
      runsWithDroppedFrames: control.filter((c) => c.dropped > 0).length,
      worstFrameMs: Math.max(...control.map((c) => c.worstMs)),
      runs: control.map((c) => ({ fps: c.fps, dropped: c.dropped, droppedAt: c.droppedAt, worstMs: c.worstMs })),
    },
    vsyncLocked: {
      note: "INFORMATIONAL, not the budget: the original W20 metric, min(60, 1000 / mean frame interval) over a 5 s scroll in the normal, frame-rate-limited browser; its minimum is what a display would show, and the control list misses it on the same machine",
      values: values(vsyncSummary),
      droppedFrames: gridDropped,
      runs: vsync,
    },
    note: "values.min is the grid's CAPACITY in frames a second (uncapped, equal scroll per frame, minimum of 5 runs), judged against a floor of 60; the late-frame comparison with the control list is an assertion of the spec (controlRule); the frame-rate-limited number is vsyncLocked, for information",
  });
  test.info().annotations.push({
    type: "result",
    description: `${path}: capacity min ${round1(capacitySummary.min)} fps (floor ${CAPACITY_FLOOR_FPS}, runs ${JSON.stringify(capacity.map((c) => c.fps))}); late frames: grid ${gridDropped}, control ${controlDropped}, allowed ${verdict.allowed}; vsync-locked (information) min ${vsyncSummary.min} fps, runs ${JSON.stringify(vsync.map((r) => r.fps))}; ${JSON.stringify(environment())}`,
  });

  expect(verdict.passed, verdict.message).toBe(true);
  expect(capacity).toHaveLength(RUNS);
});
