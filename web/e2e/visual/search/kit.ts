// The kit of the visual matrix of slices S5 (search, docs, status), S6a (findings), S7 (approvals, audit) and S8 (admin): one table
// of frames per area, one runner for all of them. It lives here and not in a shared folder because these seven directories are the
// matrix's part 2 (the plan's "Visual regression: references, tiers, dataset, baselines").
//
// A frame is a board frame of the design (`src/handoff/boards.generated.ts`: its board, its id, its theme, its size and the props
// the Shell page is drawn with) and the scenario, address and steps that bring the real app to the same state. Three kinds:
//
//  - tier 1: the frame is rendered from the handoff in the same browser (e2e/visual/render-references.mjs: the design's own
//    runtime, React 18.3.1 from LK_REACT18_DIR, the clock frozen, animations off) and the app is screenshotted at the frame's size.
//    The two are compared pixel by pixel (e2e/visual/compare.mjs: threshold 0.2 per pixel, at most 0.5% of the compared pixels).
//    What is compared is the frame without its `masks` (what the design draws and the contract cannot send: each names the row of
//    design/DEVIATIONS.md) and, when `regions` is given, only those regions (a dialog against the dialog). At least MIN_COMPARED of
//    the frame must remain, so a mask cannot swallow a frame: a frame that needs more is an app baseline with its reason.
//    Without the React files the test is skipped and says "NOT RUN" with the reason; with CI set it FAILS instead, because a
//    reference that is missing must never read as a pass.
//  - app: no reference applies (the screen is the app's own, or its documented deviations make the frame incomparable):
//    `toHaveScreenshot` against a baseline that is generated in CI only (the plan's "Baseline bootstrap"). A missing baseline
//    fails in CI and is NOT RUN anywhere else (e2e/visual/baseline-rule.mjs); it never passes.
//  - state: a state of the States index (loading, empty, error, permission, stale, truncated, 409, 423) that the board has no full
//    frame for: also `toHaveScreenshot`, light mode, held or failed requests made with `page.route`.
//
// The reference is rendered as its source asks: the design's runtime keeps only position and size of the style of a <dc-import> host, so
// without a correction every page the Shell draws is as wide as its content (see openFrameReference).
//
// LK_VISUAL_OUT=<dir> writes the reference, the app and the difference of every tier 1 frame there, and one JSON line of ratios.
// LK_DIST=<dir> serves another build than `dist` (a run on a private build).
import { appendFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BrowserContext, Locator, Page } from "@playwright/test";
import { FRAMES } from "../../../src/handoff/boards.generated.ts";
import { baselineIsNotRun } from "../baseline-rule.mjs";
import { expect, test } from "../../fixtures/test";
import type { Harness } from "../../harness/serve.ts";
import { compareImages } from "../compare.mjs";
import { FROZEN_NOW, openReference, reactUmd, startReferenceServer } from "../render-references.mjs";

export type Rect = { x: number; y: number; width: number; height: number };
/** A region that is painted over in the reference and in the app. `row` names the DEVIATIONS row (or the data reason) in the spec. */
export type Mask = Rect & { row: string };
/** A region that is compared and nothing else is (a dialog against the dialog). `name` is for the report. */
export type Region = Rect & { name: string };
export type Size = { width: number; height: number };

export const BOARDS = {
  search: "Lanekeeper Search and Status.dc.html",
  findings: "Lanekeeper Findings and Adoption.dc.html",
  approvals: "Lanekeeper Approvals and Audit.dc.html",
  admin: "Lanekeeper Admin.dc.html",
} as const;
export type BoardKey = keyof typeof BOARDS;

/** The pixel rules of the plan: 0.2 per pixel (compare.mjs), 0.5% of the pixels of the frame. */
export const THRESHOLD = 0.2;
export const MAX_RATIO = 0.005;
/** The least of a frame that must be compared. A frame that needs more masks than that is not tier 1: it is an app-owned baseline with the reason. */
export const MIN_COMPARED = 0.1;

/**
 * The swimlane switcher of the header. The design draws the swimlane of the Shell page (`sitb`, its branch and its drift) on every page;
 * the app has no swimlane on a route that is not a swimlane's and says "Choose a swimlane". This is the app's shell (S0) and has no row
 * in design/DEVIATIONS.md for the routes across swimlanes (the row "Swimlane switcher and swimlane headers" is about the tenants), so
 * it is reported to the integrator; until it is decided the switcher is not compared on these frames.
 */
export const SWITCHER: Mask = { x: 864, y: 4, width: 244, height: 36, row: "header switcher: sitb in the design, \"Choose a swimlane\" in the app on a route without a swimlane" };

