// The frame of the app at the four widths: the sidebar expanded and collapsed, the header, the one scroller, and the content cap.
// Boxes only. The numbers are the design's: a 224 px sidebar (a 48 px rail when collapsed), a 44 px top bar, content up to 1680 px.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { settle } from "../a11y/axe.ts";
import { SIDEBAR, freezeDate } from "../a11y/helpers.ts";
import { auditLayout, describeReport, hasDefects } from "./audit.ts";

test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
  { width: 2560, height: 1440 },
] as const;

type Box = { left: number; top: number; right: number; bottom: number; width: number; height: number };
type Frame = { sidebar: Box; header: Box; main: Box; content: Box | null; viewport: { width: number; height: number } };

async function frame(page: Page): Promise<Frame> {
  return page.evaluate(() => {
    const plain = (el: Element | null): Box => {
      const r = el ? el.getBoundingClientRect() : new DOMRect();
      return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
    };
    return {
      sidebar: plain(document.querySelector("aside")),
      header: plain(document.querySelector("header")),
      main: plain(document.querySelector("main")),
      content: document.querySelector("main > div") ? plain(document.querySelector("main > div")) : null,
      viewport: { width: window.innerWidth, height: window.innerHeight },
    };
  });
}

const near = (a: number, b: number, tolerance = 1): boolean => Math.abs(a - b) <= tolerance;

test.describe("the frame", () => {
  test.use({ scenario: "design-swimlanes-six" });

  for (const collapsed of [false, true]) {
    test(`${collapsed ? "collapsed" : "expanded"} sidebar: its width, the header, the main region and the content cap at every width`, async ({ page, harness }) => {
      test.setTimeout(120_000);
      await freezeDate(page);
      await page.goto(`${harness.url}/swimlanes`);
      await expect(page.locator("h1").first()).toBeVisible();
      if (collapsed) {
        await page.keyboard.press("ControlOrMeta+b");
        await expect(SIDEBAR(page)).toHaveAttribute("data-collapsed", "true");
      }
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await settle(page, 200);
        const f = await frame(page);
        const label = `${collapsed ? "collapsed" : "expanded"} @${size.width}x${size.height}`;
        const railWidth = collapsed ? 48 : 224;
        expect.soft(near(f.sidebar.width, railWidth + 1), `${label}: the sidebar is ${f.sidebar.width}px wide (${railWidth} and its border)`).toBe(true);
        expect.soft(near(f.sidebar.top, 0) && near(f.sidebar.height, size.height), `${label}: the sidebar fills the height (${f.sidebar.top}, ${f.sidebar.height})`).toBe(true);
        expect.soft(near(f.header.left, f.sidebar.right) && near(f.header.right, size.width) && near(f.header.top, 0), `${label}: the header sits beside the sidebar and spans the rest (${f.header.left}..${f.header.right})`).toBe(true);
        expect.soft(near(f.header.height, 44 + 1), `${label}: the header is ${f.header.height}px high (44 and its border)`).toBe(true);
        expect.soft(near(f.main.left, f.sidebar.right) && near(f.main.right, size.width), `${label}: main fills the width beside the sidebar (${f.main.left}..${f.main.right})`).toBe(true);
        expect.soft(near(f.main.top, f.header.bottom, 70), `${label}: main is under the header (the live banner may sit between)`).toBe(true);
        expect.soft(f.content === null || f.content.width <= 1681, `${label}: the content is ${f.content?.width}px wide, past the cap of 1680`).toBe(true);
        if (size.width === 2560) expect.soft(f.content !== null && f.content.width >= 1500, `${label}: at 2560 the content uses the cap (${f.content?.width}px), it is not squeezed`).toBe(true);
        if (size.width === 2560 && f.content) expect.soft(near(f.content.left - f.main.left, f.main.right - f.content.right, 2), `${label}: the capped content is centred in main`).toBe(true);
        const report = await auditLayout(page);
        expect.soft(hasDefects(report), describeReport(label, report)).toBe(false);
      }
    });
  }

  test("the header's controls fit inside the header at 1024 wide, expanded and collapsed, and do not overlap each other", async ({ page, harness }) => {
    await freezeDate(page);
    await page.setViewportSize({ width: 1024, height: 768 });
    await page.goto(`${harness.url}/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml`);
    await expect(page.locator("h1").first()).toBeVisible();
    for (const collapsed of [false, true]) {
      if (collapsed) await page.keyboard.press("ControlOrMeta+b");
      await settle(page, 200);
      const boxes = await page.evaluate(() => {
        const header = document.querySelector("header");
        if (header === null) return null;
        const h = header.getBoundingClientRect();
        const items = [...header.querySelectorAll("button, a[href], [data-slot='live-indicator'], nav")].map((el) => {
          const r = el.getBoundingClientRect();
          return { name: `${el.tagName.toLowerCase()} "${(el.getAttribute("aria-label") ?? el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 24)}"`, left: r.left, right: r.right, top: r.top, bottom: r.bottom };
        });
        return { header: { left: h.left, right: h.right, top: h.top, bottom: h.bottom }, items: items.filter((i) => i.right > i.left) };
      });
      expect(boxes).not.toBeNull();
      if (boxes === null) continue;
      for (const item of boxes.items) {
        expect(item.left, `${item.name} (collapsed: ${collapsed}) starts inside the header`).toBeGreaterThanOrEqual(boxes.header.left - 1);
        expect(item.right, `${item.name} (collapsed: ${collapsed}) ends inside the header`).toBeLessThanOrEqual(boxes.header.right + 1);
      }
      // Two controls that are not nested do not cover each other.
      const flat = boxes.items.filter((a) => !a.name.startsWith("nav"));
      for (let i = 0; i < flat.length; i++) {
        for (let j = i + 1; j < flat.length; j++) {
          const a = flat[i];
          const b = flat[j];
          if (a === undefined || b === undefined) continue;
          const overlap = Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1;
          expect(overlap, `${a.name} and ${b.name} overlap in the header (collapsed: ${collapsed})`).toBe(false);
        }
      }
    }
  });
});

test.describe("sticky parts stay put", () => {
  test.use({ scenario: "design-file-core", viewport: { width: 1280, height: 600 } });

  test("the sidebar and the top bar stay where they are while the content scrolls", async ({ page, harness }) => {
    await freezeDate(page);
    await page.goto(`${harness.url}/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml`);
    await expect(page.locator("h1").first()).toBeVisible();
    await settle(page);
    const before = await frame(page);
    // The one scroller of a screen is main, or a pane inside it: scroll every scroller that can.
    const scrolled = await page.evaluate(() => {
      let n = 0;
      for (const el of document.querySelectorAll("main, main *")) {
        const cs = getComputedStyle(el);
        if ((cs.overflowY === "auto" || cs.overflowY === "scroll") && el.scrollHeight > el.clientHeight + 50) {
          el.scrollTop = 400;
          n += 1;
        }
      }
      return n;
    });
    expect(scrolled, "something on the file screen scrolls").toBeGreaterThan(0);
    await settle(page, 200);
    const after = await frame(page);
    expect(after.sidebar.top).toBe(before.sidebar.top);
    expect(after.header.top).toBe(before.header.top);
    expect(after.sidebar.height).toBe(before.sidebar.height);
    expect(await page.evaluate(() => window.scrollY + document.documentElement.scrollTop)).toBe(0);
  });
});
