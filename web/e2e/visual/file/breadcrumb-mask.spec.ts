// The mask of File View frame 1m's breadcrumb (masks.ts), read against the two pages it is for and against the comparison it is part of.
//
// Frame 1m differs from the design at the breadcrumb: the app names every folder of the file's path and the design only the parent folder
// (design/DEVIATIONS.md, "Header: switcher and breadcrumb off a swimlane's own pages"). Those pixels are 87% to 88% of the frame's
// differences and how many cross the threshold depends on the machine's text anti-aliasing (1,264 of 255,104 compared pixels on the
// sandbox, 0.495%; 1,423 on the runner of the bootstrap job, 0.558%, over the limit of 0.5%). The frame is held to the limit as before
// (MAX_RATIO and the threshold are not touched); what is not compared is the breadcrumb, and only it. These tests say what "only it" means:
//  - the mask covers the breadcrumb of the app and of the design, a few pixels over, and no other box of either page is inside it;
//  - it is one of the frame's masks, and the frame has no other new one;
//  - a visible change inside it changes nothing in the comparison, and a visible change outside it, next to it or far from it, still does.
import type { Page } from "@playwright/test";
import { expect, test } from "../../fixtures/test";
import type { Harness } from "../../harness/serve.ts";
import { pin, settle } from "../access/lib/app";
import { MAX_RATIO, compareFrames, type Mask } from "../access/lib/compare";
import { frameOf, reference, referencePage, referencesAvailable, type Theme } from "../access/lib/reference";
import { FRAME_1M } from "./frame-1m";
import { BREADCRUMB, CHROME_ONLY } from "./masks";

test.use({ dist: process.env["LK_DIST"] ?? "dist" });

const IN_CI = Boolean(process.env["CI"]);
const ROW = "Header: switcher and breadcrumb off a swimlane's own pages";

type Survey = { nav: { x: number; y: number; right: number; bottom: number }; inside: string[] };

/** In a page: the box of the breadcrumb under `root`, and the elements of `root` that are not the breadcrumb, inside it or around it, whose box meets `rect`. */
const survey = (args: { root: string; rect: { x: number; y: number; width: number; height: number } }): Survey => {
  const root = document.querySelector(args.root);
  const nav = root?.querySelector("nav[aria-label='Breadcrumb']");
  if (!root || !nav) throw new Error(`no breadcrumb under ${args.root}`);
  const n = nav.getBoundingClientRect();
  const r = args.rect;
  const inside: string[] = [];
  for (const el of Array.from(root.querySelectorAll("*"))) {
    if (nav.contains(el) || el.contains(nav)) continue;
    const b = el.getBoundingClientRect();
    if (b.width === 0 || b.height === 0) continue;
    if (b.right > r.x && b.left < r.x + r.width && b.bottom > r.y && b.top < r.y + r.height) inside.push(`<${el.tagName.toLowerCase()}> ${el.getAttribute("data-slot") ?? ""} ${Math.round(b.x)},${Math.round(b.y)} ${Math.round(b.width)}x${Math.round(b.height)}`);
  }
  return { nav: { x: n.x, y: n.y, right: n.right, bottom: n.bottom }, inside };
};

/** The app in the state of frame 1m, as the matrix reaches it (e2e/visual/access/lib/matrix.ts): the scenario, the size of the frame, the route. */
async function openFrame({ page, harness }: { page: Page; harness: Harness }): Promise<void> {
  const frame = frameOf(FRAME_1M.board, "1m");
  harness.setScenario(FRAME_1M.scenario);
  await pin(page, frame.theme as Theme, FRAME_1M.viewport ?? { width: frame.size.w, height: frame.size.h });
  await page.goto(`${harness.url}${FRAME_1M.path}`);
  await settle(page);
}

const skipWithoutReferences = (): boolean => {
  const missing = referencesAvailable();
  if (missing === null) return false;
  if (IN_CI) throw new Error(`the reference of frame 1m cannot be rendered in CI: ${missing}`);
  test.skip(true, `NOT RUN: ${missing}`);
  return true;
};

