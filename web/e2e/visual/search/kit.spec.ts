// The matrix itself, so that it cannot pass for any two images: the share that is compared, the props the reference is drawn with, and
// the masks and regions (a difference outside them is found, a difference inside the masks is not). None of it needs the React files.
import { expect, test } from "../../fixtures/test";
import { compareImages } from "../compare.mjs";
import { comparedShare, MIN_COMPARED, paint, withPropDefaults } from "./kit";

const SIZE = { width: 1440, height: 900 };

test.describe("the visual matrix machinery", () => {
  test("the compared share is the frame without its masks, or the regions without their masks, with overlaps counted once", () => {
    expect(comparedShare(undefined, [], SIZE)).toBe(1);
    expect(comparedShare(undefined, [{ x: 0, y: 0, width: 720, height: 900 }], SIZE)).toBeCloseTo(0.5, 6);
    expect(comparedShare([{ x: 0, y: 0, width: 144, height: 900 }], [], SIZE)).toBeCloseTo(0.1, 6);
    const half = { x: 0, y: 0, width: 720, height: 900 };
    expect(comparedShare(undefined, [half, half, { x: 100, y: 100, width: 50, height: 50 }], SIZE)).toBeCloseTo(0.5, 6);
    expect(comparedShare([{ x: 0, y: 0, width: 288, height: 450 }], [{ x: 144, y: 0, width: 144, height: 450 }], SIZE)).toBeCloseTo(0.05, 6);
    // Outside the frame counts for nothing.
    expect(comparedShare([{ x: 1400, y: 0, width: 400, height: 900 }], [], SIZE)).toBeCloseTo(40 / 1440, 6);
    expect(MIN_COMPARED).toBeGreaterThan(0);
  });

  test("the Shell's props are set in memory: a declared int and boolean are typed, an undeclared one is added as the board writes it", () => {
    const props = { page: { editor: "enum", default: "file" }, w: { editor: "int", default: 1440 }, collapsed: { editor: "boolean", default: false }, navMode: { editor: "enum", default: "hide" } };
    const html = `<script data-props="${JSON.stringify(props).replaceAll("&", "&amp;").replaceAll('"', "&quot;")}"></script>`;
    const out = withPropDefaults(html, { page: "search", w: "1280", collapsed: "true", "nav-mode": "disable", stab: "text", fsonlydiff: true });
    const match = /data-props="([^"]*)"/.exec(out);
    const parsed = JSON.parse((match?.[1] ?? "").replaceAll("&quot;", '"').replaceAll("&amp;", "&")) as Record<string, { default: unknown }>;
    expect(parsed["page"]?.default).toBe("search");
    expect(parsed["w"]?.default).toBe(1280);
    expect(parsed["collapsed"]?.default).toBe(true);
    expect(parsed["navMode"]?.default).toBe("disable");
    expect(parsed["stab"]?.default).toBe("text");
    expect(parsed["fsonlydiff"]?.default).toBe(true);
  });

  test("two different images differ, a mask over the difference hides it, a region that leaves it out hides it, and a difference elsewhere is found", async ({ page }) => {
    await page.setViewportSize({ width: 200, height: 100 });
    const shot = async (square: { x: number; y: number }) => {
      await page.setContent(`<body style="margin:0;background:#fff"><div style="position:absolute;left:${square.x}px;top:${square.y}px;width:40px;height:40px;background:#c00"></div></body>`);
      return page.screenshot();
    };
    const a = await shot({ x: 10, y: 10 });
    const b = await shot({ x: 120, y: 10 });
    expect((await compareImages(page, a, a)).diff).toBe(0);
    expect((await compareImages(page, a, b)).diff).toBeGreaterThan(0);
    // The two squares are at the left and at the right: painting both over hides every difference.
    const masks = [{ x: 0, y: 0, width: 60, height: 100 }, { x: 100, y: 0, width: 100, height: 100 }];
    const masked = await compareImages(page, await paint(page, a, undefined, masks), await paint(page, b, undefined, masks));
    expect(masked.diff).toBe(0);
    // Only the middle is compared: no difference there.
    const middle = [{ x: 70, y: 0, width: 30, height: 100 }];
    expect((await compareImages(page, await paint(page, a, middle, []), await paint(page, b, middle, []))).diff).toBe(0);
    // Only the left is compared: the square of `a` is there and the one of `b` is not.
    const left = [{ x: 0, y: 0, width: 60, height: 100 }];
    expect((await compareImages(page, await paint(page, a, left, []), await paint(page, b, left, []))).diff).toBeGreaterThan(0);
    // A mask over the left only leaves the right difference.
    const onlyLeft = [{ x: 0, y: 0, width: 60, height: 100 }];
    expect((await compareImages(page, await paint(page, a, undefined, onlyLeft), await paint(page, b, undefined, onlyLeft))).diff).toBeGreaterThan(0);
  });
});
