// Deciding what axe could not decide. An "incomplete" is axe saying "a person has to look". The spec is the person: each kind of incomplete
// that turns up has an answer that is computed in the browser, not assumed, and a result that is a pass leaves nothing behind while a
// result that is a fail is reported as the violation it is.
//
//   color-contrast, any reason   the node is scrolled to the middle of its scroller (a row at the edge of a table is cut by the edge, a cell past
//                                the right edge is under the page's own box) and axe is asked again about that one node. A node that
//                                is still undecided because something REALLY covers it stays undecided, and the spec fails on it.
//   color-contrast, nonBmp        axe does not read symbol-only text (a key hint "⌘" or an arrow): the ratio is computed from the colours the
//                                 browser resolved, the way WCAG 2.2 defines it (relative luminance), and compared with 4.5, or 3 for large text.
//   color-contrast, bgGradient    the hatch of an "absent" cell is two flat colours: the ratio is computed against EVERY colour of the gradient.
//   color-contrast in Monaco      Monaco draws its background in a layer beside the lines, so no ancestor of a token has it: the ratio is computed
//                                 against that layer.
//   aria-valid-attr-value (controlsWithinPopup)
//                                 axe flags every trigger that has aria-haspopup and aria-controls without looking: the spec checks that the id
//                                 names an element that exists, and that the trigger says it is expanded.
//   aria-hidden-focus (modal)     see modalHoldsFocus in axe.ts: Tab and Shift+Tab, 28 presses, focus never leaves the dialog.
import type { Page } from "@playwright/test";
import type { AxeNode } from "./axe.ts";

export type Resolution = { violations: Array<AxeNode & { ratio?: number | undefined; needed?: number | undefined }>; remaining: AxeNode[]; resolvedByAxe: number; resolvedByRatio: number };

type PageNode = { target: string; reason: string };
type PageResult = { index: number; verdict: "pass-axe" | "pass-ratio" | "fail" | "undecided"; ratio?: number; needed?: number; why?: string };

/** The selector of the node itself: the last of its target path (the ones before it lead through frames and shadow roots). */
function lastTarget(node: AxeNode): string {
  const last = node.target[node.target.length - 1];
  return typeof last === "string" ? last : "";
}

