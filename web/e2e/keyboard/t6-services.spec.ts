// The keyboard on the Services tab and in the dialogs of T6, slice A (pickup chips) and slice B (notify, config-server restart), in a
// real browser: the Tab order of the tab is what it was with the new controls in reading order and no chip a stop; Escape closes a
// tooltip; the Notify dialog starts on Cancel, keeps Tab inside, is refused Escape while a request is out, and gives focus back to its
// opener; the type-to-confirm dialogs (config-server restart: the Deployment's name; restart all: the swimlane id) are never
// confirmed by Enter, not in the field and not on the button.
//
// T6 adds no row to the keymap, so none of these tests names one (the coverage spec counts only `row(...)`). Every positive test is
// keyboard only: a pointer event that reaches the page fails it (a tooltip is opened by moving the mouse, which is not a press).
import type { Locator, Page } from "@playwright/test";
import { callsTo, finish } from "../journeys/support";
import { holdRequest, NOTIFY_PATH, RESTART_PATH, restartAll, APPLIED, SERVICES_TAB, CORE_FILE_PAGE, signedInAs } from "../journeys/t6";
import { DIST, expect, focused, keyboardOnly, ready, test } from "./support";

test.use({ dist: DIST });

const notify = (page: Page) => page.getByRole("button", { name: "Notify services" });
const notifyDialog = (page: Page) => page.getByRole("alertdialog", { name: "Notify services in sitb?" });
const NAME = "csp-configuration-server";
const csBanner = (page: Page) => page.getByRole("status").filter({ hasText: "Config-server restart needed" });
const csOpener = (page: Page) => csBanner(page).getByRole("button", { name: "Restart config-server" });
const csDialog = (page: Page) => page.getByRole("alertdialog", { name: `Restart ${NAME} in sitb?` });
const csField = (page: Page) => csDialog(page).getByRole("textbox", { name: `Type ${NAME} to confirm` });
const csConfirm = (page: Page) => csDialog(page).getByRole("button", { name: `Restart ${NAME}` });

/** True when the focused element is inside the locator. */
const focusIn = (scope: Locator): Promise<boolean> => scope.evaluate((el) => el.contains(document.activeElement));

/** The accessible names of what Tab reaches, one stop at a time, from the current focus. */
async function tabStops(page: Page, count: number, key = "Tab"): Promise<string[]> {
  const seen: string[] = [];
  for (let i = 0; i < count; i += 1) {
    await page.keyboard.press(key);
    seen.push(await focused(page));
  }
  return seen;
}

