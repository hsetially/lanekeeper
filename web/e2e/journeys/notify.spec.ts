// Notify services (T6, slice B) on the Services tab of a swimlane, as a person walks it: the dialog says what will be announced and
// that it is audited, sends nothing until it is confirmed, sends one request, and tells the hub's answer and nothing more (a count and
// an audit event: the contract has no result for each service, and the page never makes one up). Sent for approval, partly accepted,
// refused because the swimlane is locked, and a server error, each as that and not as success. A button that a person cannot press
// is disabled with the reason, never hidden, and nothing it would have sent is sent.
//
// Real browser, the Rust fixture's headers and CSP, a mock scenario of the hub; each journey ends in `finish`.
import { callsTo, DIST, expect, finish, open, test, writesOf } from "./support";
import { derive, responseOf } from "./scenarios";
import { acceptDroppedConnections, holdRequest, NOTIFY_PATH, SERVICES_TAB, serviceRow, chipOf, signedInAs, UNAVAILABLE } from "./t6";

test.use({ dist: DIST });

const notifyButton = (page: import("@playwright/test").Page) => page.getByRole("button", { name: "Notify services" });
const dialog = (page: import("@playwright/test").Page) => page.getByRole("alertdialog", { name: "Notify services in sitb?" });
const result = (page: import("@playwright/test").Page, name: string) => page.getByRole("dialog", { name });
const FILES = ["application.properties", "tx-infinity-api/tx-infinity-core-sit7.yml"];

