import type { Mask } from "@/api/generated/model";
import { byteRangesToUtf16, utf8Length } from "@/lib/text/byte-ranges";
import { parseMarker, splitMarkers, type MarkerInfo } from "./marker";

// What the server's `masks` mean for a text (S14b, M1, plan "Masking and secrets in the browser").
//
// The server replaces a flagged value with its marker `[REDACTED: N chars, rule]` and lists where each marker sits: `start` and
// `end` are 0-based UTF-8 byte offsets into the whole `text` of the same object (`FileContent`, `ServedFile`, `DocText`), half open,
// and `line` is the 1-based line that holds it. The web never masks anything itself and never sees the value: the text it is given
// already holds the marker, and a mask holds no value and no part of one. This module only turns the offsets into places inside each
// line (T2's byte-ranges converts bytes to UTF-16), so that a marker can be drawn as a marker.
//
// A mask is untrusted input like everything else the server sends, and the plan FAILS CLOSED. It never guesses what the server meant:
//  - a mask whose span it can place on a line but cannot check (the span is not exactly a marker, it cuts a character, it overlaps
//    another, the line is too long to convert) hides that WHOLE LINE: nothing of the line is drawn as text;
//  - a mask it cannot place at all (it is not a plain count, it names a line the text does not have, or a range that leaves the line
//    it names) REFUSES the whole text: with a number it cannot trust, it cannot say which part of the text was meant to be hidden;
//  - more masks than the contract's limit refuse the text too.
// A conforming server never causes any of this, so the price of failing closed is paid only when something is already wrong.
//
// The contract says a marker past its list of 1,000 "is recognised by its form and has no reveal": text between and outside placed
// markers is scanned for the pattern too (marker.ts), and a marker found that way is drawn as a marker with no reveal.

export const MAX_MASKS = 1_000;
const MAX_SETTING_CHARS = 1_024;

export type RefusalReason = "too_many" | "malformed" | "missing_line" | "outside_line";
export type HiddenReason = "not_a_marker" | "unverifiable" | "overlap";

export type PlannedMark = {
  /** The mask's place in the text, unique within it: the line and the byte offset. Not derived from anything hidden. */
  readonly key: string;
  /** 1-based, in the numbering of the text (`DocText.lines` shifts only what is displayed, not this). */
  readonly line: number;
  /** UTF-16 index of the marker's first character in the line. */
  readonly start: number;
  /** UTF-16 index just after its last character. */
  readonly end: number;
  readonly info: MarkerInfo;
  /** What `revealValue` takes with the file's path, or null when no reveal can name the value (plain text, docs, served text). */
  readonly setting: string | null;
  readonly mask: Mask;
};

export type LinePlan =
  | { readonly kind: "marks"; readonly marks: readonly PlannedMark[] }
  | { readonly kind: "hidden"; readonly reason: HiddenReason };

export type MaskPlan =
  | { readonly status: "none" }
  | { readonly status: "ok"; readonly lines: ReadonlyMap<number, LinePlan> }
  | { readonly status: "refused"; readonly reason: RefusalReason };

const NONE = { status: "none" } as const;
const refused = (reason: RefusalReason): MaskPlan => ({ status: "refused", reason });
const HIDE = (reason: HiddenReason): LinePlan => ({ kind: "hidden", reason });
const OUTSIDE = Symbol("outside");

/** The lines of a text as the contract numbers them: a line feed ends one, a carriage return before it stays with its line. */
export function lfLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

const isCount = (value: unknown, min: number): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= min;

function settingOf(mask: Mask): string | null {
  const setting: unknown = mask.setting;
  return typeof setting === "string" && setting.length >= 1 && setting.length <= MAX_SETTING_CHARS ? setting : null;
}