test.describe("File View 1m: the breadcrumb mask", () => {
  test("it is the breadcrumb of the app and of the design, a few pixels over, and nothing else of either page", async ({ page, harness, browser }) => {
    test.setTimeout(120_000);
    if (skipWithoutReferences()) return;
    await openFrame({ page, harness });
    const app = await page.evaluate(survey, { root: "body", rect: BREADCRUMB });
    const design = await referencePage(browser, FRAME_1M.board, "1m");
    let ref: Survey;
    try {
      ref = await design.page.evaluate(survey, { root: "[id='1m'] .sc-host > *", rect: BREADCRUMB });
    } finally {
      await design.close();
    }
    for (const [side, found] of [["app", app], ["design", ref]] as const) {
      const over = { left: found.nav.x - BREADCRUMB.x, top: found.nav.y - BREADCRUMB.y, right: BREADCRUMB.x + BREADCRUMB.width - found.nav.right, bottom: BREADCRUMB.y + BREADCRUMB.height - found.nav.bottom };
      // Covers the breadcrumb, with room for the anti-aliasing of its glyphs (at least 1 px, at most 4 px) and no more.
      for (const [edge, px] of Object.entries(over)) {
        expect(px, `${side}: the mask is ${px.toFixed(1)} px over the breadcrumb's ${edge} edge`).toBeGreaterThanOrEqual(1);
        expect(px, `${side}: the mask is ${px.toFixed(1)} px over the breadcrumb's ${edge} edge`).toBeLessThanOrEqual(4);
      }
      expect(found.inside, `${side}: boxes other than the breadcrumb's that the mask meets`).toEqual([]);
    }
    // The mask is in the top bar, between the sidebar and the swimlane switcher, and above the page under it, which CHROME_ONLY masks.
    expect(BREADCRUMB.x).toBeGreaterThanOrEqual(224);
    expect(BREADCRUMB.y + BREADCRUMB.height).toBeLessThanOrEqual(CHROME_ONLY.y);
  });

  test("it names the row of design/DEVIATIONS.md, and it is one of the frame's masks together with the one it had", () => {
    expect(BREADCRUMB.why).toContain(`design/DEVIATIONS.md: "${ROW}"`);
    expect(FRAME_1M.masks).toEqual([CHROME_ONLY, BREADCRUMB]);
    expect(FRAME_1M.ids).toEqual(["1m"]);
  });

  test("a visible change inside the mask is not counted; a visible change outside it, next to it or far from it, still is", async ({ page, harness, browser }) => {
    test.setTimeout(150_000);
    if (skipWithoutReferences()) return;
    await openFrame({ page, harness });
    const masks = (FRAME_1M.masks ?? []).filter((m): m is Mask => typeof m !== "function");
    expect(masks.length, "every mask of the frame is a fixed rectangle").toBe(FRAME_1M.masks?.length);
    const ref = await reference(browser, FRAME_1M.board, "1m");
    const shoot = (): Promise<Buffer> => page.screenshot({ animations: "disabled", caret: "hide" });
    const scratch = await browser.newPage();
    try {
      const before = await shoot();
      const unmasked = [CHROME_ONLY];
      const base = await compareFrames(scratch, ref, before, masks);
      const baseUnmasked = await compareFrames(scratch, ref, before, unmasked);

      // Inside the mask: the whole breadcrumb disappears. It is a visible change (without the mask the comparison counts it), and with the
      // mask the comparison is exactly what it was.
      await page.evaluate(() => {
        const nav = document.querySelector<HTMLElement>("nav[aria-label='Breadcrumb']");
        if (nav) nav.style.visibility = "hidden";
      });
      const hidden = await shoot();
      const seenWithout = await compareFrames(scratch, ref, hidden, unmasked);
      const seenWith = await compareFrames(scratch, ref, hidden, masks);
      expect(seenWithout.diff, "the breadcrumb's disappearing is visible to the comparison that has no mask for it").not.toBe(baseUnmasked.diff);
      expect(seenWith.diff, "with the mask the comparison does not see it").toBe(base.diff);
      expect(seenWith.considered).toBe(base.considered);
      await page.evaluate(() => {
        const nav = document.querySelector<HTMLElement>("nav[aria-label='Breadcrumb']");
        if (nav) nav.style.visibility = "";
      });

      // Next to the mask: the swimlane switcher, 8 px to its right, disappears. The comparison counts it.
      await page.evaluate(() => {
        const button = Array.from(document.querySelectorAll<HTMLElement>("header button")).find((b) => (b.getAttribute("aria-label") ?? "").startsWith("Swimlane: "));
        if (!button) throw new Error("no switcher");
        button.style.visibility = "hidden";
      });
      const switcher = await compareFrames(scratch, ref, await shoot(), masks);
      expect(switcher.diff, "the switcher is next to the mask and not in it").toBeGreaterThan(base.diff);
      await page.evaluate(() => {
        for (const b of Array.from(document.querySelectorAll<HTMLElement>("header button"))) b.style.visibility = "";
      });

      // Far from it, in the sidebar: a block of colour the width of the sidebar. The comparison fails, as it would without the mask.
      await page.evaluate(() => {
        const block = document.createElement("div");
        block.setAttribute("data-probe", "block");
        Object.assign(block.style, { position: "fixed", left: "0", top: "200px", width: "224px", height: "30px", background: "rgb(255, 0, 0)", zIndex: "2147483647" });
        document.body.appendChild(block);
      });
      const sidebar = await compareFrames(scratch, ref, await shoot(), masks);
      expect(sidebar.diff).toBeGreaterThan(base.diff + 5000);
      expect(sidebar.ratio, "a block of colour in the sidebar fails the frame").toBeGreaterThan(MAX_RATIO);
    } finally {
      await scratch.close();
    }
  });
});
