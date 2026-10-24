// The references of tier 1: frames of the design handoff's boards, rendered in the browser the app runs in (never exported
// images). A board file (`Lanekeeper Swimlanes.dc.html`, ...) holds one `<dc-import>` per frame, each with its own props
// (page, theme, dataset, role, ...), so the frame is read from the board itself and the props are the design's, not ours.
//
//  - The files come from `design/handoff/screens`, served read only by e2e/visual/render-references.mjs (S0), with no CSP and none
//    of the app's code. React 18.3.1 comes from `LK_REACT18_DIR` (its SHA-384 is checked against support.js there). Without it
//    nothing can be rendered: `referencesAvailable()` says why, and a spec reports NOT RUN, never a pass.
//  - A frame is the first child of its `.sc-host`: `[data-theme]` div of exactly the frame's size. Before it is screenshotted its
//    `--background` is read and must be the token of the theme the board says (so a light frame can never be taken for a dark one),
//    and its size must be the one the board says.
//  - The clock is frozen, animations are off, and the three normalisations of S0 apply (body margin 0, buttons inherit the font, the
//    notifications bell the app does not have is removed: DEVIATIONS "Shell: notifications bell").
//  - Rendering a board costs a few seconds (every frame of it is drawn), so the PNGs are kept in `e2e/visual/.refs/v1-<hash>/`
//    (gitignored), keyed by the hash of every handoff file, the React files and this module's version: a changed handoff renders
//    again. `LK_VISUAL_REFRESH=1` renders again whatever is there.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import { FRAMES } from "../../../../src/handoff/boards.generated.ts";
import { elementProbe } from "../../diff-report.mjs";
import { BACKGROUND, FROZEN_NOW, SCREENS_DIR, reactUmd, startReferenceServer } from "../../render-references.mjs";

export type Theme = "light" | "dark";
export type BoardFrame = (typeof FRAMES)[number];

/** Bump when the way a frame is rendered changes, so that the cached PNGs are rendered again. */
const RENDER_VERSION = "v1-6";
const REFS_DIR = fileURLToPath(new URL("../../.refs/", import.meta.url));

/** The two React files. `ok: false` says why there are none. Evaluated once per worker. */
export const umd = reactUmd() as { ok: true; react: string; reactDom: string } | { ok: false; reason?: string };

/** Why a reference cannot be rendered, or `null` when it can. */
export function referencesAvailable(): string | null {
  return umd.ok ? null : (umd.reason ?? "the React 18 files are not available");
}

/** What the design does not have a counterpart for in the app, removed from every reference before it is photographed. */
const OMITTED = ["button[aria-label^='Notifications']"];

export function frameOf(board: string, id: string): BoardFrame {
  const found = FRAMES.find((f) => f.board === board && f.id === id);
  if (!found) throw new Error(`the board ${board} has no frame ${id}`);
  return found;
}

function inputsHash(): string {
  const hash = createHash("sha256");
  hash.update(RENDER_VERSION);
  hash.update(String(FROZEN_NOW));
  hash.update(OMITTED.join("|"));
  for (const name of readdirSync(SCREENS_DIR).sort()) {
    if (!/\.(html|css|js)$/.test(name)) continue;
    hash.update(name);
    hash.update(readFileSync(join(SCREENS_DIR, name)));
  }
  if (umd.ok) {
    hash.update(umd.react);
    hash.update(umd.reactDom);
  }
  return hash.digest("hex").slice(0, 12);
}

let hashed: string | undefined;
const cacheDir = (): string => join(REFS_DIR, `v1-${(hashed ??= inputsHash())}`);
const slug = (board: string): string => board.replace(/^Lanekeeper /, "").replace(/\.dc\.html$/, "").replace(/[^A-Za-z0-9]+/g, "-").toLowerCase();
const cachePath = (board: string, id: string): string => join(cacheDir(), `${slug(board)}--${id}.png`);

function boardIds(board: string): string[] {
  // A frame that is a live prototype (the Consequence Dialogs board's 1a, with switches of its own) is not a specimen: it is not rendered.
  return FRAMES.filter((f) => f.board === board && !f.dynamic).map((f) => f.id);
}

/** The first element of the first host under the frame's slot: the board's `.dv-card` (a page) or its `.stage` (a dialog specimen). */
const frameSelector = (id: string): string => `[id='${id}'] .sc-host > *`;

