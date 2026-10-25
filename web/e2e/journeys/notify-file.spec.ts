// The file page after a write (T6, slice B): the "Saved" banner offers what the file's pickup state calls for. A file that waits for a
// notify or a restart offers Notify services (this file) and Restart services (the services the hub lists as reading it, restarted
// ONE REQUEST AT A TIME, stopping at the first the hub refuses and saying which were not sent); a file that waits for the
// config-server offers Restart config-server; a file the services already have offers nothing.
//
// The writes go through the real editor (Monaco, under the production CSP), then the dialogs of the banner. Each journey ends in `finish`.
import type { Page } from "@playwright/test";
import { callsTo, DIST, expect, finish, open, test, writesOf } from "./support";
import { APPLIED, CORE_FILE, CORE_FILE_PAGE, holdRequest, LOCKED, NOTIFY_PATH, RESTART_OFFER_PATH, RESTART_PATH, restartAll, settled } from "./t6";

test.use({ dist: DIST });

const editorBox = (page: Page) => page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" });
const banner = (page: Page) => page.getByRole("status").filter({ hasText: "Saved to sitb NFS" });
const notify = (page: Page) => banner(page).getByRole("button", { name: "Notify services" });
const restart = (page: Page) => banner(page).getByRole("button", { name: "Restart services" });
const confirmRestart = (page: Page, count: number) => page.getByRole("alertdialog", { name: `Restart ${count} services in sitb?` });

/** Edit the file and save it, so that the banner is on the page. */
async function saveCoreFile(page: Page, origin: string): Promise<void> {
  await open(page, origin, CORE_FILE_PAGE, "tx-infinity-core.yml");
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(page.getByText("Editing NFS")).toBeVisible();
  await expect(editorBox(page)).toBeFocused({ timeout: 20_000 });
  await page.keyboard.press("Control+Home");
  await page.keyboard.type("# note\r\n");
  await page.getByRole("button", { name: "Save to sitb NFS" }).click();
  await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();
  await expect(banner(page)).toBeVisible();
}

