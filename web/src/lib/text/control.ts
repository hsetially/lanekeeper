// What does not belong in a path, an id or a name (S8, S11). The question is asked in three strengths, depending on what the text is
// for, and each is written here once, so a screen does not carry a loop of its own that can drift from the others:
//
//  - `hasControlChar`: a C0 control or DEL. For an id, a name or a search term that the server stores as it is;
//  - `hasControlOrBackslash`: also a C1 control and the backslash. For a path or a URL that is handed to something that would rewrite
//    it (a browser reads a backslash as a slash, a URL parser drops a tab and a line break);
//  - `hasHiddenOrBackslash`: also format characters (bidi overrides, zero-width), line and paragraph separators, i.e. what makes two
//    different paths look the same. For a path that is shown to a person and compared by eye.
//
// These say whether the text has such a character; they never change the text. What a screen DRAWS for a hidden character is
// SafeText's business (components/app/safe-text.ts), and what a clipboard may take is copy-text.ts's.

/** A C0 control character (U+0000 to U+001F, a tab and a line break among them) or DEL (U+007F). */
export function hasControlChar(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    if (unit < 0x20 || unit === 0x7f) return true;
  }
  return false;
}

/** A control character of C0, DEL or C1 (U+0080 to U+009F), or a backslash. */
export function hasControlOrBackslash(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    if (unit < 0x20 || (unit >= 0x7f && unit <= 0x9f) || unit === 0x5c) return true;
  }
  return false;
}

const HIDDEN_OR_BACKSLASH = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\\]/u;

/** A control, format, line-separator or paragraph-separator character (Unicode general categories Cc, Cf, Zl, Zp), or a backslash. */
export function hasHiddenOrBackslash(text: string): boolean {
  return HIDDEN_OR_BACKSLASH.test(text);
}
