// T1 Verify for the editor (plan W3, option (e), G0 variant 3d): Monaco under the PRODUCTION CSP, repeating the stages of the
// G0 measurement so that every Monaco upgrade is measured again. The page is a test-only route of the perf-mode build served
// through the hub's own header code (Trusted Types required, two policy names, nonce-only style, no style-src-attr).
//
// What must hold, in a real Chromium: zero violation reports of any kind in every stage; ONE policy created
// (`lanekeeper-monaco`) and none refused; the editor renders correctly (lines where Monaco puts them, overlays with a size,
// selection, find, palette, wrap, fold, multi-cursor, a 4,000-line scroll sweep, the diff editor in both modes, five
// languages, Unicode highlighting); model text containing `style="..."` is shown as it is; and the features that render
// Markdown (hover, suggest) are not there at all.
import { expect, pageViolations, test } from "../fixtures/test";
import { FIDELITY, MEASURE, POLICY_RECORDER } from "./measure";

test.use({ dist: "dist-perf", scenario: "admin", viewport: { width: 1100, height: 760 } });
test.setTimeout(120_000);

type Measured = { viewLines: number; viewLineDistinctTops: number; overlayBoxes: Record<string, [number, number]>; lineNumbers: number; currentLineWidth: number; leftoverStyleData: number; mtkClasses: number };
type Fidelity = { matched: number; maxDev: number; within2px: number };

const EDITOR = "window.__lab.editor";
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function open(page: import("@playwright/test").Page, url: string, kind: string, extra = ""): Promise<void> {
  await page.addInitScript(POLICY_RECORDER);
  await page.goto(`${url}/testpages/editor?k=${kind}${extra}`);
  await page.waitForFunction(() => {
    const lab = (window as unknown as { __lab?: { ready: boolean; error: string | null } }).__lab;
    return lab !== undefined && (lab.ready || lab.error !== null);
  });
  const error = await page.evaluate(() => (window as unknown as { __lab: { error: string | null } }).__lab.error);
  expect(error, "the editor loaded").toBeNull();
  await sleep(1500); // lazy tokenisation, the worker, Unicode highlighting, the diff computation
}

const measure = (page: import("@playwright/test").Page, dom = `${EDITOR}.getDomNode()`) => page.evaluate<Measured>(`(${MEASURE})(${dom})`);
const fidelity = (page: import("@playwright/test").Page, editor = EDITOR) => page.evaluate<Fidelity>(`(${FIDELITY})(${editor})`);

async function expectClean(page: import("@playwright/test").Page, csp: { report(): string }, where: string): Promise<void> {
  expect(await pageViolations(page), `securitypolicyviolation events (${where})`).toEqual([]);
  expect(csp.report(), `violations (${where})`).toBe("");
  const policies = await page.evaluate(() => (window as unknown as { __policies: Array<{ name: string; ok: boolean }> }).__policies);
  expect(policies.map((p) => p.name), `policies (${where})`).toEqual(["lanekeeper-monaco"]);
  expect(policies.every((p) => p.ok)).toBe(true);
  const leftover = (await measure(page).catch(() => undefined))?.leftoverStyleData ?? 0;
  expect(leftover, `style data not applied (${where})`).toBe(0);
}

function expectLinesAtRightTop(f: Fidelity, where: string): void {
  expect(f.matched, `lines matched (${where})`).toBeGreaterThan(15);
  expect(f.within2px, `every line within 2 px of its place (${where}), worst ${f.maxDev}`).toBe(f.matched);
}

