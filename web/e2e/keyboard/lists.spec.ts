// The keymap's list rows, in a real browser, on the screen that owns them: the Swimlanes table (move, open, open in a new tab,
// select and extend, compare the selected rows, focus the filter) and the file tree of a swimlane (the tree-only rows, the
// filter, the split handle). Each test names its row with `row(...)`; keyboard.coverage.spec.ts checks that every row of the
// handoff's keymap is named in this folder or listed there as one that does not apply.
//
// Every positive test is keyboard only (a pointer event that reaches the page fails it), and each row has the negative cases
// that make a shortcut safe: a key typed into the filter field is a character, and a key does nothing it should not.
import type { Locator, Page } from "@playwright/test";
import { pageViolations } from "../fixtures/test";
import { DIST, expect, test, h1, keyboardOnly, loaded, pathOf, ready, row, searchOf } from "./support";

test.use({ dist: DIST });

const grid = (page: Page) => page.getByRole("grid", { name: "Swimlanes" });
const filter = (page: Page) => page.getByRole("searchbox", { name: "Filter swimlanes" });
const compare = (page: Page) => page.getByRole("link", { name: "Compare selected" });

/** The Swimlanes screen, with its rows drawn: a key pressed before they are is lost. */
async function openList(page: Page, origin: string): Promise<void> {
  await ready(page, origin, "/swimlanes");
  await loaded(grid(page));
}

/** The swimlane of the row the grid's keyboard cursor is on ("" when it is on none). */
async function cursor(table: Locator): Promise<string> {
  return table.evaluate((el) => {
    const id = el.getAttribute("aria-activedescendant");
    const row = id === null ? null : document.getElementById(id);
    return row?.querySelector('[role="checkbox"]')?.getAttribute("aria-label")?.replace(/^Select /, "") ?? "";
  });
}
const selectedLanes = (table: Locator): Promise<string[]> =>
  table.evaluate((el) =>
    [...el.querySelectorAll('[role="row"][aria-selected="true"]')].map((r) => r.querySelector('[role="checkbox"]')?.getAttribute("aria-label")?.replace(/^Select /, "") ?? "?"),
  );

