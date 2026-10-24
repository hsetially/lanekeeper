// Fonts are self-hosted (S12: font-src 'self'). The copies must be byte-identical to the handoff files,
// fonts.css must declare the same faces as the handoff's fonts.css, and the licences must ship with them.
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { HANDOFF_DIR, WEB_ROOT } from "../../../scripts/handoff/dc-parse.mjs";

const FONTS = dirname(fileURLToPath(import.meta.url));
const sha256 = (file: string): string => createHash("sha256").update(readFileSync(file)).digest("hex");

const FILES = ["Inter-400.woff2", "Inter-500.woff2", "Inter-600.woff2", "JetBrainsMono-400.woff2", "JetBrainsMono-500.woff2"];

// The handoff ships one file per family under several names: three Inter copies and two JetBrains Mono copies.
const INTER_SHA = "3100e775e8616cd2611beecfa23a4263d7037586789b43f035236a2e6fbd4c62";
const MONO_SHA = "83c005d49d8a6a50474c73a5a36ac0468076e9c4a29da7bdb14995d80560a5be";

type Face = { family: string; weight: string; style: string; display: string; url: string; format: string };

function faces(css: string): Face[] {
  const out: Face[] = [];
  for (const m of css.matchAll(/@font-face\s*\{([^}]*)\}/g)) {
    const body = m[1] ?? "";
    const get = (prop: string): string => new RegExp(`${prop}\\s*:\\s*([^;]+);`).exec(body)?.[1]?.trim() ?? "";
    const src = /url\(\s*['"]?([^'")]+)['"]?\s*\)\s*format\(\s*['"]([^'"]+)['"]\s*\)/.exec(get("src"));
    out.push({
      family: get("font-family").replace(/['"]/g, ""),
      weight: get("font-weight"),
      style: get("font-style"),
      display: get("font-display"),
      url: src?.[1] ?? "",
      format: src?.[2] ?? "",
    });
  }
  return out;
}

const base = (url: string): string => url.split("/").at(-1) ?? "";

describe("font files", () => {
  it.each(FILES)("%s is byte-identical to the handoff copy", (name) => {
    expect(sha256(join(FONTS, name))).toBe(sha256(join(HANDOFF_DIR, "assets", "fonts", name)));
  });

  it("pins one hash per family", () => {
    for (const name of FILES.filter((n) => n.startsWith("Inter"))) expect(sha256(join(FONTS, name))).toBe(INTER_SHA);
    for (const name of FILES.filter((n) => n.startsWith("JetBrains"))) expect(sha256(join(FONTS, name))).toBe(MONO_SHA);
  });

  it("ships no other font file", () => {
    expect(FILES.every((n) => existsSync(join(FONTS, n)))).toBe(true);
  });
});

describe("fonts.css", () => {
  const ours = faces(readFileSync(join(WEB_ROOT, "src/styles/fonts.css"), "utf8"));
  const theirs = faces(readFileSync(join(HANDOFF_DIR, "tokens/fonts.css"), "utf8"));

  it("declares the five faces of the handoff, with the same weights, style and swap", () => {
    expect(ours).toHaveLength(5);
    const key = (f: Face): string => `${f.family}|${f.weight}|${f.style}|${f.display}|${f.format}|${base(f.url)}`;
    expect(ours.map(key)).toEqual(theirs.map(key));
  });

  it("points every face at a file that exists, relative to the stylesheet", () => {
    for (const face of ours) {
      expect(face.url.startsWith("../assets/fonts/"), face.url).toBe(true);
      expect(existsSync(join(WEB_ROOT, "src/styles", face.url)), face.url).toBe(true);
    }
  });

  it("makes no external request (S12)", () => {
    const css = readFileSync(join(WEB_ROOT, "src/styles/fonts.css"), "utf8");
    expect(css).not.toMatch(/https?:|\/\/|local\(|data:/);
  });
});

describe("licences", () => {
  const text = readFileSync(join(FONTS, "LICENSES.md"), "utf8");

  it("names both families and carries the SIL Open Font License 1.1", () => {
    expect(text).toMatch(/Inter/);
    expect(text).toMatch(/JetBrains Mono/);
    expect(text).toMatch(/SIL OPEN FONT LICENSE Version 1\.1/);
    expect(text).toMatch(/PERMISSION & CONDITIONS/);
    expect(text).toMatch(/DISCLAIMER/);
  });
});
