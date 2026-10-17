// Every overlay primitive of the app, opened under the production header: dialogs, alert dialogs, the consequence and
// destructive dialogs, the three problem dialogs, a popover, a menu, a tooltip, the palette. They create <style> elements
// (the scroll lock), position with floating-ui and portal into <body>; any inline-style or Trusted Types break shows here.
// The page is a test-only entry of the perf-mode build (src/testpages/primitives.tsx).
import { expect, pageViolations, test } from "../fixtures/test";

test.use({ dist: "dist-perf", scenario: "admin" });

for (const scheme of ["light", "dark"] as const) {
  test.describe(`overlays, ${scheme}`, () => {
    test.use({ colorScheme: scheme });

    const OPEN: Array<[string, "dialog" | "alertdialog", string]> = [
      ["Open dialog", "dialog", "A dialog"],
      ["Open alert dialog", "alertdialog", "An alert dialog"],
      ["Open consequence dialog", "alertdialog", "Raise a pull request"],
      ["Open destructive confirm", "alertdialog", "Delete file"],
      ["Open conflict", "dialog", "Someone else changed it first"],
      ["Open locked", "dialog", "Editing is locked"],
      ["Open sent for approval", "dialog", "Sent for approval"],
      ["Open palette", "dialog", "Command palette"],
    ];

    for (const [button, role, title] of OPEN) {
      test(button, async ({ page, harness, csp }) => {
        await page.goto(`${harness.url}/testpages/primitives`);
        await page.getByRole("button", { name: button }).click();
        await expect(page.getByRole(role, { name: title })).toBeVisible();
        // The scroll lock of a modal makes a <style>; it must carry the nonce.
        const unnonced = await page.evaluate(() => [...document.querySelectorAll("style")].filter((s) => !s.nonce).length);
        expect(unnonced, "<style> elements without a nonce").toBe(0);
        await page.keyboard.press("Escape");
        await expect(page.getByRole(role)).toHaveCount(0);
        expect(await pageViolations(page)).toEqual([]);
        expect(csp.report()).toBe("");
      });
    }

    test("popover, menu, tooltip, tabs, toggle group, select, split pane and command list", async ({ page, harness, csp }) => {
      await page.goto(`${harness.url}/testpages/primitives`);
      await page.getByRole("button", { name: "Open popover" }).click();
      await expect(page.getByText("A popover, positioned by floating-ui.")).toBeVisible();
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Open menu" }).click();
      await expect(page.getByRole("menuitem", { name: "Theme" })).toBeVisible();
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Hover for tooltip" }).hover();
      await expect(page.getByRole("tooltip")).toBeVisible();
      await page.getByRole("tab", { name: "Two" }).click();
      await expect(page.getByText("Second tab")).toBeVisible();
      await page.getByRole("radio", { name: "Unified" }).click();
      await page.getByLabel("Tier").selectOption("presit");
      await page.getByRole("separator").first().focus();
      await page.keyboard.press("ArrowRight");
      await page.getByPlaceholder("Filter").fill("sit");
      await expect(page.getByRole("option", { name: "sit1" })).toBeVisible();
      expect(await pageViolations(page)).toEqual([]);
      expect(csp.report()).toBe("");
    });
  });
}
