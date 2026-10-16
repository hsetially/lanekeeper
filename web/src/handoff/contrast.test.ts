// The Style Guide checks 45 contrast pairs live in the browser (35 at 4.5:1, 10 at 3:1). The handoff
// says that check becomes a unit test: this is it, over the shipped tokens.css, in both themes.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { WEB_ROOT } from "../../scripts/handoff/dc-parse.mjs";
import { parseCssBlocks } from "../../scripts/handoff/extract-tokens.mjs";
import { PAIRS } from "./pairs.generated";

type Block = { selector: string; vars: [string, string][] };

// WCAG 2.x relative luminance and contrast ratio, same formula as the Style Guide's `lum` and `ratio`.
export function luminance(hex: string): number {
  let h = hex.replace("#", "");
  if (h.length === 3) h = [...h].map((c) => c + c).join("");
  if (!/^[0-9a-fA-F]{6}$/.test(h)) throw new Error(`not a hex colour: ${hex}`);
  const channel = (i: number): number => {
    const v = Number.parseInt(h.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}
export function ratio(a: string, b: string): number {
  const x = luminance(a);
  const y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

const blocks = parseCssBlocks(readFileSync(join(WEB_ROOT, "src/styles/tokens.css"), "utf8")) as Block[];
const light = new Map(blocks.find((b) => b.selector === ":root")?.vars);
const darkOverrides = new Map(blocks.find((b) => b.selector === '.dark, [data-theme="dark"]')?.vars);
const themes: Record<"light" | "dark", Map<string, string>> = {
  light,
  dark: new Map([...light, ...darkOverrides]),
};

describe("contrast helper", () => {
  it("matches the WCAG reference values", () => {
    expect(ratio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(ratio("#ffffff", "#ffffff")).toBeCloseTo(1, 5);
    expect(ratio("#777777", "#ffffff")).toBeCloseTo(4.48, 2);
    expect(ratio("#767676", "#ffffff")).toBeGreaterThanOrEqual(4.5);
  });
});

describe("contrast pairs of the Style Guide", () => {
  it("has 45 pairs: 35 at 4.5:1 and 10 at 3:1", () => {
    expect(PAIRS).toHaveLength(45);
    expect(PAIRS.filter((p) => p.min === 4.5)).toHaveLength(35);
    expect(PAIRS.filter((p) => p.min === 3)).toHaveLength(10);
  });

  for (const theme of ["light", "dark"] as const) {
    describe(theme, () => {
      it.each(PAIRS.map((p) => [p.fg, p.bg, p.min] as const))("%s on %s is at least %s:1", (fg, bg, min) => {
        const fgValue = themes[theme].get(fg);
        const bgValue = themes[theme].get(bg);
        expect(fgValue, `${theme} ${fg}`).toBeDefined();
        expect(bgValue, `${theme} ${bg}`).toBeDefined();
        const r = ratio(fgValue ?? "", bgValue ?? "");
        expect(r, `${fg} ${fgValue} on ${bg} ${bgValue} is ${r.toFixed(2)}:1`).toBeGreaterThanOrEqual(min);
      });
    });
  }
});
