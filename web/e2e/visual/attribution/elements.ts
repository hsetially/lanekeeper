// Tier 1 for the pieces of the attribution chip (T5, slice A), compared with the design's own pieces pixel by pixel.
//
// The chip's screens cannot be compared whole: the scenario's rows are not the design's, and the screens differ by documented rows (see
// frames.ts). What can be compared is what the screens are made of: the words "Marco Diaz · via Lanekeeper", the confidence meter beside
// them, the severity badge, the "unknown author" badge. The design's page is rendered in the same browser as the app (React 18.3.1 from
// LK_REACT18_DIR, the clock frozen, animations off: e2e/visual/render-references.mjs), the element is found in the design's DOM, and its
// box is photographed there and in the app. The two photographs must be the same size and differ in at most 0.5% of their pixels at a
// threshold of 0.2 per pixel (e2e/visual/compare.mjs), as every tier-1 frame does.
//
// The pieces are photographed on a STAGE, not where they sit: a copy of each is put at the window's corner, in the page it came from (so its
// classes, the theme's tokens and the font are the ones it has), at whole-pixel coordinates. Where a piece sits on its page decides at
// what fraction of a pixel its text is drawn (a row at x = 1226.5 and one at x = 1281.8 anti-alias the same word differently), and that is
// the position of the row, not the piece. A piece's size and its look are what is compared.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Browser, Page } from "@playwright/test";
import { FRAMES } from "../../../src/handoff/boards.generated.ts";
import { compareImages } from "../compare.mjs";
import { FROZEN_NOW, reactUmd, startReferenceServer } from "../render-references.mjs";

export type Box = { x: number; y: number; width: number; height: number };
export const THRESHOLD = 0.2;
export const MAX_RATIO = 0.005;

export const umd = reactUmd() as { ok: true; react: string; reactDom: string } | { ok: false; reason?: string };

/** What a function that runs in the design's page returns for a frame: the boxes of the pieces, by name, from the frame's corner. */
export type DesignPick = (frame: Element) => Record<string, { x: number; y: number; width: number; height: number }>;

/** The first element of the first host of a frame of a board, as references are photographed (e2e/visual/access/lib/reference.ts). */
const frameSelector = (id: string): string => `[id='${id}'] .sc-host > *`;

const cache = new Map<string, Promise<Map<string, Buffer>>>();

/**
 * The photographs of the pieces of one frame of the design: `pick` runs in the design's page, with the frame moved to the window's corner
 * (so a box is in the frame's own coordinates), and each box it names is photographed. Rendered once per worker and kept under `what`.
 */
export function designPieces(browser: Browser, board: string, id: string, theme: "light" | "dark", what: string, pick: DesignPick, root?: string): Promise<Map<string, Buffer>> {
  const key = `${board}|${id}|${theme}|${what}`;
  let found = cache.get(key);
  if (found === undefined) {
    found = render(browser, board, id, theme, pick, root);
    cache.set(key, found);
  }
  return found;
}

