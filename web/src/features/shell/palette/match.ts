// The palette's filter. A score of 0 means no match; a bigger score is a better one. A prefix is best, then the start of a
// word (after `-`, `_`, `.`, `/`, a space or a camelCase step), then anywhere in the text; of two of the same kind the shorter
// text wins, so `sit` finds `sit` before `sit-with-a-very-long-name`. The query is plain text: it is never a pattern.

const WORD_BREAK = /[-_./\s:]/;

export function score(query: string, text: string): number {
  if (text === "") return 0;
  const q = query.trim().toLowerCase();
  if (q === "") return 1;
  const haystack = text.toLowerCase();
  const at = haystack.indexOf(q);
  if (at < 0) return 0;
  const shortness = 1 / (1 + text.length);
  if (at === 0) return 3 + shortness;
  const before = text.charAt(at - 1);
  const camel = before !== before.toUpperCase() && text.charAt(at) === text.charAt(at).toUpperCase() && text.charAt(at) !== text.charAt(at).toLowerCase();
  if (WORD_BREAK.test(before) || camel) return 2 + shortness;
  return 1 + shortness;
}

/** The text around the first match of the query, for drawing the match in bold. Without a match the text is all `before`. */
export function highlight(query: string, text: string): { before: string; match: string; after: string } {
  const q = query.trim();
  const at = q === "" ? -1 : text.toLowerCase().indexOf(q.toLowerCase());
  if (at < 0) return { before: text, match: "", after: "" };
  return { before: text.slice(0, at), match: text.slice(at, at + q.length), after: text.slice(at + q.length) };
}
