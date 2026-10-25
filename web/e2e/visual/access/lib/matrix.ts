// The two kinds of visual test of this matrix.
//
//  - `tier1(cases)`: each frame of a handoff board is a test. The frame is rendered from the design (lib/reference.ts), the app is put
//    in the same state (the scenario, the route, the role, whatever the board's props say, reached the way a person reaches it) and
//    the two are compared pixel by pixel, 0.2 per pixel, 0.5% of the pixels that are not masked (lib/compare.ts). A mask names its
//    DEVIATIONS row. Light and dark are the board's own twins: the same app state is compared with each reference.
//  - `appOwned(cases)`: a state the design has no full-size frame for, or that this matrix guards after the first run: the app
//    against its own baseline (`toHaveScreenshot`), the baselines being generated in CI only. A baseline that is missing is a
//    failure in CI (it is never created silently) and NOT RUN anywhere else, so that a run in a sandbox never writes PNGs into the
//    tree. LK_UPDATE_BASELINES (the bootstrap job) and `--update-snapshots=all` create them (e2e/visual/baseline-rule.mjs).
//
// Without the React 18 files (`LK_REACT18_DIR`) a tier-1 test is NOT RUN, and in CI (`CI` set) it fails instead: a missing
// reference is never a pass. A tier-1 test also fails on any CSP or Trusted Types violation of the page it drives.
import { existsSync } from "node:fs";
import type { Browser, Locator, Page } from "@playwright/test";
import { expect, test } from "../../../fixtures/test";
import { baselineIsNotRun } from "../../baseline-rule.mjs";
import { LIVE_REGIONS, pin, settle, type Clock, type Prepare, type Viewport } from "./app";
import { MAX_RATIO, compareFrames, keep, type Aligned, type Comparison, type Mask } from "./compare";
import { elementProbe, reportDifference } from "../../diff-report.mjs";
import { frameOf, probeReference, reference, referencesAvailable, specimenLines, type Theme } from "./reference";

/** A rectangle that is not compared: fixed, or measured in the app once the state is reached. */
export type MaskSpec = Mask | ((page: Page) => Promise<readonly Mask[]>);

export type FrameCase = {
  /** What the frames show, for the test name. */
  name: string;
  /** The board file, and the ids of its frames that are this state (light and dark twins). */
  board: string;
  ids: readonly string[];
  /** The mock scenario the app starts in, and the address it opens. */
  scenario: string;
  path: string;
  /** The size of the window; the frame's own size when left out. */
  viewport?: Viewport;
  /** Reach the state: clicks, a held request, a pushed event. Runs after the page has drawn. */
  prepare?: Prepare;
  /** Before the app is opened (a held request has to be in place before the page asks for it). */
  before?: Prepare;
  /** The state IS the loading skeleton. */
  loading?: boolean;
  /** "date" for a page that types into the editor (see lib/app.ts). */
  clock?: Clock;
  masks?: readonly MaskSpec[];
  /** Parts the app draws somewhere else than the design does, compared where each is (see `Aligned`). Their two places are masked by `masks`. */
  aligned?: readonly Aligned[];
  /** What moves by itself in this state, for the baseline of the app (a toast that leaves, a clock). */
  moving?: (page: Page) => readonly Locator[];
};

/** Mask the box of what the app draws at `selector`, grown by `pad` pixels on every side. */
export function maskOf(selector: string, why: string, pad = 0): MaskSpec {
  return async (page) => {
    const boxes = await page.locator(selector).evaluateAll((els) => els.map((el) => el.getBoundingClientRect()).map((r) => ({ x: r.x, y: r.y, width: r.width, height: r.height })));
    return boxes.map((b) => ({ x: b.x - pad, y: b.y - pad, width: b.width + 2 * pad, height: b.height + 2 * pad, why }));
  };
}

const shortBoard = (board: string): string => board.replace(/^Lanekeeper /, "").replace(/\.dc\.html$/, "");

async function resolveMasks(page: Page, specs: readonly MaskSpec[]): Promise<Mask[]> {
  const out: Mask[] = [];
  for (const spec of specs) {
    if (typeof spec === "function") out.push(...(await spec(page)));
    else out.push(spec);
  }
  return out;
}

/**
 * Where a frame that is over its limit differs (e2e/visual/diff-report.mjs): to the console, which is what a CI log holds, and returned
 * to go into the failure message. The design's page is not open (the references are pictures), so it is opened again to be asked what is
 * under each cluster; if that cannot be done the report says so and goes on without it.
 */
