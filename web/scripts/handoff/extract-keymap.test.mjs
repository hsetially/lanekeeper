import { describe, expect, it } from "vitest";
import { buildKeymap, parseKeys, renderKeymapTs } from "./extract-keymap.mjs";

describe("parseKeys", () => {
  it("maps glyphs to Playwright key names with ⌘ as ControlOrMeta", () => {
    expect(parseKeys("⌘K").bindings).toEqual([{ steps: ["ControlOrMeta+k"] }]);
    expect(parseKeys("⌘↵").bindings).toEqual([{ steps: ["ControlOrMeta+Enter"] }]);
    expect(parseKeys("Esc").bindings).toEqual([{ steps: ["Escape"] }]);
    expect(parseKeys("/").bindings).toEqual([{ steps: ["/"] }]);
    expect(parseKeys("?").bindings).toEqual([{ steps: ["?"] }]);
  });

  it("expands sequences, alternatives and ranges", () => {
    expect(parseKeys("g then s / c / f / i / a").bindings.map((b) => b.steps)).toEqual([
      ["g", "s"], ["g", "c"], ["g", "f"], ["g", "i"], ["g", "a"],
    ]);
    expect(parseKeys("↑ ↓").bindings.map((b) => b.steps[0])).toEqual(["ArrowUp", "ArrowDown"]);
    expect(parseKeys("↑ ↓ ↵").bindings.map((b) => b.steps[0])).toEqual(["ArrowUp", "ArrowDown", "Enter"]);
    expect(parseKeys("→ / ←").bindings.map((b) => b.steps[0])).toEqual(["ArrowRight", "ArrowLeft"]);
    expect(parseKeys("Space or x").bindings.map((b) => b.steps[0])).toEqual(["Space", "x"]);
    expect(parseKeys("Tab, ⇧Tab").bindings.map((b) => b.steps[0])).toEqual(["Tab", "Shift+Tab"]);
    expect(parseKeys("⌘1 … ⌘9").bindings.map((b) => b.steps[0])).toEqual(
      ["1", "2", "3", "4", "5", "6", "7", "8", "9"].map((n) => `ControlOrMeta+${n}`),
    );
  });

  it("keeps the context a key only applies in", () => {
    expect(parseKeys("↵ on fold row").bindings).toEqual([{ steps: ["Enter"], when: "fold row" }]);
    expect(parseKeys("Backspace on empty").bindings).toEqual([{ steps: ["Backspace"], when: "empty" }]);
  });

  it("refuses a key notation it does not know", () => {
    expect(() => parseKeys("Hyper+Q")).toThrow(/unknown key/i);
  });
});

describe("extract-keymap (the keys table)", () => {
  const data = buildKeymap();

  it("reads the 31 rows, each under its scope, with a unique id", () => {
    expect(data.rows).toHaveLength(31);
    expect(new Set(data.rows.map((r) => r.id)).size).toBe(31);
    expect(data.rows[0]).toMatchObject({ scope: "Global", keys: "⌘K", action: "Command palette" });
    expect(data.rows[5]).toMatchObject({ scope: "Go to", keys: "g then s / c / f / i / a" });
    expect(data.rows.filter((r) => r.scope === "Global")).toHaveLength(5);
  });

  it("pairs a multi-binding row with the parts of its action", () => {
    const goTo = data.rows.find((r) => r.scope === "Go to");
    expect(goTo.bindings.map((b) => b.action)).toEqual(["Swimlanes", "Compare", "Feature status", "Findings", "Audit"]);
    const tree = data.rows.find((r) => r.keys === "→ / ←");
    expect(tree.bindings.map((b) => b.action)).toEqual([
      "Expand or move to first child",
      "collapse or move to parent",
    ]);
  });

  it("handles the rows that name no key explicitly", () => {
    const typeAhead = data.rows.find((r) => r.keys === "type-ahead");
    expect(typeAhead.bindings).toEqual([{ kind: "typeahead", action: typeAhead.action }]);
    const toggles = data.rows.find((r) => r.keys === "Match case, Regex");
    expect(toggles.skip).toMatch(/no shortcut assigned/);
    expect(toggles.bindings).toEqual([]);
    expect(data.rows.filter((r) => r.skip)).toHaveLength(1);
  });

  it("fails on a no-key row that is not in the allow-list", () => {
    expect(() => buildKeymap({ raw: [["^Scope", "_mystery", "Something"]], note: "" })).toThrow(/no key/i);
  });

  it("keeps the conflict note about the two shortcut sets", () => {
    expect(data.note).toMatch(/J\/K/);
    expect(renderKeymapTs(data)).toContain("export const KEYMAP = ");
  });
});
