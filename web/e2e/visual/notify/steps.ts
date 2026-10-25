// What the frames of the notify and restart pieces do to a page that more than one of them does: the places on the Services tab and in the
// file page's banner, the dialogs they open, the file saved through the real editor, and the answers a frame changes (a refusal, a held
// request, the swimlane as another person or another state of its agent has it).
import type { Locator, Page, Route } from "@playwright/test";
import type { Role, ServiceState } from "../../../src/api/generated/model/index.ts";
import { expect } from "../../fixtures/test";
import { api, rewrite, signedInAs } from "../access/lib/steps";
import type { Step } from "../pickup/runner";
import { failRequests } from "../search/steps";

export const TAB = "/swimlanes/sitb/services";
const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
export const FILE = `/swimlanes/sitb/files?path=${CORE}`;
export const CONFIG_SERVER = "csp-configuration-server";

export const NOTIFY = api("/swimlanes/sitb/notify");
export const RESTART = api("/swimlanes/sitb/restart");
export const RESTART_OFFER = api("/swimlanes/sitb/restart-offer");
export const CONFIG_SERVER_OFFER = api("/swimlanes/sitb/config-server-restart");
export const SWIMLANE = api("/swimlanes/sitb");

// ----- places ----------------------------------------------------------------------------------------------------------------------

/** The strip above the services: the filter, the count and Notify services. */
export const strip = (page: Page): Locator => page.locator("[data-slot='services-strip']");
export const notifyButton = (page: Page): Locator => strip(page).getByRole("button", { name: "Notify services" });
/** The warning that says a config-server restart is needed, and its button. */
export const configServerBanner = (page: Page): Locator => page.getByRole("status").filter({ hasText: "Config-server restart needed" });
export const configServerButton = (page: Page): Locator => configServerBanner(page).getByRole("button", { name: "Restart config-server" });
export const restartRow = (page: Page, service: string): Locator => page.getByRole("button", { name: `Restart ${service}` });
/** The banner of the file page after a write, and what it offers. */
export const saved = (page: Page): Locator => page.getByRole("status").filter({ hasText: "Saved to sitb NFS" });
export const bannerNotify = (page: Page): Locator => saved(page).getByRole("button", { name: "Notify services" });
export const bannerRestart = (page: Page): Locator => saved(page).getByRole("button", { name: "Restart services" });
export const bannerConfigServer = (page: Page): Locator => saved(page).getByRole("button", { name: "Restart config-server" });

/** The dialogs. A confirmation is an alertdialog; what happened afterwards is a dialog. */
export const asking = (page: Page): Locator => page.getByRole("alertdialog", { name: "Notify services in sitb?" });
export const confirmConfigServer = (page: Page): Locator => page.getByRole("alertdialog", { name: `Restart ${CONFIG_SERVER} in sitb?` });
export const confirmRestartFile = (page: Page, n: number): Locator => page.getByRole("alertdialog", { name: `Restart ${String(n)} services in sitb?` });
export const confirmRestartService = (page: Page, service: string): Locator => page.getByRole("alertdialog", { name: `Restart ${service} in sitb?` });
export const dialog = (page: Page, name: string | RegExp): Locator => page.getByRole("dialog", { name });

// ----- steps -----------------------------------------------------------------------------------------------------------------------

/** Press a button and wait for the dialog it opens. */
export const press = (button: (page: Page) => Locator, opens: (page: Page) => Locator): Step => async ({ page }) => {
  await button(page).click();
  await expect(opens(page)).toBeVisible();
};

export const openNotify: Step = press(notifyButton, asking);
export const openNotifyFromBanner: Step = press(bannerNotify, asking);
export const openConfigServer: Step = press(configServerButton, confirmConfigServer);

/** Confirm what a dialog asks, with the button that names the action (inside the dialog: the page behind it has a button of the same name). */
export const confirmIn = (dialogOf: (page: Page) => Locator, action: string | RegExp): Step => async ({ page }) => {
  await dialogOf(page).getByRole("button", { name: action }).click();
};

