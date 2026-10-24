// The four Admin sections that task T5 adds, as an Admin walks them, against a hub that remembers what it was told (admin-t5-hub.ts):
// the sentinels (read only, with the disconnected VM in an alert), an OS Login name mapped to a person, a severity rule added (the
// page says rules only raise, and has no control that lowers or reorders), and a retention policy that is saved only after a dry run
// for exactly those values, and, when it deletes, after the words are typed.
import type { Page } from "@playwright/test";
import { IDS } from "../../src/mocks/design/admin/fixtures.ts";
import { previewFor } from "../../src/mocks/design/admin-t5/fixtures.ts";
import { adminT5Hub, refusingRetention } from "./admin-t5-hub";
import { DIST, callsTo, designClock, expect, finish, open, test, writesOf } from "./support";

test.use({ dist: DIST });

const alertdialog = (page: Page, name: string | RegExp) => page.getByRole("alertdialog", { name });
const main = (page: Page) => page.getByRole("main");

test.describe("sentinels", () => {
  test("are read only: the list of VMs, an alert for the one that is disconnected, the explanation of what the list is, and nothing that writes", async ({ page, harness, csp }) => {
    await designClock(page);
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/sentinels", "Admin");
    const table = page.getByRole("grid", { name: "Sentinels" });
    await expect(table.getByRole("row")).toHaveCount(7);
    // The disconnected VM is told in an alert, with when it was last seen and what that costs, and in its row with a word, an icon and a colour.
    const alert = page.getByRole("alert").filter({ hasText: "nfs-presitb-01 is disconnected" });
    await expect(alert).toBeVisible();
    await expect(alert).toContainText("Last seen 14 min ago");
    await expect(alert).toContainText("Edits made on this VM can't be attributed to a person until it connects again.");
    const row = table.getByRole("row", { name: /nfs-presitb-01/ });
    await expect(row.locator("[data-slot='sentinel-status']")).toHaveText("Disconnected");
    await expect(row).toContainText("38");
    await expect(table.getByRole("row", { name: /nfs-sit-01/ }).locator("[data-slot='sentinel-status']")).toHaveText("Connected");
    // It says what its time is: the last hello or batch of records, not a heartbeat; and that the list cannot be changed here.
    await expect(main(page)).toContainText("Last seen is the time of its last hello or batch of records");
    await expect(main(page)).toContainText("this list is read only");
    // Nothing on it writes: no button, switch or field in the section, and no request but reads.
    const section = page.getByRole("main");
    await expect(section.getByRole("switch")).toHaveCount(0);
    await expect(section.getByRole("textbox")).toHaveCount(0);
    await expect(section.getByRole("button", { name: /^(Add|Register|Enable|Disable|Remove|Delete|Edit|Save)/ })).toHaveCount(0);
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });

  test("with none registered says what that costs, and shows no alert", async ({ page, harness, csp }) => {
    harness.setScenario("design-admin-t5-sentinels-empty");
    await open(page, harness.url, "/admin/sentinels", "Admin");
    await expect(page.getByText("No sentinels registered")).toBeVisible();
    await expect(main(page)).toContainText("edits made on it can't be attributed to a person");
    await expect(page.getByRole("alert").filter({ hasText: "disconnected" })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("OS Login mapping", () => {
  test("a name is mapped to a person: it is checked where it is typed, sent once with the person, encoded as one segment of the address, and shown after the hub has answered", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/oslogin", "Admin");
    const table = page.getByRole("grid", { name: "OS Login mappings" });
    await expect(table.getByRole("row")).toHaveCount(3);
    await expect(table.getByRole("row", { name: /marco_diaz_contractor_example/ })).toContainText("Marco Diaz");
    // The page says how a name is matched when no row holds it.
    await expect(page.getByRole("region", { name: "How a login name is matched" })).toContainText("dana.okoye@corp.example becomes dana_okoye_corp_example");

    await page.getByRole("button", { name: "Add mapping" }).click();
    const dialog = alertdialog(page, "Add an OS Login mapping");
    const name = dialog.getByRole("textbox", { name: "OS Login username" });
    const user = dialog.getByRole("combobox", { name: "Lanekeeper user" });
    await expect(user.locator("option")).toHaveCount(8);
    // A name that could climb out of its address, or is not one, is refused before anything is sent.
    for (const bad of ["../admin", "a/b", "dana okoye", "100%", "-leading"]) {
      await name.fill(bad);
      await user.selectOption(IDS.dana);
      await dialog.getByRole("button", { name: "Save mapping" }).click();
      await expect(dialog.getByText(/Use the name as the VM records it/)).toBeVisible();
    }
    // A name that is mapped already is refused, with the way to change it.
    await name.fill("lena_b_corp_example");
    await dialog.getByRole("button", { name: "Save mapping" }).click();
    await expect(dialog.getByText("This login name is mapped already. Use Edit on its row to change the user.")).toBeVisible();
    // No person chosen.
    await name.fill("dana_okoye_work");
    await user.selectOption("");
    await dialog.getByRole("button", { name: "Save mapping" }).click();
    await expect(dialog.getByText("Choose the user this name stands for")).toBeVisible();
    expect(writesOf(harness)).toEqual([]);

    await user.selectOption(IDS.dana);
    await dialog.getByRole("button", { name: "Save mapping" }).click();
    await expect(dialog).toHaveCount(0);
    const puts = callsTo(harness, "PUT", /^\/api\/v1\/admin\/os-login-users\//);
    expect(puts).toHaveLength(1);
    expect(puts[0]).toMatchObject({ path: "/api/v1/admin/os-login-users/dana_okoye_work", status: 200, csrf: "ok" });
    expect(JSON.parse(puts[0]?.body ?? "{}")).toEqual({ user: IDS.dana });
    expect(puts[0]?.idempotencyKey).toBeTruthy();
    // The table is the hub's answer to the list asked for again: the new row is in it, with the person's name and email.
    await expect(table.getByRole("row")).toHaveCount(4);
    await expect(table.getByRole("row", { name: /dana_okoye_work/ })).toContainText("Dana Okoye");
    await expect(table.getByRole("row", { name: /dana_okoye_work/ })).toContainText("dana.okoye@corp.example");
    await expect(page.getByText("Mapping dana_okoye_work saved")).toBeVisible();
    await expect(page.getByText("It applies to edits recorded from now on.")).toBeVisible();

    // A name with an @ is one segment of the address (encoded), whatever the hub does with it.
    await page.getByRole("button", { name: "Add mapping" }).click();
    await name.fill("ada.lovelace@corp.example");
    await user.selectOption(IDS.tomas);
    await dialog.getByRole("button", { name: "Save mapping" }).click();
    await expect(dialog).toHaveCount(0);
    expect(callsTo(harness, "PUT", /^\/api\/v1\/admin\/os-login-users\//).map((r) => r.path)).toEqual(["/api/v1/admin/os-login-users/dana_okoye_work", "/api/v1/admin/os-login-users/ada.lovelace%40corp.example"]);
    await expect(table.getByRole("row", { name: /ada\.lovelace@corp\.example/ })).toContainText("Tomás Reyes");
    await finish(page, csp, harness);
  });

  test("a mapping is changed (the name stays, the person changes) and deleted after the dialog says what that does; a disabled person is shown as one", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/oslogin", "Admin");
    const table = page.getByRole("grid", { name: "OS Login mappings" });
    await page.getByRole("button", { name: "Edit mapping lena_b_corp_example" }).click();
    const dialog = alertdialog(page, "Edit mapping lena_b_corp_example");
    await expect(dialog.getByRole("textbox", { name: "OS Login username" })).toHaveJSProperty("readOnly", true);
    // A disabled user can be chosen, and is marked: the registry attributes by an admin's mapping whatever the user's state is.
    await expect(dialog.getByRole("combobox", { name: "Lanekeeper user" }).locator("option", { hasText: "Ines Costa" })).toContainText("(disabled)");
    await dialog.getByRole("combobox", { name: "Lanekeeper user" }).selectOption(IDS.ines);
    await dialog.getByRole("button", { name: "Save mapping" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(table.getByRole("row", { name: /lena_b_corp_example/ })).toContainText("Ines Costa");
    await expect(table.getByRole("row", { name: /lena_b_corp_example/ })).toContainText("disabled");
    expect(JSON.parse(callsTo(harness, "PUT", /os-login-users/)[0]?.body ?? "{}")).toEqual({ user: IDS.ines });

    await page.getByRole("button", { name: "Delete mapping marco_diaz_contractor_example" }).click();
    const confirm = alertdialog(page, "Delete mapping marco_diaz_contractor_example");
    await expect(confirm).toContainText("Edits recorded under this login name are matched by email from now on, or shown by the name alone.");
    expect(callsTo(harness, "DELETE", /os-login-users/)).toEqual([]);
    await confirm.getByRole("button", { name: "Delete mapping" }).click();
    await expect(confirm).toHaveCount(0);
    const deletes = callsTo(harness, "DELETE", /os-login-users/);
    expect(deletes).toHaveLength(1);
    expect(deletes[0]).toMatchObject({ path: "/api/v1/admin/os-login-users/marco_diaz_contractor_example", status: 204, csrf: "ok" });
    await expect(table.getByRole("row", { name: /marco_diaz_contractor_example/ })).toHaveCount(0);
    await expect(table.getByRole("row")).toHaveCount(2);
    await finish(page, csp, harness);
  });
});

test.describe("severity rules", () => {
  test("say that a rule can only raise a severity, and have no control to lower one, to switch a built-in rule off or to put the rules in order", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/severity", "Admin");
    await expect(page.getByText("Rules can raise a severity, never lower it")).toBeVisible();
    await expect(main(page)).toContainText("The built-in rules always apply and are not listed here.");
    await expect(main(page)).toContainText("the highest severity wins");
    await expect(main(page)).toContainText("The order of the rules does not matter.");
    const table = page.getByRole("grid", { name: "Severity rules" });
    await expect(table.getByRole("row")).toHaveCount(5);
    // No order, no priority and no way to move a rule; the table's columns are the rule, what it applies to, its severity and its state.
    await expect(table.getByRole("columnheader")).toHaveText(["Rule", "Applies to", "Severity", "State", "Actions"]);
    // The buttons of the section are the one that adds a rule and, for each rule, edit and delete: nothing else (no move, no order, no lower).
    const buttons = await main(page).getByRole("button").evaluateAll((els) => els.map((el) => el.getAttribute("aria-label") ?? el.textContent?.trim() ?? ""));
    expect(buttons.filter((name) => !/^(Add rule|Edit rule \S+|Delete rule \S+)$/.test(name))).toEqual([]);
    expect(buttons.filter((name) => name.startsWith("Edit rule"))).toHaveLength(4);
    await expect(main(page).getByRole("switch")).toHaveCount(0);
    // A rule with a condition the hub does not look at says it is not applied yet; one that is off says Off.
    await expect(table.getByRole("row", { name: /removed-settings/ })).toContainText("Not applied yet");
    await expect(table.getByRole("row", { name: /payments-high/ })).toContainText("Off");

    // The dialog offers the four levels as what the rule raises TO, and says what a rule that states nothing does.
    await page.getByRole("button", { name: "Add rule" }).click();
    const dialog = alertdialog(page, "Add a severity rule");
    await expect(dialog.getByRole("combobox", { name: "Severity" }).locator("option")).toHaveText(["Critical", "High", "Medium", "Low"]);
    await expect(dialog).toContainText("What a change or a finding that this rule matches is raised to, if the rules that already match say less.");
    await expect(dialog).toContainText("This switches off this rule only: the built-in rules stay on.");
    await expect(dialog.getByRole("alert").or(dialog.locator("[data-slot='alert']")).filter({ hasText: "With no condition, this rule raises every change and every finding" })).toBeVisible();
    await expect(dialog.getByRole("spinbutton")).toHaveCount(0);
    await expect(dialog.getByLabel(/priority/i)).toHaveCount(0);
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });

  test("a rule is added: its id and name are checked, it is sent once with a fixed priority, and the table shows it after the hub answers; a change type is marked as not applied yet", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/severity", "Admin");
    const table = page.getByRole("grid", { name: "Severity rules" });
    await page.getByRole("button", { name: "Add rule" }).click();
    const dialog = alertdialog(page, "Add a severity rule");
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog.getByText("Enter a rule id")).toBeVisible();
    await dialog.getByRole("textbox", { name: "Rule id" }).fill("security-high");
    await dialog.getByRole("textbox", { name: "Name" }).fill("Duplicate");
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog.getByText("A rule with this id exists already. Use Edit on its row to change it.")).toBeVisible();
    await dialog.getByRole("textbox", { name: "Path glob" }).fill("/absolute");
    await dialog.getByRole("textbox", { name: "Rule id" }).fill("limits-critical");
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog.getByText("Paths start at the config root, with no leading /")).toBeVisible();
    expect(writesOf(harness)).toEqual([]);

    await dialog.getByRole("textbox", { name: "Name" }).fill("Payment limits are Critical");
    await dialog.getByRole("textbox", { name: "Path glob" }).fill("limits/**");
    await dialog.getByRole("combobox", { name: "Severity" }).selectOption("critical");
    await dialog.getByRole("checkbox", { name: /Setting value changed/ }).check();
    await expect(dialog.getByRole("status", { name: "What this rule matches" })).toContainText("limits/**");
    await expect(dialog.getByText("Not applied yet")).toBeVisible();
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog).toHaveCount(0);

    const puts = callsTo(harness, "PUT", /^\/api\/v1\/admin\/severity-rules\//);
    expect(puts).toHaveLength(1);
    expect(puts[0]).toMatchObject({ path: "/api/v1/admin/severity-rules/limits-critical", status: 200, csrf: "ok" });
    expect(JSON.parse(puts[0]?.body ?? "{}")).toEqual({ name: "Payment limits are Critical", match: { change_kinds: ["changed"], path_glob: "limits/**" }, severity: "critical", priority: 100, enabled: true });
    await expect(table.getByRole("row")).toHaveCount(6);
    const row = table.getByRole("row", { name: /limits-critical/ });
    await expect(row).toContainText("Payment limits are Critical");
    await expect(row.locator("[data-slot='severity-badge']")).toHaveText("Critical");
    await expect(row).toContainText("On");
    await expect(row).toContainText("Not applied yet");
    await expect(page.getByText("Rule limits-critical saved")).toBeVisible();
    await expect(page.getByText("Pending changes and open findings are rated again with it.")).toBeVisible();
    await finish(page, csp, harness);
  });

  test("an own rule is switched off or deleted from its row: Edit keeps the id and the priority, and a delete asks first and says the built-in rules are not affected", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/severity", "Admin");
    const table = page.getByRole("grid", { name: "Severity rules" });
    await page.getByRole("button", { name: "Edit rule prod-host-critical" }).click();
    const dialog = alertdialog(page, "Edit rule prod-host-critical");
    await expect(dialog.getByRole("textbox", { name: "Rule id" })).toHaveJSProperty("readOnly", true);
    await dialog.getByRole("switch", { name: "This rule is on" }).click();
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog).toHaveCount(0);
    const put = callsTo(harness, "PUT", /severity-rules/)[0];
    expect(put?.path).toBe("/api/v1/admin/severity-rules/prod-host-critical");
    expect(JSON.parse(put?.body ?? "{}")).toMatchObject({ enabled: false, priority: 200, severity: "critical" });
    await expect(table.getByRole("row", { name: /prod-host-critical/ })).toContainText("Off");

    await page.getByRole("button", { name: "Delete rule security-high" }).click();
    const confirm = alertdialog(page, "Delete rule security-high");
    await expect(confirm).toContainText("The built-in rules are not affected.");
    expect(callsTo(harness, "DELETE", /severity-rules/)).toEqual([]);
    await confirm.getByRole("button", { name: "Delete rule" }).click();
    await expect(confirm).toHaveCount(0);
    expect(callsTo(harness, "DELETE", /severity-rules/)[0]).toMatchObject({ path: "/api/v1/admin/severity-rules/security-high", status: 204, csrf: "ok" });
    await expect(table.getByRole("row", { name: /security-high/ })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("retention", () => {
  test("says plainly that it deletes, cannot say whether the job only counts, and has no control for that: the limit of the contract is on the page, not hidden", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/retention", "Admin");
    await expect(page.getByText("Retention permanently deletes stored versions")).toBeVisible();
    await expect(main(page)).toContainText("a deleted version cannot be restored");
    await expect(main(page)).toContainText("The hub's API does not report whether this hub's retention job is deleting or only counting");
    await expect(main(page)).toContainText("Read every policy here as one that deletes.");
    // No switch for the job's own dry-run setting: the contract has none, and the page does not draw a control that would pretend.
    await expect(main(page).getByRole("switch")).toHaveCount(0);
    await expect(main(page).getByRole("checkbox")).toHaveCount(0);
    await expect(page.getByRole("region", { name: "Current policy" })).toContainText("Keeps every version for 180 days and the last 50 versions of each file.");
    await expect(page.getByRole("region", { name: "Current policy" })).toContainText("by Priya Shah");
    // The saved policy's own dry run is on the page, labelled as the saved policy's.
    await expect(page.getByRole("region", { name: "Dry run" })).toContainText("The saved policy: 180 days, last 50 versions");
    await finish(page, csp, harness);
  });

  test("a dry run comes first: Save is off until the dry run for exactly these values is on the screen; then the typed words (and only these) save a policy that deletes", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/retention", "Admin");
    const days = page.getByLabel("Days to keep versions");
    const last = page.getByLabel("Versions to always keep per file");
    const save = page.getByRole("button", { name: "Save policy" });
    const dry = page.getByRole("region", { name: "Dry run" });
    await expect(days).toHaveValue("180");
    await expect(last).toHaveValue("50");

    // Nothing has changed: Save is off, and says why.
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await save.hover();
    await expect(page.getByRole("tooltip")).toContainText("Nothing has changed");
    // Values the form refuses are told where they are typed, and make nothing else possible.
    await days.fill("10");
    await page.getByRole("button", { name: "Run dry run" }).click();
    await expect(page.getByText("Minimum is 30 days")).toBeVisible();
    await last.fill("2");
    await page.getByRole("button", { name: "Run dry run" }).click();
    await expect(page.getByText("Keep at least 5 versions per file")).toBeVisible();
    expect(hub.dryRuns).toEqual([]);

    // Valid values that have not been through a dry run: Save is off, with the reason.
    await days.fill("90");
    await last.fill("50");
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await save.hover();
    await expect(page.getByRole("tooltip")).toContainText("Run the dry run for these values first");
    await save.click({ force: true });
    await expect(page.getByRole("alertdialog")).toHaveCount(0);

    // The dry run asks the hub with the flag and the values; nothing is saved by it; the answer is the hub's number.
    await page.getByRole("button", { name: "Run dry run" }).click();
    const found = previewFor(90, 50);
    await expect(dry).toContainText("This policy: 90 days, last 50 versions");
    await expect(dry).toContainText("Versions that would be deleted");
    await expect(dry).toContainText(found.versions_to_delete.toLocaleString("en-US"));
    expect(hub.dryRuns).toEqual([{ keep_days: 90, keep_last_versions: 50 }]);
    expect(hub.saves).toEqual([]);
    expect(callsTo(harness, "PUT", /^\/api\/v1\/admin\/retention\?dry_run=true$/)).toHaveLength(1);
    await expect(save).not.toHaveAttribute("aria-disabled", "true");

    // A value changed after the dry run puts Save back off, and the page says the dry run is out of date; the old values make it right again.
    await days.fill("91");
    await expect(page.getByText("You changed the values after this dry run. Run it again before you save.")).toBeVisible();
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await days.fill("90");
    await expect(save).not.toHaveAttribute("aria-disabled", "true");

    // Save: a dialog that names what it deletes and wants the words. Nothing is sent until they are typed, exactly.
    await save.click();
    const dialog = alertdialog(page, "Save a policy that deletes versions");
    await expect(dialog).toContainText(`The dry run found ${found.versions_to_delete.toLocaleString("en-US")} versions to delete`);
    await expect(dialog).toContainText("A deleted version cannot be restored.");
    const words = dialog.getByRole("textbox");
    const confirm = dialog.getByRole("button", { name: "Save and delete" });
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    for (const wrong of ["delete", "Delete versions", "delete  versions", " delete versions", "delete versions ", "90 days", "yes"]) {
      await words.fill(wrong);
      await confirm.click({ force: true });
      await expect(dialog).toBeVisible();
      await expect(confirm).toHaveAttribute("aria-disabled", "true");
    }
    expect(hub.saves).toEqual([]);
    await words.fill("delete versions");
    await expect(dialog.getByText("Name matches")).toBeVisible();
    await expect(confirm).not.toHaveAttribute("aria-disabled", "true");
    await confirm.click();
    await expect(dialog).toHaveCount(0);
    expect(hub.saves).toEqual([{ keep_days: 90, keep_last_versions: 50 }]);
    const saves = callsTo(harness, "PUT", /^\/api\/v1\/admin\/retention$/);
    expect(saves).toHaveLength(1);
    expect(saves[0]).toMatchObject({ status: 200, csrf: "ok" });
    expect(JSON.parse(saves[0]?.body ?? "{}")).toEqual({ keep_days: 90, keep_last_versions: 50 });
    await expect(page.getByText("Retention policy saved")).toBeVisible();
    await expect(page.getByText("The retention job uses it from its next run.")).toBeVisible();
    // The page shows the policy the hub has now, and the form is back to it.
    await expect(page.getByRole("region", { name: "Current policy" })).toContainText("Keeps every version for 90 days and the last 50 versions of each file.");
    await expect(days).toHaveValue("90");
    await finish(page, csp, harness);
  });

  test("a policy whose dry run finds nothing to delete needs no typed words: a plain confirmation names the policy and says nothing was found", async ({ page, harness, csp }) => {
    const hub = adminT5Hub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/retention", "Admin");
    await page.getByLabel("Days to keep versions").fill("3650");
    await page.getByRole("button", { name: "Run dry run" }).click();
    const dry = page.getByRole("region", { name: "Dry run" });
    await expect(dry).toContainText("This policy: 3650 days, last 50 versions");
    await expect(dry).toContainText("Versions that would be deleted0");
    await page.getByRole("button", { name: "Save policy" }).click();
    const dialog = alertdialog(page, "Save the retention policy");
    await expect(dialog).toContainText("The dry run found nothing that this policy would delete now.");
    // No words to type: it is a consequence dialog, with its fields none, and its button the verb.
    await expect(dialog.getByRole("textbox")).toHaveCount(0);
    await expect(page.getByRole("alertdialog", { name: "Save a policy that deletes versions" })).toHaveCount(0);
    expect(hub.saves).toEqual([]);
    await dialog.getByRole("button", { name: "Save policy" }).click();
    await expect(dialog).toHaveCount(0);
    expect(hub.saves).toEqual([{ keep_days: 3650, keep_last_versions: 50 }]);
    await finish(page, csp, harness);
  });

  for (const status of [503, 429] as const) {
    test(`a dry run the hub refuses (${String(status)}) is said where it was asked, leaves Save off, and nothing is saved`, async ({ page, harness, csp }) => {
      const hub = adminT5Hub();
      harness.setScenario(refusingRetention(hub, status));
      await open(page, harness.url, "/admin/retention", "Admin");
      await page.getByLabel("Days to keep versions").fill("90");
      await page.getByRole("button", { name: "Run dry run" }).click();
      const dry = page.getByRole("region", { name: "Dry run" });
      await expect(dry.getByRole("alert").or(dry.locator("[data-slot='write-error']")).first()).toBeVisible();
      await expect(dry).toContainText(status === 429 ? "Too many requests" : "could not run the dry run");
      await expect(page.getByRole("button", { name: "Save policy" })).toHaveAttribute("aria-disabled", "true");
      expect(hub.saves).toEqual([]);
      await finish(page, csp, harness, { expected: [status] });
    });
  }
});
