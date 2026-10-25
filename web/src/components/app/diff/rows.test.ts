import { describe, expect, it } from "vitest";
import type { DiffHunk, DiffLine } from "@/api/generated/model";
import { forAll, type Rng } from "@/test/prng";
import { buildDiffRows as build, diffStats, ROW_HEIGHT, type DiffRow, type LineRow } from "./rows";

type Options = Parameters<typeof build>[1];
const buildDiffRows = (hunks: DiffHunk[], options: Options): DiffRow[] => build(hunks, options).rows;

// The server computes the hunks (rule 7: facts come from the engine). The browser lays them out: it never compares two
// texts. These tests state the layout rules: how line numbers run, how a removed run and an added run pair up in the split
// view, where the folds go, and which rows are changes. The server's numbering is the unified diff's (1-based; for a side
// with no lines, the line before it): crates/engine/src/diff/text.rs, `start`.

const ctx = (text: string): DiffLine => ({ kind: "context", text, inline_changes: [] });
const del = (text: string, ranges: Array<[number, number]> = []): DiffLine => ({ kind: "removed", text, inline_changes: ranges.map(([start, end]) => ({ start, end })) });
const add = (text: string, ranges: Array<[number, number]> = []): DiffLine => ({ kind: "added", text, inline_changes: ranges.map(([start, end]) => ({ start, end })) });
const hunk = (old_start: number, old_lines: number, new_start: number, new_lines: number, lines: DiffLine[]): DiffHunk => ({ old_start, old_lines, new_start, new_lines, lines });

const lineRows = (rows: DiffRow[]): LineRow[] => rows.filter((r): r is LineRow => r.kind === "line");
const shape = (rows: DiffRow[]) =>
  rows.map((r) => {
    if (r.kind === "hunk") return `@ ${r.header}`;
    if (r.kind === "fold") return `fold ${r.hidden}`;
    return `${r.left ? `${r.left.no}${r.left.kind === "removed" ? "-" : "="}${r.left.text}` : "_"} | ${r.right ? `${r.right.no}${r.right.kind === "added" ? "+" : "="}${r.right.text}` : "_"}`;
  });

describe("buildDiffRows, unified", () => {
  const h = hunk(10, 4, 10, 4, [ctx("a"), del("b"), add("B"), ctx("c")]);

  it("starts with a hunk header, then one row for each line, each with its number on its side", () => {
    const rows = buildDiffRows([h], { mode: "unified" });
    expect(shape(rows)).toEqual(["fold 9", "@ @@ -10,4 +10,4 @@", "10=a | 10=a", "11-b | _", "_ | 11+B", "12=c | 12=c"]);
  });

  it("does not put a fold before a hunk that starts at the first line", () => {
    const rows = buildDiffRows([hunk(1, 2, 1, 2, [ctx("a"), ctx("b")])], { mode: "unified" });
    expect(rows[0]?.kind).toBe("hunk");
  });

  it("says how many unchanged lines lie between two hunks", () => {
    const rows = buildDiffRows([hunk(1, 2, 1, 2, [ctx("a"), del("b")]), hunk(20, 2, 20, 2, [add("x"), ctx("y")])], { mode: "unified" });
    expect(shape(rows).filter((s) => s.startsWith("fold"))).toEqual(["fold 17"]);
  });

  it("adds a trailing fold only when it is told how long the file is", () => {
    const hunks = [hunk(1, 2, 1, 2, [ctx("a"), del("b")])];
    expect(buildDiffRows(hunks, { mode: "unified" }).at(-1)?.kind).toBe("line");
    expect(shape(buildDiffRows(hunks, { mode: "unified", oldTotal: 100 })).at(-1)).toBe("fold 98");
    expect(buildDiffRows(hunks, { mode: "unified", oldTotal: 2 }).at(-1)?.kind).toBe("line");
  });

  it("follows the unified diff's rule for a side with no lines: its start is the line before", () => {
    // Three lines added after line 5 of the old file: -5,0 +6,3.
    const rows = buildDiffRows([hunk(5, 0, 6, 3, [add("x"), add("y"), add("z")])], { mode: "unified" });
    expect(shape(rows)).toEqual(["fold 5", "@ @@ -5,0 +6,3 @@", "_ | 6+x", "_ | 7+y", "_ | 8+z"]);
    // Two lines removed from the top: -1,2 +0,0.
    expect(shape(buildDiffRows([hunk(1, 2, 0, 0, [del("a"), del("b")])], { mode: "unified" }))).toEqual(["@ @@ -1,2 +0,0 @@", "1-a | _", "2-b | _"]);
  });

  it("gives every row the height of its kind: 18 for a line, 20 for a hunk header, 24 for a fold", () => {
    expect(ROW_HEIGHT).toEqual({ line: 18, hunk: 20, fold: 24 });
  });

  it("is empty for no hunks", () => {
    expect(build([], { mode: "unified" })).toEqual({ rows: [], changeStarts: [] });
    expect(build([], { mode: "split" })).toEqual({ rows: [], changeStarts: [] });
  });
});

