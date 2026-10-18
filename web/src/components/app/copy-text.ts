import { splitUnsafe } from "./safe-text";

// What may go on a clipboard. The text of a path, a name or a value is untrusted (S8), and what the person pastes is not
// what the page showed: a line break pasted into a terminal runs what follows it, and a bidi override or a zero-width
// character makes two different strings look the same. So:
//
//  - nothing is copied for an empty value;
//  - a line break or a control character is refused for a single-line value (an id, a path, a hash, a name), and refused
//    for text too unless it is a line break (`multiline`, for a draft or the support details);
//  - a hidden character (SafeText shows each as <U+XXXX>) is copied as it is, because the value really holds it and a
//    person may need the exact string, and the confirmation says so.

export const COPIED = "Copied";
export const COPIED_HIDDEN = "Copied. It contains hidden characters.";
export const COULD_NOT_COPY = "Couldn't copy. Select the text and copy it manually.";
export const NOT_COPIED_CONTROL = "Not copied: it contains a line break or control character.";
export const NOTHING_TO_COPY = "Nothing to copy.";

const IS_CONTROL = /\p{Cc}/u;

/** A C0 or C1 control character, or a line or paragraph separator. A tab is ordinary whitespace, and a line break is allowed in text. */
function hasControl(text: string, allowLineBreak: boolean): boolean {
  for (const ch of text) {
    if (ch === "\t") continue;
    if (allowLineBreak && (ch === "\n" || ch === "\r")) continue;
    if (IS_CONTROL.test(ch) || ch === "\u2028" || ch === "\u2029") return true;
  }
  return false;
}

/**
 * A masked setting value is its marker and nothing else (the contract: `^\[REDACTED: N chars, rule\]$`, S14b). The marker is not the
 * value, so a screen offers no copy for it: there is nothing on the clipboard that the person could use.
 */
const MARKER = /^\[REDACTED: [0-9]+ chars, [a-z0-9_]+\]$/;
export const isMaskedValue = (text: string): boolean => MARKER.test(text);

export type CopyCheck = { ok: true; hidden: boolean } | { ok: false; message: string };

export function checkCopyable(text: string, options: { multiline?: boolean } = {}): CopyCheck {
  if (text === "") return { ok: false, message: NOTHING_TO_COPY };
  if (hasControl(text, options.multiline === true)) return { ok: false, message: NOT_COPIED_CONTROL };
  const hidden = splitUnsafe(text).some((segment) => segment.kind === "mark" && segment.codePoint !== "U+000A" && segment.codePoint !== "U+000D");
  return { ok: true, hidden };
}
