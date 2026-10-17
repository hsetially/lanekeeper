// Token parity (T0 Verify). The committed src/styles/tokens.css is compared with:
//  1. the colour token tables of the Engineering Handoff page (light and dark values),
//     an independent second source: the page repeats the values by hand;
//  2. the 82 token names the Style Guide requires;
//  3. the non-colour rows of the same page. Those are prose copied from the same CSS files, so this
//     comparison is NOT independent of the generator's source; it only catches transcription drift.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { WEB_ROOT, loadPage } from "../../scripts/handoff/dc-parse.mjs";
import { parseCssBlocks } from "../../scripts/handoff/extract-tokens.mjs";
import { STYLE_GUIDE_TOKENS } from "./pairs.generated";

type Block = { selector: string; vars: [string, string][] };

const css = readFileSync(join(WEB_ROOT, "src/styles/tokens.css"), "utf8");
const blocks = parseCssBlocks(css) as Block[];
const rootBlock = blocks.find((b) => b.selector === ":root");
const darkBlock = blocks.find((b) => b.selector === '.dark, [data-theme="dark"]');

function toMap(block: Block | undefined): Map<string, string> {
  if (!block) throw new Error("tokens.css lacks a theme block");
  return new Map(block.vars);
}
const root = toMap(rootBlock);
const dark = toMap(darkBlock);

const page = loadPage("Lanekeeper Engineering Handoff.dc.html");
const G = page.evalConst("G") as Record<string, string>;

type Row = { name: string; light: string; dark: string; ramp: boolean };
const tableRows: Row[] = Object.entries(G).flatMap(([group, text]) =>
  text.split("\n").map((line) => {
    const [name, light, darkValue] = line.split("|");
    if (!name || !light || !darkValue) throw new Error(`bad token row: ${line}`);
    return { name: `--${name}`, light, dark: darkValue, ramp: group.startsWith("Ramps") };
  }),
);

describe("colour tokens against the Engineering Handoff tables", () => {
  it("reads the six token groups and 22 ramp rows", () => {
    expect(Object.keys(G)).toHaveLength(6);
    expect(tableRows.filter((r) => r.ramp)).toHaveLength(22);
    expect(tableRows.length).toBeGreaterThan(100);
  });

  it.each(tableRows.filter((r) => !r.ramp))("$name has the documented light and dark value", (row) => {
    expect(root.get(row.name), `light ${row.name}`).toBe(row.light);
    expect(dark.get(row.name), `dark ${row.name}`).toBe(row.dark);
  });

  it.each(tableRows.filter((r) => r.ramp))("ramp $name is the same value in :root and absent from dark", (row) => {
    expect(root.get(row.name)).toBe(row.light);
    expect(row.light).toBe(row.dark);
    expect(dark.has(row.name)).toBe(false);
  });

  it("defines no colour token the tables do not document, and none is missing", () => {
    const documented = new Set(tableRows.map((r) => r.name));
    // Themed tokens that are not plain colours; documented in the "Non-colour tokens" table instead.
    const notColours = new Set(["--shadow-popover", "--diff-empty-bg", "--overlay"]);
    const themed = [...root.keys()].filter((k) => dark.has(k) && !notColours.has(k));
    expect(themed.filter((k) => !documented.has(k))).toEqual([]);
    expect([...documented].filter((k) => !root.has(k) || (!k.match(/^--(gray|blue)-/) && !dark.has(k)))).toEqual([]);
  });

  it("defines every token name the Style Guide asks for, in both themes", () => {
    const missingLight = STYLE_GUIDE_TOKENS.filter((n) => !root.has(n));
    const missingDark = STYLE_GUIDE_TOKENS.filter((n) => !dark.has(n));
    expect(missingLight).toEqual([]);
    expect(missingDark).toEqual([]);
  });

  it("switches colour-scheme with the theme", () => {
    expect(css).toMatch(/color-scheme: light;/);
    expect(css).toMatch(/color-scheme: dark;/);
  });
});

// ---------------------------------------------------------------------------------------------
// Non-colour tokens. The page lists them as prose: "12/16 · 13/18", "4px · 6px · 6px".

const norm = (v: string): string => v.replace(/\s+/g, "").replace(/(^|[^0-9])0\./g, "$1.").toLowerCase();

