// The other half of the style rewrite (src/editor/policy.ts): copy the value of the data attribute into el.style, which is
// CSSOM and allowed by the CSP, and remove the attribute.

/**
 * A name for the data attribute that is new on every page load. Monaco's strings go through the policy, which uses this
 * name; HTML from anywhere else cannot carry it, so the observer never applies a style that Monaco did not write.
 */
export function createStyleAttrName(): string {
  const bytes = new Uint8Array(8);
  globalThis.crypto.getRandomValues(bytes);
  return `data-lk-s-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

function applyOne(el: Element, attr: string): void {
  const value = el.getAttribute(attr);
  if (value === null) return;
  el.removeAttribute(attr);
  if (el instanceof HTMLElement || el instanceof SVGElement) {
    try {
      el.style.cssText = value;
    } catch {
      // A value the browser refuses leaves the element unstyled, as an invalid style attribute would.
    }
  }
}

/** Apply every `attr` at or below `root`. */
export function applyStyleData(root: ParentNode | Element, attr: string): void {
  if (root instanceof Element && root.hasAttribute(attr)) applyOne(root, attr);
  for (const el of root.querySelectorAll(`[${attr}]`)) applyOne(el, attr);
}

/** Watch the document: whatever is added gets its style data applied, in the microtask after the insertion. */
export function startStyleObserver(doc: Document, attr: string): MutationObserver {
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.nodeType === Node.ELEMENT_NODE) applyStyleData(node as Element, attr);
      }
    }
  });
  observer.observe(doc, { childList: true, subtree: true });
  return observer;
}