/** Where the design draws Audit (and Approvals) for a Viewer or Editor: the app's sidebar shows an item only to a role that may open it. */
export const HIDDEN_NAV: Mask = { x: 8, y: 296, width: 208, height: 100, row: "Sidebar: Audit and Approvals (the app hides them from a role that cannot open them; the design's Viewer and Editor frames show Audit)" };

const SHELL = "Lanekeeper Shell.dc.html";
/** What the design draws and the app does not (DEVIATIONS: the notifications bell has nothing behind it in the contract). */
const OMITTED = ["button[aria-label^='Notifications']"];

const umd = reactUmd();
const OUT = process.env["LK_VISUAL_OUT"];
const IN_CI = Boolean(process.env["CI"]);
/** Which build the harness serves: `dist`, or another directory for a run on a private build (LK_DIST). */
const DIST = process.env["LK_DIST"] ?? "dist";

/** The state the runner gives a frame's steps. */
export type Env = { page: Page; harness: Harness; size: Size };

/** A step of a frame: what is done to the app before it is photographed. */
export type Step = (env: Env) => Promise<void>;

export type FrameDef = {
  /** Which board the frame is on, and its id there ("1a"); `state` frames use a States-index code instead ("L05"). */
  board: BoardKey | "states";
  frame: string;
  kind: "tier1" | "app" | "state";
  /** The mock scenario the app starts in (src/mocks/design/**). */
  scenario: string;
  /** The address the app is opened at. */
  path: string;
  /** Steps after the page has loaded and before it is photographed: clicks, a held request, an event. */
  steps?: Step;
  /** Steps before the first navigation (route handlers that hold or fail a request). */
  before?: Step;
  /** Extra live regions to mask in an app or state baseline (the live pill is always masked). */
  mask?: (page: Page) => Locator[];
  /** What to wait for instead of "nothing is loading" (a frame that shows loading waits for its skeleton). */
  ready?: Step;
  /** A short label for the title. */
  title: string;
  /** tier1: regions painted over in both images (what the design draws and the contract cannot send). */
  masks?: readonly Mask[];
  /** tier1: when set, only these regions are compared (a dialog is compared with the dialog, not with the page under its backdrop). */
  regions?: readonly Region[];
  /** tier1: the DEVIATIONS rows the frame is known to differ by (shown in the title so a reader sees why a mask exists). */
  rows?: readonly string[];
  /** app and state: why no reference applies. */
  why?: string;
  /** The theme and size, when the frame is not on a board (state frames). */
  theme?: "light" | "dark";
  size?: Size;
  /**
   * tier1: a ratio above MAX_RATIO that the frame is held to, the plan's per-screen override ("valid only with the design owner's
   * approval"). It needs `overrideReason`, and the integrator lists every frame that has one as waiting for that approval.
   */
  maxRatio?: number;
  overrideReason?: string;
  /** Role of the viewer, for the title only. */
  note?: string;
};

type Board = { id: string; label: string; theme: "light" | "dark"; size: { w: number; h: number }; props: Record<string, string | boolean>; group: string };

function boardFrame(board: BoardKey, id: string): Board {
  const found = FRAMES.find((f) => f.board === BOARDS[board] && f.id === id);
  if (!found) throw new Error(`no frame ${id} on ${BOARDS[board]}`);
  return found as unknown as Board;
}

const camel = (key: string): string => key.replace(/-([a-z])/g, (_m, c: string) => c.toUpperCase());

/** Set the defaults of the Shell page's props in memory, as the board's `<dc-import>` sets them (the files are read-only). */
export function withPropDefaults(html: string, values: Record<string, string | boolean>): string {
  return html.replace(/data-props="([^"]*)"/, (whole, attr: string) => {
    const json = attr.replaceAll("&quot;", '"').replaceAll("&amp;", "&");
    let props: Record<string, { editor?: string; default?: unknown }>;
    try {
      props = JSON.parse(json) as typeof props;
    } catch {
      return whole;
    }
    for (const [key, raw] of Object.entries(values)) {
      // The Shell passes the attributes it does not declare (stab, adsection, aplive ...) on to the page it draws: they are read from
      // the props as the board writes them, so an undeclared one is added as a string prop with that default.
      const entry = props[camel(key)];
      if (entry && typeof entry === "object") entry.default = entry.editor === "int" ? Number(raw) : entry.editor === "boolean" ? raw === true || raw === "true" : raw;
      else props[camel(key)] = { editor: "string", default: raw, tsType: "string" } as { editor: string; default: unknown };
    }
    return `data-props="${JSON.stringify(props).replaceAll("&", "&amp;").replaceAll('"', "&quot;")}"`;
  });
}

