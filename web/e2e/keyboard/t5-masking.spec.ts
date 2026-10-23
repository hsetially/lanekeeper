// The keyboard on a masked value and its Reveal (T5, slice C), in a real browser: Tab reaches the Reveal button, Enter or Space opens
// the dialog, the reason field has focus, Tab stays inside, Escape closes it and focus goes back to Reveal; once the value is shown
// Hide has focus, and hiding it sends focus back to Reveal. The feature adds no row to the keymap (a button and a dialog), so no
// test here names a row; the dialog's rows are the shell's and are tested in keymap.spec.ts.
//
// The value is the design's made-up one (`REVEALED_VALUE`), in no file of the repository other than the mocks.
import type { Page } from "@playwright/test";
import { pageViolations } from "../fixtures/test";
import { REVEALED_VALUE, REVEAL_PATH } from "../../src/mocks/design/masking/data.ts";
import type { Harness } from "../harness/serve.ts";
import { DIST, expect, focused, keyboardOnly, ready, test } from "./support";

test.use({ dist: DIST });

const FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const SETTING = "txInfinityOptions.remote-itm-teller.clientSecret";
const reveal = (page: Page) => page.getByRole("button", { name: `Reveal value of ${SETTING}` });
const hide = (page: Page) => page.getByRole("button", { name: `Hide value of ${SETTING}` });
const dialog = (page: Page) => page.getByRole("alertdialog", { name: "Reveal masked value?" });
const reason = (page: Page) => dialog(page).getByRole("textbox");
const text = (page: Page) => page.getByRole("table", { name: "tx-infinity-core-sit7.yml" });
const reveals = (harness: Harness) => harness.requests().filter((r) => r.method === "POST" && r.path === REVEAL_PATH.replace("{id}", "sitb"));

async function openFile(page: Page, origin: string): Promise<void> {
  await ready(page, origin, FILE, "tx-infinity-core.yml");
  await expect(reveal(page)).toBeVisible();
}

