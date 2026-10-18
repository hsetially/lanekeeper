// Pixel comparison of two PNGs, run in the browser that took them (it has the decoder; the repo has no image library and the
// plan adds none). It is the rule of Playwright's own `toHaveScreenshot` (pixelmatch): two pixels differ when their distance in
// YIQ space is over `threshold` squared of the largest possible distance (35215), with `threshold` 0.2 as the plan says. It does
// not do pixelmatch's anti-aliasing detection, so it is the stricter of the two: a result under the limit here is under it there.

/**
 * @param {import("@playwright/test").Page} page any page; its context only decodes the images
 * @param {Buffer} a
 * @param {Buffer} b
 * @param {{ threshold?: number }} [options]
 * @returns {Promise<{ width: number; height: number; diff: number; ratio: number; diffPng: string }>} `diffPng` is a data URL
 */
export async function compareImages(page, a, b, { threshold = 0.2 } = {}) {
  return page.evaluate(
    async ({ a64, b64, limit }) => {
      const load = async (base64) => {
        const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const context = canvas.getContext("2d");
        context.drawImage(bitmap, 0, 0);
        return context.getImageData(0, 0, bitmap.width, bitmap.height);
      };
      const [x, y] = await Promise.all([load(a64), load(b64)]);
      if (x.width !== y.width || x.height !== y.height) throw new Error(`the images are ${x.width}x${x.height} and ${y.width}x${y.height}`);
      const maxDelta = 35215 * limit * limit;
      const yiq = (r1, g1, b1, r2, g2, b2) => {
        const rr = r1 - r2;
        const gg = g1 - g2;
        const bb = b1 - b2;
        const y1 = rr * 0.29889531 + gg * 0.58662247 + bb * 0.11448223;
        const i1 = rr * 0.59597799 - gg * 0.2741761 - bb * 0.32180189;
        const q1 = rr * 0.21147017 - gg * 0.52261711 + bb * 0.31114694;
        return 0.5053 * y1 * y1 + 0.299 * i1 * i1 + 0.1957 * q1 * q1;
      };
      const out = new ImageData(x.width, x.height);
      let diff = 0;
      for (let i = 0; i < x.data.length; i += 4) {
        const ax = x.data[i + 3] / 255;
        const ay = y.data[i + 3] / 255;
        const mix = (c, alpha) => 255 + (c - 255) * alpha;
        const delta = yiq(mix(x.data[i], ax), mix(x.data[i + 1], ax), mix(x.data[i + 2], ax), mix(y.data[i], ay), mix(y.data[i + 1], ay), mix(y.data[i + 2], ay));
        const differs = delta > maxDelta;
        if (differs) diff += 1;
        out.data[i] = differs ? 255 : x.data[i] * 0.25 + 191;
        out.data[i + 1] = differs ? 0 : x.data[i + 1] * 0.25 + 191;
        out.data[i + 2] = differs ? 0 : x.data[i + 2] * 0.25 + 191;
        out.data[i + 3] = 255;
      }
      const canvas = new OffscreenCanvas(x.width, x.height);
      canvas.getContext("2d").putImageData(out, 0, 0);
      const blob = await canvas.convertToBlob({ type: "image/png" });
      const url = await new Promise((resolve) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result);
        reader.readAsDataURL(blob);
      });
      return { width: x.width, height: x.height, diff, ratio: diff / (x.width * x.height), diffPng: url };
    },
    { a64: a.toString("base64"), b64: b.toString("base64"), limit: threshold },
  );
}
