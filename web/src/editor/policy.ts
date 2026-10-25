// The rules of the one Trusted Types policy Monaco gets (`lanekeeper-monaco`).
//
// Monaco 0.57 builds line and overlay HTML as strings with `style="top:..;height:.."` and assigns them with innerHTML and
// insertAdjacentHTML. The hub's CSP has no `style-src-attr 'unsafe-inline'` (and a nonce does not cover an attribute), so the
// browser would drop every such attribute and the boxes would lose their position and size. The rewrite below is run by the
// policy's createHTML on every string Monaco makes into HTML: it renames the style attribute of every tag to a data
// attribute that only this page knows the name of, and src/editor/style-apply.ts copies the value into el.style, which is
// CSSOM and allowed. (Plan W3 option (e), shape "3d" of the G0 measurement; it is measured on every Monaco upgrade by
// e2e/editor/csp.spec.ts.)
//
// The rewrite reads the string the way the HTML tokenizer does and renames the style attribute of a START TAG and nothing else:
// not text (a regular expression over the whole string also matched model text that happened to contain `style="..."` and
// corrupted what the editor showed, 4 of 5 test lines in G0), not a comment, a doctype or an end tag, and not another
// attribute's VALUE. The last one matters beyond looks: a pattern over a whole tag finds ` style='x'` inside
// `title="a style='x' b"` and renames it in place, which closes the title early and turns the rest of its value into attributes
// of the tag (D133). So wherever text of a file reaches an attribute value of Monaco's markup (a title, a label), a pattern of that kind
// lets the text write attributes into it. (The CSP still refuses an inline handler. This keeps the markup the markup Monaco wrote.)
//
// What it follows, from the HTML tokenizer: a tag name ends at a space, `/` or `>`; between attributes a space or a `/` is a
// separator; an attribute name ends at a space, `/`, `>` or `=` (and may begin with `=`); a value is "..." or '...' (which may
// hold `>` and the other quote) or bare (up to a space or `>`); names are matched without regard to case. What it does not follow
// is the tokenizer's other states: the text of a raw-text element (textarea, script, style, title) is read as markup, because
// Monaco makes none of them through a string, and a tag that never closes runs to the end of the string.

const isSpace = (ch: string): boolean => ch === " " || ch === "\t" || ch === "\n" || ch === "\f" || ch === "\r";
const isAsciiAlpha = (ch: string): boolean => (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z");
const isStyle = (name: string): boolean => name.length === 5 && name.toLowerCase() === "style";

/** `-->`, or `--!>` (the tokenizer closes a comment at either). */
const COMMENT_END = /--!?>/g;

/** Where the comment that starts at `lt` (`<!--`) ends, as an index just after it. `<!-->` and `<!--->` are comments of no text. */
function endOfComment(html: string, lt: number): number {
  const body = lt + 4;
  if (html.startsWith(">", body)) return body + 1;
  if (html.startsWith("->", body)) return body + 2;
  COMMENT_END.lastIndex = body;
  const close = COMMENT_END.exec(html);
  return close === null ? html.length : close.index + close[0].length;
}

type Edit = { start: number; end: number; text: string };

/**
 * Read the tag whose name starts at `nameStart` to its `>`, honouring quoted values. Returns where it ends (just after the `>`, or the end
 * of the string when it never closes) and, when `rewrite` is set, the style attributes to rename.
 */
function scanTag(html: string, nameStart: number, rewrite: boolean, attrName: string): { end: number; edits: Edit[] } {
  const n = html.length;
  const edits: Edit[] = [];
  let j = nameStart;
  while (j < n && !isSpace(html.charAt(j)) && html.charAt(j) !== "/" && html.charAt(j) !== ">") j++;
  for (;;) {
    while (j < n && (isSpace(html.charAt(j)) || html.charAt(j) === "/")) j++;
    if (j >= n) return { end: n, edits };
    if (html.charAt(j) === ">") return { end: j + 1, edits };
    // An attribute name (an `=` at its start belongs to it).
    const nameFrom = j;
    j++;
    while (j < n && !isSpace(html.charAt(j)) && html.charAt(j) !== "/" && html.charAt(j) !== ">" && html.charAt(j) !== "=") j++;
    const name = html.slice(nameFrom, j);
    let k = j;
    while (k < n && isSpace(html.charAt(k))) k++;
    if (html.charAt(k) !== "=") continue; // an attribute with no value
    k++;
    while (k < n && isSpace(html.charAt(k))) k++;
    const first = html.charAt(k);
    let value: string;
    let after: number;
    if (first === '"' || first === "'") {
      const close = html.indexOf(first, k + 1);
      if (close === -1) return { end: n, edits }; // the value, and so the tag, never closes
      value = html.slice(k + 1, close);
      after = close + 1;
    } else {
      after = k;
      while (after < n && !isSpace(html.charAt(after)) && html.charAt(after) !== ">") after++;
      value = html.slice(k, after);
      if (value === "") {
        j = after; // `style=>`: no value, nothing to rename
        continue;
      }
    }
    if (rewrite && isStyle(name)) edits.push({ start: nameFrom, end: after, text: `${attrName}="${value.replaceAll('"', "&quot;")}"` });
    j = after;
  }
}

/** `html` with the style attribute of every start tag renamed to `attrName`. Everything else is returned as it came. */
export function rewriteStyleAttrsInTags(html: string, attrName: string): string {
  const n = html.length;
  let out = "";
  let copied = 0; // html[0, copied) is already in `out`
  let i = 0;
  while (i < n) {
    const lt = html.indexOf("<", i);
    if (lt === -1) break;
    const next = html.charAt(lt + 1);
    if (html.startsWith("<!--", lt)) {
      i = endOfComment(html, lt);
    } else if (isAsciiAlpha(next)) {
      const tag = scanTag(html, lt + 1, true, attrName);
      for (const edit of tag.edits) {
        out += html.slice(copied, edit.start) + edit.text;
        copied = edit.end;
      }
      i = tag.end;
    } else if (next === "/" && isAsciiAlpha(html.charAt(lt + 2))) {
      i = scanTag(html, lt + 2, false, attrName).end; // an end tag: its attributes are read and thrown away by the browser
    } else if (next === "!" || next === "/" || next === "?") {
      const gt = html.indexOf(">", lt + 2); // a doctype, CDATA, `</ x>` or `<?x>`: a comment up to the first `>`
      i = gt === -1 ? n : gt + 1;
    } else {
      i = lt + 1; // a `<` that starts nothing is text
    }
  }
  return out + html.slice(copied);
}

/**
 * The only script the editor may start: a same-origin file under /assets (its web worker, built by Vite with a hashed name).
 * Returns the absolute URL, or throws. No query, no fragment, no subdirectory, no other extension.
 */
export function checkWorkerUrl(url: string, origin: string): string {
  let parsed: URL;
  try {
    parsed = new URL(url, origin);
  } catch {
    throw new TypeError("refused script URL");
  }
  const plain = parsed.search === "" && parsed.hash === "" && /^\/assets\/[A-Za-z0-9._-]+\.js$/.test(parsed.pathname) && !parsed.pathname.includes("..");
  if (parsed.origin !== origin || !plain || /^(blob|data|javascript):/i.test(url.trim())) throw new TypeError("refused script URL");
  return parsed.href;
}
