import type { DiffHunk, DiffLineKind, InlineRange } from "@/api/generated/model";

// The layout of a diff. The server computes the hunks, including the context and the changed spans inside a line (rule 7:
// facts come from the engine); the browser lays them out and never compares two texts. A 6,000-line file is never diffed
// here (P10): this turns hunks into rows, once, and the virtualiser draws the rows in view.
//
// Numbering is the unified diff's, as the engine writes it (crates/engine/src/diff/text.rs, `start`): a side's start is
// 1-based, and for a side with no lines it is the line BEFORE the hunk. So the number of old lines before a hunk is
// `old_start - 1`, or `old_start` when the hunk has no old lines. The same holds for the new side.

/** One height for each kind of row (the handoff's table says "also 18" for all three; the pages draw 18, 20 and 24). */
export const ROW_HEIGHT = { line: 18, hunk: 20, fold: 24 } as const;

export type DiffCell = {
  /** The line number on its own side (1-based). */
  no: number;
  kind: DiffLineKind;
  text: string;
  /** The changed spans of the line, in UTF-8 byte offsets as the server sends them. */
  ranges: readonly InlineRange[];
};

export type LineRow = { kind: "line"; left: DiffCell | null; right: DiffCell | null; change: boolean };
export type HunkRow = { kind: "hunk"; header: string };
export type FoldRow = { kind: "fold"; id: string; hidden: number; oldFrom: number; newFrom: number };
export type DiffRow = LineRow | HunkRow | FoldRow;

export type DiffLayout = {
  rows: DiffRow[];
  /** The index of the first row of each run of changed rows: where `n` and `p` land. */
  changeStarts: number[];
};

export type BuildOptions = {
  mode: "split" | "unified";
  /** The length of the old file. Only then can a fold after the last hunk be shown. */
  oldTotal?: number;
  /** Folds the person opened, by id. */
  expanded?: ReadonlySet<string>;
  /** The text of unchanged lines (`from` is 1-based), when the screen has the file. Null when it does not. */
  getLines?: (side: "old" | "new", from: number, count: number) => readonly string[] | null;
};

const NO_RANGES: readonly InlineRange[] = [];

const linesBefore = (start: number, lines: number): number => (lines === 0 ? start : start - 1);

export function buildDiffRows(hunks: readonly DiffHunk[], options: BuildOptions): DiffLayout {
  const { mode, oldTotal, expanded, getLines } = options;
  const rows: DiffRow[] = [];
  let oldEnd = 0;
  let newEnd = 0;

  const foldOrLines = (hidden: number): void => {
    if (hidden <= 0) return;
    const oldFrom = oldEnd + 1;
    const newFrom = newEnd + 1;
    const id = `fold-${oldFrom}`;
    if (expanded?.has(id) === true && getLines) {
      const oldText = getLines("old", oldFrom, hidden);
      const newText = getLines("new", newFrom, hidden);
      if (oldText?.length === hidden && newText?.length === hidden) {
        for (let i = 0; i < hidden; i++) {
          rows.push({
            kind: "line",
            change: false,
            left: { no: oldFrom + i, kind: "context", text: oldText[i] ?? "", ranges: NO_RANGES },
            right: { no: newFrom + i, kind: "context", text: newText[i] ?? "", ranges: NO_RANGES },
          });
        }
        return;
      }
    }
    rows.push({ kind: "fold", id, hidden, oldFrom, newFrom });
  };

  for (const hunk of hunks) {
    const oldBefore = linesBefore(hunk.old_start, hunk.old_lines);
    const newBefore = linesBefore(hunk.new_start, hunk.new_lines);
    foldOrLines(oldBefore - oldEnd);
    rows.push({ kind: "hunk", header: `@@ -${hunk.old_start},${hunk.old_lines} +${hunk.new_start},${hunk.new_lines} @@` });

    let oldNo = oldBefore + 1;
    let newNo = newBefore + 1;
    let removed: DiffCell[] = [];
    let added: DiffCell[] = [];
    const flush = (): void => {
      if (mode === "split") {
        const n = Math.max(removed.length, added.length);
        for (let i = 0; i < n; i++) rows.push({ kind: "line", change: true, left: removed[i] ?? null, right: added[i] ?? null });
      }
      removed = [];
      added = [];
    };

    for (const line of hunk.lines) {
      const ranges = line.inline_changes.length > 0 ? line.inline_changes : NO_RANGES;
      if (line.kind === "context") {
        flush();
        rows.push({
          kind: "line",
          change: false,
          left: { no: oldNo++, kind: "context", text: line.text, ranges },
          right: { no: newNo++, kind: "context", text: line.text, ranges },
        });
      } else if (line.kind === "removed") {
        // A removed line after an added run starts a new change.
        if (added.length > 0) flush();
        const cell: DiffCell = { no: oldNo++, kind: "removed", text: line.text, ranges };
        if (mode === "split") removed.push(cell);
        else rows.push({ kind: "line", change: true, left: cell, right: null });
      } else {
        const cell: DiffCell = { no: newNo++, kind: "added", text: line.text, ranges };
        if (mode === "split") added.push(cell);
        else rows.push({ kind: "line", change: true, left: null, right: cell });
      }
    }
    flush();
    oldEnd = oldBefore + hunk.old_lines;
    newEnd = newBefore + hunk.new_lines;
  }
  if (oldTotal !== undefined && hunks.length > 0) foldOrLines(oldTotal - oldEnd);

  const changeStarts: number[] = [];
  let inRun = false;
  rows.forEach((row, index) => {
    const changed = row.kind === "line" && row.change;
    if (changed && !inRun) changeStarts.push(index);
    inRun = changed;
  });
  return { rows, changeStarts };
}

/** Added and removed lines over all hunks, for the header ("+12 -4"). */
export function diffStats(hunks: readonly DiffHunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "added") added += 1;
      else if (line.kind === "removed") removed += 1;
    }
  }
  return { added, removed };
}