test.describe("Reveal, for an Editor or above", () => {
  test.use({ scenario: "design-masking-reveal" });

  test("Tab reaches Reveal after the text, Enter opens the dialog on the reason field, and Escape closes it with focus back on Reveal and nothing sent", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await openFile(page, harness.url);
    // The text region is a stop of its own, and Reveal is the next one.
    await text(page).focus();
    await page.keyboard.press("Tab");
    await expect(reveal(page)).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page)).toContainText("recorded in the audit log with your name, the setting and your reason");
    await expect(dialog(page)).toContainText(SETTING);
    await expect(reason(page)).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toHaveCount(0);
    await expect(reveal(page)).toBeFocused();
    // Space opens it as well, and the dialog is the same one.
    await page.keyboard.press("Space");
    await expect(dialog(page)).toBeVisible();
    await expect(reason(page)).toBeFocused();
    // The Cancel button closes it with Enter, and focus goes back again.
    await page.keyboard.press("Tab");
    await expect(dialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toHaveCount(0);
    await expect(reveal(page)).toBeFocused();
    expect(reveals(harness), "nothing was asked of the hub").toEqual([]);
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("in the dialog Tab and Shift+Tab stay inside, Enter in the reason is a new line and never a send, and an empty reason is refused where it is typed", async ({ page, harness }) => {
    await openFile(page, harness.url);
    await reveal(page).focus();
    await page.keyboard.press("Enter");
    await expect(reason(page)).toBeFocused();
    const cancel = dialog(page).getByRole("button", { name: "Cancel" });
    const send = dialog(page).getByRole("button", { name: "Reveal value" });
    // The ring: reason, Cancel, Reveal value, and back to the reason; and the other way.
    await page.keyboard.press("Tab");
    await expect(cancel).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(send).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(reason(page)).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(send).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(cancel).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(reason(page)).toBeFocused();
    // Enter in a multi-line field is a line break: the dialog stays and nothing is sent.
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    expect(reveals(harness)).toEqual([]);
    // A reason of nothing but a line break is no reason. Enter on the button asks for one, in the dialog, and sends nothing.
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(send).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog(page).getByText("Enter a reason")).toBeVisible();
    await expect(reason(page)).toHaveAttribute("aria-invalid", "true");
    await expect(dialog(page)).toBeVisible();
    expect(reveals(harness)).toEqual([]);
  });

  test("a reason and Space on the button send one request, the value appears with Hide focused, and Enter on Hide takes it off again with focus back on Reveal", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await openFile(page, harness.url);
    await reveal(page).focus();
    await page.keyboard.press("Enter");
    await expect(reason(page)).toBeFocused();
    await page.keyboard.type("Teller login fails on sit7; checking the secret matches the vault entry (OPS-2291).");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(dialog(page).getByRole("button", { name: "Reveal value" })).toBeFocused();
    await page.keyboard.press("Space");
    // The value is drawn in the marker's place; the dialog is gone; Hide has focus (the control that took the value away is the one
    // that has to be reachable now).
    await expect(page.locator("[data-slot='revealed-value']")).toHaveText(REVEALED_VALUE);
    await expect(dialog(page)).toHaveCount(0);
    await expect(hide(page)).toBeFocused();
    await expect(page.getByText("Revealed · recorded in the audit log")).toBeVisible();
    // Exactly one request, with the reason, the file and the setting; no copy control for the value anywhere.
    expect(reveals(harness)).toHaveLength(1);
    const body = JSON.parse(reveals(harness)[0]?.body ?? "{}") as { path: string; setting: string; reason: string };
    expect(body.setting).toBe(SETTING);
    expect(body.reason).toBe("Teller login fails on sit7; checking the secret matches the vault entry (OPS-2291).");
    await expect(text(page).getByRole("button")).toHaveCount(1);
    await expect(text(page).getByRole("button", { name: /copy/i })).toHaveCount(0);
    // The announcement says the value is shown and when it goes, and never what it is.
    await expect(page.locator("p[aria-live='polite']")).toContainText("Value revealed. It is hidden again after 30 seconds");
    await expect(page.locator("p[aria-live='polite']")).not.toContainText(REVEALED_VALUE);
    // Enter on Hide: the value goes, and focus goes to the Reveal button that takes its place.
    await page.keyboard.press("Enter");
    await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(0);
    await expect(reveal(page)).toBeFocused();
    await expect(page.locator("body")).not.toContainText(REVEALED_VALUE);
    await expect(page.locator("p[aria-live='polite']")).toHaveText("Value hidden.");
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("Space on Hide hides it as well, and the value can be asked for again with a new reason (a second request, a second record)", async ({ page, harness }) => {
    await openFile(page, harness.url);
    for (const attempt of [1, 2]) {
      await reveal(page).focus();
      await page.keyboard.press("Space");
      await page.keyboard.type(`reason ${attempt}`);
      await page.keyboard.press("Tab");
      await page.keyboard.press("Tab");
      await page.keyboard.press("Enter");
      await expect(hide(page)).toBeFocused();
      await page.keyboard.press("Space");
      await expect(reveal(page)).toBeFocused();
    }
    expect(reveals(harness)).toHaveLength(2);
    expect(await focused(page)).toContain("Reveal value of");
  });
});

test.describe("Reveal, for a Viewer", () => {
  test.use({ scenario: "design-masking-viewer" });

  test("the button is a tab stop that says what it needs, and Enter and Space on it open nothing and send nothing", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openFile(page, harness.url);
    await text(page).focus();
    await page.keyboard.press("Tab");
    await expect(reveal(page)).toBeFocused();
    await expect(reveal(page)).toHaveAttribute("aria-disabled", "true");
    await expect(page.getByRole("tooltip")).toContainText("Needs the Editor role. Ask an Admin to grant it.");
    await expect(reveal(page)).toHaveAccessibleDescription("Needs the Editor role. Ask an Admin to grant it.");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(reveals(harness)).toEqual([]);
    // Escape takes the tooltip away and leaves focus where it was.
    await page.keyboard.press("Escape");
    await expect(page.getByRole("tooltip")).toHaveCount(0);
    await expect(reveal(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
  });
});
