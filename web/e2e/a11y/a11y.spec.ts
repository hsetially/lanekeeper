// Accessibility of every screen and state, in the real layout (design/13; plan, "Keyboard, accessibility, time and browsers"):
// axe-core on the page as a person sees it, in light and dark, at 1440x900 and at 1024x768, with the production
// Content-Security-Policy on (the shared test fixture fails a case on any CSP, Trusted Types or page error).
//
// Per case and per theme the spec checks, at both sizes: exactly one <h1>, exactly one <main>, the landmarks the shell draws, and
// axe with zero violations and no unexplained "incomplete" (see axe.ts). The cases are in cases*.ts.
import { expect, pageViolations, test } from "../fixtures/test";
import { TALLY, expectAxeClean, settle } from "./axe.ts";
import { ALL_CASES } from "./cases.ts";
import { freezeDate, type Ctx, type Scheme } from "./helpers.ts";

// What the spec decided where axe could not (resolve.ts), so that a run says how much was decided that way.
test.afterAll(() => {
  console.log(`a11y tally: ${JSON.stringify(TALLY)}`);
});

const SIZES = [
  { width: 1440, height: 900 },
  { width: 1024, height: 768 },
] as const;

// The build the harness serves: `dist` by default. LK_DIST names another (a throwaway build made while another run uses `dist`).
test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist" });

for (const scheme of ["light", "dark"] as const satisfies readonly Scheme[]) {
  test.describe(`axe, ${scheme}`, () => {
    test.use({ colorScheme: scheme });

    for (const c of ALL_CASES) {
      test.describe(c.id, () => {
        test.use({ scenario: c.scenario });

        test("one h1, one main, and no axe finding at 1440x900 and at 1024x768", async ({ page, harness, csp }) => {
          test.setTimeout(90_000);
          const ctx: Ctx = { page, harness, csp, scheme };
          // The clock of the design's data (the frozen "now" of the visual runs), so that "ends in 5 days" and "4 min ago" read as drawn.
          await freezeDate(page);
          await c.before?.(ctx);
          await page.goto(`${harness.url}${c.url}`);
          await expect(page.locator("h1").first()).toBeVisible();
          if (c.h1 !== undefined) await expect(page.locator("h1")).toHaveText(c.h1);
          await c.open?.(ctx);
          await expect(page.locator("html")).toHaveClass(scheme === "dark" ? /dark/ : /^(?!.*dark)/);

          // LK_A11Y_DRY=1 only brings each page into its state (to check a case), without axe.
          if (process.env["LK_A11Y_DRY"]) return;
          for (const size of SIZES) {
            await page.setViewportSize(size);
            await settle(page, 250);
            const label = `${scheme} ${c.id} @${size.width}x${size.height}`;
            const counts = await page.evaluate(() => ({ h1: document.querySelectorAll("h1").length, main: document.querySelectorAll("main, [role='main']").length }));
            expect.soft(counts.h1, `${label}: the number of <h1>`).toBe(1);
            expect.soft(counts.main, `${label}: the number of main landmarks`).toBe(1);
            await expectAxeClean(page, label);
          }
          expect(await pageViolations(page)).toEqual([]);
          expect(csp.report()).toBe("");
        });
      });
    }
  });
}