test.describe("the Swimlanes table", () => {
  test.use({ scenario: "design-swimlanes-six" });

  test(row("swimlanes.move-and-open-a-swimlane") + ": the arrow keys from anywhere on the page start moving through the list, and Enter opens the swimlane under the cursor", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await openList(page, harness.url);
    await expect(grid(page).getByRole("row")).toHaveCount(7);
    // Nothing has focus in the list yet: the page's key takes the grid and moves the cursor to the first row (sorted by name).
    expect(await grid(page).getAttribute("aria-activedescendant")).toBeNull();
    await page.keyboard.press("ArrowDown");
    await expect(grid(page)).toBeFocused();
    expect(await cursor(grid(page))).toBe("presita");
    await page.keyboard.press("ArrowDown");
    expect(await cursor(grid(page))).toBe("presitb");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/swimlanes\/presitb$/);
    await expect(h1(page)).toHaveText("presitb");
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("swimlanes: ArrowUp from the page also starts at the first row, Enter on the page does not open anything, and a key typed into the filter is text", async ({ page, harness }) => {
    await openList(page, harness.url);
    await page.keyboard.press("ArrowUp");
    await expect(grid(page)).toBeFocused();
    expect(await cursor(grid(page))).toBe("presita");
    // A fresh page: Enter with no cursor opens nothing (the list does not guess).
    await openList(page, harness.url);
    await page.keyboard.press("Enter");
    expect(pathOf(page)).toBe("/swimlanes");
    // In the filter field the arrows belong to the field (the caret), not to the list.
    await filter(page).focus();
    await page.keyboard.press("ArrowDown");
    await expect(filter(page)).toBeFocused();
    expect(await grid(page).getAttribute("aria-activedescendant")).toBeNull();
  });

  test(row("lists-tables-trees.move-selection") + ": the arrows move the cursor, stop at both ends, and Home, End and the page keys jump", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openList(page, harness.url);
    const table = grid(page);
    await page.keyboard.press("ArrowDown");
    await expect(table).toBeFocused();
    const order = ["presita", "presitb", "presitc", "sit", "sitb", "sitc"];
    for (const name of order.slice(1)) {
      await page.keyboard.press("ArrowDown");
      expect(await cursor(table), `down to ${name}`).toBe(name);
    }
    // At the last row Down stays, and Up goes back one.
    await page.keyboard.press("ArrowDown");
    expect(await cursor(table)).toBe("sitc");
    await page.keyboard.press("ArrowUp");
    expect(await cursor(table)).toBe("sitb");
    await page.keyboard.press("Home");
    expect(await cursor(table)).toBe("presita");
    // Up at the first row stays too.
    await page.keyboard.press("ArrowUp");
    expect(await cursor(table)).toBe("presita");
    await page.keyboard.press("End");
    expect(await cursor(table)).toBe("sitc");
    await page.keyboard.press("PageUp");
    expect(await cursor(table)).toBe("presita");
    await page.keyboard.press("PageDown");
    expect(await cursor(table)).toBe("sitc");
    // The cursor is a descendant of the grid, not focus on a row: the grid keeps focus the whole time.
    await expect(table).toBeFocused();
    expect(await pointer()).toEqual([]);
  });

  test(row("lists-tables-trees.open") + ": Enter opens the row under the cursor", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openList(page, harness.url);
    const table = grid(page);
    await table.focus();
    await page.keyboard.press("End");
    expect(await cursor(table)).toBe("sitc");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/swimlanes\/sitc$/);
    expect(await pointer()).toEqual([]);
  });

  test(row("lists-tables-trees.open-in-new-tab") + ": Control or Command and Enter opens the row in a new tab and leaves this one where it is", async ({ page, harness, context }) => {
    await openList(page, harness.url);
    const table = grid(page);
    await table.focus();
    for (let i = 0; i < 4; i += 1) await page.keyboard.press("ArrowDown");
    expect(await cursor(table)).toBe("sit");
    const opened = context.waitForEvent("page");
    await page.keyboard.press("ControlOrMeta+Enter");
    const tab = await opened;
    await tab.waitForURL(/\/swimlanes\/sit$/);
    expect(pathOf(page)).toBe("/swimlanes");
    await expect(table).toBeFocused();
    // The new tab is a page of the app and cannot reach back to this one.
    expect(await tab.evaluate(() => window.opener === null)).toBe(true);
    await tab.close();
    // Plain Enter still opens in this tab.
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/swimlanes\/sit$/);
  });

  test(row("lists-tables-trees.select-row-extends-the-range") + ": Space and x select the row under the cursor, again clears it, and Shift extends the range", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openList(page, harness.url);
    const table = grid(page);
    await table.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Space");
    expect(await selectedLanes(table)).toEqual(["presita"]);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("x");
    expect(await selectedLanes(table)).toEqual(["presita", "presitb"]);
    // The same key again takes the row out.
    await page.keyboard.press("x");
    expect(await selectedLanes(table)).toEqual(["presita"]);
    // Shift extends from the last row chosen to the one under the cursor.
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    expect(await cursor(table)).toBe("sitb");
    await page.keyboard.press("Shift+Space");
    expect(await selectedLanes(table)).toEqual(["presita", "presitb", "presitc", "sit", "sitb"]);
    // The count and the Compare button follow the selection.
    await expect(page.getByText("5 selected")).toBeVisible();
    await expect(compare(page)).toHaveAttribute("href", "/compare?lanes=presita%2Cpresitb%2Cpresitc%2Csit%2Csitb");
    expect(await pointer()).toEqual([]);
  });

  test("select: Space and x typed into the filter field are characters, and select nothing", async ({ page, harness }) => {
    await openList(page, harness.url);
    await filter(page).focus();
    await page.keyboard.type("a x b");
    await expect(filter(page)).toHaveValue("a x b");
    expect(await selectedLanes(grid(page))).toEqual([]);
  });

  test(row("lists-tables-trees.compare-the-selected-rows-row") + ": c compares the selected swimlanes, needs two of them, and is a letter in a field", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openList(page, harness.url);
    const table = grid(page);
    // A letter typed in the filter field is a letter, not a command. (The filter also drops a selected swimlane it hides, so it
    // is tried first and cleared before anything is selected.)
    await filter(page).focus();
    await page.keyboard.type("c");
    expect(pathOf(page)).toBe("/swimlanes");
    await filter(page).fill("");
    // Nothing selected: c does nothing.
    await table.focus();
    await page.keyboard.press("c");
    expect(pathOf(page)).toBe("/swimlanes");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Space");
    // One selected: c still does nothing.
    await page.keyboard.press("c");
    expect(pathOf(page)).toBe("/swimlanes");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Space");
    expect(await selectedLanes(table)).toEqual(["presita", "presitc"]);
    await page.keyboard.press("c");
    await expect(page).toHaveURL(/\/compare\?lanes=presita%2Cpresitc$/);
    await expect(h1(page)).toHaveText("Compare");
    expect(await pointer()).toEqual([]);
  });

  test(row("local.filter.focus") + ": / focuses the filter field of the screen and selects what is in it; typed into the field it is a character", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openList(page, harness.url);
    await page.keyboard.press("/");
    await expect(filter(page)).toBeFocused();
    expect(pathOf(page), "a screen's row beats the shell's: the address stays").toBe("/swimlanes");
    await page.keyboard.type("presit/");
    await expect(filter(page)).toHaveValue("presit/");
    // The text goes to the address a moment later, and the table follows.
    await expect.poll(() => searchOf(page)).toBe("?q=presit%2F");
    // Pressing / again outside the field selects the text, so the next key replaces it.
    await page.keyboard.press("Tab");
    await page.keyboard.press("/");
    await expect(filter(page)).toBeFocused();
    expect(await filter(page).evaluate((el: HTMLInputElement) => el.selectionStart === 0 && el.selectionEnd === el.value.length)).toBe(true);
    expect(await pointer()).toEqual([]);
  });
});

