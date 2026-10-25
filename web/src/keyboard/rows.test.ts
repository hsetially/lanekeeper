import { describe, expect, it } from "vitest";
import { productSources, withoutComments } from "@/test/source-files";
import { OWNERS } from "./bindings";
import { bindingCaps, capsText, rowCaps, stepCaps } from "./format";
import { ALL_ROWS, KEYMAP_ROWS, LOCAL_ROWS, bindingsOf, isShortcutId } from "./rows";

// Every key the app binds is a row of the handoff's keymap or of the reviewed screen-local list (H-11). A slice that needs a
// key the map does not have asks for the row to be added; it cannot invent one.

describe("the rows", () => {
  it("are the handoff's rows and the reviewed local ones, with distinct ids", () => {
    expect(KEYMAP_ROWS.length).toBeGreaterThan(0);
    expect(LOCAL_ROWS.length).toBeGreaterThan(0);
    const ids = ALL_ROWS.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.filter((id) => id.startsWith("local."))).toEqual(LOCAL_ROWS.map((r) => r.id));
  });

  it("parse: every binding with steps is readable, and a row without a key is explained", () => {
    for (const row of ALL_ROWS) {
      if (row.bindings.length === 0) expect(row.skip, `${row.id} has no key and no reason`).toBeTruthy();
      for (const binding of bindingsOf(row.id)) expect(binding.steps.length).toBeGreaterThan(0);
    }
  });
});

describe("the owners table", () => {
  it("has exactly one entry for every row, and none for a row that is not there", () => {
    expect(Object.keys(OWNERS).sort()).toEqual(ALL_ROWS.map((r) => r.id).sort());
  });
});

describe("useShortcut in the source", () => {
  const sources = productSources().map((f) => ({ ...f, code: withoutComments(f.text) }));

  it("names only ids that are rows (a cast around the type is caught here)", () => {
    const bad: string[] = [];
    for (const { path, code } of sources) {
      for (const match of code.matchAll(/useShortcut\(\s*["'`]([^"'`]+)["'`]/g)) {
        const id = match[1] ?? "";
        if (!isShortcutId(id)) bad.push(`${path}: ${id}`);
      }
    }
    expect(bad).toEqual([]);
  });

  it("is never given a computed id, so that this scan sees every binding", () => {
    const bad: string[] = [];
    for (const { path, code } of sources) {
      if (path.startsWith("src/keyboard/")) continue;
      for (const match of code.matchAll(/useShortcut\(\s*([^"'`\s][^,)]*)/g)) bad.push(`${path}: ${match[1] ?? ""}`);
    }
    expect(bad).toEqual([]);
  });

  it("is the only way a source file listens for a key on the window or the document", () => {
    const allowed = new Set(["src/keyboard/KeyboardProvider.tsx"]);
    const bad = sources.filter(({ path, code }) => !allowed.has(path) && /(window|document)\.addEventListener\(\s*["'](keydown|keyup|keypress)["']/.test(code)).map((f) => f.path);
    expect(bad).toEqual([]);
  });
});

describe("key caps", () => {
  it("draw a chord with a capital letter and a sequence with lower case and the word then", () => {
    expect(stepCaps("ControlOrMeta+k")).toEqual(["⌘", "K"]);
    expect(stepCaps("g")).toEqual(["g"]);
    expect(stepCaps("Escape")).toEqual(["Esc"]);
    expect(stepCaps("Shift+Tab")).toEqual(["⇧", "Tab"]);
    expect(stepCaps("ArrowUp")).toEqual(["↑"]);
    expect(bindingCaps(["g", "s"])).toEqual(["g", "then", "s"]);
    expect(rowCaps("global.command-palette")).toEqual(["⌘", "K"]);
    expect(rowCaps("go-to.swimlanes-compare-feature-status-findings", 2)).toEqual(["g", "then", "f"]);
    expect(rowCaps("tree-only.jump-to-the-next-row")).toEqual([]);
    expect(capsText(["⌘", "K"])).toBe("⌘K");
    expect(capsText(["g", "then", "s"])).toBe("g then s");
  });
});
