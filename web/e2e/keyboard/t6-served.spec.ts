// The keyboard on the "As served" page (T6, slice C) in a real browser: the form is File, Application, Tenant, Channel and the button, in
// that order; Enter in a text field asks (as the button does); focus stays where the person is, on the button or in the field, while
// the request is out and after it; and a form that cannot be a request sends nothing and puts focus on the first field that is wrong.
// The page adds no row to the keymap, so none of these tests names one. Every positive test is keyboard only.
import type { Page } from "@playwright/test";
import { finish } from "../journeys/support";
import { holdRequest, MCP_SERVED_LINK, SERVED_FILE, servedCalls } from "../journeys/t6";
import { DIST, expect, focused, keyboardOnly, ready, searchOf, test } from "./support";

test.use({ dist: DIST });

const form = (page: Page) => page.getByRole("form", { name: "What to ask the config-server for" });
const file = (page: Page) => form(page).getByRole("textbox", { name: "File" });
const application = (page: Page) => form(page).getByRole("textbox", { name: "Application" });
const tenant = (page: Page) => form(page).getByRole("combobox", { name: "Tenant" });
const channel = (page: Page) => form(page).getByRole("textbox", { name: "Channel" });
const show = (page: Page) => form(page).getByRole("button", { name: /^(Show served text|Asking the config-server…)$/ });
const answer = (page: Page) => page.getByRole("region", { name: "The config-server's answer" });
const facts = (page: Page) => answer(page).getByRole("group", { name: "About this answer" });

