// What the P10 specs share: how a sample is taken in the page, and how a number is written for the budget checker.
//
// Timing is taken IN the page, with `performance.now()`, so the numbers do not include Playwright's own round trips: an
// input event's `timeStamp` is the start, and the end is a task that runs after the next frame was rendered (a
// MessageChannel message posted from a requestAnimationFrame callback), which is the closest a page can get to "painted"
// without a trace. Every number is written with the browser and the box it was measured on, because a figure from a noisy
// four-core machine is only worth what it says about itself.
import { readFileSync } from "node:fs";
import { cpus, loadavg } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { droppedAt } from "../../scripts/perf/frame-rule.mjs";
import { resultsDir, summarize, writeResult } from "../../scripts/perf/results.mjs";

export { summarize };

// What src/testpages/perf/lab.ts puts on the window (the e2e project has its own program, so it says it again).
declare global {
  interface Window {
    __perf?: { phase: "loading" | "ready"; rows?: number; firstN?: number; diffReady?: number };
    __perfControl?: { dropCompare: (alsoEtag: boolean) => void };
  }
}

type Summary = ReturnType<typeof summarize>;

export function browserVersion(): string {
  try {
    const target = process.env["CARGO_TARGET_DIR"] ?? join(import.meta.dirname, "..", "..", "..", "target");
    return (JSON.parse(readFileSync(join(target, "web-gates", "browser.json"), "utf8")) as { version?: string }).version ?? "unknown";
  } catch {
    return "unknown";
  }
}

export function environment(): { browser: string; cpus: number; load1: number; node: string } {
  return { browser: browserVersion(), cpus: cpus().length, load1: Math.round((loadavg()[0] ?? 0) * 100) / 100, node: process.version };
}

export const round1 = (n: number): number => Math.round(n * 10) / 10;

/** Write the result of a bench for `cargo xtask bench-check`, and say where. Fails the spec if it could not be written. */
export function record(result: Parameters<typeof writeResult>[0]): string {
  const path = writeResult({ ...result, environment: environment() }, { dir: resultsDir() });
  if (path === null) throw new Error(`could not write the result of ${result.bench} to ${resultsDir()}`);
  return path;
}

/** Statistics rounded for a report, in the shape of `values` of a result file. */
export function values(summary: Summary): Record<string, number> {
  return { min: round1(summary.min), p50: round1(summary.p50), p95: round1(summary.p95), p99: round1(summary.p99), max: round1(summary.max), mean: round1(summary.mean) };
}

// ---------------------------------------------------------------------------------------------------------------------
// In the page

/** Long tasks, from the start of the page: `{ start, duration }`. Install before the first navigation. */
export async function observeLongTasks(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const list: Array<{ start: number; duration: number }> = [];
    (window as unknown as { __lkLong: typeof list }).__lkLong = list;
    new PerformanceObserver((entries) => {
      for (const e of entries.getEntries()) list.push({ start: e.startTime, duration: e.duration });
    }).observe({ type: "longtask", buffered: true });
    // The time of the last click or key, as the browser stamped it: the start of a route change or of a diff.
    addEventListener("click", (e) => ((window as unknown as { __lkInputAt: number }).__lkInputAt = e.timeStamp), true);
    addEventListener("keydown", (e) => ((window as unknown as { __lkInputAt: number }).__lkInputAt = e.timeStamp), true);
  });
}

export async function longTasks(page: Page): Promise<Array<{ start: number; duration: number }>> {
  return page.evaluate(() => [...((window as unknown as { __lkLong?: Array<{ start: number; duration: number }> }).__lkLong ?? [])]);
}

/**
 * Arm a watch for a route change: resolves, in the page, with the milliseconds from the next click or key to the moment the
 * h1 reads `expected` and a frame has been rendered. Call, then do the input, then `await` the returned promise handle.
 */
export async function armRouteChange(page: Page, expected: string): Promise<void> {
  await page.evaluate((text) => {
    const w = window as unknown as { __lkNav: Promise<number> };
    w.__lkNav = new Promise<number>((resolve, reject) => {
      let start: number | null = null;
      const stamp = (e: Event): void => {
        start ??= e.timeStamp;
      };
      addEventListener("click", stamp, { capture: true, once: true });
      addEventListener("keydown", stamp, { capture: true, once: true });
      const channel = new MessageChannel();
      channel.port1.onmessage = () => {
        if (start === null) reject(new Error("the h1 changed with no click or key before it"));
        else resolve(performance.now() - start);
      };
      const observer = new MutationObserver(() => {
        if (document.querySelector("h1")?.textContent !== text) return;
        observer.disconnect();
        // A task after the frame that shows it.
        requestAnimationFrame(() => channel.port2.postMessage(0));
      });
      observer.observe(document.body, { subtree: true, childList: true, characterData: true });
      setTimeout(() => {
        observer.disconnect();
        reject(new Error(`the h1 never read ${text}`));
      }, 10_000);
    });
  }, expected);
}

