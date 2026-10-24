// The geometry audit of the layout specs: what the browser says about where text and controls are, read with bounding boxes and scroll
// sizes (never a screenshot). One function, run inside the page, returns the defects it found; the specs assert that the lists are empty.
//
// What counts as a defect (design/13, "responsive"; plan, "Layout matrix"):
//   - page scroll    the page scrolls sideways. The shell is one frame of the viewport's height with `overflow: hidden`, and <main> is the
//                    scroller of the content, so "the page" is <main> (and the document, and the body, for a page with no shell).
//   - outside        a control or a piece of text that, after the clipping of the containers it sits in, still reaches past the content box
//                    of <main> (or past the window, for the sidebar and the header).
//   - clipped        text cut off by an `overflow: hidden|clip` ancestor that is not a scroller and has no `text-overflow: ellipsis`: it is
//                    lost, with no sign that it is. (A truncation with an ellipsis is intentional and counted, not reported.)
//   - overlap        the visible text of two elements covers each other (area more than 15% of the smaller one's).
//   - tables         a container that scrolls sideways is reported, so that a spec can say a table scrolls and the page does not.
import type { Page } from "@playwright/test";
import type { Harness } from "../harness/serve.ts";

/**
 * Wait until the page has stopped asking the hub for things: the number of requests the harness has seen has not changed for `quietMs`.
 * A screen that loads its list page after page (the findings, 1,000 of them in eleven requests) or fills a toolbar from the answer moves
 * what is under it when the last answer comes, and a spec that measures where things are would catch it half way. (The live stream never
 * ends, so "the network is idle" is not a state these pages reach; a request that is held back is simply one that does not come.)
 */
export async function requestsQuiet(page: Page, harness: Harness, quietMs = 600, limitMs = 15_000): Promise<void> {
  const started = Date.now();
  let last = -1;
  let since = Date.now();
  while (Date.now() - since < quietMs && Date.now() - started < limitMs) {
    const count = harness.requests().length;
    if (count !== last) {
      last = count;
      since = Date.now();
    }
    await page.waitForTimeout(50);
  }
}

export type Defect = { what: string; where: string; detail?: string };
export type LayoutReport = {
  viewport: { width: number; height: number };
  pageScroll: Defect[];
  outside: Defect[];
  clipped: Defect[];
  overlap: Defect[];
  /** Containers that scroll sideways (a table, a code view, a tree), with how much. */
  scrollers: Array<{ where: string; scrollWidth: number; clientWidth: number }>;
  /** Text that is cut short on purpose, with an ellipsis. */
  truncated: number;
  textRuns: number;
  controls: number;
};

export type AuditOptions = {
  /** Text under this selector is not looked at (a code editor draws its own layers; Monaco is audited by its own spec). */
  skip?: string;
  /** How many defects of each kind to return (the first ones). */
  limit?: number;
};

