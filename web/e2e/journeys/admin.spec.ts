// The Admin area (S8) as an Admin walks it, against a hub that remembers what it was told (admin-hub.ts): approving a request (the
// person is in the table, the count in the sidebar drops), changing a role, a swimlane's tenants and its auto-notify switch (shown
// before the hub answers, and put back when it says no), a path rule, a notification channel whose webhook is masked once saved,
// and the join token that is shown once.
import type { Page } from "@playwright/test";
import { adminHub } from "./admin-hub";
import { DIST, allowClipboard, callsTo, clipboard, expect, expectNoTrace, finish, h1, open, sidebar, test, writesOf } from "./support";

test.use({ dist: DIST });

const WEBHOOK = "https://contoso.webhook.office.com/webhookb2/11111111-2222-3333-4444-555555555555@66666666-7777-8888-9999-000000000000/IncomingWebhook/abcdef0123456789abcdef0123456789/12345678-aaaa-bbbb-cccc-dddddddddddd";
const users = (page: Page) => page.getByRole("grid", { name: "Users" });
const adminItem = (page: Page) => sidebar(page).getByRole("link", { name: /^Admin/ });

test.describe("people", () => {
  test("approving a request puts the person in the table, takes the request off the list, and the count beside Admin drops", async ({ page, harness, csp }) => {
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/users", "Admin");
    await expect(adminItem(page)).toHaveAccessibleName("Admin 1 pending request");
    await expect(page.getByRole("region", { name: "Pending requests" })).toContainText("1 waiting");
    await expect(users(page)).not.toContainText("Sam Okafor");

    // Approve as the role they asked for: a dialog names the consequence, and nothing is sent until it is confirmed.
    await page.getByRole("button", { name: "Approve request from sam.okafor@corp.example" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toBeVisible();
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: /^Approve/ }).click();
    await expect(dialog).toHaveCount(0);
    const posts = callsTo(harness, "POST", /\/api\/v1\/admin\/access-requests\/41\/approve$/);
    expect(posts).toHaveLength(1);
    expect(JSON.parse(posts[0]?.body ?? "{}")).toEqual({ role: "editor" });
    expect(posts[0]).toMatchObject({ csrf: "ok", status: 200 });

    // The lists were read again: the person is a user, the request is gone, and the sidebar no longer counts it.
    await expect(users(page).getByRole("row", { name: /Sam Okafor/ })).toBeVisible();
    await expect(page.getByRole("group", { name: "Access request from sam.okafor@corp.example" })).toHaveCount(0);
    await expect(page.getByRole("region", { name: "Pending requests" })).toContainText("sam.okafor@corp.example approved as Editor");
    await expect(page.getByRole("region", { name: "Pending requests" })).toContainText("None waiting");
    await expect(adminItem(page)).toHaveAccessibleName("Admin");
    await finish(page, csp, harness);
  });

  test("changing a role asks first, sends the role once, and the table shows it after the hub has answered", async ({ page, harness, csp }) => {
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/users", "Admin");
    const lena = users(page).getByRole("row", { name: /Lena Brandt/ });
    await expect(lena.getByRole("combobox", { name: "Role for Lena Brandt" })).toHaveValue("editor");
    await lena.getByRole("combobox", { name: "Role for Lena Brandt" }).selectOption("operator");
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toBeVisible();
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: /^(Change|Make|Set|Save)/ }).click();
    await expect(dialog).toHaveCount(0);
    const patches = callsTo(harness, "PATCH", /\/api\/v1\/admin\/users\//);
    expect(patches).toHaveLength(1);
    expect(JSON.parse(patches[0]?.body ?? "{}")).toEqual({ role: "operator" });
    await expect(lena.getByRole("combobox", { name: "Role for Lena Brandt" })).toHaveValue("operator");
    // Not your own role: the control is off, with the reason.
    await expect(users(page).getByRole("row", { name: /Priya Shah/ })).toContainText("You can't change your own role");
    await finish(page, csp, harness);
  });
});

