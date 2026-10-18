import { describe, expect, it } from "vitest";
import type { DiffHunk, DiffLineKind } from "@/api/generated/model";
import { buildDiffRows } from "@/components/app/diff/rows";
import { countChangeRuns } from "./runs";

// The Diff tab says "3 changes" before the person has pressed `n`: how many runs of changed lines the hunks hold. It must be the
// number DiffView walks with `n` and `p`, so the two are compared on the same hunks.

const hunk = (old_start: number, new_start: number, kinds: DiffLineKind[]): DiffHunk => ({
  old_start,
  new_start,
  old_lines: kinds.filter((k) => k !== "added").length,
  new_lines: kinds.filter((k) => k !== "removed").length,
  lines: kinds.map((kind) => ({ kind, text: kind, inline_changes: [] })),
});

describe("countChangeRuns", () => {
  it("is zero for no hunks and for hunks with only context", () => {
    expect(countChangeRuns([])).toBe(0);
    expect(countChangeRuns([hunk(1, 1, ["context", "context"])])).toBe(0);
  });
  it("counts a removed and an added line next to each other as one change", () => {
    expect(countChangeRuns([hunk(1, 1, ["context", "removed", "added", "context"])])).toBe(1);
  });
  it("counts runs separated by context apart, and runs in separate hunks apart", () => {
    expect(countChangeRuns([hunk(1, 1, ["removed", "context", "added", "context", "added"])])).toBe(3);
    expect(countChangeRuns([hunk(1, 1, ["added"]), hunk(9, 10, ["added"])])).toBe(2);
  });
  it("agrees with the runs DiffView walks, in both layouts", () => {
    const hunks = [hunk(1, 1, ["context", "removed", "removed", "added", "context", "added", "context"]), hunk(20, 21, ["removed", "context"]), hunk(40, 40, ["added", "added"])];
    for (const mode of ["split", "unified"] as const) expect(countChangeRuns(hunks)).toBe(buildDiffRows(hunks, { mode }).changeStarts.length);
  });
});
