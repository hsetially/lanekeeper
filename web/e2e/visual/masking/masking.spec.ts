// Masking (T5, slice C): the frames of frames.ts as app-owned baselines, and the one part of the File view board's frame 1f that the app draws as
// the design does: the revealed value in its line, with Hide and "Revealed · recorded in the audit log".
//
// The line is compared in pieces (the value, the Hide button, the note), each photographed on a stage in the design's own page and in the app's
// (e2e/visual/attribution/elements.ts says why), dark as the frame is. What differs by a documented row is not compared: `differs` names the row and
// the test asserts that the piece still differs. Without the React files the test is NOT RUN, and in CI it fails.
import { expect, test } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { signedInAs } from "../access/lib/steps";
import { MAX_RATIO, appPieces, comparePieces, designPieces, expectSamePiece, umd, type DesignPick } from "../attribution/elements";
import { openApp, registerFrames, type FrameDef } from "../search/kit";
import { frames, REASON } from "./frames";

registerFrames("masking", frames);

const IN_CI = Boolean(process.env["CI"]);
const FILE_VIEW = "Lanekeeper File View.dc.html";
const FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml";
test.use({ dist: process.env["LK_DIST"] ?? "dist" });

type Boxes = Record<string, { x: number; y: number; width: number; height: number }>;

/** In frame 1f: the revealed value, the Hide button and the note, each copied to a stage with the text properties of the place it came from. */
const designRevealed: DesignPick = (frame) => {
  const stage = document.createElement("div");
  Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", width: "max-content", background: "var(--background)", color: "var(--foreground)" });
  frame.appendChild(stage);
  const out: Boxes = {};
  const stageCopy = (name: string, source: Element, keepOnly?: (clone: Element) => void): void => {
    const cs = getComputedStyle(source.parentElement ?? source);
    const wrapper = document.createElement("div");
    Object.assign(wrapper.style, { fontFamily: cs.fontFamily, fontSize: cs.fontSize, lineHeight: cs.lineHeight, fontWeight: cs.fontWeight, letterSpacing: cs.letterSpacing, whiteSpace: cs.whiteSpace, display: "flex" });
    const clone = source.cloneNode(true) as Element;
    wrapper.appendChild(clone);
    stage.appendChild(wrapper);
    keepOnly?.(clone);
    const r = clone.getBoundingClientRect();
    // The note is a line of text: the design's line box (the code line's 18 px) is two pixels taller than the app's (`text-xs`, 16 px), with the
    // text in the middle of both, so the line of text is the 16 px in the middle.
    out[name] = name === "note" ? { x: r.x, y: r.y + (r.height - 16) / 2, width: r.width, height: 16 } : { x: r.x, y: r.y, width: r.width, height: r.height };
  };
  const value = Array.from(frame.querySelectorAll("span")).find((el) => (el.textContent ?? "").trim() === "tellerClient-9f2c71" && el.children.length === 1);
  const hide = Array.from(frame.querySelectorAll("button")).find((el) => (el.textContent ?? "").trim() === "Hide");
  if (value !== undefined) stageCopy("value", value);
  if (hide !== undefined) stageCopy("hide", hide);
  const holder = hide?.parentElement;
  if (holder !== null && holder !== undefined) {
    stageCopy("note", holder, (clone) => {
      for (const button of Array.from(clone.querySelectorAll("button"))) button.remove();
    });
  }
  return out;
};

const appRevealed = (page: Page): Promise<Boxes> =>
  page.evaluate(() => {
    const stage = document.createElement("div");
    Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", width: "max-content", background: "var(--background)", color: "var(--foreground)" });
    document.body.appendChild(stage);
    const out: Record<string, { x: number; y: number; width: number; height: number }> = {};
    const stageCopy = (name: string, source: Element | null): void => {
      if (source === null) return;
      const cs = getComputedStyle(source.parentElement ?? source);
      const wrapper = document.createElement("div");
      Object.assign(wrapper.style, { fontFamily: cs.fontFamily, fontSize: cs.fontSize, lineHeight: cs.lineHeight, fontWeight: cs.fontWeight, letterSpacing: cs.letterSpacing, whiteSpace: cs.whiteSpace, display: "flex" });
      const clone = source.cloneNode(true) as Element;
      wrapper.appendChild(clone);
      stage.appendChild(wrapper);
      const r = clone.getBoundingClientRect();
      out[name] = { x: r.x, y: r.y, width: r.width, height: r.height };
    };
    stageCopy("value", document.querySelector("[data-slot='revealed-value']"));
    stageCopy("hide", Array.from(document.querySelectorAll("button")).find((el) => (el.textContent ?? "").trim() === "Hide") ?? null);
    stageCopy("note", Array.from(document.querySelectorAll("span")).find((el) => (el.textContent ?? "").trim() === "Revealed · recorded in the audit log" && el.children.length === 0) ?? null);
    return out;
  });

const DIFFERS: Readonly<Record<string, string>> = {
  value: "design/DEVIATIONS.md: \"File: revealed value\": the value has 4 px of padding on each side, the design's has none",
  hide: "design/DEVIATIONS.md: \"File: revealed value\": Hide is the app's secondary button (its own fill), the design's is the page's background",
};

test("tier 1 · File View 1f (dark): the revealed value, Hide and the note are the design's, but for the listed differences", async ({ page, harness, browser, csp }) => {
  test.setTimeout(150_000);
  if (!umd.ok) {
    if (IN_CI) throw new Error(`NOT RUN is not allowed in CI: ${umd.reason ?? "no React files"}`);
    test.skip(true, `NOT RUN: ${umd.reason ?? "no React files"}`);
    return;
  }
  const def: FrameDef = {
    board: "states", frame: "E", kind: "app", title: "revealed", why: "revealed", scenario: "design-masking-reveal", path: FILE,
    before: async ({ page: p }) => {
      await signedInAs(p, "editor");
    },
    steps: async ({ page: p }) => {
      await p.getByRole("button", { name: /^Reveal value of / }).first().click();
      await p.getByRole("alertdialog").getByRole("textbox").fill(REASON);
      await p.getByRole("alertdialog").getByRole("button", { name: "Reveal value" }).click();
      await p.getByRole("button", { name: "Hide" }).waitFor();
    },
  };
  await openApp(def, { page, harness, size: { width: 1440, height: 900 } }, "dark");
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1439, 899);
  const design = await designPieces(browser, FILE_VIEW, "1f", "dark", "revealed", designRevealed);
  const app = await appPieces(page, appRevealed);
  const scratch = await browser.newPage();
  try {
    const names = ["value", "hide", "note"];
    const compared = await comparePieces(scratch, design, app, names, "File View 1f revealed");
    test.info().annotations.push({ type: "pieces", description: JSON.stringify(compared.map((c) => ("error" in c ? { name: c.name, error: c.error } : { name: c.name, ratio: Number((c.ratio * 100).toFixed(3)), size: `${c.width}x${c.height}` }))) });
    for (const piece of compared) {
      const row = DIFFERS[piece.name];
      if (row === undefined) {
        await expectSamePiece(piece, { scratch, design, app, group: "File View 1f revealed" });
      } else {
        const differs = "error" in piece || piece.ratio > MAX_RATIO;
        expect.soft(differs, `${piece.name} is now the same as the design's, so the row "${row}" is out of date: compare the piece`).toBe(true);
      }
    }
  } finally {
    await scratch.close();
  }
  expect(csp.report(), "CSP and Trusted Types violations").toBe("");
});