test.describe("a swimlane's settings", () => {
  async function manage(page: Page, lane: string): Promise<void> {
    await page.getByRole("button", { name: `Manage ${lane}`, exact: true }).click();
    await expect(page.getByRole("switch", { name: `Auto-notify for ${lane}` })).toBeVisible();
  }

  test("editing the tenants sends the whole set, and the swimlane's row shows the first tenant and how many more", async ({ page, harness, csp }) => {
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/swimlanes", "Admin");
    await manage(page, "sitb");
    await page.getByRole("button", { name: "Edit tenants of sitb" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Change the tenants of sitb" });
    await expect(dialog).toBeVisible();
    await dialog.getByLabel("Add a tenant").fill("not a branch name!");
    await page.keyboard.press("Enter");
    await expect(dialog.getByText(/./).filter({ hasText: /invalid|not a valid|letters|characters/i }).first()).toBeVisible();
    await dialog.getByLabel("Add a tenant").fill("sit8");
    await page.keyboard.press("Enter");
    await expect(dialog.getByRole("list", { name: "Tenants" })).toContainText("sit8");
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: "Save tenants" }).click();
    await expect(dialog).toHaveCount(0);
    const puts = callsTo(harness, "PUT", "/api/v1/admin/swimlanes/sitb/tenants");
    expect(puts).toHaveLength(1);
    expect(JSON.parse(puts[0]?.body ?? "{}")).toEqual({ tenants: ["sit7", "sit8"] });
    await expect(page.getByRole("list", { name: "Tenants of sitb" })).toContainText("sit8");
    await finish(page, csp, harness);
  });

  test("the auto-notify switch changes before the hub answers, and is put back, with the reason, when the hub says no (403)", async ({ page, harness, csp }) => {
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/swimlanes", "Admin");
    await manage(page, "sitb");
    const toggle = page.getByRole("switch", { name: "Auto-notify for sitb" });
    await expect(toggle).toBeChecked();

    // Hold the hub's answer to see the page before it: the switch is already off.
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route("**/api/v1/admin/swimlanes/sitb/auto-notify", async (route) => {
      if (route.request().method() === "PUT") await held;
      await route.continue();
    });
    await toggle.click();
    await expect(toggle).not.toBeChecked();
    expect(hub.autoNotify["sitb"], "the hub has not been told yet").toBe(true);
    release();
    await expect.poll(() => hub.autoNotify["sitb"]).toBe(false);
    await expect(toggle).not.toBeChecked();
    await expect(page.getByText("Off: services pick up a change when someone notifies them or restarts them.")).toBeVisible();

    // The next one is refused: the switch goes back to what the hub says, and the page says so.
    harness.setScenario(hub.forbidding);
    const refused = page.waitForResponse((response) => response.request().method() === "PUT" && response.status() === 403);
    await toggle.click();
    await refused;
    await expect(toggle).not.toBeChecked();
    expect(hub.autoNotify["sitb"]).toBe(false);
    await expect(page.getByText("The hub refused this change.")).toBeVisible();
    await expect(page.getByText("req-mock-0001")).toBeVisible();
    await expect(page.getByText("Off: services pick up a change when someone notifies them or restarts them.")).toBeVisible();
    expect(callsTo(harness, "PUT", "/api/v1/admin/swimlanes/sitb/auto-notify").map((r) => r.status)).toEqual([200, 403]);
    await finish(page, csp, harness, { expected: [403] });
  });

  test("a join token is made once, shown once, copied with the button (the clipboard has the whole token), and is gone when it is put away", async ({ page, harness, context, csp }) => {
    await allowClipboard(context, harness.url);
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/swimlanes", "Admin");
    await manage(page, "sitc");
    await page.getByRole("button", { name: "Create join token" }).click();
    const field = page.getByLabel(/join token/i).first();
    await expect(field).toBeVisible();
    const token = await field.inputValue();
    expect(token).toMatch(/^lkjt_[0-9a-f]{20,}$/);
    expect(callsTo(harness, "POST", "/api/v1/admin/agents/sitc/join-token")).toHaveLength(1);
    await page.evaluate(() => navigator.clipboard.writeText("something else"));
    await page.getByRole("button", { name: "Copy join token" }).click();
    await expect(page.getByText("Copied. It will not be shown again.", { exact: true })).toHaveCount(1);
    expect(await clipboard(page)).toBe(token);
    await page.getByRole("button", { name: /I have stored it|stored/i }).click();
    await expect(page.getByLabel(/join token/i)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Copy join token" })).toHaveCount(0);
    await expectNoTrace(page, csp, harness, token);
    await finish(page, csp, harness);
  });
});

test.describe("the rules", () => {
  test("a path rule is added (its fields are checked where they are typed), shows in the table, and is deleted after it is confirmed", async ({ page, harness, csp }) => {
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/paths", "Admin");
    const table = page.getByRole("grid").first();
    await expect(table).toContainText("presit-legacy");
    await page.getByRole("button", { name: "Add rule" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Add a path mapping" });
    await expect(dialog).toBeVisible();
    // Checked before anything is sent: a root that climbs out of the repository.
    await dialog.getByLabel("Rule id").fill("sit-new");
    await dialog.getByLabel("Base repo path").fill("../config");
    await dialog.getByLabel("Tenant repo path").fill("data/config");
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog.getByRole("alert").or(dialog.locator("[data-invalid]")).first()).toBeVisible();
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByLabel("Base repo path").fill("config-new");
    await dialog.getByRole("button", { name: "Save rule" }).click();
    await expect(dialog).toHaveCount(0);
    const puts = callsTo(harness, "PUT", "/api/v1/admin/path-mappings/sit-new");
    expect(puts).toHaveLength(1);
    expect(JSON.parse(puts[0]?.body ?? "{}")).toEqual({ base_root: "config-new", tenant_root: "data/config", swimlanes: [] });
    await expect(table).toContainText("sit-new");

    // Delete it: asked first, and the table loses the row only when the hub has answered.
    await page.getByRole("button", { name: /^Delete.*sit-new/ }).click();
    const confirm = page.getByRole("alertdialog");
    await expect(confirm).toBeVisible();
    expect(callsTo(harness, "DELETE", /path-mappings/)).toHaveLength(0);
    await confirm.getByRole("button", { name: /^Delete/ }).click();
    await expect(confirm).toHaveCount(0);
    expect(callsTo(harness, "DELETE", "/api/v1/admin/path-mappings/sit-new")[0]).toMatchObject({ status: 204, csrf: "ok" });
    await expect(table).not.toContainText("sit-new");
    expect(hub.paths.map((p) => p.id)).toEqual(["default", "presit-legacy"]);
    await finish(page, csp, harness);
  });

  test("a notification channel is added with its webhook, which is typed hidden, sent once, and masked from then on", async ({ page, harness, csp }) => {
    const hub = adminHub();
    harness.setScenario(hub.scenario);
    await open(page, harness.url, "/admin/notify", "Admin");
    await page.getByRole("button", { name: /^Add (a )?channel/ }).click();
    const dialog = page.getByRole("alertdialog", { name: "Add a notification channel" });
    await expect(dialog).toBeVisible();
    await dialog.getByLabel("Channel id").fill("ops");
    await dialog.getByLabel("Name").fill("Ops");
    await dialog.getByRole("checkbox", { name: "Proposal waiting for approval" }).check();
    const webhook = dialog.getByRole("textbox", { name: "Webhook URL" });
    await expect(webhook).toHaveAttribute("type", "password");
    await webhook.fill(WEBHOOK);
    // Shown only on request, and hidden again.
    await dialog.getByRole("button", { name: "Show webhook URL" }).click();
    await expect(webhook).toHaveAttribute("type", "text");
    await dialog.getByRole("button", { name: "Hide webhook URL" }).click();
    await expect(webhook).toHaveAttribute("type", "password");
    await dialog.getByRole("button", { name: /^Save channel/ }).click();
    await expect(dialog).toHaveCount(0);

    const puts = callsTo(harness, "PUT", "/api/v1/admin/notifications/ops");
    expect(puts).toHaveLength(1);
    expect(JSON.parse(puts[0]?.body ?? "{}")).toMatchObject({ name: "Ops", webhook_url: WEBHOOK });
    expect(hub.webhooks["ops"]).toBe(WEBHOOK);
    // The table says a webhook is set and never what it is; nothing the page holds or says has it.
    const table = page.getByRole("grid").first();
    await expect(table.getByRole("row", { name: /Ops/ })).toBeVisible();
    await expect(table.getByRole("row", { name: /Ops/ })).toContainText(/set|configured/i);
    await expectNoTrace(page, csp, harness, WEBHOOK);
    // Editing it later: the field starts empty and says the saved address is kept unless a new one is typed.
    await page.getByRole("button", { name: /^Edit.*ops/i }).click();
    const edit = page.getByRole("alertdialog");
    await expect(edit.getByRole("textbox", { name: "Webhook URL" })).toHaveValue("");
    await expect(edit).toContainText("Leave empty to keep the saved webhook.");
    await page.keyboard.press("Escape");
    await expectNoTrace(page, csp, harness, WEBHOOK);
    await finish(page, csp, harness);
  });
});

test("the Admin area is the same screen on every section: one h1, and the section's title below it; the first page of a visit leaves focus alone, a section reached from the rail has the h1", async ({ page, harness, csp }) => {
  harness.setScenario(adminHub().scenario);
  await open(page, harness.url, "/admin/users", "Admin");
  // A page that has just loaded: nothing has focus, so the first Tab is the skip link (WCAG 2.4.1).
  expect(await page.evaluate(() => document.activeElement === document.body)).toBe(true);
  await page.keyboard.press("Tab");
  await expect(page.getByRole("link", { name: "Skip to content" })).toBeFocused();
  const rail = page.getByRole("navigation", { name: "Admin sections" });
  for (const [path, title] of [
    ["/admin/users", "Users and access requests"],
    ["/admin/swimlanes", "Swimlanes and agents"],
    ["/admin/paths", "Path mappings"],
    ["/admin/notify", "Notifications"],
  ] as const) {
    // From the rail, as a person goes from section to section: a route change, so the heading of the page is where focus goes.
    if (path !== "/admin/users") {
      await rail.getByRole("link", { name: new RegExp(`^${title}`) }).click();
      await expect(page).toHaveURL(new RegExp(`${path}$`));
      await expect(h1(page)).toBeFocused();
    }
    await expect(page.getByRole("heading", { level: 2, name: title })).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
    await expect(h1(page)).toHaveText("Admin");
  }
  await finish(page, csp, harness);
});