describe("buildDiffRows, split", () => {
  it("puts a removed line opposite the added line that replaces it", () => {
    const rows = buildDiffRows([hunk(1, 3, 1, 3, [ctx("a"), del("b"), add("B"), ctx("c")])], { mode: "split" });
    expect(shape(rows)).toEqual(["@ @@ -1,3 +1,3 @@", "1=a | 1=a", "2-b | 2+B", "3=c | 3=c"]);
  });

  it("pairs a run of removed lines with the run of added lines that follows, row by row", () => {
    const rows = buildDiffRows([hunk(1, 3, 1, 2, [del("a"), del("b"), del("c"), add("X"), add("Y")])], { mode: "split" });
    expect(shape(rows)).toEqual(["@ @@ -1,3 +1,2 @@", "1-a | 1+X", "2-b | 2+Y", "3-c | _"]);
    const more = buildDiffRows([hunk(1, 1, 1, 3, [del("a"), add("X"), add("Y"), add("Z")])], { mode: "split" });
    expect(shape(more)).toEqual(["@ @@ -1,1 +1,3 @@", "1-a | 1+X", "_ | 2+Y", "_ | 3+Z"]);
  });

  it("leaves the other side empty for lines that were only added or only removed", () => {
    expect(shape(buildDiffRows([hunk(2, 0, 3, 1, [add("x")])], { mode: "split" }))).toEqual(["fold 2", "@ @@ -2,0 +3,1 @@", "_ | 3+x"]);
    expect(shape(buildDiffRows([hunk(3, 1, 2, 0, [del("x")])], { mode: "split" }))).toEqual(["fold 2", "@ @@ -3,1 +2,0 @@", "3-x | _"]);
  });

  it("makes a row for every context line and keeps two separate changes apart", () => {
    const rows = buildDiffRows([hunk(1, 5, 1, 5, [del("a"), add("A"), ctx("m"), ctx("n"), del("b"), add("B")])], { mode: "split" });
    expect(shape(rows)).toEqual(["@ @@ -1,5 +1,5 @@", "1-a | 1+A", "2=m | 2=m", "3=n | 3=n", "4-b | 4+B"]);
  });

  it("has no more rows than the unified view, and fewer when lines pair up", () => {
    const hunks = [hunk(1, 4, 1, 4, [del("a"), del("b"), add("A"), add("B"), ctx("c"), ctx("d")])];
    expect(lineRows(buildDiffRows(hunks, { mode: "split" }))).toHaveLength(4);
    expect(lineRows(buildDiffRows(hunks, { mode: "unified" }))).toHaveLength(6);
  });
});

describe("change rows", () => {
  it("lists where each run of changed rows begins, so n and p can jump between them", () => {
    const hunks = [hunk(1, 6, 1, 6, [ctx("a"), del("b"), add("B"), ctx("c"), ctx("d"), add("e")]), hunk(30, 2, 30, 3, [ctx("x"), add("y"), add("z")])];
    const layout = build(hunks, { mode: "split" });
    const rows = layout.rows;
    expect(layout.changeStarts.map((i) => shape(rows)[i])).toEqual(["2-b | 2+B", "_ | 5+e", "_ | 31+y"]);
    expect(rows.filter((r) => r.kind === "line" && r.change)).toHaveLength(4);
  });

  it("counts a changed run once however many rows it has, in either mode", () => {
    const hunks = [hunk(1, 3, 1, 3, [del("a"), del("b"), del("c"), add("A"), add("B"), add("C")])];
    expect(build(hunks, { mode: "unified" }).changeStarts).toHaveLength(1);
    expect(build(hunks, { mode: "split" }).changeStarts).toHaveLength(1);
  });

  it("is empty when nothing changed (a file that differs only in line endings has no hunks)", () => {
    expect(build([], { mode: "split" }).changeStarts).toEqual([]);
  });
});

describe("folds", () => {
  const hunks = [hunk(1, 2, 1, 2, [ctx("a"), del("b")]), hunk(6, 2, 6, 2, [add("y"), ctx("z")])];

  it("shows the lines of a fold the person opened, as context rows with their numbers on both sides", () => {
    const rows = buildDiffRows(hunks, { mode: "unified", expanded: new Set(["fold-3"]), getLines: (side, from, count) => Array.from({ length: count }, (_, i) => `${side}${from + i}`) });
    expect(shape(rows)).toEqual(["@ @@ -1,2 +1,2 @@", "1=a | 1=a", "2-b | _", "3=old3 | 3=new3", "4=old4 | 4=new4", "5=old5 | 5=new5", "@ @@ -6,2 +6,2 @@", "_ | 6+y", "6=z | 7=z"]);
  });

  it("keeps the fold when the text is not available", () => {
    const rows = buildDiffRows(hunks, { mode: "unified", expanded: new Set(["fold-3"]), getLines: () => null });
    expect(shape(rows)).toContain("fold 3");
  });

  it("names a fold by the old line it starts at, so it survives a change of mode", () => {
    const rows = buildDiffRows(hunks, { mode: "split" });
    const fold = rows.find((r) => r.kind === "fold");
    expect(fold).toMatchObject({ id: "fold-3", hidden: 3, oldFrom: 3, newFrom: 3 });
  });
});

