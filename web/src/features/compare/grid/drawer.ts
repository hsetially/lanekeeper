import type { Comparison, DiffHunk, LineRange } from "@/api/generated/model";

// The drawer of the grid answers one question: do these two swimlanes read the same value at this setting? The server
// answers it (`setting_changes` of the comparison of the two effective files), and the drawer shows the lines of the
// server's hunks around the setting. Nothing here compares two values: that matters most for a hidden value, whose text
// the page does not have, and whose equality only the server can say.

/** The lines on either side of the setting that a hunk may start or end at and still be "near" it. */
const CONTEXT_LINES = 3;
/** The most hunks the drawer shows; a setting sits in one, or two when the files moved. */
const MAX_HUNKS = 4;

/** What the comparison says about one setting: the words, and whether the two files agree. */
export function settingSummary(path: string, comparison: Comparison, a: string, b: string): { text: string; same: boolean } {
  const change = comparison.setting_changes.find((c) => c.path === path);
  if (change !== undefined) {
    switch (change.kind) {
      case "added":
        return { text: `Absent in ${a}, present in ${b}`, same: false };
      case "removed":
        return { text: `Present in ${a}, absent in ${b}`, same: false };
      case "type_changed":
        return { text: "Same text, different type", same: false };
      case "changed":
        return { text: "Value differs", same: false };
    }
  }
  // A file that was not parsed into settings has no setting changes to read; it is not the same for that.
  if (comparison.class !== "structured") return { text: "The lines of the two files, as compared", same: false };
  return { text: "Same value in both files", same: true };
}

const touches = (start: number, lines: number, from: number, to: number): boolean => {
  const end = lines === 0 ? start : start + lines - 1;
  return start <= to && end >= from;
};

/**
 * The server's hunks that touch the setting: the old side against the lines it has in the first file, the new side against
 * the lines it has in the second. A side with no line (the key is absent there) matches nothing on that side.
 */
export function hunksAround(hunks: readonly DiffHunk[], left: LineRange | null, right: LineRange | null): DiffHunk[] {
  const out: DiffHunk[] = [];
  for (const hunk of hunks) {
    const nearLeft = left !== null && touches(hunk.old_start, hunk.old_lines, left.start - CONTEXT_LINES, left.end + CONTEXT_LINES);
    const nearRight = right !== null && touches(hunk.new_start, hunk.new_lines, right.start - CONTEXT_LINES, right.end + CONTEXT_LINES);
    if (nearLeft || nearRight) out.push(hunk);
    if (out.length >= MAX_HUNKS) break;
  }
  return out;
}
