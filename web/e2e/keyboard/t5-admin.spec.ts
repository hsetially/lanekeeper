// The keyboard in the dialogs of the Admin sections that task T5 adds (OS Login mapping, severity rules, retention), in a real
// browser: Escape closes a dialog and gives focus back to the control that opened it; Enter submits a ConsequenceDialog from its
// field and from its button; Enter NEVER confirms a DestructiveConfirm (the retention policy that deletes), not in its field and not
// on its button, and a click or Space on the button does only when the words have been typed. None of this is a row of its own: the
// dialogs' rows (`dialogs.*`) are the shell's and are tested in keymap.spec.ts.
import type { Page } from "@playwright/test";
import { pageViolations } from "../fixtures/test";
import type { Harness } from "../harness/serve.ts";
import { DIST, expect, keyboardOnly, loaded, pathOf, ready, test } from "./support";

test.use({ dist: DIST });

const calls = (harness: Harness, method: string, path: RegExp) => harness.requests().filter((r) => r.method === method && r.kind !== "stream" && path.test(r.path));
const alertdialog = (page: Page, name: string | RegExp) => page.getByRole("alertdialog", { name });

test.describe("OS Login mappings", () => {
  test.use({ scenario: "design-admin-t5-oslogin" });
  const PUT = /^\/api\/v1\/admin\/os-login-users\/[^/]+$/;

  async function openSection(page: Page, origin: string): Promise<void> {
    await ready(page, origin, "/admin/oslogin", "Admin");
    await loaded(page.getByRole("grid", { name: "OS Login mappings" }));
  }

  test("Escape closes the add dialog and focus goes back to Add mapping; the dialog opens on its first field", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await openSection(page, harness.url);
    const add = page.getByRole("button", { name: "Add mapping" });
    await add.focus();
    await page.keyboard.press("Enter");
    const dialog = alertdialog(page, "Add an OS Login mapping");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("textbox", { name: "OS Login username" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(add).toBeFocused();
    // Space opens it again, and Cancel with Enter closes it.
    await page.keyboard.press("Space");
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Shift+Tab");
    await expect(dialog.getByRole("button", { name: "Save mapping" })).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);
    await expect(add).toBeFocused();
    expect(calls(harness, "PUT", PUT)).toEqual([]);
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("Enter in the name field submits the dialog: what is missing is said where it is, and with a user chosen the mapping is sent once", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await openSection(page, harness.url);
    await page.getByRole("button", { name: "Add mapping" }).focus();
    await page.keyboard.press("Enter");
    const dialog = alertdialog(page, "Add an OS Login mapping");
    const name = dialog.getByRole("textbox", { name: "OS Login username" });
    await expect(name).toBeFocused();
    // Nothing typed: Enter asks for it, in the dialog, and sends nothing.
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("Choose the user this name stands for")).toBeVisible();
    expect(calls(harness, "PUT", PUT)).toEqual([]);
    // The name, then the user with the arrow keys of the list, then Enter in the name field again.
    await page.keyboard.type("dana_okoye_work");
    await page.keyboard.press("Tab");
    const user = dialog.getByRole("combobox", { name: "Lanekeeper user" });
    await expect(user).toBeFocused();
    await user.selectOption({ label: await user.locator("option").nth(1).innerText() });
    await name.focus();
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);
    const puts = calls(harness, "PUT", PUT);
    expect(puts).toHaveLength(1);
    expect(puts[0]?.path).toBe("/api/v1/admin/os-login-users/dana_okoye_work");
    expect(puts[0]).toMatchObject({ status: 200, csrf: "ok" });
    expect(await pointer()).toEqual([]);
  });

  test("Delete asks first: Enter on Cancel closes it and sends nothing, Escape too, and Enter on the delete button sends one request", async ({ page, harness }) => {
    await openSection(page, harness.url);
    const remove = page.getByRole("button", { name: "Delete mapping lena_b_corp_example" });
    await remove.focus();
    await page.keyboard.press("Enter");
    const dialog = alertdialog(page, "Delete mapping lena_b_corp_example");
    await expect(dialog).toBeVisible();
    // The least destructive control has focus, so a careless Enter closes the dialog.
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);
    await expect(remove).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(remove).toBeFocused();
    expect(calls(harness, "DELETE", PUT)).toEqual([]);
    // The second time it goes through: Tab to the delete button, Enter.
    await page.keyboard.press("Enter");
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Delete mapping" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect.poll(() => calls(harness, "DELETE", PUT).length).toBe(1);
    expect(calls(harness, "DELETE", PUT)[0]?.path).toBe("/api/v1/admin/os-login-users/lena_b_corp_example");
  });
});

