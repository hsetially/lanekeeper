import { describe, expect, it } from "vitest";
import type { DiffHunk } from "@/api/generated/model";
import { foldContext } from "./folds";

// Unfolding unchanged lines. The server's hunks carry the changes and a little context; the lines between them are the same on
// both sides, so when the screen has the NFS text it can show them from there. It does so only when the hunks agree with that
// text line for line (a file that changed since the comparison was made would otherwise show the wrong lines), and never
// computes a difference: the lines it shows are the text's own.

const OLD = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
// New: line 4 changed (d -> D), a line "x" added after f.
const NEW = ["a", "b", "c", "D", "e", "f", "x", "g", "h", "i", "j"];
const hunk = (old_start: number, old_lines: number, new_start: number, new_lines: number, lines: Array<[("context" | "added" | "removed"), string]>): DiffHunk => ({
  old_start,
  old_lines,
  new_start,
  new_lines,
  lines: lines.map(([kind, text]) => ({ kind, text, inline_changes: [] })),
});
// Context of one line each side: hunk 1 covers old 3..5, hunk 2 covers old 6..7 (after f, an added x before g).
const HUNKS: DiffHunk[] = [
  hunk(3, 3, 3, 3, [["context", "c"], ["removed", "d"], ["added", "D"], ["context", "e"]]),
  hunk(6, 2, 6, 3, [["context", "f"], ["added", "x"], ["context", "g"]]),
];

describe("foldContext", () => {
  it("gives the lines before the first hunk from the new side", () => {
    const ctx = foldContext(HUNKS, "new", NEW.join("\n"));
    expect(ctx?.getLines("new", 1, 2)).toEqual(["a", "b"]);
    expect(ctx?.getLines("old", 1, 2)).toEqual(["a", "b"]);
  });

  it("maps the other side's numbers across the changes before it", () => {
    const ctx = foldContext(HUNKS, "new", NEW.join("\n"));
    // Old line 8 is "h"; the new text has one more line before it (the added x), so it is new line 9.
    expect(ctx?.getLines("old", 8, 3)).toEqual(["h", "i", "j"]);
    expect(ctx?.getLines("new", 9, 3)).toEqual(["h", "i", "j"]);
  });

  it("works from the old side when the NFS is the left side", () => {
    const ctx = foldContext(HUNKS, "old", OLD.join("\n"));
    expect(ctx?.getLines("old", 8, 3)).toEqual(["h", "i", "j"]);
    expect(ctx?.getLines("new", 9, 3)).toEqual(["h", "i", "j"]);
  });

  it("knows how long the old file is, so a fold after the last hunk can be shown", () => {
    expect(foldContext(HUNKS, "new", NEW.join("\n"))?.oldTotal).toBe(10);
    expect(foldContext(HUNKS, "old", OLD.join("\n"))?.oldTotal).toBe(10);
  });

  it("gives nothing for a range the text does not have", () => {
    const ctx = foldContext(HUNKS, "new", NEW.join("\n"));
    expect(ctx?.getLines("new", 10, 5)).toBeNull();
    expect(ctx?.getLines("new", 0, 1)).toBeNull();
  });

  it("refuses to unfold at all when the hunks do not match the text (the file changed since)", () => {
    const changed = [...NEW];
    changed[4] = "E";
    expect(foldContext(HUNKS, "new", changed.join("\n"))).toBeNull();
  });

  it("reads a Windows file by the same lines the engine counts", () => {
    expect(foldContext(HUNKS, "new", NEW.join("\r\n"))?.getLines("new", 1, 2)).toEqual(["a", "b"]);
    expect(foldContext(HUNKS, "new", NEW.join("\r"))?.getLines("new", 1, 2)).toEqual(["a", "b"]);
  });

  it("copes with a hunk that has no lines on one side (an added block)", () => {
    const added = [hunk(2, 0, 3, 2, [["added", "p"], ["added", "q"]])];
    const text = ["a", "b", "p", "q", "c"].join("\n");
    const ctx = foldContext(added, "new", text);
    expect(ctx?.getLines("old", 3, 1)).toEqual(["c"]);
    expect(ctx?.oldTotal).toBe(3);
  });
});