/** Ask the page about each undecided contrast node. */
export async function resolveContrast(page: Page, nodes: readonly AxeNode[]): Promise<Resolution> {
  const input: PageNode[] = nodes.map((n) => ({ target: lastTarget(n), reason: (n.reason ?? "").split(" ")[0] ?? "" }));
  const results = await page.evaluate(async (items: PageNode[]): Promise<PageResult[]> => {
    type Rgba = { r: number; g: number; b: number; a: number };
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    const parse = (value: string): Rgba | null => {
      if (ctx === null) return null;
      ctx.clearRect(0, 0, 1, 1);
      ctx.fillStyle = "#000";
      ctx.fillStyle = value;
      ctx.fillRect(0, 0, 1, 1);
      const d = ctx.getImageData(0, 0, 1, 1).data;
      // A transparent value reads back as 0,0,0,0; getImageData returns premultiplied-free bytes, so alpha is exact enough.
      return { r: d[0] ?? 0, g: d[1] ?? 0, b: d[2] ?? 0, a: (d[3] ?? 0) / 255 };
    };
    const over = (top: Rgba, under: Rgba): Rgba => {
      const a = top.a + under.a * (1 - top.a);
      if (a === 0) return { r: 0, g: 0, b: 0, a: 0 };
      return { r: (top.r * top.a + under.r * under.a * (1 - top.a)) / a, g: (top.g * top.a + under.g * under.a * (1 - top.a)) / a, b: (top.b * top.a + under.b * under.a * (1 - top.a)) / a, a };
    };
    const lum = (c: Rgba): number => {
      const f = (v: number): number => {
        const s = v / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
    };
    const ratioOf = (a: Rgba, b: Rgba): number => {
      const la = lum(a);
      const lb = lum(b);
      return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
    };
    const effectiveOpacity = (el: Element): number => {
      let o = 1;
      for (let a: Element | null = el; a !== null; a = a.parentElement) o *= Number(getComputedStyle(a).opacity);
      return o;
    };
    /** Every colour that can be behind the text of `el`: the nearest opaque background, and the stops of any gradient on the way. */
    const backgrounds = (el: Element): Rgba[][] => {
      const monaco = el.closest(".monaco-editor");
      if (monaco !== null) {
        const layer = monaco.querySelector(".monaco-editor-background");
        const c = layer ? parse(getComputedStyle(layer).backgroundColor) : null;
        if (c !== null && c.a > 0) return [[c]];
      }
      // Walk up, collecting layers (a layer is one or several colours); stop at the first fully opaque flat layer.
      const layers: Rgba[][] = [];
      for (let a: Element | null = el; a !== null; a = a.parentElement) {
        const cs = getComputedStyle(a);
        const image = cs.backgroundImage;
        const colour = parse(cs.backgroundColor);
        const stops = image !== "none" ? [...image.matchAll(/rgba?\([^)]*\)|oklch\([^)]*\)|color\([^)]*\)|#[0-9a-f]{3,8}\b/gi)].map((m) => parse(m[0])).filter((c): c is Rgba => c !== null) : [];
        if (stops.length > 0) layers.push(stops);
        if (colour !== null && colour.a > 0) {
          layers.push([colour]);
          if (colour.a >= 0.999 && stops.length === 0) break;
        }
      }
      layers.push([{ r: 255, g: 255, b: 255, a: 1 }]);
      return layers;
    };
    /** Composite the layers from the bottom up for every combination of the colours in them (a hatch has two). */
    const combos = (layers: Rgba[][]): Rgba[] => {
      let acc: Rgba[] = [{ r: 255, g: 255, b: 255, a: 1 }];
      for (const layer of [...layers].reverse()) acc = acc.flatMap((under) => layer.map((top) => over(top, under)));
      return acc;
    };
    const isLarge = (cs: CSSStyleDeclaration): boolean => {
      const px = parseFloat(cs.fontSize);
      const bold = Number(cs.fontWeight) >= 700;
      return px >= 24 || (bold && px >= 18.66);
    };
    const manual = (el: Element): { ok: boolean; ratio: number; needed: number } => {
      const cs = getComputedStyle(el);
      const fg = parse(cs.color);
      if (fg === null) return { ok: false, ratio: 0, needed: 4.5 };
      const o = effectiveOpacity(el);
      let worst = Infinity;
      for (const bg of combos(backgrounds(el))) {
        const text = over({ ...fg, a: fg.a * o }, bg);
        worst = Math.min(worst, ratioOf(text, bg));
      }
      const needed = isLarge(cs) ? 3 : 4.5;
      return { ok: worst >= needed, ratio: Math.round(worst * 100) / 100, needed };
    };
    /** At points inside each line of the node's own text, is nothing that paints above it? (The browser's hit test applies clips.) */
    const unobscured = (el: Element): boolean => {
      const range = document.createRange();
      const rects: DOMRect[] = [];
      const own = (n: Node): void => {
        for (const child of n.childNodes) {
          if (child.nodeType === 3 && (child.nodeValue ?? "").trim() !== "") {
            range.selectNodeContents(child);
            rects.push(...[...range.getClientRects()].filter((r) => r.width > 1 && r.height > 1));
          } else if (child.nodeType === 1 && !(child as Element).matches("[role='img'], svg")) own(child);
        }
      };
      own(el);
      if (rects.length === 0) rects.push(el.getBoundingClientRect());
      for (const r of rects) {
        for (const [fx, fy] of [[0.5, 0.5], [0.15, 0.5], [0.85, 0.5], [0.5, 0.25], [0.5, 0.75]] as const) {
          const x = r.left + r.width * fx;
          const y = r.top + r.height * fy;
          if (x < 0 || y < 0 || x >= window.innerWidth || y >= window.innerHeight) return false;
          const stack = document.elementsFromPoint(x, y);
          const at = stack.findIndex((e) => e === el || el.contains(e) || e.contains(el));
          if (at < 0) return false;
          for (const above of stack.slice(0, at)) {
            const cs = getComputedStyle(above);
            const bg = parse(cs.backgroundColor);
            if ((bg !== null && bg.a > 0.02) || cs.backgroundImage !== "none") return false;
          }
        }
      }
      return true;
    };
    /**
     * Is the node's text covered by a layer that is meant to be above the page (a drawer, a dialog and its backdrop, a popover: positioned
     * `fixed`, or a header that is `sticky`)? Then it is not what a person is reading, and what axe cannot say about it is not a finding;
     * the ratio against the node's own backgrounds is still computed, so a node that could be read when uncovered has to pass too.
     */
    const coveredByLayer = (el: Element): boolean => {
      const range = document.createRange();
      const rects: DOMRect[] = [];
      for (const child of el.childNodes) {
        if (child.nodeType === 3 && (child.nodeValue ?? "").trim() !== "") {
          range.selectNodeContents(child);
          rects.push(...[...range.getClientRects()].filter((x) => x.width > 1 && x.height > 1));
        }
      }
      if (rects.length === 0) rects.push(el.getBoundingClientRect());
      for (const r of rects) {
        for (const [fx, fy] of [[0.5, 0.5], [0.1, 0.5], [0.9, 0.5], [0.3, 0.5], [0.7, 0.5]] as const) {
          const x = r.left + r.width * fx;
          const y = r.top + r.height * fy;
          if (x < 0 || y < 0 || x >= window.innerWidth || y >= window.innerHeight) continue;
          const top = document.elementFromPoint(x, y);
          if (top === null || top === el || el.contains(top) || top.contains(el)) continue;
          for (let a: Element | null = top; a !== null; a = a.parentElement) {
            const cs = getComputedStyle(a);
            const role = a.getAttribute("role");
            // A fixed or sticky box, a dialog (a drawer is one), or a box that is placed over the page and has its own z-order.
            if (cs.position === "fixed" || cs.position === "sticky" || role === "dialog" || role === "alertdialog" || (cs.position === "absolute" && cs.zIndex !== "auto")) return true;
          }
        }
      }
      return false;
    };
    /** Is the node cut by the edge of a scroller or of a clip, so that part of it is not drawn (a row at the bottom of a code view)? */
    const cutByAnEdge = (el: Element): boolean => {
      const r = el.getBoundingClientRect();
      for (let a = el.parentElement; a !== null; a = a.parentElement) {
        const cs = getComputedStyle(a);
        if (cs.overflowX === "visible" && cs.overflowY === "visible") continue;
        const box = a.getBoundingClientRect();
        const left = box.left + a.clientLeft;
        const top = box.top + a.clientTop;
        if (r.left < left - 1 || r.right > left + a.clientWidth + 1 || r.top < top - 1 || r.bottom > top + a.clientHeight + 1) return true;
      }
      return r.top < -1 || r.left < -1 || r.bottom > window.innerHeight + 1 || r.right > window.innerWidth + 1;
    };
    const frame = (): Promise<void> => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    const w = window as unknown as { axe: { run: (ctx: unknown, opts: unknown) => Promise<{ violations: unknown[]; incomplete: unknown[]; passes: unknown[] }> } };

    const out: PageResult[] = [];
    const lookup = (selector: string): Element | null => {
      try {
        return document.querySelector(selector);
      } catch {
        return null;
      }
    };
    const verdictOf = (index: number, m: { ok: boolean; ratio: number; needed: number }): PageResult => (m.ok ? { index, verdict: "pass-ratio", ratio: m.ratio, needed: m.needed } : { index, verdict: "fail", ratio: m.ratio, needed: m.needed });
    const askAxe = async (selector: string): Promise<"pass" | "fail" | "undecided"> => {
      const again = await w.axe.run({ include: [[selector]] }, { runOnly: { type: "rule", values: ["color-contrast"] }, resultTypes: ["violations", "incomplete", "passes"] });
      if (again.violations.length > 0) return "fail";
      return again.passes.length > 0 && again.incomplete.length === 0 ? "pass" : "undecided";
    };
    const later: number[] = [];

    // Pass 1, without moving anything: what is not on the screen at all (a virtual list draws a few rows beyond the window), what is
    // not drawn, what is symbols or a gradient or Monaco, and what axe decides when it is asked about the node alone.
    for (const [index, item] of items.entries()) {
      const el = lookup(item.target);
      if (el === null) {
        out[index] = { index, verdict: "undecided", why: "the element is gone" };
        continue;
      }
      const box = el.getBoundingClientRect();
      // Monaco's input proxy (a readonly textarea it keeps for the keyboard and the IME) draws no text, whatever its size.
      if (box.width < 2 || box.height < 2 || (el.tagName === "TEXTAREA" && el.closest(".monaco-editor") !== null)) {
        // Nothing is drawn (Monaco's input proxy is a 1 px textarea): there is no text to read.
        out[index] = { index, verdict: "pass-ratio", ratio: 21, needed: 4.5 };
        continue;
      }
      const symbolic = item.reason === "nonBmp" || item.reason === "bgGradient" || item.reason === "bgImage" || el.closest(".monaco-editor") !== null;
      const offscreen = box.bottom <= 0 || box.top >= window.innerHeight || box.right <= 0 || box.left >= window.innerWidth;
      // The cheap answers first, each ending in the ratio computed from the colours the browser resolved:
      //  - not on the screen (an overscan row, a cell past the edge), symbols, a gradient, Monaco: axe does not read them;
      //  - covered by a layer meant to be above the page (a drawer, a dialog and its backdrop, a sticky header): not what a person reads;
      //  - cut by the edge of its scroller (a row at the bottom of a code view): the ratio does not depend on how much of it shows;
      //  - unobscured by the browser's own hit testing, which applies the clip of an `overflow: hidden` parent that axe's geometry ignores
      //    (an ellipsis, a breadcrumb that gives way): at points inside each line of the node's text, everything above it paints nothing.
      if (symbolic || offscreen || coveredByLayer(el) || cutByAnEdge(el) || unobscured(el)) {
        out[index] = verdictOf(index, manual(el));
        continue;
      }
      // Anything else: ask axe about this one node (its neighbours are not in the way now), and if it still cannot say, bring the node into
      // the middle of its scroller in pass 2.
      const answer = await askAxe(item.target);
      if (answer === "pass") out[index] = { index, verdict: "pass-axe" };
      else if (answer === "fail") out[index] = { index, verdict: "fail", ratio: manual(el).ratio, needed: manual(el).needed };
      else later.push(index);
    }

    // Pass 2: what is on the screen but cut by an edge of its scroller or of the window: bring it into the middle and ask axe again.
    for (const index of later) {
      const item = items[index];
      if (item === undefined) continue;
      const el = lookup(item.target);
      if (el === null) {
        out[index] = { index, verdict: "undecided", why: "the element is gone (a virtual row that scrolled away)" };
        continue;
      }
      el.scrollIntoView({ block: "center", inline: "center" });
      await frame();
      const answer = await askAxe(item.target);
      const now = lookup(item.target) ?? el;
      if (answer === "pass") out[index] = { index, verdict: "pass-axe" };
      else if (answer === "fail") out[index] = { index, verdict: "fail", ratio: manual(now).ratio, needed: manual(now).needed };
      else if (unobscured(now) || coveredByLayer(now)) out[index] = verdictOf(index, manual(now));
      else out[index] = { index, verdict: "undecided", why: "axe cannot decide with the node in the middle of its scroller, and something that paints is above its text" };
    }
    return out;
  }, input);

  const resolution: Resolution = { violations: [], remaining: [], resolvedByAxe: 0, resolvedByRatio: 0 };
  for (const r of results) {
    const node = nodes[r.index];
    if (node === undefined) continue;
    if (r.verdict === "pass-axe") resolution.resolvedByAxe += 1;
    else if (r.verdict === "pass-ratio") resolution.resolvedByRatio += 1;
    else if (r.verdict === "fail") resolution.violations.push({ ...node, ratio: r.ratio, needed: r.needed });
    else resolution.remaining.push({ ...node, reason: `${node.reason ?? ""} | still undecided: ${r.why ?? ""}` });
  }
  return resolution;
}

/** `aria-controls` on a popup trigger: the id must name an element that exists, and the trigger must say it is open. */
export async function resolveControls(page: Page, nodes: readonly AxeNode[]): Promise<{ failed: string[]; resolved: number }> {
  const targets = nodes.map(lastTarget);
  const failed = await page.evaluate((list: string[]) => {
    const bad: string[] = [];
    for (const target of list) {
      const el = document.querySelector(target);
      if (el === null) {
        bad.push(`${target}: gone`);
        continue;
      }
      const id = el.getAttribute("aria-controls") ?? "";
      const named = id.split(/\s+/).filter(Boolean);
      if (named.length === 0 || named.some((x) => document.getElementById(x) === null)) bad.push(`${target}: aria-controls="${id}" names no element on the page`);
      else if (el.getAttribute("aria-expanded") !== "true") bad.push(`${target}: aria-controls is set but the trigger does not say it is expanded`);
    }
    return bad;
  }, targets);
  return { failed, resolved: nodes.length - failed.length };
}