test.describe("after a save, a file that waits for a notify or a restart", () => {
  test.use({ scenario: "design-notify-file-saved" });

  test("offers Notify services for THIS file: the dialog names it, one request carries it, and the result is a count and an audit event; Close returns focus to the banner's button", async ({ page, harness, csp }) => {
    await saveCoreFile(page, harness.url);
    await expect(banner(page)).toContainText("Services pick it up after a notify or a restart.");
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toEqual([]);
    await notify(page).click();
    const ask = page.getByRole("alertdialog", { name: "Notify services in sitb?" });
    await expect(ask).toBeVisible();
    await expect(ask).toContainText("Tell the config-server that this file changed in sitb.");
    await expect(ask.getByRole("region", { name: "1 file to announce" })).toContainText(CORE_FILE);
    await expect(ask.getByRole("region", { name: "2 services waiting on this file" })).toContainText("sitb/ui-service");
    await ask.getByRole("button", { name: "Notify services" }).click();
    const done = page.getByRole("dialog", { name: "Notification sent" });
    await expect(done).toBeVisible();
    await expect(done).toContainText("The hub accepted 1 path for sitb.");
    await expect(done).toContainText("41");
    const calls = callsTo(harness, "POST", NOTIFY_PATH);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ csrf: "ok", status: 200 });
    expect(JSON.parse(calls[0]?.body ?? "{}")).toEqual({ paths: [CORE_FILE] });
    await done.getByRole("button", { name: "Close" }).click();
    await expect(done).toHaveCount(0);
    await expect(notify(page)).toBeFocused();
    // The banner still stands: what the services have picked up is the agent's to say.
    await expect(banner(page)).toBeVisible();
    // The writes of the whole journey: the save, then the one notification.
    expect(writesOf(harness).map((r) => `${r.method} ${r.path}`)).toEqual(["PUT /api/v1/swimlanes/sitb/files", `POST ${NOTIFY_PATH}`]);
    await finish(page, csp, harness);
  });

  test("is refused while the swimlane is locked (423): the dialog stays with the request ID and nothing else is sent", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-file-locked");
    await saveCoreFile(page, harness.url);
    await notify(page).click();
    const ask = page.getByRole("alertdialog", { name: "Notify services in sitb?" });
    await ask.getByRole("button", { name: "Notify services" }).click();
    await expect(ask.getByRole("alert")).toBeVisible();
    await expect(ask).toContainText("req-mock-0001");
    expect(callsTo(harness, "POST", NOTIFY_PATH).map((r) => r.status)).toEqual([423]);
    await ask.getByRole("button", { name: "Cancel" }).click();
    await expect(notify(page)).toBeFocused();
    await finish(page, csp, harness, { expected: [423] });
  });

  test("Restart services lists what the hub lists and asks for the swimlane's name; every service is restarted one request at a time and the result says accepted, never restarted", async ({ page, harness, csp }) => {
    harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core", "ui-service"], [APPLIED]));
    await saveCoreFile(page, harness.url);
    await restart(page).click();
    const ask = confirmRestart(page, 3);
    await expect(ask).toBeVisible();
    await expect(ask).toContainText(`which read ${CORE_FILE}`);
    await expect(ask.getByRole("region", { name: "3 services to restart" })).toContainText("sitb/ui-service");
    const button = ask.getByRole("button", { name: "Restart 3 services" });
    // The name is typed first: until it matches, a press sends nothing.
    await button.click({ force: true });
    await ask.getByRole("textbox", { name: "Type sitb to confirm" }).fill("sit");
    await button.click({ force: true });
    expect(callsTo(harness, "POST", RESTART_PATH)).toEqual([]);
    await ask.getByRole("textbox", { name: "Type sitb to confirm" }).fill("sitb");
    await button.click();
    const done = page.getByRole("dialog", { name: "Restart requested for 3 services" });
    await expect(done).toBeVisible();
    await expect(done).toContainText("The hub accepted 3 restart requests, one at a time. It reports no restart progress.");
    const steps = done.getByRole("region", { name: "Restart requests" });
    for (const name of ["tx-infinity-api", "tx-infinity-core", "ui-service"]) await expect(steps).toContainText(`sitb/${name} accepted`);
    await expect(done).not.toContainText(/restarted successfully|is back|ready again/i);
    const calls = callsTo(harness, "POST", RESTART_PATH);
    expect(calls.map((r) => r.status)).toEqual([200, 200, 200]);
    expect(calls.map((r) => (JSON.parse(r.body ?? "{}") as { namespace: string; name: string }).name)).toEqual(["tx-infinity-api", "tx-infinity-core", "ui-service"]);
    expect(new Set(calls.map((r) => r.idempotencyKey)).size, "each request has a key of its own").toBe(3);
    expect(calls.every((r) => r.csrf === "ok")).toBe(true);
    await done.getByRole("button", { name: "Close" }).click();
    await expect(restart(page)).toBeFocused();
    await finish(page, csp, harness);
  });

  test("stops at the first request the hub refuses (423) and says which were accepted, which was refused and which were NOT sent; nothing is sent again", async ({ page, harness, csp }) => {
    harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core", "ui-service"], [APPLIED, LOCKED]));
    await saveCoreFile(page, harness.url);
    await restart(page).click();
    const ask = confirmRestart(page, 3);
    await ask.getByRole("textbox", { name: "Type sitb to confirm" }).fill("sitb");
    await ask.getByRole("button", { name: "Restart 3 services" }).click();
    const stopped = page.getByRole("dialog", { name: "Restart stopped early" });
    await expect(stopped).toBeVisible();
    await expect(stopped).toContainText("The hub refused the request for sitb/tx-infinity-core. 1 earlier request was accepted and 1 was not sent.");
    await expect(stopped).toContainText("req-mock-0001");
    await expect(stopped).toContainText("Nothing was sent again.");
    const steps = stopped.getByRole("region", { name: "Restart requests" });
    await expect(steps).toContainText("sitb/tx-infinity-api accepted");
    await expect(steps).toContainText("sitb/tx-infinity-core not accepted");
    await expect(steps).toContainText("sitb/ui-service not attempted");
    await page.waitForTimeout(1_200);
    expect(callsTo(harness, "POST", RESTART_PATH).map((r) => r.status), "two requests: the third was never sent").toEqual([200, 423]);
    await stopped.getByRole("button", { name: "Close" }).click();
    await expect(restart(page)).toBeFocused();
    await finish(page, csp, harness, { expected: [423] });
  });

  test("leaving the screen stops the run: the request that is out is the last one, and the services after it are never asked for", async ({ page, harness, csp }) => {
    harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core", "ui-service"], [APPLIED]));
    // Reach the file page from the swimlane's page, so that Back is a step inside the app.
    const second = await holdRequest(page, /\/api\/v1\/swimlanes\/sitb\/restart$/, { nth: 2 });
    await open(page, harness.url, "/swimlanes/sitb", "sitb");
    await page.evaluate((href) => {
      window.history.pushState({}, "", href);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, CORE_FILE_PAGE);
    await saveInPlace(page);
    await restart(page).click();
    const ask = confirmRestart(page, 3);
    await ask.getByRole("textbox", { name: "Type sitb to confirm" }).fill("sitb");
    await ask.getByRole("button", { name: "Restart 3 services" }).click();
    await second.reached;
    await expect(ask.getByText("Request 2 of 3 sent.")).toBeVisible();
    // Escape is refused while a request is out; the person leaves the way people do, with Back.
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
    await expect(ask).toBeVisible();
    await page.goBack();
    await expect(page.getByRole("heading", { level: 1, name: "sitb" })).toBeVisible();
    await expect(ask).toHaveCount(0);
    await second.release();
    await settled(harness, 800);
    expect(callsTo(harness, "POST", RESTART_PATH).map((r) => (JSON.parse(r.body ?? "{}") as { namespace: string; name: string }).name), "the third service was never asked for").toEqual(["tx-infinity-api", "tx-infinity-core"]);
    await finish(page, csp, harness);
  });

  test("the restart-all offer is read when the dialog opens and not again once the person has confirmed it", async ({ page, harness, csp }) => {
    harness.setScenario(restartAll("design-notify-file-saved", ["tx-infinity-api", "tx-infinity-core"], [APPLIED]));
    await saveCoreFile(page, harness.url);
    const before = callsTo(harness, "GET", RESTART_OFFER_PATH).length;
    expect(before).toBe(0);
    await restart(page).click();
    const ask = confirmRestart(page, 2);
    await expect(ask).toBeVisible();
    expect(callsTo(harness, "GET", RESTART_OFFER_PATH)).toHaveLength(1);
    await ask.getByRole("textbox", { name: "Type sitb to confirm" }).fill("sitb");
    await ask.getByRole("button", { name: "Restart 2 services" }).click();
    await expect(page.getByRole("dialog", { name: "Restart requested for 2 services" })).toBeVisible();
    expect(callsTo(harness, "GET", RESTART_OFFER_PATH), "the offer was read once, before the confirmation").toHaveLength(1);
    await finish(page, csp, harness);
  });
});