// -- The file tree of a swimlane ----------------------------------------------------------------------------------------------

const tree = (page: Page) => page.getByRole("tree", { name: "Files in sitb" });
const filesFilter = (page: Page) => page.getByRole("searchbox", { name: "Filter files" });

/** The name of the row the tree's keyboard cursor is on, without the "/" a folder wears. */
async function treeCursor(page: Page): Promise<string> {
  return tree(page).evaluate((el) => {
    const id = el.getAttribute("aria-activedescendant");
    const row = id === null ? null : document.getElementById(id);
    return (row?.querySelector(".font-mono")?.textContent ?? "").replace(/\/$/, "");
  });
}
const rowByName = (page: Page, name: string) => tree(page).getByRole("treeitem").filter({ has: page.getByText(new RegExp(`^${name.replace(/[.]/g, "\\.")}/?$`)) });

test.describe("the file tree of a swimlane", () => {
  test.use({ scenario: "design-swimlanes-six" });

  async function openTree(page: Page, origin: string): Promise<void> {
    await ready(page, origin, "/swimlanes/sitb", "sitb");
    await expect(tree(page).getByRole("treeitem")).toHaveCount(8);
    await tree(page).focus();
  }

  test(row("tree-only.first-last-row") + ": Home goes to the first row and End to the last, over the rows that are showing", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openTree(page, harness.url);
    await page.keyboard.press("End");
    expect(await treeCursor(page)).toBe("channels.yml");
    await page.keyboard.press("Home");
    expect(await treeCursor(page)).toBe("ej-templates");
    // A folder that is open adds its files to what End and the arrows cross.
    await page.keyboard.press("ArrowDown");
    expect(await treeCursor(page)).toBe("limits");
    await page.keyboard.press("ArrowRight");
    await expect(tree(page).getByRole("treeitem", { name: /^limits\.yml/ })).toBeVisible();
    await page.keyboard.press("End");
    expect(await treeCursor(page)).toBe("channels.yml");
    expect(await pointer()).toEqual([]);
  });

  test(row("tree-only.expand-or-move-to-first") + ": Right opens a folder and then enters it, Left closes it and then goes to its parent", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openTree(page, harness.url);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    expect(await treeCursor(page)).toBe("limits");
    const limits = rowByName(page, "limits");
    await expect(limits).toHaveAttribute("aria-expanded", "false");
    // Right on a closed folder opens it and stays on it; its files are asked for when it opens.
    await page.keyboard.press("ArrowRight");
    await expect(limits).toHaveAttribute("aria-expanded", "true");
    expect(await treeCursor(page)).toBe("limits");
    const files = tree(page).getByRole("treeitem", { name: /limits\.yml|channel-limits\.yml/ });
    await expect(files).toHaveCount(2);
    // Right on an open folder goes to its first child, which is one level deeper.
    await page.keyboard.press("ArrowRight");
    expect(await treeCursor(page)).toBe("channel-limits.yml");
    await expect(files.first()).toHaveAttribute("aria-level", "2");
    // Right on a file does nothing.
    await page.keyboard.press("ArrowRight");
    expect(await treeCursor(page)).toBe("channel-limits.yml");
    // Left on a file goes to the folder; Left on the open folder closes it; Left on a closed top-level folder does nothing.
    await page.keyboard.press("ArrowLeft");
    expect(await treeCursor(page)).toBe("limits");
    await expect(limits).toHaveAttribute("aria-expanded", "true");
    await page.keyboard.press("ArrowLeft");
    await expect(limits).toHaveAttribute("aria-expanded", "false");
    await expect(files).toHaveCount(0);
    await page.keyboard.press("ArrowLeft");
    expect(await treeCursor(page)).toBe("limits");
    expect(await pointer()).toEqual([]);
  });

  test(row("tree-only.jump-to-the-next-row") + ": typing jumps to the next row that starts with what was typed, a pause starts over, and a letter that starts no row does nothing", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openTree(page, harness.url);
    await page.keyboard.press("s");
    expect(await treeCursor(page)).toBe("security");
    await page.keyboard.press("l");
    expect(await treeCursor(page)).toBe("limits");
    // Several characters at once are a prefix: "res" skips remote-itm-teller (which "re" also starts) for resources.
    await page.keyboard.type("res");
    expect(await treeCursor(page)).toBe("resources");
    await page.keyboard.type("m");
    expect(await treeCursor(page), "resm is no row: the cursor stays where it is").toBe("resources");
    // The same letter again goes to the next row that starts with it, and the last one wraps to the first.
    await page.waitForTimeout(1100);
    await page.keyboard.press("a");
    expect(await treeCursor(page)).toBe("application.properties");
    await page.waitForTimeout(1100);
    await page.keyboard.press("e");
    expect(await treeCursor(page)).toBe("ej-templates");
    await page.waitForTimeout(1100);
    await page.keyboard.press("z");
    expect(await treeCursor(page)).toBe("ej-templates");
    await page.waitForTimeout(1100);
    await page.keyboard.press("C");
    expect(await treeCursor(page), "in either case").toBe("channels.yml");
    expect(await pointer()).toEqual([]);
  });

  test("tree: Enter opens a file in the file view and toggles a folder, and a letter typed into the filter field filters instead of jumping", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openTree(page, harness.url);
    await page.keyboard.press("End");
    await page.keyboard.press("ArrowUp");
    expect(await treeCursor(page)).toBe("application.properties");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/files\?path=application\.properties$/);
    expect(await pointer()).toEqual([]);

    await openTree(page, harness.url);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(rowByName(page, "limits")).toHaveAttribute("aria-expanded", "true");
    await page.keyboard.press("Enter");
    await expect(rowByName(page, "limits")).toHaveAttribute("aria-expanded", "false");
  });

  test("tree: Control or Command and Enter opens a file in a new tab and leaves this one on the tree; on a folder it opens nothing", async ({ page, harness, context }) => {
    await openTree(page, harness.url);
    const opened: string[] = [];
    context.on("page", (tab) => {
      opened.push(tab.url());
    });
    // A folder is not a page: the chord neither opens a tab nor changes the folder.
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    expect(await treeCursor(page)).toBe("limits");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect(rowByName(page, "limits")).toHaveAttribute("aria-expanded", "false");
    // A file is: the first tab to open is this file's, so the folder before it opened none.
    await page.keyboard.press("End");
    await page.keyboard.press("ArrowUp");
    expect(await treeCursor(page)).toBe("application.properties");
    const next = context.waitForEvent("page");
    await page.keyboard.press("ControlOrMeta+Enter");
    const tab = await next;
    await tab.waitForURL(/\/swimlanes\/sitb\/files\?path=application\.properties$/);
    expect(opened).toHaveLength(1);
    expect(pathOf(page)).toBe("/swimlanes/sitb");
    await expect(tree(page)).toBeFocused();
    expect(await tab.evaluate(() => window.opener === null)).toBe(true);
    await tab.close();
  });

  test(row("local.detail.filter-files") + ": f focuses the file filter and selects what is in it; typed into the field it is a character, and the filter narrows the tree", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openTree(page, harness.url);
    await page.keyboard.press("f");
    await expect(filesFilter(page)).toBeFocused();
    await page.keyboard.type("fsecf");
    await expect(filesFilter(page)).toHaveValue("fsecf");
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("sec");
    await expect(tree(page).getByRole("treeitem")).toHaveCount(1);
    await expect(rowByName(page, "security")).toBeVisible();
    expect(await pointer()).toEqual([]);
    // From the page body (nothing focused) f is the same key.
    await filesFilter(page).fill("");
    await page.locator("body").press("Escape");
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.keyboard.press("f");
    await expect(filesFilter(page)).toBeFocused();
    expect(await filesFilter(page).evaluate((el: HTMLInputElement) => el.selectionStart === 0 && el.selectionEnd === el.value.length)).toBe(true);
  });
});

