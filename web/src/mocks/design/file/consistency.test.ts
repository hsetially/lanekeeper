import { describe, expect, it } from "vitest";
import type { Comparison, DiffHunk } from "@/api/generated/model";
import { planMasks } from "@/components/masking/plan";
import { coreComparison, coreFile, coreGitText } from "./data";

// A comparison in a scenario is the engine's answer for two texts, so it has to be true of those texts: a screen that folds the
// unchanged lines between hunks (the file view's Diff tab) reads the file and the hunks together, and a hunk that points at
// the wrong line makes a fold show the wrong lines or the wrong count. The mock is checked here against the texts it stands for.

const linesOf = (text: string): string[] => {
  const lines = text.split(/\r?\n/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
};

const own = (hunk: DiffHunk, kinds: readonly string[]): string[] => hunk.lines.filter((l) => kinds.includes(l.kind)).map((l) => l.text);

function check(comparison: Comparison, leftText: string, rightText: string): void {
  const left = linesOf(leftText);
  const right = linesOf(rightText);
  let previousEnd = 0;
  for (const hunk of comparison.hunks) {
    const before = own(hunk, ["context", "removed"]);
    const after = own(hunk, ["context", "added"]);
    expect(hunk.old_lines, `old_lines of the hunk at ${hunk.old_start}`).toBe(before.length);
    expect(hunk.new_lines, `new_lines of the hunk at ${hunk.old_start}`).toBe(after.length);
    expect(left.slice(hunk.old_start - 1, hunk.old_start - 1 + before.length), `the left side at line ${hunk.old_start}`).toEqual(before);
    expect(right.slice(hunk.new_start - 1, hunk.new_start - 1 + after.length), `the right side at line ${hunk.new_start}`).toEqual(after);
    expect(hunk.old_start, "hunks are in order and do not overlap").toBeGreaterThan(previousEnd);
    previousEnd = hunk.old_start + hunk.old_lines - 1;
    for (const line of hunk.lines) {
      for (const range of line.inline_changes) {
        expect(range.start).toBeLessThan(range.end);
        expect(range.end).toBeLessThanOrEqual(line.text.length);
      }
    }
  }
}

describe("the core file's comparison in the scenarios", () => {
  const nfs = coreFile({ masked: false }).content.text ?? "";
  const git = coreGitText();
  const comparison = coreComparison("git:sit7", "nfs:sitb");

  it("is true of the two texts: every hunk's lines are the lines of its side at the place it says, and its counts are right", () => {
    check(comparison, git, nfs);
  });

  it("points each changed setting at the line that has it, on each side that has one", () => {
    const left = linesOf(git);
    const right = linesOf(nfs);
    for (const change of comparison.setting_changes) {
      const leaf = change.path.slice(change.path.lastIndexOf(".") + 1);
      if (change.left_location?.lines) expect(left[change.left_location.lines.start - 1], `${change.path} on the left`).toContain(leaf);
      if (change.right_location?.lines) expect(right[change.right_location.lines.start - 1], `${change.path} on the right`).toContain(leaf);
      if (change.kind !== "added") expect(change.left_location, `${change.path} has a place on the left`).not.toBeNull();
      if (change.kind !== "removed") expect(change.right_location, `${change.path} has a place on the right`).not.toBeNull();
    }
  });

  it("names the two changes the design shows in the hunks it has", () => {
    const changed = comparison.hunks.flatMap((h) => h.lines.filter((l) => l.kind !== "context").map((l) => `${l.kind}:${l.text.trim()}`));
    expect(changed).toEqual(["removed:enableAccountSorting: false", "added:enableAccountSorting: true", "added:sortOrder: asc", "removed:level: WARN", "added:level: INFO"]);
  });
});

// A mask is UTF-8 byte offsets into the whole text, and the web believes nothing it cannot check (it hides a line whose mask is not
// exactly a marker). The core file's first line has a middle dot (two bytes), so offsets counted in characters are one byte short from
// the second line on: the first file view dropped such a mask without a word and drew the line as text, which hid this mock's error.
describe("the masks of the core file in the scenarios", () => {
  it("point at exactly the marker, counted in UTF-8 bytes, so that the file view draws the value as hidden and not the line", () => {
    const { content } = coreFile({ masked: true });
    const plan = planMasks(content.text ?? "", content.masks);
    expect(plan.status).toBe("ok");
    if (plan.status !== "ok") return;
    const line = plan.lines.get(19);
    expect(line?.kind).toBe("marks");
    if (line?.kind === "marks") expect(line.marks.map((m) => m.info.rule)).toEqual(["generic_secret"]);
  });

  it("names no mask when the file is not masked", () => {
    expect(coreFile({ masked: false }).content.masks).toBeUndefined();
  });
});
