// The config-server restart that finding C11 asks for (T6, slice B), as a person walks it from the Services tab: the banner says why,
// the dialog shows the hub's own offer (the one Deployment, and the services that cannot be served while it restarts) and asks for
// the Deployment's name to be typed, the request is one audited `restartService`, and the answer is "accepted" with no progress
// (the contract has no restarting state). With no offer the dialog says the hub offered none and offers nothing to confirm.
//
// Real browser, the Rust fixture's headers and CSP, a mock scenario of the hub; each journey ends in `finish`.
import type { Page } from "@playwright/test";
import { callsTo, DIST, expect, finish, open, test, writesOf } from "./support";
import { derive, responseOf } from "./scenarios";
import { CONFIG_SERVER_OFFER_PATH, holdRequest, RESTART_PATH, SERVICES_TAB, UNAVAILABLE } from "./t6";

test.use({ dist: DIST });

const NAME = "csp-configuration-server";
const banner = (page: Page) => page.getByRole("status").filter({ hasText: "Config-server restart needed" });
const opener = (page: Page) => banner(page).getByRole("button", { name: "Restart config-server" });
const confirm = (page: Page) => page.getByRole("alertdialog", { name: `Restart ${NAME} in sitb?` });
const nameField = (page: Page) => confirm(page).getByRole("textbox", { name: `Type ${NAME} to confirm` });
const doIt = (page: Page) => confirm(page).getByRole("button", { name: `Restart ${NAME}` });

test.describe("the config-server restart (check C11)", () => {
  test.use({ scenario: "design-notify-config-server" });

  test("the banner says why, the dialog shows the hub's offer, the name is typed, ONE request is sent and the answer is 'accepted' with no progress; Close returns focus to the banner's button", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(banner(page)).toBeVisible();
    await expect(banner(page)).toContainText("Check C11 is open: a new folder or a change to channels.yml is not served until the config-server restarts.");
    await expect(banner(page)).toContainText("2 services report needing a config-server restart.");
    // Nothing is read for the offer until the button is pressed.
    expect(callsTo(harness, "GET", CONFIG_SERVER_OFFER_PATH)).toEqual([]);
    await opener(page).click();
    await expect(confirm(page)).toBeVisible();
    expect(callsTo(harness, "GET", CONFIG_SERVER_OFFER_PATH)).toHaveLength(1);
    await expect(confirm(page)).toContainText("Restarting the config-server replaces its pods.");
    await expect(confirm(page)).toContainText("It is recorded in the audit trail.");
    await expect(confirm(page).getByRole("region", { name: "4 other services affected" })).toContainText("sitb/tx-infinity-core");
    await expect(confirm(page)).toContainText("Other people testing in sitb will be interrupted.");
    await expect(confirm(page)).not.toContainText("not the whole list");
    // The name is typed first: a press before it matches sends nothing.
    await doIt(page).click({ force: true });
    await nameField(page).fill("csp-configuration");
    await doIt(page).click({ force: true });
    expect(writesOf(harness)).toEqual([]);
    await nameField(page).fill(NAME);
    await doIt(page).click();

    const done = page.getByRole("dialog", { name: "Restart requested" });
    await expect(done).toBeVisible();
    await expect(done).toContainText(`The hub accepted the restart of ${NAME} in sitb.`);
    await expect(done).toContainText("Audit event");
    await expect(done).toContainText("77");
    await expect(done).toContainText("The hub reports no restart progress.");
    // No progress is drawn, and nothing says the config-server is back.
    await expect(done.getByRole("progressbar")).toHaveCount(0);
    await expect(done).not.toContainText(/is back|restarted successfully|ready again|restarting\b/i);
    const calls = callsTo(harness, "POST", RESTART_PATH);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ csrf: "ok", status: 200 });
    expect(calls[0]?.idempotencyKey).toBeTruthy();
    expect(JSON.parse(calls[0]?.body ?? "{}")).toEqual({ namespace: "sitb", name: NAME });
    // The offer is not read again once the person has confirmed it: what they confirmed is what was restarted.
    expect(callsTo(harness, "GET", CONFIG_SERVER_OFFER_PATH)).toHaveLength(1);
    await done.getByRole("button", { name: "Close" }).click();
    await expect(done).toHaveCount(0);
    await expect(opener(page)).toBeFocused();
    // Never optimistic: the banner and the services are as they were.
    await expect(banner(page)).toBeVisible();
    await finish(page, csp, harness);
  });

  test("a refusal (423) stays in the dialog with the typed name intact and the request ID; nothing is sent again", async ({ page, harness, csp }) => {
    const base = "design-notify-config-server";
    const restart = responseOf(base, "POST", "/api/v1/swimlanes/{id}/restart");
    harness.setScenario(derive(base, [{ ...restart, status: 423, body: { type: "urn:lanekeeper:problem:locked", title: "Locked", status: 423, request_id: "req-mock-0001", until: null } }], "journey-cs-locked"));
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await opener(page).click();
    await nameField(page).fill(NAME);
    await doIt(page).click();
    await expect(confirm(page).getByRole("alert")).toBeVisible();
    await expect(confirm(page)).toContainText("req-mock-0001");
    await expect(nameField(page)).toHaveValue(NAME);
    await page.waitForTimeout(1_200);
    expect(callsTo(harness, "POST", RESTART_PATH).map((r) => r.status)).toEqual([423]);
    await confirm(page).getByRole("button", { name: "Cancel" }).click();
    await expect(opener(page)).toBeFocused();
    await finish(page, csp, harness, { expected: [423] });
  });

  test("while the request is out the dialog is locked: Escape and Cancel do nothing, and a second press sends nothing", async ({ page, harness, csp }) => {
    const held = await holdRequest(page, /\/api\/v1\/swimlanes\/sitb\/restart$/);
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await opener(page).click();
    await nameField(page).fill(NAME);
    await doIt(page).click();
    await held.reached;
    await expect(confirm(page).getByRole("button", { name: "Restarting…" })).toBeVisible();
    await expect(confirm(page).getByRole("button", { name: "Cancel" })).toBeDisabled();
    await expect(nameField(page)).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(confirm(page)).toBeVisible();
    await confirm(page).getByRole("button", { name: "Restarting…" }).click({ force: true });
    expect(held.count()).toBe(1);
    await held.release();
    await expect(page.getByRole("dialog", { name: "Restart requested" })).toBeVisible();
    expect(callsTo(harness, "POST", RESTART_PATH)).toHaveLength(1);
    await finish(page, csp, harness);
  });

  test("an offer that cannot be read (503) says so with the request ID and offers Retry, never a confirm button; Retry reads it again", async ({ page, harness, csp }) => {
    const base = "design-notify-config-server";
    const offer = responseOf(base, "GET", "/api/v1/swimlanes/{id}/config-server-restart");
    harness.setScenario(derive(base, [{ ...offer, status: UNAVAILABLE.status, body: UNAVAILABLE.body }], "journey-cs-offer-503"));
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await opener(page).click();
    const failed = page.getByRole("dialog").filter({ has: page.getByRole("button", { name: "Retry" }) });
    await expect(failed).toBeVisible({ timeout: 15_000 });
    await expect(failed).toContainText("req-mock-0001");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(writesOf(harness)).toEqual([]);
    harness.setScenario(base);
    const reads = callsTo(harness, "GET", CONFIG_SERVER_OFFER_PATH).length;
    await failed.getByRole("button", { name: "Retry" }).click();
    await expect(confirm(page)).toBeVisible();
    expect(callsTo(harness, "GET", CONFIG_SERVER_OFFER_PATH).length).toBeGreaterThan(reads);
    await finish(page, csp, harness, { expected: [503] });
  });
});

