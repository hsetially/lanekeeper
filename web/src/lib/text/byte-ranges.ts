// Byte offsets from the API to indexes into a JavaScript string (F10).
//
// `InlineRange` and the `start` and `end` of a mask are 0-based UTF-8 byte offsets into the `text` of the object that carries
// them, half-open, on character boundaries. A JavaScript string is UTF-16, so slicing it with those numbers is wrong for
// every line that holds a non-ASCII character. This module converts them, and treats the ranges as untrusted input: a range
// that is empty, backwards, fractional, past the end, or that starts or ends inside a character is dropped, never thrown on
// (a bad span from the server must not blank the whole diff).
//
// Bounded (rule 5): a line over MAX_CHARS and more than MAX_RANGES ranges are not worth a lookup table, so they get no
// highlight. The text is still shown; only the word-level emphasis is lost.

export type ByteRange = { readonly start: number; readonly end: number };
export type Utf16Range = { start: number; end: number };

/** A line longer than this is shown without word highlights. */
export const MAX_CHARS = 100_000;
/** Ranges read from one line. */
export const MAX_RANGES = 10_000;

/** The width in bytes of one code point in UTF-8. A lone surrogate is written as U+FFFD, three bytes, as the encoder does. */
function bytesOf(codePoint: number): number {
  if (codePoint < 0x80) return 1;
  if (codePoint < 0x800) return 2;
  if (codePoint >= 0xd800 && codePoint <= 0xdfff) return 3;
  return codePoint < 0x10000 ? 3 : 4;
}

/** The length of `text` once encoded as UTF-8. */
export function utf8Length(text: string): number {
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    if (unit < 0x80) bytes += 1;
    else if (unit < 0x800) bytes += 2;
    else if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        bytes += 4;
        i += 1;
      } else bytes += 3;
    } else bytes += 3;
  }
  return bytes;
}

/**
 * Convert half-open UTF-8 byte ranges of `text` to half-open UTF-16 ranges of the same characters. The result is sorted,
 * and ranges that overlap or touch are merged, so the caller can cut the text at the boundaries in one pass.
 */
export function byteRangesToUtf16(text: string, ranges: readonly ByteRange[]): Utf16Range[] {
  if (ranges.length === 0 || text.length === 0 || text.length > MAX_CHARS) return [];
  const total = utf8Length(text);
  const ascii = total === text.length;

  const candidates: Array<{ start: number; end: number }> = [];
  for (const range of ranges.length > MAX_RANGES ? ranges.slice(0, MAX_RANGES) : ranges) {
    const { start, end } = range;
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || start >= end || end > total) continue;
    candidates.push({ start, end });
  }
  if (candidates.length === 0) return [];
  candidates.sort((a, b) => a.start - b.start || a.end - b.end);

  const unitOf = ascii ? undefined : boundaryTable(text, candidates);
  const out: Utf16Range[] = [];
  for (const { start, end } of candidates) {
    const from = unitOf ? unitOf.get(start) : start;
    const to = unitOf ? unitOf.get(end) : end;
    // Not on a character boundary: the span cuts a character in two, so it is not a span of this text.
    if (from === undefined || to === undefined || from >= to) continue;
    const last = out[out.length - 1];
    if (last && from <= last.end) last.end = Math.max(last.end, to);
    else out.push({ start: from, end: to });
  }
  return out;
}

/** Byte offset to UTF-16 index, for the offsets in `candidates` that sit on a character boundary and no others. */
function boundaryTable(text: string, candidates: ReadonlyArray<{ start: number; end: number }>): Map<number, number> {
  const wanted = new Set<number>();
  for (const { start, end } of candidates) {
    wanted.add(start);
    wanted.add(end);
  }
  const table = new Map<number, number>();
  let byte = 0;
  let unit = 0;
  while (unit <= text.length) {
    if (wanted.has(byte)) table.set(byte, unit);
    if (unit === text.length) break;
    const codePoint = text.codePointAt(unit) ?? 0;
    byte += bytesOf(codePoint);
    unit += codePoint > 0xffff ? 2 : 1;
  }
  return table;
}
