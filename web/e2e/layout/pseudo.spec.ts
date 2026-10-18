// German-length labels: every screen and state of the accessibility cases, with the interface's own words 40% longer (pseudo.ts), at 1024
// and 1280 wide, where the room is least. The same assertions as the layout matrix: no sideways page scroll, nothing cut off without an
// ellipsis, nothing covering other text, and every table still together.
import { expect, test } from "../fixtures/test";
import { settle } from "../a11y/axe.ts";
import { ALL_CASES } from "../a11y/cases.ts";
import { freezeDate, type Ctx } from "../a11y/helpers.ts";
import { auditLayout, describeReport, hasDefects, requestsQuiet } from "./audit.ts";
import { pseudoCount, pseudoLocalise } from "./pseudo.ts";
import { auditTables } from "./tables.ts";

test.use({ dist: process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1280, height: 800 },
] as const;

for (const c of ALL_CASES) {
  test.describe(c.id, () => {
    test.use({ scenario: c.scenario });

    test("with every label 40% longer", async ({ page, harness, csp }) => {
      test.setTimeout(120_000);
      const ctx: Ctx = { page, harness, csp, scheme: "light" };
      await freezeDate(page);
      await c.before?.(ctx);
      await page.goto(`${harness.url}${c.url}`);
      await expect(page.locator("h1").first()).toBeVisible();
      // The steps of a case find their buttons by name, so the words are stretched once the case is in its state (and, from then on, whatever
      // is drawn afterwards).
      await c.open?.(ctx);
      // The last answer has come (a list of 1,000 is eleven requests): what is measured is the screen as it ends up, not half way.
      await requestsQuiet(page, harness);
      await pseudoLocalise(page);
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await settle(page, 300);
        const label = `${c.id} (labels +40%) @${size.width}x${size.height}`;
        expect.soft(await pseudoCount(page), `${label}: the words were stretched`).toBeGreaterThan(0);
        const report = await auditLayout(page, { skip: ".monaco-editor" });
        expect.soft(hasDefects(report), describeReport(label, report)).toBe(false);
        const tables = await auditTables(page);
        expect.soft(tables.defects, `${label}: ${JSON.stringify(tables.defects)}`).toEqual([]);
      }
    });
  });
}
