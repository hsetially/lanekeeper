// Who may press what, and when (T6, slice B, and the Restart dialog of T4): notify and restart are an Operator's. Below that the buttons
// stay on screen, disabled, with the reason as a tooltip and as the accessible description, and a press sends nothing; with live
// updates paused every confirm button of every restart and notify dialog turns off, including one that was already open.
// The server enforces all of it regardless; these journeys prove the page does not even ask.
//
// Real browser, the Rust fixture's headers and CSP, a mock scenario of the hub; each journey ends in `finish`.
import type { Page } from "@playwright/test";
import { callsTo, DIST, expect, finish, open, test, writesOf } from "./support";
import { asRole, CORE_FILE_PAGE, goOffline, goOfflineInRealTime, NOTIFY_PATH, pill, RESTART_PATH, restartAll, APPLIED, SERVICES_TAB, signedInAs, WHO } from "./t6";

test.use({ dist: DIST });

const notify = (page: Page) => page.getByRole("button", { name: "Notify services" });
const configServerBanner = (page: Page) => page.getByRole("status").filter({ hasText: "Config-server restart needed" });

/** Hover a gated button and read the tooltip that says why it is off. */
async function reasonOf(page: Page, button: ReturnType<typeof notify>): Promise<string> {
  await button.hover();
  const tip = page.getByRole("tooltip");
  await expect(tip).toBeVisible();
  return (await tip.textContent()) ?? "";
}