async function whereItDiffers(args: { scratch: Page; page: Page; browser: Browser; board: string; id: string; label: string; result: Comparison }): Promise<string> {
  const { scratch, page, browser, board, id, label, result } = args;
  const notes: string[] = [];
  let design: ReturnType<typeof elementProbe> | undefined;
  let close = async (): Promise<void> => {};
  try {
    const opened = await probeReference(browser, board, id);
    design = opened.probe;
    close = opened.close;
  } catch (error) {
    notes.push(`design page: not queried (${error instanceof Error ? error.message.split("\n")[0] : String(error)})`);
  }
  try {
    const text = await reportDifference(
      scratch,
      { reference: result.painted.reference, app: result.painted.app, diffPng: result.diffPng },
      { label, ratio: result.ratio, limit: MAX_RATIO, differing: result.diff, compared: result.considered, notes, probes: { app: elementProbe(page), ...(design ? { design } : {}) } },
    );
    return `\n${text}`;
  } finally {
    await close();
  }
}

export function tier1(cases: readonly FrameCase[]): void {
  for (const c of cases) {
    for (const id of c.ids) {
      const frame = frameOf(c.board, id);
      const theme = frame.theme as Theme;
      test(`tier 1 · ${shortBoard(c.board)} ${id} (${theme}): ${c.name}`, async ({ page, harness, browser, csp }) => {
        test.setTimeout(150_000);
        const missing = referencesAvailable();
        if (missing !== null) {
          // Never a pass without a reference: skipped and said so, and a failure where CI runs.
          if (process.env["CI"]) throw new Error(`the reference of frame ${id} cannot be rendered in CI: ${missing}`);
          test.skip(true, `NOT RUN: ${missing}`);
        }
        const ref = await reference(browser, c.board, id);
        harness.setScenario(c.scenario);
        await pin(page, theme, c.viewport ?? { width: frame.size.w, height: frame.size.h }, c.clock);
        if (c.before) await c.before(page, harness);
        await page.goto(`${harness.url}${c.path}`);
        await settle(page, { loading: c.loading ?? false });
        if (c.prepare) {
          await c.prepare(page, harness);
          await settle(page, { loading: c.loading ?? false, park: false });
        }
        const app = await page.screenshot({ animations: "disabled", caret: "hide" });
        const masks = await resolveMasks(page, c.masks ?? []);
        const name = `${shortBoard(c.board).toLowerCase().replace(/\W+/g, "-")}-${id}-${theme}`;
        const scratch = await browser.newPage();
        try {
          const result = await compareFrames(scratch, ref, app, masks, c.aligned ?? []);
          keep(name, ref, app, result, { masks, aligned: c.aligned ?? [] });
          test.info().annotations.push({ type: "ratio", description: JSON.stringify({ frame: id, theme, ratio: Number((result.ratio * 100).toFixed(3)), maskedShare: Number(((result.masked / (result.width * result.height)) * 100).toFixed(1)), aligned: result.aligned, masks: masks.map((m) => m.why) }) });
          let where = "";
          if (result.ratio > MAX_RATIO) {
            await test.info().attach("reference", { body: ref, contentType: "image/png" });
            await test.info().attach("app", { body: app, contentType: "image/png" });
            await test.info().attach("diff", { body: Buffer.from(result.diffPng.split(",")[1] ?? "", "base64"), contentType: "image/png" });
            where = await whereItDiffers({ scratch, page, browser, board: c.board, id, label: `frame ${id} (${theme})`, result });
          }
          expect(result.ratio, `frame ${id} (${theme}): share of the unmasked pixels that differ (${result.masked} of ${result.width * result.height} pixels masked)${where}`).toBeLessThanOrEqual(MAX_RATIO);
        } finally {
          await scratch.close();
        }
        expect(csp.report(), "CSP and Trusted Types violations").toBe("");
        // The same frame, unmasked, against the app's own baseline: it is what keeps the parts the design is masked from from drifting.
        await baseline(page, `${name}.png`, { mask: c.moving?.(page) ?? [] });
      });
    }
  }
}

/**
 * What the design's States specimen says of this state (tier 2). `key` are the lines of the specimen that ARE the state (its title, its
 * sentence, the names of its buttons): each must be a line of the specimen (so that a typo here cannot pass) and must be in the app's page.
 * What the app does not say is listed in `differences` with the reason; the list is compared both ways (see lib/parity.ts).
 */
export type DesignStates = { row: string; specimen: string; key: readonly string[]; differences?: readonly { text: string; why: string }[] };

