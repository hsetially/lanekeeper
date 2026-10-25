// THE MONACO GLOBAL PATCHES (lint exception `monaco-global-patches`; docs/threat-model.md, "Web app (08)", row S12-M).
//
// Monaco under the hub's CSP (Trusted Types required, a nonce-only style-src, no style-src-attr) needs three changes to
// built-ins of the page. They are installed when the lazy editor chunk loads and never before, they are measured on every
// Monaco upgrade by e2e/editor/csp.spec.ts, and each is as narrow as it can be:
//
//  1. `Document.prototype.createElement`: a <style> element made by Monaco's `createStyleSheet` gets the page nonce, because
//     the CSP lets a <style> run only with it. Nothing else is touched.
//  2. `Element.prototype.setAttribute`: `setAttribute('style', v)` (the one direct call in Monaco's diff editor, in inline
//     mode) is written to `el.style.cssText`, which the CSP allows, instead of to the attribute, which it blocks. Every other
//     attribute goes to the original method.
//  3. A MutationObserver on the document that applies the style data the policy's createHTML rewrote (src/editor/policy.ts).
//
// What an attacker gains by these: nothing, unless they already control HTML or script on the page. The nonce goes on <style>
// elements that script creates (script is already trusted); `setAttribute('style')` is as powerful as `el.style.cssText`,
// which any script can do; the observer acts only on an attribute whose name is random per page load, so HTML that arrives
// any other way (docs HTML, whose allowlist has no data attributes) cannot use it.

import { applyStyleData, startStyleObserver } from "./style-apply";

export type Uninstall = () => void;

type PatchOptions = { nonce: string; styleAttr: string };

let installed: { uninstall: Uninstall } | undefined;

/** Install the patches. A second call while they are installed does nothing and returns a no-op. */
export function installGlobalPatches({ nonce, styleAttr }: PatchOptions): Uninstall {
  if (installed) return () => undefined;

  const originalCreate = Document.prototype.createElement;
  const originalSetAttribute = Element.prototype.setAttribute;

  Document.prototype.createElement = function createElement(this: Document, tag: string, options?: ElementCreationOptions): HTMLElement {
    const el = originalCreate.call(this, tag, options);
    if (tag.toLowerCase() === "style") el.nonce = nonce;
    return el;
  };

  Element.prototype.setAttribute = function setAttribute(this: Element, name: string, value: string): void {
    if (typeof name === "string" && name.toLowerCase() === "style" && (this instanceof HTMLElement || this instanceof SVGElement)) {
      this.style.cssText = value;
      return;
    }
    originalSetAttribute.call(this, name, value);
  };

  const observer = startStyleObserver(document, styleAttr);
  // Anything already in the document when the editor loads (nothing is, but a second editor may be).
  applyStyleData(document, styleAttr);

  const uninstall: Uninstall = () => {
    observer.disconnect();
    Document.prototype.createElement = originalCreate;
    Element.prototype.setAttribute = originalSetAttribute;
    installed = undefined;
  };
  installed = { uninstall };
  return uninstall;
}