test.describe("Notify services on the Services tab", () => {
  test.use({ scenario: "design-notify-services-tab" });

  test("says what will be announced first, sends nothing until confirmed, then sends ONE request and tells the count and the audit event (no per-service result); Close puts focus back on the button", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(serviceRow(page, "tx-infinity-core")).toBeVisible();
    await notifyButton(page).click();
    const ask = dialog(page);
    await expect(ask).toBeVisible();
    // The consequence comes first, and the files and services are what the hub says wait.
    await expect(ask).toContainText("Tell the config-server that these 2 files changed in sitb.");
    await expect(ask).toContainText("It is recorded in the audit trail, and nothing is restarted.");
    await expect(ask.getByRole("region", { name: "2 files to announce" })).toContainText("application.properties");
    await expect(ask.getByRole("region", { name: "2 files to announce" })).toContainText("tx-infinity-core-sit7.yml");
    const waiting = ask.getByRole("region", { name: "2 services waiting on these files" });
    await expect(waiting).toContainText("sitb/tx-infinity-core");
    await expect(waiting).toContainText("notifications reported on");
    await expect(waiting).toContainText("sitb/ui-service");
    await expect(waiting).toContainText("notifications reported off");
    // Nothing has been sent, and the dialog does not decide what the flag means.
    expect(writesOf(harness)).toEqual([]);
    await expect(ask).not.toContainText(/subscribe|ignored|will not be told/i);

    await ask.getByRole("button", { name: "Notify services" }).click();
    const done = result(page, "Notification sent");
    await expect(done).toBeVisible();
    await expect(ask).toHaveCount(0);
    const calls = callsTo(harness, "POST", NOTIFY_PATH);
    expect(calls, "one request, however it was reached").toHaveLength(1);
    expect(calls[0]).toMatchObject({ csrf: "ok", status: 200 });
    expect(calls[0]?.idempotencyKey).toBeTruthy();
    const body = JSON.parse(calls[0]?.body ?? "{}") as { paths: string[] };
    expect([...body.paths].sort()).toEqual(FILES);

    // The answer is a count and an audit event. The page does not say which service took it, or that anything is back: it cannot know.
    await expect(done).toContainText("The hub accepted 2 paths for sitb.");
    await expect(done).toContainText("Audit event");
    await expect(done).toContainText("41");
    await expect(done).toContainText("does not say which services picked the change up");
    await expect(done.getByRole("list")).toHaveCount(0);
    await expect(done).not.toContainText(/tx-infinity-core|ui-service|restarted|is back|picked up by/i);
    // Reading the result is a dialog the person reads: focus is on it, and Close is next.
    await expect(done).toBeFocused();
    await done.getByRole("button", { name: "Close" }).click();
    await expect(done).toHaveCount(0);
    await expect(notifyButton(page)).toBeFocused();
    // Never optimistic: the services are as they were, until the agent says otherwise.
    await expect(chipOf(serviceRow(page, "tx-infinity-core"))).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    await expect(chipOf(serviceRow(page, "ui-service"))).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    await finish(page, csp, harness);
  });

  test("Cancel sends nothing, closes, and puts focus back on the button that opened it", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    await expect(dialog(page)).toBeVisible();
    await dialog(page).getByRole("button", { name: "Cancel" }).click();
    await expect(dialog(page)).toHaveCount(0);
    await expect(notifyButton(page)).toBeFocused();
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });

  test("a count lower than what was sent is said as partial: the hub's answer does not say which, and the page does not guess", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-partial");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    await dialog(page).getByRole("button", { name: "Notify services" }).click();
    const done = result(page, "Notification partly accepted");
    await expect(done).toBeVisible();
    await expect(done).toContainText("The hub accepted 1 of 2 paths for sitb. Its answer does not say which paths it left out or why.");
    await expect(done).toContainText("Nothing was sent again.");
    await expect(done).toContainText("42");
    // No path is named as accepted or left out.
    await expect(done).not.toContainText(/application\.properties|tx-infinity-core-sit7/);
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    await done.getByRole("button", { name: "Close" }).click();
    await expect(notifyButton(page)).toBeFocused();
    await finish(page, csp, harness);
  });

  test("sent for approval (200, proposal_created): nothing was sent to the config-server, the request is named, and an Admin can open it", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-approval");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    await dialog(page).getByRole("button", { name: "Notify services" }).click();
    const sent = result(page, "Notification sent for approval");
    await expect(sent).toBeVisible();
    await expect(sent).toContainText("Nothing is sent to the config-server until an Admin approves request AP-2043.");
    await expect(sent.getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/2043");
    await expect(sent).not.toContainText(/hub accepted/i);
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    await sent.getByRole("button", { name: "Close" }).click();
    await expect(notifyButton(page)).toBeFocused();
    await finish(page, csp, harness);
  });

  test("an Operator whose changes need approval is told so before sending, and gets no link to a request they cannot open", async ({ page, harness, csp }) => {
    harness.setScenario(signedInAs("design-notify-approval", "operator", { requires_approval: true }));
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    const ask = dialog(page);
    await expect(ask).toContainText("Ask an Admin to tell the config-server that these 2 files changed in sitb. Nothing is sent until the request is approved.");
    await expect(ask).toContainText("Your changes need approval, so this goes to an Admin as a request.");
    await ask.getByRole("button", { name: "Notify services" }).click();
    const sent = result(page, "Notification sent for approval");
    await expect(sent).toBeVisible();
    await expect(sent.getByRole("link", { name: "View request" })).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("is refused while the swimlane is locked (423): the dialog stays with the cause and the request ID, nothing is sent again, and Cancel leaves", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-locked");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    const ask = dialog(page);
    await ask.getByRole("button", { name: "Notify services" }).click();
    await expect(ask.getByRole("alert")).toBeVisible();
    await expect(ask).toContainText("req-mock-0001");
    await expect(page.getByRole("dialog", { name: /Notification/ })).toHaveCount(0);
    expect(callsTo(harness, "POST", NOTIFY_PATH).map((r) => r.status)).toEqual([423]);
    // The page does not try again by itself.
    await page.waitForTimeout(1_200);
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    await ask.getByRole("button", { name: "Cancel" }).click();
    await expect(ask).toHaveCount(0);
    await expect(notifyButton(page)).toBeFocused();
    await finish(page, csp, harness, { expected: [423] });
  });

  test("a server error (503) stays in the dialog with the request ID and is NOT sent again by the page; pressing the button again is the person's decision", async ({ page, harness, csp }) => {
    const base = "design-notify-services-tab";
    const notify = responseOf(base, "POST", "/api/v1/swimlanes/{id}/notify");
    harness.setScenario(derive(base, [{ ...notify, status: UNAVAILABLE.status, body: UNAVAILABLE.body }], "journey-notify-503"));
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    const ask = dialog(page);
    await ask.getByRole("button", { name: "Notify services" }).click();
    await expect(ask.getByRole("alert")).toBeVisible();
    await expect(ask).toContainText("req-mock-0001");
    await page.waitForTimeout(1_500);
    expect(callsTo(harness, "POST", NOTIFY_PATH).map((r) => r.status), "one request, no retry").toEqual([503]);
    // The dialog is as it was: the same files, the button works again, and a second press is a second, deliberate request.
    harness.setScenario(base);
    await ask.getByRole("button", { name: "Notify services" }).click();
    await expect(result(page, "Notification sent")).toBeVisible();
    expect(callsTo(harness, "POST", NOTIFY_PATH).map((r) => r.status)).toEqual([503, 200]);
    await finish(page, csp, harness, { expected: [503] });
  });

  test("the button is off while the request is out: a second press sends nothing, and the dialog does not close", async ({ page, harness, csp }) => {
    const held = await holdRequest(page, /\/api\/v1\/swimlanes\/sitb\/notify$/);
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    const ask = dialog(page);
    await ask.getByRole("button", { name: "Notify services" }).click();
    await held.reached;
    await expect(ask.getByRole("button", { name: "Notifying…" })).toBeVisible();
    await expect(ask.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await ask.getByRole("button", { name: "Notifying…" }).click({ force: true });
    await page.keyboard.press("Escape");
    await expect(ask).toBeVisible();
    expect(held.count()).toBe(1);
    await held.release();
    await expect(result(page, "Notification sent")).toBeVisible();
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toHaveLength(1);
    await finish(page, csp, harness);
  });
});