/** Type the name that a type-to-confirm dialog asks for. Enter never confirms it: this presses nothing. */
export const typeName = (dialogOf: (page: Page) => Locator, name: string): Step => async ({ page }) => {
  await dialogOf(page).getByRole("textbox", { name: new RegExp(`Type ${name} to confirm`) }).fill(name);
  await expect(dialogOf(page).getByRole("button", { name: /^Restart / })).not.toHaveAttribute("aria-disabled", "true");
};

/** Edit the core file in the real editor and save it, so that the banner of the file page is on screen. */
export const saveCoreFile: Step = async ({ page }) => {
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(page.getByText("Editing NFS")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" })).toBeFocused({ timeout: 20_000 });
  await page.keyboard.press("Control+Home");
  await page.keyboard.type("# note\r\n");
  await page.getByRole("button", { name: "Save to sitb NFS" }).click();
  await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();
  await expect(saved(page)).toBeVisible();
};

/** Put the pointer on a control and wait for its tooltip. */
export const hover = (target: (page: Page) => Locator): Step => async ({ page }) => {
  await target(page).hover();
  await page.getByRole("tooltip").first().waitFor();
};

export const tipSays = (...words: Array<string | RegExp>): Step => async ({ page }) => {
  for (const word of words) await expect(page.getByRole("tooltip").first()).toContainText(word);
};

// ----- what a frame changes about the hub's answers ---------------------------------------------------------------------------------

const PROBLEM_CODES: Record<number, string> = { 403: "forbidden", 404: "not_found", 409: "conflict", 422: "validation_failed", 423: "locked", 429: "rate_limited", 500: "internal", 503: "service_unavailable" };

/** Answer the writes to an address with a problem, the way the hub does (a request id, and a `retry-after` where the status has one). */
export const refuse = (address: RegExp, status: number, title: string, retryAfter?: number): Step => async ({ page }) => {
  await page.route(address, (route) => {
    if (route.request().method() === "GET") return route.fallback();
    return route.fulfill({
      status,
      contentType: "application/problem+json",
      headers: { "x-request-id": "req-mock-0001", ...(retryAfter === undefined ? {} : { "retry-after": String(retryAfter) }) },
      body: JSON.stringify({ type: "about:blank", title, status, code: PROBLEM_CODES[status] ?? "error", request_id: "req-mock-0001" }),
    });
  });
};

/** The write never gets an answer: the connection is cut, as when the network goes. */
export const cutOff = (address: RegExp): Step => async ({ page }) => {
  await page.route(address, (route: Route) => (route.request().method() === "GET" ? route.fallback() : route.abort("connectionreset")));
};

/** Answer the reads of an address with a problem (an offer that cannot be read). */
export const unreadable = (address: RegExp, status: number, title: string): Step => async ({ page }) => {
  await failRequests(page, address, status, title);
};

/** Never answer the reads of an address: the dialog that waits for the offer stays on its skeleton (held in the browser, so the hub is never asked). */
export const held = (address: RegExp): Step => async ({ page }) => {
  await page.route(address, (route) => (route.request().method() === "GET" ? undefined : route.fallback()));
};

/** The swimlane's agent is not connected. */
export const agentOffline: Step = async ({ page }) => {
  await rewrite(page, SWIMLANE, (body) => ({ ...body, agent: "disconnected" }));
};

/** A person with this role is signed in (the scenario's own is an Admin). */
export const as = (role: Role): Step => async ({ page }) => {
  await signedInAs(page, role);
};

/** Change the services of sitb as the hub reports them. */
export const servicesAre = (change: (services: ServiceState[]) => ServiceState[], counts?: Array<[string, number]>): Step => async ({ page }) => {
  await rewrite(page, SWIMLANE, (body) => ({ ...body, services: change(body["services"] as ServiceState[]), ...(counts === undefined ? {} : { pickup_counts: counts }) }));
};

/** Replace what the restart-all offer lists. */
export const offerIs = (offer: { services: Array<{ namespace: string; name: string }>; truncated: boolean }): Step => async ({ page }) => {
  await rewrite(page, RESTART_OFFER, () => offer);
};

/** The live stream goes down and the shell says so: every write is off, and its dialogs follow. */
export const streamDown: Step = async ({ page, harness }) => {
  harness.refuseStreams(503, 99, { retryAfter: 1 });
  harness.closeStreams();
  await expect(page.getByText("Live updates paused, retrying")).toBeVisible({ timeout: 30_000 });
};