test.describe("the config-server restart, other answers of the hub", () => {
  test("a list of affected services that is not the whole of them says so, in words", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-config-server-truncated");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await opener(page).click();
    await expect(confirm(page).getByRole("region", { name: "4 other services affected (not the whole list)" })).toBeVisible();
    await expect(confirm(page)).toContainText("The swimlane has more services than this list shows, so the list is not the whole of the services affected.");
    await finish(page, csp, harness);
  });

  test("with no offer the dialog says the hub offered none, and why it cannot tell which condition is missing; nothing can be confirmed", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-config-server-none");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await opener(page).click();
    const none = page.getByRole("dialog", { name: "No config-server restart is offered" });
    await expect(none).toBeVisible();
    await expect(none).toContainText("only while check C11 is open for it and the cluster report lists exactly one config-server Deployment");
    await expect(none).toContainText("Its answer does not say which of the two is missing.");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(none.getByRole("button", { name: /^Restart/ })).toHaveCount(0);
    await none.getByRole("button", { name: "Close" }).click();
    await expect(opener(page)).toBeFocused();
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });

  test("is sent for approval (200, proposal_created): nothing restarts, the request is named, an Admin can open it", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-config-server-approval");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await opener(page).click();
    await nameField(page).fill(NAME);
    await doIt(page).click();
    const sent = page.getByRole("dialog", { name: "Config-server restart sent for approval" });
    await expect(sent).toBeVisible();
    await expect(sent).toContainText("Nothing restarts until an Admin approves request AP-2044.");
    await expect(sent.getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/2044");
    await expect(sent).not.toContainText(/hub accepted the restart/i);
    expect(callsTo(harness, "POST", RESTART_PATH)).toHaveLength(1);
    await sent.getByRole("button", { name: "Close" }).click();
    await expect(opener(page)).toBeFocused();
    await finish(page, csp, harness);
  });

  test("a swimlane where no restart is needed has no banner", async ({ page, harness, csp }) => {
    harness.setScenario("design-notify-services-tab");
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(page.getByRole("button", { name: "Notify services" })).toBeVisible();
    await expect(banner(page)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Restart config-server" })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});
