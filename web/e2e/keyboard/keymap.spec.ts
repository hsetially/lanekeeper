// The keyboard, in a real browser: one test for every row of the keymap that the app shell and the dialogs own (the Global, Go
// to, Command palette and Dialogs scopes of the handoff's keys table), each with the negative cases that make a shortcut safe
// (a key typed into a field is text, a key does nothing while a dialog is open, a sequence times out, the wrong role does
// nothing). A row that the keymap gains in one of these scopes and that has no test here fails the last test of this file.
//
// The rows of the other scopes belong to the components (lists, tree, diff, split pane: their own specs) and to the slices
// (S2 to S7: `src/keyboard/bindings.ts` names who owns each).
//
// Chords are written ControlOrMeta, which is Control on the machine the suite runs on; the registry accepts Control or Meta.
import { KEYMAP_DATA } from "../../src/keyboard/keymap.ts";
import type { Page } from "@playwright/test";
import { expect, pageViolations, test } from "../fixtures/test";

const SCOPES = ["Global", "Go to", "Command palette", "Dialogs"] as const;
const COVERED = new Set<string>();
const row = (id: string): string => {
  COVERED.add(id);
  return id;
};

const palette = (page: Page) => page.getByRole("dialog", { name: "Command palette" });
const sheet = (page: Page) => page.getByRole("dialog", { name: "Keyboard shortcuts" });
const sidebar = (page: Page) => page.getByRole("complementary", { name: "Primary" });
const h1 = (page: Page) => page.getByRole("heading", { level: 1 });
const pathOf = (page: Page): string => new URL(page.url()).pathname;
const selected = (page: Page) => palette(page).locator("[cmdk-item][aria-selected='true']");

async function ready(page: Page, harnessUrl: string, path = "/swimlanes"): Promise<void> {
  await page.goto(`${harnessUrl}${path}`);
  await expect(h1(page)).toBeVisible();
  await expect(sidebar(page)).toBeVisible();
}

async function openPalette(page: Page): Promise<void> {
  await page.keyboard.press("ControlOrMeta+k");
  await expect(palette(page)).toBeVisible();
  await expect(palette(page).getByRole("combobox")).toBeFocused();
}