/** A page of `context` with the board open and every frame of `ids` drawn, normalised as the references are (S0). */
async function openBoard(context: BrowserContext, server: { origin: string }, board: string, ids: readonly string[]): Promise<Page> {
  if (!umd.ok) throw new Error(`cannot render a reference: ${umd.reason ?? "no React"}`);
  const page = await context.newPage();
  await page.clock.setFixedTime(FROZEN_NOW);
  await page.addInitScript(
    ({ react, reactDom }) => {
      // support.js uses window.React and window.ReactDOM when they are there, and asks unpkg when they are not.
      (0, eval)(react);
      (0, eval)(reactDom);
    },
    { react: umd.react, reactDom: umd.reactDom },
  );
  await page.goto(`${server.origin}/${encodeURIComponent(board)}`);
  await page.waitForFunction(
    (selectors) => selectors.every((selector) => {
      const element = document.querySelector(selector);
      return element !== null && element.getBoundingClientRect().height > 100;
    }),
    ids.map(frameSelector),
    { timeout: 60_000 },
  );
  await page.addStyleTag({ content: "html,body{margin:0!important}button{font:inherit}*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
  for (const selector of OMITTED) await page.evaluate((sel) => document.querySelectorAll(sel).forEach((el) => el.remove()), selector);
  await page.evaluate(() => document.fonts.ready);
  return page;
}

/** Render every frame of a board, write the PNGs to the cache, and return nothing: the caller reads them back. */
async function renderBoard(browser: Browser, board: string): Promise<void> {
  if (!umd.ok) throw new Error(`cannot render a reference: ${umd.reason}`);
  const ids = boardIds(board);
  const server = await startReferenceServer();
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, timezoneId: "UTC", locale: "en-GB", colorScheme: "light", reducedMotion: "reduce" });
  try {
    const page = await openBoard(context, server, board, ids);
    mkdirSync(cacheDir(), { recursive: true });
    for (const id of ids) {
      const frame = frameOf(board, id);
      const element = page.locator(frameSelector(id)).first();
      const token = await element.evaluate((el) => getComputedStyle(el).getPropertyValue("--background").trim().toLowerCase());
      const theme: Theme | "" = frame.theme;
      // A board's own labelled frames name their theme; the Consequence Dialogs board's specimens have none and stay as they are.
      if (theme && token !== BACKGROUND[theme]) throw new Error(`frame ${id} of ${board} is ${theme} and has --background ${token}, not ${BACKGROUND[theme]}`);
      const box = await element.boundingBox();
      // A page frame has the size its board says; a dialog specimen (no theme) is as tall as its content, and the board's size is a hint.
      if (!box || (theme && (Math.round(box.width) !== frame.size.w || Math.round(box.height) !== frame.size.h))) {
        throw new Error(`frame ${id} of ${board} is ${box ? `${Math.round(box.width)}x${Math.round(box.height)}` : "not drawn"}, the board says ${frame.size.w}x${frame.size.h}`);
      }
      // The board lays the frames out with fractional offsets (the label above each has a line height of 15.6 px), which would put the
      // frame's own pixels half a pixel off the app's grid. Each frame is moved to the window's corner for its photograph.
      const style = await element.evaluate((el) => {
        const before = el.getAttribute("style") ?? "";
        el.setAttribute("style", `${before};position:fixed!important;left:0!important;top:0!important;margin:0!important;z-index:2147483647!important`);
        return before;
      });
      const png = await element.screenshot({ animations: "disabled", caret: "hide" });
      // What a reader reads in the frame, for the text parity of tier 2 (the visible text, as the browser lays it out).
      const text = await element.evaluate((el) => (el as HTMLElement).innerText);
      await element.evaluate((el, before) => el.setAttribute("style", before), style);
      const target = cachePath(board, id);
      // Written beside and renamed, so that a worker reading it never sees half a file.
      writeFileSync(`${target}.${process.pid}.tmp`, png);
      renameSync(`${target}.${process.pid}.tmp`, target);
      writeFileSync(`${target}.${process.pid}.txt.tmp`, text);
      renameSync(`${target}.${process.pid}.txt.tmp`, `${target.replace(/\.png$/, "")}.txt`);
    }
  } finally {
    await context.close();
    await server.close();
  }
}