export type BaselineCase = {
  /** The name of the baseline (unique in its spec file) and of the test. */
  name: string;
  scenario: string;
  path: string;
  theme?: Theme;
  viewport?: Viewport;
  /** Reach the state. */
  prepare?: Prepare;
  before?: Prepare;
  loading?: boolean;
  clock?: Clock;
  /** The state is a stream that is down: the connection pill is not live after `prepare`, and the settle does not wait for it to be. */
  streamDown?: boolean;
  /** The States specimen of this row, for the words of the state. */
  design?: DesignStates;
  /** What the state is, said in words, so that the test means something with no baseline too. */
  check: (page: Page) => Promise<void>;
  /** Regions that move on their own (besides the connection pill). */
  mask?: (page: Page) => readonly Locator[];
  /** Photograph the element and not the window. */
  element?: (page: Page) => Locator;
};

const missingBaselineIsNotRun = (path: string): boolean => baselineIsNotRun({ exists: existsSync(path), ci: Boolean(process.env["CI"]), updateSnapshots: test.info().config.updateSnapshots });

/**
 * The app against its own baseline, 0.2 per pixel and 0.5% of the pixels, the connection pill and what the state names masked.
 * Without the baseline file: a failure in CI (Playwright's own rule for a missing snapshot, which this does not soften), and
 * NOT RUN anywhere else, said in an annotation, so that a run in a sandbox never writes PNGs into the tree. LK_UPDATE_BASELINES
 * (the bootstrap job) creates it (e2e/visual/baseline-rule.mjs). Returns whether the comparison ran.
 */
export async function baseline(page: Page, file: string, options: { mask?: readonly Locator[]; element?: Locator } = {}): Promise<boolean> {
  if (missingBaselineIsNotRun(test.info().snapshotPath(file))) {
    test.info().annotations.push({ type: "baseline NOT RUN", description: `${file}: no baseline yet (baselines are generated in CI only)` });
    return false;
  }
  const masks = [...LIVE_REGIONS.map((s) => page.locator(s)), ...(options.mask ?? [])];
  const settings = { animations: "disabled", caret: "hide", threshold: 0.2, maxDiffPixelRatio: MAX_RATIO, mask: masks } as const;
  if (options.element) await expect(options.element).toHaveScreenshot(file, settings);
  else await expect(page).toHaveScreenshot(file, settings);
  return true;
}

const collapse = (text: string): string => text.replace(/\s+/g, " ").trim();

async function statesParity(page: Page, browser: import("@playwright/test").Browser, design: DesignStates): Promise<void> {
  const reason = referencesAvailable();
  if (reason !== null) {
    if (process.env["CI"]) throw new Error(`the States specimen ${design.specimen} cannot be rendered in CI: ${reason}`);
    test.info().annotations.push({ type: "tier 2 NOT RUN", description: `${design.row}: ${reason}` });
    return;
  }
  const lines = await specimenLines(browser, design.specimen);
  const notInDesign = design.key.filter((line) => !lines.includes(collapse(line)));
  expect(notInDesign, `${design.row}: key lines that are not lines of the design's specimen ${design.specimen}`).toEqual([]);
  const haystack = collapse(await page.locator("body").innerText());
  const missing = design.key.filter((line) => !haystack.includes(collapse(line)));
  const listed = (design.differences ?? []).map((d) => collapse(d.text));
  if (process.env["LK_PARITY_LOG"]) console.log(`STATES ${design.row} ${JSON.stringify(missing)}`);
  expect([...missing].map(collapse).sort(), `${design.row}: the design's words that the app does not say, against the listed differences`).toEqual([...listed].sort());
}

export function appOwned(cases: readonly BaselineCase[]): void {
  for (const c of cases) {
    const theme = c.theme ?? "light";
    test(`app-owned · ${c.name} (${theme})`, async ({ page, harness, csp, browser }) => {
      test.setTimeout(90_000);
      harness.setScenario(c.scenario);
      await pin(page, theme, c.viewport ?? { width: 1440, height: 900 }, c.clock);
      if (c.before) await c.before(page, harness);
      await page.goto(`${harness.url}${c.path}`);
      await settle(page, { loading: c.loading ?? false });
      if (c.prepare) {
        await c.prepare(page, harness);
        await settle(page, { loading: c.loading ?? false, park: false, live: !(c.streamDown ?? false) });
      }
      await c.check(page);
      if (c.design) await statesParity(page, browser, c.design);
      expect(csp.report(), "CSP and Trusted Types violations").toBe("");
      const file = `${c.name.replace(/\W+/g, "-").toLowerCase()}-${theme}.png`;
      // The checks above ran; without a baseline the test is reported skipped, so that a screenshot test that compared nothing is not a pass.
      if (!(await baseline(page, file, { mask: c.mask?.(page) ?? [], ...(c.element ? { element: c.element(page) } : {}) }))) test.skip(true, "NOT RUN: no baseline yet (baselines are generated in CI only)");
    });
  }
}
