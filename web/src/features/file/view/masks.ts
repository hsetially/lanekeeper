import type { Mask } from "@/api/generated/model";
import { byteRangesToUtf16, utf8Length } from "@/lib/text/byte-ranges";

// Where the server's masks sit in the lines of a text (S14b, M1). The web never masks anything itself: the server replaces a
// flagged value with its marker `[REDACTED: N chars, rule]` and says where the marker is, as UTF-8 byte offsets into the whole
// `text`, with the 1-based line that holds it. Lines end at a line feed and a carriage return before it belongs to the line.
// Here those offsets become places inside each line's own string, so the view can mark the marker.
//
// A mask is untrusted input like everything else the server sends: one that is not exactly a marker, that names a line the text
// does not have, or that overlaps another is dropped (the text is still shown as it came), and at most MAX_MASKS are read.

/** The contract's limit on the list. */
const MAX_MASKS = 1000;

const MARKER = /^\[REDACTED: [0-9]+ chars, [a-z0-9_]+\]$/;

/** The form of a redaction marker, the same pattern the contract documents for a value. */
export function isMarker(text: string): boolean {
  return MARKER.test(text);
}

/** The lines of a text as the contract numbers them (a line feed ends one), without the carriage return before it. */
export function splitLfLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.map((line) => (line.endsWith("\r") ? line.slice(0, -1) : line));
}

export type LinePlace = {
  /** UTF-16 index of the marker's first character in the line's string. */
  readonly start: number;
  /** UTF-16 index just after its last character. */
  readonly end: number;
  readonly mask: Mask;
};

/** The marker places of each line, by 1-based line number, each line's in order of position. */
export function lineMasks(text: string, masks: readonly Mask[]): Map<number, LinePlace[]> {
  const byLine = new Map<number, Mask[]>();
  let last = 0;
  for (const mask of masks.slice(0, MAX_MASKS)) {
    if (!Number.isInteger(mask.line) || mask.line < 1 || !Number.isInteger(mask.start) || !Number.isInteger(mask.end)) continue;
    const list = byLine.get(mask.line);
    if (list) list.push(mask);
    else byLine.set(mask.line, [mask]);
    last = Math.max(last, mask.line);
  }
  const out = new Map<number, LinePlace[]>();
  if (byLine.size === 0) return out;

  // Raw lines (a carriage return stays: it counts as a byte of the line), up to the last line a mask names.
  let from = 0;
  let offset = 0;
  for (let line = 1; line <= last; line++) {
    const at = text.indexOf("\n", from);
    if (at < 0 && from >= text.length) break;
    const raw = at < 0 ? text.slice(from) : text.slice(from, at);
    const wanted = byLine.get(line);
    if (wanted) {
      const places = placesIn(raw, offset, wanted);
      if (places.length > 0) out.set(line, places);
    }
    offset += utf8Length(raw) + 1;
    if (at < 0) break;
    from = at + 1;
  }
  return out;
}

function placesIn(raw: string, lineStart: number, masks: readonly Mask[]): LinePlace[] {
  const bytes = utf8Length(raw);
  const places: LinePlace[] = [];
  for (const mask of masks) {
    const start = mask.start - lineStart;
    const end = mask.end - lineStart;
    if (start < 0 || end > bytes || start >= end) continue;
    const [range] = byteRangesToUtf16(raw, [{ start, end }]);
    if (!range || !isMarker(raw.slice(range.start, range.end))) continue;
    places.push({ start: range.start, end: range.end, mask });
  }
  places.sort((a, b) => a.start - b.start);
  // Two places that overlap cannot both be a marker: keep the first.
  return places.filter((place, i) => i === 0 || place.start >= (places[i - 1]?.end ?? 0));
}
