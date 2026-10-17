// The page's CSP nonce (S12). The hub puts a fresh nonce in the header of every response and in the page; Vite's
// `html.cspNonce` writes the placeholder `__CSP_NONCE__` into every generated tag and into
// `<meta property="csp-nonce" nonce="...">`, and the hub swaps in the real value. Script and style tags the build wrote
// already carry it. This module makes it available to code that creates a <style> later (Radix's scroll lock reads
// `window.__webpack_nonce__`; Monaco's stylesheets get it from src/editor/style-nonce.ts).
//
// The value must be read from the element's `.nonce` property: a browser hides the `nonce` attribute of a connected
// element from `getAttribute`, so the attribute reads as an empty string.

const NONCE = /^[A-Za-z0-9+/_=-]{8,128}$/;
/** What the build writes and the hub replaces. Seeing it means nothing replaced it (a dev server), so there is no nonce. */
const PLACEHOLDER = "__CSP_NONCE__";

/** The nonce of this page, or undefined (a dev server has none, and an unsupported value is ignored). */
export function readNonce(doc: Document = document): string | undefined {
  const meta = doc.querySelector<HTMLMetaElement>('meta[property="csp-nonce"]');
  if (!meta) return undefined;
  const value = (meta as HTMLElement & { nonce?: string }).nonce || meta.getAttribute("nonce") || "";
  return value !== PLACEHOLDER && NONCE.test(value) ? value : undefined;
}

/** Make the nonce available as `window.__webpack_nonce__`. Returns it. */
export function installNonce(win: Window = window, doc: Document = document): string | undefined {
  const nonce = readNonce(doc);
  if (nonce !== undefined) win.__webpack_nonce__ = nonce;
  return nonce;
}
