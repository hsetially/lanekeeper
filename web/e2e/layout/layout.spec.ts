// The layout matrix (plan, "Visual regression ... Layout matrix", widened): every screen and every state of the accessibility cases at
// 1024, 1280, 1440 and 2560 wide (states with a dialog, a menu or a drawer open at 1024 and 1440), in light. No screenshots: every
// assertion is a box or a scroll size read from the browser (audit.ts, tables.ts).
//
//   - the page never scrolls sideways (<main> is the page's scroller, and neither it nor the document does);
//   - no text or control is cut off without an ellipsis, none reaches past the content box, no text covers other text;
//   - every table keeps its cells under its headers, its header at the top and its columns together, and scrolls INSIDE its container;
//   - at 2560 the content stops at the design's cap of 1680 px.
import { expect, test } from "../fixtures/test";
import { settle } from "../a11y/axe.ts";
import { ALL_CASES } from "../a11y/cases.ts";
import { freezeDate, type Ctx } from "../a11y/helpers.ts";
import { auditLayout, describeReport, hasDefects, requestsQuiet } from "./audit.ts";
import { auditTables } from "./tables.ts";

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
  { width: 2560, height: 1440 },
] as const;
const STATE_SIZES = [SIZES[0], SIZES[2]] as const;
const CONTENT_CAP = 1680;

test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

for (const c of ALL_CASES) {
  test.describe(c.id, () => {
    test.use({ scenario: c.scenario });

    test("fits: no sideways page scroll, nothing cut off or covered, tables stay together", async ({ page, harness, csp }) => {
      test.setTimeout(120_000);
      const ctx: Ctx = { page, harness, csp, scheme: "light" };
      await freezeDate(page);
      await c.before?.(ctx);
      await page.goto(`${harness.url}${c.url}`);
      await expect(page.locator("h1").first()).toBeVisible();
      await c.open?.(ctx);
      // The last answer has come (a list of 1,000 is eleven requests): what is measured is the screen as it ends up, not half way.
      await requestsQuiet(page, harness);

      for (const size of c.open ? STATE_SIZES : SIZES) {
        await page.setViewportSize(size);
        await settle(page, 250);
        const label = `${c.id} @${size.width}x${size.height}`;
        const report = await auditLayout(page, { skip: ".monaco-editor" });
        expect.soft(hasDefects(report), describeReport(label, report)).toBe(false);
        const tables = await auditTables(page);
        expect.soft(tables.defects, `${label}: ${JSON.stringify(tables.defects)}`).toEqual([]);
        if (size.width >= 2560 && !c.noChrome) {
          const width = await page.evaluate(() => document.querySelector("main > div")?.getBoundingClientRect().width ?? 0);
          expect.soft(width, `${label}: the content is wider than the design's cap of ${CONTENT_CAP}px`).toBeLessThanOrEqual(CONTENT_CAP + 1);
        }
      }
      expect(csp.report()).toBe("");
    });
  });
}
