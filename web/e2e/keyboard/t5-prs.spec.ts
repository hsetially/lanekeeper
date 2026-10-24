// The keyboard on the Pull requests page of a swimlane (T5, slice B), in a real browser: the state filter (a radio group with one tab
// stop), the table (the arrow keys move the cursor, Enter follows the pull request on GitHub, in another tab) and what Enter does
// with an address the page does not trust. The page adds no row to the keymap: it uses the table's and the filter's own keys, so
// none of these tests names a row of the keymap (the coverage spec counts only `row(...)`).
//
// Every positive test is keyboard only (a pointer event that reaches the page fails it).
import type { Locator, Page } from "@playwright/test";
import { pageViolations } from "../fixtures/test";
import { DIST, expect, keyboardOnly, loaded, pathOf, ready, searchOf, test } from "./support";

test.use({ dist: DIST });

const PATH = "/swimlanes/sitb/pull-requests";
const grid = (page: Page) => page.getByRole("grid", { name: "Pull requests" });
const state = (page: Page) => page.getByRole("radiogroup", { name: "State" });
const radio = (page: Page, name: string) => state(page).getByRole("radio", { name, exact: true });

/** The pull request number of the row the table's keyboard cursor is on ("" when it is on none). */
async function cursor(table: Locator): Promise<string> {
  return table.evaluate((el) => {
    const id = el.getAttribute("aria-activedescendant");
    const row = id === null ? null : document.getElementById(id);
    return row?.querySelector("[data-pr-link]")?.textContent?.replace(/, on GitHub$/, "").trim() ?? row?.textContent?.match(/#\d+/)?.[0] ?? "";
  });
}

/** Github is not reachable from the sandbox, and the test must not depend on it: answer any address on it with a small page. */
async function stubGithub(page: Page): Promise<void> {
  await page.context().route("https://github.com/**", (route) => route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>stub</title>" }));
}

test.describe("the Pull requests page", () => {
  test.use({ scenario: "design-prs-list" });

  test("the filter is one tab stop: Tab reaches the chosen state, the arrows move through the states, and Space or Enter chooses one and writes it to the address", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, PATH, "Pull requests");
    await loaded(grid(page));
    // The skip link, then the sidebar, the header and the page; the first stop of the page's own controls is the filter, on the state
    // that is chosen (Open is the default).
    await radio(page, "Open").focus();
    await expect(radio(page, "Open")).toBeChecked();
    await expect(radio(page, "Open")).toHaveAttribute("tabindex", "0");
    for (const other of ["Merged", "Closed", "All"]) await expect(radio(page, other)).toHaveAttribute("tabindex", "-1");
    // The arrows move focus along the group and wrap; they choose nothing.
    await page.keyboard.press("ArrowRight");
    await expect(radio(page, "Merged")).toBeFocused();
    await expect(radio(page, "Open")).toBeChecked();
    expect(searchOf(page)).toBe("");
    await page.keyboard.press("ArrowRight");
    await expect(radio(page, "Closed")).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(radio(page, "All")).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(radio(page, "Open")).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(radio(page, "All")).toBeFocused();
    // Space chooses the focused state: the address says so, the list is the hub's answer for it, and focus stays on the control.
    await page.keyboard.press("ArrowLeft");
    await expect(radio(page, "Closed")).toBeFocused();
    await page.keyboard.press("Space");
    await expect(radio(page, "Closed")).toBeChecked();
    expect(searchOf(page)).toBe("?state=closed");
    await expect(grid(page).getByRole("row")).toHaveCount(2);
    await expect(grid(page)).toContainText("limits/limits.yml");
    await expect(radio(page, "Closed")).toBeFocused();
    // Enter chooses too, and Open (the default) is the address without a state.
    // One key at a time: the group moves focus after the key (a script a keystroke later), so two presses in a row would both start from
    // the same state.
    await page.keyboard.press("ArrowLeft");
    await expect(radio(page, "Merged")).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(radio(page, "Open")).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(radio(page, "Open")).toBeChecked();
    expect(searchOf(page)).toBe("");
    await expect(grid(page).getByRole("row")).toHaveCount(4);
    // Choosing the chosen state again does not clear the choice: the filter is never empty.
    await page.keyboard.press("Space");
    await expect(radio(page, "Open")).toBeChecked();
    // The chosen state is the one tab stop: Tab leaves the group for the table, and Shift+Tab comes back to the chosen state.
    await page.keyboard.press("Tab");
    await expect(grid(page)).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(radio(page, "Open")).toBeFocused();
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("the table is the next stop after the filter, the arrows move its cursor and stop at both ends, and Home and End jump", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, PATH, "Pull requests");
    await loaded(grid(page));
    // Open is the default state: three links (418 and the two files of 421), newest first.
    await expect(grid(page).getByRole("row")).toHaveCount(4);
    await radio(page, "Open").focus();
    await page.keyboard.press("Tab");
    await expect(grid(page)).toBeFocused();
    expect(await grid(page).getAttribute("aria-activedescendant"), "focus alone moves no cursor").toBeNull();
    await page.keyboard.press("ArrowDown");
    expect(await cursor(grid(page))).toBe("#418");
    await page.keyboard.press("ArrowDown");
    expect(await cursor(grid(page))).toBe("#421");
    await page.keyboard.press("ArrowDown");
    expect(await cursor(grid(page))).toBe("#421");
    // At the last row Down stays; Up goes back one; Home and End jump.
    await page.keyboard.press("ArrowDown");
    expect(await cursor(grid(page))).toBe("#421");
    await page.keyboard.press("Home");
    expect(await cursor(grid(page))).toBe("#418");
    await page.keyboard.press("ArrowUp");
    expect(await cursor(grid(page))).toBe("#418");
    await page.keyboard.press("End");
    await expect(grid(page)).toBeFocused();
    // The cursor is a descendant of the grid: the grid keeps focus the whole time, and Tab leaves it for the next stop.
    await page.keyboard.press("Tab");
    await expect(grid(page)).not.toBeFocused();
    expect(pathOf(page)).toBe(PATH);
    expect(await pointer()).toEqual([]);
  });

  test("Enter on a row opens that pull request on GitHub in another tab, which cannot reach back, and leaves this page where it is", async ({ page, harness, context, csp }) => {
    const pointer = await keyboardOnly(page);
    await stubGithub(page);
    await ready(page, harness.url, PATH, "Pull requests");
    await loaded(grid(page));
    await grid(page).focus();
    await page.keyboard.press("ArrowDown");
    expect(await cursor(grid(page))).toBe("#418");
    const opened = context.waitForEvent("page");
    await page.keyboard.press("Enter");
    const tab = await opened;
    await tab.waitForURL("https://github.com/csp-tenant/csp-tenant-data/pull/418");
    expect(await tab.evaluate(() => window.opener === null), "noopener").toBe(true);
    await tab.close();
    // This tab did not move, the table kept focus and its cursor, and nothing was asked of the hub for it.
    expect(pathOf(page)).toBe(PATH);
    await expect(grid(page)).toBeFocused();
    expect(await cursor(grid(page))).toBe("#418");
    expect(harness.requests().filter((r) => r.method !== "GET")).toEqual([]);
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("a row's file is a link into the app: Tab from the table reaches it, and Enter on it opens the file in this tab", async ({ page, harness }) => {
    await ready(page, harness.url, PATH, "Pull requests");
    await loaded(grid(page));
    const file = grid(page).getByRole("link", { name: "tx-infinity-api/tx-infinity-core-sit7.yml" });
    await file.focus();
    await expect(file).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/files\?path=tx-infinity-api%2Ftx-infinity-core-sit7\.yml/);
  });
});

test.describe("an address the page does not trust", () => {
  test.use({ scenario: "design-prs-hostile" });

  test("Enter on a row whose address is not https with a host and no credentials opens nothing; on the one that is, it opens it", async ({ page, harness, context, csp }) => {
    await stubGithub(page);
    const pages: string[] = [];
    context.on("page", (opened) => pages.push(opened.url()));
    await ready(page, harness.url, PATH, "Pull requests");
    await loaded(grid(page));
    await expect(grid(page).getByRole("row")).toHaveCount(5);
    // The rows are newest first: #11 is a script, #10 has credentials in the address, #9 is plain http, and #8 is a good address with a
    // very long path. None of the first three has a link at all.
    await expect(grid(page).locator("a[data-pr-link]")).toHaveCount(1);
    await grid(page).focus();
    for (const number of ["#11", "#10", "#9"]) {
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("Enter");
      await page.waitForTimeout(150);
      expect(pages, `Enter on ${number} opened a tab`).toEqual([]);
    }
    expect(pathOf(page)).toBe(PATH);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page.keyboard.press("ArrowDown");
    const opened = context.waitForEvent("page");
    await page.keyboard.press("Enter");
    await (await opened).waitForURL("https://github.com/csp-tenant/csp-tenant-data/pull/8");
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });
});