export async function routeChangeMs(page: Page): Promise<number> {
  return page.evaluate(() => (window as unknown as { __lkNav: Promise<number> }).__lkNav);
}

export type ScrollRun = { stamps: number[]; maxTop: number; longTasks: number; heapMb: number | null; lastIndex: number };

/** How a scroll run advances: by the clock (what a person's scroll does) or by a fixed step each frame (equal work per frame). */
export type ScrollPlan = { by: "time"; durationMs: number } | { by: "frames"; frames: number };

/**
 * Scroll the whole extent of `selector`, recording each frame's time. By the clock, the scroll position follows `performance.now()`;
 * by frames, it moves 1/`frames` of the extent in each animation frame, whatever the clock says, so the work in a frame is the
 * same whether the browser makes 60 frames a second or 300 (see grid-5000.spec.ts for why that matters).
 */
export async function scrollRun(page: Page, selector: string, plan: ScrollPlan): Promise<ScrollRun> {
  return page.evaluate(
    ({ selector: sel, plan: how }) =>
      new Promise<ScrollRun>((resolve, reject) => {
        const el = document.querySelector<HTMLElement>(sel);
        if (!el) {
          reject(new Error(`no element ${sel}`));
          return;
        }
        const maxTop = el.scrollHeight - el.clientHeight;
        const stamps: number[] = [];
        let longTasks = 0;
        const observer = new PerformanceObserver((entries) => {
          longTasks += entries.getEntries().length;
        });
        observer.observe({ type: "longtask" });
        el.scrollTop = 0;
        const begin = performance.now();
        let step = 0;
        const frame = (now: number): void => {
          stamps.push(now);
          step += 1;
          const t = how.by === "time" ? Math.min(1, (now - begin) / how.durationMs) : Math.min(1, step / how.frames);
          el.scrollTop = maxTop * t;
          if (t < 1) {
            requestAnimationFrame(frame);
            return;
          }
          observer.disconnect();
          // The last row that is in the DOM, read two frames after the last scroll position was set (the list renders in
          // response to it): the run really went through the whole list.
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              const indexes = [...el.querySelectorAll<HTMLElement>("[data-slot='virtual-row']")].map((row) => Number(row.dataset["index"]));
              const heap = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize;
              resolve({ stamps, maxTop, longTasks, heapMb: heap === undefined ? null : Math.round(heap / 104857.6) / 10, lastIndex: Math.max(...indexes) });
            }),
          );
        };
        requestAnimationFrame(frame);
      }),
    { selector, plan },
  );
}

/**
 * The ORIGINAL W20 metric of a run, kept for information (it is not the budgeted number: see grid-5000.spec.ts): frames per second
 * capped at 60, from the mean frame interval, and the dropped frames, an interval over 25 ms (scripts/perf/frame-rule.mjs). In a
 * frame-rate-limited browser it measures the display, and a frame the machine drops shows in it whatever the page does.
 */
export function framesPerSecond(run: ScrollRun): { fps: number; meanMs: number; dropped: number; droppedAt: number[]; worstMs: number; frames: number } {
  const intervals = run.stamps.slice(1).map((t, i) => t - (run.stamps[i] ?? t));
  const meanMs = intervals.reduce((n, v) => n + v, 0) / intervals.length;
  // Where the dropped frames fall (the index of the frame that was late), so that a run's misses can be told from its noise.
  const late = droppedAt(run.stamps);
  return { fps: round1(Math.min(60, 1000 / meanMs)), meanMs: round1(meanMs), dropped: late.length, droppedAt: late, worstMs: round1(Math.max(...intervals)), frames: run.stamps.length };
}

/**
 * A hand-written virtualised list with no application code in it: 140,000 px of scroll, 40 absolutely positioned text rows that
 * are moved and rewritten on every frame, nothing else. It is the control of the grid's frame-rate number: what a browser on
 * this machine does with the least a virtualised list can ask of it. It uses inline script and style, so it is loaded into a
 * context with `bypassCSP`, and it has no part in what the app is checked for. Scroll element: `#s`.
 */
export const CONTROL_LIST_HTML = `<!doctype html><html><body style="margin:0"><div id="s" style="height:880px;width:1440px;overflow:auto;position:relative"><div style="height:140000px;width:1440px;position:relative"></div></div><script>
  const root = document.querySelector('#s > div');
  const rows = Array.from({ length: 40 }, () => { const d = document.createElement('div'); d.style.cssText = 'position:absolute;left:0;top:0;height:28px;width:100%;white-space:nowrap'; root.appendChild(d); return d; });
  const s = document.getElementById('s');
  function tick() { const first = Math.floor(s.scrollTop / 28); rows.forEach((d, i) => { const idx = first + i; d.style.transform = 'translateY(' + (idx * 28) + 'px)'; d.textContent = 'row ' + idx + ' ' + 'x'.repeat(60); }); requestAnimationFrame(tick); }
  tick();
</script></body></html>`;