test.describe("Severity rules", () => {
  test.use({ scenario: "design-admin-t5-severity" });
  const PUT = /^\/api\/v1\/admin\/severity-rules\/[^/]+$/;

  test("Escape closes the dialog and focus goes back; Enter in the id field asks for what is missing and sends nothing; a rule is sent with Enter from its name", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, "/admin/severity", "Admin");
    await loaded(page.getByRole("grid", { name: "Severity rules" }));
    const add = page.getByRole("button", { name: "Add rule" });
    await add.focus();
    await page.keyboard.press("Enter");
    const dialog = alertdialog(page, "Add a severity rule");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("textbox", { name: "Rule id" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(add).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog.getByRole("textbox", { name: "Rule id" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog.getByText("Enter a rule id")).toBeVisible();
    expect(calls(harness, "PUT", PUT)).toEqual([]);
    await page.keyboard.type("limits-high");
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("textbox", { name: "Name" })).toBeFocused();
    await page.keyboard.type("Limits are High");
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);
    const puts = calls(harness, "PUT", PUT);
    expect(puts).toHaveLength(1);
    expect(puts[0]?.path).toBe("/api/v1/admin/severity-rules/limits-high");
    expect(JSON.parse(puts[0]?.body ?? "{}")).toMatchObject({ name: "Limits are High", severity: "high", enabled: true });
    expect(await pointer()).toEqual([]);
  });
});

test.describe("Retention", () => {
  test.use({ scenario: "design-admin-t5-retention" });
  // A save is a PUT without the dry-run flag; a dry run is the same PUT with it.
  const puts = (harness: Harness) => calls(harness, "PUT", /^\/api\/v1\/admin\/retention(\?.*)?$/);
  const saves = (harness: Harness) => puts(harness).filter((r) => !r.path.includes("dry_run=true"));
  const dryRuns = (harness: Harness) => puts(harness).filter((r) => r.path.includes("dry_run=true"));

  async function openSection(page: Page, origin: string): Promise<void> {
    await ready(page, origin, "/admin/retention", "Admin");
    await expect(page.getByLabel("Days to keep versions")).toBeVisible();
  }

  test("a policy that deletes: the save waits for its dry run, and Enter never confirms the typed words, in the field or on the button; Space on the button does, once they match", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await openSection(page, harness.url);
    const days = page.getByLabel("Days to keep versions");
    const save = page.getByRole("button", { name: "Save policy" });
    const run = page.getByRole("button", { name: "Run dry run" });
    // Edit with the keyboard: select the field, type, Tab.
    await days.focus();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("90");
    await expect(days).toHaveValue("90");
    // Before a dry run for these values the save is off, and Enter on it opens nothing.
    await save.focus();
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    // Enter on Run dry run asks the hub, once, with the flag; the answer is on the page.
    await run.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByText("This policy: 90 days, last 50 versions")).toBeVisible();
    expect(dryRuns(harness)).toHaveLength(1);
    expect(saves(harness)).toEqual([]);
    await expect(save).not.toHaveAttribute("aria-disabled", "true");
    // Enter on Save policy: the dialog that names what it deletes, with focus on Cancel; Escape closes it and focus goes back.
    await save.focus();
    await page.keyboard.press("Enter");
    const dialog = alertdialog(page, "Save a policy that deletes versions");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(save).toBeFocused();

    // Again. The words are asked for; Enter in the field does nothing, typed right or not.
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    const words = dialog.getByRole("textbox");
    const confirm = dialog.getByRole("button", { name: "Save and delete" });
    await words.focus();
    await page.keyboard.type("delete");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("Enter does nothing until the name matches")).toBeVisible();
    // On the button, before the words match: neither key does anything.
    await confirm.focus();
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    await expect(dialog).toBeVisible();
    expect(saves(harness)).toEqual([]);
    // The words typed exactly: Enter in the field still does nothing; Enter on the button still does nothing.
    await words.focus();
    await page.keyboard.type(" versions");
    await expect(words).toHaveValue("delete versions");
    await expect(dialog.getByText("Name matches")).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await confirm.focus();
    await expect(confirm).not.toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(300);
    await expect(dialog).toBeVisible();
    expect(saves(harness), "Enter did not confirm").toEqual([]);
    // Space on the button is a click: the policy is saved, once, with the values that were looked at.
    await page.keyboard.press("Space");
    await expect(dialog).toHaveCount(0);
    expect(saves(harness)).toHaveLength(1);
    expect(saves(harness)[0]).toMatchObject({ status: 200, csrf: "ok" });
    expect(JSON.parse(saves(harness)[0]?.body ?? "{}")).toEqual({ keep_days: 90, keep_last_versions: 50 });
    await expect(page.getByText("Retention policy saved")).toBeVisible();
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("a changed value after the dry run puts the save back off, with the reason, until the dry run is run again", async ({ page, harness }) => {
    await openSection(page, harness.url);
    const days = page.getByLabel("Days to keep versions");
    await days.focus();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("120");
    await page.getByRole("button", { name: "Run dry run" }).focus();
    await page.keyboard.press("Space");
    await expect(page.getByText("This policy: 120 days, last 50 versions")).toBeVisible();
    const save = page.getByRole("button", { name: "Save policy" });
    await expect(save).not.toHaveAttribute("aria-disabled", "true");
    await days.focus();
    await page.keyboard.type("0");
    await expect(page.getByText("You changed the values after this dry run. Run it again before you save.")).toBeVisible();
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await save.focus();
    await expect(page.getByRole("tooltip")).toContainText("Run the dry run for these values first");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(saves(harness)).toEqual([]);
    expect(pathOf(page)).toBe("/admin/retention");
  });
});