/**
 * Keep only `keep` of a PNG (everything else is painted magenta, in the browser that decodes it) and paint `masks` over it.
 * No `keep` means the whole image.
 */
export async function paint(page: Page, png: Buffer, keep: readonly Rect[] | undefined, masks: readonly Rect[]): Promise<Buffer> {
  if (keep === undefined && masks.length === 0) return png;
  const base64 = await page.evaluate(
    async ({ b64, kept, painted }) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const context = canvas.getContext("2d");
      if (!context) throw new Error("no 2d context");
      if (kept === null) context.drawImage(bitmap, 0, 0);
      else {
        context.fillStyle = "#ff00ff";
        context.fillRect(0, 0, bitmap.width, bitmap.height);
        for (const r of kept) context.drawImage(bitmap, r.x, r.y, r.width, r.height, r.x, r.y, r.width, r.height);
      }
      context.fillStyle = "#ff00ff";
      for (const r of painted) context.fillRect(r.x, r.y, r.width, r.height);
      const blob = await canvas.convertToBlob({ type: "image/png" });
      const buffer = new Uint8Array(await blob.arrayBuffer());
      let text = "";
      for (let i = 0; i < buffer.length; i += 0x8000) text += String.fromCharCode(...buffer.subarray(i, i + 0x8000));
      return btoa(text);
    },
    { b64: png.toString("base64"), kept: keep === undefined ? null : keep.map(({ x, y, width, height }) => ({ x, y, width, height })), painted: masks.map(({ x, y, width, height }) => ({ x, y, width, height })) },
  );
  return Buffer.from(base64, "base64");
}

/** The share of the frame that is compared: the regions (or the whole frame) without the masks. Overlaps are counted once. */
export function comparedShare(regions: readonly Rect[] | undefined, masks: readonly Rect[], size: Size): number {
  const covered = new Uint8Array(size.width * size.height);
  const mark = (r: Rect, value: number): void => {
    const x0 = Math.max(0, Math.floor(r.x));
    const y0 = Math.max(0, Math.floor(r.y));
    const x1 = Math.min(size.width, Math.ceil(r.x + r.width));
    const y1 = Math.min(size.height, Math.ceil(r.y + r.height));
    for (let y = y0; y < y1; y += 1) covered.fill(value, y * size.width + x0, y * size.width + x1);
  };
  if (regions === undefined) covered.fill(1);
  else for (const r of regions) mark(r, 1);
  for (const m of masks) mark(m, 0);
  let count = 0;
  for (const v of covered) count += v;
  return count / (size.width * size.height);
}

/** Wait until the page has nothing loading: no skeleton, no busy region, and no new request for a moment. */
export async function settle(page: Page, harness: Harness): Promise<void> {
  await expect(page.locator("[data-slot='skeleton'], [aria-busy='true']")).toHaveCount(0);
  let last = -1;
  for (let i = 0; i < 20; i += 1) {
    const now = harness.requests().length;
    if (now === last) break;
    last = now;
    await page.waitForTimeout(150);
  }
  await expect(page.locator("[data-slot='skeleton'], [aria-busy='true']")).toHaveCount(0);
}

/** The photograph of the app: fonts in, pointer parked, caret and animations off. */
export async function photograph(page: Page, size: Size): Promise<Buffer> {
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(size.width - 1, size.height - 1);
  await page.waitForTimeout(250);
  return page.screenshot({ clip: { x: 0, y: 0, ...size }, animations: "disabled", caret: "hide" });
}

type ReferenceServer = Awaited<ReturnType<typeof startReferenceServer>>;

export type FrameGeometry = { theme: "light" | "dark"; size: Size; onBoard: Board | undefined };

export function geometryOf(def: FrameDef): FrameGeometry {
  const onBoard = def.board === "states" ? undefined : boardFrame(def.board, def.frame);
  const theme = onBoard?.theme ?? def.theme ?? "light";
  const size: Size = onBoard ? { width: onBoard.size.w, height: onBoard.size.h } : (def.size ?? { width: 1440, height: 900 });
  return { theme, size, onBoard };
}

/** Bring the app to the frame: scenario, size, scheme, frozen clock, the address, the steps; return when nothing is loading. */
export async function openApp(def: FrameDef, env: Env, theme: "light" | "dark"): Promise<void> {
  const { page, harness, size } = env;
  // A step that waits for something that never comes fails in seconds, not after the test's own limit.
  page.setDefaultTimeout(15_000);
  harness.setScenario(def.scenario);
  await page.setViewportSize(size);
  await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
  await page.clock.setFixedTime(FROZEN_NOW);
  await def.before?.(env);
  await page.goto(`${harness.url}${def.path}`);
  await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
  if (def.ready) {
    await def.steps?.(env);
    await def.ready(env);
    return;
  }
  await settle(page, harness);
  await def.steps?.(env);
  await settle(page, harness);
}