test.describe("the As served form", () => {
  test.use({ scenario: "design-served-text" });

  test("Tab goes File, Application, Tenant, Channel, Show served text; Shift+Tab goes back the same way; the page's own controls follow the form", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(facts(page)).toBeVisible();
    await file(page).focus();
    await page.keyboard.press("Tab");
    await expect(application(page)).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(tenant(page)).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(channel(page)).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(show(page)).toBeFocused();
    // After the form come the answer's own stops, in reading order: the path's copy button, the hash (which can be read whole) and its
    // copy button, Ask again, the placeholders the config-server left (a scrollable list), the text, and the file the engine reads.
    const after: string[] = [];
    for (let i = 0; i < 7; i += 1) {
      await page.keyboard.press("Tab");
      after.push(await focused(page));
    }
    expect(after).toEqual([
      'button "Copy path"',
      expect.stringMatching(/^span "5c1e0a9/),
      'button "Copy hash"',
      'button "Ask again"',
      'div[role=region] "Placeholders left as written"',
      'div[role=table] "Served text of connection-pool.yml"',
      'a "tx-infinity-api/connection-pool-sit7.yml"',
    ]);
    // The way back.
    await show(page).focus();
    await page.keyboard.press("Shift+Tab");
    await expect(channel(page)).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(tenant(page)).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(application(page)).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(file(page)).toBeFocused();
    // Nothing was asked by moving around.
    expect(servedCalls(harness)).toHaveLength(1);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("Enter in a text field asks, exactly as the button does, and focus stays in that field: the address says what was asked", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(facts(page)).toContainText("sit7");
    const before = servedCalls(harness).length;

    await file(page).focus();
    await page.keyboard.press("End");
    await page.keyboard.type("x");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/file=tx-infinity-api%2Fconnection-pool\.ymlx/);
    await expect(file(page)).toBeFocused();
    await expect.poll(() => servedCalls(harness).length).toBe(before + 1);
    expect(servedCalls(harness).at(-1)?.file).toBe(`${SERVED_FILE}x`);

    await application(page).focus();
    await page.keyboard.press("End");
    await page.keyboard.type("-two");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/application=tx-infinity-core-two/);
    await expect(application(page)).toBeFocused();
    await expect.poll(() => servedCalls(harness).length).toBe(before + 2);
    expect(servedCalls(harness).at(-1)?.application).toBe("tx-infinity-core-two");

    await channel(page).focus();
    await page.keyboard.type("beta");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/channel=beta/);
    await expect(channel(page)).toBeFocused();
    await expect(facts(page)).toContainText("channel beta");
    await expect.poll(() => servedCalls(harness).length).toBe(before + 3);
    expect(servedCalls(harness).at(-1)?.channel).toBe("beta");

    // Each Enter was one ask and one step of history: Back (the browser's, from the keyboard) returns to the one before.
    await page.goBack();
    await expect(channel(page)).toHaveValue("");
    expect(searchOf(page)).not.toContain("channel=");
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("the tenant is chosen with the arrow keys, and Enter on the button asks; the button keeps focus while the request is out and after the answer", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    const held = await holdRequest(page, /\/api\/v1\/swimlanes\/sitb\/served\?/, { method: "GET", nth: 2 });
    await ready(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(facts(page)).toContainText("tx-infinity-core · sit7");
    await tenant(page).focus();
    await page.keyboard.press("ArrowDown");
    await expect(tenant(page)).toHaveValue("sit9");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(show(page)).toBeFocused();
    await page.keyboard.press("Enter");
    await held.reached;
    // The request is out: the button says so, it is busy, and it still has focus (a press now is no second request).
    const busy = form(page).getByRole("button", { name: "Asking the config-server…" });
    await expect(busy).toBeVisible();
    await expect(busy).toHaveAttribute("aria-busy", "true");
    await expect(busy).toBeFocused();
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    expect(held.count(), "the first ask of the link and the held one: a press on the busy button is not a third").toBe(2);
    await held.release();
    await expect(facts(page)).toContainText("tx-infinity-core · sit9");
    const idle = form(page).getByRole("button", { name: "Show served text" });
    await expect(idle).toBeFocused();
    expect(servedCalls(harness).filter((c) => c.tenant === "sit9")).toHaveLength(1);
    // Space on the button asks too (it is the same selection: it asks again).
    const asked = servedCalls(harness).length;
    await page.keyboard.press("Space");
    await expect.poll(() => servedCalls(harness).length).toBe(asked + 1);
    await expect(idle).toBeFocused();
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("a form that cannot be a request", () => {
  test.use({ scenario: "design-served-text" });

  test("sends nothing, says what is wrong next to the field without repeating what was typed, and puts focus on the first field that is wrong", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/swimlanes/sitb/served", "As served");
    await expect(page.getByText("Choose what to ask for", { exact: true })).toBeVisible();
    // Nothing given: the first field that is wrong is the first field.
    await file(page).focus();
    await page.keyboard.press("Enter");
    await expect(file(page)).toBeFocused();
    await expect(file(page)).toHaveAttribute("aria-invalid", "true");
    await expect(form(page).getByRole("alert").filter({ hasText: "A file is required" })).toBeVisible();
    await expect(file(page)).toHaveAccessibleDescription(/A file is required/);

    // A file: the application is the first that is wrong now, and focus moves there from the field Enter was pressed in.
    await page.keyboard.type(SERVED_FILE);
    await page.keyboard.press("Enter");
    await expect(application(page)).toBeFocused();
    await expect(application(page)).toHaveAttribute("aria-invalid", "true");
    await expect(file(page)).not.toHaveAttribute("aria-invalid", "true");
    await expect(form(page).getByRole("alert").filter({ hasText: "An application is required" })).toBeVisible();

    // An application with a character it may not have: the message says what is allowed, and never what was typed.
    await page.keyboard.type("tx core<b>");
    await page.keyboard.press("Enter");
    await expect(application(page)).toBeFocused();
    const message = form(page).getByRole("alert").filter({ hasText: "Use letters, digits" });
    await expect(message).toBeVisible();
    await expect(message).not.toContainText("tx core");
    await expect(application(page)).toHaveAccessibleDescription(/Use letters, digits/);

    // A good application: the tenant is next. It is a select, and it takes focus.
    await page.keyboard.press("Control+a");
    await page.keyboard.type("tx-infinity-core");
    await page.keyboard.press("Enter");
    await expect(tenant(page)).toBeFocused();
    await expect(tenant(page)).toHaveAttribute("aria-invalid", "true");
    await expect(form(page).getByRole("alert").filter({ hasText: "A tenant is required" })).toBeVisible();

    // A tenant, and a channel that is not a name: the channel is the one that is wrong.
    await page.keyboard.press("ArrowDown");
    await expect(tenant(page)).toHaveValue("sit7");
    await page.keyboard.press("Tab");
    await expect(channel(page)).toBeFocused();
    await page.keyboard.type("a/b");
    await page.keyboard.press("Enter");
    await expect(channel(page)).toBeFocused();
    await expect(channel(page)).toHaveAttribute("aria-invalid", "true");
    expect(servedCalls(harness), "none of the five presses was a request").toEqual([]);
    expect(searchOf(page)).toBe("");

    // Fixed, it asks: the errors go, the address holds the selection, and focus is still in the field.
    await page.keyboard.press("Control+a");
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Enter");
    await expect(facts(page)).toContainText("tx-infinity-core · sit7 · no channel");
    await expect(form(page).getByRole("alert")).toHaveCount(0);
    await expect(channel(page)).toBeFocused();
    expect(servedCalls(harness)).toHaveLength(1);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });

  test("the button does the same as Enter in a field: an invalid press sends nothing and focus goes to the first bad field", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/swimlanes/sitb/served", "As served");
    await show(page).focus();
    await page.keyboard.press("Enter");
    await expect(file(page)).toBeFocused();
    await expect(form(page).getByRole("alert")).toHaveCount(3);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(show(page)).toBeFocused();
    await page.keyboard.press("Space");
    await expect(file(page)).toBeFocused();
    expect(servedCalls(harness)).toEqual([]);
    expect(await pointer()).toEqual([]);
    await finish(page, csp, harness);
  });
});
