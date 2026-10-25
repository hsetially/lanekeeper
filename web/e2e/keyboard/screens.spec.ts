// The keymap's rows for the screens that bind a key of their own, in a real browser: Search (/ focuses the query), the filter
// fields of Compare, Feature status and the Services tab (/), the Approvals queue (move, open the drawer, Escape closes it), and
// the drawers of Audit and Findings (Escape closes them and focus goes back to where the person was). Each has the cases that make
// a key safe: typed into a field it is text, and a role that may not see the screen gets nothing.
import type { Locator, Page } from "@playwright/test";
import { pageViolations } from "../fixtures/test";
import { DIST, expect, test, keyboardOnly, loaded, pathOf, ready, row, searchOf } from "./support";

test.use({ dist: DIST });

/** The text of the row a grid's keyboard cursor is on. */
async function cursorText(table: Locator): Promise<string> {
  return table.evaluate((el) => {
    const id = el.getAttribute("aria-activedescendant");
    return id === null ? "" : (document.getElementById(id)?.textContent ?? "");
  });
}

test.describe("Search", () => {
  test.use({ scenario: "design-search-admin" });
  const query = (page: Page) => page.getByRole("searchbox", { name: "Query" });

  test(row("search.focus-query") + ": / focuses the query of the tab that is open and selects what is in it; typed into a field it is a character", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/search?type=text", "Search");
    await expect(query(page)).toBeVisible();
    await page.keyboard.press("/");
    await expect(query(page)).toBeFocused();
    await page.keyboard.type("a/b");
    await expect(query(page)).toHaveValue("a/b");
    expect(pathOf(page), "no jump to another screen").toBe("/search");
    // Out of the field and pressed again, the text is selected, so the next key replaces it.
    await page.keyboard.press("Tab");
    await page.keyboard.press("/");
    await expect(query(page)).toBeFocused();
    expect(await query(page).evaluate((el: HTMLInputElement) => el.selectionStart === 0 && el.selectionEnd === el.value.length)).toBe(true);
    // The same on the settings tab and the docs tab: it is the query of the tab that is showing.
    for (const [type, name] of [
      ["settings", "Setting query"],
      ["docs", "Docs query"],
    ] as const) {
      await ready(page, harness.url, `/search?type=${type}`, "Search");
      await expect(page.getByRole("searchbox", { name })).toBeVisible();
      await page.keyboard.press("/");
      await expect(page.getByRole("searchbox", { name })).toBeFocused();
      expect(pathOf(page)).toBe("/search");
    }
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });
});

test.describe("the filter fields", () => {
  test.use({ scenario: "design-compare-default" });

  test(row("local.filter.focus") + ": on Compare, / focuses Search settings in this file", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/compare?file=services%2Fpayments%2Fapplication.yml&lanes=sit,sitb,sitc", "Compare");
    const field = page.getByRole("searchbox", { name: "Search settings" });
    await expect(field).toBeVisible();
    await page.keyboard.press("/");
    await expect(field).toBeFocused();
    expect(pathOf(page), "a screen's row beats the shell's").toBe("/compare");
    await page.keyboard.type("/x/");
    await expect(field).toHaveValue("/x/");
    expect(await pointer()).toEqual([]);
  });

  test("on Feature status, / focuses Filter flags", async ({ page, harness }) => {
    harness.setScenario("design-search-admin");
    await ready(page, harness.url, "/status", "Feature status");
    const field = page.getByRole("searchbox", { name: "Filter flags" });
    await expect(field).toBeVisible();
    await page.keyboard.press("/");
    await expect(field).toBeFocused();
    expect(pathOf(page)).toBe("/status");
  });

  test("on the Services tab, / focuses Filter services, and the field on the Files side keeps its own key", async ({ page, harness }) => {
    harness.setScenario("design-swimlanes-six");
    await ready(page, harness.url, "/swimlanes/sitb/services", "sitb");
    const field = page.getByRole("searchbox", { name: "Filter services" });
    await expect(field).toBeVisible();
    await page.keyboard.press("/");
    await expect(field).toBeFocused();
    expect(pathOf(page)).toBe("/swimlanes/sitb/services");
    await page.keyboard.type("hold");
    await expect(page.getByRole("table", { name: "Services" }).getByRole("row")).toHaveCount(2);
    // f is the file filter's key, and it still is while this one has the page (nothing in a field has focus when it is pressed).
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.keyboard.press("f");
    await expect(page.getByRole("searchbox", { name: "Filter files" })).toBeFocused();
  });
});