async function render(browser: Browser, board: string, id: string, theme: "light" | "dark", pick: DesignPick, root: string | undefined): Promise<Map<string, Buffer>> {
  if (!umd.ok) throw new Error(`cannot render a reference: ${umd.reason ?? "no React"}`);
  // `root` is a selector for a part of a board that is not a frame of its own (the Consequence Dialogs board's frame 1l is a card drawn
  // in the board itself); it must carry the theme's tokens, which is checked below as for a frame.
  const selector = root ?? frameSelector(id);
  if (root === undefined && !FRAMES.find((f) => f.board === board && f.id === id)) throw new Error(`the board ${board} has no frame ${id}`);
  const server = await startReferenceServer();
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, timezoneId: "UTC", locale: "en-GB", colorScheme: theme, reducedMotion: "reduce" });
  try {
    const page = await context.newPage();
    await page.clock.setFixedTime(FROZEN_NOW);
    await page.addInitScript(
      ({ react, reactDom }) => {
        (0, eval)(react);
        (0, eval)(reactDom);
      },
      { react: umd.react, reactDom: umd.reactDom },
    );
    await page.goto(`${server.origin}/${encodeURIComponent(board)}`);
    // A dark frame is a board's own: the whole board is drawn with the frame's props, and its `--background` says which theme it is.
    await page.waitForFunction((target) => (document.querySelector(target)?.getBoundingClientRect().height ?? 0) > 100, selector, { timeout: 60_000 });
    await page.addStyleTag({ content: "html,body{margin:0!important}button{font:inherit}*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
    await page.evaluate(() => document.fonts.ready);
    const element = page.locator(selector).first();
    const token = await element.evaluate((el) => getComputedStyle(el).getPropertyValue("--background").trim().toLowerCase());
    const wanted = theme === "dark" ? "#0e1116" : "#ffffff";
    if (token !== wanted) throw new Error(`frame ${id} of ${board} was drawn with --background ${token}, not ${wanted} (${theme})`);
    await element.evaluate((el) => {
      el.setAttribute("style", `${el.getAttribute("style") ?? ""};position:fixed!important;left:0!important;top:0!important;margin:0!important;z-index:2147483647!important`);
    });
    const boxes = await element.evaluate(pick);
    const out = new Map<string, Buffer>();
    for (const [name, box] of Object.entries(boxes)) out.set(name, await page.screenshot({ clip: clipOf(box), animations: "disabled", caret: "hide" }));
    return out;
  } finally {
    await context.close();
    await server.close();
  }
}

/** The pixels a box paints: its corners rounded, as the browser snaps a box to the pixel grid. */
export function clipOf(box: Box): Box {
  const x = Math.round(box.x);
  const y = Math.round(box.y);
  return { x, y, width: Math.round(box.x + box.width) - x, height: Math.round(box.y + box.height) - y };
}

/** Photograph the boxes of the app's pieces, by name. `pick` runs in the app's page and returns the boxes in the window's coordinates. */
export async function appPieces(page: Page, pick: (page: Page) => Promise<Record<string, Box>>): Promise<Map<string, Buffer>> {
  const boxes = await pick(page);
  const out = new Map<string, Buffer>();
  for (const [name, box] of Object.entries(boxes)) out.set(name, await page.screenshot({ clip: clipOf(box), animations: "disabled", caret: "hide" }));
  return out;
}

export type Piece = { name: string; ratio: number; diff: number; width: number; height: number } | { name: string; error: string };

/** LK_VISUAL_OUT=<dir> keeps the design's photograph, the app's and the difference of every piece there, to look at (as e2e/visual/search/kit.ts does for a frame). */
const OUT = process.env["LK_VISUAL_OUT"];

/** Compare each named piece of the design with the same named piece of the app. A piece that is not the same size is a difference, not an error. */
export async function comparePieces(page: Page, design: Map<string, Buffer>, app: Map<string, Buffer>, names: readonly string[], group = "pieces"): Promise<Piece[]> {
  const out: Piece[] = [];
  for (const name of names) {
    const a = design.get(name);
    const b = app.get(name);
    if (a === undefined || b === undefined) {
      out.push({ name, error: `${a === undefined ? "the design" : "the app"} has no piece called ${name}` });
      continue;
    }
    try {
      const result = await compareImages(page, a, b, { threshold: THRESHOLD });
      out.push({ name, ratio: result.ratio, diff: result.diff, width: result.width, height: result.height });
      keep(group, name, a, b, Buffer.from(result.diffPng.split(",")[1] ?? "", "base64"));
    } catch (error) {
      keep(group, name, a, b, undefined);
      out.push({ name, error: error instanceof Error ? error.message.replace(/^page\.evaluate: Error: /, "") : String(error) });
    }
  }
  return out;
}

function keep(group: string, name: string, design: Buffer, app: Buffer, diff: Buffer | undefined): void {
  if (!OUT) return;
  const dir = join(OUT, "attribution-pieces", group.replace(/\W+/g, "-").toLowerCase().slice(0, 60));
  mkdirSync(dir, { recursive: true });
  const base = name.replace(/\W+/g, "-").toLowerCase();
  writeFileSync(join(dir, `${base}-design.png`), design);
  writeFileSync(join(dir, `${base}-app.png`), app);
  if (diff) writeFileSync(join(dir, `${base}-diff.png`), diff);
}
