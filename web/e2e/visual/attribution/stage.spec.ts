// The stage the pieces of the attribution chip are photographed on (stage.ts) must draw the same pixels whatever the machine's text
// settings are. The sandbox draws text with grey anti-aliasing; the runner of the visual-baselines job draws it with sub-pixel (LCD)
// anti-aliasing, because its font configuration asks for it (`rgba=rgb`). Whether a piece's text is LCD or grey is not the piece's, it is
// a decision of the compositor for the layer the text is in, and in the app's page, with the copies of the chips' buttons in the stage, that
// decision came out grey for "meter Medium" and "meter High" where the design's were LCD (hiding the four buttons made the app's LCD too; the
// rule inside the compositor was not traced). The two differ along the edges of the stems of "g" and "h" in "High", 7 of 1134 pixels,
// 0.617% against the 0.5% limit: the failure of the bootstrap run.
//
// This test photographs the pieces of File 1j in two browsers, one with the default font configuration and one whose configuration asks
// for LCD text, and says that each piece is the same picture in both, in the design and in the app. A stage whose text depends on the
// machine fails it; the comparison with the design (elements.spec.ts) is the same on every machine only if this holds. It does not touch
// a tolerance: nothing is compared with a margin here, the pictures are the same bytes or the test fails.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, type Browser, type Page } from "@playwright/test";
import { chromiumPath, launchArgs } from "../../harness/browser.ts";
import type { Harness } from "../../harness/serve.ts";
import { expect, test } from "../../fixtures/test";
import { openApp } from "../search/kit";
import { HISTORY } from "./frames";
import { appPieces, designPieces, umd } from "./elements";
import { appStage, designPieces_ } from "./stage";

test.use({ dist: process.env["LK_DIST"] ?? "dist" });

const IN_CI = Boolean(process.env["CI"]);
const FILE_VIEW = "Lanekeeper File View.dc.html";
const PIECES = ["words Marco Diaz · via Lanekeeper", "meter Certain", "words NFS client (no sync running)", "meter Medium", "meter High"] as const;

/** A font configuration that asks for sub-pixel (LCD) text: the system's own, with the RGB sub-pixel order added. */
const LCD_FONTS = `<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <include ignore_missing="yes">/etc/fonts/fonts.conf</include>
  <match target="font">
    <edit name="rgba" mode="assign"><const>rgb</const></edit>
    <edit name="lcdfilter" mode="assign"><const>lcddefault</const></edit>
    <edit name="antialias" mode="assign"><bool>true</bool></edit>
  </match>
</fontconfig>
`;

/** How many pixels of a picture carry colour (red, green and blue apart by more than 6): text drawn with sub-pixel anti-aliasing does. */
async function coloured(page: Page, png: Buffer): Promise<number> {
  return page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext("2d");
    if (!context) throw new Error("no 2d context");
    context.drawImage(bitmap, 0, 0);
    const data = context.getImageData(0, 0, bitmap.width, bitmap.height).data;
    let n = 0;
    for (let i = 0; i < data.length; i += 4) if (Math.max(data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0) - Math.min(data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0) > 6) n += 1;
    return n;
  }, png.toString("base64"));
}

async function photographs(browser: Browser, page: Page, harness: Harness, what: string): Promise<{ design: Map<string, Buffer>; app: Map<string, Buffer> }> {
  const size = { width: 1440, height: 900 };
  await openApp({ board: "states", frame: "E", kind: "app", title: "stage", why: "stage", scenario: "design-file-core", path: HISTORY }, { page, harness, size }, "light");
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(size.width - 1, size.height - 1);
  await page.waitForTimeout(250);
  const design = await designPieces(browser, FILE_VIEW, "1j", "light", what, designPieces_);
  const app = await appPieces(page, appStage("aside[aria-label='History']"));
  return { design, app };
}

test("the pieces are the same pictures whatever the machine's font settings (LCD text or grey)", async ({ page, harness, browser }) => {
  test.setTimeout(240_000);
  if (!umd.ok) {
    if (IN_CI) throw new Error(`NOT RUN is not allowed in CI: ${umd.reason ?? "no React files"}`);
    test.skip(true, `NOT RUN: ${umd.reason ?? "no React files"}`);
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "lk-lcd-fonts-"));
  const conf = join(dir, "fonts.conf");
  writeFileSync(conf, LCD_FONTS);
  const lcd = await chromium.launch({ executablePath: chromiumPath(), args: launchArgs(), env: { ...process.env, FONTCONFIG_FILE: conf } });
  try {
    // The control: this machine can draw LCD text with that configuration. If it cannot, the test would compare two identical renderings
    // and prove nothing, so it is not run (and in CI, which is Linux, it fails).
    const probe = await (await lcd.newContext({ viewport: { width: 200, height: 60 } })).newPage();
    await probe.setContent("<body style='margin:0;background:#fff;color:#000;font:16px sans-serif'><p style='margin:8px'>High agh</p></body>");
    const control = await coloured(probe, await probe.screenshot());
    await probe.context().close();
    if (control === 0) {
      if (IN_CI) throw new Error("the browser with the LCD font configuration drew grey text: the test cannot tell the two renderings apart");
      test.skip(true, "NOT RUN: this machine does not draw LCD text with a font configuration that asks for it");
      return;
    }

    const plain = await photographs(browser, page, harness, "plain");
    const context = await lcd.newContext({ viewport: { width: 1440, height: 900 }, timezoneId: "UTC", locale: "en-GB", colorScheme: "light", reducedMotion: "reduce" });
    const lcdPage = await context.newPage();
    const wide = await photographs(lcd, lcdPage, harness, "lcd");
    await context.close();

    const different: string[] = [];
    for (const name of PIECES) {
      for (const side of ["design", "app"] as const) {
        const a = plain[side].get(name);
        const b = wide[side].get(name);
        if (a === undefined || b === undefined) different.push(`${side} ${name}: not photographed`);
        else if (!a.equals(b)) different.push(`${side} ${name}`);
      }
    }
    expect(different, "pieces that are not the same picture with LCD text on (the stage's text depends on the machine's font settings)").toEqual([]);
  } finally {
    await lcd.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
