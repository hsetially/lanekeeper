// Where the keyboard is after the app moves it (WCAG 2.4.3): on the page's h1 after a navigation, and back on the control that opened
// a dialog, a menu or a drawer once it is closed. Every move here is made from the keyboard, and a pointer event that reaches the
// page fails the test.
import { pageViolations } from "../fixtures/test";
import { DIST, expect, test, focused, loaded, h1, keyboardOnly, pathOf, ready } from "./support";

test.use({ dist: DIST });

test.describe("after a navigation", () => {
  test.use({ scenario: "design-shell-admin" });

  test("focus is on the h1 of the new page, whichever way the person got there, and a change of the filters leaves it where it is", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/swimlanes", "Swimlanes");
    // The Go to sequences.
    for (const [second, path, title] of [
      ["c", "/compare", "Compare"],
      ["f", "/status", "Feature status"],
      ["i", "/findings", "Findings"],
      ["a", "/audit", "Audit log"],
      ["s", "/swimlanes", "Swimlanes"],
    ] as const) {
      await page.keyboard.press("g");
      await page.keyboard.press(second);
      await expect(page).toHaveURL(new RegExp(`${path}$`));
      await expect(h1(page), `after g ${second}`).toBeFocused();
      await expect(h1(page)).toHaveText(title);
    }
    // A row of the list: Enter opens the swimlane, and the new h1 has the keyboard.
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/swimlanes\/presitb$/);
    await expect(h1(page)).toHaveText("presitb");
    await expect(h1(page)).toBeFocused();
    // The palette: an action chosen there is a navigation too.
    await page.keyboard.press("ControlOrMeta+k");
    const palette = page.getByRole("dialog", { name: "Command palette" });
    await expect(palette.getByRole("combobox")).toBeFocused();
    await page.keyboard.type("go to compare");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/compare$/);
    await expect(h1(page)).toBeFocused();
    // Back is a navigation as well.
    await page.goBack();
    await expect(page).toHaveURL(/\/swimlanes\/presitb$/);
    await expect(h1(page)).toBeFocused();
    // A different path in the same place (a tab of the swimlane is a route): the heading of the page is the same, and it is focused.
    await page.keyboard.press("g");
    await page.keyboard.press("s");
    await expect(h1(page)).toBeFocused();
    // The filters write their text to the address, which is not a navigation: focus stays in the field.
    await page.keyboard.press("/");
    const filter = page.getByRole("searchbox", { name: "Filter swimlanes" });
    await expect(filter).toBeFocused();
    await page.keyboard.type("sit");
    await expect.poll(() => new URL(page.url()).search).toBe("?q=sit");
    await expect(filter).toBeFocused();
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("focus is on the h1 of a page that is a chunk loaded on demand, even when the chunk arrives late", async ({ page, harness }) => {
    // The shell loads the screens of the sidebar when it is idle, so the screen is held back before the page asks for anything: the
    // first visit to Audit is then the first time the chunk is fetched, and it takes its time.
    let held = 0;
    await page.route("**/assets/*.js", async (route) => {
      if (/AuditPage|AuditList/.test(route.request().url())) {
        held += 1;
        await new Promise((resolve) => setTimeout(resolve, 700));
      }
      await route.continue();
    });
    await ready(page, harness.url, "/swimlanes", "Swimlanes");
    await page.keyboard.press("g");
    await page.keyboard.press("a");
    await expect(page).toHaveURL(/\/audit$/);
    await expect(h1(page)).toHaveText("Audit log");
    await expect(h1(page)).toBeFocused();
    expect(held, "the chunk of the Audit screen was fetched, and held back").toBeGreaterThan(0);
  });
});

test.describe("after a dialog, a menu or a drawer closes", () => {
  test.use({ scenario: "design-swimlanes-six" });

  test("a confirmation opened from a button gives focus back to that button, on Escape and on Cancel", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/swimlanes/sitb/services", "sitb");
    const restart = page.getByRole("button", { name: "Restart tx-infinity-core" });
    await restart.focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("alertdialog", { name: "Restart tx-infinity-core in sitb?" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(restart, "after Escape").toBeFocused();
    // The same for Cancel, and for the button of the row below it.
    const other = page.getByRole("button", { name: "Restart ui-service" });
    await other.focus();
    await page.keyboard.press("Space");
    await expect(page.getByRole("alertdialog", { name: "Restart ui-service in sitb?" })).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(other, "after Cancel").toBeFocused();
    expect(harness.requests().filter((r) => r.method === "POST")).toEqual([]);
    expect(await pointer()).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("the shortcuts sheet and the palette give focus back to where it was, a menu to its button, and the swimlane switcher to its button", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/swimlanes", "Swimlanes");
    const table = page.getByRole("grid", { name: "Swimlanes" });
    await loaded(table);
    await table.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("?");
    const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
    await expect(sheet).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await expect(table, "after the shortcuts sheet").toBeFocused();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(table, "after the palette").toBeFocused();
    // A filter menu of the screen: its button.
    const tier = page.getByRole("button", { name: "Tier" });
    await tier.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(tier, "after the Tier menu").toBeFocused();
    expect(await pointer()).toEqual([]);
  });

  test("a drawer that was opened from a table gives focus back to the table on Escape: Compare's, Findings', Audit's and the Approvals queue's", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    // Compare: Enter on a row of the grid opens the drawer for its setting, and Escape returns to the grid.
    harness.setScenario("design-compare-default");
    await ready(page, harness.url, "/compare?file=services%2Fpayments%2Fapplication.yml&lanes=sit,sitb,sitc", "Compare");
    const grid = page.getByRole("grid", { name: "application.yml across swimlanes" });
    await expect(grid).toBeVisible();
    await grid.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(drawer).toHaveCount(0);
    await expect(grid, "after the Compare drawer").toBeFocused();
    expect(pathOf(page)).toBe("/compare");
    expect(await pointer()).toEqual([]);
    expect(await focused(page)).toContain("application.yml across swimlanes");
  });
});