test.describe("the editor under the production policy", () => {
  test("loads with one policy, renders, and has zero violations", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor");
    const m = await measure(page);
    expect(m.viewLines).toBeGreaterThan(15);
    expect(m.viewLineDistinctTops, "every line has its own top").toBe(m.viewLines);
    expect(m.lineNumbers).toBeGreaterThan(15);
    expect(m.mtkClasses, "syntax tokens").toBeGreaterThanOrEqual(5);
    expectLinesAtRightTop(await fidelity(page), "initial");
    const asked = await page.evaluate(() => (window as unknown as { __lab: { askedNames: () => string[] } }).__lab.askedNames());
    expect(asked.length, "Monaco asked the hook for its policy names").toBeGreaterThanOrEqual(8);
    expect(asked, "the vendored DOMPurify must never ask for its policy").not.toContain("dompurify");
    await expectClean(page, csp, "load");
  });

  test("typing, selection, scrolling, find and the palette", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor");
    const box = await page.evaluate(() => {
      const r = (window as unknown as { __lab: { editor: { getDomNode(): HTMLElement } } }).__lab.editor.getDomNode().querySelector(".view-lines")?.getBoundingClientRect();
      return { x: (r?.x ?? 0) + 40, y: (r?.y ?? 0) + 30 };
    });
    await page.mouse.click(box.x, box.y);
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("zz: 1");
    await page.keyboard.press("Enter");
    await page.keyboard.type("yy: 2");
    await expect.poll(() => page.evaluate(`${EDITOR}.getModel().getLineContent(1)`)).toBe("zz: 1");
    expect(await page.evaluate(`[...${EDITOR}.getDomNode().querySelectorAll('.view-line')].some((l) => l.textContent.replace(/\\u00a0/g, ' ').includes('zz: 1'))`), "the DOM shows what was typed").toBe(true);
    expectLinesAtRightTop(await fidelity(page), "after typing");

    // The current line has a size (G0: 4 px wide without the rewrite, 823 with it).
    expect((await measure(page)).currentLineWidth).toBeGreaterThan(600);
    const guides = (await measure(page)).overlayBoxes["indentGuides"] ?? [0, 0];
    expect(guides[0], "indent guides exist").toBeGreaterThan(0);
    expect(guides[1], "and every one has a size").toBe(guides[0]);

    await page.keyboard.press("Control+A");
    await expect.poll(async () => ((await measure(page)).overlayBoxes["selectedText"] ?? [0, 0])[0]).toBeGreaterThan(10);
    const sel = (await measure(page)).overlayBoxes["selectedText"] ?? [0, 0];
    expect(sel[1], "every selection rectangle has a size").toBe(sel[0]);
    await page.keyboard.press("Control+Home");

    await page.evaluate(`${EDITOR}.setScrollTop(500)`);
    // Monaco draws a scroll on the next animation frame, not when setScrollTop returns: sampled in the same task, none of 29 lines is in
    // place and the worst is 500 px, the scroll itself (measured), and this spec failed that way about one run in ten. So the check waits
    // for the frame it is about (at most 5 s), and the assertion is unchanged: lines that never get their place still fail it.
    let drawn = await fidelity(page);
    for (let i = 0; i < 50 && !(drawn.matched > 15 && drawn.within2px === drawn.matched); i++) {
      await sleep(100);
      drawn = await fidelity(page);
    }
    const scrolled = await measure(page);
    expect(scrolled.viewLineDistinctTops, "lines inserted after a scroll have their top").toBe(scrolled.viewLines);
    expectLinesAtRightTop(drawn, "after scroll");
    expectLinesAtRightTop(await fidelity(page), "after scroll, again");
    await page.evaluate(`${EDITOR}.setScrollTop(0)`);

    await page.keyboard.press("Control+F");
    await expect(page.locator(".find-widget.visible")).toBeVisible();
    await page.keyboard.type("key1");
    await expect(page.locator(".find-widget .matchesCount")).toContainText("of");
    await page.keyboard.press("Escape");

    await page.keyboard.press("F1");
    await expect(page.locator(".quick-input-widget")).toBeVisible();
    await page.keyboard.press("Escape");
    await expectClean(page, csp, "typing, selection, scroll, find, palette");
  });

  test("a 4,000-line document scrolls with every line where it belongs, and a typing burst keeps them there", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor");
    await page.evaluate(`(() => { const lines = []; for (let i = 0; i < 4000; i++) lines.push(('k' + i + ': v' + i).padEnd(30 + (i % 7), ' ') + '# row ' + i); ${EDITOR}.setValue(lines.join('\\n')); })()`);
    await sleep(500);
    let worst = 0;
    for (let i = 0; i < 24; i++) {
      await page.evaluate((y) => (window as unknown as { __lab: { editor: { setScrollTop(y: number): void } } }).__lab.editor.setScrollTop(y), (i * 3571) % 70_000);
      await sleep(80);
      const f = await fidelity(page);
      expectLinesAtRightTop(f, `sweep ${i}`);
      worst = Math.max(worst, f.maxDev);
    }
    expect(worst).toBeLessThanOrEqual(2);
    await page.evaluate(`${EDITOR}.setScrollTop(0)`);
    await page.mouse.click(300, 300);
    await page.keyboard.press("Control+Home");
    for (let i = 0; i < 12; i++) await page.keyboard.type(`burst-line-${i}\n`, { delay: 0 });
    await sleep(300);
    expectLinesAtRightTop(await fidelity(page), "after a typing burst");
    await expectClean(page, csp, "4,000 lines");
  });

  test("context menu, go to line, folding, wrapping, mouse selection, multiple cursors, screen reader mode", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor");
    const origin = await page.evaluate(() => {
      const r = (window as unknown as { __lab: { editor: { getDomNode(): HTMLElement } } }).__lab.editor.getDomNode().querySelector(".view-lines")?.getBoundingClientRect();
      return { x: r?.x ?? 0, y: r?.y ?? 0 };
    });
    await page.mouse.click(origin.x + 80, origin.y + 40, { button: "right" });
    await expect(page.locator(".context-view .monaco-menu")).toBeVisible();
    await page.keyboard.press("Escape");

    await page.keyboard.press("Control+G");
    await expect(page.locator(".quick-input-widget")).toBeVisible();
    await page.keyboard.type("40");
    await page.keyboard.press("Enter");
    expect(await page.evaluate(`${EDITOR}.getPosition().lineNumber`)).toBe(40);

    await page.evaluate(`${EDITOR}.trigger('test', 'editor.foldAll', null)`);
    await sleep(300);
    await page.evaluate(`${EDITOR}.trigger('test', 'editor.unfoldAll', null)`);
    await page.evaluate(`${EDITOR}.updateOptions({ wordWrap: 'on' })`);
    await sleep(300);
    await page.evaluate(`${EDITOR}.updateOptions({ wordWrap: 'off' })`);
    await page.mouse.move(origin.x + 100, origin.y + 60);
    await page.mouse.down();
    await page.mouse.move(origin.x + 400, origin.y + 140, { steps: 8 });
    await page.mouse.up();
    await page.keyboard.press("Control+Alt+ArrowDown");
    await page.evaluate(`${EDITOR}.updateOptions({ accessibilitySupport: 'on' })`);
    await sleep(500);
    expectLinesAtRightTop(await fidelity(page), "after the interactions");
    await expectClean(page, csp, "interactions");
  });

  test("shows model text that contains style=\"...\" exactly as it is (the rewrite is tag-aware)", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor", "&c=1");
    await page.evaluate(`${EDITOR}.getModel().setValue(['a: <div style="color:red">hi</div>', "b: x style='top:5px' y", 'c: width style = "width: 3px" end', 'd: plain line', 'e: <span class="k" style="top:1px;left:2px">t</span> style="z"'].join('\\n'))`);
    await sleep(500);
    const shown = await page.evaluate<string[]>(`[...${EDITOR}.getDomNode().querySelectorAll('.view-lines > .view-line')].map((l) => l.textContent.replace(/\\u00a0/g, ' '))`);
    const model = await page.evaluate<string[]>(`${EDITOR}.getModel().getLinesContent()`);
    expect(shown.length).toBe(model.length);
    for (let i = 0; i < model.length; i++) expect(shown[i], `line ${i + 1}`).toBe(model[i]);
    await expectClean(page, csp, "text with style attributes");
  });

  test("marks invisible and ambiguous characters (S8)", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor");
    await expect
      .poll(() => page.evaluate(`${EDITOR}.getModel().getAllDecorations().filter((d) => /unicode-highlight/.test((d.options.className || '') + ' ' + (d.options.inlineClassName || ''))).length`), { timeout: 10_000 })
      .toBeGreaterThanOrEqual(2);
    const sizes = await page.evaluate<number[]>(`[...${EDITOR}.getDomNode().querySelectorAll('.unicode-highlight')].map((e) => Math.round(e.getBoundingClientRect().width))`);
    expect(sizes.length, "the marked characters are in view").toBeGreaterThanOrEqual(2);
    expect(Math.min(...sizes), "each box has a visible size (G0: 8 and 63 px; 2 px without the rewrite)").toBeGreaterThanOrEqual(6);
    await expectClean(page, csp, "unicode highlighting");
  });

  test("has no hover and no suggest: the contributions that render Markdown are not in the entry", async ({ page, harness, csp }) => {
    await open(page, harness.url, "editor");
    const box = await page.evaluate(() => {
      const r = (window as unknown as { __lab: { editor: { getDomNode(): HTMLElement } } }).__lab.editor.getDomNode().querySelector(".view-lines")?.getBoundingClientRect();
      return { x: (r?.x ?? 0) + 100, y: (r?.y ?? 0) + 60 };
    });
    // A provider that returns Markdown would make G0's hover and suggest widgets fail (a dompurify policy, DOMParser).
    await page.evaluate(`(() => {
      const m = window.__lab.monaco;
      m.languages.registerHoverProvider('yaml', { provideHover: () => ({ contents: [{ value: '**HOVER-MD** [a link](https://example.invalid/x)' }] }) });
      m.languages.registerCompletionItemProvider('yaml', { provideCompletionItems: (model, pos) => ({ suggestions: [{ label: 'alpha-item', kind: m.languages.CompletionItemKind.Property, insertText: 'alpha-item', range: { startLineNumber: pos.lineNumber, endLineNumber: pos.lineNumber, startColumn: 1, endColumn: 1 }, documentation: { value: '**DOC-MD**' } }] }) });
    })()`);
    await page.mouse.click(box.x, box.y);
    await page.mouse.move(box.x + 3, box.y);
    await sleep(900);
    await page.keyboard.press("Control+Space");
    await page.keyboard.press("Control+Space");
    await sleep(600);
    await expect(page.locator(".monaco-hover:visible")).toHaveCount(0);
    await expect(page.locator(".suggest-widget.visible")).toHaveCount(0);
    await expectClean(page, csp, "hover and suggest attempts");
  });
});