function otherTable(): string[][] {
  const call = (page.calls("sec") as { arguments: { value?: string }[] }[]).find((c) => c.arguments[0]?.value === "tok-other");
  if (!call) throw new Error("no tok-other table");
  return (page.evalArg(call, 7) as unknown[][]).map((row) => row.map((c) => (typeof c === "string" ? c : (c as { t: string }).t).replace(/^`/, "")));
}
const other = otherTable();
const rowFor = (prefix: string): string[] => {
  const row = other.find((r) => (r[0] ?? "").startsWith(prefix));
  if (!row) throw new Error(`no row ${prefix}`);
  return row;
};
const val = (name: string): string => {
  const v = root.get(name);
  if (v === undefined) throw new Error(`tokens.css lacks ${name}`);
  return v;
};
const dashList = (text: string): string[] => text.split("·").map((s) => s.trim());

describe("non-colour tokens against the prose rows (not an independent source)", () => {
  it("type scale: size and line height pairs", () => {
    const pairs = dashList(rowFor("--text-xs")[2] ?? "").map((p) => p.split("/"));
    const names = ["xs", "sm", "base", "lg", "xl", "2xl"];
    expect(pairs).toHaveLength(names.length);
    names.forEach((n, i) => {
      expect(val(`--text-${n}`)).toBe(`${pairs[i]?.[0]}px`);
      expect(val(`--leading-${n}`)).toBe(`${pairs[i]?.[1]}px`);
    });
  });

  it("weights, tracking, radius, controls, header, rows and motion", () => {
    expect(dashList(rowFor("--weight-")[2] ?? "")).toEqual(["400", "500", "600"]);
    expect(["regular", "medium", "semibold"].map((n) => val(`--weight-${n}`))).toEqual(["400", "500", "600"]);

    expect(dashList(rowFor("--tracking-")[2] ?? "")).toEqual([val("--tracking-tight"), val("--tracking-caps")].map((v) => v.replace(/^-?0\./, (m) => m.replace("0.", "0."))));

    expect(dashList(rowFor("--radius-sm")[2] ?? "")).toEqual([val("--radius-sm"), val("--radius-md"), val("--radius")]);
    expect(dashList(rowFor("--control-")[2] ?? "")).toEqual(["sm", "md", "lg"].map((n) => val(`--control-${n}`).replace("px", "")));
    expect(dashList(rowFor("--header-height")[2] ?? "")).toEqual([val("--header-height"), val("--toolbar-height")].map((v) => v.replace("px", "")));
    expect(dashList(rowFor("--row-compact")[2] ?? "")).toEqual(["compact", "default", "tree", "diff"].map((n) => val(`--row-${n}`).replace("px", "")));

    const motion = dashList(rowFor("--duration-fast")[2] ?? "");
    expect(motion.map(norm)).toEqual([val("--duration-fast"), val("--duration-base"), val("--ease-out")].map(norm));
  });

  it("spacing steps (4px grid with the 2px and 6px half steps)", () => {
    const steps = (rowFor("--space-1")[2] ?? "").split(/\s+/).map(Number);
    const tokens = ["1", "1-5", "2", "3", "4", "5", "6", "8", "10", "12"].map((n) => Number.parseInt(val(`--space-${n}`), 10));
    expect(steps).toEqual(tokens);
    expect(val("--space-0-5")).toBe("2px");
  });

  it("fonts and the overlay scrim", () => {
    expect(val("--font-sans")).toMatch(/^'Inter', ui-sans-serif, system-ui/);
    expect(val("--font-mono")).toMatch(/^'JetBrains Mono', ui-monospace/);
    const overlay = (rowFor("--overlay")[2] ?? "").replace(/^L\s*/, "").split("·").map((s) => s.replace(/^\s*D\s*/, "").trim());
    expect(overlay.map(norm)).toEqual([root.get("--overlay"), dark.get("--overlay")].map((v) => norm(v ?? "")));
  });

  it("the dark shadow and the hatch gradient keep the colours the prose names", () => {
    expect(dark.get("--shadow-popover")).toContain("#2a313c");
    expect(root.get("--diff-empty-bg")).toContain("#f5f6f8 0 4px, #eef0f3 4px 8px");
    expect(dark.get("--diff-empty-bg")).toContain("#12161c 0 4px, #171c23 4px 8px");
  });
});

describe("generated Tailwind theme", () => {
  const theme = readFileSync(join(WEB_ROOT, "src/styles/theme.generated.css"), "utf8");

  it("maps every themed colour to var(--name) and never a ramp", () => {
    const notColours = new Set(["--shadow-popover", "--diff-empty-bg"]);
    const keys = [...root.keys()].filter((k) => dark.has(k) && !notColours.has(k));
    expect(keys).toHaveLength(83); // the 82 Style Guide names plus --overlay
    for (const k of keys) expect(theme, k).toContain(`--color-${k.slice(2)}: var(${k});`);
    expect(theme).not.toMatch(/--color-(gray|blue)-/);
  });
});
