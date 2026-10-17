// Measurements of Monaco's DOM, run in the page, ported from the G0 preflight (plans/08-frontend.blockers.md). They are
// strings so they run in the page without a bundler; every number they return is one the G0 report recorded as correct.

/** What the editor's DOM looks like: lines, overlay boxes, widths, leftovers of the style rewrite. */
export const MEASURE = `(dom) => {
  const q = (s) => [...dom.querySelectorAll(s)];
  const top0 = dom.getBoundingClientRect().top;
  const tops = (els) => els.map((e) => Math.round((e.getBoundingClientRect().top - top0) * 10) / 10);
  const distinct = (a) => new Set(a).size;
  const lines = q('.view-lines > .view-line');
  const lt = tops(lines);
  const nums = q('.margin-view-overlays .line-numbers');
  const boxes = (selector) => { const els = q(selector); const nz = els.filter((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }).length; return [els.length, nz]; };
  const leftover = [...dom.querySelectorAll('*')].filter((e) => [...e.attributes].some((a) => a.name.startsWith('data-lk-s-'))).length;
  const current = q('.current-line');
  return {
    viewLines: lines.length,
    viewLineDistinctTops: distinct(lt),
    overlayBoxes: { selectedText: boxes('.selected-text'), currentLine: boxes('.current-line'), indentGuides: boxes('.core-guide'), decorationRanges: boxes('.cdr'), cursor: boxes('.cursor') },
    lineNumbers: nums.length,
    currentLineWidth: Math.max(0, ...current.map((e) => Math.round(e.getBoundingClientRect().width))),
    leftoverStyleData: leftover,
    mtkClasses: distinct(q('.view-line span[class^="mtk"]').map((e) => e.className)),
    host: [Math.round(dom.getBoundingClientRect().width), Math.round(dom.getBoundingClientRect().height)],
  };
}`;

/** Where each rendered line sits against where Monaco says it should (getTopForLineNumber minus the scroll position). */
export const FIDELITY = `(ed) => {
  const dom = ed.getDomNode();
  const model = ed.getModel();
  const top0 = dom.getBoundingClientRect().top;
  const strip = (t) => t.replace(/\\u00a0/g, ' ').replace(/[\\u200b\\u202e]/g, '');
  const byText = new Map();
  for (let n = 1; n <= model.getLineCount(); n++) byText.set(strip(model.getLineContent(n)), n);
  const rows = [];
  for (const el of dom.querySelectorAll('.view-lines > .view-line')) {
    const n = byText.get(strip(el.textContent));
    if (!n) continue;
    const expected = ed.getTopForLineNumber(n) - ed.getScrollTop();
    const observed = el.getBoundingClientRect().top - top0;
    rows.push(Math.round(Math.abs(observed - expected)));
  }
  return { matched: rows.length, maxDev: Math.max(0, ...rows), within2px: rows.filter((d) => d <= 2).length };
}`;

/** Records every Trusted Types policy the page tries to create, and whether the browser allowed it. */
export const POLICY_RECORDER = `(() => {
  const rec = (window.__policies = []);
  if (!window.trustedTypes) return;
  const proto = Object.getPrototypeOf(window.trustedTypes);
  const original = proto.createPolicy;
  proto.createPolicy = function (name, rules) {
    try { const policy = original.call(this, name, rules); rec.push({ name, ok: true }); return policy; }
    catch (error) { rec.push({ name, ok: false, error: String(error && error.message).slice(0, 160) }); throw error; }
  };
})();`;