/** Render the frame of the design in a new page of `context`. The caller closes the context. */
export async function openFrameReference(context: BrowserContext, server: ReferenceServer, def: FrameDef): Promise<Page> {
  const { theme, size, onBoard } = geometryOf(def);
  if (!umd.ok || !onBoard) throw new Error("no reference: LK_REACT18_DIR is not set or the frame is not on a board");
  const ref = await context.newPage();
  await ref.route(/Lanekeeper%20Shell\.dc\.html/, async (route) => {
    const response = await route.fetch();
    const { theme: _theme, w: _w, h: _h, ...rest } = onBoard.props;
    await route.fulfill({ response, body: withPropDefaults(await response.text(), { ...rest, theme, w: String(size.width), h: String(size.height) }) });
  });
  await openReference(ref, server, umd, { file: SHELL, theme, viewport: size, omit: OMITTED });
  // The design's runtime keeps only position and size of the style of a <dc-import> (support.js, HOST_STYLE_PROPS), so the page
  // the Shell draws is as wide as its content (841 of 1216 px for Search) although its source says `flex:1;min-width:0`. The
  // frame is drawn as its source asks, as the app does.
  await ref.addStyleTag({ content: "main > .sc-host{flex:1 1 0%!important;min-width:0!important}" });
  await ref.evaluate(() => document.fonts.ready);
  return ref;
}

export async function startReferences(): Promise<ReferenceServer | undefined> {
  return umd.ok ? startReferenceServer({ themeFor: () => "light" }) : undefined;
}

export function referenceMissing(): string | undefined {
  return umd.ok ? undefined : umd.reason;
}

/**
 * Register the frames of one area as tests. Call once per spec file, at the top level.
 * `area` names the output folder under LK_VISUAL_OUT and the group of the tests.
 */