test.describe("the diff editor under the production policy", () => {
  for (const [mode, label] of [["", "side by side"], ["&m=inline", "inline"]] as const) {
    test(`renders ${label} with zero violations`, async ({ page, harness, csp }) => {
      await open(page, harness.url, "diff", mode);
      const changes = await page.evaluate(() => (window as unknown as { __lab: { diff: { getLineChanges(): unknown[] | null } } }).__lab.diff.getLineChanges()?.length ?? 0);
      expect(changes, "the worker computed the changes").toBe(3);
      const modified = await measure(page, "window.__lab.diff.getModifiedEditor().getDomNode()");
      expect(modified.viewLines).toBeGreaterThan(15);
      const deco = modified.overlayBoxes["decorationRanges"] ?? [0, 0];
      expect(deco[1], "every decoration box has a size").toBe(deco[0]);
      const lineBg = await page.evaluate(() => [...document.querySelectorAll(".line-insert, .line-delete, .char-insert, .char-delete")].filter((e) => e.getBoundingClientRect().width > 0).length);
      expect(lineBg, "the diff colours are drawn").toBeGreaterThan(0);
      expectLinesAtRightTop(await fidelity(page, "window.__lab.diff.getModifiedEditor()"), `diff ${label}`);
      await expectClean(page, csp, `diff ${label}`);
    });
  }
});

test.describe("languages", () => {
  test("YAML, JSON, XML, properties and plain text all tokenise, with no violation", async ({ page, harness, csp }) => {
    await open(page, harness.url, "langs");
    const expected: Record<string, number> = { yaml: 5, json: 6, xml: 6, ini: 6, plaintext: 1 };
    for (const [language, tokens] of Object.entries(expected)) {
      await page.evaluate((l) => (window as unknown as { __lab: { setLanguage(l: string): string } }).__lab.setLanguage(l), language);
      await expect.poll(async () => (await measure(page)).mtkClasses, { message: language }).toBeGreaterThanOrEqual(tokens);
    }
    await expectClean(page, csp, "five languages");
  });
});
