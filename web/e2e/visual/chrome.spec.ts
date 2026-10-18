// Tier 1, chrome only: the app shell's sidebar and header against the design's Shell frames 1a (light) and 1b (dark), pixel by
// pixel (threshold 0.2 per pixel, 0.5% of the pixels of a region), at 1440x900, in the browser the app runs in.
//
// Chrome only means the two regions the shell draws itself: the sidebar (224 px by 900) and the top bar (the rest of the first
// 44 px). The page below the top bar is a placeholder until the slices are built, and their regions are compared by the slices'
// own specs. The data the chrome shows (the six swimlanes, 11 findings, 6 approvals, Priya Shah) is the Shell page's, from the
// `design-shell-admin` scenario, with the clock frozen.
//
// The reference is rendered (e2e/visual/render-references.mjs) and needs the two React 18.3.1 UMD files that the design's
// runtime asks unpkg for: set LK_REACT18_DIR to a directory holding them (`npm pack react@18.3.1 react-dom@18.3.1`, then the two
// `umd/*.production.min.js`). Without it the test is skipped and says "NOT RUN" with the reason: it is never a pass, and in CI (`CI` set)
// it FAILS instead; the workflow fetches the files from the npm registry (W30).
//
// LK_VISUAL_OUT=<dir> writes the reference, the app, and the difference of each region there, and the ratios as JSON.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { compareImages } from "./compare.mjs";
import { FROZEN_NOW, openReference, reactUmd, startReferenceServer } from "./render-references.mjs";

const VIEWPORT = { width: 1440, height: 900 };
const REGIONS = {
  sidebar: { x: 0, y: 0, width: 224, height: 900 },
  header: { x: 224, y: 0, width: 1216, height: 44 },
} as const;
const THRESHOLD = 0.2;
const MAX_RATIO = 0.005;

// What the design draws and the app does not, each a row of design/DEVIATIONS.md: the notifications bell (the contract has no
// notifications to put behind it). Removed from the reference, so that the cluster to its left sits where the app's does.
const OMITTED = ["button[aria-label^='Notifications']"];

const umd = reactUmd();
const OUT = process.env["LK_VISUAL_OUT"];

/** The route the Shell page's default frame is on (page "file"): a file of a swimlane, which gives the breadcrumb and the switcher. */
const APP_PATH = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit1.yml";

async function appShot(page: Page, region: { x: number; y: number; width: number; height: number }): Promise<Buffer> {
  return page.screenshot({ clip: region, animations: "disabled", caret: "hide" });
}

test.describe("the chrome against the Shell frames", () => {
  test.use({ scenario: "design-shell-admin", viewport: VIEWPORT });
  // Never a pass without a reference: skipped and said so where the files are not to be had (a sandbox that cannot reach unpkg), and
  // a failure where CI runs, which supplies them (.github/workflows/web-e2e.yml).
  test.beforeEach(() => {
    if (umd.ok) return;
    if (process.env["CI"]) throw new Error(`the Shell reference cannot be rendered in CI: ${umd.reason}`);
    test.skip(true, `NOT RUN: ${umd.reason}`);
  });

  for (const [frame, theme] of [["1a", "light"], ["1b", "dark"]] as const) {
    test(`frame ${frame} (${theme}): sidebar and header`, async ({ page, harness, browser }) => {
      test.setTimeout(120_000);
      if (!umd.ok) return;
      const server = await startReferenceServer({ themeFor: () => theme });
      const context = await browser.newContext({ viewport: VIEWPORT, timezoneId: "UTC", locale: "en-GB", colorScheme: theme, reducedMotion: "reduce" });
      try {
        const ref = await context.newPage();
        await openReference(ref, server, umd, { file: "Lanekeeper Shell.dc.html", theme, viewport: VIEWPORT, omit: OMITTED });
        const references = { sidebar: await ref.screenshot({ clip: REGIONS.sidebar }), header: await ref.screenshot({ clip: REGIONS.header }) };

        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        await page.clock.setFixedTime(FROZEN_NOW);
        await page.goto(`${harness.url}${APP_PATH}`);
        await expect(page.getByRole("complementary", { name: "Primary" })).toBeVisible();
        await expect(page.locator("[data-slot='live-indicator']")).toHaveAttribute("data-status", "live");
        // The first fetches are done and the fonts are in.
        await page.evaluate(() => document.fonts.ready);
        await page.mouse.move(VIEWPORT.width - 1, VIEWPORT.height - 1);
        await page.waitForTimeout(300);
        const apps = { sidebar: await appShot(page, REGIONS.sidebar), header: await appShot(page, REGIONS.header) };

        const ratios: Record<string, number> = {};
        for (const name of ["sidebar", "header"] as const) {
          const result = await compareImages(ref, references[name], apps[name], { threshold: THRESHOLD });
          ratios[name] = result.ratio;
          if (OUT) {
            mkdirSync(OUT, { recursive: true });
            writeFileSync(join(OUT, `${frame}-${name}-reference.png`), references[name]);
            writeFileSync(join(OUT, `${frame}-${name}-app.png`), apps[name]);
            writeFileSync(join(OUT, `${frame}-${name}-diff.png`), Buffer.from(result.diffPng.split(",")[1] ?? "", "base64"));
          }
        }
        if (OUT) writeFileSync(join(OUT, `${frame}-ratios.json`), JSON.stringify({ frame, theme, threshold: THRESHOLD, max: MAX_RATIO, ratios }, null, 2));
        test.info().annotations.push({ type: "ratio", description: JSON.stringify(ratios) });
        for (const name of ["sidebar", "header"] as const) expect(ratios[name], `${frame} ${name}: share of pixels that differ`).toBeLessThanOrEqual(MAX_RATIO);
      } finally {
        await context.close();
        await server.close();
      }
    });
  }
});

// The comparison would pass for any two images if it were broken: it must see no difference between an image and itself, and a
// difference between the sidebar and the same region shifted by two pixels. This one needs no reference, so it always runs.
test.describe("the comparison itself", () => {
  test.use({ scenario: "design-shell-admin", viewport: VIEWPORT });

  test("finds nothing between an image and itself, and finds a two pixel shift of the sidebar", async ({ page, harness }) => {
    await page.goto(`${harness.url}${APP_PATH}`);
    await expect(page.getByRole("complementary", { name: "Primary" })).toBeVisible();
    await page.mouse.move(VIEWPORT.width - 1, VIEWPORT.height - 1);
    await page.waitForTimeout(300);
    const region = { ...REGIONS.sidebar, height: 898 };
    const a = await appShot(page, region);
    const shifted = await appShot(page, { ...region, y: 2 });
    expect((await compareImages(page, a, a)).diff).toBe(0);
    expect((await compareImages(page, a, shifted)).ratio).toBeGreaterThan(MAX_RATIO);
  });
});