test.describe("the Approvals queue", () => {
  test.use({ scenario: "design-approvals-pending" });
  const table = (page: Page) => page.getByRole("grid", { name: "Proposals" });
  const drawer = (page: Page) => page.getByRole("dialog", { name: "Proposal detail" });

  test(row("approvals.move-and-open-the-drawer") + ": the arrows from the page start moving through the queue, Enter opens the drawer for the proposal under the cursor, and inside the drawer the arrows are the drawer's", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/approvals", "Approvals");
    await expect(table(page).getByRole("row")).toHaveCount(7);
    expect(await table(page).getAttribute("aria-activedescendant")).toBeNull();
    await page.keyboard.press("ArrowDown");
    await expect(table(page)).toBeFocused();
    expect(await cursorText(table(page))).toContain("Wen Zhao");
    await page.keyboard.press("ArrowDown");
    expect(await cursorText(table(page))).toContain("Ana Ruiz");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowUp");
    expect(await cursorText(table(page))).toContain("Ana Ruiz");
    // Enter opens the drawer, which is the address of the proposal, and puts the keyboard on its title.
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/approvals\/\d+$/);
    await expect(drawer(page)).toBeVisible();
    await expect(drawer(page).getByRole("heading", { level: 2 })).toBeFocused();
    // Inside the drawer the arrows do not take the keyboard back to the queue.
    await page.keyboard.press("ArrowDown");
    await expect(drawer(page).getByRole("heading", { level: 2 })).toBeFocused();
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("approvals: Control or Command and Enter opens the proposal in a new tab, and the arrows typed into the reason field are the field's", async ({ page, harness, context }) => {
    await ready(page, harness.url, "/approvals", "Approvals");
    await loaded(table(page));
    await table(page).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    const opened = context.waitForEvent("page");
    await page.keyboard.press("ControlOrMeta+Enter");
    const tab = await opened;
    await tab.waitForURL(/\/approvals\/\d+$/);
    expect(pathOf(page), "this tab stays where it is").toBe("/approvals");
    await tab.close();
    await page.keyboard.press("Enter");
    await expect(drawer(page)).toBeVisible();
    const reason = drawer(page).getByRole("textbox", { name: /Reason/ });
    await reason.focus();
    await page.keyboard.type("no ");
    await page.keyboard.press("ArrowLeft");
    await expect(reason).toBeFocused();
    await expect(reason).toHaveValue("no ");
  });

  test(row("approvals.close-drawer-reject-needs-a") + ": Escape closes the drawer, from the title and from the reason field, and Reject waits for a reason", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/approvals", "Approvals");
    await loaded(table(page));
    await table(page).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(drawer(page)).toBeVisible();
    // Reject needs a reason first.
    const reject = drawer(page).getByRole("button", { name: "Reject" });
    await expect(reject).toBeDisabled();
    await drawer(page).getByRole("textbox", { name: /Reason/ }).focus();
    await page.keyboard.type("ab");
    await expect(reject).toBeDisabled();
    await page.keyboard.type("c");
    await expect(reject).toBeEnabled();
    // Escape in the reason field closes the drawer; nothing was decided, and the keyboard is back on the queue it came from.
    await page.keyboard.press("Escape");
    await expect(drawer(page)).toHaveCount(0);
    await expect(page).toHaveURL(/\/approvals$/);
    await expect(table(page)).toBeFocused();
    expect(harness.requests().filter((r) => r.method === "POST")).toEqual([]);
    // And from the title.
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(drawer(page)).toBeVisible();
    await expect(drawer(page).getByRole("heading", { level: 2 })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(drawer(page)).toHaveCount(0);
    await expect(table(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
  });
});

test.describe("the Audit drawer", () => {
  test.use({ scenario: "design-approvals-audit" });
  const table = (page: Page) => page.getByRole("grid", { name: "Audit events" });
  const drawer = (page: Page) => page.getByRole("dialog", { name: "Event detail" });

  test(row("global.close-dialog-menu-or-palette") + ": Escape closes the audit event's drawer, and the keyboard goes back to the table that opened it", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/audit", "Audit log");
    await loaded(table(page));
    await table(page).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(drawer(page)).toBeVisible();
    await expect(drawer(page).getByRole("heading", { level: 2 })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(drawer(page)).toHaveCount(0);
    await expect(table(page)).toBeFocused();
    expect(pathOf(page)).toBe("/audit");
    expect(await pointer()).toEqual([]);
  });
});

test.describe("the Findings drawer", () => {
  test.use({ scenario: "design-findings-open" });
  const table = (page: Page) => page.getByRole("grid", { name: "Findings" });
  const drawer = (page: Page) => page.getByRole("dialog");

  test("findings: Enter opens the drawer for the row under the cursor, Escape closes it, and the keyboard goes back to the table", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/findings", "Findings");
    await loaded(table(page));
    await table(page).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(drawer(page)).toBeVisible();
    // The drawer opens with focus on Close: a button that appears on hover is never where a drawer opens.
    await expect(drawer(page).getByRole("button", { name: "Close" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(drawer(page)).toHaveCount(0);
    await expect(table(page)).toBeFocused();
    expect(await pointer()).toEqual([]);
  });

  test("findings: Control or Command and Enter on a row with a line opens the file in a new tab", async ({ page, harness, context }) => {
    await ready(page, harness.url, "/findings", "Findings");
    await loaded(table(page));
    await table(page).focus();
    await page.keyboard.press("ArrowDown");
    const opened = context.waitForEvent("page");
    await page.keyboard.press("ControlOrMeta+Enter");
    const tab = await opened;
    await tab.waitForURL(/\/swimlanes\/sitb\/files\?/);
    expect(searchOf(page)).toBe("");
    await tab.close();
  });
});