test.describe("the Services tab", () => {
  test.use({ scenario: "design-pickup-states" });

  test("Tab reaches the banner's button, the filter, Notify services, the Admin link, the table and then each Restart button in order; no pickup chip is ever a stop", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await expect(page.getByRole("button", { name: "Restart limits-service" })).toBeVisible();
    await page.getByRole("tab", { name: /^Services/ }).focus();
    const forward = await tabStops(page, 14);
    expect(forward).toEqual([
      expect.stringMatching(/^div\[role=tabpanel\]/),
      expect.stringMatching(/^button "Restart config-server/),
      'input "Filter services"',
      'button "Notify services"',
      'a "Change in Admin"',
      expect.stringMatching(/^div\[role=table\] "Services"/),
      'button "Restart tx-infinity-core"',
      'button "Restart ui-service"',
      'button "Restart config-server"',
      'button "Restart remote-itm-teller"',
      'button "Restart document-service"',
      'button "Restart holds-service"',
      'button "Restart limits-service"',
      expect.any(String),
    ]);
    // Back the same way: shift-tab retraces the stops.
    await page.getByRole("button", { name: "Restart limits-service" }).focus();
    const backward = await tabStops(page, 6, "Shift+Tab");
    expect(backward).toEqual(['button "Restart holds-service"', 'button "Restart document-service"', 'button "Restart remote-itm-teller"', 'button "Restart config-server"', 'button "Restart ui-service"', 'button "Restart tx-infinity-core"']);
    // The chips are not controls: nothing inside one has ever had focus, and none is focusable.
    expect(await page.locator("[data-slot='pickup-chip']").evaluateAll((chips) => chips.filter((chip) => chip.matches("[tabindex]:not([tabindex='-1'])") || chip.querySelector("a, button, input, [tabindex]:not([tabindex='-1'])") !== null).length)).toBe(0);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("the file tree is ONE stop whose rows the arrow keys walk, and the marks on its rows are not stops of their own", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    const tree = page.getByRole("tree", { name: "Files in sitb" });
    await expect(tree.getByRole("treeitem", { name: /^application\.properties/ })).toBeVisible();
    await page.getByRole("switch").focus();
    const stops = await tabStops(page, 3);
    expect(stops).toEqual([expect.stringMatching(/^div\[role=tree\] "Files in sitb"/), expect.stringMatching(/^div\[role=separator\] "Resize file tree"/), expect.stringMatching(/^a\[role=tab\] "Services/)]);
    await tree.focus();
    const rows: string[] = [];
    for (let i = 0; i < 6; i += 1) {
      await page.keyboard.press("ArrowDown");
      const id = await tree.getAttribute("aria-activedescendant");
      rows.push(id === null ? "" : ((await page.locator(`[id="${id}"]`).getAttribute("aria-label")) ?? (await page.locator(`[id="${id}"]`).textContent()) ?? ""));
      await expect(tree).toBeFocused();
    }
    expect(rows.some((row) => /application\.properties/.test(row) && /Needs notify or restart/.test(row)), "the row names its mark").toBe(true);
    expect(rows.some((row) => /channels\.yml/.test(row) && /Needs config-server restart/.test(row))).toBe(true);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("Escape closes the tooltip of a chip in the table and of a mark in the tree, and does nothing else: no dialog opens or closes, the address stays", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    const where = page.url();
    const row = page.getByRole("table", { name: "Services" }).getByRole("row").filter({ has: page.getByRole("cell", { name: "tx-infinity-core", exact: true }) });
    // The mouse only moves: a tooltip is the one thing here a keyboard cannot open (the chip is not a control), and closing it is the key's.
    const chip = row.locator("[data-slot='pickup-chip']");
    const box = await chip.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move((box?.x ?? 0) + 8, (box?.y ?? 0) + (box?.height ?? 0) / 2);
    const tip = page.getByRole("tooltip");
    await expect(tip).toBeVisible();
    await expect(tip).toContainText("A property file changed. This service takes it after it is notified or restarted.");
    await page.keyboard.press("Escape");
    await expect(tip).toHaveCount(0);
    // The mark of a file in the tree.
    const mark = page.getByRole("treeitem", { name: /^application\.properties/ }).getByRole("img", { name: "Needs notify or restart" });
    const markBox = await mark.boundingBox();
    await page.mouse.move(2, 2);
    await page.mouse.move((markBox?.x ?? 0) + 4, (markBox?.y ?? 0) + 4);
    await expect(tip).toBeVisible();
    await expect(tip).toContainText("A property file: services take the new version after they are notified or restarted.");
    await page.keyboard.press("Escape");
    await expect(tip).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(page.url()).toBe(where);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("the tooltip of a button that is off", () => {
  test.use({ scenario: "design-notify-viewer" });

  test("opens when Tab reaches it, names the role, and Escape closes it with focus where it was; Enter on the button opens nothing", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await expect(notify(page)).toBeVisible();
    await page.getByRole("searchbox", { name: "Filter services" }).focus();
    await page.keyboard.press("Tab");
    await expect(notify(page)).toBeFocused();
    const tip = page.getByRole("tooltip");
    await expect(tip).toBeVisible();
    await expect(tip).toContainText("Needs the Operator role. Ask an Admin to grant it.");
    await page.keyboard.press("Escape");
    await expect(tip).toHaveCount(0);
    await expect(notify(page)).toBeFocused();
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    await page.waitForTimeout(300);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(harness.requests().filter((r) => r.method !== "GET")).toEqual([]);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("the Notify services dialog", () => {
  test.use({ scenario: "design-notify-services-tab" });

  test("Enter on the button opens it with focus on Cancel; Tab and Shift+Tab never leave it; Escape closes it with focus back on the button, and nothing was sent", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await expect(page.getByRole("button", { name: "Restart remote-itm-teller" })).toBeVisible();
    await notify(page).focus();
    await page.keyboard.press("Enter");
    const ask = notifyDialog(page);
    await expect(ask).toBeVisible();
    // Focus starts on the least destructive control, whatever the consequence says.
    await expect(ask.getByRole("button", { name: "Cancel" })).toBeFocused();
    for (let i = 0; i < 8; i += 1) {
      await page.keyboard.press("Tab");
      expect(await focusIn(ask), `Tab ${i + 1} stayed inside the dialog`).toBe(true);
    }
    for (let i = 0; i < 8; i += 1) {
      await page.keyboard.press("Shift+Tab");
      expect(await focusIn(ask), `Shift+Tab ${i + 1} stayed inside the dialog`).toBe(true);
    }
    await page.keyboard.press("Escape");
    await expect(ask).toHaveCount(0);
    await expect(notify(page)).toBeFocused();
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toEqual([]);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("Tab from Cancel reaches the confirm button; Enter on it sends ONE request; the result takes focus itself, Tab reaches Close, and Enter on Close gives focus back to the button", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await notify(page).focus();
    await page.keyboard.press("Enter");
    const ask = notifyDialog(page);
    await expect(ask.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(ask.getByRole("button", { name: "Notify services" })).toBeFocused();
    await page.keyboard.press("Enter");
    const done = page.getByRole("dialog", { name: "Notification sent" });
    await expect(done).toBeVisible();
    // A dialog the person reads: focus is on it, not on a button that the key they just pressed could press again.
    await expect(done).toBeFocused();
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    // A second Enter, held down or repeated, lands on the dialog and sends nothing.
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    await expect(done).toBeVisible();
    const reached: string[] = [];
    for (let i = 0; i < 4 && !(await done.getByRole("button", { name: "Close" }).evaluate((el) => el === document.activeElement)); i += 1) {
      await page.keyboard.press("Tab");
      reached.push(await focused(page));
    }
    await expect(done.getByRole("button", { name: "Close" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(done).toHaveCount(0);
    // Chromium: the element that started it, not the page's body.
    await expect(notify(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("Escape closes the result dialog too, with focus back on the button", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await notify(page).focus();
    await page.keyboard.press("Enter");
    // The dialog is a chunk of its own, fetched when the button is first pressed: wait for it before the next key.
    await expect(notifyDialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    const done = page.getByRole("dialog", { name: "Notification sent" });
    await expect(done).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(done).toHaveCount(0);
    await expect(notify(page)).toBeFocused();
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("while the request is out Escape is refused and the dialog stays; when the answer comes, the result takes focus", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    const held = await holdRequest(page, /\/api\/v1\/swimlanes\/sitb\/notify$/);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await notify(page).focus();
    await page.keyboard.press("Enter");
    await expect(notifyDialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await held.reached;
    const ask = notifyDialog(page);
    await expect(ask.getByRole("button", { name: "Notifying…" })).toBeVisible();
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
    await expect(ask).toBeVisible();
    // Enter on the busy button is not a second request.
    await page.keyboard.press("Enter");
    expect(held.count()).toBe(1);
    await held.release();
    const done = page.getByRole("dialog", { name: "Notification sent" });
    await expect(done).toBeVisible();
    await expect(done).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(notify(page)).toBeFocused();
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("the config-server restart dialog", () => {
  test.use({ scenario: "design-notify-config-server" });

  test("focus starts on Cancel; Enter in the name field never confirms, with the right name or the wrong one; Escape closes it with focus back on the banner's button", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await expect(csBanner(page)).toBeVisible();
    await csOpener(page).focus();
    await page.keyboard.press("Enter");
    await expect(csDialog(page)).toBeVisible();
    await expect(csDialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    // Back from Cancel: the copy button of the name, then the field.
    await page.keyboard.press("Shift+Tab");
    await expect(csDialog(page).getByRole("button", { name: "Copy name" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(csField(page)).toBeFocused();
    // The wrong name, then Enter: nothing.
    await page.keyboard.type("csp-config");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    await expect(csDialog(page)).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    // The right name, then Enter in the field: still nothing.
    await page.keyboard.type("uration-server");
    await expect(csField(page)).toHaveValue(NAME);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    await expect(csDialog(page)).toBeVisible();
    await expect(csField(page)).toBeFocused();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    // Escape leaves, with the name cleared and nothing sent; focus is back on the button that opened it.
    await page.keyboard.press("Escape");
    await expect(csDialog(page)).toHaveCount(0);
    await expect(csOpener(page)).toBeFocused();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    // Opened again, it starts clean.
    await page.keyboard.press("Enter");
    await expect(csDialog(page)).toBeVisible();
    await expect(csField(page)).toHaveValue("");
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("Enter on the confirm button, with the name typed, does not confirm either; Space does, and the result takes focus; Escape closes it with focus back on the banner's button", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await csOpener(page).focus();
    await page.keyboard.press("Enter");
    await expect(csDialog(page)).toBeVisible();
    await expect(csDialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(csField(page)).toBeFocused();
    await page.keyboard.type(NAME);
    // Tab: the copy button, Cancel, then the confirm button (still the dialog: Tab does not leave it).
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(csConfirm(page)).toBeFocused();
    await expect(csConfirm(page)).not.toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(250);
    await expect(csDialog(page)).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    await page.keyboard.press("Space");
    const done = page.getByRole("dialog", { name: "Restart requested" });
    await expect(done).toBeVisible();
    await expect(done).toBeFocused();
    expect(callsTo(harness, "POST", RESTART_PATH)).toHaveLength(1);
    await page.keyboard.press("Escape");
    await expect(done).toHaveCount(0);
    await expect(csOpener(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("with the name not typed the confirm button is focusable but off: Space does nothing, and the dialog says nothing was sent", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await csOpener(page).focus();
    await page.keyboard.press("Enter");
    await expect(csDialog(page)).toBeVisible();
    await expect(csDialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(csConfirm(page)).toBeFocused();
    await expect(csConfirm(page)).toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Space");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(250);
    await expect(csDialog(page)).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("while the request is out Escape is refused, the field and Cancel are off, and the dialog stays until the answer comes", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    const held = await holdRequest(page, /\/api\/v1\/swimlanes\/sitb\/restart$/);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await csOpener(page).focus();
    await page.keyboard.press("Enter");
    await expect(csDialog(page)).toBeVisible();
    await expect(csDialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(csField(page)).toBeFocused();
    await page.keyboard.type(NAME);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Space");
    await held.reached;
    await expect(csDialog(page).getByRole("button", { name: "Restarting…" })).toBeVisible();
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
    await expect(csDialog(page)).toBeVisible();
    await expect(csField(page)).toBeDisabled();
    await expect(csDialog(page).getByRole("button", { name: "Cancel" })).toBeDisabled();
    await held.release();
    await expect(page.getByRole("dialog", { name: "Restart requested" })).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toHaveLength(1);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("Restart services after a save", () => {
  test.use({ scenario: "design-notify-file-saved" });

  const banner = (page: Page) => page.getByRole("status").filter({ hasText: "Saved to sitb NFS" });
  const opener = (page: Page) => banner(page).getByRole("button", { name: "Restart services" });
  const dialog = (page: Page) => page.getByRole("alertdialog", { name: "Restart 2 services in sitb?" });
  const field = (page: Page) => dialog(page).getByRole("textbox", { name: "Type sitb to confirm" });
  const confirm = (page: Page) => dialog(page).getByRole("button", { name: "Restart 2 services" });

  /** Edit and save with the keyboard alone: e, a line, Control+S, and Enter on the review's own button. */
  async function saveWithKeys(page: Page, origin: string): Promise<void> {
    await ready(page, origin, CORE_FILE_PAGE, "tx-infinity-core.yml");
    await page.keyboard.press("e");
    const editor = page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" });
    await expect(editor).toBeFocused({ timeout: 20_000 });
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("# note\r\n");
    await page.keyboard.press("Control+s");
    const review = page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    await expect(review).toBeVisible();
    await review.getByRole("button", { name: "Save to sitb NFS" }).focus();
    await page.keyboard.press("Enter");
    await expect(banner(page)).toBeVisible();
  }

  test("the banner's buttons come in reading order, Enter on Restart services opens the dialog on Cancel, and Enter never confirms it: not in the field, not on the button", async ({ page, harness, csp }) => {
    harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core"], [APPLIED]));
    const pointer = await keyboardOnly(page);
    await saveWithKeys(page, harness.url);
    // Reading order inside the banner: Notify services, Restart services, Dismiss.
    await banner(page).getByRole("button", { name: "Notify services" }).focus();
    expect(await tabStops(page, 2)).toEqual(['button "Restart services"', 'button "Dismiss"']);
    await opener(page).focus();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(field(page)).toBeFocused();
    await page.keyboard.type("sitb");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(250);
    await expect(dialog(page)).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(confirm(page)).toBeFocused();
    await page.keyboard.press("Enter");
    await page.waitForTimeout(250);
    await expect(dialog(page)).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    // Escape leaves; focus is on the button that opened it.
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toHaveCount(0);
    await expect(opener(page)).toBeFocused();
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("Space on the confirm button, with the swimlane's name typed, sends the restarts one at a time; the result takes focus and Escape returns it to the button", async ({ page, harness, csp }) => {
    harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core"], [APPLIED]));
    const pointer = await keyboardOnly(page);
    await saveWithKeys(page, harness.url);
    await opener(page).focus();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page).getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(field(page)).toBeFocused();
    await page.keyboard.type("sitb");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Space");
    const done = page.getByRole("dialog", { name: "Restart requested for 2 services" });
    await expect(done).toBeVisible();
    await expect(done).toBeFocused();
    expect(callsTo(harness, "POST", RESTART_PATH)).toHaveLength(2);
    await page.keyboard.press("Escape");
    await expect(done).toHaveCount(0);
    await expect(opener(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("an Editor, on the Services tab", () => {
  test("Notify services and Restart are off for the keyboard too: the tooltip says why on focus, Enter opens nothing", async ({ page, harness, csp }) => {
    harness.setScenario(signedInAs("design-notify-services-tab", "editor"));
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, SERVICES_TAB, "sitb");
    await notify(page).focus();
    await expect(page.getByRole("tooltip")).toContainText("Needs the Operator role");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(250);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});
