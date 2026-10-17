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
// The rewrite is tag-aware: it touches an attribute only inside a real tag, honouring quoted values. A regular expression
// over the whole string also matched model text that happened to contain `style="..."` and corrupted what the editor showed
// (4 of 5 test lines in G0).

const STYLE_ATTR = /(\s)style\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))/gi;

/** `html` with the style attribute of every tag renamed to `attrName`. Text between tags is returned as it came. */
export function rewriteStyleAttrsInTags(html: string, attrName: string): string {
  const n = html.length;
  let out = "";
  let i = 0;
  while (i < n) {
    const lt = html.indexOf("<", i);
    if (lt === -1) {
      out += html.slice(i);
      break;
    }
    out += html.slice(i, lt);
    if (!/[A-Za-z/!]/.test(html[lt + 1] ?? "")) {
      out += "<";
      i = lt + 1;
      continue;
    }
    // Scan to the closing '>' of this tag, honouring quoted attribute values.
    let j = lt + 1;
    let quote: string | null = null;
    while (j < n) {
      const ch = html.charAt(j);
      if (quote !== null) {
        if (ch === quote) quote = null;
      } else if (ch === '"' || ch === "'") quote = ch;
      else if (ch === ">") break;
      j++;
    }
    out += html.slice(lt, j + 1).replace(STYLE_ATTR, (_match, ws: string, dq: string | undefined, sq: string | undefined, bare: string | undefined) => {
      const value = dq ?? (sq !== undefined ? sq.replaceAll('"', "&quot;") : (bare ?? ""));
      return `${ws}${attrName}="${value}"`;
    });
    i = j + 1;
  }
  return out;
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