export function registerFrames(area: string, defs: readonly FrameDef[]): void {
  let server: ReferenceServer | undefined;
  for (const def of defs) {
    if (def.maxRatio !== undefined && !def.overrideReason) throw new Error(`frame ${def.board} ${def.frame} has a ratio override and no reason: an override needs the design owner's approval, and says why`);
  }
  test.describe(`visual matrix: ${area}`, () => {
    test.use({ dist: DIST });
    test.beforeAll(async () => {
      if (defs.some((d) => d.kind === "tier1")) server = await startReferences();
    });
    test.afterAll(async () => {
      await server?.close();
      server = undefined;
    });

    for (const def of defs) {
      const { theme, size } = geometryOf(def);
      const label = `${def.kind === "tier1" ? "tier 1" : def.kind === "app" ? "app baseline" : "state"} ${def.board === "states" ? "state " : "frame "}${def.frame} (${theme}${def.note ? `, ${def.note}` : ""}): ${def.title}`;

      test(label, async ({ page, harness, browser }) => {
        test.setTimeout(150_000);
        const tier1 = def.kind === "tier1";
        if (tier1 && !umd.ok) {
          if (IN_CI) throw new Error(`NOT RUN is not allowed in CI: ${umd.reason}`);
          test.skip(true, `NOT RUN: ${umd.reason}`);
          return;
        }
        if (!tier1) {
          // The app against its own baseline: where there is none, CI fails (Playwright refuses to create it) and a sandbox says NOT RUN.
          const file = `${area}-${def.board}-${def.frame}-${theme}.png`;
          if (baselineIsNotRun({ exists: existsSync(test.info().snapshotPath(file)), ci: IN_CI, updateSnapshots: test.info().config.updateSnapshots })) {
            test.info().annotations.push({ type: "baseline NOT RUN", description: `${file}: no baseline yet (baselines are generated in CI only)` });
            test.skip(true, "NOT RUN: no baseline yet (baselines are generated in CI only)");
            return;
          }
        }
        await openApp(def, { page, harness, size }, theme);

        if (!tier1) {
          await page.evaluate(() => document.fonts.ready);
          await page.mouse.move(size.width - 1, size.height - 1);
          await page.waitForTimeout(250);
          // The live pill is the one live region of a frame: while the stream is refused it counts down to the next try with a jitter.
          await expect(page).toHaveScreenshot(`${area}-${def.board}-${def.frame}-${theme}.png`, {
            animations: "disabled",
            caret: "hide",
            clip: { x: 0, y: 0, ...size },
            mask: [page.locator("[data-slot='live-indicator']"), ...(def.mask?.(page) ?? [])],
            threshold: THRESHOLD,
            maxDiffPixelRatio: MAX_RATIO,
          });
          return;
        }

        if (!server) return;
        const app = await photograph(page, size);
        const context = await browser.newContext({ viewport: { width: size.width, height: size.height }, timezoneId: "UTC", locale: "en-GB", colorScheme: theme, reducedMotion: "reduce" });
        try {
          const ref = await openFrameReference(context, server, def);
          const reference = await ref.screenshot({ clip: { x: 0, y: 0, ...size } });

          const masks = def.masks ?? [];
          const share = comparedShare(def.regions, masks, size);
          expect(share, `only ${(share * 100).toFixed(1)}% of the frame is compared; under ${MIN_COMPARED * 100}% is not a comparison, make it an app baseline`).toBeGreaterThanOrEqual(MIN_COMPARED);
          const refMasked = await paint(ref, reference, def.regions, masks);
          const appMasked = await paint(ref, app, def.regions, masks);
          const result = await compareImages(ref, refMasked, appMasked, { threshold: THRESHOLD });
          // The share is taken over the pixels that are compared (a pixel outside them is the same in both and is not counted).
          const compared = size.width * size.height * share;
          const ratio = compared > 0 ? result.diff / compared : 0;
          const limit = def.maxRatio ?? MAX_RATIO;
          if (OUT) {
            const dir = join(OUT, area);
            mkdirSync(dir, { recursive: true });
            const name = `${def.board}-${def.frame}-${theme}`;
            writeFileSync(join(dir, `${name}-reference.png`), refMasked);
            writeFileSync(join(dir, `${name}-app.png`), appMasked);
            writeFileSync(join(dir, `${name}-diff.png`), Buffer.from(result.diffPng.split(",")[1] ?? "", "base64"));
            appendFileSync(join(OUT, "ratios.jsonl"), `${JSON.stringify({ area, board: def.board, frame: def.frame, theme, ratio, diff: result.diff, compared: share, limit })}\n`);
          }
          test.info().annotations.push({ type: "ratio", description: JSON.stringify({ ratio, compared: share, limit, ...(def.overrideReason ? { override: def.overrideReason } : {}) }) });
          expect(ratio, `${def.board} ${def.frame}: share of pixels that differ at threshold ${THRESHOLD} (${(share * 100).toFixed(1)}% of the frame compared)`).toBeLessThanOrEqual(limit);
        } finally {
          await context.close();
        }
      });
    }
  });
}

/** A frame of a board that no test draws, and why (a row of design/DEVIATIONS.md, or the contract). */
export type Excluded = { board: BoardKey; frame: string; why: string };

/**
 * The matrix has no silent gaps: every frame id of `ids` on `board` is a test of this area or is listed as excluded with its reason, and
 * nothing is both. `ids` is the range of the area on its board (the Admin board's 1a to 1bn, the Audit part of the Approvals board's 1v
 * to 1ap, ...).
 */
export function registerCoverage(area: string, board: BoardKey, ids: readonly string[], covered: readonly FrameDef[], excluded: readonly Excluded[]): void {
  test(`visual matrix: ${area}: every frame of ${ids[0]} to ${ids[ids.length - 1]} on the ${board} board is tested or excluded with a reason`, () => {
    const tested = new Set(covered.filter((d) => d.board === board).map((d) => d.frame));
    const left = new Set(excluded.filter((d) => d.board === board).map((d) => d.frame));
    for (const id of ids) {
      boardFrame(board, id);
      expect(tested.has(id) && left.has(id), `${board} ${id} is both tested and excluded`).toBe(false);
      expect(tested.has(id) || left.has(id), `${board} ${id} is neither tested nor excluded`).toBe(true);
    }
    for (const id of [...tested, ...left]) expect(ids, `${board} ${id} is not in the range of ${area}`).toContain(id);
    for (const e of excluded) expect(e.why.length, `${board} ${e.frame}: an exclusion has a reason`).toBeGreaterThan(20);
  });
}

/** The ids of a board from one to another, in the board's order (the ids run 1a, 1b, ... 1z, 1aa, ...). */
export function idsBetween(board: BoardKey, from: string, to: string): string[] {
  const ids = FRAMES.filter((f) => f.board === BOARDS[board]).map((f) => f.id as string);
  const a = ids.indexOf(from);
  const b = ids.indexOf(to);
  if (a < 0 || b < a) throw new Error(`no range ${from} to ${to} on ${board}`);
  return ids.slice(a, b + 1);
}
