// The overlays of the app shell (S0) under the production header: the swimlane switcher's popover, the account menu with its
// theme switch, the command palette, the shortcuts sheet, the toasts, and the tooltips of the collapsed sidebar. They portal
// into <body>, create <style> elements (the scroll lock), position with floating-ui and animate by class; an inline style or a
// Trusted Types break shows here. The page is the real shell on the production build (`dist`) with the Shell page's data
// (src/mocks/design/shell), not the test page of the primitives (overlays.spec.ts covers those).
//
// Restricted to the shell's overlays and the T0 primitives: the screens behind the shell are stubs until the slices land, and
// their own overlays are the slices' to cover.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { expect, pageViolations, test } from "../fixtures/test";

const golden = (name: string): string => readFileSync(new URL(`../fixtures/sse-golden/${name}`, import.meta.url), "utf8");
const sidebar = (page: Page) => page.getByRole("complementary", { name: "Primary" });
const pill = (page: Page) => page.locator("[data-slot='live-indicator']");

async function clean(page: Page, csp: { report(): string }): Promise<void> {
  const unnonced = await page.evaluate(() => [...document.querySelectorAll("style")].filter((s) => !s.nonce).length);
  expect(unnonced, "<style> elements without a nonce").toBe(0);
  expect(await pageViolations(page)).toEqual([]);
  expect(csp.report()).toBe("");
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`the shell's overlays, ${scheme}`, () => {
    test.use({ colorScheme: scheme, scenario: "design-shell-admin" });

    test("the chrome is drawn: the sidebar for the role, the header, and focus on the one h1", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expect(sidebar(page)).toBeVisible();
      for (const name of ["Swimlanes", "Compare", "Feature status", "Findings", "Approvals", "Audit", "Admin"]) {
        await expect(sidebar(page).getByRole("link", { name: new RegExp(`^${name}`) }), name).toBeVisible();
      }
      await expect(page.locator("main")).toHaveCount(1);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      await expect(pill(page)).toHaveAttribute("data-status", "live");
      await clean(page, csp);
    });

    test("the swimlane switcher opens its list, filters it, and a choice goes to that swimlane", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes/sitb`);
      const trigger = page.getByRole("button", { name: /^Swimlane: sitb/ });
      await trigger.click();
      const list = page.getByRole("dialog");
      await expect(list.getByRole("option").first()).toBeVisible();
      await expect(list.getByRole("option")).toHaveCount(6);
      await page.keyboard.type("presita");
      await expect(list.getByRole("option")).toHaveCount(1);
      await page.keyboard.press("Enter");
      await expect(page).toHaveURL(/\/swimlanes\/presita$/);
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(page.getByRole("button", { name: /^Swimlane: presita/ })).toBeVisible();
      await clean(page, csp);
    });

    test("the account menu opens, switches the theme without an inline style, and Sign out is in it", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes`);
      await sidebar(page).getByRole("button", { name: /Priya Shah/ }).click();
      await expect(page.getByRole("menu")).toBeVisible();
      for (const name of ["My GitHub token", "Keyboard shortcuts", "Sign out"]) await expect(page.getByRole("menuitem", { name })).toBeVisible();
      const dark = page.getByRole("menuitemradio", { name: "Dark" });
      await dark.click();
      await expect(page.locator("html")).toHaveClass(/dark/);
      // The menu stays open while the theme is chosen, so that the three can be compared.
      await expect(page.getByRole("menu")).toBeVisible();
      await page.getByRole("menuitemradio", { name: "Light" }).click();
      await expect(page.locator("html")).not.toHaveClass(/dark/);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("menu")).toHaveCount(0);
      await expect(sidebar(page).getByRole("button", { name: /Priya Shah/ })).toBeFocused();
      await clean(page, csp);
    });

    test("the command palette opens on Command-K, searches, and closes on Escape with focus back where it was", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await page.keyboard.press("ControlOrMeta+k");
      const palette = page.getByRole("dialog", { name: "Command palette" });
      await expect(palette).toBeVisible();
      await page.keyboard.type("sit");
      await expect(palette.locator("[data-value='swimlanes:sitb']")).toBeVisible();
      // The server's two groups answer after the debounce.
      await expect(palette.getByRole("option", { name: /logging\.level/ })).toBeVisible();
      await expect(palette.getByRole("option", { name: /Account sorting/ })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await clean(page, csp);
    });

    test("the shortcuts sheet opens on ? and from the account menu", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await page.keyboard.press("?");
      const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
      await expect(sheet).toBeVisible();
      await expect(sheet.getByText("Command palette")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);

      await sidebar(page).getByRole("button", { name: /Priya Shah/ }).click();
      await page.getByRole("menuitem", { name: "Keyboard shortcuts" }).click();
      await expect(sheet).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await clean(page, csp);
    });

    test("a file that changed outside the app shows a toast with a link, and it can be dismissed", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes`);
      await expect(pill(page)).toHaveAttribute("data-status", "live");
      harness.emit(golden("file-observed.txt"));
      const region = page.getByRole("region", { name: "Notifications" });
      await expect(region.getByText(/changed on NFS \(unknown author\)/)).toBeVisible();
      await expect(region.getByRole("link", { name: "View swimlane" })).toHaveAttribute("href", "/swimlanes/sit1");
      await region.getByRole("button", { name: "Dismiss" }).click();
      await expect(region.getByText(/changed on NFS/)).toHaveCount(0);
      await clean(page, csp);
    });

    test("the collapsed sidebar names each item in a tooltip", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/swimlanes`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await page.keyboard.press("ControlOrMeta+b");
      await expect(sidebar(page)).toHaveAttribute("data-collapsed", "true");
      await sidebar(page).getByRole("link", { name: /^Findings/ }).hover();
      await expect(page.getByRole("tooltip")).toContainText("Findings");
      await clean(page, csp);
    });

    test("an unknown path and the 403 screen are drawn plain, with one h1 and a way home", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/no/such/page`);
      await expect(page.getByRole("heading", { level: 1, name: "Page not found" })).toBeVisible();
      await expect(page.getByRole("link", { name: /swimlanes/i })).toBeVisible();
      await page.goto(`${harness.url}/403?req=req-test-0001`);
      await expect(page.getByRole("heading", { level: 1, name: "You do not have access" })).toBeVisible();
      await expect(page.getByText("req-test-0001")).toBeVisible();
      await clean(page, csp);
    });
  });
}

test.describe("a Viewer", () => {
  test.use({ scenario: "design-shell-viewer" });

  test("has no Audit, Approvals or Admin in the sidebar, and a typed address of one says which role it needs", async ({ page, harness, csp }) => {
    await page.goto(`${harness.url}/swimlanes`);
    await expect(sidebar(page)).toBeVisible();
    for (const name of ["Audit", "Approvals", "Admin"]) await expect(sidebar(page).getByRole("link", { name: new RegExp(`^${name}`) })).toHaveCount(0);
    await page.goto(`${harness.url}/audit`);
    await expect(page.getByRole("heading", { level: 1 })).toContainText(/needs the Admin role/);
    await expect(page.getByText(/Ask an Admin/)).toBeVisible();
    await clean(page, csp);
  });
});
