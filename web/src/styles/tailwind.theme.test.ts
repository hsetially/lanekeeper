// Compiles the real tailwind.css (the app's style entry point) and checks the utilities it produces:
// the handoff vocabulary exists, Tailwind's own palette, scale and shadows do not, and dark mode follows
// the class or attribute the theme script sets, never the media query.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "@tailwindcss/node";
import { beforeAll, describe, expect, it } from "vitest";

const STYLES = dirname(fileURLToPath(import.meta.url));
let build: (candidates: string[]) => string;

beforeAll(async () => {
  const css = readFileSync(join(STYLES, "tailwind.css"), "utf8");
  const compiler = await compile(css, { base: STYLES, onDependency: () => {} });
  build = (candidates) => compiler.build(candidates);
});

function rule(css: string, selector: string): string {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) return "";
  return css.slice(start, css.indexOf("}", start) + 1);
}

describe("handoff vocabulary", () => {
  it("emits colour utilities that resolve through the theme variables", () => {
    const css = build(["bg-background", "text-status-sync-fg", "border-status-untracked-border", "bg-primary-hover", "ring-ring", "bg-overlay", "bg-diff-empty"]);
    expect(rule(css, ".bg-background")).toContain("background-color: var(--background)");
    expect(rule(css, ".text-status-sync-fg")).toContain("color: var(--status-sync-fg)");
    expect(rule(css, ".border-status-untracked-border")).toContain("border-color: var(--status-untracked-border)");
    expect(rule(css, ".bg-primary-hover")).toContain("var(--primary-hover)");
    expect(rule(css, ".ring-ring")).toContain("var(--ring)");
    expect(rule(css, ".bg-overlay")).toContain("var(--overlay)");
    expect(rule(css, ".bg-diff-empty")).toContain("background-image: var(--diff-empty-bg)");
  });

  it("uses the handoff type scale, where body text is 13/18 and code is 12/18", () => {
    const css = build(["text-xs", "text-sm", "text-base", "text-lg", "text-xl", "text-2xl", "text-mono"]);
    const expected: Record<string, [string, string]> = {
      "text-xs": ["12px", "16px"],
      "text-sm": ["13px", "18px"],
      "text-base": ["14px", "20px"],
      "text-lg": ["16px", "22px"],
      "text-xl": ["20px", "26px"],
      "text-2xl": ["24px", "30px"],
      "text-mono": ["12px", "18px"],
    };
    for (const [name, [size, lead]] of Object.entries(expected)) {
      const r = rule(css, `.${name}`);
      expect(r, name).toContain(`font-size: ${size}`);
      expect(r, name).toContain(`var(--tw-leading, ${lead})`);
    }
  });

  it("uses 4px and 6px radii, the 4px spacing grid and the one popover shadow", () => {
    const css = build(["rounded-sm", "rounded-md", "rounded", "h-7", "h-8", "p-1.5", "shadow-popover"]);
    expect(rule(css, ".rounded-sm")).toContain("border-radius: 4px");
    expect(rule(css, ".rounded-md")).toContain("border-radius: 6px");
    expect(rule(css, ".rounded")).toContain("border-radius: 6px");
    expect(rule(css, ".h-7")).toContain("calc(var(--spacing) * 7)");
    expect(css).toMatch(/--spacing: 4px;/);
    expect(rule(css, ".shadow-popover")).toContain("--tw-shadow: var(--shadow-popover)");
  });

  it("lets the real token declarations win over the theme layer's pass-through of --shadow-popover", () => {
    const css = build(["shadow-popover"]);
    const layered = css.indexOf("--shadow-popover: var(--shadow-popover)");
    const unlayered = css.indexOf("--shadow-popover: 0 8px 24px rgba(14,17,22,0.12)");
    expect(layered).toBeGreaterThan(-1);
    expect(unlayered).toBeGreaterThan(layered);
    // The real value sits in a top-level (unlayered) :root rule, which beats any @layer rule.
    const before = css.slice(0, unlayered);
    const depth = [...before].reduce((d, ch) => d + (ch === "{" ? 1 : ch === "}" ? -1 : 0), 0);
    expect(depth).toBe(1); // inside :root { ... } only
    expect(before.slice(before.lastIndexOf("{") - 8, before.lastIndexOf("{"))).toMatch(/:root\s*$/);
  });

  it("uses the three weights and the handoff fonts", () => {
    const css = build(["font-normal", "font-medium", "font-semibold", "font-sans", "font-mono", "tracking-tight", "tracking-caps"]);
    expect(rule(css, ".font-medium")).toContain("font-weight: 500");
    expect(rule(css, ".font-semibold")).toContain("font-weight: 600");
    expect(rule(css, ".font-sans")).toContain("'Inter'");
    expect(rule(css, ".font-mono")).toContain("'JetBrains Mono'");
    expect(rule(css, ".tracking-tight")).toContain("-0.01em");
    expect(rule(css, ".tracking-caps")).toContain("0.04em");
  });
});

describe("custom utilities", () => {
  it("has the pressed-button shadow of the Style Guide", () => {
    const css = build(["shadow-pressed", "active:shadow-pressed"]);
    expect(rule(css, ".shadow-pressed")).toContain("inset 0 1px 2px rgba(0, 0, 0, 0.25)");
    expect(css).toContain(".active\\:shadow-pressed");
  });
});

describe("what does not exist", () => {
  it("has no Tailwind palette, no ramps, no extra shadows, no bold, no bounce", () => {
    const none = ["bg-blue-500", "text-gray-500", "bg-red-500", "bg-gray-950", "shadow-md", "shadow-sm", "font-bold", "animate-bounce", "animate-spin", "text-3xl", "rounded-lg", "rounded-xl", "bg-white", "text-black"];
    const css = build(none);
    for (const name of none) expect(css, name).not.toContain(`.${name.replace(":", "\\:")} {`);
  });
});

describe("dark mode and motion", () => {
  it("scopes dark: to the class or attribute, not to prefers-color-scheme", () => {
    const css = build(["dark:bg-primary"]);
    expect(css).toContain('.dark\\:bg-primary:where(.dark, .dark *, [data-theme="dark"], [data-theme="dark"] *)');
    expect(css).not.toContain("prefers-color-scheme");
  });

  it("defines the spinner, pulse and open animations and slows or stops them under reduced motion", () => {
    const css = build(["animate-lk-spin", "animate-lk-pulse", "animate-lk-fade-in", "animate-lk-pop-in"]);
    expect(css).toContain("@keyframes lk-spin");
    expect(css).toContain("@keyframes lk-pulse");
    const reduced = css.slice(css.indexOf("@media (prefers-reduced-motion: reduce)"));
    expect(reduced).toContain("animation-duration: 2.4s");
    expect(reduced).toContain("animation: none");
    expect(css).not.toContain("bounce");
  });
});