test.describe("the split between the tree and the tabs", () => {
  test.use({ scenario: "design-swimlanes-six" });

  const handle = (page: Page) => page.getByRole("separator", { name: "Resize file tree" });
  const treeWidth = (page: Page): Promise<number> => page.getByRole("region", { name: "Files" }).evaluate((el) => Math.round(el.getBoundingClientRect().width));

  test(row("split-pane-handle.resize-16px-64px-home-end") + ": Left and Right move the handle 16 px, with Shift 64 px, Home collapses the tree, and End takes it to the most it may have", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/swimlanes/sitb", "sitb");
    await handle(page).focus();
    const start = await treeWidth(page);
    await page.keyboard.press("ArrowRight");
    await expect.poll(() => treeWidth(page)).toBe(start + 16);
    await page.keyboard.press("ArrowLeft");
    await expect.poll(() => treeWidth(page)).toBe(start);
    await page.keyboard.press("Shift+ArrowRight");
    await expect.poll(() => treeWidth(page)).toBe(start + 64);
    await page.keyboard.press("Shift+ArrowLeft");
    await expect.poll(() => treeWidth(page)).toBe(start);
    // Home collapses the tree: it has no width, nothing in it can be reached by Tab (it is inert), and the tabs have the room.
    await page.keyboard.press("Home");
    await expect.poll(() => treeWidth(page)).toBe(0);
    await expect(page.locator("#swimlane-tree")).toHaveJSProperty("inert", true);
    await expect(handle(page)).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("Shift+ArrowLeft");
    expect(await treeWidth(page), "nothing further to take away").toBe(0);
    // Right brings it back, and it is reachable again.
    await page.keyboard.press("ArrowRight");
    await expect.poll(() => treeWidth(page)).toBeGreaterThanOrEqual(240);
    await expect(page.locator("#swimlane-tree")).toHaveJSProperty("inert", false);
    // End takes the tree to the most it may have.
    await page.keyboard.press("End");
    await expect.poll(() => treeWidth(page)).toBeGreaterThan(start + 64);
    await page.keyboard.press("Home");
    await expect.poll(() => treeWidth(page)).toBe(0);
    // From collapsed, End brings it back to the most as well.
    await page.keyboard.press("End");
    await expect.poll(() => treeWidth(page)).toBeGreaterThan(start + 64);
    // f ("Filter files") on a collapsed tree brings it back and puts the keyboard in its filter.
    await page.keyboard.press("Home");
    await expect.poll(() => treeWidth(page)).toBe(0);
    await page.keyboard.press("f");
    await expect.poll(() => treeWidth(page)).toBeGreaterThanOrEqual(240);
    await expect(filesFilter(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
    // The handle says what it is for and where it stands.
    await expect(handle(page)).toHaveAttribute("aria-orientation", "vertical");
    await expect(handle(page)).toHaveAttribute("aria-valuenow", /\d/);
  });
});