test.describe("a notification that gets no answer", () => {
  test.use({ scenario: "design-notify-services-tab" });

  /** Fail the first `fail` attempts at the notify address in the browser, as a dropped connection does, and record every attempt's key. */
  async function dropAttempts(page: import("@playwright/test").Page, fail: number): Promise<{ attempts(): string[] }> {
    const keys: string[] = [];
    await page.route(/\/api\/v1\/swimlanes\/sitb\/notify$/, async (route) => {
      if (route.request().method() !== "POST") {
        await route.fallback();
        return;
      }
      keys.push(route.request().headers()["idempotency-key"] ?? "");
      if (keys.length <= fail) await route.abort("connectionreset");
      else await route.fallback();
    });
    return { attempts: () => [...keys] };
  }

  test("is sent once more by the transport, with the SAME idempotency key (the hub does it once); nothing else retries it", async ({ page, harness, csp }) => {
    const seen = await dropAttempts(page, 1);
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    await dialog(page).getByRole("button", { name: "Notify services" }).click();
    await expect(result(page, "Notification sent")).toBeVisible();
    const keys = seen.attempts();
    expect(keys, "two attempts: the one that was lost and the one transport retry").toHaveLength(2);
    expect(keys[0]).toBeTruthy();
    expect(keys[1]).toBe(keys[0]);
    const calls = callsTo(harness, "POST", NOTIFY_PATH);
    expect(calls, "the hub saw the retry only").toHaveLength(1);
    expect(calls[0]?.idempotencyKey).toBe(keys[0]);
    acceptDroppedConnections(csp, 1);
    await finish(page, csp, harness);
  });

  test("when the retry is lost too the dialog says the server could not be reached, with nothing sent to the hub, and the page tries no third time", async ({ page, harness, csp }) => {
    const seen = await dropAttempts(page, 5);
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    const ask = dialog(page);
    await ask.getByRole("button", { name: "Notify services" }).click();
    await expect(ask.getByRole("alert")).toBeVisible();
    await expect(ask.getByRole("alert")).toContainText("Couldn't reach the server");
    await expect(ask.getByRole("alert")).toContainText("The notification may or may not have been sent. Check the services before you try again.");
    await page.waitForTimeout(1_500);
    expect(seen.attempts(), "the original and one retry, never a third").toHaveLength(2);
    expect(callsTo(harness, "POST", NOTIFY_PATH)).toEqual([]);
    await expect(ask).toBeVisible();
    acceptDroppedConnections(csp, 2);
    await finish(page, csp, harness);
  });
});
