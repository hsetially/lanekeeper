import { describe, expect, it } from "vitest";
import { forAll } from "../../test/prng.ts";
import { makeComparison, type DiffShape } from "./diff.ts";

// The 6,000-line diff of the P10 gate. It must look like what the engine sends (crates/engine/src/diff/text.rs): hunks with
// at most three lines of context each side, numbered as a unified diff, removed lines before the added lines that replace
// them, and changed spans as UTF-8 byte offsets on character boundaries. A generator that did not would measure a view
// of data the server never sends.

const enc = new TextEncoder();
const SHAPES: DiffShape[] = [{ lines: 6000, changeEvery: 10 }, { lines: 6000, changeEvery: 25 }, { lines: 600, changeEvery: 7 }, { lines: 6000, full: true }, { lines: 50, changeEvery: 4 }];

describe("makeComparison", () => {
  it("is a comparison of one file with hunks and nothing else to show", () => {
    const c = makeComparison({ lines: 6000, changeEvery: 10 });
    expect(c.identical).toBe(false);
    expect(c.eol_only).toBe(false);
    expect(c.binary).toBeNull();
    expect(c.hunks.length).toBeGreaterThan(100);
    expect(c.class).toBe("text");
  });

  it("is the same every time, so a run measures the code and not the data", () => {
    expect(makeComparison({ lines: 600, changeEvery: 7 })).toEqual(makeComparison({ lines: 600, changeEvery: 7 }));
  });

  it("covers a file of the asked length: the hunks lie inside it, apart, in order", () => {
    for (const shape of SHAPES) {
      const c = makeComparison(shape);
      let end = 0;
      for (const h of c.hunks) {
        const before = h.old_lines === 0 ? h.old_start : h.old_start - 1;
        expect(before, JSON.stringify(shape)).toBeGreaterThanOrEqual(end);
        end = before + h.old_lines;
        expect(end).toBeLessThanOrEqual(shape.lines);
      }
    }
  });

  it("counts each hunk's lines the way a unified diff does", () => {
    for (const shape of SHAPES) {
      for (const h of makeComparison(shape).hunks) {
        expect(h.old_lines).toBe(h.lines.filter((l) => l.kind !== "added").length);
        expect(h.new_lines).toBe(h.lines.filter((l) => l.kind !== "removed").length);
      }
    }
  });

  it("has at most three lines of context each side of a change, and puts removed lines before the added ones that replace them", () => {
    const c = makeComparison({ lines: 6000, changeEvery: 10 });
    for (const h of c.hunks) {
      const kinds = h.lines.map((l) => l.kind);
      const firstChange = kinds.findIndex((k) => k !== "context");
      const lastChange = kinds.length - 1 - [...kinds].reverse().findIndex((k) => k !== "context");
      expect(firstChange).toBeLessThanOrEqual(3);
      expect(kinds.length - 1 - lastChange).toBeLessThanOrEqual(3);
      for (let i = 1; i < kinds.length; i++) if (kinds[i] === "removed") expect(kinds[i - 1]).not.toBe("added");
    }
  });

  it("makes a change about every ten lines in the realistic shape: a few hundred hunks (changes six lines apart or closer share one), a few thousand changed lines", () => {
    const c = makeComparison({ lines: 6000, changeEvery: 10 });
    const changed = c.hunks.flatMap((h) => h.lines).filter((l) => l.kind !== "context").length;
    expect(c.hunks.length).toBeGreaterThan(350);
    expect(c.hunks.length).toBeLessThanOrEqual(600);
    // A change removes one to three lines and adds one to three, so two thousand or so lines are changed in all.
    expect(changed).toBeGreaterThan(1800);
    expect(changed).toBeLessThan(3000);
  });

  it("in the full shape changes every line: 6,000 removed and 6,000 added, as one hunk", () => {
    const c = makeComparison({ lines: 6000, full: true });
    expect(c.hunks).toHaveLength(1);
    expect(c.hunks[0]?.lines.filter((l) => l.kind === "removed")).toHaveLength(6000);
    expect(c.hunks[0]?.lines.filter((l) => l.kind === "added")).toHaveLength(6000);
  });

  it("gives changed spans as byte offsets on character boundaries, inside the line, ordered and apart", () => {
    for (const shape of SHAPES) {
      for (const h of makeComparison(shape).hunks) {
        for (const line of h.lines) {
          const bytes = enc.encode(line.text);
          let last = 0;
          for (const r of line.inline_changes) {
            expect(r.start).toBeGreaterThanOrEqual(last);
            expect(r.end).toBeGreaterThan(r.start);
            expect(r.end).toBeLessThanOrEqual(bytes.length);
            expect(r.start === 0 || ((bytes[r.start] ?? 0) & 0xc0) !== 0x80).toBe(true);
            expect(r.end === bytes.length || ((bytes[r.end] ?? 0) & 0xc0) !== 0x80).toBe(true);
            last = r.end;
          }
          if (line.kind === "context") expect(line.inline_changes).toEqual([]);
        }
      }
    }
  });

  it("includes lines with accents, symbols and emoji, so the byte-offset conversion is exercised", () => {
    const text = makeComparison({ lines: 600, changeEvery: 7 }).hunks.flatMap((h) => h.lines).map((l) => l.text).join("\n");
    expect(enc.encode(text).length).toBeGreaterThan(text.length);
  });

  it("keeps every size the contract allows: lines under the schema limits", () => {
    const c = makeComparison({ lines: 6000, full: true });
    expect(c.hunks.length).toBeLessThanOrEqual(100_000);
    for (const h of c.hunks) expect(h.lines.length).toBeLessThanOrEqual(100_000);
    for (const l of c.hunks.flatMap((h) => h.lines)) expect(l.inline_changes.length).toBeLessThanOrEqual(10_000);
  });

  it("property: any shape gives consistent hunks (500 cases)", () => {
    forAll(
      500,
      (rng) => ({ lines: 20 + rng.int(800), changeEvery: 3 + rng.int(30) }),
      (shape) => {
        const c = makeComparison(shape);
        let end = 0;
        for (const h of c.hunks) {
          const before = h.old_lines === 0 ? h.old_start : h.old_start - 1;
          expect(before).toBeGreaterThanOrEqual(end);
          end = before + h.old_lines;
          expect(h.old_lines).toBe(h.lines.filter((l) => l.kind !== "added").length);
          expect(h.new_lines).toBe(h.lines.filter((l) => l.kind !== "removed").length);
        }
        expect(end).toBeLessThanOrEqual(shape.lines);
      },
    );
  }, 30_000);
});
