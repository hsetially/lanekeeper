import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Monaco is about 4 MB. It must stay a lazy chunk (P10: the initial JavaScript is under 250 KB gzipped), and nothing may
// touch it before the Trusted Types hook is installed (F7). Source scans; the bundle gate (T2) and the CSP spec are the
// proofs in a build and in a browser.
const SRC = fileURLToPath(new URL("../src/", import.meta.url));

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) && !full.includes("/api/generated/")) out.push(full);
  }
  return out;
}
const files = walk(SRC);
const text = (f) => readFileSync(f, "utf8");
const rel = (f) => relative(SRC, f);

describe("the editor stays behind its loader", () => {
  it("is imported with import(), and nowhere else statically, so it is never in the initial chunk", () => {
    const staticEntry = files.filter((f) => /^\s*import[^;]*from\s+["']\.\/entry(\.ts)?["']/m.test(text(f)) && !/import type/.test(text(f).match(/^.*from\s+["']\.\/entry(\.ts)?["'].*$/m)?.[0] ?? ""));
    expect(staticEntry.map(rel)).toEqual([]);
    const dynamic = files.filter((f) => /import\(\s*["']\.\/entry["']\s*\)/.test(text(f))).map(rel);
    expect(dynamic).toEqual(["editor/loader.ts"]);
  });

  it("is the only code that imports monaco-editor", () => {
    const users = files.filter((f) => /from\s+["']monaco-editor|import\s+["']monaco-editor/.test(text(f))).map(rel);
    expect(users.every((u) => u.startsWith("editor/"))).toBe(true);
  });

  it("installs the environment before it imports the entry", () => {
    const loader = text(join(SRC, "editor/loader.ts"));
    const code = loader.replace(/^\s*\/\/.*$/gm, "");
    expect(code.indexOf("installEditorEnvironment();")).toBeGreaterThan(-1);
    expect(code.indexOf("installEditorEnvironment();")).toBeLessThan(code.indexOf('await import("./entry")'));
  });

  it("leaves out every contribution that renders Markdown", () => {
    const entry = text(join(SRC, "editor/entry.ts"));
    for (const banned of ["hover/browser/hoverContribution", "suggest/browser/suggestController", "suggest/browser/suggestInlineCompletions", "parameterHints/browser/parameterHints", "inlineCompletions/browser/inlineCompletions.contribution", "colorPicker/browser/colorPickerContribution", "links/browser/links", "gotoSymbol/browser/link/goToDefinitionAtPosition", "codelens/browser/codelensController", "inlayHints/browser/inlayHintsContribution", "referenceSearch/standaloneReferenceSearch", "gotoSymbol/browser/goToCommands", "quickAccess/standaloneGotoSymbolQuickAccess"]) {
      expect(entry, banned).not.toContain(banned);
    }
  });

  it("keeps only the languages it needs", () => {
    const entry = text(join(SRC, "editor/entry.ts"));
    const languages = [...entry.matchAll(/languages\/definitions\/([a-z0-9-]+)\/register/g)].map((m) => m[1]).sort();
    expect(languages).toEqual(["ini", "xml", "yaml"]);
  });
});
