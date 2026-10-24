// The keyboard on the attribution chip (T5, slice A), in a real browser: the chip is a button, it is reached with Tab, Enter and
// Space open the evidence behind it, Escape closes it and focus goes back to the chip. The chip adds no row to the keymap (it is a
// button with a popover), so no test here names a row.
//
// The file page of `design-attribution-history` has the chip in two places: the head of the file ("Changed by") and every version of
// the History panel. The hostile scenario has text a VM or a cluster could send (a right-to-left override, markup, a script).
import type { Locator, Page } from "@playwright/test";
import { pageViolations } from "../fixtures/test";
import { DIST, expect, keyboardOnly, pathOf, ready, test } from "./support";

test.use({ dist: DIST });

const FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml&panel=history";
const evidence = (page: Page) => page.getByRole("dialog", { name: "Attribution evidence" });
const headChip = (page: Page): Locator => page.locator("[data-slot=file-changed-by] [data-slot=attribution] button");
const historyChips = (page: Page): Locator => page.getByRole("complementary", { name: "History" }).locator("[data-slot=attribution] button");

async function openFile(page: Page, origin: string, versions: number): Promise<void> {
  await ready(page, origin, FILE, "tx-infinity-core.yml");
  // The head's chip and the history's rows are drawn once the file and its history are there; a key pressed before is lost.
  await expect(headChip(page)).toBeVisible();
  await expect(historyChips(page)).toHaveCount(versions);
}

test.describe("the chip on the file page", () => {
  test.use({ scenario: "design-attribution-history" });

  test("Tab reaches the head's chip after the file's own controls, Enter opens the evidence and Escape closes it with focus back on the chip", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await openFile(page, harness.url, 9);
    // The chip is the stop that follows the copy buttons of the head: nothing between them and it is skipped.
    await page.getByRole("button", { name: "Copy link to this file" }).focus();
    await page.keyboard.press("Tab");
    await expect(headChip(page)).toBeFocused();
    await expect(headChip(page)).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toBeVisible();
    await expect(evidence(page)).toBeFocused();
    await expect(headChip(page)).toHaveAttribute("aria-expanded", "true");
    // What the hub recorded, as definitions: the source, how sure, who, and each piece of evidence.
    await expect(evidence(page)).toContainText("Edited on the NFS VM");
    await expect(evidence(page)).toContainText("dana.okoye");
    await expect(evidence(page)).toContainText("Executable");
    await expect(evidence(page)).toContainText("Attribution is upgraded when stronger evidence turns up, and never downgraded.");
    await page.keyboard.press("Escape");
    await expect(evidence(page)).toHaveCount(0);
    await expect(headChip(page)).toBeFocused();
    await expect(headChip(page)).toHaveAttribute("aria-expanded", "false");
    // Escape on the chip when nothing is open closes nothing else: the page stays where it is.
    await page.keyboard.press("Escape");
    expect(pathOf(page)).toBe("/swimlanes/sitb/files");
    // Space opens it too, and Escape gives focus back again.
    await page.keyboard.press("Space");
    await expect(evidence(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(evidence(page)).toHaveCount(0);
    await expect(headChip(page)).toBeFocused();
    // Shift+Tab goes back to the stop before it: the chip is one stop.
    await page.keyboard.press("Shift+Tab");
    await expect(page.getByRole("button", { name: "Copy link to this file" })).toBeFocused();
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("with the evidence open Tab and Shift+Tab stay in it (it holds nothing to reach), and Escape is the way out", async ({ page, harness }) => {
    await openFile(page, harness.url, 9);
    await headChip(page).focus();
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(evidence(page)).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(evidence(page)).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(headChip(page)).toBeFocused();
  });

  test("every version in the History panel has its chip, each is a tab stop, and only one evidence is open at a time", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openFile(page, harness.url, 9);
    const chips = historyChips(page);
    // The first version has no known author: it says so, with no confidence level (the unknown source carries none).
    await chips.nth(0).focus();
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toContainText("Unknown");
    await expect(evidence(page)).toContainText("Not identified");
    await expect(evidence(page)).toContainText("Nothing identified who changed the file.");
    await page.keyboard.press("Escape");
    await expect(chips.nth(0)).toBeFocused();
    // A version is four stops: its chip, its hash and the hash's copy button, and Revert. The next version's chip follows, in the
    // order of the list; the signed-in person is "You".
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(page.locator(":focus")).toHaveAccessibleName("Copy hash");
    await page.keyboard.press("Tab");
    await expect(page.locator(":focus")).toHaveAccessibleName(/^Revert to /);
    await page.keyboard.press("Tab");
    await expect(chips.nth(1)).toBeFocused();
    await expect(chips.nth(1)).toContainText("You");
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toHaveCount(1);
    await expect(evidence(page)).toContainText("Via Lanekeeper");
    await page.keyboard.press("Escape");
    await expect(chips.nth(1)).toBeFocused();
    // A user the Admin's list does not know is the start of the id, never a made-up name.
    await chips.nth(8).focus();
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toContainText("Lanekeeper user cccccccc");
    await page.keyboard.press("Escape");
    await expect(chips.nth(8)).toBeFocused();
    expect(await pointer()).toEqual([]);
  });
});

test.describe("a chip with text the hub did not write", () => {
  test.use({ scenario: "design-attribution-hostile" });

  test("Enter opens evidence that shows the text as text: the hidden characters are visible, markup stays markup, nothing runs and no page appears", async ({ page, harness, csp }) => {
    const dialogs: string[] = [];
    page.on("dialog", (dialog) => {
      dialogs.push(dialog.message());
      void dialog.dismiss();
    });
    await openFile(page, harness.url, 4);
    const chips = historyChips(page);
    // 0: a right-to-left override in the login, a zero-width joiner in the executable, an isolate in the login identity.
    await chips.nth(0).focus();
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toContainText("invoice<U+202E>gnp.exe");
    await expect(evidence(page)).toContainText("v<U+200D>i");
    await expect(evidence(page)).toContainText("do<U+2066>kye");
    await page.keyboard.press("Escape");
    await expect(chips.nth(0)).toBeFocused();
    // 1: a zero-width space in the job's name and a right-to-left override in its uid.
    await chips.nth(1).focus();
    await page.keyboard.press("Space");
    await expect(evidence(page)).toContainText("csp<U+200B>-tenant-data-sit7");
    await expect(evidence(page)).toContainText("u<U+202E>1");
    await page.keyboard.press("Escape");
    // 2: markup as a login and a script as the executable.
    await expect(chips.nth(2)).toContainText("<img src=x onerror=alert(1)>");
    await chips.nth(2).focus();
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toContainText("<script>alert(1)</script>");
    expect(await page.locator("img, [onerror]").count(), "no element was made from the text").toBe(0);
    expect(await evidence(page).locator("script").count(), "no script was made from the text").toBe(0);
    await page.keyboard.press("Escape");
    // 3: a login of 256 characters stays inside the box: the popover does not scroll the page sideways.
    await chips.nth(3).focus();
    await page.keyboard.press("Enter");
    await expect(evidence(page)).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    await page.keyboard.press("Escape");
    expect(dialogs).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });
});
