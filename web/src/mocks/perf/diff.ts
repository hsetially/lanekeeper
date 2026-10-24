// A comparison of a long file, shaped like what the engine sends (crates/engine/src/diff/text.rs), for the P10 gate of the
// 6,000-line diff. Deterministic: the same shape gives the same hunks, so a run measures the code and not the data.
//
// The engine's rules, kept here: changes at most six old lines apart share a hunk; a hunk has at most three lines of context
// before its first change and after its last (fewer when another change is closer); lines run as context, removed, added,
// context; numbering is the unified diff's, 1-based and, for a side with no lines, the line before; a changed span is a
// range of UTF-8 bytes on character boundaries. Loaded by Node (the harness) as well as by Vite: relative imports with the
// `.ts` extension and `import type` for types.

import type { Comparison, DiffHunk, DiffLine } from "../../api/generated/model/index.ts";

export type DiffShape = {
  /** Lines of the old file. */
  lines: number;
  /** A change about this many lines apart (the realistic shape). Default 10. */
  changeEvery?: number;
  /** Every line changed: one hunk of `lines` removed and `lines` added, the worst case. */
  full?: boolean;
};

const CONTEXT = 3;
const MERGE_GAP = 2 * CONTEXT;
const enc = new TextEncoder();

/** Every seventh line has accents, an arrow and an emoji, so the byte offsets differ from the UTF-16 ones. */
function oldText(i: number): string {
  return i % 7 === 3 ? `app.nôde-${i}.label: "naïve → déjà vu ✓ 😀 ${i}"` : `service.group${i % 40}.option-${i}: value-${i % 97}`;
}

function newText(i: number): string {
  return i % 7 === 3 ? `app.nôde-${i}.label: "naïve → déjà vu ✗ 🙂 ${i}"` : `service.group${i % 40}.option-${i}: setting-${i % 89}`;
}

const charsOf = (text: string): string[] => Array.from(text);

/** The changed middle of two lines, as byte ranges: what is left after the common start and the common end. */
function pairRanges(a: string, b: string): [Array<{ start: number; end: number }>, Array<{ start: number; end: number }>] {
  const x = charsOf(a);
  const y = charsOf(b);
  let prefix = 0;
  while (prefix < x.length && prefix < y.length && x[prefix] === y[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < x.length - prefix && suffix < y.length - prefix && x[x.length - 1 - suffix] === y[y.length - 1 - suffix]) suffix += 1;
  const bytes = (chars: string[]): number => enc.encode(chars.join("")).length;
  const head = bytes(x.slice(0, prefix));
  const range = (chars: string[]): Array<{ start: number; end: number }> => {
    const middle = chars.slice(prefix, chars.length - suffix);
    return middle.length === 0 ? [] : [{ start: head, end: head + bytes(middle) }];
  };
  return [range(x), range(y)];
}

type Change = { oldStart: number; oldEnd: number; newStart: number; added: string[] };

function changesOf({ lines, changeEvery = 10, full = false }: DiffShape): Change[] {
  const changes: Change[] = [];
  if (full) {
    changes.push({ oldStart: 0, oldEnd: lines, newStart: 0, added: Array.from({ length: lines }, (_, i) => newText(i)) });
    return changes;
  }
  let delta = 0;
  let previousEnd = 0;
  for (let k = 0; ; k++) {
    const at = 5 + k * changeEvery + ((k * 7) % 3);
    const removed = 1 + (k % 3);
    const added = 1 + ((k + 1) % 3);
    if (at + removed > lines || at < previousEnd) break;
    changes.push({ oldStart: at, oldEnd: at + removed, newStart: at + delta, added: Array.from({ length: added }, (_, i) => newText(at + i)) });
    delta += added - removed;
    previousEnd = at + removed;
  }
  return changes;
}

const line = (kind: DiffLine["kind"], text: string, inline_changes: DiffLine["inline_changes"] = []): DiffLine => ({ kind, text, inline_changes });
/** The unified diff's start: 1-based, and for a side with no lines the line before it. */
const startOf = (from: number, count: number): number => (count === 0 ? from : from + 1);

export function makeComparison(shape: DiffShape): Comparison {
  const changes = changesOf(shape);
  const hunks: DiffHunk[] = [];
  for (let i = 0; i < changes.length; ) {
    let j = i;
    while (j + 1 < changes.length && (changes[j + 1] as Change).oldStart - (changes[j] as Change).oldEnd <= MERGE_GAP) j += 1;
    const group = changes.slice(i, j + 1);
    const first = group[0] as Change;
    const last = group[group.length - 1] as Change;
    const before = changes[i - 1];
    const after = changes[j + 1];
    const lead = Math.min(CONTEXT, first.oldStart - (before?.oldEnd ?? 0));
    const trail = Math.min(CONTEXT, (after?.oldStart ?? shape.lines) - last.oldEnd);
    const oldFrom = first.oldStart - lead;
    const newFrom = first.newStart - lead;

    const lines: DiffLine[] = [];
    const context = (from: number, count: number): void => {
      for (let n = 0; n < count; n++) lines.push(line("context", oldText(from + n)));
    };
    context(oldFrom, lead);
    group.forEach((change, n) => {
      const removed = Array.from({ length: change.oldEnd - change.oldStart }, (_, k) => oldText(change.oldStart + k));
      const ranges = removed.map((text, k) => (change.added[k] === undefined ? ([[], []] as const) : pairRanges(text, change.added[k])));
      removed.forEach((text, k) => lines.push(line("removed", text, [...(ranges[k] as readonly [Array<{ start: number; end: number }>, unknown])[0]])));
      change.added.forEach((text, k) => lines.push(line("added", text, removed[k] === undefined ? [] : pairRanges(removed[k], text)[1])));
      const next = group[n + 1];
      if (next) context(change.oldEnd, next.oldStart - change.oldEnd);
    });
    context(last.oldEnd, trail);

    const oldLines = last.oldEnd + trail - oldFrom;
    const newLines = last.newStart + last.added.length + trail - newFrom;
    hunks.push({ old_start: startOf(oldFrom, oldLines), old_lines: oldLines, new_start: startOf(newFrom, newLines), new_lines: newLines, lines });
    i = j + 1;
  }
  return {
    left: "nfs:swimlane-01",
    right: "git:base@main",
    file: "services/payments/application.yml",
    class: "text",
    identical: false,
    eol_only: false,
    hunks,
    setting_changes: [],
    binary: null,
  };
}