test.describe("a Viewer", () => {
  test.use({ scenario: "design-notify-viewer" });

  test("sees Notify services and every Restart disabled, with the role they need in a tooltip and in the description; a press opens nothing and sends nothing", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(notify(page)).toBeVisible();
    await expect(notify(page)).toHaveAttribute("aria-disabled", "true");
    await expect(notify(page)).toHaveAccessibleDescription(/Operator/);
    expect(await reasonOf(page, notify(page))).toContain("Needs the Operator role. Ask an Admin to grant it.");
    for (const name of ["tx-infinity-core", "ui-service", "document-service"]) {
      const button = page.getByRole("button", { name: `Restart ${name}` });
      await expect(button).toHaveAttribute("aria-disabled", "true");
      await expect(button).toHaveAccessibleDescription(/Operator/);
    }
    expect(await reasonOf(page, page.getByRole("button", { name: "Restart tx-infinity-core" }))).toContain("Operator");
    await notify(page).click({ force: true });
    await page.getByRole("button", { name: "Restart tx-infinity-core" }).click({ force: true });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(300);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(writesOf(harness)).toEqual([]);
    // Auto-notify is read-only for a Viewer: it says who changes it and offers no link.
    await expect(page.getByRole("group", { name: "Auto-notify" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Change in Admin" })).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("sees the config-server banner, with its button disabled for the same reason; the offer is never read", async ({ page, harness, csp }) => {
    harness.setScenario(asRole("design-notify-config-server", "viewer", WHO));
    await open(page, harness.url, SERVICES_TAB, "sitb");
    const button = configServerBanner(page).getByRole("button", { name: "Restart config-server" });
    await expect(button).toBeVisible();
    await expect(button).toHaveAttribute("aria-disabled", "true");
    expect(await reasonOf(page, button)).toContain("Needs the Operator role");
    await button.click({ force: true });
    await page.waitForTimeout(300);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(callsTo(harness, "GET", "/api/v1/swimlanes/sitb/config-server-restart")).toEqual([]);
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("an Editor", () => {
  test("is told on the tab that restarting needs Operator, and every button that acts on services says it", async ({ page, harness, csp }) => {
    harness.setScenario(signedInAs("design-notify-services-tab", "editor"));
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(page.getByRole("status").filter({ hasText: "Restart needs Operator" })).toBeVisible();
    await expect(notify(page)).toHaveAttribute("aria-disabled", "true");
    expect(await reasonOf(page, notify(page))).toContain("Needs the Operator role");
    await expect(page.getByRole("button", { name: "Restart tx-infinity-core" })).toHaveAttribute("aria-disabled", "true");
    await finish(page, csp, harness);
  });

  test("after a save, the banner's Notify services and Restart services are disabled with the same reason, and nothing is read for the restart offer", async ({ page, harness, csp }) => {
    harness.setScenario(signedInAs("design-notify-file-saved", "editor"));
    await open(page, harness.url, CORE_FILE_PAGE, "tx-infinity-core.yml");
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await expect(page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" })).toBeFocused({ timeout: 20_000 });
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("# note\r\n");
    await page.getByRole("button", { name: "Save to sitb NFS" }).click();
    await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();
    const banner = page.getByRole("status").filter({ hasText: "Saved to sitb NFS" });
    await expect(banner).toBeVisible();
    for (const name of ["Notify services", "Restart services"]) {
      const button = banner.getByRole("button", { name });
      await expect(button).toHaveAttribute("aria-disabled", "true");
      await expect(button).toHaveAccessibleDescription(/Operator/);
      await button.click({ force: true });
    }
    await page.waitForTimeout(300);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(callsTo(harness, "GET", "/api/v1/swimlanes/sitb/restart-offer")).toEqual([]);
    expect(writesOf(harness).map((r) => `${r.method} ${r.path}`)).toEqual(["PUT /api/v1/swimlanes/sitb/files"]);
    await finish(page, csp, harness);
  });
});

test.describe("an agent that is offline", () => {
  test("turns Notify services and Restart off with the reason, and a press opens nothing", async ({ page, harness, csp }) => {
    harness.setScenario("design-swimlanes-agent-off");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(notify(page)).toHaveAttribute("aria-disabled", "true");
    expect(await reasonOf(page, notify(page))).toContain("Unavailable while the agent is offline");
    await expect(page.getByRole("button", { name: "Restart tx-infinity-core" })).toHaveAttribute("aria-disabled", "true");
    await notify(page).click({ force: true });
    await page.getByRole("button", { name: "Restart tx-infinity-core" }).click({ force: true });
    await page.waitForTimeout(250);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });
});

// Live updates paused: the stream is put out of order (the same way for each), and the page says nothing can be written until it is
// back. Each journey opens the dialog first, while the page is live, and then the stream goes down: the confirm button must follow.
test.describe("live updates paused", () => {
  async function assertOff(page: Page, confirmName: string | RegExp, scope: ReturnType<Page["getByRole"]>): Promise<void> {
    const confirm = scope.getByRole("button", { name: confirmName });
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await expect(scope).toContainText("Unavailable offline");
    await confirm.click({ force: true });
    await page.keyboard.press("Enter");
  }

  test.describe("the Restart dialog of one service (T4)", () => {
    test.use({ scenario: "design-notify-services-tab" });

    test("the confirm button of a dialog that was open when the stream went down turns off, says why, and a press sends nothing; Cancel still closes", async ({ page, harness, csp }) => {
      await page.clock.install();
      await open(page, harness.url, SERVICES_TAB, "sitb");
      await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
      const ask = page.getByRole("alertdialog", { name: "Restart tx-infinity-core in sitb?" });
      await expect(ask.getByRole("button", { name: "Restart tx-infinity-core" })).not.toHaveAttribute("aria-disabled", "true");
      await goOffline(page, harness);
      await assertOff(page, "Restart tx-infinity-core", ask);
      expect(writesOf(harness)).toEqual([]);
      await ask.getByRole("button", { name: "Cancel" }).click();
      await expect(ask).toHaveCount(0);
      await expect(pill(page)).toHaveAttribute("data-status", "offline");
      await finish(page, csp, harness, { expected: [503] });
    });
  });

  test.describe("Notify services", () => {
    test.use({ scenario: "design-notify-services-tab" });

    test("the confirm button turns off with the stream, says why, and a press sends nothing; the button that opens the dialog is off too", async ({ page, harness, csp }) => {
      await page.clock.install();
      await open(page, harness.url, SERVICES_TAB, "sitb");
      await notify(page).click();
      const ask = page.getByRole("alertdialog", { name: "Notify services in sitb?" });
      await expect(ask.getByRole("button", { name: "Notify services" })).not.toHaveAttribute("aria-disabled", "true");
      await goOffline(page, harness);
      await assertOff(page, "Notify services", ask);
      expect(callsTo(harness, "POST", NOTIFY_PATH)).toEqual([]);
      await ask.getByRole("button", { name: "Cancel" }).click();
      await expect(notify(page)).toHaveAttribute("aria-disabled", "true");
      expect(await reasonOf(page, notify(page))).toContain("Unavailable offline");
      await finish(page, csp, harness, { expected: [503] });
    });
  });

  test.describe("the config-server restart", () => {
    test.use({ scenario: "design-notify-config-server" });

    test("the confirm button stays off even with the name typed, says why, and a press sends nothing", async ({ page, harness, csp }) => {
      await page.clock.install();
      await open(page, harness.url, SERVICES_TAB, "sitb");
      await configServerBanner(page).getByRole("button", { name: "Restart config-server" }).click();
      const ask = page.getByRole("alertdialog", { name: "Restart csp-configuration-server in sitb?" });
      await ask.getByRole("textbox", { name: "Type csp-configuration-server to confirm" }).fill("csp-configuration-server");
      await expect(ask.getByRole("button", { name: "Restart csp-configuration-server" })).not.toHaveAttribute("aria-disabled", "true");
      await goOffline(page, harness);
      await assertOff(page, "Restart csp-configuration-server", ask);
      expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
      await finish(page, csp, harness, { expected: [503] });
    });
  });

  test.describe("restart services after a save", () => {
    test.use({ scenario: "design-notify-file-saved" });

    test("the confirm button stays off even with the name typed, says why, and no restart is sent", async ({ page, harness, csp }) => {
      harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core"], [APPLIED]));
      await open(page, harness.url, CORE_FILE_PAGE, "tx-infinity-core.yml");
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await expect(page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" })).toBeFocused({ timeout: 20_000 });
      await page.keyboard.press("Control+Home");
      await page.keyboard.type("# note\r\n");
      await page.getByRole("button", { name: "Save to sitb NFS" }).click();
      await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();
      await page.getByRole("status").filter({ hasText: "Saved to sitb NFS" }).getByRole("button", { name: "Restart services" }).click();
      const ask = page.getByRole("alertdialog", { name: "Restart 2 services in sitb?" });
      await ask.getByRole("textbox", { name: "Type sitb to confirm" }).fill("sitb");
      await expect(ask.getByRole("button", { name: "Restart 2 services" })).not.toHaveAttribute("aria-disabled", "true");
      await goOfflineInRealTime(page, harness);
      await assertOff(page, "Restart 2 services", ask);
      expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
      await finish(page, csp, harness, { expected: [503] });
    });
  });
});
