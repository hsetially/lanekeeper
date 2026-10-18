// Tier 1 compares the app's screenshot with the rendered reference of the same frame, pixel by pixel (threshold 0.2 per pixel,
// 0.5% of the pixels). The comparison itself is S0's (e2e/visual/compare.mjs, the rule of Playwright's own pixelmatch without its
// anti-aliasing allowance); what is added here is the MASK: a rectangle of the frame that is painted the same colour in both
// images before they are compared, for a part of the design that the app deliberately does not draw the same way (each a row of
// design/DEVIATIONS.md; the row is named next to the rectangle, so a mask without a reason cannot be written).
//
// The share that is reported and limited is the share of the pixels that are NOT masked, so a mask never makes the number smaller
// by being large: its size is reported beside it (`masked`), and a spec that masks most of a frame shows it.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { compareImages } from "../../compare.mjs";

export const THRESHOLD = 0.2;
export const MAX_RATIO = 0.005;
const OUT = process.env["LK_VISUAL_OUT"];

export type Rect = { x: number; y: number; width: number; height: number };
/** A part of the frame that is not compared, and why: the id of the DEVIATIONS row (or a short reason where the row is a candidate). */
export type Mask = Rect & { why: string };

/**
 * A part of the design that the app draws somewhere else, because something around it is not drawn (a row the contract cannot fill
 * shifts what follows it): `ref` in the reference and `app` in the app, the same size. They are compared with each other, and the
 * two places are masked out of the comparison of the whole frame by the caller.
 */
export type Aligned = { ref: Rect; app: Rect; why: string };

export type Comparison = { width: number; height: number; diff: number; considered: number; masked: number; ratio: number; diffPng: string; aligned: number };

/** Paint the rectangles one flat colour, and count the pixels they cover (their union, inside the image). */
async function paint(page: Page, png: Buffer, rects: readonly Rect[]): Promise<{ png: Buffer; covered: number }> {
  const result = await page.evaluate(
    async ({ b64, boxes }) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const context = canvas.getContext("2d");
      if (!context) throw new Error("no 2d context");
      context.drawImage(bitmap, 0, 0);
      context.fillStyle = "#ff00ff";
      const seen = new Uint8Array(bitmap.width * bitmap.height);
      let covered = 0;
      for (const box of boxes) {
        const x0 = Math.max(0, Math.floor(box.x));
        const y0 = Math.max(0, Math.floor(box.y));
        const x1 = Math.min(bitmap.width, Math.ceil(box.x + box.width));
        const y1 = Math.min(bitmap.height, Math.ceil(box.y + box.height));
        if (x1 <= x0 || y1 <= y0) continue;
        context.fillRect(x0, y0, x1 - x0, y1 - y0);
        for (let y = y0; y < y1; y += 1) {
          for (let x = x0; x < x1; x += 1) {
            const at = y * bitmap.width + x;
            if (seen[at] === 0) {
              seen[at] = 1;
              covered += 1;
            }
          }
        }
      }
      const blob = await canvas.convertToBlob({ type: "image/png" });
      const url = await new Promise<string>((resolve) => {
        const reader = new FileReader();
        reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
        reader.readAsDataURL(blob);
      });
      return { url, covered };
    },
    { b64: png.toString("base64"), boxes: rects.map((r) => ({ x: r.x, y: r.y, width: r.width, height: r.height })) },
  );
  return { png: Buffer.from(result.url.split(",")[1] ?? "", "base64"), covered: result.covered };
}

/** The part of a PNG inside a rectangle, as a PNG. */
async function crop(page: Page, png: Buffer, rect: Rect): Promise<Buffer> {
  const url = await page.evaluate(
    async ({ b64, box }) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const canvas = new OffscreenCanvas(box.width, box.height);
      const context = canvas.getContext("2d");
      if (!context) throw new Error("no 2d context");
      context.drawImage(bitmap, box.x, box.y, box.width, box.height, 0, 0, box.width, box.height);
      const blob = await canvas.convertToBlob({ type: "image/png" });
      return await new Promise<string>((resolve) => {
        const reader = new FileReader();
        reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
        reader.readAsDataURL(blob);
      });
    },
    { b64: png.toString("base64"), box: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } },
  );
  return Buffer.from(url.split(",")[1] ?? "", "base64");
}

/**
 * Compare two screenshots of the same frame, with the masked parts painted out of both, and the `aligned` parts compared with each
 * other where each is. The share is over every pixel that was compared. `page` only decodes the images (any page, best a blank one
 * in a context of its own).
 */
export async function compareFrames(page: Page, reference: Buffer, app: Buffer, masks: readonly Rect[] = [], aligned: readonly Aligned[] = []): Promise<Comparison> {
  const [a, b] = masks.length > 0 ? [await paint(page, reference, masks), await paint(page, app, masks)] : [{ png: reference, covered: 0 }, { png: app, covered: 0 }];
  const result = await compareImages(page, a.png, b.png, { threshold: THRESHOLD });
  let diff = result.diff;
  let considered = result.width * result.height - a.covered;
  for (const pair of aligned) {
    if (pair.ref.width !== pair.app.width || pair.ref.height !== pair.app.height) throw new Error(`aligned parts differ in size: ${pair.why}`);
    const part = await compareImages(page, await crop(page, reference, pair.ref), await crop(page, app, pair.app), { threshold: THRESHOLD });
    diff += part.diff;
    considered += part.width * part.height;
  }
  return { width: result.width, height: result.height, diff, considered, masked: a.covered, ratio: considered > 0 ? diff / considered : 0, diffPng: result.diffPng, aligned: aligned.length };
}

/** `LK_VISUAL_OUT=<dir>` keeps the reference, the app, the difference and the numbers of each comparison there, to look at. */
export function keep(name: string, reference: Buffer, app: Buffer, comparison: Comparison, extra: Record<string, unknown> = {}): void {
  if (!OUT) return;
  mkdirSync(OUT, { recursive: true });
  writeFileSync(join(OUT, `${name}-reference.png`), reference);
  writeFileSync(join(OUT, `${name}-app.png`), app);
  writeFileSync(join(OUT, `${name}-diff.png`), Buffer.from(comparison.diffPng.split(",")[1] ?? "", "base64"));
  const { diffPng: _diffPng, ...numbers } = comparison;
  writeFileSync(join(OUT, `${name}-ratio.json`), JSON.stringify({ name, threshold: THRESHOLD, max: MAX_RATIO, ...numbers, ...extra }, null, 2));
}
