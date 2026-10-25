import type { DiffHunk } from "@/api/generated/model";
import { splitLines } from "@/components/app/virtual/CodeView";

// Unfolding the unchanged lines of a diff. The server's hunks carry the changes and a little context; the lines between two
// hunks are the same on both sides, so when the screen has the text of one side (the NFS file) it can show them from there.
//
// The text is the NFS head and the hunks are a separate read: a file that changed between the two would show the wrong
// lines. So the hunks are checked against the text line for line before anything is unfolded, and a mismatch means no
// unfolding at all (the folds stay counts). Nothing here compares two texts: the lines shown are the text's own.
//
// Lines are counted as the engine counts them (`\r\n`, `\n` and a lone `\r` each end one).

export type FoldContext = {
  getLines: (side: "old" | "new", from: number, count: number) => readonly string[] | null;
  /** The length of the old side, so that a fold after the last hunk can be shown. */
  oldTotal: number;
};

type Side = "old" | "new";

/** The lines of a side before a hunk: the unified diff counts a side with no lines in the hunk by the line before it. */
const linesBefore = (start: number, count: number): number => (count === 0 ? start : start - 1);

export function foldContext(hunks: readonly DiffHunk[], nfsSide: Side, text: string): FoldContext | null {
  const lines = splitLines(text);

  // The hunks must say what the text says, on the NFS side, line for line.
  for (const hunk of hunks) {
    const start = nfsSide === "new" ? hunk.new_start : hunk.old_start;
    const count = nfsSide === "new" ? hunk.new_lines : hunk.old_lines;
    let no = linesBefore(start, count) + 1;
    let seen = 0;
    for (const line of hunk.lines) {
      // A line that is only on the other side is not in this text.
      if (line.kind === (nfsSide === "new" ? "removed" : "added")) continue;
      if (lines[no - 1] !== line.text) return null;
      no += 1;
      seen += 1;
    }
    if (seen !== count) return null;
  }

  // A line number on one side, for a line outside every hunk, is the same line's number on the other side shifted by what the
  // hunks before it added or removed.
  const across = (from: Side, n: number): number => {
    let delta = 0;
    for (const hunk of hunks) {
      const start = from === "old" ? hunk.old_start : hunk.new_start;
      const count = from === "old" ? hunk.old_lines : hunk.new_lines;
      if (linesBefore(start, count) + count >= n) break;
      delta += from === "old" ? hunk.new_lines - hunk.old_lines : hunk.old_lines - hunk.new_lines;
    }
    return n + delta;
  };

  const getLines = (side: Side, from: number, count: number): readonly string[] | null => {
    if (!Number.isInteger(from) || !Number.isInteger(count) || from < 1 || count < 1) return null;
    const at = side === nfsSide ? from : across(side, from);
    if (at < 1) return null;
    const slice = lines.slice(at - 1, at - 1 + count);
    return slice.length === count ? slice : null;
  };

  const growth = hunks.reduce((sum, h) => sum + (h.new_lines - h.old_lines), 0);
  return { getLines, oldTotal: nfsSide === "old" ? lines.length : lines.length - growth };
}
