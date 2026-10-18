// Pseudo-localisation for the layout specs: the app has no translations, but a German label is about 40% longer than an English one,
// and a layout that only fits English is the layout of one language. This stretches every word of the interface's own text by 40%
// (with letters that carry diacritics, which are also taller) in the live DOM, and keeps doing it for what is drawn afterwards (a
// dialog, a virtualised row). Not touched: untrusted text (SafeText), code and keyboard hints, numbers, and the code editor.
import type { Page } from "@playwright/test";

export async function pseudoLocalise(page: Page, factor = 0.4): Promise<void> {
  await page.evaluate((f) => {
    const w = window as unknown as { __lkPseudo?: { count: number } };
    if (w.__lkPseudo) return;
    w.__lkPseudo = { count: 0 };
    const FILL = "äöüßéñ";
    const SKIP = "[data-slot='safe-text'], .monaco-editor, script, style, code, pre, kbd, textarea, input, select, [contenteditable='true']";
    const produced = new WeakMap<Node, string>();
    const stretch = (text: string): string =>
      text.replace(/[\p{L}][\p{L}\p{M}'’-]*/gu, (word) => {
        const extra = Math.ceil(Array.from(word).length * f);
        let pad = "";
        for (let i = 0; i < extra; i++) pad += FILL[i % FILL.length] ?? "";
        return word + pad;
      });
    const localise = (node: Node): void => {
      const text = node.nodeValue ?? "";
      if (text.trim() === "" || produced.get(node) === text) return;
      const parent = node.parentElement;
      if (parent === null || parent.closest(SKIP) !== null) return;
      const next = stretch(text);
      produced.set(node, next);
      node.nodeValue = next;
      (w.__lkPseudo as { count: number }).count += 1;
    };
    const walk = (root: Node): void => {
      const tree = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let n = tree.nextNode(); n !== null; n = tree.nextNode()) localise(n);
      if (root instanceof Element) for (const input of root.querySelectorAll("input[placeholder]")) input.setAttribute("placeholder", stretch(input.getAttribute("placeholder") ?? ""));
    };
    walk(document.body);
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === "characterData") localise(r.target);
        for (const added of r.addedNodes) walk(added);
      }
    }).observe(document.body, { childList: true, subtree: true, characterData: true });
  }, factor);
}

/** How many text nodes were stretched (a run that stretched none proves nothing). */
export async function pseudoCount(page: Page): Promise<number> {
  return page.evaluate(() => (window as unknown as { __lkPseudo?: { count: number } }).__lkPseudo?.count ?? 0);
}