test.describe("the shell's rows", () => {
  test.use({ scenario: "design-shell-admin" });

  // -- Global ---------------------------------------------------------------------------------------------------------

  test(row("global.command-palette") + ": Command-K opens the palette with the field focused; the wrong chords and a bare k do not", async ({ page, harness, csp }) => {
    await ready(page, harness.url);
    await page.keyboard.press("k");
    await page.keyboard.press("Shift+ControlOrMeta+k");
    await page.keyboard.press("Alt+ControlOrMeta+k");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await openPalette(page);
    // Already open: another Command-K does not open a second one.
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog", { name: "Command palette" })).toHaveCount(1);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test(row("global.search-text-focus-query") + ": / goes to Search (text); typed into a field it is a character", async ({ page, harness }) => {
    // On a screen with no filter of its own the shell sends you to Search.
    await ready(page, harness.url, "/approvals");
    await page.keyboard.press("/");
    await expect(page).toHaveURL(/\/search\?type=text$/);
    // On a screen with a filter, `/` is that filter's (a screen's row beats the shell's): focus moves to it and the address stays.
    await ready(page, harness.url);
    await page.keyboard.press("/");
    await expect(page.getByRole("searchbox", { name: "Filter swimlanes" })).toBeFocused();
    expect(pathOf(page)).toBe("/swimlanes");
    await ready(page, harness.url);
    await openPalette(page);
    await page.keyboard.type("a/b");
    await expect(palette(page).getByRole("combobox")).toHaveValue("a/b");
    expect(pathOf(page)).toBe("/swimlanes");
  });

  test(row("global.shortcuts-sheet") + ": ? opens the sheet, lists only what the role can use, and typed into a field is a character", async ({ page, harness }) => {
    await ready(page, harness.url);
    await page.keyboard.press("?");
    await expect(sheet(page)).toBeVisible();
    for (const group of ["Global", "Go to", "Diff navigation", "Lists and trees"]) await expect(sheet(page).getByText(group, { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet(page)).toHaveCount(0);
    await openPalette(page);
    await page.keyboard.type("what?");
    await expect(palette(page).getByRole("combobox")).toHaveValue("what?");
    await expect(sheet(page)).toHaveCount(0);
  });

  test(row("global.toggle-sidebar") + ": Command-B collapses and expands the sidebar, and the choice is still there after a reload", async ({ page, harness }) => {
    await ready(page, harness.url);
    await expect(sidebar(page)).not.toHaveAttribute("data-collapsed", "true");
    await page.keyboard.press("ControlOrMeta+b");
    await expect(sidebar(page)).toHaveAttribute("data-collapsed", "true");
    await page.reload();
    await expect(sidebar(page)).toHaveAttribute("data-collapsed", "true");
    await page.keyboard.press("ControlOrMeta+b");
    await expect(sidebar(page)).not.toHaveAttribute("data-collapsed", "true");
    // The wrong chord does nothing.
    await page.keyboard.press("Shift+ControlOrMeta+b");
    await expect(sidebar(page)).not.toHaveAttribute("data-collapsed", "true");
  });

  test(row("global.close-dialog-menu-or-palette") + ": Escape closes the palette, the sheet, the account menu and the switcher, and focus goes back to what opened it", async ({ page, harness }) => {
    await ready(page, harness.url, "/swimlanes/sitb");
    await openPalette(page);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await page.keyboard.press("?");
    await expect(sheet(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);

    const account = sidebar(page).getByRole("button", { name: /Priya Shah/ });
    await account.click();
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(account).toBeFocused();

    const switcher = page.getByRole("button", { name: /^Swimlane: sitb/ });
    await switcher.click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(switcher).toBeFocused();
  });

  // -- Go to ----------------------------------------------------------------------------------------------------------

  test(row("go-to.swimlanes-compare-feature-status-findings") + ": g then s, c, f, i and a go to Swimlanes, Compare, Feature status, Findings and Audit", async ({ page, harness }) => {
    await ready(page, harness.url, "/status");
    for (const [second, path] of [
      ["s", "/swimlanes"],
      ["c", "/compare"],
      ["f", "/status"],
      ["i", "/findings"],
      ["a", "/audit"],
    ] as const) {
      // Start from somewhere else, so that a key that did nothing cannot pass by accident.
      await ready(page, harness.url, path === "/swimlanes" ? "/status" : "/swimlanes");
      await page.keyboard.press("g");
      await page.keyboard.press(second);
      await expect(page, `g ${second}`).toHaveURL(new RegExp(`${path}$`));
      await expect(h1(page)).toBeVisible();
    }
  });

  test("go-to: a letter that is not in the row, a second key too late, and a g typed into a field do nothing", async ({ page, harness }) => {
    await ready(page, harness.url);
    await page.keyboard.press("g");
    await page.keyboard.press("x");
    await page.keyboard.press("c");
    expect(pathOf(page), "g x is nothing, and the c after it is not a second key").toBe("/swimlanes");

    await page.keyboard.press("g");
    // The sequence waits 1.2 s for its second key.
    await page.waitForTimeout(1500);
    await page.keyboard.press("c");
    expect(pathOf(page), "a second key after the sequence timed out").toBe("/swimlanes");

    await openPalette(page);
    await page.keyboard.type("gc");
    await expect(palette(page).getByRole("combobox")).toHaveValue("gc");
    expect(pathOf(page)).toBe("/swimlanes");
  });

  test("go-to: nothing happens while a menu is open", async ({ page, harness }) => {
    await ready(page, harness.url);
    await sidebar(page).getByRole("button", { name: /Priya Shah/ }).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("g");
    await page.keyboard.press("c");
    expect(pathOf(page)).toBe("/swimlanes");
  });

  // -- Command palette ------------------------------------------------------------------------------------------------

  test(row("command-palette.move-and-run") + ": the arrow keys move the highlight (and wrap), and Enter runs the highlighted row", async ({ page, harness }) => {
    await ready(page, harness.url);
    await openPalette(page);
    await page.keyboard.type("go to");
    const options = palette(page).getByRole("option");
    await expect(options.nth(1)).toBeVisible();
    await expect(selected(page)).toHaveCount(1);
    const first = await selected(page).getAttribute("data-value");
    await page.keyboard.press("ArrowDown");
    const second = await selected(page).getAttribute("data-value");
    expect(second).not.toBe(first);
    await page.keyboard.press("ArrowUp");
    expect(await selected(page).getAttribute("data-value")).toBe(first);
    // Up from the first row wraps to the last.
    await page.keyboard.press("ArrowUp");
    const count = await options.count();
    expect(await selected(page).getAttribute("data-value")).toBe(await options.nth(count - 1).getAttribute("data-value"));

    await page.keyboard.press("Control+a");
    await page.keyboard.type("go to compare");
    await expect(selected(page)).toHaveAttribute("data-value", "actions:go-compare");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/compare$/);
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test(row("command-palette.run-the-nth-result") + ": Command-1 to Command-9 run the first nine results; one that is not there does nothing", async ({ page, harness }) => {
    await ready(page, harness.url);
    for (let n = 1; n <= 9; n += 1) {
      await ready(page, harness.url);
      await openPalette(page);
      await page.keyboard.type("sit");
      // Swimlanes at once, settings and docs after their debounce: wait for the last group of the design's data.
      await expect(palette(page).getByRole("option", { name: /Tenant overrides/ })).toBeVisible();
      const ids = await palette(page).getByRole("option").evaluateAll((els) => els.map((e) => e.getAttribute("data-value") ?? ""));
      const results = ids.filter((id) => !id.startsWith("actions:"));
      expect(results.length, "the design's data gives more than nine results").toBeGreaterThan(9);
      const id = results[n - 1];
      expect(id).toBeDefined();
      await page.keyboard.press(`ControlOrMeta+${String(n)}`);
      await expect(page.getByRole("dialog")).toHaveCount(0);
      const [group, key] = (id ?? "").split(":");
      if (group === "swimlanes") await expect(page, `Command-${String(n)} is ${String(id)}`).toHaveURL(new RegExp(`/swimlanes/${String(key)}$`));
      else if (group === "settings") await expect(page, `Command-${String(n)} is ${String(id)}`).toHaveURL(new RegExp(`/swimlanes/${String(key)}/files\\?`));
      else await expect(page, `Command-${String(n)} is ${String(id)}`).toHaveURL(/\/docs\/view\?/);
    }
    // Fewer results than the number asked for: nothing runs and the palette stays.
    await ready(page, harness.url);
    await openPalette(page);
    await page.keyboard.type("presitc");
    await expect(palette(page).locator("[data-value='swimlanes:presitc']")).toBeVisible();
    await page.keyboard.press("ControlOrMeta+9");
    await expect(palette(page)).toBeVisible();
    expect(pathOf(page)).toBe("/swimlanes");
  });

  test(row("command-palette.clear-the-group-filter") + ": > limits the results to actions, and Backspace in the empty box lifts it", async ({ page, harness }) => {
    await ready(page, harness.url);
    await openPalette(page);
    await page.keyboard.type(">");
    const chip = palette(page).locator("[data-slot='palette-chip']");
    await expect(chip).toHaveText(/Actions/);
    await expect(palette(page).locator("[data-value^='swimlanes:']")).toHaveCount(0);
    await expect(palette(page).locator("[data-value^='actions:']").first()).toBeVisible();
    // Backspace with something typed deletes the character, not the chip.
    await page.keyboard.type("go");
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Backspace");
    await expect(chip).toBeVisible();
    await page.keyboard.press("Backspace");
    await expect(chip).toHaveCount(0);
    await expect(palette(page).locator("[data-value^='swimlanes:']").first()).toBeVisible();
  });
});

test.describe("a Viewer's keys", () => {
  test.use({ scenario: "design-shell-viewer" });

  test("go-to: g then a does nothing for a role that cannot open Audit, and the sheet does not list it", async ({ page, harness }) => {
    await ready(page, harness.url);
    await page.keyboard.press("g");
    await page.keyboard.press("a");
    expect(pathOf(page)).toBe("/swimlanes");
    await expect(page.getByRole("heading", { level: 1, name: /needs the/ })).toHaveCount(0);
    await page.keyboard.press("?");
    await expect(sheet(page)).toBeVisible();
    await expect(sheet(page).getByText("Audit", { exact: true })).toHaveCount(0);
    await expect(sheet(page).getByText("Compare", { exact: true })).toBeVisible();
  });
});

// -- Dialogs (the T0 dialogs, on their test page) -----------------------------------------------------------------------

test.describe("the dialogs' rows", () => {
  test.use({ dist: "dist-perf", scenario: "admin" });

  const inside = (page: Page, role: "alertdialog" | "dialog") => page.evaluate((r) => document.activeElement?.closest(`[role='${r}']`) !== null, role);

  test(row("dialogs.trapped-inside-initial-focus-on") + ": focus starts on the least destructive control, Tab and Shift+Tab stay inside, and Escape gives focus back", async ({ page, harness }) => {
    await page.goto(`${harness.url}/testpages/primitives`);

    // A confirmation: Cancel first, then the action, then back round.
    const open = page.getByRole("button", { name: "Open alert dialog" });
    await open.click();
    const dialog = page.getByRole("alertdialog", { name: "An alert dialog" });
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Confirm change" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(dialog.getByRole("button", { name: "Confirm change" })).toBeFocused();
    for (let i = 0; i < 6; i += 1) {
      await page.keyboard.press("Tab");
      expect(await inside(page, "alertdialog"), "focus stays inside the dialog").toBe(true);
    }
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(open).toBeFocused();

    // The destructive one: initial focus is Cancel, not the field and not the verb.
    await page.getByRole("button", { name: "Open destructive confirm" }).click();
    const destructive = page.getByRole("alertdialog", { name: "Delete file" });
    await expect(destructive.getByRole("button", { name: "Cancel" })).toBeFocused();
    for (let i = 0; i < 8; i += 1) {
      await page.keyboard.press("Shift+Tab");
      expect(await inside(page, "alertdialog"), "Shift+Tab stays inside the dialog").toBe(true);
    }

    // A form dialog: the first control of the dialog, inside it.
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Open dialog" }).click();
    await expect(page.getByRole("dialog", { name: "A dialog" })).toBeVisible();
    await page.keyboard.press("Tab");
    expect(await inside(page, "dialog")).toBe(true);
  });

  test(row("dialogs.confirms-only-when-the-primary") + ": Enter confirms a consequence dialog from its action and never a destructive confirm, in the field or on the button", async ({ page, harness }) => {
    await page.goto(`${harness.url}/testpages/primitives`);

    // A consequence dialog: Enter on the focused Cancel cancels (it is the button that has focus); Tab to the action and Enter runs it.
    await page.getByRole("button", { name: "Open consequence dialog" }).click();
    let dialog = page.getByRole("alertdialog", { name: "Raise a pull request" });
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Raise PR" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);

    // A destructive confirm: the verb is disabled until the exact name is typed, and Enter does nothing before that.
    await page.getByRole("button", { name: "Open destructive confirm" }).click();
    dialog = page.getByRole("alertdialog", { name: "Delete file" });
    const field = dialog.getByRole("textbox");
    const verb = dialog.getByRole("button", { name: "Delete file" });
    await expect(verb).toHaveAttribute("aria-disabled", "true");
    await field.focus();
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();

    // Typed in full: the verb is enabled, and Enter in the field still does not confirm.
    await page.keyboard.type("core-sit1.yml");
    await expect(verb).not.toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();

    // On the verb itself, Enter does not confirm either; Space does (and so does a click).
    await verb.focus();
    await page.keyboard.press("Enter");
    await expect(dialog, "Enter on the destructive button").toBeVisible();
    await page.keyboard.press("Space");
    await expect(dialog, "Space on the destructive button").toHaveCount(0);
  });
});

// A row added to one of the four scopes needs a test here.
test("every row of the Global, Go to, Command palette and Dialogs scopes has a test in this file", () => {
  const rows: string[] = KEYMAP_DATA.filter((r) => (SCOPES as readonly string[]).includes(r.scope)).map((r) => r.id);
  expect(rows.length).toBeGreaterThan(0);
  expect(rows.filter((id) => !COVERED.has(id)), "rows without a test").toEqual([]);
  expect([...COVERED].filter((id) => !rows.includes(id)), "tests for rows that the keymap no longer has").toEqual([]);
});