/** Where the markers of one line are, or HIDE when the line cannot be trusted, or OUTSIDE when a range leaves the line. */
function placeLine(raw: string, line: number, lineStart: number, size: number, masks: readonly Mask[]): LinePlan | typeof OUTSIDE {
  const inLine = masks.map((mask) => ({ mask, start: mask.start - lineStart, end: mask.end - lineStart }));
  if (inLine.some((m) => m.start < 0 || m.end > size)) return OUTSIDE;

  inLine.sort((a, b) => a.start - b.start || a.end - b.end);
  // One conversion for the line. It merges ranges that overlap or touch and drops one that cuts a character or sits on a line it
  // will not convert, so one range out for each range in is the proof that none of those happened.
  const converted = byteRangesToUtf16(
    raw,
    inLine.map((m) => ({ start: m.start, end: m.end })),
  );
  if (converted.length !== inLine.length) {
    const touching = inLine.some((m, i) => i > 0 && m.start <= (inLine[i - 1]?.end ?? 0));
    return HIDE(touching ? "overlap" : "unverifiable");
  }
  const marks: PlannedMark[] = [];
  for (const [i, m] of inLine.entries()) {
    const range = converted[i];
    if (range === undefined) return HIDE("unverifiable");
    const info = parseMarker(raw.slice(range.start, range.end));
    if (info === null) return HIDE("not_a_marker");
    marks.push({ key: `${line}:${m.mask.start}`, line, start: range.start, end: range.end, info, setting: settingOf(m.mask), mask: m.mask });
  }
  return { kind: "marks", marks };
}

/** The plan for a text and the masks that came with it. `none` when the server supplied none (absent means none, M1). */
export function planMasks(text: string, masks: readonly Mask[] | undefined): MaskPlan {
  if (masks === undefined || masks.length === 0) return NONE;
  if (masks.length > MAX_MASKS) return refused("too_many");

  const byLine = new Map<number, Mask[]>();
  let last = 0;
  for (const mask of masks) {
    if (!isCount(mask.line, 1) || !isCount(mask.start, 0) || !isCount(mask.end, 1) || mask.end <= mask.start) return refused("malformed");
    const list = byLine.get(mask.line);
    if (list) list.push(mask);
    else byLine.set(mask.line, [mask]);
    if (mask.line > last) last = mask.line;
  }

  const rows = lfLines(text);
  if (last > rows.length) return refused("missing_line");

  const lines = new Map<number, LinePlan>();
  let offset = 0;
  for (let n = 1; n <= last; n++) {
    const raw = rows[n - 1] ?? "";
    const size = utf8Length(raw);
    const wanted = byLine.get(n);
    if (wanted) {
      const placed = placeLine(raw, n, offset, size, wanted);
      if (placed === OUTSIDE) return refused("outside_line");
      lines.set(n, placed);
    }
    offset += size + 1;
  }
  return { status: "ok", lines };
}

export type Segment =
  | { readonly kind: "text"; readonly text: string }
  /** A marker. `planned` is the mask that placed it, or null when it was recognised by its form (no reveal). */
  | { readonly kind: "mark"; readonly text: string; readonly info: MarkerInfo; readonly planned: PlannedMark | null }
  /** A line nothing of which may be drawn. */
  | { readonly kind: "hidden" };

function pushScanned(out: Segment[], text: string): void {
  for (const part of splitMarkers(text)) {
    if (part.kind === "text") out.push({ kind: "text", text: part.text });
    else out.push({ kind: "mark", text: part.text, info: part.info, planned: null });
  }
}

/** What to draw for one line, in order: its text, its markers and, for a line that cannot be trusted, nothing of it. */
export function segmentsOf(display: string, plan: LinePlan | undefined): Segment[] {
  if (plan?.kind === "hidden") return [{ kind: "hidden" }];
  const out: Segment[] = [];
  let at = 0;
  for (const mark of plan?.kind === "marks" ? plan.marks : []) {
    const end = Math.min(mark.end, display.length);
    if (mark.start > at) pushScanned(out, display.slice(at, mark.start));
    out.push({ kind: "mark", text: display.slice(mark.start, end), info: mark.info, planned: mark });
    at = end;
  }
  if (at < display.length) pushScanned(out, display.slice(at));
  return out;
}