/** The edit and save of `saveCoreFile`, on a page that is already on the file. */
async function saveInPlace(page: Page): Promise<void> {
  await expect(page.getByRole("heading", { level: 1, name: "tx-infinity-core.yml" })).toBeVisible();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(editorBox(page)).toBeFocused({ timeout: 20_000 });
  await page.keyboard.press("Control+Home");
  await page.keyboard.type("# note\r\n");
  await page.getByRole("button", { name: "Save to sitb NFS" }).click();
  await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();
  await expect(banner(page)).toBeVisible();
}

test.describe("after a save, a file that the config-server only serves after it restarts", () => {
  test.use({ scenario: "design-notify-file-config-server" });

  test("offers Restart config-server and nothing else: no Notify, no restart-all", async ({ page, harness, csp }) => {
    await saveCoreFile(page, harness.url);
    await expect(banner(page).getByRole("button", { name: "Restart config-server" })).toBeVisible();
    await expect(notify(page)).toHaveCount(0);
    await expect(restart(page)).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("after a save, a file the services already have", () => {
  test.use({ scenario: "design-notify-file-live" });

  test("says only that the file is the new version, and offers nothing to do", async ({ page, harness, csp }) => {
    await saveCoreFile(page, harness.url);
    await expect(banner(page)).toContainText("The file on sitb NFS is the new version.");
    await expect(banner(page)).not.toContainText("Services pick it up");
    await expect(banner(page).getByRole("button", { name: /Notify|Restart/ })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});
