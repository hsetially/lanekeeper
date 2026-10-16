// Making hidden characters visible (S8, W21). Text from file names, paths, setting values, diff lines and hits is
// data, never trusted: a bidi override can make "invoice\u202Egnp.exe" display as "invoiceexe.png", and zero-width
// characters make two different names look identical. Every such character is replaced by a visible <U+XXXX> marker.
//
// Not covered: look-alike letters (a Cyrillic "а" for a Latin "a"). Those are ordinary letters, and detecting them
// needs a confusables table; compare names by their immutable ids where it matters (access approval does).

export type SafeSegment = { kind: "text"; text: string } | { kind: "mark"; text: string; codePoint: string; name: string };

// Control, format (bidi, zero width, soft hyphen), unpaired surrogates, line and paragraph separators.
const UNSAFE = /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}]/u;
// Letters and marks that draw nothing: Hangul fillers, braille blank, combining grapheme joiner. Matched by code
// point, because a combining mark inside a character class is read as part of its neighbour.
const BLANK_LOOKING = new Set([0x115f, 0x1160, 0x3164, 0xffa0, 0x2800, 0x034f]);

const NAMES = new Map<number, string>([
  [0x202a, "left-to-right embedding"],
  [0x202b, "right-to-left embedding"],
  [0x202c, "pop directional formatting"],
  [0x202d, "left-to-right override"],
  [0x202e, "right-to-left override"],
  [0x2066, "left-to-right isolate"],
  [0x2067, "right-to-left isolate"],
  [0x2068, "first strong isolate"],
  [0x2069, "pop directional isolate"],
  [0x200b, "zero width space"],
  [0x200c, "zero width non-joiner"],
  [0x200d, "zero width joiner"],
  [0x200e, "left-to-right mark"],
  [0x200f, "right-to-left mark"],
  [0xfeff, "zero width no-break space"],
  [0x061c, "arabic letter mark"],
  [0x00ad, "soft hyphen"],
  [0x2060, "word joiner"],
  [0x2028, "line separator"],
  [0x2029, "paragraph separator"],
  [0x3164, "hangul filler"],
  [0x115f, "hangul choseong filler"],
  [0x1160, "hangul jungseong filler"],
  [0xffa0, "halfwidth hangul filler"],
  [0x2800, "braille pattern blank"],
  [0x034f, "combining grapheme joiner"],
]);

function nameOf(ch: string, cp: number): string {
  const known = NAMES.get(cp);
  if (known) return known;
  if (/\p{Cc}/u.test(ch)) return "control character";
  if (/\p{Cs}/u.test(ch)) return "unpaired surrogate";
  if (/\p{Cf}/u.test(ch)) return "format character";
  return "invisible character";
}

function hex(cp: number): string {
  return `U+${cp.toString(16).toUpperCase().padStart(4, "0")}`;
}

/** Split text into plain runs and markers for the characters that must not be shown as themselves. */
export function splitUnsafe(text: string): SafeSegment[] {
  const out: SafeSegment[] = [];
  let run = "";
  for (const ch of text) {
    // A tab is ordinary whitespace in a diff line or a value; every other control character is not.
    const cp = ch.codePointAt(0) ?? 0;
    if (ch !== "\t" && (UNSAFE.test(ch) || BLANK_LOOKING.has(cp))) {
      if (run) out.push({ kind: "text", text: run });
      run = "";
      const codePoint = hex(cp);
      out.push({ kind: "mark", text: `<${codePoint}>`, codePoint, name: nameOf(ch, cp) });
    } else {
      run += ch;
    }
  }
  if (run) out.push({ kind: "text", text: run });
  return out;
}

/** The same text as a plain string, for attributes (title, aria-label) and logs where JSX cannot go. */
export function visibleText(text: string): string {
  return splitUnsafe(text)
    .map((s) => s.text)
    .join("");
}