export async function auditLayout(page: Page, options: AuditOptions = {}): Promise<LayoutReport> {
  return page.evaluate(
    ({ skip, limit }) => {
      const EPS = 1.5;
      type Box = { left: number; top: number; right: number; bottom: number };
      const out: LayoutReport = {
        viewport: { width: window.innerWidth, height: window.innerHeight },
        pageScroll: [],
        outside: [],
        clipped: [],
        overlap: [],
        scrollers: [],
        truncated: 0,
        textRuns: 0,
        controls: 0,
      };
      // The same element is reported once however many lines of it are looked at.
      const known = new Set<string>();
      const push = (list: Defect[], d: Defect): void => {
        const key = `${d.what}|${d.where}`;
        if (list.length >= limit || known.has(key)) return;
        known.add(key);
        list.push(d);
      };
      const name = (el: Element): string => {
        const slot = el.getAttribute("data-slot");
        const cls = typeof el.className === "string" ? el.className.split(/\s+/).filter(Boolean).slice(0, 3).join(".") : "";
        const text = (el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40);
        return `${el.tagName.toLowerCase()}${el.id ? `#${el.id}` : ""}${slot ? `[${slot}]` : ""}${cls ? `.${cls}` : ""}${text ? ` "${text}"` : ""}`;
      };
      const box = (r: DOMRect | Box): Box => ({ left: r.left, top: r.top, right: r.right, bottom: r.bottom });
      const inter = (a: Box, b: Box): Box | null => {
        const left = Math.max(a.left, b.left);
        const top = Math.max(a.top, b.top);
        const right = Math.min(a.right, b.right);
        const bottom = Math.min(a.bottom, b.bottom);
        return right > left && bottom > top ? { left, top, right, bottom } : null;
      };
      const area = (b: Box): number => Math.max(0, b.right - b.left) * Math.max(0, b.bottom - b.top);

      const main = document.querySelector("main");
      const root = document.documentElement;
      const isHiddenBox = (el: Element, cs: CSSStyleDeclaration): boolean => {
        if (cs.display === "none" || cs.visibility === "hidden" || cs.visibility === "collapse") return true;
        if (Number(cs.opacity) === 0) return true;
        // The visually hidden idiom (sr-only, the focus guards): one pixel, clipped, or moved out of sight.
        const r = el.getBoundingClientRect();
        if (r.width <= 1.01 && r.height <= 1.01) return true;
        if (cs.getPropertyValue("clip") === "rect(0px, 0px, 0px, 0px)" || cs.clipPath === "inset(50%)") return true;
        return false;
      };
      const hiddenCache = new Map<Element, boolean>();
      const hidden = (el: Element | null): boolean => {
        if (el === null) return false;
        const known = hiddenCache.get(el);
        if (known !== undefined) return known;
        const cs = getComputedStyle(el);
        const value = isHiddenBox(el, cs) || (el.parentElement !== null && hidden(el.parentElement));
        hiddenCache.set(el, value);
        return value;
      };

      // The containers that cut what is inside them. Per axis: it scrolls (what is past the edge is reachable), it clips (what is past
      // the edge is lost), or neither.
      type Cut = { el: Element; box: Box; scrollX: boolean; scrollY: boolean; clipX: boolean; clipY: boolean };
      const cutsCache = new Map<Element, Cut | null>();
      const cutOf = (el: Element): Cut | null => {
        const known = cutsCache.get(el);
        if (known !== undefined) return known;
        const cs = getComputedStyle(el);
        const scrolls = (v: string): boolean => v === "auto" || v === "scroll";
        const clips = (v: string): boolean => v === "hidden" || v === "clip";
        let cut: Cut | null = null;
        if (scrolls(cs.overflowX) || scrolls(cs.overflowY) || clips(cs.overflowX) || clips(cs.overflowY)) {
          const r = el.getBoundingClientRect();
          const left = r.left + el.clientLeft;
          const top = r.top + el.clientTop;
          cut = { el, box: { left, top, right: left + el.clientWidth, bottom: top + el.clientHeight }, scrollX: scrolls(cs.overflowX), scrollY: scrolls(cs.overflowY), clipX: clips(cs.overflowX), clipY: clips(cs.overflowY) };
        }
        cutsCache.set(el, cut);
        return cut;
      };
      const BLOCK = new Set(["block", "flow-root", "inline-block", "table-cell", "list-item"]);
      /** Is what is in `el` elided with an ellipsis (or clamped to lines) by the container `cutEl`, as a person would see it? */
      const elided = (el: Element, cutEl: Element): boolean => {
        const cs = getComputedStyle(cutEl);
        const clamp = (cs as unknown as { webkitLineClamp?: string }).webkitLineClamp;
        if (clamp !== undefined && clamp !== "none" && clamp !== "") return true;
        if (!cs.textOverflow.includes("ellipsis") || !BLOCK.has(cs.display)) return false;
        // An atomic inline or a flex or grid box in between is clipped whole, not elided.
        for (let a: Element | null = el; a !== null && a !== cutEl; a = a.parentElement) if (getComputedStyle(a).display !== "inline") return false;
        return true;
      };
      const positionFixed = (el: Element): boolean => getComputedStyle(el).position === "fixed";

      // 1. Does the page scroll sideways?
      const se = document.scrollingElement ?? root;
      if (se.scrollWidth > se.clientWidth + 1) push(out.pageScroll, { what: "the document scrolls sideways", where: "html", detail: `${se.scrollWidth} > ${se.clientWidth}` });
      if (document.body.scrollWidth > document.body.clientWidth + 1 && getComputedStyle(document.body).overflowX !== "visible") push(out.pageScroll, { what: "the body scrolls sideways", where: "body", detail: `${document.body.scrollWidth} > ${document.body.clientWidth}` });
      if (main && main.scrollWidth > main.clientWidth + 1) push(out.pageScroll, { what: "the main region scrolls sideways", where: name(main), detail: `${main.scrollWidth} > ${main.clientWidth}` });

      // 2. The text and the controls that are drawn.
      type Run = { el: Element; box: Box; visible: Box; text: string };
      const runs: Run[] = [];
      const skipEl = skip ? document.querySelector(skip) : null;
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
        const text = node.nodeValue ?? "";
        if (text.trim() === "") continue;
        const el = node.parentElement;
        if (el === null || hidden(el)) continue;
        const tag = el.tagName;
        if (tag === "SCRIPT" || tag === "STYLE" || tag === "NOSCRIPT" || tag === "OPTION") continue;
        if (skipEl !== null && skipEl.contains(el)) continue;
        range.selectNodeContents(node);
        const rects = [...range.getClientRects()].filter((r) => r.width > 0.5 && r.height > 0.5);
        if (rects.length === 0) continue;
        // One run per line box: the box around all the lines of a paragraph would take in the inline elements that sit in it.
        for (const r of rects) {
          const b: Box = { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
          runs.push({ el, box: b, visible: b, text: text.trim().slice(0, 50) });
        }
      }
      out.textRuns = runs.length;

      const controlSelector = "button, a[href], input, select, textarea, img, svg[role='img'], [role='button'], [role='tab'], [role='checkbox'], [role='switch'], [role='radio'], [role='menuitem'], [role='option']";
      const controls: Array<{ el: Element; box: Box; visible: Box | null }> = [];
      for (const el of document.querySelectorAll(controlSelector)) {
        if (hidden(el)) continue;
        if (skipEl !== null && skipEl.contains(el)) continue;
        const r = el.getBoundingClientRect();
        if (r.width < 1 || r.height < 1) continue;
        controls.push({ el, box: box(r), visible: null });
      }
      out.controls = controls.length;

      // 3. Clip each box by the containers above it; say what was lost.
      const mainRect = main ? main.getBoundingClientRect() : null;
      const mainBox: Box | null = main && mainRect ? { left: mainRect.left + main.clientLeft, top: mainRect.top, right: mainRect.left + main.clientLeft + main.clientWidth, bottom: mainRect.bottom } : null;
      const windowBox: Box = { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
      const examine = (el: Element, b: Box, isText: boolean, label: string): Box | null => {
        let visible: Box | null = b;
        let fixed = false;
        let lostBy: Element | null = null;
        let elidedHere = false;
        for (let a: Element | null = el; a !== null; a = a.parentElement) {
          if (a === main) break;
          if (positionFixed(a)) fixed = true;
          // The text of an element is clipped by the element's own overflow; the element itself is clipped only by what is above it.
          const cut = a === el && !isText ? null : cutOf(a);
          if (cut !== null) {
            // What is already past an inner scroller is not seen; what is lost is judged on what is still seen.
            const lostX = cut.clipX && !cut.scrollX && (visible.left < cut.box.left - EPS || visible.right > cut.box.right + EPS);
            const lostY = cut.clipY && !cut.scrollY && (visible.top < cut.box.top - EPS || visible.bottom > cut.box.bottom + EPS);
            if ((lostX || lostY) && lostBy === null) {
              // A plain link (an inline `a`) in a container that elides its text with an ellipsis is a truncated link, as its text is.
              if ((isText || (el.tagName === "A" && getComputedStyle(el).display === "inline")) && elided(el, cut.el)) elidedHere = true;
              else lostBy = cut.el;
            }
            visible = inter(visible, cut.box);
            if (visible === null) return null;
          }
          if (fixed) break;
        }
        if (lostBy !== null) {
          // What stands between the text and the container that cuts it: an ellipsis does not reach through a flex or grid box.
          const via: string[] = [];
          for (let a: Element | null = el; a !== null && a !== lostBy; a = a.parentElement) {
            const display = getComputedStyle(a).display;
            if (display !== "inline") via.push(`${a.tagName.toLowerCase()}:${display}`);
          }
          push(out.clipped, { what: isText ? "text is cut off with no ellipsis" : "a control is cut off", where: `${label} in ${name(lostBy)}`, detail: `at ${Math.round(b.left)}..${Math.round(b.right)} x ${Math.round(b.top)}..${Math.round(b.bottom)}${via.length > 0 ? ` via ${via.join(" < ")}` : ""}` });
        }
        if (elidedHere) out.truncated += 1;
        // Past the content box of <main> (or past the window, outside it): the page would have to scroll sideways, or it is lost.
        const inMain = !fixed && main !== null && main.contains(el) && mainBox !== null;
        const limit: Box = inMain ? { ...windowBox, left: mainBox.left, right: mainBox.right } : windowBox;
        if (visible.right > limit.right + EPS || visible.left < limit.left - EPS) {
          push(out.outside, { what: isText ? "text reaches past the content box" : "a control reaches past the content box", where: label, detail: `${Math.round(visible.left)}..${Math.round(visible.right)} against ${Math.round(limit.left)}..${Math.round(limit.right)}` });
        }
        return visible;
      };

      const seen = new Set<Element>();
      for (const r of runs) {
        const v = examine(r.el, r.box, true, name(r.el));
        r.visible = v ?? { left: 0, top: 0, right: 0, bottom: 0 };
      }
      for (const c of controls) {
        if (seen.has(c.el)) continue;
        seen.add(c.el);
        c.visible = examine(c.el, c.box, false, name(c.el));
      }

      // 4. Scrollers that scroll sideways (the tables and code views).
      for (const el of document.querySelectorAll("*")) {
        if (el.scrollWidth > el.clientWidth + 1 && el !== main && el !== root && el !== document.body) {
          const cs = getComputedStyle(el);
          if ((cs.overflowX === "auto" || cs.overflowX === "scroll") && !hidden(el)) out.scrollers.push({ where: name(el), scrollWidth: el.scrollWidth, clientWidth: el.clientWidth });
        }
      }

      // 5. Text over text, and a control over text. Only what is really on top counts. A run that is under a layer that is meant to be above
      // it (a sticky header the rows scroll under, a drawer, a dialog, a popover: positioned `sticky` or `fixed`) is not seen and is not a
      // defect. A run that is under something in the normal flow (a button that grew into its neighbour's column) is.
      const live = runs.filter((r) => area(r.visible) > 0);
      // The layer (sticky or fixed: a header, a drawer, a dialog, a popover, a tooltip) that is on top at a point, if any.
      const layerAt = (x: number, y: number): Element | null => {
        if (x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight) return null;
        for (let a: Element | null = document.elementFromPoint(x, y); a !== null; a = a.parentElement) {
          const cs = getComputedStyle(a);
          if (cs.position === "sticky" || cs.position === "fixed") return a;
          // A panel laid over the page inside its box (the findings drawer: `absolute`, with a z-index and a background that hides what is under it).
          if (cs.position === "absolute" && cs.zIndex !== "auto" && cs.backgroundColor !== "rgba(0, 0, 0, 0)" && cs.backgroundColor !== "transparent") return a;
        }
        return null;
      };
      // Two elements whose boxes meet at `o` do not cover each other when a layer is on top of that place and not both are in the layer:
      // one of them is under it. When both are in it (two texts of a drawer), or when there is no layer, they do.
      const underALayer = (o: Box, a: Element, b: Element): boolean => {
        const layer = layerAt((o.left + o.right) / 2, (o.top + o.bottom) / 2);
        return layer !== null && !(layer.contains(a) && layer.contains(b));
      };
      const sorted = [...live].sort((a, b) => a.visible.left - b.visible.left);
      for (let i = 0; i < sorted.length; i++) {
        const a = sorted[i];
        if (a === undefined) continue;
        for (let j = i + 1; j < sorted.length; j++) {
          const b = sorted[j];
          if (b === undefined || b.visible.left >= a.visible.right - EPS) break;
          if (a.el === b.el) continue;
          const o = inter(a.visible, b.visible);
          if (o === null) continue;
          const smaller = Math.min(area(a.visible), area(b.visible));
          if (area(o) < 4 || area(o) < smaller * 0.15) continue;
          if (underALayer(o, a.el, b.el)) continue;
          push(out.overlap, { what: "text covers text", where: `${name(a.el)}  <>  ${name(b.el)}`, detail: `${Math.round(area(o))}px2 of ${Math.round(smaller)}px2` });
        }
      }
      // A control (a button, a link, a field) whose box covers some other element's text. A control with nothing in it is an overlay that
      // is meant to cover (a stretched link) and is left out.
      for (const c of controls) {
        const hasContent = (c.el.textContent ?? "").trim() !== "" || c.el.querySelector("svg, img") !== null || c.el instanceof HTMLInputElement || c.el instanceof HTMLSelectElement;
        if (!hasContent) continue;
        const clip = c.visible;
        if (clip === null) continue;
        for (const r of live) {
          if (c.el.contains(r.el) || r.el.contains(c.el)) continue;
          const o = inter(clip, r.visible);
          if (o === null || area(o) < 6 || area(o) < area(r.visible) * 0.15) continue;
          if (underALayer(o, c.el, r.el)) continue;
          push(out.overlap, { what: "a control covers text", where: `${name(c.el)}  >>  ${name(r.el)}`, detail: `${Math.round(area(o))}px2 of ${Math.round(area(r.visible))}px2` });
        }
      }
      return out;
    },
    { skip: options.skip ?? "", limit: options.limit ?? 12 },
  );
}

/** One line per defect, for an assertion message. */
export function describeReport(label: string, report: LayoutReport): string {
  const lines: string[] = [];
  for (const [kind, list] of [["page scroll", report.pageScroll], ["outside", report.outside], ["clipped", report.clipped], ["overlap", report.overlap]] as const) {
    for (const d of list) lines.push(`  ${kind}: ${d.what}: ${d.where}${d.detail ? ` (${d.detail})` : ""}`);
  }
  return `${label} at ${report.viewport.width}x${report.viewport.height}\n${lines.join("\n")}`;
}

export function hasDefects(report: LayoutReport): boolean {
  return report.pageScroll.length + report.outside.length + report.clipped.length + report.overlap.length > 0;
}
