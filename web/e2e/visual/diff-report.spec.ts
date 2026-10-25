// The text that says WHERE a tier-1 comparison differs (e2e/visual/diff-report.mjs), run on pictures whose difference is known: a block
// of 6 by 4 pixels at a known place. None of it needs the React files or the app: the pictures are taken from a blank page, as
// e2e/visual/search/kit.spec.ts does for the comparison itself.
//
// The parts of the report that need no browser (the clusters, the format, the limit of 40 lines, the names cut to 60 characters) are
// read in scripts/diff-report.test.mjs; what is read here is the part that decodes the pictures and asks the page what is under a point.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { compareFrames } from "./access/lib/compare";
import { compareImages } from "./compare.mjs";
import { elementProbe, explainDifference } from "./diff-report.mjs";

const SIZE = { width: 200, height: 100 };

type Block = { x: number; y: number; w?: number; h?: number; html?: string };

/** A white page with red blocks (6 by 4 pixels unless it says otherwise), photographed. The page keeps showing them afterwards. */
async function shot(page: Page, blocks: readonly Block[]): Promise<Buffer> {
  await page.setViewportSize(SIZE);
  const divs = blocks.map((b) => `<div style="position:absolute;left:${b.x}px;top:${b.y}px;width:${b.w ?? 6}px;height:${b.h ?? 4}px;background:#c00;overflow:hidden">${b.html ?? ""}</div>`).join("");
  await page.setContent(`<body style="margin:0;background:#fff">${divs}</body>`);
  return page.screenshot();
}

/** The report of two pictures, as a tier-1 test makes it. `page` is showing the app's. */
async function report(page: Page, reference: Buffer, app: Buffer, probe = true): Promise<string> {
  const result = await compareImages(page, reference, app, { threshold: 0.2 });
  return explainDifference(
    page,
    { reference, app, diffPng: result.diffPng },
    {
      label: "synthetic frame",
      ratio: result.ratio,
      limit: 0.005,
      differing: result.diff,
      compared: result.width * result.height,
      ...(probe ? { probes: { app: elementProbe(page) }, notes: ["design page: not queried (a synthetic picture has none)"] } : {}),
    },
  );
}

