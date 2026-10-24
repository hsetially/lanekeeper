import { describe, expect, it } from "vitest";
import type { Comparison, DiffHunk, SettingChange } from "@/api/generated/model";
import { hunksAround, settingSummary } from "./drawer";

const hunk = (oldStart: number, oldLines: number, newStart: number, newLines: number): DiffHunk => ({ old_start: oldStart, old_lines: oldLines, new_start: newStart, new_lines: newLines, lines: [] });
const change = (path: string, kind: SettingChange["kind"]): SettingChange => ({ path, kind, left: null, right: null, left_location: null, right_location: null, severity: null });
const comparison = (changes: SettingChange[], over: Partial<Comparison> = {}): Comparison => ({
  left: "effective:sit",
  right: "effective:sitb",
  file: "a.yml",
  class: "structured",
  identical: false,
  eol_only: false,
  hunks: [],
  setting_changes: changes,
  binary: null,
  ...over,
});

describe("what the server says about a setting in two files", () => {
  it("reads a change of the setting as the server reports it", () => {
    expect(settingSummary("a.b", comparison([change("a.b", "changed")]), "sit", "sitb")).toEqual({ text: "Value differs", same: false });
    expect(settingSummary("a.b", comparison([change("a.b", "added")]), "sit", "sitb")).toEqual({ text: "Absent in sit, present in sitb", same: false });
    expect(settingSummary("a.b", comparison([change("a.b", "removed")]), "sit", "sitb")).toEqual({ text: "Present in sit, absent in sitb", same: false });
    expect(settingSummary("a.b", comparison([change("a.b", "type_changed")]), "sit", "sitb")).toEqual({ text: "Same text, different type", same: false });
  });

  it("calls a setting the same when the server lists no change for it", () => {
    expect(settingSummary("a.b", comparison([change("other", "changed")]), "sit", "sitb")).toEqual({ text: "Same value in both files", same: true });
  });

  it("does not say the setting is the same when the file was not compared setting by setting", () => {
    expect(settingSummary("a.b", comparison([], { class: "text" }), "sit", "sitb")).toEqual({ text: "The lines of the two files, as compared", same: false });
  });
});

describe("the hunks near a setting", () => {
  const hunks = [hunk(10, 7, 10, 8), hunk(200, 5, 215, 5), hunk(900, 4, 950, 4)];

  it("keeps the hunks that touch the lines of the setting in either file, with a few lines around", () => {
    expect(hunksAround(hunks, { start: 12, end: 12 }, { start: 12, end: 12 })).toEqual([hunks[0]]);
    expect(hunksAround(hunks, { start: 203, end: 203 }, { start: 218, end: 218 })).toEqual([hunks[1]]);
    // The line is far from the hunk in the left file and inside it in the right one.
    expect(hunksAround(hunks, { start: 500, end: 500 }, { start: 952, end: 952 })).toEqual([hunks[2]]);
  });

  it("keeps nothing when the setting is not near a hunk or a side has no line", () => {
    expect(hunksAround(hunks, { start: 500, end: 500 }, { start: 500, end: 500 })).toEqual([]);
    expect(hunksAround(hunks, null, null)).toEqual([]);
  });

  it("is bounded", () => {
    const many = Array.from({ length: 50 }, (_, i) => hunk(10 + i, 2, 10 + i, 2));
    expect(hunksAround(many, { start: 30, end: 30 }, { start: 30, end: 30 }).length).toBeLessThanOrEqual(4);
  });

  it("takes a hunk with no lines on one side as the single line it sits at", () => {
    expect(hunksAround([hunk(20, 0, 21, 3)], { start: 20, end: 20 }, { start: 400, end: 400 })).toHaveLength(1);
  });
});