/** The reference PNG of one frame of one board: from the cache, or rendered (the whole board) and cached. */
export async function reference(browser: Browser, board: string, id: string): Promise<Buffer> {
  frameOf(board, id);
  const path = cachePath(board, id);
  if (process.env["LK_VISUAL_REFRESH"] === "1" && !refreshed.has(board)) {
    refreshed.add(board);
    await renderBoard(browser, board);
  } else if (!existsSync(path)) {
    await renderBoard(browser, board);
  }
  return readFileSync(path);
}

const refreshed = new Set<string>();

/**
 * The design's page of one frame, open, to ask what is at a point of it (e2e/visual/diff-report.mjs): only for the report of a frame that
 * failed its comparison, because the references are kept as pictures and their page is closed. It opens the board again (a few seconds),
 * moves the frame to the window's corner as its photograph was taken, and answers for the frame's own pixels. `close` ends it.
 */
export async function probeReference(browser: Browser, board: string, id: string): Promise<{ probe: ReturnType<typeof elementProbe>; close: () => Promise<void> }> {
  const frame = frameOf(board, id);
  const server = await startReferenceServer();
  const context = await browser.newContext({ viewport: { width: Math.max(1600, frame.size.w), height: Math.max(1000, frame.size.h) }, timezoneId: "UTC", locale: "en-GB", colorScheme: "light", reducedMotion: "reduce" });
  const close = async (): Promise<void> => {
    await context.close();
    await server.close();
  };
  try {
    const page = await openBoard(context, server, board, boardIds(board));
    await page.locator(frameSelector(id)).first().evaluate((el) => {
      el.setAttribute("style", `${el.getAttribute("style") ?? ""};position:fixed!important;left:0!important;top:0!important;margin:0!important;z-index:2147483647!important`);
    });
    return { probe: elementProbe(page), close };
  } catch (error) {
    await close();
    throw error;
  }
}

/** The visible text of a frame, one line per line the browser laid out, as it reads in the reference (rendered with the PNG). */
export async function referenceText(browser: Browser, board: string, id: string): Promise<string> {
  await reference(browser, board, id);
  return readFileSync(cachePath(board, id).replace(/\.png$/, ".txt"), "utf8");
}

/**
 * The States board (`Lanekeeper States.dc.html`): one specimen per row of the States index, `#s-<state>-<screen>` (`s-error-file` is R03), each
 * drawn in light and in dark. What is kept is the light one's visible text, one entry per line, to read the words of a state in the design. The
 * board is rendered once and kept with the references.
 */
export async function specimenLines(browser: Browser, id: string): Promise<string[]> {
  if (!umd.ok) throw new Error(`cannot render a reference: ${umd.reason ?? "no React"}`);
  const file = join(cacheDir(), "states-specimens.json");
  if (!existsSync(file) || process.env["LK_VISUAL_REFRESH"] === "1") {
    const server = await startReferenceServer();
    const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, timezoneId: "UTC", locale: "en-GB", colorScheme: "light", reducedMotion: "reduce" });
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
      await page.goto(`${server.origin}/${encodeURIComponent("Lanekeeper States.dc.html")}`);
      await page.waitForSelector("[id='s-error-file']", { timeout: 60_000 });
      await page.evaluate(() => document.fonts.ready);
      const specimens = await page.evaluate(() => {
        const out: Record<string, string> = {};
        for (const element of document.querySelectorAll<HTMLElement>("[id^='s-'][id*='-']")) {
          const parts = element.id.split("-");
          if (parts.length < 3) continue;
          const text = element.innerText;
          // "R03 / File / Open screen / LIGHT / ... / DARK / ...": the words of the light one.
          const light = text.split("\nDARK\n")[0]?.split("\nLIGHT\n")[1] ?? text;
          out[element.id] = light;
        }
        return out;
      });
      mkdirSync(cacheDir(), { recursive: true });
      writeFileSync(`${file}.${process.pid}.tmp`, JSON.stringify(specimens));
      renameSync(`${file}.${process.pid}.tmp`, file);
    } finally {
      await context.close();
      await server.close();
    }
  }
  const all = JSON.parse(readFileSync(file, "utf8")) as Record<string, string>;
  const text = all[id];
  if (text === undefined) throw new Error(`the States board has no specimen ${id}`);
  return text.split("\n").map((line) => line.replace(/\s+/g, " ").trim()).filter((line) => line !== "");
}