test.describe("where two pictures differ", () => {
  test("names the position, the size and the count of a block of 6 by 4 pixels, and the element of the app under it", async ({ page }) => {
    const reference = await shot(page, []);
    const app = await shot(page, [{ x: 40, y: 20 }]);
    const text = await report(page, reference, app);
    expect(text).toContain("x 40..45 y 20..23 (6x4)");
    expect(text).toContain("24 px");
    expect(text).toContain("1 cluster");
    expect(text).toContain("<div>");
    expect(text).toContain("6x4@(40,20)");
    expect(text).toContain("not queried");
    expect(text.split("\n").length).toBeLessThanOrEqual(40);
  });

  test("says that nothing differs between a picture and itself", async ({ page }) => {
    const a = await shot(page, [{ x: 40, y: 20 }]);
    const text = await report(page, a, a);
    expect(text).toContain("no differing pixels");
    expect(text).not.toContain("#1");
  });

  test("reports nothing of a difference that is masked, and only the one that is not", async ({ page }) => {
    const reference = await shot(page, []);
    const app = await shot(page, [{ x: 40, y: 20 }, { x: 120, y: 60 }]);
    // The first block is masked, as a mask of a tier-1 case is: painted over in both pictures before they are compared.
    const mask = { x: 36, y: 16, width: 16, height: 12, why: "a row of DEVIATIONS.md" };
    const only = await compareFrames(page, reference, app, [mask]);
    expect(only.diff).toBe(24);
    const text = await explainDifference(
      page,
      { reference: only.painted.reference, app: only.painted.app, diffPng: only.diffPng },
      { label: "synthetic frame", ratio: only.ratio, limit: 0.005, differing: only.diff, compared: only.considered, probes: { app: elementProbe(page) } },
    );
    expect(text).toContain("x 120..125 y 60..63 (6x4)");
    expect(text).not.toContain("x 40..45");
    expect(text).toContain("1 cluster");
    // Both blocks masked: no difference at all.
    const all = await compareFrames(page, reference, app, [mask, { x: 116, y: 56, width: 16, height: 12, why: "a row of DEVIATIONS.md" }]);
    expect(all.diff).toBe(0);
    const none = await explainDifference(
      page,
      { reference: all.painted.reference, app: all.painted.app, diffPng: all.diffPng },
      { label: "synthetic frame", ratio: all.ratio, limit: 0.005, differing: all.diff, compared: all.considered },
    );
    expect(none).toContain("no differing pixels");
  });

  test("tells a block that is one pixel to the right from one that is not the same, and by how much the shift helps", async ({ page }) => {
    const reference = await shot(page, [{ x: 40, y: 20 }]);
    const app = await shot(page, [{ x: 41, y: 20 }]);
    const text = await report(page, reference, app);
    // The left column of the design's block and the right column of the app's: 8 pixels in a cluster of 7 by 4.
    expect(text).toContain("x 40..46 y 20..23 (7x4)");
    expect(text).toContain("8 px");
    expect(text).toContain("ink 6x4 | 6x4: same size");
    expect(text).toContain("(+1,0)");
    expect(text).toContain("8 -> 0");
  });

  test("tells a block that is a pixel wider from one that is the same size", async ({ page }) => {
    const reference = await shot(page, [{ x: 40, y: 20 }]);
    const app = await shot(page, [{ x: 40, y: 20, w: 7 }]);
    const text = await report(page, reference, app);
    expect(text).toContain("ink 6x4 | 7x4: sizes differ");
    expect(text).toContain("4 px");
  });

  test("gives the mean absolute difference of the colour of the pixels that differ", async ({ page }) => {
    const reference = await shot(page, []);
    const app = await shot(page, [{ x: 40, y: 20 }]);
    const text = await report(page, reference, app);
    // White (255, 255, 255) against #c00 (204, 0, 0): (51 + 255 + 255) / 3 = 187.
    expect(text).toMatch(/mean abs diff 187\/255/);
    expect(text).toContain("#ffffff -> #cc0000");
  });

  test("never prints more than 60 characters of the text of an element, and never the value of a field", async ({ page }) => {
    const reference = await shot(page, []);
    const app = await shot(page, [
      { x: 40, y: 20, w: 40, h: 20, html: `<span>${"words ".repeat(80)}</span>` },
      // A field that fills its block and is drawn in the block's colour: its value is not shown, and must not be in the report either.
      { x: 120, y: 60, w: 40, h: 20, html: `<input value="hunter2-not-for-a-log" style="display:block;width:100%;height:100%;box-sizing:border-box;border:0;padding:0;background:#c00;color:#c00">` },
    ]);
    const text = await report(page, reference, app);
    expect(text).not.toContain("words ".repeat(11));
    expect(text).toContain("words words");
    expect(text).not.toContain("hunter2");
    for (const line of text.split("\n")) expect(line.length, line).toBeLessThan(400);
  });

  test("does not fail a test that has already failed: a page that cannot be asked gives a note, not an error", async ({ page }) => {
    const reference = await shot(page, []);
    const app = await shot(page, [{ x: 40, y: 20 }]);
    const result = await compareImages(page, reference, app, { threshold: 0.2 });
    const text = await explainDifference(
      page,
      { reference, app, diffPng: result.diffPng },
      {
        label: "synthetic frame",
        ratio: result.ratio,
        limit: 0.005,
        differing: result.diff,
        compared: result.width * result.height,
        probes: {
          app: () => {
            throw new Error("the page is closed");
          },
        },
      },
    );
    expect(text).toContain("x 40..45 y 20..23 (6x4)");
    expect(text).toContain("app: not queried (the page is closed)");
  });
});
