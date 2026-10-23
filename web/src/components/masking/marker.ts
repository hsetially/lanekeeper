// The redaction marker, as the contract states it (S14b, M1): a flagged value is replaced by `[REDACTED: N chars, rule]`, which
// matches `^\[REDACTED: [0-9]+ chars, [a-z0-9_]+\]$` as a whole value and `\[REDACTED: [0-9]+ chars, [a-z0-9_]+\]` inside a text.
//
// The web never makes a marker and never masks anything by itself (rule 7: only the engine scans). This module only RECOGNISES
// what the server sent, on the surfaces that carry no `masks` list (a diff line, a search hit, a setting value, a doc): there "the
// marker pattern is the recognition". Anything that is not exactly the pattern is ordinary text.
//
// Bounded (rule 5): at most MAX_MARKERS markers are read from one text and a text past MAX_SCAN_CHARS is not scanned (it is shown as
// text, which is the marker's own words and not a value). The pattern has no nested quantifier, so a scan is linear.

export type MarkerInfo = {
  /** The length of the value that was hidden, or null when the digits are not a length a number can hold. */
  readonly chars: number | null;
  /** The scan rule that flagged it (`generic_secret`, `aws_key`, `unscanned`). */
  readonly rule: string;
};

export type MarkedPart = { readonly kind: "text"; readonly text: string } | { readonly kind: "marker"; readonly text: string; readonly info: MarkerInfo };

export const MAX_MARKERS = 1_000;
export const MAX_SCAN_CHARS = 200_000;
/** A length with more digits than this is not a length. */
const MAX_LENGTH_DIGITS = 9;

const PREFIX = "[REDACTED: ";
const WHOLE = /^\[REDACTED: ([0-9]+) chars, ([a-z0-9_]+)\]$/;
const INLINE = /\[REDACTED: ([0-9]+) chars, ([a-z0-9_]+)\]/g;

function infoOf(digits: string, rule: string): MarkerInfo {
  return { chars: digits.length > MAX_LENGTH_DIGITS ? null : Number(digits), rule };
}

/** The length and rule of `text` when it is exactly one marker, else null. */
export function parseMarker(text: string): MarkerInfo | null {
  if (!text.startsWith(PREFIX)) return null;
  const match = WHOLE.exec(text);
  const digits = match?.[1];
  const rule = match?.[2];
  return digits === undefined || rule === undefined ? null : infoOf(digits, rule);
}

/** A text in the order it is written, as plain runs and markers. The parts always add up to the text. */
export function splitMarkers(text: string): MarkedPart[] {
  if (text === "") return [];
  if (text.length > MAX_SCAN_CHARS || !text.includes(PREFIX)) return [{ kind: "text", text }];
  const parts: MarkedPart[] = [];
  let at = 0;
  let found = 0;
  for (const match of text.matchAll(INLINE)) {
    if (found === MAX_MARKERS) break;
    const digits = match[1];
    const rule = match[2];
    if (digits === undefined || rule === undefined) continue;
    found += 1;
    if (match.index > at) parts.push({ kind: "text", text: text.slice(at, match.index) });
    parts.push({ kind: "marker", text: match[0], info: infoOf(digits, rule) });
    at = match.index + match[0].length;
  }
  if (at < text.length) parts.push({ kind: "text", text: text.slice(at) });
  return parts;
}