describe("diffStats", () => {
  it("counts added and removed lines over all hunks", () => {
    expect(diffStats([hunk(1, 3, 1, 3, [del("a"), add("A"), add("B")]), hunk(9, 1, 9, 1, [del("z")])])).toEqual({ added: 2, removed: 2 });
    expect(diffStats([])).toEqual({ added: 0, removed: 0 });
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// Properties: whatever hunks the server sends, the layout holds together.

function randomHunks(rng: Rng): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  // How many lines of each side lie before the next hunk. The lines between two hunks are the same on both sides.
  let oldPos = 0;
  let newPos = 0;
  for (let i = rng.int(5); i > 0; i--) {
    const gap = rng.int(6) + 1;
    const oldBefore = oldPos + gap;
    const newBefore = newPos + gap;
    const lines: DiffLine[] = [];
    for (let p = rng.int(4) + 1; p > 0; p--) {
      for (let k = rng.int(3); k > 0; k--) lines.push(ctx(`c${i}.${p}.${k}`));
      for (let k = rng.int(3); k > 0; k--) lines.push(del(`d${i}.${p}.${k}`));
      for (let k = rng.int(3); k > 0; k--) lines.push(add(`a${i}.${p}.${k}`));
    }
    if (lines.length === 0) lines.push(add("only"));
    const oldLines = lines.filter((l) => l.kind !== "added").length;
    const newLines = lines.filter((l) => l.kind !== "removed").length;
    hunks.push(hunk(oldLines === 0 ? oldBefore : oldBefore + 1, oldLines, newLines === 0 ? newBefore : newBefore + 1, newLines, lines));
    oldPos = oldBefore + oldLines;
    newPos = newBefore + newLines;
  }
  return hunks;
}

describe("buildDiffRows properties", () => {
  it("never throws, whatever the hunks (10,000 cases)", () => {
    forAll(10_000, randomHunks, (hunks) => {
      for (const mode of ["unified", "split"] as const) expect(() => buildDiffRows(hunks, { mode })).not.toThrow();
    });
  }, 30_000);

  it("keeps every line of every hunk, once, in order: the unified view is the hunks, flattened (10,000 cases)", () => {
    forAll(10_000, randomHunks, (hunks) => {
      const rows = buildDiffRows(hunks, { mode: "unified" });
      const texts = lineRows(rows).map((r) => (r.left ?? r.right)?.text);
      expect(texts).toEqual(hunks.flatMap((h) => h.lines.map((l) => l.text)));
    });
  }, 30_000);

  it("keeps every line in the split view too, and puts each line on the side it belongs to (10,000 cases)", () => {
    forAll(10_000, randomHunks, (hunks) => {
      const rows = lineRows(buildDiffRows(hunks, { mode: "split" }));
      const left = rows.flatMap((r) => (r.left ? [r.left] : []));
      const right = rows.flatMap((r) => (r.right ? [r.right] : []));
      const all = hunks.flatMap((h) => h.lines);
      expect(left.map((c) => c.text)).toEqual(all.filter((l) => l.kind !== "added").map((l) => l.text));
      expect(right.map((c) => c.text)).toEqual(all.filter((l) => l.kind !== "removed").map((l) => l.text));
      expect(left.every((c) => c.kind !== "added")).toBe(true);
      expect(right.every((c) => c.kind !== "removed")).toBe(true);
    });
  }, 30_000);

  it("numbers each side upward, one at a time inside a hunk (10,000 cases)", () => {
    forAll(10_000, randomHunks, (hunks) => {
      for (const mode of ["unified", "split"] as const) {
        const rows = lineRows(buildDiffRows(hunks, { mode }));
        for (const side of ["left", "right"] as const) {
          const numbers = rows.flatMap((r) => (r[side] ? [r[side].no] : []));
          for (let i = 1; i < numbers.length; i++) expect(numbers[i] ?? 0).toBeGreaterThan(numbers[i - 1] ?? 0);
        }
      }
    });
  }, 30_000);

  it("is deterministic: the same hunks give the same rows, and different modes agree on the changes (10,000 cases)", () => {
    forAll(10_000, randomHunks, (hunks) => {
      const a = buildDiffRows(hunks, { mode: "split" });
      const b = buildDiffRows(hunks, { mode: "split" });
      expect(a).toEqual(b);
      const changed = (mode: "split" | "unified") => lineRows(buildDiffRows(hunks, { mode })).flatMap((r) => [r.left, r.right]).filter((c) => c && c.kind !== "context").map((c) => c?.text);
      expect([...changed("split")].sort()).toEqual([...changed("unified")].sort());
    });
  }, 30_000);
});
